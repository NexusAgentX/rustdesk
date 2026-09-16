//! Per-binding remote text observations, separate from the local system clipboard.
use super::{
    api,
    control::Permit,
    error::{BridgeError, Result},
    sessions::{self, ConnectionState, SessionHandle, SessionKind},
    wire,
};
use hbb_common::{
    message_proto::{Clipboard, ClipboardFormat, Message},
    tokio::{self, sync::watch},
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
const MAX_TEXT: usize = 1024 * 1024;
struct Observation {
    binding: String,
    epoch: u64,
    revision: u64,
    text: Option<String>,
    error: Option<&'static str>,
    at: Instant,
    received: chrono::DateTime<chrono::Utc>,
}
struct Cache {
    values: HashMap<String, Observation>,
    changed: watch::Sender<u64>,
}
fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let (changed, _) = watch::channel(0);
        Mutex::new(Cache {
            values: HashMap::new(),
            changed,
        })
    })
}
pub fn observe(session: &SessionHandle, clipboards: &[Clipboard]) {
    let Some(cb) = clipboards
        .iter()
        .find(|c| c.format.enum_value() == Ok(ClipboardFormat::Text))
    else {
        return;
    };
    let s = session.snapshot();
    let Some(binding) = session.control().binding_id() else {
        return;
    };
    let (text, error) = if cb.content.len() > MAX_TEXT {
        (None, Some("TEXT_TOO_LARGE"))
    } else {
        let bytes = if cb.compress {
            hbb_common::compress::decompress(&cb.content)
        } else {
            cb.content.to_vec()
        };
        if bytes.len() > MAX_TEXT {
            (None, Some("TEXT_TOO_LARGE"))
        } else {
            match String::from_utf8(bytes) {
                Ok(text) => (Some(text), None),
                Err(_) => (None, Some("INVALID_UTF8")),
            }
        }
    };
    let mut c = cache().lock().unwrap();
    c.values
        .retain(|_, v| v.at.elapsed() < Duration::from_secs(300));
    if c.values.get(&s.session_id).is_some_and(|v| {
        v.binding == binding && v.epoch == s.connection_epoch && v.text == text && v.error == error
    }) {
        return;
    }
    if c.values.len() >= 32 && !c.values.contains_key(&s.session_id) {
        if let Some(oldest) = c
            .values
            .iter()
            .min_by_key(|(_, v)| v.at)
            .map(|(k, _)| k.clone())
        {
            c.values.remove(&oldest);
        }
    }
    let revision = *c.changed.borrow() + 1;
    c.values.insert(
        s.session_id,
        Observation {
            binding,
            epoch: s.connection_epoch,
            revision,
            text,
            error,
            at: Instant::now(),
            received: chrono::Utc::now(),
        },
    );
    c.changed.send_replace(revision);
}
pub fn check(permit: &Permit, write: bool, require_enabled: bool) -> Result<()> {
    if write {
        permit.check()?;
    } else {
        permit.read_check()?;
    }
    let s = sessions::get(&permit.authority.session_id)
        .ok_or_else(|| BridgeError::new("SESSION_CLOSED", "Session closed"))?
        .snapshot();
    if s.kind != SessionKind::Desktop {
        return Err(BridgeError::new(
            "WRONG_SESSION_KIND",
            "Text clipboard requires a desktop session",
        ));
    }
    if s.permissions.get("clipboard") != Some(&true) {
        return Err(BridgeError::new(
            "PERMISSION_DENIED",
            "Clipboard permission is not granted",
        ));
    }
    if write && (!s.authenticated || s.state != ConnectionState::Ready) {
        return Err(BridgeError::new("NOT_READY", "Desktop is not ready"));
    }
    let core = sessions::core(&s.session_id)
        .ok_or_else(|| BridgeError::new("GUI_UNAVAILABLE", "Desktop window unavailable"))?;
    let lc = core.lc.read().unwrap();
    if lc.view_only.v || (require_enabled && lc.disable_clipboard.v) {
        return Err(BridgeError::new(
            "CLIPBOARD_DISABLED",
            "Clipboard is disabled or the session is view-only",
        ));
    }
    Ok(())
}
pub fn settings(permit: &Permit) -> Result<Value> {
    permit.read_check()?;
    let s = sessions::get(&permit.authority.session_id)
        .ok_or_else(|| BridgeError::new("SESSION_CLOSED", "Session closed"))?
        .snapshot();
    if s.kind != SessionKind::Desktop {
        return Err(BridgeError::new(
            "WRONG_SESSION_KIND",
            "Text clipboard requires a desktop session",
        ));
    }
    let core = sessions::core(&s.session_id)
        .ok_or_else(|| BridgeError::new("GUI_UNAVAILABLE", "Desktop window unavailable"))?;
    let lc = core.lc.read().unwrap();
    Ok(
        json!({"enabled":!lc.disable_clipboard.v,"effective_enabled":!lc.disable_clipboard.v&&!lc.view_only.v&&s.permissions.get("clipboard")==Some(&true),"view_only":lc.view_only.v,"allowed":s.permissions.get("clipboard"),"scope":"peer_preference"}),
    )
}
pub async fn set_enabled(permit: Permit, enabled: bool) -> Result<Value> {
    check(&permit, true, false)?;
    let p = permit.clone();
    let msg = tokio::task::spawn_blocking(move || -> Result<Option<Message>> {
        check(&p, true, false)?;
        let core = sessions::core(&p.authority.session_id)
            .ok_or_else(|| BridgeError::new("GUI_UNAVAILABLE", "Desktop window unavailable"))?;
        let mut lc = core.lc.write().unwrap();
        if lc.disable_clipboard.v == !enabled {
            Ok(None)
        } else {
            Ok(lc.toggle_option("disable-clipboard".into()))
        }
    })
    .await
    .map_err(|_| BridgeError::new("INTERNAL_ERROR", "Clipboard setting worker failed"))??;
    if let Some(msg) = msg {
        wire::send(permit.clone(), msg)
            .await
            .map_err(|e| e.details(json!({"local_preference_updated":true,"enabled":enabled})))?;
    }
    crate::flutter::update_text_clipboard_required();
    settings(&permit)
}
pub async fn read(permit: &Permit, after: Option<u64>, ms: u64) -> Result<Value> {
    api::wait_budget(ms)?;
    check(permit, false, true)?;
    let mut changed = cache().lock().unwrap().changed.subscribe();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
    loop {
        check(permit, false, true)?;
        let s = sessions::get(&permit.authority.session_id)
            .ok_or_else(|| BridgeError::new("SESSION_CLOSED", "Session closed"))?
            .snapshot();
        let value = {
            let c = cache().lock().unwrap();
            let obs = c.values.get(&s.session_id).filter(|v| {
                v.binding == permit.binding_id()
                    && v.epoch == s.connection_epoch
                    && v.at.elapsed() < Duration::from_secs(300)
            });
            match obs {
                Some(v) => {
                    json!({"known":true,"source":"remote_sync","text":v.text,"error":v.error,"revision":v.revision,"received_at":v.received,"connection_epoch":v.epoch.to_string()})
                }
                None => {
                    json!({"known":false,"source":"remote_sync","text":null,"revision":0,"received_at":null,"connection_epoch":s.connection_epoch.to_string()})
                }
            }
        };
        let qualifies = after.map_or(value["known"] == true, |r| {
            value["known"] == true && value["revision"].as_u64() != Some(r)
        });
        if qualifies || ms == 0 || tokio::time::Instant::now() >= deadline {
            let mut result = value;
            result["changed"] = json!(qualifies);
            return Ok(result);
        }
        // Also recheck permissions and binding during an otherwise quiet clipboard stream.
        let until = std::cmp::min(
            deadline,
            tokio::time::Instant::now() + Duration::from_millis(250),
        );
        let _ = tokio::time::timeout_at(until, changed.changed()).await;
    }
}
pub async fn write(permit: Permit, text: String) -> Result<()> {
    check(&permit, true, true)?;
    if text.len() > MAX_TEXT {
        return Err(BridgeError::invalid("Text exceeds 1 MiB"));
    }
    let mut cb = Clipboard::new();
    cb.content = text.into_bytes().into();
    cb.format = ClipboardFormat::Text.into();
    let mut msg = Message::new();
    msg.set_clipboard(cb);
    wire::send(permit, msg).await
}
pub async fn local_text() -> Result<String> {
    tokio::task::spawn_blocking(|| {
        let mut board = arboard::Clipboard::new().map_err(|_| {
            BridgeError::new("CLIPBOARD_UNAVAILABLE", "Cannot open local clipboard")
        })?;
        let text = board.get_text().map_err(|_| {
            BridgeError::new("CLIPBOARD_UNAVAILABLE", "Local clipboard has no text")
        })?;
        if text.len() > MAX_TEXT {
            return Err(BridgeError::invalid("Local clipboard exceeds 1 MiB"));
        }
        Ok(text)
    })
    .await
    .map_err(|_| BridgeError::new("INTERNAL_ERROR", "Local clipboard worker failed"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::control::Agent;
    #[test]
    fn observations_preserve_empty_text_and_replace_invalid_data_without_leaking_old_text() {
        let session = SessionHandle::new("peer".into(), SessionKind::Desktop, Default::default());
        let agent = Agent::new();
        session.control().attach(&agent, true).unwrap();
        let sid = session.snapshot().session_id;
        let mut cb = Clipboard::new();
        cb.content = "中文\ntext".as_bytes().to_vec().into();
        observe(&session, std::slice::from_ref(&cb));
        let revision = {
            let c = cache().lock().unwrap();
            let v = &c.values[&sid];
            assert_eq!(v.text.as_deref(), Some("中文\ntext"));
            v.revision
        };
        observe(&session, std::slice::from_ref(&cb));
        assert_eq!(cache().lock().unwrap().values[&sid].revision, revision);
        cb.content = vec![0xff].into();
        observe(&session, std::slice::from_ref(&cb));
        {
            let c = cache().lock().unwrap();
            assert!(c.values[&sid].text.is_none());
            assert_eq!(c.values[&sid].error, Some("INVALID_UTF8"));
        }
        cb.content = Vec::new().into();
        observe(&session, std::slice::from_ref(&cb));
        {
            let c = cache().lock().unwrap();
            assert_eq!(c.values[&sid].text.as_deref(), Some(""));
        }
        cache().lock().unwrap().values.remove(&sid);
    }
}
