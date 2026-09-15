use super::{
    control::Agent,
    error::{BridgeError, Result},
    sessions::{self, SessionHandle, SessionKind},
};
use hbb_common::tokio::sync::watch;
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use uuid::Uuid;

struct Pending {
    agent: Agent,
    session: SessionHandle,
    reference: String,
    password: Option<String>,
    password_applied: bool,
    force_relay: bool,
    deadline: Instant,
    claimed: bool,
    error: Option<BridgeError>,
    changed: watch::Sender<bool>,
}

fn pending() -> &'static Mutex<HashMap<String, Pending>> {
    static PENDING: OnceLock<Mutex<HashMap<String, Pending>>> = OnceLock::new();
    PENDING.get_or_init(Default::default)
}

thread_local! { static REGISTERING: RefCell<Option<SessionHandle>> = const { RefCell::new(None) }; }

pub(crate) fn registering() -> Option<SessionHandle> {
    REGISTERING.with(|value| value.borrow().clone())
}

pub fn reserve(
    agent: &Agent,
    peer_id: &str,
    kind: SessionKind,
    password: Option<String>,
    force_relay: bool,
) -> Result<(SessionHandle, bool)> {
    if peer_id.is_empty() || peer_id.len() > 256 || peer_id.chars().any(char::is_control) {
        return Err(BridgeError::invalid("Invalid peer_id"));
    }
    if password.as_ref().is_some_and(|p| p.len() > 16384) {
        return Err(BridgeError::invalid("Password is too long"));
    }
    let mut requests = pending().lock().unwrap();
    let existing = sessions::registered()
        .into_iter()
        .chain(
            requests
                .values()
                .filter(|p| !p.claimed && p.error.is_none())
                .map(|p| p.session.clone()),
        )
        .filter(|s| {
            let s = s.snapshot();
            s.peer_id == peer_id && s.kind == kind && s.state != sessions::ConnectionState::Closed
        })
        .collect::<Vec<_>>();
    if existing.len() > 1 {
        return Err(BridgeError::new(
            "AMBIGUOUS_SESSION",
            "Choose a specific local session with attach",
        ));
    }
    if let Some(session) = existing.into_iter().next() {
        session.control().attach(agent, false)?;
        session.set_capture_enabled(kind == SessionKind::Desktop);
        return Ok((session, false));
    }
    if requests.len() >= 256 {
        return Err(BridgeError::new(
            "LIMIT_EXCEEDED",
            "Too many outstanding GUI requests",
        ));
    }
    let session = SessionHandle::new(peer_id.to_owned(), kind, sessions::shared_cache());
    session.control().attach(agent, true)?;
    session.set_capture_enabled(kind == SessionKind::Desktop);
    let id = format!("g_{}", Uuid::new_v4());
    let reference = session
        .control()
        .view()
        .session_ref
        .ok_or_else(|| BridgeError::new("INTERNAL_ERROR", "Missing initial session reference"))?;
    let terminal_id = if kind == SessionKind::Terminal {
        let permit = session.control().resolve(agent, &reference, true)?;
        Some(super::terminals::prepare(permit, 24, 80)?.1)
    } else {
        None
    };
    let (changed, _) = watch::channel(false);
    let event = serde_json::json!({"name":"automation_open", "request_id":id, "peer_id":peer_id, "kind": if kind == SessionKind::Desktop {"desktop"} else {"terminal"}, "terminal_id":terminal_id.map(|id| id.to_string()).unwrap_or_default(), "force_relay":force_relay.to_string()}).to_string();
    if crate::flutter::push_global_event(crate::flutter::APP_TYPE_MAIN, event) != Some(true) {
        return Err(BridgeError::new(
            "GUI_UNAVAILABLE",
            "Main GUI event channel is unavailable",
        ));
    }
    requests.insert(
        id.clone(),
        Pending {
            agent: agent.clone(),
            session: session.clone(),
            reference,
            password,
            password_applied: false,
            force_relay,
            deadline: Instant::now() + Duration::from_secs(30),
            claimed: false,
            error: None,
            changed,
        },
    );
    Ok((session, true))
}

