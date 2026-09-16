//! Stock desktop actions and request-correlated original screenshots.
use super::{
    control::Permit,
    displays,
    error::{BridgeError, Result},
    sessions::{self, SessionSnapshot},
    wire,
};
use hbb_common::{
    message_proto::{Message, Misc, ScreenshotRequest, ScreenshotResponse},
    tokio,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::Duration,
};
use uuid::Uuid;

pub fn restart_check(permit: &Permit) -> Result<SessionSnapshot> {
    let s = displays::check(permit, true)?;
    if !matches!(s.platform.as_deref(), Some("Windows" | "Linux" | "Mac OS")) {
        return Err(BridgeError::new(
            "UNSUPPORTED",
            "Restart requires a desktop operating system",
        ));
    }
    if s.permissions.get("restart") != Some(&true) {
        return Err(BridgeError::new(
            "PERMISSION_DENIED",
            "Remote restart permission is not granted",
        ));
    }
    Ok(s)
}

pub async fn restart(permit: Permit) -> Result<Value> {
    let s = restart_check(&permit)?;
    let mut misc = Misc::new();
    misc.set_restart_remote_device(true);
    let mut message = Message::new();
    message.set_misc(misc);
    wire::send(permit, message).await?;
    Ok(
        json!({"delivery":"sent","confirmed":false,"scope":"remote_machine","connection_epoch":s.connection_epoch.to_string(),"recovery":"No restart acknowledgement exists. Disconnection does not prove reboot. Observe session state and explicitly reconnect; a portable peer may require local reopening."}),
    )
}

pub async fn lock(permit: Permit) -> Result<Value> {
    displays::remote_write_check(&permit)?;
    let mut message = Message::new();
    message.set_key_event(crate::keyboard::client::event_lock_screen());
    wire::send(permit, message).await?;
    Ok(
        json!({"delivery":"sent","confirmed":false,"scope":"remote_machine","verification":"Observe the lock screen separately; the stock protocol does not acknowledge locking."}),
    )
}

pub async fn refresh(permit: Permit, display: &str) -> Result<Value> {
    let s = displays::check(&permit, true)?;
    let session = sessions::get(&s.session_id)
        .ok_or_else(|| BridgeError::new("SESSION_CLOSED", "Session closed"))?;
    let id = super::screen::display(&session, display)?;
    let multi = s
        .peer_version
        .as_deref()
        .is_some_and(crate::common::is_support_multi_ui_session);
    let message = if multi {
        crate::client::LoginConfigHandler::refresh_display(id as _)
    } else {
        crate::client::LoginConfigHandler::refresh()
    };
    wire::send(permit, message).await?;
    Ok(
        json!({"delivery":"sent","confirmed":false,"scope":"session","display_id":id.to_string(),"effective_target":if multi {"display"} else {"all_displays"},"verification":"Capture with after_frame_seq to observe a newer decoded frame."}),
    )
}

struct Pending {
    session_id: String,
    sender: tokio::sync::oneshot::Sender<ScreenshotResponse>,
}
fn pending() -> &'static Mutex<HashMap<String, Pending>> {
    static PENDING: OnceLock<Mutex<HashMap<String, Pending>>> = OnceLock::new();
    PENDING.get_or_init(Default::default)
}
struct Request(String);
impl Drop for Request {
    fn drop(&mut self) {
        pending().lock().unwrap().remove(&self.0);
    }
}
// Consume late replies too: MCP screenshots must never enter the GUI's global
// screenshot cache or open a save dialog in an unrelated window.
pub fn observe(session_id: &str, response: &ScreenshotResponse) -> bool {
    if !response.sid.starts_with("mcp_screenshot_") {
        return false;
    }
    let mut all = pending().lock().unwrap();
    if all
        .get(&response.sid)
        .is_some_and(|p| p.session_id == session_id)
    {
        if let Some(p) = all.remove(&response.sid) {
            let _caller_cancelled = p.sender.send(response.clone());
        }
    }
    true
}

