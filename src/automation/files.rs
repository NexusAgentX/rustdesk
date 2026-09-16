//! Controller-side adapters for the stock file-transfer protocol.
use super::{
    api,
    control::Permit,
    error::{BridgeError, Result},
    sessions::{self, ConnectionState, SessionKind},
};
use crate::{
    client::Data,
    ui_session_interface::{InvokeUiSession, Session},
};
use hbb_common::{
    fs,
    message_proto::{FileAction, Message, ReadDir, ReadEmptyDirs},
    tokio::{
        self,
        sync::{oneshot, watch},
    },
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicI32, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct Envelope {
    permit: Permit,
    data: Box<Data>,
    write: bool,
    parent_job: Option<String>,
    active: Arc<AtomicBool>,
    reply: Arc<Mutex<Option<oneshot::Sender<Result<()>>>>>,
}
struct Job {
    permit: Permit,
    native_id: i32,
    value: Value,
    entries: Vec<Value>,
    bytes: usize,
    changed: watch::Sender<u64>,
    updated: Instant,
}
fn jobs() -> &'static Mutex<HashMap<String, Job>> {
    static JOBS: OnceLock<Mutex<HashMap<String, Job>>> = OnceLock::new();
    JOBS.get_or_init(Default::default)
}
fn check(permit: &Permit, write: bool) -> Result<()> {
    if write {
        permit.check()?;
    } else {
        permit.read_check()?;
    }
    let s = sessions::get(&permit.authority.session_id)
        .ok_or_else(|| BridgeError::new("SESSION_CLOSED", "Session is closed"))?
        .snapshot();
    if s.kind != SessionKind::FileTransfer {
        return Err(BridgeError::new(
            "WRONG_SESSION_KIND",
            "Open kind=file_transfer first",
        ));
    }
    if s.connection_epoch != permit.epoch {
        return Err(BridgeError::new("CONTROL_EXPIRED", "Connection changed"));
    }
    if !s.authenticated || s.state != ConnectionState::Ready {
        return Err(BridgeError::new(
            "NOT_READY",
            "File connection is not ready",
        ));
    }
    if s.permissions.get("file") != Some(&true) {
        return Err(BridgeError::new(
            "PERMISSION_DENIED",
            "File permission is not granted",
        ));
    }
    Ok(())
}
pub fn receive(envelope: Envelope) -> Option<Data> {
    let result = if envelope.active.load(Ordering::Acquire) {
        check(&envelope.permit, envelope.write).and_then(|()| {
            if let Some(parent) = &envelope.parent_job { check_parent(&envelope.permit, parent) } else { Ok(()) }
        })
    } else {
        Err(BridgeError::new(
            "CANCELLED",
            "File request cancelled before dispatch",
        ))
    };
    let accepted = result.is_ok();
    if let Some(reply) = envelope.reply.lock().unwrap().take() {
        let _ = reply.send(result);
    }
    accepted.then(|| *envelope.data)
}
async fn send(permit: Permit, data: Data, write: bool) -> Result<()> {
    send_for_parent(permit, data, write, None).await
}
async fn send_for_parent(permit: Permit, data: Data, write: bool, parent_job: Option<String>) -> Result<()> {
    check(&permit, write)?;
    let core = sessions::core(&permit.authority.session_id)
        .ok_or_else(|| BridgeError::new("GUI_UNAVAILABLE", "File window is unavailable"))?;
    let sender = core
        .sender
        .read()
        .unwrap()
        .clone()
        .ok_or_else(|| BridgeError::new("DISCONNECTED", "File sender is unavailable"))?;
    struct Active(Arc<AtomicBool>);
    impl Drop for Active {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    let active = Active(Arc::new(AtomicBool::new(true)));
    let (reply, response) = oneshot::channel();
    sender
        .send(Data::AutomationFile(Envelope {
            permit,
            data: Box::new(data),
            write,
            parent_job,
            active: active.0.clone(),
            reply: Arc::new(Mutex::new(Some(reply))),
        }))
        .map_err(|_| BridgeError::new("DISCONNECTED", "File sender closed"))?;
    response.await.map_err(|_| {
        BridgeError::new(
            "DELIVERY_UNKNOWN",
            "File sender ended before dispatch acknowledgement",
        )
    })?
}
struct DispatchGuard {
    id: String,
    committed: bool,
}
impl Drop for DispatchGuard {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        let target = {
            let mut all = jobs().lock().unwrap();
            all.get_mut(&self.id).and_then(|j| {
                if terminal(j) {
                    return None;
                }
                j.value["state"] = json!("cancelled");
                touch(j);
                Some((j.permit.authority.session_id.clone(), j.native_id))
            })
        };
        if let Some((sid, native)) = target {
            if let Some(core) = sessions::core(&sid) {
                if let Some(sender) = core.sender.read().unwrap().as_ref() {
                    if sender.send(Data::CancelJob(native)).is_err() {
                        hbb_common::log::debug!("File dispatch cleanup sender closed");
                    }
                }
            }
        }
    }
}

