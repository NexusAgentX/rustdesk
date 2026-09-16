use super::{
    control::Permit,
    error::{BridgeError, Result},
    sessions,
};
use crate::client::Data;
use hbb_common::{
    message_proto::{
        terminal_action, terminal_response, CloseTerminal, Message, OpenTerminal, ResizeTerminal,
        TerminalAction, TerminalData, TerminalResponse,
    },
    tokio::sync::watch,
};
use serde::Serialize;
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicI32, Ordering},
        Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
use uuid::Uuid;

const PER_TERMINAL: usize = 4 * 1024 * 1024;
const TOTAL: usize = 64 * 1024 * 1024;
static NEXT_ID: AtomicI32 = AtomicI32::new(1_000_000);
static ARCHIVE_ID: AtomicI32 = AtomicI32::new(-1);

#[derive(Clone, Serialize)]
pub struct TerminalState {
    pub terminal_id: String,
    pub state: String,
    pub requested_size: Size,
    pub pid: Option<u32>,
    pub shell_exit_code: Option<i32>,
    pub error: Option<String>,
    pub stream_epoch: String,
    pub replay: bool,
}
#[derive(Clone, Copy, Serialize)]
pub struct Size {
    pub rows: u32,
    pub cols: u32,
}
struct Chunk {
    bytes: Vec<u8>,
    at: Instant,
}
struct Entry {
    state: TerminalState,
    official: i32,
    epoch: u64,
    chunks: VecDeque<Chunk>,
    oldest: u64,
    end: u64,
    closed_at: Option<Instant>,
    closed_owner: Option<String>,
    created: Instant,
    visible: bool,
    cancelled_intent: bool,
    intent: Option<Permit>,
    changed: watch::Sender<u64>,
    revision: u64,
}
#[derive(Default)]
struct Hub {
    entries: HashMap<(String, i32), Entry>,
    bytes: usize,
}
fn hub() -> &'static Mutex<Hub> {
    static HUB: OnceLock<Mutex<Hub>> = OnceLock::new();
    HUB.get_or_init(Default::default)
}

