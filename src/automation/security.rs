//! Stock security actions. State comes from peer messages, never toolbar toggles.
use super::{control::Permit, displays, error::{BridgeError, Result}, sessions::{self, SessionSnapshot}, wire};
use hbb_common::{message_proto::*, tokio};
use hbb_common::message_proto::option_message::BoolOption;
use serde::Serialize;
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Clone, Copy)]
pub enum Kind { Block, Privacy, Elevation }
#[derive(Clone, Debug, Default, Serialize)]
pub struct Observation {
    pub enabled: Option<bool>,
    pub outcome: Option<String>,
    pub sequence: u64,
    pub observed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub pending: bool,
    #[serde(skip)]
    pub request_token: u64,
}
impl Observation {
    fn report(&mut self, enabled: Option<bool>, outcome: &str) {
        self.enabled = enabled;
        self.outcome = Some(outcome.into());
        self.sequence += 1;
        self.observed_at = Some(chrono::Utc::now());
        self.pending = false;
    }
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct State {
    pub sas_enabled: Option<bool>,
    pub privacy_supported: Option<bool>,
    pub portable_service_running: Option<bool>,
    pub block: Observation,
    pub privacy: Observation,
    pub elevation: Observation,
    pub privacy_implementation: Option<String>,
}
pub enum Event {
    Back(BackNotification),
    Elevation(bool), // response accepted, not elevation success
    Portable(bool),
}
impl State {
    pub fn observation_mut(&mut self, kind: Kind) -> &mut Observation {
        match kind { Kind::Block => &mut self.block, Kind::Privacy => &mut self.privacy, Kind::Elevation => &mut self.elevation }
    }
    pub fn observation(&self, kind: Kind) -> &Observation {
        match kind { Kind::Block => &self.block, Kind::Privacy => &self.privacy, Kind::Elevation => &self.elevation }
    }
    pub fn observe(&mut self, event: Event) {
        use back_notification::{BlockInputState as B, PrivacyModeState as P};
        match event {
            Event::Back(n) => match n.union {
                Some(back_notification::Union::BlockInputState(s)) => match s.enum_value() {
                    Ok(B::BlkOnSucceeded) => self.block.report(Some(true), "on_succeeded"),
                    Ok(B::BlkOffSucceeded) => self.block.report(Some(false), "off_succeeded"),
                    Ok(B::BlkOnFailed) => self.block.report(None, "on_failed"),
                    Ok(B::BlkOffFailed) => self.block.report(None, "off_failed"),
                    _ => {},
                },
                Some(back_notification::Union::PrivacyModeState(s)) => {
                    let (enabled, outcome) = match s.enum_value() {
                        Ok(P::PrvOnSucceeded) => (Some(true), "on_succeeded"),
                        Ok(P::PrvOffSucceeded) => (Some(false), "off_succeeded"),
                        Ok(P::PrvOffByPeer) => (Some(false), "off_by_peer"),
                        Ok(P::PrvOnByOther) => (None, "on_by_other"),
                        Ok(P::PrvNotSupported) => (None, "unsupported"),
                        Ok(P::PrvOnFailedDenied) => (None, "permission_denied"),
                        Ok(P::PrvOnFailedPlugin) => (None, "plugin_missing"),
                        Ok(P::PrvOnFailed) => (None, "on_failed"),
                        Ok(P::PrvOffFailed) => (None, "off_failed"),
                        Ok(P::PrvOffUnknown) => (None, "off_unknown"),
                        _ => return,
                    };
                    self.privacy.report(enabled, outcome);
                    self.privacy_implementation = if enabled == Some(true) {
                        Some(if n.impl_key.is_empty() { "privacy_mode_impl_mag".into() } else { n.impl_key.chars().take(128).collect() })
                    } else { None };
                }
                _ => {},
            },
            Event::Elevation(true) => {
                // Empty stock reply means the elevation flow started; wait for service state.
                if self.elevation.pending {
                    self.elevation.outcome = Some("awaiting_portable_service".into());
                }
            }
            Event::Elevation(false) => self.elevation.report(None, "remote_elevation_failed"),
            Event::Portable(running) => {
                self.portable_service_running = Some(running);
                if running { self.elevation.report(Some(true), "portable_service_running"); }
                else if !self.elevation.pending { self.elevation.report(Some(false), "portable_service_stopped"); }
            }
        }
    }
}

pub fn implementations(s: &SessionSnapshot) -> Vec<String> {
    match s.platform_additions.get("supported_privacy_mode_impl") {
        Some(Value::Array(items)) => items.iter().filter_map(|v| v.get(0)?.as_str().map(str::to_owned)).collect(),
        _ if s.security.privacy_supported == Some(true) => vec!["privacy_mode_impl_mag".into()],
        _ => vec![],
    }
}
pub fn get(permit: &Permit) -> Result<Value> {
    let s = displays::check(permit, false)?;
    let fresh = s.authenticated && s.state == sessions::ConnectionState::Ready;
    Ok(json!({"scope":"remote_machine", "connection_epoch":s.connection_epoch.to_string(), "fresh":fresh,
        "state":s.security, "privacy_implementations":implementations(&s),
        "installed":s.platform_additions.get("is_installed"), "headless":s.platform_additions.get("headless"),
        "permissions":s.permissions, "auth_challenge":s.auth_challenge,
        "headless_login_tool":"rd_session_authenticate with credentials.kind=os_login and current OS-login challenge",
        "observation":"Unknown is not off. Notifications have no request IDs; only peer-reported state is confirmed. Privacy/elevation timeouts remain pending until a peer outcome or reconnect. Stock input blocking reports failures only; no success report means unknown, and its request lock is released after the observation window.",
        "cleanup":"Returning AI control attempts to disable AI-requested input blocking/privacy. Read a fresh peer report to verify. Offline state is unknown; stock peer handles disconnect cleanup."}))
}
fn permission(s: &SessionSnapshot, name: &str) -> Result<()> {
    if s.permissions.get(name) != Some(&true) { return Err(BridgeError::new("PERMISSION_DENIED", "Required remote permission is not granted")); }
    Ok(())
}
pub fn block_check(permit: &Permit, enabled: bool) -> Result<()> {
    let s = displays::check(permit, true)?;
    if s.platform.as_deref() != Some("Windows") { return Err(BridgeError::new("UNSUPPORTED", "Input blocking requires Windows")); }
    if enabled { displays::remote_write_check(permit)?; permission(&s, "block_input")?; }
    Ok(())
}
pub fn privacy_check(permit: &Permit, enabled: bool, implementation: &str) -> Result<()> {
    let s = displays::check(permit, true)?;
    if enabled {
        displays::remote_write_check(permit)?;
        permission(&s, "privacy_mode")?;
        if s.security.privacy_supported != Some(true) || !implementations(&s).iter().any(|v| v == implementation) {
            return Err(BridgeError::new("UNSUPPORTED", "Choose a privacy implementation reported by the peer"));
        }
        let multi = s.platform.as_deref() == Some("Mac OS") || (s.peer_version.as_deref().is_some_and(|v| hbb_common::get_version_number(v) >= hbb_common::get_version_number("1.4.8")) && matches!(implementation, "privacy_mode_impl_mag" | "privacy_mode_impl_exclude_from_capture"));
        if !multi && (s.current_display != 0 || s.ui_session_ids.len() != 1) {
            return Err(BridgeError::new("DISPLAY_CONSTRAINT", "This privacy implementation requires display 0 and one desktop view"));
        }
    }
    Ok(())
}
pub fn elevation_check(permit: &Permit) -> Result<()> {
    let s = displays::remote_write_check(permit)?;
    if s.platform.as_deref() != Some("Windows") { return Err(BridgeError::new("UNSUPPORTED", "Elevation requires Windows")); }
    if s.security.sas_enabled == Some(true) || s.platform_additions["is_installed"] == true || s.security.portable_service_running == Some(true) {
        return Err(BridgeError::new("NOT_NEEDED", "Peer is installed or already elevated"));
    }
    if s.security.sas_enabled.is_none() { return Err(BridgeError::new("SUPPORT_UNKNOWN", "Peer elevation support is not known")); }
    Ok(())
}
pub fn cad_check(permit: &Permit) -> Result<()> {
    let s = displays::remote_write_check(permit)?;
    if s.platform.as_deref() != Some("Linux") && !(s.platform.as_deref() == Some("Windows") && s.security.sas_enabled == Some(true)) {
        return Err(BridgeError::new("UNSUPPORTED", "Ctrl+Alt+Del requires Linux or Windows reporting SAS support"));
    }
    Ok(())
}
fn packet(misc: Misc) -> Message { let mut m = Message::new(); m.set_misc(misc); m }
pub fn block_message(enabled: bool) -> Message {
    let mut misc = Misc::new(); misc.set_option(OptionMessage { block_input: (if enabled { BoolOption::Yes } else { BoolOption::No }).into(), ..Default::default() }); packet(misc)
}
pub fn privacy_message(enabled: bool, implementation: String) -> Message {
    let mut misc = Misc::new(); misc.set_toggle_privacy_mode(TogglePrivacyMode { on:enabled, impl_key:implementation, ..Default::default() }); packet(misc)
}
async fn request(permit: Permit, kind: Kind, enabled: bool, message: Message, wait_ms: u64) -> Result<Value> {
    super::api::wait_budget(wait_ms)?;
    let s = displays::check(&permit, true)?;
    let session = sessions::get(&s.session_id).ok_or_else(|| BridgeError::new("SESSION_CLOSED", "Session closed"))?;
    let (seq, token) = session.security_request(kind, !enabled)?;
    if let Err(error) = wire::send(permit.clone(), message).await {
        if error.code != "DELIVERY_UNKNOWN" { session.security_request_finished(kind, s.connection_epoch, token); }
        return Err(error);
    }
    let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms);
    let mut changed = session.subscribe();
    loop {
        permit.check()?;
        let now = session.snapshot();
        let observed = now.security.observation(kind);
        if observed.sequence > seq || tokio::time::Instant::now() >= deadline {
            let confirmed = observed.sequence > seq && observed.enabled == Some(enabled);
            // Stock BlockOn/BlockOff emit failure notifications only. Silence cannot
            // confirm success and must not permanently prevent the next explicit set.
            if matches!(kind, Kind::Block) {
                session.security_request_finished(kind, s.connection_epoch, token);
            }
            return Ok(json!({"delivery":"sent","confirmed":confirmed,"scope":"remote_machine","requested_enabled":enabled,"state":get(&permit)?,"outcome":if observed.sequence > seq {observed.outcome.clone()} else {Some("unknown".into())}}));
        }
        tokio::select! { _ = changed.changed() => {}, _ = tokio::time::sleep(Duration::from_millis(100)) => {} }
    }
}
pub async fn block(permit: Permit, enabled: bool, wait_ms: u64) -> Result<Value> {
    block_check(&permit, enabled)?;
    request(permit, Kind::Block, enabled, block_message(enabled), wait_ms).await
}
pub async fn privacy(permit: Permit, enabled: bool, implementation: Option<String>, wait_ms: u64) -> Result<Value> {
    let s = displays::check(&permit, true)?;
    let implementation = implementation.or(s.security.privacy_implementation.clone()).unwrap_or_default();
    privacy_check(&permit, enabled, &implementation)?;
    request(permit, Kind::Privacy, enabled, privacy_message(enabled, implementation), wait_ms).await
}
pub async fn elevate(permit: Permit, credentials: Option<(String,String)>, wait_ms: u64) -> Result<Value> {
    elevation_check(&permit)?;
    let mut r = ElevationRequest::new();
    if let Some((username,password)) = credentials {
        if username.is_empty() || username.len()>256 || password.len()>16384 { return Err(BridgeError::invalid("Invalid elevation credential lengths")); }
        r.set_logon(ElevationRequestWithLogon { username,password,..Default::default() });
    } else { r.set_direct(true); }
    let mut misc = Misc::new(); misc.set_elevation_request(r);
    request(permit, Kind::Elevation, true, packet(misc), wait_ms).await
}
pub async fn cad(permit: Permit) -> Result<Value> {
    cad_check(&permit)?;
    let mut message = Message::new(); message.set_key_event(crate::keyboard::client::event_ctrl_alt_del());
    wire::send(permit,message).await?;
    Ok(json!({"delivery":"sent","confirmed":false,"scope":"remote_machine"}))
}
pub async fn os_password(permit: Permit, password: String, activate: bool) -> Result<Value> {
    displays::remote_write_check(&permit)?;
    if password.is_empty() || password.len()>16384 { return Err(BridgeError::invalid("Password must be nonempty and at most 16 KiB")); }
    use super::input::{Action,KeyName};
    let mut actions=Vec::new();
    if activate { actions.push(Action::KeyPress {key:KeyName::Enter}); actions.push(Action::Wait {duration_ms:1200}); }
    actions.push(Action::Text {text:password}); actions.push(Action::KeyPress {key:KeyName::Enter});
    let progress=super::input::send(permit,actions,None,std::future::pending()).await?;
    if let Some(error)=progress.error { return Err(error); }
    Ok(json!({"delivery":"sent","confirmed":false,"scope":"remote_machine","hint":"Typed into the focused OS login field and pressed Enter. Inspect the desktop; no login acknowledgement exists. Password was not saved."}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn elevation_acceptance_is_not_service_success() {
        let mut state=State::default();
        state.elevation.pending=true;
        state.observe(Event::Elevation(true));
        assert!(state.elevation.pending);
        assert_eq!(state.elevation.enabled,None);
        assert_eq!(state.elevation.sequence,0);
        state.observe(Event::Portable(true));
        assert!(!state.elevation.pending);
        assert_eq!(state.elevation.enabled,Some(true));
        state.observe(Event::Elevation(true));
        assert_eq!(state.elevation.outcome.as_deref(),Some("portable_service_running"));
    }
    #[test]
    fn failed_privacy_off_does_not_claim_screen_restored() {
        let mut state=State::default();
        let mut notification=BackNotification::new();
        notification.set_privacy_mode_state(back_notification::PrivacyModeState::PrvOnSucceeded);
        notification.impl_key="privacy_mode_impl_mag".into();
        state.observe(Event::Back(notification));
        assert_eq!(state.privacy.enabled,Some(true));
        let mut notification=BackNotification::new();
        notification.set_privacy_mode_state(back_notification::PrivacyModeState::PrvOffFailed);
        state.observe(Event::Back(notification));
        assert_eq!(state.privacy.enabled,None);
        assert_eq!(state.privacy.outcome.as_deref(),Some("off_failed"));
        let mut notification=BackNotification::new();
        notification.set_privacy_mode_state(back_notification::PrivacyModeState::PrvOffByPeer);
        state.observe(Event::Back(notification));
        assert_eq!(state.privacy.enabled,Some(false));
    }
}