pub fn add_session(request_id: &str, view: Uuid) -> Result<()> {
    let (session, peer, kind, force_relay) = {
        let mut requests = pending().lock().unwrap();
        let request = requests
            .get_mut(request_id)
            .ok_or_else(|| BridgeError::new("GUI_UNAVAILABLE", "GUI request expired"))?;
        if request.claimed {
            return Err(BridgeError::new(
                "GUI_UNAVAILABLE",
                "GUI request was already registered",
            ));
        }
        let active = request.error.is_none()
            && Instant::now() < request.deadline
            && request.agent.is_alive()
            && request
                .session
                .control()
                .resolve(&request.agent, &request.reference, true)
                .is_ok();
        if !active {
            request.password = None;
            if let Err(error) = request.session.control().release(None, true) {
                hbb_common::log::warn!("GUI reservation cleanup: {}", error.code);
            }
        }
        let snapshot = request.session.snapshot();
        (
            if snapshot.state == sessions::ConnectionState::Closed {
                SessionHandle::new(
                    snapshot.peer_id.clone(),
                    snapshot.kind,
                    sessions::shared_cache(),
                )
            } else {
                request.session.clone()
            },
            snapshot.peer_id,
            snapshot.kind,
            request.force_relay,
        )
    };
    if sessions::registered().iter().any(|s| {
        let s = s.snapshot();
        s.peer_id == peer && s.kind == kind && s.session_id != session.snapshot().session_id
    }) {
        failed(
            request_id,
            "A human session opened concurrently; attach to that session explicitly",
        );
        return Err(BridgeError::new(
            "SESSION_BUSY",
            "A human session opened concurrently",
        ));
    }
    struct Registration;
    impl Drop for Registration {
        fn drop(&mut self) {
            REGISTERING.with(|value| *value.borrow_mut() = None);
        }
    }
    REGISTERING.with(|value| *value.borrow_mut() = Some(session.clone()));
    let _registration = Registration;
    crate::flutter::session_add(
        &view,
        &peer,
        false,
        false,
        false,
        false,
        kind == SessionKind::Terminal,
        "",
        force_relay,
        String::new(),
        false,
        None,
    )
    .map_err(|_| BridgeError::new("GUI_UNAVAILABLE", "Official session registration failed"))?;
    let mut requests = pending().lock().unwrap();
    if let Some(request) = requests.get_mut(request_id) {
        request.claimed = true;
        request.changed.send_replace(true);
    }
    Ok(())
}

pub(crate) fn take_password<T: crate::ui_session_interface::InvokeUiSession>(
    core: &crate::ui_session_interface::Session<T>,
) -> Option<(super::control::Permit, String)> {
    let session = sessions::for_core(core)?;
    let id = session.snapshot().session_id;
    let mut requests = pending().lock().unwrap();
    let request = requests
        .values_mut()
        .find(|p| p.session.snapshot().session_id == id && p.claimed)?;
    let permit = match request
        .session
        .control()
        .resolve(&request.agent, &request.reference, true)
        .and_then(|permit| {
            permit.check()?;
            Ok(permit)
        }) {
        Ok(permit) => permit,
        Err(_) => {
            request.password = None;
            return None;
        }
    };
    request.password_applied = request.password.is_some();
    request.password.take().map(|password| (permit, password))
}

pub fn failed(request_id: &str, message: &str) {
    if let Some(request) = pending().lock().unwrap().get_mut(request_id) {
        request.password = None;
        request.error = Some(BridgeError::new("GUI_UNAVAILABLE", message));
        request.session.close_placeholder(message);
        request.changed.send_replace(true);
    }
}

pub fn placeholders() -> Vec<SessionHandle> {
    pending()
        .lock()
        .unwrap()
        .values()
        .filter(|p| !p.claimed)
        .map(|p| p.session.clone())
        .collect()
}

pub fn tick() {
    let mut requests = pending().lock().unwrap();
    for request in requests.values_mut() {
        if !request.claimed
            && request.error.is_none()
            && (Instant::now() >= request.deadline || !request.agent.is_alive())
        {
            request.password = None;
            request.error = Some(BridgeError::new(
                "GUI_UNAVAILABLE",
                "GUI creation timed out or its agent disconnected",
            ));
            request
                .session
                .close_placeholder("GUI creation timed out or was cancelled");
            request.changed.send_replace(true);
        }
        if !request.agent.is_alive()
            || request
                .session
                .control()
                .resolve(&request.agent, &request.reference, true)
                .is_err()
        {
            request.password = None;
        }
    }
    requests.retain(|_, request| request.deadline.elapsed() < Duration::from_secs(300));
}