pub fn validate_size(rows: u32, cols: u32) -> Result<()> {
    if !(1..=500).contains(&rows) || !(1..=1000).contains(&cols) {
        return Err(BridgeError::invalid(
            "Terminal rows must be 1..500 and cols 1..1000",
        ));
    }
    Ok(())
}
fn entry(official: i32, epoch: u64, size: Size, intent: Option<Permit>) -> Entry {
    let (changed, _) = watch::channel(0);
    Entry {
        state: TerminalState {
            terminal_id: format!("t_{}", Uuid::new_v4()),
            state: "opening".into(),
            requested_size: size,
            pid: None,
            shell_exit_code: None,
            error: None,
            stream_epoch: Uuid::new_v4().to_string(),
            replay: false,
        },
        official,
        epoch,
        chunks: VecDeque::new(),
        oldest: 0,
        end: 0,
        closed_at: None,
        closed_owner: None,
        created: Instant::now(),
        visible: false,
        cancelled_intent: false,
        intent,
        changed,
        revision: 0,
    }
}
pub fn prepare(permit: Permit, rows: u32, cols: u32) -> Result<(TerminalState, i32)> {
    validate_size(rows, cols)?;
    let mut hub = hub().lock().unwrap();
    if hub.entries.len() >= 256 {
        return Err(BridgeError::new(
            "LIMIT_EXCEEDED",
            "Too many observed terminals",
        ));
    }
    let official = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    if official < 0 {
        return Err(BridgeError::new(
            "LIMIT_EXCEEDED",
            "Terminal identifiers exhausted",
        ));
    }
    let session_id = permit.authority.session_id.clone();
    let entry = entry(official, permit.epoch, Size { rows, cols }, Some(permit));
    let result = entry.state.clone();
    hub.entries.insert((session_id, official), entry);
    Ok((result, official))
}
pub fn open_from_gui(view: Uuid, official: i32, rows: u32, cols: u32) -> bool {
    let Some(session) = sessions::for_view(&view) else {
        return false;
    };
    let snapshot = session.snapshot();
    let mut hub = hub().lock().unwrap();
    let key = (snapshot.session_id.clone(), official);
    if hub.entries.get(&key).is_some_and(|e| e.cancelled_intent) {
        return true;
    }
    if hub.entries.get(&key).is_some_and(|e| {
        e.closed_at.is_some() || (e.epoch != 0 && e.epoch != snapshot.connection_epoch)
    }) {
        if let Some(previous) = hub.entries.remove(&key) {
            hub.entries.insert(
                (
                    snapshot.session_id.clone(),
                    ARCHIVE_ID.fetch_sub(1, Ordering::Relaxed),
                ),
                previous,
            );
        }
    }
    if !hub.entries.contains_key(&key) {
        if hub.entries.len() >= 256 {
            return false;
        }
        hub.entries.insert(
            key.clone(),
            entry(
                official,
                snapshot.connection_epoch,
                Size { rows, cols },
                None,
            ),
        );
    }
    let Some(terminal) = hub.entries.get_mut(&key) else {
        return false;
    };
    let Some(mut permit) = terminal.intent.take() else {
        terminal.state.requested_size = Size { rows, cols };
        return false;
    };
    if permit.epoch == 0 {
        permit.epoch = snapshot.connection_epoch;
        terminal.epoch = snapshot.connection_epoch;
    }
    if let Err(error) = permit.check() {
        terminal.state.state = "failed".into();
        terminal.state.error = Some(error.message);
        terminal.closed_at = Some(Instant::now());
        terminal.closed_owner = Some(permit.binding_id().to_owned());
        terminal.revision += 1;
        terminal.changed.send_replace(terminal.revision);
        return true;
    }
    terminal.closed_owner = Some(permit.binding_id().to_owned());
    let size = terminal.state.requested_size;
    drop(hub);
    let Some(core) = sessions::core(&snapshot.session_id) else {
        return true;
    };
    let mut action = TerminalAction::new();
    action.set_open(OpenTerminal {
        terminal_id: official,
        rows: size.rows,
        cols: size.cols,
        ..Default::default()
    });
    let mut message = Message::new();
    message.set_terminal_action(action);
    if let Some(sender) = core.sender.read().unwrap().as_ref() {
        if sender
            .send(Data::Automation(super::wire::Envelope {
                permit,
                message,
                mapping: None,
                release: false,
                reply: Default::default(),
                active: None,
            }))
            .is_err()
        {
            fail(&snapshot.session_id, official, "Terminal sender closed");
        }
    } else {
        fail(
            &snapshot.session_id,
            official,
            "Terminal sender is unavailable",
        );
    }
    true
}
pub fn fail(session: &str, official: i32, error: &str) {
    if let Some(entry) = hub()
        .lock()
        .unwrap()
        .entries
        .get_mut(&(session.to_owned(), official))
    {
        entry.state.state = "failed".into();
        entry.state.error = Some(error.to_owned());
        entry.closed_at = Some(Instant::now());
        entry.revision += 1;
        entry.changed.send_replace(entry.revision);
    }
}
pub(crate) fn response(
    session: &str,
    epoch: u64,
    owner: Option<String>,
    response: &TerminalResponse,
) {
    let official = match &response.union {
        Some(terminal_response::Union::Opened(v)) => v.terminal_id,
        Some(terminal_response::Union::Data(v)) => v.terminal_id,
        Some(terminal_response::Union::Closed(v)) => v.terminal_id,
        Some(terminal_response::Union::Error(v)) => v.terminal_id,
        _ => return,
    };
    let mut hub = hub().lock().unwrap();
    let Some(entry) = hub.entries.get_mut(&(session.to_owned(), official)) else {
        return;
    };
    if entry.epoch != epoch {
        return;
    }
    let mut added = 0;
    let mut pre_removed = 0;
    match &response.union {
        Some(terminal_response::Union::Opened(v)) => {
            entry.state.state = if v.success { "open" } else { "failed" }.into();
            entry.state.pid = v.success.then_some(v.pid);
            entry.state.error = (!v.success).then(|| v.message.chars().take(1024).collect());
            entry.state.replay = v.replay_terminal_output;
            if !v.success {
                entry.closed_at = Some(Instant::now());
            }
        }
        Some(terminal_response::Union::Data(v)) => {
            let bytes = v.data.as_ref();
            entry.end += bytes.len() as u64;
            let retained = &bytes[bytes.len().saturating_sub(PER_TERMINAL)..];
            if retained.len() != bytes.len() {
                pre_removed = entry.chunks.iter().map(|c| c.bytes.len()).sum::<usize>();
                entry.chunks.clear();
                entry.oldest = entry.end - retained.len() as u64;
            }
            if !retained.is_empty() {
                entry.chunks.push_back(Chunk {
                    bytes: retained.to_vec(),
                    at: Instant::now(),
                });
            }
            added = retained.len();
        }
        Some(terminal_response::Union::Closed(v)) => {
            entry.state.state = "closed".into();
            entry.state.shell_exit_code = Some(v.exit_code);
            entry.closed_at = Some(Instant::now());
        }
        Some(terminal_response::Union::Error(v)) => {
            entry.state.state = "failed".into();
            entry.state.error = Some(v.message.chars().take(1024).collect());
            entry.closed_at = Some(Instant::now());
        }
        _ => {}
    }
    if entry.closed_at.is_some() {
        entry.closed_owner = owner;
    }
    entry.revision += 1;
    entry.changed.send_replace(entry.revision);
    let removed = trim(entry, PER_TERMINAL) + pre_removed;
    hub.bytes = hub.bytes + added - removed;
    while hub.bytes > TOTAL {
        let oldest = hub
            .entries
            .iter()
            .filter_map(|(key, entry)| entry.chunks.front().map(|chunk| (key.clone(), chunk.at)))
            .min_by_key(|(_, at)| *at)
            .map(|(key, _)| key);
        let Some(key) = oldest else { break };
        let Some(entry) = hub.entries.get_mut(&key) else {
            break;
        };
        let Some(chunk) = entry.chunks.pop_front() else {
            break;
        };
        entry.oldest += chunk.bytes.len() as u64;
        hub.bytes -= chunk.bytes.len();
    }
}
fn trim(entry: &mut Entry, limit: usize) -> usize {
    let mut removed = 0;
    while entry.end - entry.oldest > limit as u64 {
        let excess = (entry.end - entry.oldest - limit as u64) as usize;
        let Some(front) = entry.chunks.front_mut() else {
            break;
        };
        let n = excess.min(front.bytes.len());
        front.bytes = front.bytes[n..].to_vec();
        entry.oldest += n as u64;
        removed += n;
        if front.bytes.is_empty() {
            entry.chunks.pop_front();
        }
    }
    removed
}
pub fn list(session: &str) -> Vec<TerminalState> {
    hub()
        .lock()
        .unwrap()
        .entries
        .iter()
        .filter(|((id, _), _)| id == session)
        .map(|(_, entry)| entry.state.clone())
        .collect()
}
pub fn official(session: &str, terminal: &str) -> Result<i32> {
    hub()
        .lock()
        .unwrap()
        .entries
        .iter()
        .find(|((id, _), entry)| id == session && entry.state.terminal_id == terminal)
        .map(|(_, entry)| entry.official)
        .ok_or_else(|| BridgeError::new("TERMINAL_EXPIRED", "Terminal is absent or expired"))
}
#[derive(Serialize)]
pub struct Output {
    pub terminal: TerminalState,
    pub stream_epoch: String,
    pub start_offset: String,
    pub end_offset: String,
    pub next_cursor: String,
    pub oldest_cursor: String,
    pub has_more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_lossy: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_base64: Option<String>,
}
pub fn read(
    session: &str,
    terminal: &str,
    cursor: Option<&str>,
    max_bytes: usize,
    format: &str,
) -> Result<Output> {
    if !(1..=65536).contains(&max_bytes) || !matches!(format, "text" | "base64" | "both") {
        return Err(BridgeError::invalid("Invalid terminal read size or format"));
    }
    let hub = hub().lock().unwrap();
    let entry = hub
        .entries
        .iter()
        .find(|((id, _), entry)| id == session && entry.state.terminal_id == terminal)
        .map(|(_, entry)| entry)
        .ok_or_else(|| BridgeError::new("TERMINAL_EXPIRED", "Terminal is absent or expired"))?;
    let make_cursor = |offset| format!("{}:{offset}", entry.state.stream_epoch);
    let start = if let Some(cursor) = cursor {
        let (epoch, offset) = cursor
            .rsplit_once(':')
            .ok_or_else(|| BridgeError::invalid("Invalid terminal cursor"))?;
        if epoch != entry.state.stream_epoch {
            return Err(BridgeError::new(
                "OUTPUT_EPOCH_CHANGED",
                "Terminal stream changed",
            ));
        }
        offset
            .parse::<u64>()
            .map_err(|_| BridgeError::invalid("Invalid terminal byte offset"))?
    } else {
        entry.oldest
    };
    if start < entry.oldest {
        return Err(BridgeError::new(
            "OUTPUT_GAP",
            "Output was evicted; resume explicitly at oldest_cursor",
        )
        .details(serde_json::json!({"oldest_cursor":make_cursor(entry.oldest)})));
    }
    if start > entry.end {
        return Err(BridgeError::invalid("Cursor is beyond the terminal output"));
    }
    let end = (start + max_bytes as u64).min(entry.end);
    let bytes = entry
        .chunks
        .iter()
        .flat_map(|chunk| chunk.bytes.iter())
        .skip((start - entry.oldest) as usize)
        .take((end - start) as usize)
        .copied()
        .collect::<Vec<_>>();
    let with_text = format != "base64";
    Ok(Output {
        terminal: entry.state.clone(),
        stream_epoch: entry.state.stream_epoch.clone(),
        start_offset: start.to_string(),
        end_offset: end.to_string(),
        next_cursor: make_cursor(end),
        oldest_cursor: make_cursor(entry.oldest),
        has_more: end < entry.end,
        text: with_text.then(|| String::from_utf8_lossy(&bytes).into_owned()),
        text_lossy: with_text.then(|| std::str::from_utf8(&bytes).is_err()),
        data_base64: (format != "text").then(|| crate::encode64(&bytes)),
    })
}
pub fn subscribe(session: &str, terminal: &str) -> Result<watch::Receiver<u64>> {
    hub()
        .lock()
        .unwrap()
        .entries
        .iter()
        .find(|((id, _), entry)| id == session && entry.state.terminal_id == terminal)
        .map(|(_, entry)| entry.changed.subscribe())
        .ok_or_else(|| BridgeError::new("TERMINAL_EXPIRED", "Terminal is absent or expired"))
}
pub fn tick() {
    let mut hub = hub().lock().unwrap();
    for entry in hub.entries.values_mut() {
        if entry.state.state == "opening"
            && !entry.visible
            && entry.intent.is_some()
            && entry.created.elapsed().as_secs() >= 30
        {
            entry.state.state = "failed".into();
            entry.state.error = Some("Visible terminal tab was not ready within 30 seconds".into());
            entry.closed_at = Some(Instant::now());
            entry.closed_owner = entry.intent.as_ref().map(|p| p.binding_id().to_owned());
            entry.intent = None;
            entry.cancelled_intent = true;
            entry.revision += 1;
            entry.changed.send_replace(entry.revision);
        }
    }
    hub.entries.retain(|_, entry| {
        !entry
            .closed_at
            .is_some_and(|at| at.elapsed() > Duration::from_secs(60))
    });
    hub.bytes = hub
        .entries
        .values()
        .map(|entry| (entry.end - entry.oldest) as usize)
        .sum();
}

