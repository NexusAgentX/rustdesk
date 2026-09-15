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
    pub platform: Option<String>,
    pub terminal_supported: Option<bool>,
    /// Missing entries are unknown, never permission grants.
    pub permissions: BTreeMap<String, bool>,
    pub displays: Vec<Display>,
    pub current_display: usize,
    pub layout_revision: u64,
    pub revision: u64,
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
    state: Mutex<State>,
    changes: watch::Sender<u64>,
    frames_changed: watch::Sender<u64>,
    cache: Arc<Mutex<FrameCache>>,
}

/// Shared by every GUI view of one core connection. This is not agent authorization.
#[derive(Clone)]
pub struct SessionHandle(Arc<Record>);

impl SessionHandle {
    fn new(peer_id: String, kind: SessionKind, cache: Arc<Mutex<FrameCache>>) -> Self {
        let (changes, _) = watch::channel(0);
        let (frames_changed, _) = watch::channel(0);
        Self(Arc::new(Record {
            state: Mutex::new(State {
                snapshot: SessionSnapshot {
                    session_id: format!("s_{}", Uuid::new_v4()),
                    peer_id,
                    kind,
                    ui_session_ids: BTreeSet::new(),
                    state: ConnectionState::Connecting,
                    connection_epoch: 0,
                    authenticated: false,
                    platform: None,
                    terminal_supported: None,
                    permissions: BTreeMap::new(),
                    displays: vec![],
                    current_display: 0,
                    layout_revision: 0,
                    revision: 0,
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

    pub fn snapshot(&self) -> SessionSnapshot {
        self.0.state.lock().unwrap().snapshot.clone()
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

    fn notify(&self, state: &mut State) {
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
        state.started = true;
        state.snapshot.connection_epoch = epoch;
        state.snapshot.state = ConnectionState::Connecting;
        state.snapshot.authenticated = false;
        state.snapshot.platform = None;
        state.snapshot.terminal_supported = None;
        state.snapshot.permissions.clear();
        state.snapshot.displays.clear();
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

    pub(crate) fn login_error(&self, error: &str) {
        self.update(|state| {
            state.snapshot.authenticated = false;
            state.snapshot.state = if matches!(
                error,
                crate::client::LOGIN_MSG_PASSWORD_EMPTY
                    | crate::client::LOGIN_MSG_PASSWORD_WRONG
                    | crate::client::REQUIRE_2FA
                    | crate::client::LOGIN_MSG_2FA_WRONG
            ) {
                ConnectionState::AwaitingAuth
            } else {
                ConnectionState::AwaitingHuman
            };
            state.snapshot.last_error = Some(error.chars().take(1024).collect());
            self.session.invalidate_frames(state);
        });
    }

    pub(crate) fn authenticated(&self, peer: &PeerInfo) {
        self.update(|state| {
            state.snapshot.authenticated = true;
            state.snapshot.platform = Some(peer.platform.clone());
            state.snapshot.terminal_supported = peer.features.as_ref().map(|f| f.terminal);
            state.snapshot.current_display = peer.current_display as usize;
            state.snapshot.displays = displays(&peer.displays);
            state.snapshot.layout_revision += 1;
            state.snapshot.last_error = None;
            state.snapshot.state = match state.snapshot.kind {
                SessionKind::Terminal if state.snapshot.terminal_supported == Some(true) => {
                    ConnectionState::Ready
                }
                SessionKind::Terminal => ConnectionState::AwaitingHuman,
                SessionKind::Desktop => ConnectionState::AwaitingFrame,
            };
            self.session.invalidate_frames(state);
        });
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

    pub(crate) fn layout(&self, incoming: &[DisplayInfo]) {
        let incoming = displays(incoming);
        self.update(|state| {
            if state.snapshot.displays != incoming {
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
                if *d != before {
                    self.layout_changed(state);
                }
                state.snapshot.current_display = display.display as usize;
            }
        });
    }

    pub(crate) fn disconnected(&self) {
        self.update(|state| {
            state.snapshot.state = ConnectionState::Disconnected;
            state.snapshot.authenticated = false;
            state.snapshot.permissions.clear();
            state.sequence += 1;
            self.session.0.frames_changed.send_replace(state.sequence);
        });
    }

    pub(crate) fn frame(&self, display: usize, image: &scrap::ImageRgb, pixelbuffer: bool) {
        let mut state = self.session.0.state.lock().unwrap();
        if state.snapshot.connection_epoch != self.epoch
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
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

fn session<T: InvokeUiSession>(core: &Session<T>) -> Option<SessionHandle> {
    let kind = match core.lc.read().unwrap().conn_type {
        ConnType::DEFAULT_CONN => SessionKind::Desktop,
        ConnType::TERMINAL => SessionKind::Terminal,
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
                session: SessionHandle::new(core.get_id(), kind, cache),
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
        state.snapshot.state = ConnectionState::Closed;
        state.snapshot.authenticated = false;
        state.snapshot.permissions.clear();
        state.capture_enabled = false;
        entry.session.invalidate_frames(&mut state);
    }
    entry.session.notify(&mut state);
    drop(state);
    if closed {
        registry.entries.remove(&key);
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

pub fn list() -> Vec<SessionHandle> {
    let mut registry = registry().lock().unwrap();
    registry.entries.retain(|_, entry| {
        if entry.core.strong_count() == 0 {
            let mut state = entry.session.0.state.lock().unwrap();
            state.snapshot.state = ConnectionState::Closed;
            state.snapshot.authenticated = false;
            state.snapshot.permissions.clear();
            state.capture_enabled = false;
            entry.session.invalidate_frames(&mut state);
            entry.session.notify(&mut state);
            false
        } else {
            true
        }
    });
    registry
        .entries
        .values()
        .map(|e| e.session.clone())
        .collect()
}

pub fn get(session_id: &str) -> Option<SessionHandle> {
    list()
        .into_iter()
        .find(|s| s.snapshot().session_id == session_id)
}

#[cfg(test)]
mod tests {
    use super::*;
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