pub async fn original(
    permit: &Permit,
    display: &str,
    wait_ms: u64,
) -> Result<super::screen::Observation> {
    let s = displays::check(permit, true)?;
    if !s.peer_version.as_deref().is_some_and(|v| {
        crate::common::is_support_screenshot_num(hbb_common::get_version_number(v))
    }) {
        return Err(BridgeError::new(
            "UNSUPPORTED",
            "Original screenshots require peer 1.4.0 or newer",
        ));
    }
    if wait_ms == 0 || wait_ms > 30000 {
        return Err(BridgeError::invalid(
            "Original screenshot wait_ms must be 1..30000",
        ));
    }
    let session = sessions::get(&s.session_id)
        .ok_or_else(|| BridgeError::new("SESSION_CLOSED", "Session closed"))?;
    let display = super::screen::display(&session, display)?;
    super::subscriptions::need(permit, display as i32)?;
    let request = Request(format!("mcp_screenshot_{}", Uuid::new_v4()));
    let (sender, mut response) = tokio::sync::oneshot::channel();
    {
        let mut all = pending().lock().unwrap();
        if all.len() >= 16 {
            return Err(BridgeError::new(
                "LIMIT_EXCEEDED",
                "Too many original screenshots pending",
            ));
        }
        all.insert(
            request.0.clone(),
            Pending {
                session_id: s.session_id.clone(),
                sender,
            },
        );
    }
    let mut message = Message::new();
    message.set_screenshot_request(ScreenshotRequest {
        display: display as i32,
        sid: request.0.clone(),
        ..Default::default()
    });
    wire::send(permit.clone(), message).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms);
    let response = loop {
        permit.check()?;
        tokio::select! {
            value = &mut response => break value.map_err(|_| BridgeError::new("DELIVERY_UNKNOWN", "Screenshot reply channel closed"))?,
            _ = tokio::time::sleep_until(deadline) => return Err(BridgeError::new("SCREENSHOT_TIMEOUT", "Request was sent but no original screenshot arrived; outcome unknown")),
            _ = tokio::time::sleep(Duration::from_millis(100)) => {},
        }
    };
    permit.check()?;
    if session.snapshot().layout_revision != s.layout_revision {
        return Err(BridgeError::new(
            "DISPLAY_CHANGED",
            "Display layout changed while taking screenshot",
        ));
    }
    if !response.msg.is_empty() {
        return Err(BridgeError::new(
            "REMOTE_SCREENSHOT_FAILED",
            response.msg.chars().take(1024).collect::<String>(),
        ));
    }
    if response.data.len() > super::capture::MAX_PNG_BYTES {
        return Err(BridgeError::new(
            "IMAGE_TOO_LARGE",
            "Original PNG exceeds 8 MiB; use decoded capture with size limits",
        ));
    }
    let dimensions = image::io::Reader::with_format(
        std::io::Cursor::new(&response.data),
        image::ImageFormat::Png,
    )
    .into_dimensions()
    .map_err(|_| BridgeError::new("INVALID_IMAGE", "Peer did not return a valid PNG"))?;
    Ok(super::screen::Observation {
        data: json!({"session_ref":session.control().view().session_ref,"source":"remote_original","remote_capture":{"display_id":display.to_string(),"image_width":dimensions.0,"image_height":dimensions.1,"received_at":chrono::Utc::now(),"connection_epoch":s.connection_epoch.to_string(),"layout_revision":s.layout_revision.to_string(),"snapshot_id":null,"cursor_composited":false},"image_content_index":1}),
        png: Some(response.data.to_vec()),
        unchanged: false,
    })
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(&self.0) {
            if e.kind() != std::io::ErrorKind::NotFound {
                hbb_common::log::warn!("Screenshot temporary cleanup failed: {e}");
            }
        }
    }
}
pub fn validate_path(path: &str) -> Result<()> {
    let path = Path::new(path);
    if !path.is_absolute()
        || path.file_name().is_none()
        || path.extension().and_then(|s| s.to_str()) != Some("png")
    {
        return Err(BridgeError::invalid(
            "save_path must be an absolute PNG file path ending in .png",
        ));
    }
    Ok(())
}
pub async fn save(permit: Permit, path: String, png: Vec<u8>) -> Result<Value> {
    validate_path(&path)?;
    tokio::task::spawn_blocking(move || -> Result<Value> {
        permit.check()?;
        let destination = Path::new(&path);
        let parent = destination.parent().ok_or_else(|| BridgeError::invalid("Missing parent directory"))?;
        let parent = std::fs::canonicalize(parent).map_err(|e| BridgeError::new("SAVE_FAILED", e.to_string()))?;
        let name = destination.file_name().ok_or_else(|| BridgeError::invalid("Missing file name"))?;
        let destination = parent.join(name);
        let temp_path = parent.join(format!(".rustdesk-screenshot-{}.tmp", Uuid::new_v4()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
        let mut file = options.open(&temp_path).map_err(|e| BridgeError::new("SAVE_FAILED", e.to_string()))?;
        let temp = Temporary(temp_path);
        file.write_all(&png).and_then(|_| file.sync_all()).map_err(|e| BridgeError::new("SAVE_FAILED", e.to_string()))?;
        permit.check()?;
        // Publish without overwriting an existing file or following a target symlink.
        std::fs::hard_link(&temp.0, &destination).map_err(|e| BridgeError::new(if e.kind() == std::io::ErrorKind::AlreadyExists { "FILE_EXISTS" } else { "SAVE_FAILED" }, e.to_string()))?;
        Ok(json!({"path":destination,"bytes":png.len(),"sha256":format!("{:x}",Sha256::digest(&png)),"scope":"local_filesystem","confirmed":true}))
    }).await.map_err(|_| BridgeError::new("SAVE_FAILED", "Screenshot save worker failed"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn original_screenshot_replies_are_isolated_by_session() {
        let id = format!("mcp_screenshot_{}", Uuid::new_v4());
        let (sender, mut receiver) = tokio::sync::oneshot::channel();
        pending().lock().unwrap().insert(
            id.clone(),
            Pending {
                session_id: "a".into(),
                sender,
            },
        );
        let response = ScreenshotResponse {
            sid: id.clone(),
            ..Default::default()
        };
        assert!(observe("b", &response));
        assert!(receiver.try_recv().is_err());
        assert!(observe("a", &response));
        assert!(receiver.try_recv().is_ok());
        assert!(observe("a", &response));
        assert!(!observe(
            "a",
            &ScreenshotResponse {
                sid: "gui".into(),
                ..Default::default()
            }
        ));
    }
}
