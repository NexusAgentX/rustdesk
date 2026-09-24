use super::{protocol, security::Principal, Server, State};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    body::Incoming,
    header::{self, HeaderMap, HeaderValue},
    server::conn::http1,
    service::service_fn,
    Method, Request, Response, StatusCode,
};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde_json::{json, Value};
use std::{
    convert::Infallible,
    io::{self, Write},
    sync::Arc,
};
use tokio::{
    sync::Semaphore,
    task::JoinSet,
    time::{interval, timeout},
};
use tokio_util::sync::CancellationToken;

pub(super) struct Reply {
    pub status: StatusCode,
    pub body: Option<Value>,
    pub session: Option<String>,
}

impl Reply {
    pub fn empty(status: StatusCode) -> Self {
        Self {
            status,
            body: None,
            session: None,
        }
    }

    pub fn error(status: StatusCode, id: Option<Value>, code: i32, message: &str) -> Self {
        let mut body = json!({"jsonrpc":"2.0", "error":{"code":code,"message":message}});
        if let Some(id) = id {
            body["id"] = id;
        }
        Self {
            status,
            body: Some(body),
            session: None,
        }
    }

    fn response(self, maximum: usize) -> Response<Full<Bytes>> {
        let mut response = Response::new(Full::new(Bytes::new()));
        *response.status_mut() = self.status;
        let headers = response.headers_mut();
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        headers.insert(
            "x-content-type-options",
            HeaderValue::from_static("nosniff"),
        );
        if self.status == StatusCode::UNAUTHORIZED {
            headers.insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static("Bearer realm=\"RustDesk MCP\""),
            );
        }
        if self.status == StatusCode::METHOD_NOT_ALLOWED {
            headers.insert(header::ALLOW, HeaderValue::from_static("POST, DELETE"));
        }
        if let Some(session) = self.session {
            if let Ok(value) = HeaderValue::from_str(&session) {
                headers.insert("mcp-session-id", value);
            }
        }
        if let Some(body) = self.body {
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            let mut writer = BoundedWriter {
                bytes: Vec::new(),
                maximum,
            };
            if serde_json::to_writer(&mut writer, &body).is_ok() {
                *response.body_mut() = Full::new(Bytes::from(writer.bytes));
            } else {
                *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                // Keep the ID without attempting to serialize the oversized tool result again.
                let failure = Reply::error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    body.get("id").cloned(),
                    -32603,
                    "Response exceeds the server limit",
                );
                let mut fallback = BoundedWriter {
                    bytes: Vec::new(),
                    maximum,
                };
                if let Some(body) = failure.body {
                    if serde_json::to_writer(&mut fallback, &body).is_ok() {
                        *response.body_mut() = Full::new(Bytes::from(fallback.bytes));
                    } else {
                        *response.body_mut() = Full::new(Bytes::from_static(br#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"Response exceeds the server limit"}}"#));
                    }
                }
            }
        }
        response
    }
}

struct BoundedWriter {
    bytes: Vec<u8>,
    maximum: usize,
}

impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.maximum.saturating_sub(self.bytes.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "MCP response limit",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct StopOnDrop(CancellationToken);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

pub(super) async fn serve(server: Server, shutdown: CancellationToken) -> io::Result<()> {
    let address = server.local_addr()?;
    let state = server.state;
    let _stop = StopOnDrop(state.stopped.clone());
    let slots = Arc::new(Semaphore::new(state.config.limits.connections));
    let mut connections = JoinSet::new();
    let mut cleanup = interval(
        state
            .config
            .limits
            .session_timeout
            .min(std::time::Duration::from_secs(30)),
    );
    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            _ = cleanup.tick() => state.expire_sessions(),
            result = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(error)) = result {
                    tracing::error!(%error, "MCP connection task failed");
                }
            }
            result = server.listener.accept() => {
                let (socket, peer) = result?;
                if !peer.ip().is_loopback() { continue; }
                let permit = match slots.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => continue,
                };
                let state = state.clone();
                connections.spawn(async move {
                    let _permit = permit;
                    let mut builder = http1::Builder::new();
                    builder.timer(TokioTimer::new()).header_read_timeout(state.config.limits.body_timeout)
                        .max_headers(state.config.limits.header_count).max_buf_size(state.config.limits.header_bytes);
                    let service_state = state.clone();
                    let service = service_fn(move |request| {
                        let state = service_state.clone();
                        async move {
                            let maximum = state.config.limits.response_bytes;
                            Ok::<_, Infallible>(handle(state, address.port(), request).await.response(maximum))
                        }
                    });
                    tokio::select! {
                        _ = state.stopped.cancelled() => {},
                        result = builder.serve_connection(TokioIo::new(socket), service) => {
                            if result.is_err() { tracing::debug!("MCP HTTP connection ended with a transport error"); }
                        }
                    }
                });
            }
        }
    }
    state.stopped.cancel();
    while let Some(result) = connections.join_next().await {
        if let Err(error) = result {
            tracing::error!(%error, "MCP connection task failed during shutdown");
        }
    }
    // Mobile resume must not rebind until cancelled handlers have dropped their input guards.
    loop {
        let drained = state.drained.notified();
        if state.active.lock().unwrap().is_empty() {
            break;
        }
        drained.await;
    }
    Ok(())
}