fn touch(job: &mut Job) {
    job.updated = Instant::now();
    let revision = job.value["revision"].as_u64().unwrap_or(0) + 1;
    job.value["revision"] = json!(revision);
    job.changed.send_replace(revision);
}
fn terminal(job: &Job) -> bool {
    matches!(
        job.value["state"].as_str(),
        Some("completed" | "failed" | "cancelled" | "interrupted")
    )
}
fn add(permit: &Permit, kind: &str, mut value: Value) -> Result<String> {
    static NEXT: AtomicI32 = AtomicI32::new(1_000_000);
    let native_id = NEXT
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_add(1))
        .map_err(|_| BridgeError::new("LIMIT_EXCEEDED", "File job IDs exhausted"))?;
    let id = format!("file_{}", uuid::Uuid::new_v4());
    value["job_id"] = json!(id);
    value["kind"] = json!(kind);
    value["state"] = json!("queued");
    value["revision"] = json!(0);
    let mut all = jobs().lock().unwrap();
    all.retain(|_, j| !terminal(j) || j.updated.elapsed() < Duration::from_secs(300));
    if all.len() >= 128 {
        return Err(BridgeError::new("LIMIT_EXCEEDED", "File job cache is full"));
    }
    if matches!(kind, "directory" | "empty_directories")
        && all.values().any(|j| {
            j.permit.authority.session_id == permit.authority.session_id
                && matches!(
                    j.value["kind"].as_str(),
                    Some("directory" | "empty_directories")
                )
                && !terminal(j)
        })
    {
        return Err(BridgeError::new(
            "BUSY",
            "Wait for the outstanding directory request before listing another directory",
        ));
    }
    let (changed, _) = watch::channel(0);
    all.insert(
        id.clone(),
        Job {
            permit: permit.clone(),
            native_id,
            value,
            entries: vec![],
            bytes: 0,
            changed,
            updated: Instant::now(),
        },
    );
    Ok(id)
}
fn fail(id: &str, error: &BridgeError) {
    if let Some(j) = jobs().lock().unwrap().get_mut(id) {
        j.value["state"] = json!("failed");
        j.value["error"] = json!(error);
        touch(j);
    }
}
fn authorized(job: &Job, permit: &Permit) -> Result<()> {
    permit.read_check()?;
    if job.permit.authority.session_id != permit.authority.session_id
        || job.permit.binding_id() != permit.binding_id()
    {
        return Err(BridgeError::new(
            "JOB_NOT_FOUND",
            "File job not found in this binding",
        ));
    }
    Ok(())
}
pub fn list(permit: &Permit) -> Result<Value> {
    permit.read_check()?;
    let mut all = jobs().lock().unwrap();
    all.retain(|_, j| !terminal(j) || j.updated.elapsed() < Duration::from_secs(300));
    Ok(json!(all
        .values()
        .filter(|j| authorized(j, permit).is_ok())
        .map(|j| j.value.clone())
        .collect::<Vec<_>>()))
}
pub async fn get(
    permit: &Permit,
    id: &str,
    after: Option<u64>,
    ms: u64,
    offset: usize,
    limit: usize,
) -> Result<Value> {
    api::wait_budget(ms)?;
    if !(1..=1000).contains(&limit) {
        return Err(BridgeError::invalid("limit must be 1..1000"));
    }
    let mut changed = {
        let all = jobs().lock().unwrap();
        let job = all
            .get(id)
            .ok_or_else(|| BridgeError::new("JOB_NOT_FOUND", "File job not found"))?;
        authorized(job, permit)?;
        job.changed.subscribe()
    };
    let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
    loop {
        permit.read_check()?;
        let result = {
            let all = jobs().lock().unwrap();
            let job = all
                .get(id)
                .ok_or_else(|| BridgeError::new("JOB_NOT_FOUND", "File job expired"))?;
            authorized(job, permit)?;
            if ms == 0
                || terminal(job)
                || after.is_some_and(|r| job.value["revision"].as_u64() != Some(r))
                || tokio::time::Instant::now() >= deadline
            {
                let mut value = job.value.clone();
                if value["kind"] == "directory" || value["kind"] == "empty_directories" {
                    value["entries"] = json!(job
                        .entries
                        .iter()
                        .skip(offset)
                        .take(limit)
                        .collect::<Vec<_>>());
                    value["next_offset"] = json!((offset.saturating_add(limit)
                        < job.entries.len())
                    .then(|| offset + limit));
                }
                Some(value)
            } else {
                None
            }
        };
        if let Some(value) = result {
            return Ok(value);
        }
        let _ = tokio::time::timeout_at(deadline, changed.changed()).await;
    }
}
pub fn validate_path(path: &str, empty: bool) -> Result<()> {
    if (!empty && path.is_empty()) || path.len() > 16384 || path.contains('\0') {
        return Err(BridgeError::invalid("Invalid file path"));
    }
    Ok(())
}
pub async fn directory(permit: Permit, path: String, local: bool, hidden: bool) -> Result<String> {
    check(&permit, false)?;
    validate_path(&path, true)?;
    let id = add(
        &permit,
        "directory",
        json!({"path":path,"location":if local {"local"} else {"remote"}}),
    )?;
    let mut guard = DispatchGuard {
        id: id.clone(),
        committed: false,
    };
    if local {
        let target = path.clone();
        let result = tokio::task::spawn_blocking(move || {
            fs::read_dir(&fs::get_path(&target), hidden)
                .map_err(|e| BridgeError::new("FILE_ERROR", e.to_string()))
        })
        .await
        .map_err(|_| BridgeError::new("INTERNAL_ERROR", "Directory worker failed"))?;
        match result {
            Ok(fd) => {
                let value = crate::common::_make_fd_to_json(fd.id, fd.path, &fd.entries);
                complete_directory(&id, &Value::Object(value));
            }
            Err(e) => fail(&id, &e),
        }
    } else {
        let mut action = FileAction::new();
        action.set_read_dir(ReadDir {
            path,
            include_hidden: hidden,
            ..Default::default()
        });
        let mut msg = Message::new();
        msg.set_file_action(action);
        if let Err(e) = send(permit, Data::Message(msg), false).await {
            fail(&id, &e);
        }
    }
    guard.committed = true;
    Ok(id)
}
fn complete_directory(id: &str, value: &Value) {
    let mut all = jobs().lock().unwrap();
    let total: usize = all.values().map(|j| j.bytes).sum();
    if let Some(j) = all.get_mut(id) {
        if terminal(j) {
            return;
        }
        let bytes = value.to_string().len();
        let entries = value["entries"].as_array().cloned().unwrap_or_default();
        if entries.len() > 100_000 || bytes > 8 * 1024 * 1024 || total + bytes > 32 * 1024 * 1024 {
            j.value["state"] = json!("failed");
            j.value["error"] = json!({"code":"LIMIT_EXCEEDED","message":"Directory exceeds 100000 entries, 8 MiB per directory, or 32 MiB total cache"});
        } else {
            j.entries = entries;
            j.bytes = bytes;
            j.value["state"] = json!("completed");
            j.value["path"] = value["path"].clone();
            j.value["entry_count"] = json!(j.entries.len());
        }
        touch(j);
    }
}
// The stock GUI copies empty directories separately from TransferJob file payloads.
fn relative_directory(root: &str, path: &str, windows: bool) -> Result<Vec<String>> {
    let root = if windows {
        root.replace('\\', "/")
    } else {
        root.to_owned()
    };
    let path = if windows {
        path.replace('\\', "/")
    } else {
        path.to_owned()
    };
    let root = root
        .split('/')
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>();
    let path = path
        .split('/')
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>();
    if path.len() < root.len()
        || !root.iter().zip(&path).all(|(a, b)| {
            if windows {
                a.eq_ignore_ascii_case(b)
            } else {
                a == b
            }
        })
    {
        return Err(BridgeError::new(
            "UNSAFE_PATH",
            "Empty directory is outside the requested source",
        ));
    }
    let relative = &path[root.len()..];
    if relative
        .iter()
        .any(|v| matches!(*v, "." | "..") || v.contains(':') || v.contains('\\'))
    {
        return Err(BridgeError::new(
            "UNSAFE_PATH",
            "Unsafe empty directory path",
        ));
    }
    Ok(relative.iter().map(|v| v.to_string()).collect())
}
async fn copy_empty_directories(
    permit: &Permit,
    source: &str,
    destination: &str,
    download: bool,
    hidden: bool,
) -> Result<()> {
    let session = sessions::get(&permit.authority.session_id)
        .ok_or_else(|| BridgeError::new("SESSION_CLOSED", "File session closed"))?;
    let snapshot = session.snapshot();
    let windows = snapshot.platform.as_deref() == Some("Windows");
    let directories = if download {
        let id = add(permit, "empty_directories", json!({"path":source}))?;
        let mut guard = DispatchGuard {
            id: id.clone(),
            committed: false,
        };
        let mut action = FileAction::new();
        action.set_read_empty_dirs(ReadEmptyDirs {
            path: source.into(),
            include_hidden: hidden,
            ..Default::default()
        });
        let mut msg = Message::new();
        msg.set_file_action(action);
        send(permit.clone(), Data::Message(msg), false).await?;
        let result = get(permit, &id, None, 10000, 0, 1000).await?;
        if result["state"] != "completed" {
            return Err(BridgeError::new(
                "DIRECTORY_METADATA_UNAVAILABLE",
                "Could not enumerate remote empty directories; peer must support ReadEmptyDirs",
            ));
        }
        let paths = {
            let all = jobs().lock().unwrap();
            all.get(&id)
                .map(|j| {
                    j.entries
                        .iter()
                        .filter_map(|v| v["path"].as_str().map(str::to_owned))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        guard.committed = true;
        jobs().lock().unwrap().remove(&id);
        paths
    } else {
        let path = source.to_owned();
        tokio::task::spawn_blocking(move || {
            fs::get_empty_dirs_recursive(&path, hidden)
                .map(|dirs| dirs.into_iter().map(|d| d.path).collect::<Vec<_>>())
                .map_err(|e| BridgeError::new("FILE_ERROR", e.to_string()))
        })
        .await
        .map_err(|_| BridgeError::new("INTERNAL_ERROR", "Empty directory worker failed"))??
    };
    if directories.len() > 4096 {
        return Err(BridgeError::new(
            "LIMIT_EXCEEDED",
            "More than 4096 empty directories",
        ));
    }
    for path in directories {
        check(permit, true)?;
        let parts = relative_directory(source, &path, download && windows)?;
        let separator = if !download && windows { "\\" } else { "/" };
        let target = if parts.is_empty() {
            destination.to_owned()
        } else {
            format!(
                "{}{}{}",
                destination.trim_end_matches(['/', '\\']),
                separator,
                parts.join(separator)
            )
        };
        let id = add(permit, "mkdir", json!({"path":target}))?;
        let mut guard = DispatchGuard {
            id: id.clone(),
            committed: false,
        };
        let native = jobs()
            .lock()
            .unwrap()
            .get(&id)
            .map(|j| j.native_id)
            .ok_or_else(|| BridgeError::new("JOB_NOT_FOUND", "Directory job disappeared"))?;
        send(
            permit.clone(),
            Data::CreateDir((native, target, !download)),
            true,
        )
        .await?;
        let result = get(permit, &id, None, 10000, 0, 1).await?;
        if result["state"] != "completed" {
            return Err(BridgeError::new(
                "DIRECTORY_CREATE_FAILED",
                "Empty directory creation was not confirmed",
            )
            .details(json!({"job_id":id,"result":result})));
        }
        guard.committed = true;
        jobs().lock().unwrap().remove(&id);
    }
    Ok(())
}

pub async fn transfer(
    permit: Permit,
    source: String,
    destination: String,
    download: bool,
    hidden: bool,
    conflict: &str,
) -> Result<String> {
    check(&permit, true)?;
    validate_path(&source, false)?;
    validate_path(&destination, false)?;
    let id = add(
        &permit,
        "transfer",
        json!({"source_path":source,"destination_path":destination,"direction":if download {"download"} else {"upload"},"conflict_policy":conflict,"include_hidden":hidden,"finished_bytes":0}),
    )?;
    let mut guard = DispatchGuard {
        id: id.clone(),
        committed: false,
    };
    if let Err(error) =
        copy_empty_directories(&permit, &source, &destination, download, hidden).await
    {
        fail(&id, &error);
        guard.committed = true;
        return Ok(id);
    }
    let native = jobs()
        .lock()
        .unwrap()
        .get(&id)
        .map(|j| j.native_id)
        .ok_or_else(|| BridgeError::new("JOB_NOT_FOUND", "Job expired"))?;
    if let Err(e) = send(
        permit,
        Data::SendFiles((
            native,
            fs::JobType::Generic,
            source,
            destination,
            0,
            hidden,
            download,
        )),
        true,
    )
    .await
    {
        fail(&id, &e);
    }
    guard.committed = true;
    Ok(id)
}
/// Native GUI restoration must not silently restart jobs owned by an MCP binding.
pub fn owns_native<T: InvokeUiSession>(core: &Session<T>, native: i32) -> bool {
    let Some(session) = sessions::for_core(core) else { return false; };
    let sid = session.snapshot().session_id;
    jobs().lock().unwrap().values().any(|j| j.permit.authority.session_id == sid && j.native_id == native)
}
pub async fn resume(permit: Permit, previous_id: &str) -> Result<String> {
    check(&permit, true)?;
    let snapshot = sessions::get(&permit.authority.session_id)
        .ok_or_else(|| BridgeError::new("SESSION_CLOSED", "File session closed"))?.snapshot();
    if !snapshot.peer_version.as_deref().is_some_and(crate::is_support_file_transfer_resume) {
        return Err(BridgeError::new("UNSUPPORTED", "Peer does not support file transfer resume"));
    }
    let previous = {
        let all = jobs().lock().unwrap();
        let j = all.get(previous_id).ok_or_else(|| BridgeError::new("JOB_NOT_FOUND", "Previous file job expired"))?;
        authorized(j, &permit)?;
        if j.value["kind"] != "transfer" || !matches!(j.value["state"].as_str(), Some("interrupted" | "failed" | "cancelled")) {
            return Err(BridgeError::new("NOT_READY", "Only stopped transfer jobs can be resumed"));
        }
        j.value.clone()
    };
    let source = previous["source_path"].as_str().ok_or_else(|| BridgeError::new("INVALID_STATE", "Missing source path"))?.to_owned();
    let destination = previous["destination_path"].as_str().ok_or_else(|| BridgeError::new("INVALID_STATE", "Missing destination path"))?.to_owned();
    let download = previous["direction"] == "download";
    let hidden = previous["include_hidden"].as_bool().unwrap_or(false);
    let id = add(&permit, "transfer", json!({"source_path":source,"destination_path":destination,"direction":previous["direction"],"conflict_policy":previous["conflict_policy"],"include_hidden":hidden,"finished_bytes":0,"resumed_from":previous_id,"resume_mode":"stock_partial_digest"}))?;
    let mut guard = DispatchGuard { id: id.clone(), committed: false };
    if let Err(error) = copy_empty_directories(&permit, &source, &destination, download, hidden).await {
        fail(&id, &error); guard.committed = true; return Ok(id);
    }
    let native = jobs().lock().unwrap().get(&id).map(|j| j.native_id)
        .ok_or_else(|| BridgeError::new("JOB_NOT_FOUND", "Resumed job expired"))?;
    let result = async {
        // Re-enumerate from file zero; stock size/mtime digest matching resumes eligible
        // .download files and skips identical completed files. It is not a content hash.
        send(permit.clone(), Data::AddJob((native, fs::JobType::Generic, source, destination, 0, hidden, download)), true).await?;
        send(permit, Data::ResumeJob((native, download)), true).await
    }.await;
    if let Err(error) = result { fail(&id, &error); }
    guard.committed = true;
    Ok(id)
}
pub async fn cancel(permit: Permit, id: &str) -> Result<()> {
    let native = {
        let all = jobs().lock().unwrap();
        let j = all
            .get(id)
            .ok_or_else(|| BridgeError::new("JOB_NOT_FOUND", "File job not found"))?;
        authorized(j, &permit)?;
        if terminal(j) {
            return Ok(());
        }
        j.native_id
    };
    send(permit, Data::CancelJob(native), true).await?;
    if let Some(j) = jobs().lock().unwrap().get_mut(id) {
        j.value["state"] = json!("cancelled");
        touch(j);
    }
    Ok(())
}
pub async fn confirm(permit: Permit, id: &str, overwrite: bool, remember: bool) -> Result<()> {
    let (native, file_num, upload) = {
        let all = jobs().lock().unwrap();
        let j = all
            .get(id)
            .ok_or_else(|| BridgeError::new("JOB_NOT_FOUND", "File job not found"))?;
        authorized(j, &permit)?;
        if j.value["state"] != "awaiting_conflict" {
            return Err(BridgeError::new("NOT_READY", "No pending conflict"));
        }
        (
            j.native_id,
            j.value["conflict"]["file_num"].as_i64().unwrap_or(0) as i32,
            j.value["direction"] == "upload",
        )
    };
    send(
        permit,
        Data::SetConfirmOverrideFile((native, file_num, overwrite, remember, upload)),
        true,
    )
    .await?;
    if let Some(j) = jobs().lock().unwrap().get_mut(id) {
        if j.value["conflict"]["file_num"].as_i64() == Some(file_num as i64) && !terminal(j) {
            j.value["state"] = json!("running");
            j.value["conflict"] = Value::Null;
        }
        if remember {
            j.value["conflict_policy"] = json!(if overwrite { "overwrite" } else { "skip" });
        }
        touch(j);
    }
    Ok(())
}

pub fn observe(view: &uuid::Uuid, name: &str, event: &HashMap<&str, Value>) {
    if !matches!(
        name,
        "file_dir"
            | "empty_dirs"
            | "job_progress"
            | "job_done"
            | "job_error"
            | "override_file_confirm"
    ) {
        return;
    }
    let Some(session) = sessions::for_view(view) else {
        return;
    };
    let s = session.snapshot();
    if s.kind != SessionKind::FileTransfer {
        return;
    }
    let num = event
        .get("id")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0);
    if name == "empty_dirs" {
        let Some(value) = event
            .get("value")
            .and_then(Value::as_str)
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
        else {
            return;
        };
        let ids = {
            let all = jobs().lock().unwrap();
            all.iter()
                .filter(|(_, j)| {
                    j.permit.authority.session_id == s.session_id
                        && j.permit.epoch == s.connection_epoch
                        && j.value["kind"] == "empty_directories"
                        && !terminal(j)
                        && j.value["path"] == value["path"]
                })
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>()
        };
        for id in ids {
            complete_directory(
                &id,
                &json!({"path":value["path"],"entries":value["empty_dirs"]}),
            );
        }
        return;
    }
    if name == "file_dir" {
        let Some(value) = event
            .get("value")
            .and_then(Value::as_str)
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
        else {
            return;
        };
        if value["id"] != 0 {
            return;
        }
        let id = {
            let all = jobs().lock().unwrap();
            all.iter()
                .find(|(_, j)| {
                    j.permit.authority.session_id == s.session_id
                        && j.permit.epoch == s.connection_epoch
                        && j.value["kind"] == "directory"
                        && j.value["location"] == "remote"
                        && !terminal(j)
                        && (j.value["path"] == "" || j.value["path"] == value["path"])
                })
                .map(|(id, _)| id.clone())
        };
        if let Some(id) = id {
            complete_directory(&id, &value);
        }
        return;
    }
    let mut auto = None;
    {
        let mut all = jobs().lock().unwrap();
        let Some((id, j)) = all.iter_mut().find(|(_, j)| {
            j.permit.authority.session_id == s.session_id
                && j.permit.epoch == s.connection_epoch
                && (j.native_id == num
                    || (num == 0
                        && matches!(
                            j.value["kind"].as_str(),
                            Some("directory" | "empty_directories")
                        )
                        && !terminal(j)))
        }) else {
            return;
        };
        if terminal(j) {
            return;
        }
        match name {
            "job_progress" => {
                if j.value["state"] != "awaiting_conflict" {
                    j.value["state"] = json!("running");
                }
                j.value["finished_bytes"] = json!(event
                    .get("finished_size")
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse::<f64>().ok()));
                j.value["bytes_per_second"] = json!(event
                    .get("speed")
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse::<f64>().ok()));
            }
            "job_done" => j.value["state"] = json!("completed"),
            "job_error" => {
                // Stock TransferJob reports an intentional single-file skip as an error event.
                if event.get("err").and_then(Value::as_str) == Some("skipped") {
                    j.value["state"] = json!("completed");
                    j.value["outcome"] = json!("skipped");
                } else {
                    j.value["state"] = json!("failed");
                    j.value["error"] = json!({"code":"FILE_ERROR","message":event.get("err")});
                }
            }
            "override_file_confirm" => {
                j.value["state"] = json!("awaiting_conflict");
                j.value["conflict"] = json!({"path":event.get("read_path"),"file_num":event.get("file_num").and_then(Value::as_str).and_then(|s|s.parse::<i32>().ok()),"identical":event.get("is_identical").and_then(Value::as_str)==Some("true")});
                if matches!(
                    j.value["conflict_policy"].as_str(),
                    Some("overwrite" | "skip")
                ) {
                    auto = Some((
                        j.permit.clone(),
                        id.clone(),
                        j.value["conflict_policy"] == "overwrite",
                    ));
                }
            }
            _ => {}
        }
        touch(j);
    }
    if let Some((permit, id, overwrite)) = auto {
        hbb_common::tokio::spawn(async move {
            if let Err(e) = confirm(permit, &id, overwrite, true).await {
                fail(&id, &e);
            }
        });
    }
}

