use super::{
    tools::{self, Reply},
    Client,
};
use crate::automation::{
    api,
    control::Permit,
    error::{BridgeError, Result},
};
use hbb_common::{
    rand::{rngs::OsRng, RngCore},
    tokio::{self, sync::watch},
};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

struct Completed {
    at: Instant,
    expires: chrono::DateTime<chrono::Utc>,
    reply: Reply,
}
struct Record {
    digest: [u8; 32],
    tool: String,
    target: Mutex<Option<Permit>>,
    result: Mutex<Option<Completed>>,
    changed: watch::Sender<bool>,
}
#[derive(Default)]
pub(super) struct Operations {
    records: Mutex<HashMap<String, Arc<Record>>>,
}
struct Observation {
    at: Instant,
    kind: String,
    value: Value,
    png: Option<Vec<u8>>,
    bytes: usize,
}
fn observations() -> &'static Mutex<HashMap<String, Observation>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Observation>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}
fn digest(name: &str, args: &Map<String, Value>) -> Result<[u8; 32]> {
    static KEY: OnceLock<[u8; 32]> = OnceLock::new();
    let key = if let Some(key) = KEY.get() {
        *key
    } else {
        let mut key = [0; 32];
        OsRng
            .try_fill_bytes(&mut key)
            .map_err(|_| BridgeError::new("INTERNAL_ERROR", "Secure random source failed"))?;
        let _already_initialized = KEY.set(key);
        *KEY.get()
            .ok_or_else(|| BridgeError::new("INTERNAL_ERROR", "Digest key unavailable"))?
    };
    let mut inner = [0x36; 64];
    let mut outer = [0x5c; 64];
    for i in 0..32 {
        inner[i] ^= key[i];
        outer[i] ^= key[i];
    }
    let mut hash = Sha256::new();
    hash.update(inner);
    hash.update(name.as_bytes());
    hash.update([0]);
    hash.update(
        serde_json::to_vec(args).map_err(|_| BridgeError::invalid("Could not encode arguments"))?,
    );
    let inside = hash.finalize();
    let mut hash = Sha256::new();
    hash.update(outer);
    hash.update(inside);
    Ok(hash.finalize().into())
}
fn cache_key(client: &Client, id: &str) -> String {
    format!("{}:{id}", client.agent.id)
}
fn save_observation(client: &Client, key: &str, reply: &mut Reply) {
    let Some(value) = reply
        .value
        .get_mut("data")
        .and_then(|d| d.as_object_mut())
        .and_then(|d| d.remove("observation"))
    else {
        return;
    };
    let kind = value["kind"].as_str().unwrap_or("terminal").to_owned();
    let bytes = if kind == "screen" {
        reply.png.as_ref().map_or(0, Vec::len)
    } else {
        value.to_string().len()
    };
    let mut cache = observations().lock().unwrap();
    if !client.agent.is_alive() {
        reply.png = None;
        reply.value["data"]["observation"] = json!({"kind":kind,"status":"expired"});
        return;
    }
    cache.retain(|_, v| v.at.elapsed() < Duration::from_secs(30));
    let limit = if kind == "screen" {
        32 * 1024 * 1024
    } else {
        8 * 1024 * 1024
    };
    while cache
        .values()
        .filter(|v| v.kind == kind)
        .map(|v| v.bytes)
        .sum::<usize>()
        + bytes
        > limit
    {
        let oldest = cache
            .iter()
            .filter(|(_, v)| v.kind == kind)
            .min_by_key(|(_, v)| v.at)
            .map(|(key, _)| key.clone());
        if let Some(oldest) = oldest {
            cache.remove(&oldest);
        } else {
            break;
        }
    }
    cache.insert(
        key.into(),
        Observation {
            at: Instant::now(),
            kind: kind.clone(),
            value,
            png: reply.png.take(),
            bytes,
        },
    );
    reply.value["data"]["observation"] = json!({"kind":kind,"status":"expired"});
}
fn restore_observation(key: &str, reply: &mut Reply) {
    let cache = observations().lock().unwrap();
    if let Some(value) = cache
        .get(key)
        .filter(|v| v.at.elapsed() < Duration::from_secs(30))
    {
        reply.value["data"]["observation"] = value.value.clone();
        reply.png = value.png.clone();
    }
}
fn snapshot(record: &Record, id: &str, key: &str, replayed: bool) -> Result<Reply> {
    if replayed {
        if let Some(target) = record.target.lock().unwrap().as_ref() {
            target.read_check()?;
        }
    }
    let completed = record.result.lock().unwrap();
    let mut reply = if let Some(result) = completed.as_ref() {
        if result.at.elapsed() >= Duration::from_secs(300) {
            return Err(BridgeError::new(
                "OPERATION_EXPIRED",
                "Operation result expired",
            ));
        }
        let mut reply = result.reply.clone();
        reply.value["operation"] =
            json!({"id":id,"dedupe_expires_at":result.expires,"replayed":replayed});
        restore_observation(key, &mut reply);
        reply
    } else {
        let mut reply = Reply::success(json!({})).status("pending");
        reply.value["operation"] = json!({"id":id,"dedupe_expires_at":null,"replayed":replayed});
        reply
    };
    if reply.value["operation"].is_null() {
        reply.value["operation"] = json!({"id":id,"replayed":replayed});
    }
    let (execution, outcome) = execution_state(completed.as_ref().map(|r| &r.reply.value));
    reply.value["operation"]["tool"] = json!(record.tool);
    reply.value["operation"]["execution_state"] = json!(execution);
    reply.value["operation"]["outcome"] = json!(outcome);
    Ok(reply)
}

