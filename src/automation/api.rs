use super::{
    control::{Agent, Permit},
    error::{BridgeError, Result},
    screen,
    sessions::{self, ConnectionState, SessionHandle, SessionKind},
    terminals, wire,
};
use hbb_common::tokio;
use serde_json::{json, Value};
use std::time::Duration;

pub fn resolve(agent: &Agent, reference: &str, write: bool) -> Result<(SessionHandle, Permit)> {
    if !agent.is_alive() {
        return Err(BridgeError::new(
            "BINDING_EXPIRED",
            "MCP logical client is disconnected",
        ));
    }
    for session in sessions::list() {
        let control = session.control();
        if let Ok(permit) = control.resolve(agent, reference, false) {
            if write {
                if session.snapshot().state == ConnectionState::Closed {
                    return Err(BridgeError::new(
                        "SESSION_CLOSED",
                        "Only read-only completion records remain",
                    ));
                }
                let permit = control.resolve(agent, reference, true)?;
                return Ok((session, permit));
            }
            return Ok((session, permit));
        }
    }
    Err(BridgeError::new(
        "SESSION_REF_EXPIRED",
        "Session reference is absent, expired, or belongs to another binding",
    ))
}
pub fn state_name(state: ConnectionState) -> &'static str {
    match state {
        ConnectionState::Connecting => "connecting",
        ConnectionState::AwaitingAuth => "awaiting_auth",
        ConnectionState::AwaitingHuman => "awaiting_human",
        ConnectionState::AwaitingFrame => "awaiting_frame",
        ConnectionState::Ready => "ready",
        ConnectionState::Disconnected => "disconnected",
        ConnectionState::Closed => "closed",
    }
}
pub fn revision(session: &SessionHandle) -> String {
    format!(
        "{}{}",
        session.snapshot().revision,
        format!(
            "{:020}",
            session
                .control()
                .view()
                .revision
                .parse::<u64>()
                .unwrap_or(0)
        )
    )
}
pub fn view(session: &SessionHandle, full: bool) -> Value {
    let s = session.snapshot();
    let control = session.control().view();
    let primary = screen::primary(session);
    let mut result = json!({"session_id":s.session_id,"session_ref":control.session_ref,"peer_id":s.peer_id,"kind":s.kind.name(),"state":state_name(s.state),"control":{"mode":control.mode,"approval_required":control.approval_required,"transitioning":control.transitioning},"revision":revision(session),"platform":s.platform,"can_input":s.kind==SessionKind::Desktop&&control.mode==super::control::Mode::Ai&&!control.transitioning&&s.authenticated&&s.state==ConnectionState::Ready&&s.permissions.get("keyboard")==Some(&true),"can_capture":s.kind==SessionKind::Desktop&&s.authenticated,"can_use_terminal":s.kind==SessionKind::Terminal&&s.authenticated&&s.terminal_supported==Some(true)});
    if s.kind == SessionKind::Desktop {
        result["displays"]=json!(s.displays.iter().map(|d|json!({"id":d.id.to_string(),"name":d.name,"primary":primary.map(|p|p==d.id),"width":d.width,"height":d.height})).collect::<Vec<_>>());
    } else if s.kind == SessionKind::Terminal {
        result["terminals"] = json!(terminals::visible_list(
            &s.session_id,
            session.control().binding_id().as_deref()
        ));
    }
    if let Some(approval) = control.approval {
        result["approval"] = json!(approval);
    }
    if s.state == ConnectionState::AwaitingAuth {
        result["auth_challenge"] = json!(s.auth_challenge);
        result["auth_error"] = json!(s.last_error);
    }
    if s.state == ConnectionState::AwaitingHuman {
        result["human_action"] = json!({"kind":"remote_confirmation","message":s.last_error});
    }
    result["connection"] = json!({"state":state_name(s.state),"epoch":s.connection_epoch.to_string(),"authenticated":s.authenticated,"error":s.last_error});
    if full {
        result["control"]["release_error"] = json!(control.release_error);
        result["control"]["released_inputs"] = json!(control.released_inputs);
        result["updated_at"] = json!(s.updated_at);
        result["owner"] = json!("self");
        result["ui_session_ids"] = json!(s.ui_session_ids);
        result["gui"] = json!({"registered":!s.ui_session_ids.is_empty(),"visibility":super::gui::visibility(session)});
        result["capabilities"] = json!({"keyboard":{"supported":s.kind==SessionKind::Desktop,"allowed":s.permissions.get("keyboard")},"terminal":{"supported":s.terminal_supported,"allowed":if s.kind==SessionKind::Terminal&&s.authenticated{Some(true)}else{None}}});
        result["layout_revision"] = json!(s.layout_revision.to_string());
        result["displays"]=json!(s.displays.iter().map(|d|json!({"id":d.id.to_string(),"name":d.name,"x":d.x,"y":d.y,"width":d.width,"height":d.height,"scale":d.scale,"online":d.online,"primary":primary.map(|p|p==d.id)})).collect::<Vec<_>>());
    }
    if control.mode == super::control::Mode::Human {
        result["next_action"] = json!({"tool":"rd_control_request","reason":"Human control permits reads; request control explicitly before writing"});
    } else if s.state == ConnectionState::AwaitingAuth {
        result["next_action"] = json!({"tool":"rd_session_authenticate","reason":"Submit credentials for the current authentication challenge"});
    }
    result
}
pub fn wait_budget(ms: u64) -> Result<()> {
    if ms > 30_000 {
        Err(BridgeError::invalid("wait_ms must be 0..30000"))
    } else {
        Ok(())
    }
}
pub async fn wait_session(
    session: &SessionHandle,
    permit: &Permit,
    after: Option<&str>,
    ms: u64,
) -> Result<bool> {
    wait_budget(ms)?;
    let mut state = session.subscribe();
    let mut control = session.control().subscribe();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
    loop {
        permit.read_check()?;
        let snapshot = session.snapshot();
        let done = match after {
            Some(revision) => self::revision(session) != revision,
            None => matches!(
                snapshot.state,
                ConnectionState::Ready
                    | ConnectionState::AwaitingAuth
                    | ConnectionState::AwaitingHuman
                    | ConnectionState::Disconnected
                    | ConnectionState::Closed
            ),
        };
        if done {
            return Ok(true);
        }
        if ms == 0 {
            return Ok(false);
        }
        if tokio::time::timeout_at(deadline, async {
            tokio::select! {_=state.changed()=>{},_=control.changed()=>{}}
        })
        .await
        .is_err()
        {
            return Ok(false);
        }
    }
}
pub fn transition(session: &SessionHandle) {
    let control = session.control();
    if let Some((generation, _)) = control.pending() {
        if matches!(
            session.snapshot().state,
            ConnectionState::Connecting | ConnectionState::Disconnected | ConnectionState::Closed
        ) {
            control.complete_transition(generation, None);
        } else {
            wire::wake(&session.snapshot().session_id);
        }
    }
}
pub fn terminal_session(session: &SessionHandle) -> Result<()> {
    if session.snapshot().kind != SessionKind::Terminal {
        Err(BridgeError::new(
            "WRONG_SESSION_KIND",
            "This tool requires a terminal connection",
        ))
    } else {
        Ok(())
    }
}

