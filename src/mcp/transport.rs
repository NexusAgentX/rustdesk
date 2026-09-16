use super::{credentials, state, tools, Client};
use crate::automation::error::{BridgeError, Result};
use axum::{
    body::Body,
    extract::Request,
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    Router,
};
use hbb_common::{
    log,
    tokio::{self, net::TcpListener},
};
use rmcp::{
    model::*,
    service::{NotificationContext, RequestContext, RoleServer},
    transport::streamable_http_server::{
        session::{local::LocalSessionManager, SessionManager},
        StreamableHttpServerConfig, StreamableHttpService,
    },
    ErrorData, ServerHandler,
};
use std::{
    borrow::Cow,
    sync::{atomic::Ordering, Arc, Mutex, Weak},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
struct HandshakeLink(Arc<Mutex<Option<Weak<Client>>>>);
struct Handler {
    client: Arc<Client>,
    sessions: Arc<LocalSessionManager>,
}
impl Drop for Handler {
    fn drop(&mut self) {
        self.client.finish();
    }
}
impl ServerHandler for Handler {
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Owned(vec![ProtocolVersion::V_2025_11_25])
    }
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.protocol_version = ProtocolVersion::V_2025_11_25;
        info.server_info = Implementation::new("RustDesk MCP", "0.1.1");
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.instructions = Some("Operate visible RustDesk GUI sessions. Open or attach to obtain a session_ref; human control permits reads only. Request control explicitly and wait for approval. Refresh session_ref after handover or reconnect. Images use native PNG content blocks. Use operation_id before writes when retry safety matters; sent is transport evidence, not remote application acknowledgement. Keep the GET event stream open and answer server pings. Terminal I/O is raw interactive shell I/O, not command execution with individual exit codes.".into());
        info
    }
    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<InitializeResult, ErrorData> {
        let result = self.negotiate_initialize(&request)?;
        context.peer.set_peer_info(request.clone());
        *self.client.name.lock().unwrap() = request.client_info.name.chars().take(128).collect();
        *self.client.version.lock().unwrap() =
            request.client_info.version.chars().take(128).collect();
        if let Some(parts) = context.extensions.get::<axum::http::request::Parts>() {
            if let Some(link) = parts.extensions.get::<HandshakeLink>() {
                *link.0.lock().unwrap() = Some(Arc::downgrade(&self.client));
            }
        }
        let client = self.client.clone();
        let sessions = self.sessions.clone();
        tokio::spawn(async move {
            tokio::select! { _ = client.cancel.cancelled() => {}, _ = tokio::time::sleep(Duration::from_secs(10)) => {
                if !client.initialized.load(Ordering::Acquire) { close(&client, &sessions).await; }
            }}
        });
        Ok(result)
    }
    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        if !self.client.agent.is_alive() || self.client.initialized.swap(true, Ordering::AcqRel) {
            return;
        }
        self.client.touch();
        if let Some(parts) = context.extensions.get::<axum::http::request::Parts>() {
            if let Some(id) = parts
                .headers
                .get("mcp-session-id")
                .and_then(|v| v.to_str().ok())
            {
                *self.client.session_id.lock().unwrap() = Some(id.into());
            }
        }
        state()
            .clients
            .lock()
            .unwrap()
            .insert(self.client.agent.id.clone(), self.client.clone());
        let client = self.client.clone();
        let sessions = self.sessions.clone();
        client.agent.renew();
        tokio::spawn(async move {
            loop {
                tokio::select! { _ = client.cancel.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(15)) => {} }
                if !client.agent.is_alive() {
                    close(&client, &sessions).await;
                    break;
                }
                let response = tokio::time::timeout(
                    Duration::from_secs(15),
                    context
                        .peer
                        .send_request(ServerRequest::PingRequest(Default::default())),
                );
                let healthy = tokio::select! { _ = client.cancel.cancelled() => false, result = response => matches!(result, Ok(Ok(_))) };
                if !healthy || !client.agent.is_alive() {
                    close(&client, &sessions).await;
                    break;
                }
                client.agent.renew();
                client.touch();
            }
        });
    }
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, ErrorData> {
        self.client.touch();
        {
            let mut result = ListToolsResult::default();
            result.tools = tools::definitions();
            Ok(result)
        }
    }
    fn get_tool(&self, name: &str) -> Option<Tool> {
        tools::definitions()
            .into_iter()
            .find(|tool| tool.name == name)
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, ErrorData> {
        if self.get_tool(&request.name).is_none() {
            return Err(ErrorData::invalid_params("Unknown RustDesk tool", None));
        }
        self.client.touch();
        Ok(tools::call(
            self.client.clone(),
            &request.name,
            request.arguments.unwrap_or_default(),
            context.ct,
        )
        .await
        .into())
    }
}
async fn close(client: &Client, sessions: &LocalSessionManager) {
    client.finish();
    let id = client.session_id.lock().unwrap().clone();
    if let Some(id) = id {
        if sessions.close_session(&id.into()).await.is_err() {
            log::debug!("MCP logical session was already closed");
        }
    }
}