// Stop ongoing transfers when the binding, control grant or connection changes.
pub fn reap<T: InvokeUiSession>(core: &Session<T>) {
    let Some(session) = sessions::for_core(core) else {
        return;
    };
    let sid = session.snapshot().session_id;
    let candidates = jobs()
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, j)| j.permit.authority.session_id == sid && !terminal(j))
        .map(|(id, j)| {
            (
                id.clone(),
                j.permit.clone(),
                j.native_id,
                !matches!(
                    j.value["kind"].as_str(),
                    Some("directory" | "empty_directories")
                ),
                j.updated,
            )
        })
        .collect::<Vec<_>>();
    for (id, permit, native, write, updated) in candidates {
        if check(&permit, write).is_err() || (!write && updated.elapsed() > Duration::from_secs(30))
        {
            if let Some(sender) = core.sender.read().unwrap().as_ref() {
                if sender.send(Data::CancelJob(native)).is_err() {
                    hbb_common::log::debug!("File cleanup sender already closed");
                }
            }
            if let Some(j) = jobs().lock().unwrap().get_mut(&id) {
                j.value["state"] = json!("interrupted");
                j.value["error"] = json!({"code":"INTERRUPTED","message":"Control, binding or connection changed, or directory response timed out"});
                touch(j);
            }
        }
    }
}