pub fn authorize(permit: &Permit, terminal: &str) -> Result<()> {
    permit.read_check()?;
    let hub = hub().lock().unwrap();
    let entry = hub
        .entries
        .iter()
        .find(|((s, _), e)| s == &permit.authority.session_id && e.state.terminal_id == terminal)
        .map(|(_, e)| e)
        .ok_or_else(|| BridgeError::new("TERMINAL_EXPIRED", "Terminal is absent or expired"))?;
    if entry.closed_at.is_some() && entry.closed_owner.as_deref() != Some(permit.binding_id()) {
        return Err(BridgeError::new(
            "TERMINAL_EXPIRED",
            "Completion record belongs to a previous binding",
        ));
    }
    Ok(())
}
pub fn disconnected(session: &str, owner: Option<String>) {
    let mut hub = hub().lock().unwrap();
    for ((id, _), entry) in &mut hub.entries {
        if id == session && entry.closed_at.is_none() {
            entry.state.state = "closed".into();
            entry.state.error =
                Some("Remote connection ended; shell exit status is unknown".into());
            entry.closed_at = Some(Instant::now());
            entry.closed_owner = owner.clone();
            entry.intent = None;
            entry.revision += 1;
            entry.changed.send_replace(entry.revision);
        }
    }
}
pub fn end_cursor(session: &str, terminal: &str) -> Result<String> {
    let output = read(session, terminal, None, 1, "base64")?;
    let hub = hub().lock().unwrap();
    let entry = hub
        .entries
        .values()
        .find(|e| e.state.terminal_id == terminal)
        .ok_or_else(|| BridgeError::new("TERMINAL_EXPIRED", "Terminal expired"))?;
    Ok(format!("{}:{}", output.stream_epoch, entry.end))
}
pub fn update_size(session: &str, terminal: &str, rows: u32, cols: u32) {
    if let Some(entry) = hub()
        .lock()
        .unwrap()
        .entries
        .iter_mut()
        .find(|((s, _), e)| s == session && e.state.terminal_id == terminal)
        .map(|(_, e)| e)
    {
        entry.state.requested_size = Size { rows, cols };
        entry.revision += 1;
        entry.changed.send_replace(entry.revision);
    }
}
pub fn closing(session: &str, terminal: &str) {
    if let Some(entry) = hub()
        .lock()
        .unwrap()
        .entries
        .iter_mut()
        .find(|((s, _), e)| s == session && e.state.terminal_id == terminal)
        .map(|(_, e)| e)
    {
        if entry.state.state == "open" {
            entry.state.state = "closing".into();
            entry.revision += 1;
            entry.changed.send_replace(entry.revision);
        }
    }
}
pub async fn wait_read(
    permit: &Permit,
    terminal: &str,
    cursor: Option<&str>,
    max_bytes: usize,
    format: &str,
    wait_ms: u64,
) -> Result<(Output, bool)> {
    super::api::wait_budget(wait_ms)?;
    authorize(permit, terminal)?;
    let session = &permit.authority.session_id;
    let mut changed = subscribe(session, terminal)?;
    let initial = *changed.borrow();
    let deadline = hbb_common::tokio::time::Instant::now() + Duration::from_millis(wait_ms);
    loop {
        authorize(permit, terminal)?;
        let output = read(session, terminal, cursor, max_bytes, format)?;
        if output.start_offset != output.end_offset
            || matches!(output.terminal.state.as_str(), "closed" | "failed")
            || *changed.borrow() != initial
        {
            return Ok((output, false));
        }
        if wait_ms == 0
            || hbb_common::tokio::time::timeout_at(deadline, changed.changed())
                .await
                .is_err()
        {
            return Ok((output, true));
        }
    }
}
pub async fn wait_open(permit: &Permit, terminal: &str, wait_ms: u64) -> Result<TerminalState> {
    super::api::wait_budget(wait_ms)?;
    let mut changed = subscribe(&permit.authority.session_id, terminal)?;
    let deadline = hbb_common::tokio::time::Instant::now() + Duration::from_millis(wait_ms);
    loop {
        authorize(permit, terminal)?;
        let output = read(&permit.authority.session_id, terminal, None, 1, "base64")?;
        if output.terminal.state != "opening" || wait_ms == 0 {
            return Ok(output.terminal);
        }
        if hbb_common::tokio::time::timeout_at(deadline, changed.changed())
            .await
            .is_err()
        {
            return Ok(output.terminal);
        }
    }
}
pub(crate) fn check_peer(snapshot: &sessions::SessionSnapshot) -> Result<()> {
    if snapshot.kind!=sessions::SessionKind::Terminal {return Err(BridgeError::new("WRONG_SESSION_KIND","A terminal connection is required"));}
    if snapshot.terminal_supported==Some(false) {return Err(BridgeError::new("UNSUPPORTED","Peer reports no terminal support"));}
    if snapshot.terminal_supported!=Some(true) || !snapshot.authenticated || snapshot.state!=sessions::ConnectionState::Ready {return Err(BridgeError::new("NOT_READY","Terminal support and authentication must be negotiated first"));}
    Ok(())
}
pub fn create(permit: Permit, rows: u32, cols: u32) -> Result<TerminalState> {
    permit.check()?;
    let session = sessions::get(&permit.authority.session_id)
        .ok_or_else(|| BridgeError::new("SESSION_CLOSED", "Session is closed"))?;
    super::api::terminal_session(&session)?;
    check_peer(&session.snapshot())?;
    let (state, official) = prepare(permit, rows, cols)?;
    let event=serde_json::json!({"name":"automation_open","request_id":"","peer_id":session.snapshot().peer_id,"kind":"terminal","terminal_id":official.to_string(),"force_relay":"false"}).to_string();
    if crate::flutter::push_global_event(crate::flutter::APP_TYPE_MAIN, event) != Some(true) {
        fail(
            &session.snapshot().session_id,
            official,
            "GUI request channel is unavailable",
        );
        return Err(BridgeError::new(
            "GUI_UNAVAILABLE",
            "Cannot create a visible terminal tab",
        ));
    }
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session() -> String {
        Uuid::new_v4().to_string()
    }
    fn register(session: &str) -> String {
        let e = entry(17, 1, Size { rows: 24, cols: 80 }, None);
        let id = e.state.terminal_id.clone();
        hub()
            .lock()
            .unwrap()
            .entries
            .insert((session.into(), 17), e);
        id
    }
    fn data(session: &str, bytes: Vec<u8>) {
        let mut response = TerminalResponse::new();
        response.set_data(hbb_common::message_proto::TerminalData {
            terminal_id: 17,
            data: bytes.into(),
            ..Default::default()
        });
        super::response(session, 1, Some("binding".into()), &response);
    }
    #[test]
    fn terminal_peer_support_must_be_negotiated_before_creating_tabs() {
        let session=sessions::SessionHandle::new("terminal-test".into(),sessions::SessionKind::Terminal,Default::default());
        let mut snapshot=session.snapshot();
        assert_eq!(check_peer(&snapshot).unwrap_err().code,"NOT_READY");
        snapshot.terminal_supported=Some(false);
        assert_eq!(check_peer(&snapshot).unwrap_err().code,"UNSUPPORTED");
        snapshot.terminal_supported=Some(true);snapshot.authenticated=true;snapshot.state=sessions::ConnectionState::Ready;
        assert!(check_peer(&snapshot).is_ok());
        snapshot.state=sessions::ConnectionState::Disconnected;
        assert_eq!(check_peer(&snapshot).unwrap_err().code,"NOT_READY");
    }
    #[test]
    fn raw_output_is_repeatable_and_preserves_invalid_utf8() {
        let s = session();
        let id = register(&s);
        data(&s, vec![27, b'[', b'3', b'1', b'm', 255, b'\r']);
        let first = read(&s, &id, None, 65536, "both").unwrap();
        let second = read(&s, &id, None, 65536, "both").unwrap();
        assert_eq!(first.data_base64, second.data_base64);
        assert_eq!(first.text_lossy, Some(true));
        assert_eq!(first.end_offset, "7");
        assert!(first.text.unwrap().ends_with('\r'));
    }
    #[test]
    fn oversized_chunk_evicts_exact_bytes_and_reports_recovery_cursor() {
        let s = session();
        let id = register(&s);
        let original = read(&s, &id, None, 1, "base64").unwrap().next_cursor;
        data(&s, vec![b'a'; PER_TERMINAL + 23]);
        let error = read(&s, &id, Some(&original), 1, "text").err().unwrap();
        assert_eq!(error.code, "OUTPUT_GAP");
        let cursor = error.details.unwrap()["oldest_cursor"]
            .as_str()
            .unwrap()
            .to_owned();
        let output = read(&s, &id, Some(&cursor), 10, "text").unwrap();
        assert_eq!(output.start_offset, "23");
        assert_eq!(output.text.unwrap(), "aaaaaaaaaa");
        let hub = hub().lock().unwrap();
        let e = hub.entries.get(&(s, 17)).unwrap();
        assert_eq!(
            e.chunks.iter().map(|c| c.bytes.capacity()).sum::<usize>(),
            PER_TERMINAL
        );
    }
    #[test]
    fn old_epoch_and_disconnect_do_not_invent_a_shell_exit_status() {
        let s = session();
        let id = register(&s);
        let mut response = TerminalResponse::new();
        response.set_closed(hbb_common::message_proto::TerminalClosed {
            terminal_id: 17,
            exit_code: 7,
            ..Default::default()
        });
        super::response(&s, 0, Some("binding".into()), &response);
        assert_eq!(
            read(&s, &id, None, 1, "text")
                .unwrap()
                .terminal
                .shell_exit_code,
            None
        );
        disconnected(&s, Some("binding".into()));
        let terminal = read(&s, &id, None, 1, "text").unwrap().terminal;
        assert_eq!(terminal.state, "closed");
        assert_eq!(terminal.shell_exit_code, None);
    }
    #[test]
    fn shell_exit_code_is_taken_only_from_remote_close_event() {
        let s = session();
        let id = register(&s);
        let mut response = TerminalResponse::new();
        response.set_closed(hbb_common::message_proto::TerminalClosed {
            terminal_id: 17,
            exit_code: 7,
            ..Default::default()
        });
        super::response(&s, 1, Some("binding".into()), &response);
        assert_eq!(
            read(&s, &id, None, 1, "text")
                .unwrap()
                .terminal
                .shell_exit_code,
            Some(7)
        );
    }
}

