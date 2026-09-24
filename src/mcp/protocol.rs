use super::{
    http::{single_header, Reply},
    model::{CallContext, ProtocolVersion, RpcError, CURRENT_VERSION, LEGACY_VERSION},
    security::Principal,
    RequestKey, Session, State,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use hyper::{HeaderMap, StatusCode};
use serde_json::{json, Map, Value};
use std::{sync::Arc, time::Instant};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn failure(id: Option<Value>, error: RpcError) -> Reply {
    let mut body = json!({"jsonrpc":"2.0", "error":error});
    if let Some(id) = id {
        body["id"] = id;
    }
    Reply {
        status: StatusCode::OK,
        body: Some(body),
        session: None,
    }
}

fn success(state: &State, version: ProtocolVersion, id: Value, mut result: Value) -> Reply {
    if version == ProtocolVersion::Current {
        result["resultType"] = json!("complete");
        result["_meta"] = json!({"io.modelcontextprotocol/serverInfo":state.config.info});
    }
    Reply {
        status: StatusCode::OK,
        body: Some(json!({"jsonrpc":"2.0","id":id,"result":result})),
        session: None,
    }
}

fn invalid(id: Option<Value>, message: &str) -> Reply {
    let status = if id.is_some() {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    let mut reply = failure(id, RpcError::invalid_params(message));
    reply.status = status;
    reply
}

fn request_id(value: &Value) -> bool {
    value.as_str().is_some_and(|id| id.len() <= 256) || value.is_i64() || value.is_u64()
}

fn version_error(id: Option<Value>, requested: &str) -> Reply {
    let mut error = RpcError::new(-32022, "Unsupported protocol version");
    error.data = Some(json!({"requested":requested,"supported":[CURRENT_VERSION,LEGACY_VERSION]}));
    let mut reply = failure(id, error);
    reply.status = StatusCode::BAD_REQUEST;
    reply
}

fn header_mismatch(id: Option<Value>) -> Reply {
    Reply::error(
        StatusCode::BAD_REQUEST,
        id,
        -32020,
        "Missing, malformed or mismatched MCP header",
    )
}

fn check_current(
    headers: &HeaderMap,
    method: &str,
    params: &Map<String, Value>,
    id: Option<Value>,
) -> Result<(), Reply> {
    if single_header(headers, "mcp-method") != Ok(Some(method)) {
        return Err(header_mismatch(id));
    }
    let meta = params
        .get("_meta")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(id.clone(), "Request metadata required"))?;
    if meta
        .get("io.modelcontextprotocol/protocolVersion")
        .and_then(Value::as_str)
        != Some(CURRENT_VERSION)
    {
        return Err(header_mismatch(id));
    }
    if !meta
        .get("io.modelcontextprotocol/clientCapabilities")
        .is_some_and(Value::is_object)
    {
        return Err(invalid(id, "Client capabilities required"));
    }
    if let Some(info) = meta.get("io.modelcontextprotocol/clientInfo") {
        if !valid_info(info) {
            return Err(invalid(id, "Invalid client information"));
        }
    }
    let name = match method {
        "tools/call" | "prompts/get" => params.get("name"),
        "resources/read" => params.get("uri"),
        _ => None,
    };
    if let Some(name) = name {
        let name = name
            .as_str()
            .ok_or_else(|| invalid(id.clone(), "Invalid name"))?;
        let header = single_header(headers, "mcp-name")
            .map_err(|_| header_mismatch(id.clone()))?
            .ok_or_else(|| header_mismatch(id.clone()))?;
        let decoded;
        let header = if let Some(encoded) = header
            .strip_prefix("=?base64?")
            .and_then(|value| value.strip_suffix("?="))
        {
            decoded = STANDARD
                .decode(encoded)
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .ok_or_else(|| header_mismatch(id.clone()))?;
            decoded.as_str()
        } else {
            header
        };
        if header != name {
            return Err(header_mismatch(id));
        }
    }
    Ok(())
}

fn valid_info(info: &Value) -> bool {
    info.get("name").is_some_and(Value::is_string)
        && info.get("version").is_some_and(Value::is_string)
}

fn legacy_session(
    state: &State,
    principal: &Principal,
    headers: &HeaderMap,
    ready: bool,
) -> Result<(String, CancellationToken), Reply> {
    let id = single_header(headers, "mcp-session-id")
        .map_err(|_| Reply::empty(StatusCode::BAD_REQUEST))?
        .ok_or_else(|| Reply::empty(StatusCode::BAD_REQUEST))?;
    state.expire_sessions();
    let mut sessions = state.sessions.lock().unwrap();
    match sessions.get_mut(id) {
        Some(session)
            if session.principal.credential == principal.credential
                && !session.principal.revoked.is_cancelled() =>
        {
            if ready && !session.ready {
                return Err(Reply::error(
                    StatusCode::BAD_REQUEST,
                    None,
                    -32000,
                    "Session is not initialized",
                ));
            }
            session.touched = Instant::now();
            Ok((id.to_owned(), session.cancellation.clone()))
        }
        _ => Err(Reply::empty(StatusCode::NOT_FOUND)),
    }
}

pub(super) fn delete_session(state: &State, principal: &Principal, headers: &HeaderMap) -> Reply {
    match single_header(headers, "mcp-protocol-version") {
        Ok(Some(CURRENT_VERSION)) => return Reply::empty(StatusCode::METHOD_NOT_ALLOWED),
        Ok(Some(LEGACY_VERSION) | None) => {}
        Ok(Some(version)) => return version_error(None, version),
        Err(_) => return header_mismatch(None),
    }
    let (id, cancellation) = match legacy_session(state, principal, headers, false) {
        Ok(session) => session,
        Err(reply) => return reply,
    };
    state.sessions.lock().unwrap().remove(&id);
    cancellation.cancel();
    Reply::empty(StatusCode::OK)
}

pub(super) async fn dispatch(
    state: Arc<State>,
    principal: Principal,
    headers: HeaderMap,
    bytes: &[u8],
) -> Reply {
    let message: Value = match serde_json::from_slice(bytes) {
        Ok(message) => message,
        Err(_) => return Reply::error(StatusCode::BAD_REQUEST, None, -32700, "Invalid JSON"),
    };
    let object = match message.as_object() {
        Some(object) => object,
        None => {
            return Reply::error(
                StatusCode::BAD_REQUEST,
                None,
                -32600,
                "Expected one JSON-RPC message",
            )
        }
    };
    let id = object.get("id").cloned();
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || id.as_ref().is_some_and(|id| !request_id(id))
    {
        return Reply::error(
            StatusCode::BAD_REQUEST,
            None,
            -32600,
            "Invalid JSON-RPC envelope",
        );
    }
    let method = match object.get("method").and_then(Value::as_str) {
        Some(method)
            if method.len() <= 128
                && !object.contains_key("result")
                && !object.contains_key("error") =>
        {
            method
        }
        _ => {
            return Reply::error(
                StatusCode::BAD_REQUEST,
                id,
                -32600,
                "Expected a request or notification",
            )
        }
    };
    let empty = Map::new();
    let params = match object.get("params") {
        None => &empty,
        Some(Value::Object(params)) => params,
        Some(_) => return invalid(id, "Parameters must be an object"),
    };
    if method == "initialize"
        && single_header(&headers, "mcp-protocol-version") != Ok(Some(CURRENT_VERSION))
    {
        let id = match id {
            Some(id) => id,
            None => return Reply::empty(StatusCode::BAD_REQUEST),
        };
        if params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .is_none()
            || !params.get("capabilities").is_some_and(Value::is_object)
            || !params.get("clientInfo").is_some_and(valid_info)
        {
            return invalid(Some(id), "Invalid initialization parameters");
        }
        if headers.contains_key("mcp-session-id") {
            return invalid(Some(id), "Initialize without a session ID");
        }
        state.expire_sessions();
        let session_id = Uuid::new_v4().to_string();
        {
            let mut sessions = state.sessions.lock().unwrap();
            if sessions.len() >= state.config.limits.sessions {
                return Reply::empty(StatusCode::TOO_MANY_REQUESTS);
            }
            sessions.insert(
                session_id.clone(),
                Session {
                    principal: principal.clone(),
                    ready: false,
                    touched: Instant::now(),
                    cancellation: principal.revoked.child_token(),
                },
            );
        }
        let mut reply = success(
            &state,
            ProtocolVersion::Legacy,
            id,
            json!({
                "protocolVersion":LEGACY_VERSION,"capabilities":{"tools":{}},"serverInfo":state.config.info
            }),
        );
        reply.session = Some(session_id);
        return reply;
    }
    let version_header = match single_header(&headers, "mcp-protocol-version") {
        Ok(header) => header,
        Err(_) => return header_mismatch(id),
    };
    let (version, session, cancellation) = match version_header {
        Some(CURRENT_VERSION) => {
            if let Err(reply) = check_current(&headers, method, params, id.clone()) {
                return reply;
            }
            (
                ProtocolVersion::Current,
                None,
                principal.revoked.child_token(),
            )
        }
        Some(LEGACY_VERSION) | None => {
            if version_header.is_none() && !headers.contains_key("mcp-session-id") {
                return header_mismatch(id);
            }
            let ready = !matches!(method, "notifications/initialized" | "ping");
            let (session, cancellation) = match legacy_session(&state, &principal, &headers, ready)
            {
                Ok(session) => session,
                Err(reply) => return reply,
            };
            (ProtocolVersion::Legacy, Some(session), cancellation)
        }
        Some(version) => return version_error(id, version),
    };
    let id = match id {
        Some(id) => id,
        None => {
            if version == ProtocolVersion::Legacy {
                match method {
                    "notifications/initialized" => {
                        if let Some(id) = &session {
                            if let Some(session) = state.sessions.lock().unwrap().get_mut(id) {
                                session.ready = true;
                            }
                        }
                    }
                    "notifications/cancelled" => {
                        if let Some(id) = params.get("requestId").filter(|id| request_id(id)) {
                            let key = RequestKey {
                                principal: principal.id.clone(),
                                credential: principal.credential.clone(),
                                session,
                                id: id.to_string(),
                            };
                            if let Some(token) = state.active.lock().unwrap().get(&key) {
                                token.cancel();
                            }
                        }
                    }
                    _ => {}
                }
            }
            return Reply::empty(StatusCode::ACCEPTED);
        }
    };
    match method {
        "server/discover" if version == ProtocolVersion::Current => success(
            &state,
            version,
            id,
            json!({
                "supportedVersions":[CURRENT_VERSION,LEGACY_VERSION],"capabilities":{"tools":{}},"ttlMs":0,"cacheScope":"private"
            }),
        ),
        "ping" if version == ProtocolVersion::Legacy => success(&state, version, id, json!({})),
        "tools/list" => {
            if params.contains_key("cursor") {
                return invalid(Some(id), "No pagination cursor is available");
            }
            let mut result = json!({"tools":state.tools.values().map(|tool| &tool.definition).collect::<Vec<_>>()});
            if version == ProtocolVersion::Current {
                result["ttlMs"] = json!(0);
                result["cacheScope"] = json!("private");
            }
            success(&state, version, id, result)
        }
        "tools/call" => {
            let name = match params.get("name").and_then(Value::as_str) {
                Some(name) => name,
                None => return invalid(Some(id), "Tool name required"),
            };
            let tool = match state.tools.get(name) {
                Some(tool) => tool,
                None => return invalid(Some(id), "Unknown tool"),
            };
            let arguments = match params.get("arguments") {
                None => Map::new(),
                Some(Value::Object(arguments)) => arguments.clone(),
                Some(_) => return invalid(Some(id), "Tool arguments must be an object"),
            };
            if params.contains_key("task") || params.contains_key("inputResponses") {
                return invalid(
                    Some(id),
                    "Tasks and multi round-trip requests are not supported",
                );
            }
            let permit = match state.calls.clone().try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => {
                    return Reply::error(
                        StatusCode::TOO_MANY_REQUESTS,
                        Some(id),
                        -32000,
                        "Too many tool calls",
                    )
                }
            };
            let key = RequestKey {
                principal: principal.id.clone(),
                credential: principal.credential.clone(),
                session,
                id: id.to_string(),
            };
            let cancellation = cancellation.child_token();
            {
                let mut active = state.active.lock().unwrap();
                if active.contains_key(&key) {
                    return invalid(Some(id), "Request ID is already active");
                }
                active.insert(key.clone(), cancellation.clone());
            }
            let disconnect = if version == ProtocolVersion::Current {
                Some(cancellation.clone())
            } else {
                None
            };
            let context = CallContext {
                principal: principal.id,
                credential: principal.credential,
                protocol_version: version,
                cancellation: cancellation.clone(),
            };
            let handler = tool.handler.clone();
            let worker_state = state.clone();
            let worker_id = id.clone();
            let worker = tokio::spawn(async move {
                let _permit = permit;
                let _active = ActiveRequest {
                    state: worker_state.clone(),
                    key,
                    cancellation: cancellation.clone(),
                };
                tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => Reply::empty(StatusCode::ACCEPTED),
                    _ = worker_state.stopped.cancelled() => Reply::empty(StatusCode::SERVICE_UNAVAILABLE),
                    result = timeout(worker_state.config.limits.request_timeout, handler.call(context, arguments)) => {
                        match result {
                            Ok(Ok(result)) => match serde_json::to_value(result) {
                                Ok(result) => success(&worker_state, version, worker_id, result),
                                Err(_) => failure(Some(worker_id), RpcError::new(-32603, "Invalid tool result")),
                            },
                            Ok(Err(error)) => failure(Some(worker_id), error),
                            Err(_) => failure(Some(worker_id), RpcError::new(-32000, "Tool call timed out")),
                        }
                    }
                }
            });
            // Legacy HTTP disconnection is not cancellation. Its bounded worker stays alive.
            let _disconnect = DisconnectGuard {
                cancellation: disconnect,
            };
            match worker.await {
                Ok(reply) => reply,
                Err(error) => {
                    tracing::error!(%error, "MCP tool task failed");
                    failure(Some(id), RpcError::new(-32603, "Tool task failed"))
                }
            }
        }
        _ => {
            let mut reply = failure(Some(id), RpcError::new(-32601, "Method not found"));
            if version == ProtocolVersion::Current {
                reply.status = StatusCode::NOT_FOUND;
            }
            reply
        }
    }
}

struct ActiveRequest {
    state: Arc<State>,
    key: RequestKey,
    cancellation: CancellationToken,
}

impl Drop for ActiveRequest {
    fn drop(&mut self) {
        self.cancellation.cancel();
        self.state.active.lock().unwrap().remove(&self.key);
        self.state.drained.notify_one();
    }
}

struct DisconnectGuard {
    cancellation: Option<CancellationToken>,
}
impl Drop for DisconnectGuard {
    fn drop(&mut self) {
        if let Some(token) = &self.cancellation {
            token.cancel();
        }
    }
}