fn close_requests() -> &'static Mutex<HashMap<String, (Instant, super::control::Permit)>> {
    static CLOSE: OnceLock<Mutex<HashMap<String, (Instant, super::control::Permit)>>> =
        OnceLock::new();
    CLOSE.get_or_init(Default::default)
}
pub fn close(permit: super::control::Permit) -> Result<()> {
    permit.check()?;
    let session = sessions::get(&permit.authority.session_id)
        .ok_or_else(|| BridgeError::new("SESSION_CLOSED", "Session is closed"))?;
    let request = format!("close_{}", Uuid::new_v4());
    let mut pending = close_requests().lock().unwrap();
    pending.retain(|_, (at, p)| at.elapsed().as_secs() < 30 && p.read_check().is_ok());
    if pending.len() >= 256 {
        return Err(BridgeError::new(
            "LIMIT_EXCEEDED",
            "Too many pending GUI close requests",
        ));
    }
    let event=serde_json::json!({"name":"automation_close","request_id":request,"peer_id":session.snapshot().peer_id,"kind":if session.snapshot().kind==SessionKind::Desktop{"desktop"}else{"terminal"}}).to_string();
    pending.insert(request.clone(), (Instant::now(), permit));
    if crate::flutter::push_global_event(crate::flutter::APP_TYPE_MAIN, event) != Some(true) {
        pending.remove(&request);
        return Err(BridgeError::new(
            "GUI_UNAVAILABLE",
            "Cannot deliver GUI close request",
        ));
    }
    Ok(())
}
pub fn can_close(request: &str, view: Uuid) -> bool {
    let pending = close_requests().lock().unwrap();
    let Some((at, permit)) = pending.get(request) else {
        return false;
    };
    at.elapsed().as_secs() < 30
        && permit.check().is_ok()
        && sessions::for_view(&view)
            .is_some_and(|s| s.snapshot().session_id == permit.authority.session_id)
}
pub fn password_applied(session: &str) -> bool {
    pending()
        .lock()
        .unwrap()
        .values()
        .find(|p| p.session.snapshot().session_id == session)
        .is_some_and(|p| p.password_applied)
}

pub struct OpenGuard {
    session: SessionHandle,
    armed: bool,
}
impl OpenGuard {
    pub fn new(session: SessionHandle, created: bool) -> Self {
        Self {
            session,
            armed: created,
        }
    }
    pub fn finish(&mut self) {
        self.armed = false;
    }
}
impl Drop for OpenGuard {
    fn drop(&mut self) {
        if self.armed {
            if let Err(error) = self.session.control().release(None, true) {
                hbb_common::log::warn!("Cancelled open cleanup: {}", error.code);
            }
            super::api::transition(&self.session);
        }
    }
}

fn views() -> &'static Mutex<HashMap<String, &'static str>> {
    static VIEWS: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    VIEWS.get_or_init(Default::default)
}
pub fn set_visibility(view: Uuid, visible: bool, minimized: bool) {
    let Some(session) = sessions::for_view(&view) else {
        return;
    };
    let value = if minimized {
        "minimized"
    } else if visible {
        "visible"
    } else {
        "hidden"
    };
    let changed = views().lock().unwrap().insert(view.to_string(), value) != Some(value);
    if changed {
        session.changed();
    }
}
pub fn visibility(session: &SessionHandle) -> &'static str {
    let snapshot = session.snapshot();
    let views = views().lock().unwrap();
    let values = snapshot
        .ui_session_ids
        .iter()
        .filter_map(|id| views.get(id));
    if values.clone().any(|v| *v == "visible") {
        "visible"
    } else if values.clone().any(|v| *v == "minimized") {
        "minimized"
    } else {
        "hidden"
    }
}
pub fn forget_view(view: &Uuid) {
    views().lock().unwrap().remove(&view.to_string());
}