pub fn view_mounted(view: Uuid, official: i32) {
    if let Some(session) = sessions::for_view(&view) {
        let id = session.snapshot().session_id;
        if let Some(entry) = hub().lock().unwrap().entries.get_mut(&(id, official)) {
            entry.visible = true;
        }
    }
}
pub struct IntentGuard {
    session: String,
    terminal: String,
    armed: bool,
}
impl IntentGuard {
    pub fn new(session: &str, terminal: &str) -> Self {
        Self {
            session: session.into(),
            terminal: terminal.into(),
            armed: true,
        }
    }
    pub fn finish(&mut self) {
        self.armed = false;
    }
}
impl Drop for IntentGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut hub = hub().lock().unwrap();
        if let Some(entry) = hub
            .entries
            .iter_mut()
            .find(|((s, _), e)| s == &self.session && e.state.terminal_id == self.terminal)
            .map(|(_, e)| e)
        {
            if let Some(intent) = entry.intent.take() {
                entry.cancelled_intent = true;
                entry.closed_owner = Some(intent.binding_id().into());
                entry.closed_at = Some(Instant::now());
                entry.state.state = "failed".into();
                entry.state.error = Some("Terminal creation was cancelled before opening".into());
                entry.revision += 1;
                entry.changed.send_replace(entry.revision);
            }
        }
    }
}

