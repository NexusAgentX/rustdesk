//! Native desktop file clipboard. Its system clipboard and serving files are process-wide.
use super::{
    control::Permit,
    error::{BridgeError, Result},
    input,
    sessions::{self, ConnectionState, SessionKind},
    wire,
};
use crate::ui_session_interface::{InvokeUiSession, Session};
use hbb_common::tokio;
use serde_json::{json, Value};
use std::{
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

struct Offer {
    session: String,
    binding: String,
    epoch: u64,
    at: Instant,
}
fn offer() -> &'static Mutex<Option<Offer>> {
    static OFFER: OnceLock<Mutex<Option<Offer>>> = OnceLock::new();
    OFFER.get_or_init(Default::default)
}
pub fn observe<T: InvokeUiSession>(core: &Session<T>) {
    let Some(s) = sessions::for_core(core) else {
        return;
    };
    let Some(binding) = s.control().binding_id() else {
        return;
    };
    let snapshot = s.snapshot();
    *offer().lock().unwrap() = Some(Offer {
        session: snapshot.session_id,
        binding,
        epoch: snapshot.connection_epoch,
        at: Instant::now(),
    });
}
fn known(permit: &Permit) -> bool {
    offer().lock().unwrap().as_ref().is_some_and(|o| {
        o.session == permit.authority.session_id
            && o.binding == permit.binding_id()
            && o.epoch == permit.epoch
            && o.at.elapsed() < Duration::from_secs(300)
    })
}
pub fn check(permit: &Permit, enabled: bool) -> Result<()> {
    permit.check()?;
    let s = sessions::get(&permit.authority.session_id)
        .ok_or_else(|| BridgeError::new("SESSION_CLOSED", "Desktop closed"))?
        .snapshot();
    if s.kind != SessionKind::Desktop {
        return Err(BridgeError::new(
            "WRONG_SESSION_KIND",
            "File clipboard requires a desktop session",
        ));
    }
    if !cfg!(any(windows, feature = "unix-file-copy-paste"))
        || !s.peer_version.as_deref().is_some_and(|v| {
            hbb_common::get_version_number(v) >= hbb_common::get_version_number("1.3.8")
        })
    {
        return Err(BridgeError::new(
            "UNSUPPORTED",
            "File clipboard is unsupported by this build or peer",
        ));
    }
    if s.state != ConnectionState::Ready || !s.authenticated {
        return Err(BridgeError::new("NOT_READY", "Desktop is not ready"));
    }
    if s.permissions.get("file") != Some(&true) || s.permissions.get("keyboard") != Some(&true) {
        return Err(BridgeError::new(
            "PERMISSION_DENIED",
            "File and keyboard permissions are required",
        ));
    }
    let core = sessions::core(&s.session_id)
        .ok_or_else(|| BridgeError::new("GUI_UNAVAILABLE", "Desktop is unavailable"))?;
    let lc = core.lc.read().unwrap();
    if lc.view_only.v || (enabled && !lc.enable_file_copy_paste.v) {
        return Err(BridgeError::new(
            "CLIPBOARD_DISABLED",
            "File clipboard is disabled or view-only",
        ));
    }
    Ok(())
}
pub fn settings(permit: &Permit) -> Result<Value> {
    permit.read_check()?;
    let s = sessions::get(&permit.authority.session_id)
        .ok_or_else(|| BridgeError::new("SESSION_CLOSED", "Desktop closed"))?
        .snapshot();
    if s.kind != SessionKind::Desktop {
        return Err(BridgeError::new(
            "WRONG_SESSION_KIND",
            "File clipboard requires a desktop session",
        ));
    }
    let core = sessions::core(&s.session_id)
        .ok_or_else(|| BridgeError::new("GUI_UNAVAILABLE", "Desktop unavailable"))?;
    let lc = core.lc.read().unwrap();
    let supported = cfg!(any(windows, feature = "unix-file-copy-paste"))
        && s.peer_version.as_deref().is_some_and(|v| {
            hbb_common::get_version_number(v) >= hbb_common::get_version_number("1.3.8")
        });
    Ok(
        json!({"enabled":lc.enable_file_copy_paste.v,"effective_enabled":supported&&lc.enable_file_copy_paste.v&&!lc.view_only.v&&s.permissions.get("file")==Some(&true)&&s.permissions.get("keyboard")==Some(&true),"supported":supported,"direct_local_paste_supported":cfg!(all(target_os="macos",feature="unix-file-copy-paste")),"scope":"peer_preference","clipboard_scope":"local_system_global","remote_offer_known":known(permit),"local_paste":paste_view(permit)}),
    )
}
pub async fn set_enabled(permit: Permit, enabled: bool) -> Result<Value> {
    check(&permit, false)?;
    let p = permit.clone();
    let message = tokio::task::spawn_blocking(move || -> Result<_> {
        check(&p, false)?;
        let core = sessions::core(&p.authority.session_id)
            .ok_or_else(|| BridgeError::new("GUI_UNAVAILABLE", "Desktop unavailable"))?;
        let mut lc = core.lc.write().unwrap();
        Ok(if lc.enable_file_copy_paste.v == enabled {
            None
        } else {
            lc.toggle_option("enable-file-copy-paste".into())
        })
    })
    .await
    .map_err(|_| BridgeError::new("INTERNAL_ERROR", "Setting worker failed"))??;
    if let Some(msg) = message {
        wire::send(permit.clone(), msg).await?;
    }
    #[cfg(feature = "unix-file-copy-paste")]
    crate::flutter::update_file_clipboard_required();
    settings(&permit)
}
async fn shortcut(permit: Permit, key: input::KeyName) -> Result<Value> {
    check(&permit, true)?;
    let s = sessions::get(&permit.authority.session_id)
        .ok_or_else(|| BridgeError::new("SESSION_CLOSED", "Desktop closed"))?
        .snapshot();
    let modifier = if s.platform.as_deref() == Some("Mac OS") {
        input::Modifier::Meta
    } else {
        input::Modifier::Control
    };
    let result = input::send(
        permit,
        vec![input::Action::Shortcut {
            modifiers: vec![modifier],
            key,
        }],
        None,
        std::future::pending::<()>(),
    )
    .await?;
    if let Some(e) = result.error {
        return Err(e);
    }
    Ok(
        json!({"delivery":result.delivery,"clipboard_scope":"local_system_global","application_result":"unknown"}),
    )
}
pub async fn copy(permit: Permit, paths: Option<Vec<String>>) -> Result<Value> {
    check(&permit, true)?;
    if let Some(paths) = paths {
        if paths.is_empty() || paths.len() > 128 {
            return Err(BridgeError::invalid(
                "paths must contain 1..128 local absolute paths",
            ));
        }
        let p = permit.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            check(&p, true)?;
            for path in &paths {
                if !std::path::Path::new(path).is_absolute() || !std::path::Path::new(path).exists()
                {
                    return Err(BridgeError::invalid(
                        "Every clipboard path must exist and be absolute",
                    ));
                }
            }
            #[cfg(feature = "unix-file-copy-paste")]
            clipboard::platform::unix::serv_files::sync_files(&paths)
                .map_err(|e| BridgeError::new("CLIPBOARD_ERROR", e.to_string()))?;
            crate::clipboard::set_local_file_clipboard(paths)
                .map_err(|e| BridgeError::new("CLIPBOARD_ERROR", e.to_string()))?;
            *offer().lock().unwrap() = None;
            Ok(())
        })
        .await
        .map_err(|_| BridgeError::new("INTERNAL_ERROR", "Clipboard worker failed"))??;
        #[cfg(feature = "unix-file-copy-paste")]
        {
            check(&permit, true)?;
            wire::send(
                permit,
                crate::clipboard_file::clip_2_msg(
                    crate::clipboard_file::unix_file_clip::get_format_list(),
                ),
            )
            .await?;
            return Ok(
                json!({"delivery":"sent","clipboard_scope":"local_system_global","remote_delivery":"file_offer_sent"}),
            );
        }
        #[cfg(not(feature = "unix-file-copy-paste"))]
        return Ok(
            json!({"delivery":"local_clipboard_updated","clipboard_scope":"local_system_global","remote_delivery":"native_sync_pending"}),
        );
    }
    // Stock clipboard reception is restricted to the active local desktop tab.
    let core = sessions::core(&permit.authority.session_id)
        .ok_or_else(|| BridgeError::new("GUI_UNAVAILABLE", "Desktop unavailable"))?;
    if core.lc.read().unwrap().get_id().to_owned() != crate::flutter::get_cur_peer_id() {
        return Err(BridgeError::new(
            "NOT_ACTIVE_SESSION",
            "Select this desktop tab before copying remote files",
        ));
    }
    *offer().lock().unwrap() = None;
    shortcut(permit, input::KeyName::KeyC).await
}
pub async fn paste_remote(permit: Permit, delay_ms: u64) -> Result<Value> {
    super::api::wait_budget(delay_ms)?;
    check(&permit, true)?;
    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    shortcut(permit, input::KeyName::KeyV).await
}
struct PasteRecord {
    id: String,
    permit: Permit,
    value: Value,
}
fn paste_record() -> &'static Mutex<Option<PasteRecord>> {
    static RECORD: OnceLock<Mutex<Option<PasteRecord>>> = OnceLock::new();
    RECORD.get_or_init(Default::default)
}
fn paste_terminal(value: &Value) -> bool {
    matches!(
        value["state"].as_str(),
        Some("completed" | "failed" | "cancelled" | "interrupted")
    )
}
fn paste_view(permit: &Permit) -> Value {
    paste_record()
        .lock()
        .unwrap()
        .as_ref()
        .filter(|r| {
            r.permit.binding_id() == permit.binding_id()
                && r.permit.authority.session_id == permit.authority.session_id
        })
        .map(|r| r.value.clone())
        .unwrap_or(Value::Null)
}
#[cfg(all(target_os = "macos", feature = "unix-file-copy-paste"))]
async fn monitor_paste(id: String, permit: Permit) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
    loop {
        let error = check(&permit, true).err().or_else(|| {
            (tokio::time::Instant::now() >= deadline)
                .then(|| BridgeError::new("TIMEOUT", "Local clipboard paste exceeded ten minutes"))
        });
        let mut state = Value::Null;
        let native = clipboard::ContextSend::proc(|c| {
            if error.is_some() {
                c.cancel_paste(&id);
            }
            state = serde_json::to_value(c.paste_status())?;
            Ok(())
        });
        if let Some(error) = error {
            state = json!({"state":"interrupted","error":error});
        } else if let Err(error) = native {
            state = json!({"state":"failed","error":error.to_string()});
        } else if state["request_id"] != id {
            state = json!({"state":"interrupted","error":"Native paste context changed"});
        }
        let done = paste_terminal(&state);
        {
            let mut record = paste_record().lock().unwrap();
            let Some(record) = record.as_mut().filter(|r| r.id == id) else {
                return;
            };
            state["job_id"] = json!(id);
            record.value = state;
        }
        if done {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
#[cfg(all(target_os = "macos", feature = "unix-file-copy-paste"))]
pub async fn paste_local(permit: Permit, destination: String, wait_ms: u64) -> Result<Value> {
    super::api::wait_budget(wait_ms)?;
    check(&permit, true)?;
    if !known(&permit) {
        return Err(BridgeError::new(
            "CLIPBOARD_UNKNOWN",
            "Copy remote files in this binding before local paste",
        ));
    }
    let core = sessions::core(&permit.authority.session_id)
        .ok_or_else(|| BridgeError::new("GUI_UNAVAILABLE", "Desktop unavailable"))?;
    let conn = clipboard::get_client_conn_id(&core.lc.read().unwrap().get_id().to_owned())
        .ok_or_else(|| BridgeError::new("NOT_READY", "Clipboard connection unavailable"))?;
    let p = permit.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        check(&p, true)?;
        if !clipboard::ContextSend::is_enabled() {
            return Err(BridgeError::new(
                "NOT_READY",
                "Native clipboard context unavailable",
            ));
        }
        let mut record = paste_record().lock().unwrap();
        if record.as_ref().is_some_and(|r| !paste_terminal(&r.value)) {
            return Err(BridgeError::new(
                "BUSY",
                "A local clipboard paste is still active",
            ));
        }
        let id = format!("paste_{}", uuid::Uuid::new_v4());
        clipboard::ContextSend::proc(|c| {
            c.request_paste(&id, conn, std::path::Path::new(&destination))
                .map_err(Into::into)
        })
        .map_err(|e| BridgeError::new("CLIPBOARD_ERROR", e.to_string()))?;
        *record = Some(PasteRecord {
            id: id.clone(),
            permit: p.clone(),
            value: json!({"job_id":id,"state":"awaiting_metadata"}),
        });
        tokio::spawn(monitor_paste(id, p));
        Ok(())
    })
    .await
    .map_err(|_| BridgeError::new("INTERNAL_ERROR", "Paste worker failed"))??;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms);
    loop {
        permit.read_check()?;
        let state = paste_view(&permit);
        if paste_terminal(&state) || tokio::time::Instant::now() >= deadline {
            return Ok(
                json!({"delivery":"requested","paste":state,"clipboard_scope":"local_system_global"}),
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
#[cfg(not(all(target_os = "macos", feature = "unix-file-copy-paste")))]
pub async fn paste_local(_permit: Permit, _destination: String, _wait_ms: u64) -> Result<Value> {
    Err(BridgeError::new(
        "UNSUPPORTED",
        "Direct local paste currently requires macOS file clipboard support",
    ))
}

#[cfg(all(target_os = "macos", feature = "unix-file-copy-paste"))]
pub fn cancel_local(permit: &Permit) -> Result<Value> {
    permit.check()?;
    let record = paste_record().lock().unwrap();
    let r = record
        .as_ref()
        .filter(|r| {
            r.permit.binding_id() == permit.binding_id()
                && r.permit.authority.session_id == permit.authority.session_id
        })
        .ok_or_else(|| {
            BridgeError::new("JOB_NOT_FOUND", "No local clipboard paste in this binding")
        })?;
    if !paste_terminal(&r.value) {
        clipboard::ContextSend::proc(|c| {
            c.cancel_paste(&r.id);
            Ok(())
        })
        .map_err(|e| BridgeError::new("CLIPBOARD_ERROR", e.to_string()))?;
    }
    Ok(json!({"job_id":r.id,"delivery":"cancel_requested","partial_files_may_remain":true}))
}
#[cfg(not(all(target_os = "macos", feature = "unix-file-copy-paste")))]
pub fn cancel_local(_permit: &Permit) -> Result<Value> {
    Err(BridgeError::new(
        "UNSUPPORTED",
        "Direct local paste requires macOS file clipboard support",
    ))
}