// A completed local call may only have sent a request. Preserve its evidence instead
// of promoting an observation timeout into remote success or replaying the write.
fn execution_state(result: Option<&Value>) -> (&'static str, &'static str) {
    match result {
        None => ("running", "pending"),
        Some(v) if v["error"]["code"] == "CANCELLED" => ("cancelled", "unknown"),
        Some(v) if v["status"] == "partial" => ("finished", "partial"),
        Some(v) if v["ok"] == false => ("failed", "unknown"),
        Some(v) if v["status"] == "pending" => ("finished", "unknown"),
        Some(_) => ("finished", "reported"),
    }
}

pub async fn wait_query(client: &Client, id: &str, ms: u64) -> Result<Value> {
    api::wait_budget(ms)?;
    let record = client.operations.records.lock().unwrap().get(id).cloned()
        .ok_or_else(|| BridgeError::new("OPERATION_EXPIRED", "Operation is absent or expired"))?;
    let mut changed = record.changed.subscribe();
    if ms > 0 && record.result.lock().unwrap().is_none() {
        let _ = tokio::time::timeout(Duration::from_millis(ms), changed.changed()).await;
    }
    query(client, id)
}
pub async fn execute(
    client: Arc<Client>,
    name: &str,
    args: Map<String, Value>,
    cancel: CancellationToken,
) -> Result<Reply> {
    let Some(id) = args
        .get("operation_id")
        .and_then(Value::as_str)
        .filter(|_| !matches!(name, "rd_session_get" | "rd_operation_get"))
    else {
        return run(&client, name, &args, cancel).await;
    };
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(BridgeError::invalid(
            "operation_id must contain 1..64 ASCII letters, digits, underscores or hyphens",
        ));
    }
    let id = id.to_owned();
    let hash = digest(name, &args)?;
    let key = cache_key(&client, &id);
    let cleanup = matches!(
        name,
        "rd_session_detach" | "rd_control_release" | "rd_control_cancel"
    );
    let reservation = (|| -> Result<Option<(Arc<Record>, bool)>> {
        let mut records = client.operations.records.lock().unwrap();
        records.retain(|_, r| {
            r.result
                .lock()
                .unwrap()
                .as_ref()
                .is_none_or(|v| v.at.elapsed() < Duration::from_secs(300))
        });
        if let Some(record) = records.get(&id) {
            if record.digest != hash {
                return Err(BridgeError::new(
                    "OPERATION_CONFLICT",
                    "operation_id was already used with different arguments",
                ));
            }
            Ok(Some((record.clone(), true)))
        } else {
            tools::preflight(&client, name, &args)?;
            if records.len() >= 256 {
                if cleanup {
                    return Ok(None);
                }
                return Err(BridgeError::new(
                    "LIMIT_EXCEEDED",
                    "All 256 operation records are still retained; retry after expiry",
                ));
            }
            let target = args
                .get("session_ref")
                .and_then(Value::as_str)
                .map(|reference| {
                    api::resolve(&client.agent, reference, false).map(|(_, permit)| permit)
                })
                .transpose()?;
            let (changed, _) = watch::channel(false);
            let record = Arc::new(Record {
                digest: hash,
                tool: name.to_owned(),
                target: Mutex::new(if name == "rd_session_detach" {
                    None
                } else {
                    target
                }),
                result: Mutex::new(None),
                changed,
            });
            records.insert(id.clone(), record.clone());
            Ok(Some((record, false)))
        }
    })()?;
    let Some((record, replayed)) = reservation else {
        return run(&client, name, &args, cancel).await;
    };
    if replayed {
        return snapshot(&record, &id, &key, true);
    }
    let mut changed = record.changed.subscribe();
    let worker = record.clone();
    let client_worker = client.clone();
    let tool = name.to_owned();
    let operation = id.clone();
    let observation_key = key.clone();
    tokio::spawn(async move {
        let mut reply = run(&client_worker, &tool, &args, cancel)
            .await
            .unwrap_or_else(Reply::error);
        if let Some(reference) = reply.value["data"]["session"]["session_ref"].as_str() {
            if let Ok((_, target)) = api::resolve(&client_worker.agent, reference, false) {
                *worker.target.lock().unwrap() = Some(target);
            }
        }
        save_observation(&client_worker, &observation_key, &mut reply);
        *worker.result.lock().unwrap() = Some(Completed {
            at: Instant::now(),
            expires: chrono::Utc::now() + chrono::Duration::minutes(5),
            reply,
        });
        worker.changed.send_replace(true);
        let _ = operation;
    });
    if !*changed.borrow() {
        let _finished = changed.changed().await;
    }
    snapshot(&record, &id, &key, false)
}
async fn run(
    client: &Client,
    name: &str,
    args: &Map<String, Value>,
    cancel: CancellationToken,
) -> Result<Reply> {
    if matches!(name, "rd_input_send" | "rd_terminal_write") {
        return tools::dispatch(client, name, args, &cancel).await;
    }
    tokio::select! {_=cancel.cancelled()=>Err(BridgeError::new("CANCELLED","Call was cancelled; already-sent input cannot be rolled back")),_=client.cancel.cancelled()=>Err(BridgeError::new("BINDING_EXPIRED","MCP logical client disconnected")),result=tools::dispatch(client,name,args,&cancel)=>result}
}
pub fn query(client: &Client, id: &str) -> Result<Value> {
    let record = client
        .operations
        .records
        .lock()
        .unwrap()
        .get(id)
        .cloned()
        .ok_or_else(|| BridgeError::new("OPERATION_EXPIRED", "Operation is absent or expired"))?;
    let reply = snapshot(&record, id, &cache_key(client, id), true)?;
    Ok(reply.value)
}
pub fn tick() {
    observations()
        .lock()
        .unwrap()
        .retain(|_, v| v.at.elapsed() < Duration::from_secs(30));
}
pub fn forget(client: &Client) {
    client.operations.records.lock().unwrap().clear();
    let prefix = format!("{}:", client.agent.id);
    observations()
        .lock()
        .unwrap()
        .retain(|key, _| !key.starts_with(&prefix));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{automation::sessions, flutter::FlutterHandler, ui_session_interface::Session};
    use uuid::Uuid;

    #[test]
    fn local_execution_does_not_claim_remote_completion() {
        assert_eq!(execution_state(None), ("running", "pending"));
        assert_eq!(execution_state(Some(&json!({"ok":true,"status":"pending"}))), ("finished", "unknown"));
        assert_eq!(execution_state(Some(&json!({"ok":true,"status":"completed"}))), ("finished", "reported"));
        assert_eq!(execution_state(Some(&json!({"ok":false,"status":"partial"}))), ("finished", "partial"));
        assert_eq!(execution_state(Some(&json!({"ok":false,"error":{"code":"CANCELLED"}}))), ("cancelled", "unknown"));
        assert_eq!(execution_state(Some(&json!({"ok":false,"error":{"code":"DISCONNECTED"}}))), ("failed", "unknown"));
    }

    #[tokio::test]
    async fn operation_wait_timeout_completion_and_expiry() {
        let client = Client::new();
        let (changed, _) = watch::channel(false);
        let record = Arc::new(Record {
            digest: [0; 32], tool: "rd_session_open".into(), target: Mutex::new(None),
            result: Mutex::new(None), changed,
        });
        client.operations.records.lock().unwrap().insert("job".into(), record.clone());
        let pending = wait_query(&client, "job", 1).await.unwrap();
        assert_eq!(pending["operation"]["execution_state"], "running");
        let worker = record.clone();
        let finish = async move {
            tokio::task::yield_now().await;
            *worker.result.lock().unwrap() = Some(Completed {
                at: Instant::now(), expires: chrono::Utc::now() + chrono::Duration::minutes(5),
                reply: Reply::success(json!({})).status("pending"),
            });
            worker.changed.send_replace(true);
        };
        let (result, ()) = tokio::join!(wait_query(&client, "job", 1000), finish);
        let result = result.unwrap();
        assert_eq!(result["operation"]["execution_state"], "finished");
        assert_eq!(result["operation"]["outcome"], "unknown");
        record.result.lock().unwrap().as_mut().unwrap().at = Instant::now() - Duration::from_secs(301);
        assert_eq!(query(&client, "job").unwrap_err().code, "OPERATION_EXPIRED");
        client.finish();
    }

    #[tokio::test]
    async fn concurrent_retries_share_one_record_and_replay_original_reference() {
        let core = Session::<FlutterHandler>::default();
        let view = Uuid::new_v4();
        sessions::add_view(&core, &view);
        let session = sessions::for_core(&core).unwrap();
        let client = Client::new();
        let args =
            json!({"session_id": session.snapshot().session_id,"operation_id":"attach-once"})
                .as_object()
                .unwrap()
                .clone();
        let (first, second) = tokio::join!(
            execute(
                client.clone(),
                "rd_session_attach",
                args.clone(),
                CancellationToken::new()
            ),
            execute(
                client.clone(),
                "rd_session_attach",
                args.clone(),
                CancellationToken::new()
            )
        );
        assert!(first.is_ok() && second.is_ok());
        assert_eq!(client.operations.records.lock().unwrap().len(), 1);
        let original = wait_query(&client, "attach-once", 0).await.unwrap();
        assert_eq!(original["operation"]["tool"], "rd_session_attach");
        assert_eq!(original["operation"]["execution_state"], "finished");
        let other = Client::new();
        assert_eq!(wait_query(&other, "attach-once", 0).await.unwrap_err().code, "OPERATION_EXPIRED");
        assert_eq!(wait_query(&client, "attach-once", 30001).await.unwrap_err().code, "INVALID_ARGUMENT");
        other.finish();
        let old = original["data"]["session"]["session_ref"].clone();
        session.control().release(None, false).unwrap();
        assert_ne!(json!(session.control().view().session_ref), old);
        let replay = execute(
            client.clone(),
            "rd_session_attach",
            args.clone(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(replay.value["data"]["session"]["session_ref"], old);
        assert_eq!(replay.value["operation"]["replayed"], true);
        let mut conflicting = args;
        conflicting.insert("session_id".into(), json!("another"));
        assert_eq!(
            execute(
                client.clone(),
                "rd_session_attach",
                conflicting,
                CancellationToken::new()
            )
            .await
            .err()
            .unwrap()
            .code,
            "OPERATION_CONFLICT"
        );
        client.finish();
        sessions::remove_view(&core, &view);
    }
}