pub fn disconnect(permit: Permit) -> Result<()> {
    permit.check()?;
    let core = sessions::core(&permit.authority.session_id)
        .ok_or_else(|| BridgeError::new("GUI_UNAVAILABLE", "GUI session is unavailable"))?;
    let sender = core
        .sender
        .read()
        .unwrap()
        .clone()
        .ok_or_else(|| BridgeError::new("DISCONNECTED", "Remote sender is unavailable"))?;
    sender
        .send(crate::client::Data::AutomationDisconnect(permit))
        .map_err(|_| BridgeError::new("DISCONNECTED", "Remote sender closed"))
}
pub fn reconnect(permit: &Permit, force_relay: bool) -> Result<()> {
    permit.check()?;
    let session = sessions::get(&permit.authority.session_id)
        .ok_or_else(|| BridgeError::new("SESSION_CLOSED", "Session is closed"))?;
    if session.snapshot().state == ConnectionState::Connecting {
        return Err(BridgeError::new(
            "NOT_READY",
            "Session is already connecting",
        ));
    }
    let core = sessions::core(&permit.authority.session_id)
        .ok_or_else(|| BridgeError::new("GUI_UNAVAILABLE", "GUI session is unavailable"))?;
    session.control().prepare_reconnect(permit)?;
    core.reconnect(force_relay);
    Ok(())
}

pub async fn wait_until(
    session: &SessionHandle,
    permit: &Permit,
    ms: u64,
    done: impl Fn(&sessions::SessionSnapshot) -> bool,
) -> Result<bool> {
    wait_budget(ms)?;
    let mut changes = session.subscribe();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
    loop {
        permit.read_check()?;
        if done(&session.snapshot()) {
            return Ok(true);
        }
        if ms == 0
            || tokio::time::timeout_at(deadline, changes.changed())
                .await
                .is_err()
        {
            return Ok(false);
        }
    }
}
