use super::frames::{FrameCache, FrameError, FrameRead, FrameStamp};
use crate::{
    client::Interface,
    ui_session_interface::{ConnectionRoundState, InvokeUiSession, Session},
};
use hbb_common::{
    message_proto::{DisplayInfo, PeerInfo, PermissionInfo, SwitchDisplay},
    rendezvous_proto::ConnType,
    tokio::sync::watch,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::{Arc, Mutex, OnceLock, Weak},
};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionKind {
    Desktop,
    Terminal,
    FileTransfer,
}

impl SessionKind {
    pub fn name(self) -> &'static str {
        match self { Self::Desktop => "desktop", Self::Terminal => "terminal", Self::FileTransfer => "file_transfer" }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    AwaitingAuth,
    AwaitingHuman,
    AwaitingFrame,
    Ready,
    Disconnected,
    Closed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Display {
    pub id: usize,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub scale: f64,
    pub cursor_embedded: bool,
    pub online: bool,
    pub original_resolution: Option<(i32, i32)>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct AuthChallenge {
    pub id: String,
    pub kind: String,
    pub fields: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct SessionSnapshot {
    pub session_id: String,
    pub peer_id: String,
    pub kind: SessionKind,
    pub ui_session_ids: BTreeSet<String>,
    pub state: ConnectionState,
    pub connection_epoch: u64,
    pub authenticated: bool,
    pub auth_challenge: Option<AuthChallenge>,
    pub platform: Option<String>,
    pub peer_version: Option<String>,
    pub terminal_supported: Option<bool>,
    /// Missing entries are unknown, never permission grants.
    pub permissions: BTreeMap<String, bool>,
    pub displays: Vec<Display>,
    pub current_display: usize,
    pub resolutions: BTreeMap<usize, Vec<(i32, i32)>>,
    pub platform_additions: serde_json::Value,
    pub layout_revision: u64,
    pub revision: u64,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub last_error: Option<String>,
    pub frame_error: Option<FrameError>,
}

struct State {
    snapshot: SessionSnapshot,
    started: bool,
    capture_enabled: bool,
    sequence: u64,
}

struct Record {
    control: Arc<super::control::Authority>,
    state: Mutex<State>,
    changes: watch::Sender<u64>,
    frames_changed: watch::Sender<u64>,
    cache: Arc<Mutex<FrameCache>>,
}

/// Shared by every GUI view of one core connection. This is not agent authorization.
#[derive(Clone)]
pub struct SessionHandle(Arc<Record>);

impl SessionHandle {
    pub(crate) fn new(peer_id: String, kind: SessionKind, cache: Arc<Mutex<FrameCache>>) -> Self {
        let session_id = format!("s_{}", Uuid::new_v4());
        let (changes, _) = watch::channel(0);
        let (frames_changed, _) = watch::channel(0);
        Self(Arc::new(Record {
            control: Arc::new(super::control::Authority::new(session_id.clone())),
            state: Mutex::new(State {
                snapshot: SessionSnapshot {
                    session_id,
                    peer_id,
                    kind,
                    ui_session_ids: BTreeSet::new(),
                    state: ConnectionState::Connecting,
                    connection_epoch: 0,
                    authenticated: false,
                    auth_challenge: None,
                    platform: None,
                    peer_version: None,
                    terminal_supported: None,
                    permissions: BTreeMap::new(),
                    displays: vec![],
                    current_display: 0,
                    resolutions: BTreeMap::new(),
                    platform_additions: serde_json::Value::Null,
                    layout_revision: 0,
                    revision: 0,
                    updated_at: chrono::Utc::now(),
                    last_error: None,
                    frame_error: None,
                },
                started: false,
                capture_enabled: false,
                sequence: 0,
            }),
            changes,
            frames_changed,
            cache,
        }))
    }

    pub fn selection_changed(&self) {
        let mut state = self.0.state.lock().unwrap();
        state.snapshot.layout_revision += 1;
        self.invalidate_frames(&mut state);
        self.notify(&mut state);
        let id = state.snapshot.session_id.clone();
        drop(state);
        super::wire::wake(&id);
    }

    pub fn snapshot(&self) -> SessionSnapshot {
        self.0.state.lock().unwrap().snapshot.clone()
    }

    pub fn control(&self) -> Arc<super::control::Authority> {
        self.0.control.clone()
    }

    pub(crate) fn close_placeholder(&self, error: &str) {
        let mut state = self.0.state.lock().unwrap();
        state.snapshot.state = ConnectionState::Closed;
        state.snapshot.last_error = Some(error.to_owned());
        self.0
            .control
            .disconnected(state.snapshot.connection_epoch, true);
        self.notify(&mut state);
    }

    /// Subscribe before reading snapshot() so a concurrent update cannot be missed.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.0.changes.subscribe()
    }
    pub fn subscribe_frames(&self) -> watch::Receiver<u64> {
        self.0.frames_changed.subscribe()
    }

    pub fn set_capture_enabled(&self, enabled: bool) {
        let mut state = self.0.state.lock().unwrap();
        let enabled = enabled && state.snapshot.state != ConnectionState::Closed;
        if state.capture_enabled != enabled {
            state.capture_enabled = enabled;
            self.invalidate_frames(&mut state);
        }
    }

    pub fn read_frame(&self, display: usize, after: Option<u64>) -> Result<FrameRead, FrameError> {
        let state = self.0.state.lock().unwrap();
        if !state.capture_enabled {
            return Err(FrameError::CaptureDisabled);
        }
        if state.snapshot.state == ConnectionState::Closed {
            return Err(FrameError::StaleConnection);
        }
        let mut result = self.0.cache.lock().unwrap().read(
            &state.snapshot.session_id,
            display,
            state.snapshot.connection_epoch,
            state.snapshot.layout_revision,
            after,
        )?;
        result.disconnected = state.snapshot.state == ConnectionState::Disconnected;
        Ok(result)
    }

    pub(crate) fn capture_state(&self, stamp: FrameStamp) -> Result<ConnectionState, FrameError> {
        let state = self.0.state.lock().unwrap();
        if !state.capture_enabled {
            return Err(FrameError::CaptureDisabled);
        }
        if state.snapshot.state == ConnectionState::Closed
            || state.snapshot.connection_epoch != stamp.connection_epoch
        {
            return Err(FrameError::StaleConnection);
        }
        if state.snapshot.layout_revision != stamp.layout_revision {
            return Err(FrameError::StaleLayout);
        }
        if !matches!(
            state.snapshot.state,
            ConnectionState::Ready | ConnectionState::Disconnected
        ) {
            return Err(FrameError::NoFrame);
        }
        Ok(state.snapshot.state)
    }

    pub fn changed(&self) {
        let mut state = self.0.state.lock().unwrap();
        self.notify(&mut state);
    }

    fn notify(&self, state: &mut State) {
        state.snapshot.updated_at = chrono::Utc::now();
        state.snapshot.revision += 1;
        self.0.changes.send_replace(state.snapshot.revision);
    }

    fn invalidate_frames(&self, state: &mut State) {
        self.0
            .cache
            .lock()
            .unwrap()
            .clear(&state.snapshot.session_id);
        state.sequence += 1;
        self.0.frames_changed.send_replace(state.sequence);
    }

    fn begin(&self, epoch: u64) -> Option<Connection> {
        let mut state = self.0.state.lock().unwrap();
        if state.snapshot.state == ConnectionState::Closed
            || (state.started && epoch <= state.snapshot.connection_epoch)
        {
            return None;
        }
        if state.started {
            super::terminals::disconnected(&state.snapshot.session_id, self.control().binding_id());
            self.0.control.disconnected(epoch, false);
        } else {
            self.0.control.first_connection(epoch);
        }
        state.started = true;
        state.snapshot.connection_epoch = epoch;
        state.snapshot.state = ConnectionState::Connecting;
        state.snapshot.authenticated = false;
        state.snapshot.auth_challenge = None;
        state.snapshot.platform = None;
        state.snapshot.peer_version = None;
        state.snapshot.terminal_supported = None;
        state.snapshot.permissions.clear();
        state.snapshot.displays.clear();
        state.snapshot.resolutions.clear();
        state.snapshot.platform_additions = serde_json::Value::Null;
        state.snapshot.layout_revision += 1;
        state.snapshot.last_error = None;
        state.snapshot.frame_error = None;
        self.invalidate_frames(&mut state);
        self.notify(&mut state);
        Some(Connection {
            session: self.clone(),
            epoch,
        })
    }
}

/// Captured by an individual IO loop and its decoder threads, never retargeted on reconnect.
#[derive(Clone)]
pub(crate) struct Connection {
    session: SessionHandle,
    epoch: u64,
}

impl Connection {
    pub(crate) fn terminal(
        &self,
        mut response: hbb_common::message_proto::TerminalResponse,
    ) -> hbb_common::message_proto::TerminalResponse {
        if let Some(hbb_common::message_proto::terminal_response::Union::Data(data)) =
            &mut response.union
        {
            if data.compressed {
                data.data = hbb_common::compress::decompress(&data.data).into();
                data.compressed = false;
            }
        }
        let mut state = self.session.0.state.lock().unwrap();
        if state.snapshot.connection_epoch == self.epoch
            && state.snapshot.authenticated
            && state.snapshot.state != ConnectionState::Closed
        {
            super::terminals::response(
                &state.snapshot.session_id,
                self.epoch,
                self.session.control().binding_id(),
                &response,
            );
            if !matches!(
                response.union,
                Some(hbb_common::message_proto::terminal_response::Union::Data(_))
            ) {
                self.session.notify(&mut state);
            }
        }
        response
    }
    fn update(&self, f: impl FnOnce(&mut State)) {
        let mut state = self.session.0.state.lock().unwrap();
        if state.snapshot.connection_epoch != self.epoch
            || matches!(
                state.snapshot.state,
                ConnectionState::Closed | ConnectionState::Disconnected
            )
        {
            return;
        }
        f(&mut state);
        self.session.notify(&mut state);
    }

    pub(crate) fn connection_error(&self, error: &str) {
        self.update(|state| {
            state.snapshot.last_error = Some(error.chars().take(1024).collect());
        });
    }

    pub(crate) fn login_error(&self, error: &str) {
        self.update(|state| {
            state.snapshot.authenticated = false;
            state.snapshot.auth_challenge = None;
            state.snapshot.state = if matches!(
                error,
                crate::client::LOGIN_MSG_PASSWORD_EMPTY
                    | crate::client::LOGIN_MSG_PASSWORD_WRONG
                    | crate::client::REQUIRE_2FA
                    | crate::client::LOGIN_MSG_2FA_WRONG
                    | crate::client::LOGIN_MSG_DESKTOP_SESSION_NOT_READY_PASSWORD_EMPTY
                    | crate::client::LOGIN_MSG_DESKTOP_SESSION_NOT_READY_PASSWORD_WRONG
            ) {
                ConnectionState::AwaitingAuth
            } else {
                ConnectionState::AwaitingHuman
            };
            let kind = if matches!(
                error,
                crate::client::REQUIRE_2FA | crate::client::LOGIN_MSG_2FA_WRONG
            ) {
                "two_factor"
            } else if matches!(
                error,
                crate::client::LOGIN_MSG_DESKTOP_SESSION_NOT_READY_PASSWORD_EMPTY
                    | crate::client::LOGIN_MSG_DESKTOP_SESSION_NOT_READY_PASSWORD_WRONG
            ) {
                "os_login"
            } else {
                "password"
            };
            state.snapshot.auth_challenge = (state.snapshot.state == ConnectionState::AwaitingAuth)
                .then(|| AuthChallenge {
                    id: format!("auth_{}", Uuid::new_v4()),
                    kind: kind.into(),
                    fields: match kind {
                        "two_factor" => vec!["code".into()],
                        "os_login" => vec!["username".into(), "password".into()],
                        _ => vec!["password".into()],
                    },
                });
            state.snapshot.last_error = Some(error.chars().take(1024).collect());
            self.session.invalidate_frames(state);
        });
    }

    pub(crate) fn authenticated(&self, peer: &PeerInfo) {
        self.update(|state| {
            state.snapshot.authenticated = true;
            state.snapshot.auth_challenge = None;
            state.snapshot.platform = Some(peer.platform.clone());
            state.snapshot.peer_version = Some(peer.version.clone());
            // Desktop peers send initial permission messages only for denials, before PeerInfo.
            if matches!(peer.platform.as_str(), "Windows" | "Mac OS" | "Linux") {
                state
                    .snapshot
                    .permissions
                    .entry("keyboard".into())
                    .or_insert(true);
                for permission in ["clipboard", "file", "restart"] {
                    state.snapshot.permissions.entry(permission.into()).or_insert(true);
                }
            }
            state.snapshot.terminal_supported = peer.features.as_ref().map(|f| f.terminal);
            state.snapshot.current_display = peer.current_display as usize;
            state.snapshot.displays = displays(&peer.displays);
            state.snapshot.platform_additions = serde_json::from_str(&peer.platform_additions).unwrap_or_default();
            if let Some(modes) = peer.resolutions.as_ref() { state.snapshot.resolutions.insert(peer.current_display as usize, modes.resolutions.iter().map(|r| (r.width, r.height)).collect()); }
            state.snapshot.layout_revision += 1;
            state.snapshot.last_error = None;
            state.snapshot.state = match state.snapshot.kind {
                SessionKind::Terminal if state.snapshot.terminal_supported == Some(true) => {
                    ConnectionState::Ready
                }
                SessionKind::Terminal => ConnectionState::AwaitingHuman,
                SessionKind::Desktop => ConnectionState::AwaitingFrame,
                SessionKind::FileTransfer => ConnectionState::Ready,
            };
            self.session.invalidate_frames(state);
        });
    }

    pub(crate) fn clipboard(&self, clipboards: &[hbb_common::message_proto::Clipboard]) {
        super::text_clipboard::observe(&self.session, clipboards);
    }

    pub(crate) fn permission(&self, permission: &PermissionInfo) {
        if let Ok(kind) = permission.permission.enum_value() {
            self.update(|state| {
                state
                    .snapshot
                    .permissions
                    .insert(format!("{kind:?}").to_ascii_lowercase(), permission.enabled);
            });
        }
    }

    pub(crate) fn platform_additions(&self, value: &str) {
        let incoming = if value.is_empty() { serde_json::Map::new() } else {
            match serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(value) {
                Ok(value) => value,
                Err(error) => { hbb_common::log::warn!("Invalid display platform additions: {error}"); return; }
            }
        };
        self.update(|state| {
            // Display-service PeerInfo contains a partial update, not login facts.
            if !state.snapshot.platform_additions.is_object() { state.snapshot.platform_additions = serde_json::json!({}); }
            if let Some(current) = state.snapshot.platform_additions.as_object_mut() {
                for key in ["rustdesk_virtual_displays", "amyuni_virtual_displays"] { current.remove(key); }
                current.extend(incoming);
            }
        });
    }

    pub(crate) fn layout(&self, incoming: &[DisplayInfo]) {
        let incoming = displays(incoming);
        self.update(|state| {
            if state.snapshot.displays != incoming {
                // A resolution change can send SwitchDisplay (with modes) before
                // PeerInfo (geometry only). Keep modes for the same display identity.
                let retained = incoming.iter().filter(|d| state.snapshot.displays.iter().any(|old|
                    old.id == d.id && old.name == d.name && old.online && d.online
                )).map(|d| d.id).collect::<BTreeSet<_>>();
                state.snapshot.resolutions.retain(|id, _| retained.contains(id));
                state.snapshot.displays = incoming;
                self.layout_changed(state);
            }
        });
    }

    fn layout_changed(&self, state: &mut State) {
        state.snapshot.layout_revision += 1;
        if state.snapshot.authenticated && state.snapshot.kind == SessionKind::Desktop {
            state.snapshot.state = ConnectionState::AwaitingFrame;
        }
        self.session.invalidate_frames(state);
    }

    pub(crate) fn switch_display(&self, display: &SwitchDisplay) {
        self.update(|state| {
            if let Some(d) = state
                .snapshot
                .displays
                .iter_mut()
                .find(|d| d.id as i32 == display.display)
            {
                let before = d.clone();
                d.x = display.x;
                d.y = display.y;
                d.width = display.width;
                d.height = display.height;
                d.cursor_embedded = display.cursor_embedded;
                d.original_resolution = display.original_resolution.as_ref().map(|r| (r.width, r.height));
                if *d != before {
                    self.layout_changed(state);
                }
                state.snapshot.current_display = display.display as usize;
                if let Some(modes) = display.resolutions.as_ref() { state.snapshot.resolutions.insert(display.display as usize, modes.resolutions.iter().map(|r| (r.width, r.height)).collect()); }
            }
        });
    }

    pub(crate) fn disconnected(&self) {
        self.update(|state| {
            super::files::disconnected(&state.snapshot.session_id, self.epoch);
            super::terminals::disconnected(
                &state.snapshot.session_id,
                self.session.control().binding_id(),
            );
            self.session.0.control.disconnected(self.epoch, false);
            state.snapshot.state = ConnectionState::Disconnected;
            state.snapshot.authenticated = false;
            state.snapshot.auth_challenge = None;
            state.snapshot.permissions.clear();
            state.sequence += 1;
            self.session.0.frames_changed.send_replace(state.sequence);
        });
    }

    pub(crate) fn layout_revision(&self) -> u64 {
        self.session
            .0
            .state
            .lock()
            .unwrap()
            .snapshot
            .layout_revision
    }

    #[cfg(test)]
    fn frame(&self, display: usize, image: &scrap::ImageRgb, pixelbuffer: bool) {
        self.frame_at_layout(display, image, pixelbuffer, self.layout_revision());
    }

    pub(crate) fn frame_at_layout(
        &self,
        display: usize,
        image: &scrap::ImageRgb,
        pixelbuffer: bool,
        layout_revision: u64,
    ) {
        let mut state = self.session.0.state.lock().unwrap();
        if state.snapshot.connection_epoch != self.epoch
            || state.snapshot.layout_revision != layout_revision
            || !state.snapshot.authenticated
            || !matches!(
                state.snapshot.state,
                ConnectionState::AwaitingFrame | ConnectionState::Ready
            )
        {
            return;
        }
        let Some(info) = state.snapshot.displays.iter().find(|d| d.id == display) else {
            return;
        };
        let cursor_embedded = info.cursor_embedded;
        if !pixelbuffer {
            if state.snapshot.frame_error != Some(FrameError::TextureOnly) {
                state.snapshot.frame_error = Some(FrameError::TextureOnly);
                self.session.invalidate_frames(&mut state);
                self.session.notify(&mut state);
            }
            return;
        }
        if let Err(error) = super::frames::validate(image) {
            if state.snapshot.frame_error != Some(error) {
                state.snapshot.frame_error = Some(error);
                self.session.invalidate_frames(&mut state);
                self.session.notify(&mut state);
            }
            return;
        }
        if state.snapshot.state != ConnectionState::Ready {
            state.snapshot.state = ConnectionState::Ready;
            self.session.notify(&mut state);
        }
        if !state.capture_enabled {
            if state.snapshot.frame_error.take().is_some() {
                self.session.notify(&mut state);
            }
            return;
        }
        state.sequence += 1;
        let stamp = FrameStamp {
            connection_epoch: self.epoch,
            layout_revision: state.snapshot.layout_revision,
            display_id: display,
            sequence: state.sequence,
        };
        let result = self.session.0.cache.lock().unwrap().copy(
            &state.snapshot.session_id,
            stamp,
            image,
            cursor_embedded,
        );
        let error = result.err();
        if state.snapshot.frame_error != error {
            state.snapshot.frame_error = error;
            self.session.notify(&mut state);
        }
        self.session.0.frames_changed.send_replace(state.sequence);
    }
}

fn displays(incoming: &[DisplayInfo]) -> Vec<Display> {
    incoming
        .iter()
        .enumerate()
        .map(|(id, d)| Display {
            id,
            name: d.name.clone(),
            x: d.x,
            y: d.y,
            width: d.width,
            height: d.height,
            scale: d.scale,
            cursor_embedded: d.cursor_embedded,
            online: d.online,
            original_resolution: d.original_resolution.as_ref().map(|r| (r.width, r.height)),
        })
        .collect()
}

struct Entry {
    // Keeping the weak allocation alive also prevents pointer reuse while registered.
    core: Weak<Mutex<ConnectionRoundState>>,
    session: SessionHandle,
}

#[derive(Default)]
struct Registry {
    entries: HashMap<usize, Entry>,
    cache: Arc<Mutex<FrameCache>>,
    closed: Vec<(std::time::Instant, SessionHandle)>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

fn session<T: InvokeUiSession>(core: &Session<T>) -> Option<SessionHandle> {
    let kind = match core.lc.read().unwrap().conn_type {
        ConnType::DEFAULT_CONN => SessionKind::Desktop,
        ConnType::TERMINAL => SessionKind::Terminal,
        ConnType::FILE_TRANSFER => SessionKind::FileTransfer,
        _ => return None,
    };
    let key = Arc::as_ptr(&core.connection_round_state) as usize;
    let mut registry = registry().lock().unwrap();
    let cache = registry.cache.clone();
    Some(
        registry
            .entries
            .entry(key)
            .or_insert_with(|| Entry {
                core: Arc::downgrade(&core.connection_round_state),
                session: super::gui::registering()
                    .unwrap_or_else(|| SessionHandle::new(core.get_id(), kind, cache)),
            })
            .session
            .clone(),
    )
}

pub(crate) fn add_view<T: InvokeUiSession>(core: &Session<T>, view: &Uuid) {
    if let Some(session) = session(core) {
        let mut state = session.0.state.lock().unwrap();
        if state.snapshot.ui_session_ids.insert(view.to_string()) {
            session.notify(&mut state);
        }
    }
}

pub(crate) fn remove_view<T: InvokeUiSession>(core: &Session<T>, view: &Uuid) {
    super::gui::forget_view(view);
    let key = Arc::as_ptr(&core.connection_round_state) as usize;
    let mut registry = registry().lock().unwrap();
    let Some(entry) = registry.entries.get(&key) else {
        return;
    };
    let mut state = entry.session.0.state.lock().unwrap();
    if !state.snapshot.ui_session_ids.remove(&view.to_string()) {
        return;
    }
    let closed = state.snapshot.ui_session_ids.is_empty();
    if closed {
        super::subscriptions::forget(&state.snapshot.session_id);
        super::input::forget(&state.snapshot.session_id);
        super::terminals::disconnected(
            &state.snapshot.session_id,
            entry.session.control().binding_id(),
        );
        entry
            .session
            .0
            .control
            .disconnected(state.snapshot.connection_epoch, true);
        state.snapshot.state = ConnectionState::Closed;
        state.snapshot.authenticated = false;
        state.snapshot.auth_challenge = None;
        state.snapshot.permissions.clear();
        state.capture_enabled = false;
        entry.session.invalidate_frames(&mut state);
    }
    entry.session.notify(&mut state);
    drop(state);
    if closed {
        if let Some(entry) = registry.entries.remove(&key) {
            if entry.session.control().binding_id().is_some() {
                registry
                    .closed
                    .push((std::time::Instant::now(), entry.session));
            }
        }
    }
}

pub(crate) fn begin<T: InvokeUiSession>(core: &Session<T>, round: u32) -> Option<Connection> {
    if core
        .connection_round_state
        .lock()
        .unwrap()
        .is_round_gt(round)
    {
        return None;
    }
    let key = Arc::as_ptr(&core.connection_round_state) as usize;
    let handle = registry()
        .lock()
        .unwrap()
        .entries
        .get(&key)?
        .session
        .clone();
    handle.begin(u64::from(round))
}

pub(crate) fn registered() -> Vec<SessionHandle> {
    let mut registry = registry().lock().unwrap();
    let mut closed = Vec::new();
    registry.entries.retain(|_, entry| {
        if entry.core.strong_count() == 0 {
            let mut state = entry.session.0.state.lock().unwrap();
            super::subscriptions::forget(&state.snapshot.session_id);
            super::input::forget(&state.snapshot.session_id);
            super::terminals::disconnected(
                &state.snapshot.session_id,
                entry.session.control().binding_id(),
            );
            entry
                .session
                .control()
                .disconnected(state.snapshot.connection_epoch, true);
            state.snapshot.state = ConnectionState::Closed;
            state.snapshot.authenticated = false;
            state.snapshot.auth_challenge = None;
            state.snapshot.permissions.clear();
            state.capture_enabled = false;
            entry.session.invalidate_frames(&mut state);
            entry.session.notify(&mut state);
            if entry.session.control().binding_id().is_some() {
                closed.push((std::time::Instant::now(), entry.session.clone()));
            }
            false
        } else {
            true
        }
    });
    registry.closed.extend(closed);
    registry
        .entries
        .values()
        .map(|e| e.session.clone())
        .collect()
}

pub fn list() -> Vec<SessionHandle> {
    let mut sessions = registered();
    sessions.extend(super::gui::placeholders());
    {
        let mut registry = registry().lock().unwrap();
        registry
            .closed
            .retain(|(at, _)| at.elapsed().as_secs() < 60);
        sessions.extend(registry.closed.iter().map(|(_, s)| s.clone()));
    }
    sessions
}

pub(crate) fn shared_cache() -> Arc<Mutex<FrameCache>> {
    registry().lock().unwrap().cache.clone()
}

pub fn get(session_id: &str) -> Option<SessionHandle> {
    list()
        .into_iter()
        .find(|s| s.snapshot().session_id == session_id)
}

pub(crate) fn for_core<T: InvokeUiSession>(core: &Session<T>) -> Option<SessionHandle> {
    registry()
        .lock()
        .unwrap()
        .entries
        .get(&(Arc::as_ptr(&core.connection_round_state) as usize))
        .map(|entry| entry.session.clone())
}

pub fn for_view(view: &Uuid) -> Option<SessionHandle> {
    let core = crate::flutter::sessions::get_session_by_session_id(view)?;
    for_core(&core)
}

pub fn core(session_id: &str) -> Option<Arc<Session<crate::flutter::FlutterHandler>>> {
    get(session_id)?
        .snapshot()
        .ui_session_ids
        .iter()
        .find_map(|view| {
            Uuid::parse_str(view)
                .ok()
                .and_then(|view| crate::flutter::sessions::get_session_by_session_id(&view))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hbb_common::tokio;
    use scrap::{ImageFormat, ImageRgb};

    fn fixture() -> (SessionHandle, PeerInfo, ImageRgb) {
        let session =
            SessionHandle::new("peer".to_owned(), SessionKind::Desktop, Default::default());
        let mut peer = PeerInfo::new();
        let mut display = DisplayInfo::new();
        display.x = -1920;
        display.width = 1;
        display.height = 1;
        peer.displays.push(display);
        let image = ImageRgb {
            raw: vec![1, 2, 3, 255],
            w: 1,
            h: 1,
            fmt: ImageFormat::ARGB,
            align: 0,
        };
        (session, peer, image)
    }

    #[test]
    fn authentication_and_first_frame_are_separate() {
        let (session, peer, image) = fixture();
        let connection = session.begin(0).unwrap();
        session.set_capture_enabled(true);
        connection.frame(0, &image, true);
        assert_eq!(session.snapshot().state, ConnectionState::Connecting);
        assert!(session.read_frame(0, None).is_err());
        connection.authenticated(&peer);
        assert_eq!(session.snapshot().state, ConnectionState::AwaitingFrame);
        assert!(session.snapshot().permissions.is_empty());
        connection.frame(0, &image, true);
        assert_eq!(session.snapshot().state, ConnectionState::Ready);
        assert_eq!(session.snapshot().displays[0].x, -1920);
        let revision = session.snapshot().revision;
        connection.frame(0, &image, true);
        assert_eq!(session.snapshot().revision, revision);
    }

    #[test]
    fn desktop_authentication_applies_protocol_permission_defaults_but_keeps_denials() {
        let (session, mut peer, _) = fixture();
        peer.platform = "Windows".into();
        peer.version = "1.4.9".into();
        let connection = session.begin(0).unwrap();
        assert!(session.snapshot().permissions.get("keyboard").is_none());
        connection.authenticated(&peer);
        assert_eq!(session.snapshot().permissions.get("keyboard"), Some(&true));
        assert_eq!(session.snapshot().permissions.get("restart"), Some(&true));
        assert_eq!(session.snapshot().peer_version.as_deref(), Some("1.4.9"));
        let next = session.begin(1).unwrap();
        assert!(session.snapshot().peer_version.is_none());
        next.permission(&PermissionInfo {
            permission: hbb_common::message_proto::permission_info::Permission::Keyboard.into(),
            enabled: false,
            ..Default::default()
        });
        next.permission(&PermissionInfo {
            permission: hbb_common::message_proto::permission_info::Permission::Restart.into(),
            enabled: false,
            ..Default::default()
        });
        next.authenticated(&peer);
        assert_eq!(session.snapshot().permissions.get("keyboard"), Some(&false));
        assert_eq!(session.snapshot().permissions.get("restart"), Some(&false));
    }

    #[test]
    fn geometry_update_does_not_erase_a_preceding_switch_mode_report() {
        let (session,mut peer,_)=fixture();
        peer.displays[0].online=true;
        let connection=session.begin(0).unwrap();connection.authenticated(&peer);
        connection.switch_display(&SwitchDisplay {
            display:0,width:3840,height:2160,
            resolutions:Some(hbb_common::message_proto::SupportedResolutions {
                resolutions:vec![hbb_common::message_proto::Resolution{width:1920,height:1080,..Default::default()}],..Default::default()
            }).into(),..Default::default()
        });
        peer.displays[0].width=3840;peer.displays[0].height=2160;
        connection.layout(&peer.displays);
        assert_eq!(session.snapshot().resolutions[&0],vec![(1920,1080)]);
        peer.displays[0].name="different monitor".into();
        connection.layout(&peer.displays);
        assert!(!session.snapshot().resolutions.contains_key(&0));
    }

    #[test]
    fn display_platform_updates_preserve_installation_facts_and_clear_removed_monitors() {
        let (session,mut peer,_)=fixture();
        peer.platform_additions=r#"{"is_installed":true,"idd_impl":"amyuni_idd","amyuni_virtual_displays":1}"#.into();
        let connection=session.begin(0).unwrap();connection.authenticated(&peer);
        connection.platform_additions(r#"{"idd_impl":"amyuni_idd","amyuni_virtual_displays":2}"#);
        assert_eq!(session.snapshot().platform_additions["is_installed"],true);
        assert_eq!(session.snapshot().platform_additions["amyuni_virtual_displays"],2);
        connection.platform_additions("{}");
        assert_eq!(session.snapshot().platform_additions["is_installed"],true);
        assert!(session.snapshot().platform_additions["amyuni_virtual_displays"].is_null());
    }

    #[test]
    fn selection_invalidates_same_geometry_frames_and_retains_modes_until_reconnect() {
        let (session, mut peer, image) = fixture();
        peer.resolutions = Some(hbb_common::message_proto::SupportedResolutions {
            resolutions: vec![hbb_common::message_proto::Resolution { width:1920,height:1080,..Default::default() }],
            ..Default::default()
        }).into();
        session.set_capture_enabled(true);
        let connection=session.begin(0).unwrap();connection.authenticated(&peer);
        connection.frame(0,&image,true);
        let before=connection.layout_revision();
        session.selection_changed();
        connection.frame_at_layout(0,&image,true,before);
        assert_eq!(session.read_frame(0,None).err(),Some(FrameError::NoFrame));
        assert_eq!(session.snapshot().resolutions[&0],vec![(1920,1080)]);
        connection.frame(0,&image,true);
        assert!(session.read_frame(0,None).is_ok());
        session.begin(1).unwrap();
        assert!(session.snapshot().resolutions.is_empty());
        assert!(session.snapshot().platform_additions.is_null());
    }

    #[test]
    fn connection_errors_survive_disconnect_and_reconnect_rejects_stale_errors() {
        let (session, peer, _) = fixture();
        for (epoch, error) in ["Remote desktop is offline", "Timeout", "Reset by the peer", "Connection rejected"]
            .iter().enumerate()
        {
            let connection = session.begin(epoch as u64).unwrap();
            assert!(session.snapshot().last_error.is_none());
            connection.connection_error(error);
            connection.disconnected();
            let snapshot = session.snapshot();
            assert_eq!(snapshot.state, ConnectionState::Disconnected);
            assert_eq!(snapshot.last_error.as_deref(), Some(*error));
            let summary = super::super::api::view(&session, false);
            assert_eq!(summary["connection"]["error"], *error);
            assert_eq!(summary["connection"]["state"], "disconnected");
            connection.connection_error("late error after disconnect");
            assert_eq!(session.snapshot().last_error.as_deref(), Some(*error));
        }
        let old = session.begin(4).unwrap();
        old.connection_error("old attempt failed");
        let current = session.begin(5).unwrap();
        old.connection_error("late error from old attempt");
        old.disconnected();
        assert!(session.snapshot().last_error.is_none());
        assert_eq!(session.snapshot().state, ConnectionState::Connecting);
        current.authenticated(&peer);
        current.disconnected();
        assert!(session.snapshot().last_error.is_none());
    }

    #[test]
    fn connection_error_text_is_bounded_by_characters() {
        let (session, _, _) = fixture();
        let connection = session.begin(0).unwrap();
        connection.connection_error(&"错".repeat(1100));
        assert_eq!(session.snapshot().last_error.unwrap(), "错".repeat(1024));
    }

    #[test]
    fn reconnect_rejects_old_decoder_and_old_disconnect() {
        let (session, peer, image) = fixture();
        session.set_capture_enabled(true);
        let old = session.begin(0).unwrap();
        old.authenticated(&peer);
        old.frame(0, &image, true);
        let current = session.begin(1).unwrap();
        old.frame(0, &image, true);
        old.disconnected();
        assert_eq!(session.snapshot().state, ConnectionState::Connecting);
        assert!(session.read_frame(0, None).is_err());
        current.authenticated(&peer);
        current.frame(0, &image, true);
        assert_eq!(
            session
                .read_frame(0, None)
                .unwrap()
                .frame
                .stamp
                .connection_epoch,
            1
        );
    }

    #[test]
    fn layout_invalidates_pixels_and_disconnect_marks_them_stale() {
        let (session, mut peer, image) = fixture();
        session.set_capture_enabled(true);
        let connection = session.begin(0).unwrap();
        connection.authenticated(&peer);
        connection.frame(0, &image, true);
        peer.displays[0].x = 0;
        connection.layout(&peer.displays);
        assert_eq!(session.snapshot().state, ConnectionState::AwaitingFrame);
        assert!(session.read_frame(0, None).is_err());
        connection.frame(0, &image, true);
        connection.disconnected();
        connection.frame(0, &image, true);
        assert_eq!(session.snapshot().state, ConnectionState::Disconnected);
        let cached = session.read_frame(0, None).unwrap();
        assert!(cached.disconnected);
        assert!(cached.is_stale());
        assert_eq!(cached.newer_than_cursor, None);
    }

    #[test]
    fn queued_old_layout_pixels_cannot_repopulate_new_layout() {
        let (session, mut peer, image) = fixture();
        session.set_capture_enabled(true);
        let connection = session.begin(0).unwrap();
        connection.authenticated(&peer);
        let old_layout = connection.layout_revision();
        connection.frame_at_layout(0, &image, true, old_layout);
        peer.displays[0].x = 20;
        connection.layout(&peer.displays);
        connection.frame_at_layout(0, &image, true, old_layout);
        assert_eq!(session.read_frame(0, None).err(), Some(FrameError::NoFrame));
        assert_eq!(session.snapshot().state, ConnectionState::AwaitingFrame);
        connection.frame_at_layout(0, &image, true, connection.layout_revision());
        assert_eq!(
            session
                .read_frame(0, None)
                .unwrap()
                .frame
                .stamp
                .layout_revision,
            connection.layout_revision()
        );
    }

    #[tokio::test]
    async fn capture_returns_png_and_honest_freshness_without_consuming_gui_pixels() {
        use crate::automation::capture::{capture, CaptureOptions};
        let (session, peer, image) = fixture();
        session.set_capture_enabled(true);
        let connection = session.begin(0).unwrap();
        connection.authenticated(&peer);
        connection.frame(0, &image, true);
        let output = capture(&session, 0, CaptureOptions::default())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            image::load_from_memory(&output.png)
                .unwrap()
                .to_rgb8()
                .as_raw(),
            &[3, 2, 1]
        );
        assert_eq!(image.raw, [1, 2, 3, 255]);
        assert_eq!(output.remote_rect.x, -1920);
        assert!(!output.is_stale);
        assert!(capture(
            &session,
            0,
            CaptureOptions {
                after_frame_seq: Some(output.stamp.sequence),
                ..Default::default()
            }
        )
        .await
        .unwrap()
        .is_none());
        connection.disconnected();
        let old = capture(&session, 0, CaptureOptions::default())
            .await
            .unwrap()
            .unwrap();
        assert!(old.disconnected && old.is_stale);
        session.begin(1).unwrap();
        assert!(capture(&session, 0, CaptureOptions::default())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn display_captures_keep_their_pixels_and_geometry_separate() {
        use crate::automation::capture::{capture, CaptureOptions};
        let (session, mut peer, mut image) = fixture();
        let mut second = peer.displays[0].clone();
        second.x = 100;
        second.y = -200;
        peer.displays.push(second);
        session.set_capture_enabled(true);
        let connection = session.begin(0).unwrap();
        connection.authenticated(&peer);
        connection.frame(0, &image, true);
        image.raw = vec![9, 8, 7, 255];
        connection.frame(1, &image, true);
        let first = capture(&session, 0, CaptureOptions::default())
            .await
            .unwrap()
            .unwrap();
        let second = capture(&session, 1, CaptureOptions::default())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.remote_point(0, 0), Some((-1920, 0)));
        assert_eq!(second.remote_point(0, 0), Some((100, -200)));
        assert_eq!(
            image::load_from_memory(&first.png)
                .unwrap()
                .to_rgb8()
                .as_raw(),
            &[3, 2, 1]
        );
        assert_eq!(
            image::load_from_memory(&second.png)
                .unwrap()
                .to_rgb8()
                .as_raw(),
            &[7, 8, 9]
        );
        assert_ne!(first.stamp.display_id, second.stamp.display_id);
        assert!(session.capture_state(first.stamp).is_ok());
        session.set_capture_enabled(false);
        assert_eq!(
            session.capture_state(first.stamp),
            Err(FrameError::CaptureDisabled)
        );
    }

    #[test]
    fn terminal_readiness_does_not_require_desktop_pixels() {
        let session = SessionHandle::new(
            "terminal".to_owned(),
            SessionKind::Terminal,
            Default::default(),
        );
        let connection = session.begin(0).unwrap();
        let mut peer = PeerInfo::new();
        let mut features = hbb_common::message_proto::Features::new();
        features.terminal = true;
        peer.features = Some(features).into();
        connection.authenticated(&peer);
        assert!(session.snapshot().authenticated);
        assert_eq!(session.snapshot().state, ConnectionState::Ready);
        assert!(session.snapshot().displays.is_empty());
    }

    #[test]
    fn bad_pixels_and_textures_do_not_claim_cpu_readiness() {
        let (session, peer, mut image) = fixture();
        let connection = session.begin(0).unwrap();
        connection.authenticated(&peer);
        connection.frame(0, &image, false);
        assert_eq!(
            session.snapshot().frame_error,
            Some(FrameError::TextureOnly)
        );
        image.raw.truncate(2);
        connection.frame(0, &image, true);
        assert_eq!(
            session.snapshot().frame_error,
            Some(FrameError::InvalidPixels)
        );
        assert_eq!(session.snapshot().state, ConnectionState::AwaitingFrame);
    }

    #[test]
    fn switching_to_texture_only_drops_old_cpu_cache_and_wakes_readers() {
        let (session, peer, image) = fixture();
        session.set_capture_enabled(true);
        let connection = session.begin(0).unwrap();
        connection.authenticated(&peer);
        connection.frame(0, &image, true);
        assert!(session.read_frame(0, None).is_ok());
        let frames = session.subscribe_frames();
        connection.frame(0, &image, false);
        assert!(frames.has_changed().unwrap());
        assert_eq!(session.read_frame(0, None).err(), Some(FrameError::NoFrame));
    }

    #[test]
    fn state_and_frame_waiters_have_separate_revisions() {
        let (session, peer, image) = fixture();
        let mut state_changes = session.subscribe();
        let mut frames = session.subscribe_frames();
        session.set_capture_enabled(true);
        let connection = session.begin(0).unwrap();
        connection.authenticated(&peer);
        connection.frame(0, &image, true);
        state_changes.borrow_and_update();
        frames.borrow_and_update();
        connection.frame(0, &image, true);
        assert!(!state_changes.has_changed().unwrap());
        assert!(frames.has_changed().unwrap());
        frames.borrow_and_update();
        session.set_capture_enabled(false);
        assert!(frames.has_changed().unwrap());
        assert_eq!(
            session.read_frame(0, None).err(),
            Some(FrameError::CaptureDisabled)
        );
    }

    #[test]
    fn bound_closed_sessions_keep_readable_completion_records() {
        for drop_core in [false, true] {
            let core: Session<crate::flutter::FlutterHandler> = Default::default();
            let view = Uuid::new_v4();
            add_view(&core, &view);
            let handle = session(&core).unwrap();
            let agent = super::super::control::Agent::new();
            handle.control().attach(&agent, false).unwrap();
            let reference = handle.control().view().session_ref.unwrap();
            if !drop_core {
                remove_view(&core, &view);
            }
            drop(core);
            let closed = get(&handle.snapshot().session_id).unwrap();
            assert_eq!(closed.snapshot().state, ConnectionState::Closed);
            assert!(closed.control().resolve(&agent, &reference, false).is_ok());
            assert!(closed.control().resolve(&agent, &reference, true).is_err());
            closed.control().release(None, true).unwrap();
        }
    }
    #[test]
    fn multiple_views_share_one_identity_and_last_close_fences_callbacks() {
        let core: Session<crate::flutter::FlutterHandler> = Default::default();
        let view1 = Uuid::new_v4();
        let view2 = Uuid::new_v4();
        add_view(&core, &view1);
        add_view(&core.clone(), &view2);
        let handle = session(&core).unwrap();
        let id = handle.snapshot().session_id;
        assert_eq!(get(&id).unwrap().snapshot().ui_session_ids.len(), 2);
        let connection = begin(&core, 0).unwrap();
        remove_view(&core, &view1);
        assert_eq!(get(&id).unwrap().snapshot().ui_session_ids.len(), 1);
        remove_view(&core, &view2);
        assert!(get(&id).is_none());
        assert!(begin(&core, 1).is_none());
        let (_, peer, image) = fixture();
        connection.authenticated(&peer);
        connection.frame(0, &image, true);
        assert_eq!(handle.snapshot().state, ConnectionState::Closed);
    }
}