pub fn disconnected(session_id: &str, epoch: u64) {
    let mut all = jobs().lock().unwrap();
    for job in all.values_mut().filter(|j| {
        j.permit.authority.session_id == session_id && j.permit.epoch == epoch && !terminal(j)
    }) {
        job.value["state"] = json!("interrupted");
        job.value["error"] = json!({"code":"DISCONNECTED","message":"Connection ended; partial destination files may remain"});
        touch(job);
    }
}

#[derive(Clone, Copy, serde::Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ManageAction { CreateDirectory, Rename, RemoveFile, RemoveDirectory }
fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || matches!(name, "." | "..") || name.contains(['/', '\\', ':', '\0']) {
        return Err(BridgeError::invalid("new_name must be a single file name"));
    }
    Ok(())
}
fn validate_manage_path(path: &str, windows: bool) -> Result<()> {
    validate_path(path, false)?;
    let normalized = if windows { path.replace('\\', "/") } else { path.to_owned() };
    let parts = normalized.split('/').filter(|s| !s.is_empty()).collect::<Vec<_>>();
    let absolute = if windows {
        (normalized.as_bytes().get(1) == Some(&b':') && normalized.as_bytes().get(2) == Some(&b'/')) || normalized.starts_with("//")
    } else { normalized.starts_with('/') };
    let root_parts = if windows && normalized.starts_with("//") { 2 } else if windows { 1 } else { 0 };
    if !absolute || parts.len() <= root_parts || parts.iter().any(|s| matches!(*s, "." | "..")) {
        return Err(BridgeError::invalid("Use an absolute path below a filesystem root without dot segments"));
    }
    Ok(())
}
fn check_parent(permit: &Permit, parent: &str) -> Result<()> {
    check(permit, true)?;
    let all = jobs().lock().unwrap();
    let j = all.get(parent).ok_or_else(|| BridgeError::new("JOB_NOT_FOUND", "Management job expired"))?;
    if terminal(j) { return Err(BridgeError::new("CANCELLED", "Management job stopped")); }
    Ok(())
}
async fn management_step(permit: &Permit, parent: &str, action: ManageAction, path: String, local: bool, new_name: &str) -> Result<()> {
    check_parent(permit, parent)?;
    if local {
        let p = permit.clone(); let root = parent.to_owned(); let name = new_name.to_owned();
        tokio::task::spawn_blocking(move || -> Result<()> {
            check_parent(&p, &root)?;
            let result = match action {
                ManageAction::CreateDirectory => fs::create_dir(&path),
                ManageAction::Rename => fs::rename_file(&path, &name),
                ManageAction::RemoveFile => fs::remove_file(&path),
                ManageAction::RemoveDirectory => std::fs::remove_dir(&path).map_err(Into::into),
            };
            result.map_err(|e| BridgeError::new("FILE_ERROR", e.to_string()))
        }).await.map_err(|_| BridgeError::new("INTERNAL_ERROR", "File management worker failed"))??;
    } else {
        let id = add(permit, "management_step", json!({"path":path}))?;
        let mut guard = DispatchGuard { id: id.clone(), committed: false };
        let native = jobs().lock().unwrap().get(&id).map(|j| j.native_id)
            .ok_or_else(|| BridgeError::new("JOB_NOT_FOUND", "Management step expired"))?;
        let data = match action {
            ManageAction::CreateDirectory => Data::CreateDir((native, path, true)),
            ManageAction::Rename => Data::RenameFile((native, path, new_name.into(), true)),
            ManageAction::RemoveFile => Data::RemoveFile((native, path, 0, true)),
            ManageAction::RemoveDirectory => {
                let mut action = FileAction::new();
                // Stock recursive=true only removes empty subdirectories and can hide failures.
                action.set_remove_dir(hbb_common::message_proto::FileRemoveDir { id: native, path, recursive: false, ..Default::default() });
                let mut message = Message::new(); message.set_file_action(action); Data::Message(message)
            }
        };
        send_for_parent(permit.clone(), data, true, Some(parent.to_owned())).await?;
        let result = get(permit, &id, None, 10000, 0, 1).await?;
        if result["state"] != "completed" {
            return Err(BridgeError::new("FILE_ERROR", "File management step failed or its result is unknown").details(result));
        }
        guard.committed = true;
        jobs().lock().unwrap().remove(&id);
    }
    if let Some(job) = jobs().lock().unwrap().get_mut(parent) {
        job.value["completed_steps"] = json!(job.value["completed_steps"].as_u64().unwrap_or(0) + 1);
        touch(job);
    }
    Ok(())
}
async fn manage_run(permit: &Permit, id: &str, action: ManageAction, path: String, local: bool, recursive: bool, new_name: &str) -> Result<()> {
    if !matches!(action, ManageAction::RemoveDirectory) || !recursive {
        return management_step(permit, id, action, path, local, new_name).await;
    }
    let windows = if local { cfg!(windows) } else {
        sessions::get(&permit.authority.session_id).is_some_and(|s| s.snapshot().platform.as_deref() == Some("Windows"))
    };
    let separator = if windows { "\\" } else { "/" };
    // A caller-supplied root must also be a real directory, not a directory symlink.
    let normalized = if windows { path.replace('\\', "/") } else { path.clone() };
    let normalized = normalized.trim_end_matches('/');
    let (parent, leaf) = normalized.rsplit_once('/').ok_or_else(|| BridgeError::invalid("Missing parent directory"))?;
    let parent = format!("{}/", parent);
    let listing = directory(permit.clone(), parent, local, true).await?;
    let result = get(permit, &listing, None, 10000, 0, 1).await?;
    if result["state"] != "completed" {
        return Err(BridgeError::new("FILE_ERROR", "Could not verify recursive deletion root").details(result));
    }
    let entries = jobs().lock().unwrap().remove(&listing).map(|j| j.entries)
        .ok_or_else(|| BridgeError::new("JOB_NOT_FOUND", "Root listing expired"))?;
    let root = entries.iter().find(|e| e["name"].as_str().is_some_and(|name| if windows { name.eq_ignore_ascii_case(leaf) } else { name == leaf }));
    if root.map(|e| e["entry_type"].as_u64()) != Some(Some(0)) {
        return Err(BridgeError::new("UNSAFE_PATH", "Recursive deletion root must be an existing directory, not a symlink"));
    }
    let mut pending = vec![(path, false, 0usize)];
    let mut count = 0;
    while let Some((path, visited, depth)) = pending.pop() {
        check_parent(permit, id)?;
        if visited {
            management_step(permit, id, ManageAction::RemoveDirectory, path, local, "").await?;
            continue;
        }
        if depth > 64 { return Err(BridgeError::new("LIMIT_EXCEEDED", "Directory depth exceeds 64")); }
        let listing = directory(permit.clone(), path.clone(), local, true).await?;
        let result = get(permit, &listing, None, 10000, 0, 1).await?;
        if result["state"] != "completed" {
            return Err(BridgeError::new("FILE_ERROR", "Directory listing failed or timed out").details(result));
        }
        let entries = jobs().lock().unwrap().remove(&listing).map(|j| j.entries)
            .ok_or_else(|| BridgeError::new("JOB_NOT_FOUND", "Directory listing expired"))?;
        count += entries.len();
        if count > 10000 { return Err(BridgeError::new("LIMIT_EXCEEDED", "Recursive deletion exceeds 10000 entries")); }
        pending.push((path.clone(), true, depth));
        for entry in entries {
            let name = entry["name"].as_str().ok_or_else(|| BridgeError::new("INVALID_RESPONSE", "Missing file name"))?;
            validate_name(name)?;
            let child = format!("{}{}{}", path.trim_end_matches(['/', '\\']), separator, name);
            match entry["entry_type"].as_u64() {
                Some(0) => pending.push((child, false, depth + 1)),
                Some(2 | 4 | 5) => management_step(permit, id, ManageAction::RemoveFile, child, local, "").await?,
                _ => return Err(BridgeError::new("UNSUPPORTED", "Cannot recursively delete this file type")),
            }
        }
    }
    Ok(())
}
pub fn manage(permit: Permit, action: ManageAction, path: String, local: bool, recursive: bool, new_name: Option<String>) -> Result<String> {
    check(&permit, true)?;
    let windows = if local { cfg!(windows) } else {
        sessions::get(&permit.authority.session_id).is_some_and(|s| s.snapshot().platform.as_deref() == Some("Windows"))
    };
    validate_manage_path(&path, windows)?;
    if matches!(action, ManageAction::Rename) { validate_name(new_name.as_deref().unwrap_or(""))?; }
    else if new_name.is_some() { return Err(BridgeError::invalid("new_name is only valid for rename")); }
    if recursive && !matches!(action, ManageAction::RemoveDirectory) {
        return Err(BridgeError::invalid("recursive is only valid for remove_directory"));
    }
    let id = add(&permit, "management", json!({"path":path,"location":if local {"local"} else {"remote"},"completed_steps":0}))?;
    let job_id = id.clone();
    tokio::spawn(async move {
        let result = manage_run(&permit, &job_id, action, path, local, recursive, new_name.as_deref().unwrap_or("")).await;
        let mut all = jobs().lock().unwrap();
        if let Some(job) = all.get_mut(&job_id) {
            if !terminal(job) {
                match result {
                    Ok(()) => job.value["state"] = json!("completed"),
                    Err(error) => { job.value["state"] = json!("failed"); job.value["error"] = json!(error); }
                }
                touch(job);
            }
        }
    });
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::{control::Agent, sessions::SessionHandle};
    fn permit() -> Permit {
        let session =
            SessionHandle::new("peer".into(), SessionKind::FileTransfer, Default::default());
        let agent = Agent::new();
        session.control().attach(&agent, true).unwrap();
        let reference = session.control().view().session_ref.unwrap();
        session.control().resolve(&agent, &reference, true).unwrap()
    }
    #[tokio::test]
    async fn directory_pages_are_owned_and_late_results_cannot_revive_cancelled_jobs() {
        let owner = permit();
        let other = permit();
        let id = add(
            &owner,
            "directory",
            json!({"path":"test","location":"remote"}),
        )
        .unwrap();
        complete_directory(
            &id,
            &json!({"path":"test","entries":[{"name":"a"},{"name":"b"},{"name":"c"}]}),
        );
        let first = get(&owner, &id, None, 0, 0, 2).await.unwrap();
        assert_eq!(first["next_offset"], 2);
        assert_eq!(first["entries"].as_array().unwrap().len(), 2);
        let second = get(&owner, &id, None, 0, 2, 2).await.unwrap();
        assert_eq!(second["entries"][0]["name"], "c");
        assert!(second["next_offset"].is_null());
        assert_eq!(
            get(&other, &id, None, 0, 0, 2).await.unwrap_err().code,
            "JOB_NOT_FOUND"
        );
        jobs().lock().unwrap().remove(&id);
        let id = add(&owner, "directory", json!({"path":"cancel"})).unwrap();
        drop(DispatchGuard {
            id: id.clone(),
            committed: false,
        });
        complete_directory(&id, &json!({"path":"cancel","entries":[]}));
        assert_eq!(
            get(&owner, &id, None, 0, 0, 2).await.unwrap()["state"],
            "cancelled"
        );
        jobs().lock().unwrap().remove(&id);
    }
    #[tokio::test]
    async fn disconnect_wakes_job_waiters_without_claiming_completion() {
        let owner = permit();
        let id = add(&owner, "transfer", json!({})).unwrap();
        let observe = get(&owner, &id, Some(0), 1000, 0, 1);
        let disconnect = async {
            tokio::task::yield_now().await;
            disconnected(&owner.authority.session_id, owner.epoch);
        };
        let (result, ()) = tokio::join!(observe, disconnect);
        assert_eq!(result.unwrap()["state"], "interrupted");
        jobs().lock().unwrap().remove(&id);
    }
    #[test]
    fn empty_directory_mapping_rejects_remote_path_escape() {
        assert_eq!(
            relative_directory("C:\\root", "c:\\root\\中文\\空", true).unwrap(),
            vec!["中文", "空"]
        );
        assert!(relative_directory("C:\\root", "C:\\rooted\\bad", true).is_err());
        assert!(relative_directory("C:\\root", "C:\\root\\..\\bad", true).is_err());
        assert!(relative_directory("/root", "/elsewhere", false).is_err());
    }

    #[test]
    fn management_paths_reject_roots_and_traversal() {
        for p in ["/", "/a/../b", "relative/path", "/a/./b"] { assert!(validate_manage_path(p, false).is_err(), "{p}"); }
        for p in ["C:\\", "C:relative", "C:\\a\\..\\b", "\\\\server\\share"] { assert!(validate_manage_path(p, true).is_err(), "{p}"); }
        assert!(validate_manage_path("C:\\测试\\file", true).is_ok());
        assert!(validate_manage_path("/tmp/test", false).is_ok());
        for p in ["", ".", "..", "../escape", "a\\b", "C:evil"] { assert!(validate_name(p).is_err()); }
    }

    #[test]
    fn path_validation_preserves_unicode_but_rejects_nul_and_empty_targets() {
        assert!(validate_path("C:\\测试 文件.txt", false).is_ok());
        assert!(validate_path("", true).is_ok());
        assert!(validate_path("", false).is_err());
        assert!(validate_path("a\0b", false).is_err());
    }
}