pub fn ready(permit: &crate::automation::control::Permit, id: &str) -> Result<i32> {
    permit.check()?;
    authorize(permit, id)?;
    let sid = &permit.authority.session_id;
    let terminal = list(sid)
        .into_iter()
        .find(|t| t.terminal_id == id)
        .ok_or_else(|| BridgeError::new("TERMINAL_NOT_FOUND", "Terminal does not exist"))?;
    if terminal.state != "open" {
        return Err(BridgeError::new(
            "TERMINAL_NOT_OPEN",
            "Terminal is not open",
        ));
    }
    official(sid, id)
}
fn terminal_message(action: TerminalAction) -> Message {
    let mut message = Message::new();
    message.set_terminal_action(action);
    message
}

pub async fn write(permit: Permit, terminal: &str, text: String) -> Result<()> {
    if text.len() > 16384 {
        return Err(BridgeError::invalid("Terminal text exceeds 16 KiB"));
    }
    let mut action = TerminalAction::new();
    action.set_data(TerminalData {
        terminal_id: ready(&permit, terminal)?,
        data: text.into_bytes().into(),
        ..Default::default()
    });
    super::wire::send(permit, terminal_message(action)).await
}
pub async fn resize(permit: Permit, terminal: &str, rows: u32, cols: u32) -> Result<()> {
    validate_size(rows, cols)?;
    let mut action = TerminalAction::new();
    action.set_resize(ResizeTerminal {
        terminal_id: ready(&permit, terminal)?,
        rows,
        cols,
        ..Default::default()
    });
    super::wire::send(permit.clone(), terminal_message(action)).await?;
    update_size(&permit.authority.session_id, terminal, rows, cols);
    Ok(())
}
pub async fn close(permit: Permit, terminal: &str) -> Result<()> {
    let mut action = TerminalAction::new();
    action.set_close(CloseTerminal {
        terminal_id: ready(&permit, terminal)?,
        ..Default::default()
    });
    super::wire::send(permit.clone(), terminal_message(action)).await?;
    closing(&permit.authority.session_id, terminal);
    Ok(())
}
pub fn check_action(permit: &Permit, action: &TerminalAction) -> Result<()> {
    let (id, opening) = match &action.union {
        Some(terminal_action::Union::Open(v)) => (v.terminal_id, true),
        Some(terminal_action::Union::Data(v)) => (v.terminal_id, false),
        Some(terminal_action::Union::Resize(v)) => (v.terminal_id, false),
        Some(terminal_action::Union::Close(v)) => (v.terminal_id, false),
        _ => return Err(BridgeError::invalid("Unsupported terminal action")),
    };
    let hub = hub().lock().unwrap();
    let entry = hub
        .entries
        .get(&(permit.authority.session_id.clone(), id))
        .ok_or_else(|| BridgeError::new("TERMINAL_EXPIRED", "Terminal is absent"))?;
    if entry.epoch != permit.epoch
        || entry.state.state != if opening { "opening" } else { "open" }
        || (opening && !entry.visible)
    {
        return Err(BridgeError::new(
            "TERMINAL_NOT_OPEN",
            "Terminal is not ready for this action",
        ));
    }
    Ok(())
}
pub fn visible_list(session: &str, binding: Option<&str>) -> Vec<TerminalState> {
    hub()
        .lock()
        .unwrap()
        .entries
        .iter()
        .filter(|((id, _), e)| {
            id == session
                && (e.closed_at.is_none()
                    || binding.is_some() && e.closed_owner.as_deref() == binding)
        })
        .map(|(_, e)| e.state.clone())
        .collect()
}