pub async fn listen(
    port: u16,
    token: String,
    shutdown: CancellationToken,
) -> Result<tokio::task::JoinHandle<()>> {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .map_err(|_| {
            BridgeError::new(
                "SERVICE_UNAVAILABLE",
                "Could not bind the configured MCP loopback port",
            )
        })?;
    let mut manager = LocalSessionManager::default();
    manager.session_config.init_timeout = Some(Duration::from_secs(10));
    manager.session_config.completed_cache_ttl = Duration::ZERO;
    let sessions = Arc::new(manager);
    let mut config = StreamableHttpServerConfig::default();
    config.legacy_session_mode = true;
    config.cancellation_token = shutdown.clone();
    config.max_request_body_bytes = 256 * 1024;
    config.allowed_hosts = vec![format!("127.0.0.1:{port}")];
    config.allowed_origins = vec![format!("http://127.0.0.1:{port}")];
    let factory_sessions = sessions.clone();
    let request_sessions = sessions.clone();
    let service = StreamableHttpService::new(
        move || {
            Ok(Handler {
                client: Client::new(),
                sessions: factory_sessions.clone(),
            })
        },
        sessions,
        config,
    );
    let server_shutdown = shutdown.clone();
    let response_slots = Arc::new(tokio::sync::Semaphore::new(8));
    let app = Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn(
            move |request: Request<Body>, next: Next| {
                authenticate(
                    request,
                    next,
                    token.clone(),
                    shutdown.clone(),
                    request_sessions.clone(),
                    response_slots.clone(),
                )
            },
        ));
    // Axum and the SDK share the existing application runtime.
    Ok(tokio::spawn(async move {
        if axum::serve(listener, app)
            .with_graceful_shutdown(server_shutdown.cancelled_owned())
            .await
            .is_err()
        {
            super::stop_now();
            super::failed("MCP HTTP listener failed");
        }
    }))
}
async fn authenticate(
    mut request: Request<Body>,
    next: Next,
    token: String,
    shutdown: CancellationToken,
    sessions: Arc<LocalSessionManager>,
    response_slots: Arc<tokio::sync::Semaphore>,
) -> Response {
    if shutdown.is_cancelled() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let authorized = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|candidate| credentials::matches(&token, candidate));
    if !authorized {
        return (StatusCode::UNAUTHORIZED, [("www-authenticate", "Bearer")]).into_response();
    }
    if !request.headers().contains_key("mcp-session-id")
        && sessions.sessions.read().await.len() >= 64
    {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let tool_response = if request.method() == axum::http::Method::POST {
        let (parts, body) = request.into_parts();
        let bytes = match axum::body::to_bytes(body, 256 * 1024).await {
            Ok(bytes) => bytes,
            Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
        };
        let is_tool = serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|v| {
                v.get("method")
                    .and_then(serde_json::Value::as_str)
                    .map(|m| m.starts_with("tools/"))
            })
            .unwrap_or(false);
        request = Request::from_parts(parts, Body::from(bytes));
        is_tool
    } else {
        false
    };
    // Heartbeat responses and cancellations must remain usable while tool responses are full.
    let response_slot = if tool_response {
        match response_slots.try_acquire_owned() {
            Ok(permit) => Some(permit),
            Err(_) => return StatusCode::TOO_MANY_REQUESTS.into_response(),
        }
    } else {
        None
    };
    let link = HandshakeLink::default();
    request.extensions_mut().insert(link.clone());
    let response = next.run(request).await;
    if let Some(id) = response
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
    {
        if let Some(client) = link.0.lock().unwrap().as_ref().and_then(Weak::upgrade) {
            *client.session_id.lock().unwrap() = Some(id.into());
        }
    }
    if let Some(permit) = response_slot {
        use hbb_common::futures_util::{stream, StreamExt};
        let (parts, body) = response.into_parts();
        let stream = stream::unfold(
            (body.into_data_stream(), permit),
            |(mut body, permit)| async move { body.next().await.map(|chunk| (chunk, (body, permit))) },
        );
        return Response::from_parts(parts, Body::from_stream(stream));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    fn decode(body: &str) -> Value {
        if body.trim_start().starts_with('{') {
            serde_json::from_str(body).unwrap()
        } else {
            let data = body
                .lines()
                .filter_map(|line| line.strip_prefix("data: "))
                .find(|line| line.starts_with('{'))
                .unwrap();
            serde_json::from_str(data).unwrap()
        }
    }
    async fn post(
        http: &reqwest::Client,
        url: &str,
        session: Option<&str>,
        body: Value,
    ) -> reqwest::Response {
        let mut request = http
            .post(url)
            .bearer_auth("test-token")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2025-11-25")
            .json(&body);
        if let Some(session) = session {
            request = request.header("Mcp-Session-Id", session);
        }
        request.send().await.unwrap()
    }
    async fn initialize(http: &reqwest::Client, url: &str, name: &str) -> String {
        let response=post(http,url,None,json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":name,"version":"test"}}})).await;
        assert!(response.status().is_success());
        let id = response
            .headers()
            .get("mcp-session-id")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let result = decode(&response.text().await.unwrap());
        assert_eq!(result["result"]["protocolVersion"], "2025-11-25");
        let initialized = post(
            http,
            url,
            Some(&id),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        )
        .await;
        assert!(initialized.status().is_success());
        id
    }
    #[tokio::test]
    async fn saturated_tool_responses_do_not_block_heartbeat_or_cancellation() {
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        let slots = Arc::new(tokio::sync::Semaphore::new(0));
        let sessions = Arc::new(LocalSessionManager::default());
        let app = Router::new()
            .route(
                "/mcp",
                axum::routing::post(|| async { StatusCode::NO_CONTENT }),
            )
            .layer(middleware::from_fn(
                move |request: Request<Body>, next: Next| {
                    authenticate(
                        request,
                        next,
                        "test-token".into(),
                        stop.clone(),
                        sessions.clone(),
                        slots.clone(),
                    )
                },
            ));
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let url = format!(
            "http://127.0.0.1:{}/mcp",
            listener.local_addr().unwrap().port()
        );
        let shutdown = cancel.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
                .unwrap();
        });
        let http = reqwest::Client::new();
        assert_eq!(
            post(
                &http,
                &url,
                None,
                json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})
            )
            .await
            .status()
            .as_u16(),
            429
        );
        for body in [
            json!({"jsonrpc":"2.0","id":2,"result":{}}),
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}),
        ] {
            assert_eq!(post(&http, &url, None, body).await.status().as_u16(), 204);
        }
        cancel.cancel();
        server.await.unwrap();
    }
    #[tokio::test]
    async fn authenticated_http_is_stateful_isolated_and_returns_all_schemas() {
        let socket = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = socket.local_addr().unwrap().port();
        drop(socket);
        let cancel = CancellationToken::new();
        let mut server = listen(port, "test-token".into(), cancel.clone())
            .await
            .unwrap();
        let url = format!("http://127.0.0.1:{port}/mcp");
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        for method in [
            reqwest::Method::GET,
            reqwest::Method::POST,
            reqwest::Method::DELETE,
        ] {
            let response = http.request(method, &url).send().await.unwrap();
            assert_eq!(response.status().as_u16(), 401);
        }
        let origin = http
            .get(&url)
            .bearer_auth("test-token")
            .header("Accept", "text/event-stream")
            .header("Origin", "https://untrusted.example")
            .send()
            .await
            .unwrap();
        assert_eq!(origin.status().as_u16(), 403);
        let oversized = http
            .post(&url)
            .bearer_auth("test-token")
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json")
            .body("x".repeat(256 * 1024 + 1))
            .send()
            .await
            .unwrap();
        assert_eq!(oversized.status().as_u16(), 413);
        let first = initialize(&http, &url, "transport-test-first").await;
        let second = initialize(&http, &url, "transport-test-second").await;
        assert_ne!(first, second);
        let response = post(
            &http,
            &url,
            Some(&first),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        )
        .await;
        let result = decode(&response.text().await.unwrap());
        let tools = result["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 22);
        for tool in tools {
            assert!(tool["inputSchema"].is_object());
            assert!(tool["outputSchema"].is_object());
        }
        let unknown=post(&http,&url,Some(&second),json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"rd_session_list","arguments":{"agent_id":"spoof"}}})).await;
        let result = decode(&unknown.text().await.unwrap());
        assert!(result["error"].is_object() || result["result"]["isError"] == true);
        let delete = http
            .delete(&url)
            .bearer_auth("test-token")
            .header("Mcp-Session-Id", &first)
            .send()
            .await
            .unwrap();
        assert!(delete.status().is_success());
        let remaining=post(&http,&url,Some(&second),json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"rd_session_list","arguments":{}}})).await;
        let result = decode(&remaining.text().await.unwrap());
        assert_eq!(result["result"]["structuredContent"]["ok"], true);
        cancel.cancel();
        assert!(tokio::time::timeout(Duration::from_secs(3), &mut server)
            .await
            .is_ok());
    }
}