pub(super) fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, ()> {
    let mut values = headers.get_all(name).iter();
    match values.next() {
        None => Ok(None),
        Some(value) if values.next().is_none() => value.to_str().map(Some).map_err(|_| ()),
        Some(_) => Err(()),
    }
}

fn authenticate(state: &State, port: u16, headers: &HeaderMap) -> Result<Principal, Reply> {
    let forbidden = || {
        Reply::error(
            StatusCode::FORBIDDEN,
            None,
            -32000,
            "Forbidden local endpoint",
        )
    };
    let host = single_header(headers, "host")
        .map_err(|_| forbidden())?
        .ok_or_else(forbidden)?;
    if ![
        format!("127.0.0.1:{port}"),
        format!("[::1]:{port}"),
        format!("localhost:{port}"),
    ]
    .iter()
    .any(|allowed| allowed.eq_ignore_ascii_case(host))
    {
        return Err(forbidden());
    }
    if let Some(origin) = single_header(headers, "origin").map_err(|_| forbidden())? {
        if !state
            .config
            .allowed_origins
            .iter()
            .any(|allowed| allowed == origin)
        {
            return Err(forbidden());
        }
    }
    let unauthorized = || {
        Reply::error(
            StatusCode::UNAUTHORIZED,
            None,
            -32000,
            "Authentication required",
        )
    };
    let authorization = single_header(headers, "authorization")
        .map_err(|_| unauthorized())?
        .ok_or_else(unauthorized)?;
    let (scheme, token) = authorization.split_once(' ').ok_or_else(unauthorized)?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return Err(unauthorized());
    }
    state
        .credentials
        .authenticate(token)
        .ok_or_else(unauthorized)
}

async fn handle(state: Arc<State>, port: u16, request: Request<Incoming>) -> Reply {
    let principal = match authenticate(&state, port, request.headers()) {
        Ok(principal) => principal,
        Err(reply) => return reply,
    };
    if request.uri().path() != "/mcp"
        || request.uri().query().is_some()
        || request.uri().authority().is_some()
    {
        return Reply::empty(StatusCode::NOT_FOUND);
    }
    if request.method() == Method::DELETE {
        return protocol::delete_session(&state, &principal, request.headers());
    }
    if request.method() != Method::POST {
        return Reply::empty(StatusCode::METHOD_NOT_ALLOWED);
    }
    let _permit = match state.requests.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => return Reply::empty(StatusCode::TOO_MANY_REQUESTS),
    };
    let (parts, body) = request.into_parts();
    let content_type = single_header(&parts.headers, "content-type");
    if !matches!(content_type, Ok(Some(value)) if value.split(';').next().is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json")))
        || parts.headers.contains_key(header::CONTENT_ENCODING)
    {
        return Reply::empty(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    let accepts = parts
        .headers
        .get_all(header::ACCEPT)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|value| {
            let mut parts = value.split(';');
            let kind = parts.next()?.trim().to_ascii_lowercase();
            for parameter in parts {
                if let Some((key, value)) = parameter.trim().split_once('=') {
                    if key.eq_ignore_ascii_case("q")
                        && !value
                            .parse::<f32>()
                            .is_ok_and(|quality| quality > 0.0 && quality <= 1.0)
                    {
                        return None;
                    }
                }
            }
            Some(kind)
        })
        .collect::<Vec<_>>();
    if !["application/json", "text/event-stream"]
        .iter()
        .all(|kind| accepts.iter().any(|value| value == kind))
    {
        return Reply::empty(StatusCode::NOT_ACCEPTABLE);
    }
    let body = match timeout(
        state.config.limits.body_timeout,
        Limited::new(body, state.config.limits.body_bytes).collect(),
    )
    .await
    {
        Ok(Ok(body)) => body.to_bytes(),
        Ok(Err(error)) => {
            return Reply::empty(if error.is::<http_body_util::LengthLimitError>() {
                StatusCode::PAYLOAD_TOO_LARGE
            } else {
                StatusCode::BAD_REQUEST
            });
        }
        Err(_) => return Reply::empty(StatusCode::REQUEST_TIMEOUT),
    };
    if principal.revoked.is_cancelled() || state.stopped.is_cancelled() {
        return Reply::empty(StatusCode::UNAUTHORIZED);
    }
    protocol::dispatch(state, principal, parts.headers, &body).await
}
