use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Map, Value};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicUsize, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::Notify,
    time::sleep,
};

struct Fixture {
    calls: AtomicUsize,
    finished: AtomicUsize,
    started: Notify,
    release: Notify,
}

impl ToolHandler for Arc<Fixture> {
    fn call(&self, context: CallContext, arguments: Map<String, Value>) -> ToolFuture {
        let fixture = self.clone();
        Box::pin(async move {
            let _finished = FinishGuard(fixture.clone());
            fixture.calls.fetch_add(1, Ordering::SeqCst);
            fixture.started.notify_one();
            match arguments.get("mode").and_then(Value::as_str) {
                Some("wait") => fixture.release.notified().await,
                Some("large") => return Ok(ToolResult::text("x".repeat(8192))),
                Some("error") => return Ok(ToolResult::error("Fixture failure")),
                Some("invalid") => return Err(RpcError::invalid_params("Fixture parameter error")),
                _ => {}
            }
            let mut result = ToolResult::text("fixture result");
            result.structured_content = Some(
                json!({"principal":context.principal,"ok":true})
                    .as_object()
                    .unwrap()
                    .clone(),
            );
            let png = STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGD4DwABBAEAX+XDSwAAAABJRU5ErkJggg==").unwrap();
            result.content.push(Content::image(&png, "image/png"));
            Ok(result)
        })
    }
}

struct TestServer {
    address: SocketAddr,
    token: String,
    principal: String,
    credentials: Arc<Credentials>,
    shutdown: CancellationToken,
    fixture: Arc<Fixture>,
    task: tokio::task::JoinHandle<io::Result<()>>,
}

impl TestServer {
    async fn start(limits: Limits) -> Self {
        let credentials = Credentials::new(8);
        let (principal, token) = credentials.issue().unwrap();
        let fixture = Arc::new(Fixture {
            calls: AtomicUsize::new(0),
            finished: AtomicUsize::new(0),
            started: Notify::new(),
            release: Notify::new(),
        });
        let mut config = ServerConfig::local(Implementation {
            name: "RustDesk test fixture".into(),
            version: "1".into(),
        });
        config.limits = limits;
        config.allowed_origins.push("https://client.example".into());
        let tool = RegisteredTool {
            definition: ToolDefinition {
                name: "fixture".into(),
                description: "Protocol tests only".into(),
                input_schema: json!({"type":"object","properties":{"mode":{"type":"string"}}}),
                output_schema: None,
                annotations: None,
            },
            handler: Arc::new(fixture.clone()),
        };
        let server = Server::bind(config, credentials.clone(), vec![tool])
            .await
            .unwrap();
        let address = server.local_addr().unwrap();
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(server.run(shutdown.clone()));
        Self {
            address,
            token,
            principal,
            credentials,
            shutdown,
            fixture,
            task,
        }
    }

    async fn post(
        &self,
        message: Value,
        version: Option<&str>,
        session: Option<&str>,
    ) -> (u16, HashMap<String, String>, Value) {
        let mut headers = Vec::new();
        if let Some(version) = version {
            headers.push(("MCP-Protocol-Version".into(), version.into()));
        }
        if let Some(session) = session {
            headers.push(("MCP-Session-Id".into(), session.into()));
        }
        if version == Some(CURRENT_VERSION) {
            headers.push((
                "Mcp-Method".into(),
                message["method"].as_str().unwrap().into(),
            ));
            if let Some(name) = message["params"]["name"].as_str() {
                headers.push(("Mcp-Name".into(), name.into()));
            }
        }
        raw(
            self.address,
            Some(&self.token),
            "POST",
            &headers,
            &message.to_string(),
        )
        .await
    }

    async fn legacy(&self) -> String {
        let (status, headers, result) = self.post(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
            "protocolVersion":LEGACY_VERSION,"capabilities":{},"clientInfo":{"name":"fixture","version":"1"}
        }}), None, None).await;
        assert_eq!(status, 200);
        assert_eq!(result["result"]["protocolVersion"], LEGACY_VERSION);
        let session = headers["mcp-session-id"].clone();
        let (status, _, _) = self
            .post(
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                Some(LEGACY_VERSION),
                Some(&session),
            )
            .await;
        assert_eq!(status, 202);
        session
    }

    async fn stop(self) {
        self.shutdown.cancel();
        self.task.await.unwrap().unwrap();
    }
}

fn modern(id: Value, method: &str, mut params: Value) -> Value {
    params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":CURRENT_VERSION,
        "io.modelcontextprotocol/clientCapabilities":{}});
    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
}

async fn raw(
    address: SocketAddr,
    token: Option<&str>,
    method: &str,
    extra: &[(String, String)],
    body: &str,
) -> (u16, HashMap<String, String>, Value) {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let request = wire(address, token, method, extra, body);
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    let response = String::from_utf8(response).unwrap();
    let (header, body) = response.split_once("\r\n\r\n").unwrap();
    let mut lines = header.lines();
    let status = lines
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let body = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(body).unwrap()
    };
    (status, headers, body)
}

fn wire(
    address: SocketAddr,
    token: Option<&str>,
    method: &str,
    extra: &[(String, String)],
    body: &str,
) -> String {
    let mut request = format!("{method} /mcp HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\n", body.len());
    if let Some(token) = token {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    for (name, value) in extra {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(body);
    request
}

#[tokio::test]
async fn two_protocol_lifecycles_and_real_tool_results() {
    let server = TestServer::start(Limits::default()).await;
    let session = server.legacy().await;
    let (_, _, list) = server
        .post(
            json!({"jsonrpc":"2.0","id":"list","method":"tools/list"}),
            Some(LEGACY_VERSION),
            Some(&session),
        )
        .await;
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 1);
    assert!(list["result"].get("resultType").is_none());
    let (_, _, discover) = server
        .post(
            modern(json!(1), "server/discover", json!({})),
            Some(CURRENT_VERSION),
            None,
        )
        .await;
    assert_eq!(discover["result"]["resultType"], "complete");
    assert_eq!(
        discover["result"]["supportedVersions"],
        json!([CURRENT_VERSION, LEGACY_VERSION])
    );
    let (_, _, result) = server
        .post(
            modern(
                json!(2),
                "tools/call",
                json!({"name":"fixture","arguments":{}}),
            ),
            Some(CURRENT_VERSION),
            None,
        )
        .await;
    assert_eq!(
        result["result"]["structuredContent"]["principal"],
        server.principal
    );
    assert!(result["result"]["content"][1]["data"]
        .as_str()
        .unwrap()
        .starts_with("iVBORw0KGgo"));
    let (_, _, list) = server
        .post(
            modern(json!(3), "tools/list", json!({})),
            Some(CURRENT_VERSION),
            None,
        )
        .await;
    assert_eq!(list["result"]["ttlMs"], 0);
    assert_eq!(list["result"]["cacheScope"], "private");
    server.stop().await;
}

#[tokio::test]
async fn authentication_host_origin_and_revocation() {
    let server = TestServer::start(Limits::default()).await;
    let (status, headers, _) = raw(server.address, None, "POST", &[], "{}").await;
    assert_eq!(status, 401);
    assert!(headers.contains_key("www-authenticate"));
    assert_eq!(
        raw(server.address, Some("invalid"), "POST", &[], "{}")
            .await
            .0,
        401
    );
    assert_eq!(
        raw(
            server.address,
            Some(&server.token),
            "POST",
            &[("Host".into(), "evil.test".into())],
            "{}"
        )
        .await
        .0,
        403
    );
    assert_eq!(
        raw(
            server.address,
            Some(&server.token),
            "POST",
            &[("Origin".into(), "https://evil.test".into())],
            "{}"
        )
        .await
        .0,
        403
    );
    assert_eq!(
        raw(
            server.address,
            Some(&server.token),
            "GET",
            &[("Origin".into(), "https://client.example".into())],
            ""
        )
        .await
        .0,
        405
    );
    let session = server.legacy().await;
    let (_, other_token) = server.credentials.issue().unwrap();
    assert_eq!(
        raw(
            server.address,
            Some(&other_token),
            "DELETE",
            &[("Mcp-Session-Id".into(), session.clone())],
            ""
        )
        .await
        .0,
        404
    );
    assert!(server.credentials.revoke(&server.principal));
    assert_eq!(
        server
            .post(
                json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
                Some(LEGACY_VERSION),
                Some(&session)
            )
            .await
            .0,
        401
    );
    server.stop().await;
}

#[tokio::test]
async fn errors_metadata_and_unadvertised_methods() {
    let server = TestServer::start(Limits::default()).await;
    assert_eq!(
        raw(server.address, Some(&server.token), "POST", &[], "[")
            .await
            .2["error"]["code"],
        -32700
    );
    for value in [
        json!([]),
        json!({"jsonrpc":"2.0","id":null,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":1.5,"method":"tools/list"}),
    ] {
        assert_eq!(
            server.post(value, None, None).await.2["error"]["code"],
            -32600
        );
    }
    let mut message = modern(json!(1), "tools/list", json!({}));
    message["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] = json!(LEGACY_VERSION);
    assert_eq!(
        server.post(message, Some(CURRENT_VERSION), None).await.2["error"]["code"],
        -32020
    );
    let (status, _, result) = server
        .post(
            modern(json!(2), "ping", json!({})),
            Some(CURRENT_VERSION),
            None,
        )
        .await;
    assert_eq!(status, 404);
    assert_eq!(result["error"]["code"], -32601);
    let (_, _, result) = server
        .post(
            modern(
                json!(3),
                "tools/call",
                json!({"name":"fixture","arguments":{"mode":"error"}}),
            ),
            Some(CURRENT_VERSION),
            None,
        )
        .await;
    assert_eq!(result["result"]["isError"], true);
    assert!(result.get("error").is_none());
    let (_, _, result) = server
        .post(
            modern(
                json!(4),
                "tools/call",
                json!({"name":"fixture","arguments":{"mode":"invalid"}}),
            ),
            Some(CURRENT_VERSION),
            None,
        )
        .await;
    assert_eq!(result["error"]["code"], -32602);
    assert_eq!(
        raw(server.address, Some(&server.token), "GET", &[], "")
            .await
            .0,
        405
    );
    server.stop().await;
}

#[tokio::test]
async fn bounded_body_and_expiring_sessions() {
    let server = TestServer::start(Limits {
        body_bytes: 512,
        session_timeout: Duration::from_millis(40),
        ..Limits::default()
    })
    .await;
    assert_eq!(
        raw(
            server.address,
            Some(&server.token),
            "POST",
            &[],
            &" ".repeat(513)
        )
        .await
        .0,
        413
    );
    let session = server.legacy().await;
    sleep(Duration::from_millis(70)).await;
    assert_eq!(
        server
            .post(
                json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
                Some(LEGACY_VERSION),
                Some(&session)
            )
            .await
            .0,
        404
    );
    server.stop().await;
}

#[tokio::test]
async fn explicit_cancel_and_duplicate_inflight_id() {
    let server = Arc::new(TestServer::start(Limits::default()).await);
    let session = server.legacy().await;
    let message = json!({"jsonrpc":"2.0","id":50,"method":"tools/call","params":{"name":"fixture","arguments":{"mode":"wait"}}});
    let client = server.clone();
    let call_session = session.clone();
    let call_message = message.clone();
    let call = tokio::spawn(async move {
        client
            .post(call_message, Some(LEGACY_VERSION), Some(&call_session))
            .await
    });
    server.fixture.started.notified().await;
    assert_eq!(
        server
            .post(message, Some(LEGACY_VERSION), Some(&session))
            .await
            .2["error"]["code"],
        -32602
    );
    assert_eq!(server.post(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":50}}), Some(LEGACY_VERSION), Some(&session)).await.0, 202);
    assert_eq!(call.await.unwrap().0, 202);
    assert_eq!(server.fixture.calls.load(Ordering::SeqCst), 1);
    server.shutdown.cancel();
}

#[tokio::test]
async fn revocation_and_timeout_stop_inflight_work() {
    let server = Arc::new(
        TestServer::start(Limits {
            request_timeout: Duration::from_millis(70),
            ..Limits::default()
        })
        .await,
    );
    let client = server.clone();
    let call = tokio::spawn(async move {
        client
            .post(
                modern(
                    json!(1),
                    "tools/call",
                    json!({"name":"fixture","arguments":{"mode":"wait"}}),
                ),
                Some(CURRENT_VERSION),
                None,
            )
            .await
    });
    server.fixture.started.notified().await;
    let (_, _, result) = call.await.unwrap();
    assert_eq!(result["error"]["code"], -32000);
    let client = server.clone();
    let call = tokio::spawn(async move {
        client
            .post(
                modern(
                    json!(2),
                    "tools/call",
                    json!({"name":"fixture","arguments":{"mode":"wait"}}),
                ),
                Some(CURRENT_VERSION),
                None,
            )
            .await
    });
    server.fixture.started.notified().await;
    server.credentials.revoke(&server.principal);
    assert_eq!(call.await.unwrap().0, 202);
    server.shutdown.cancel();
}

#[tokio::test]
async fn listener_rejects_non_loopback_configuration() {
    let mut config = ServerConfig::local(Implementation {
        name: "test".into(),
        version: "1".into(),
    });
    config.address.set_ip("0.0.0.0".parse().unwrap());
    assert!(Server::bind(config, Credentials::new(1), vec![])
        .await
        .is_err());
}

#[tokio::test]
#[ignore = "requires pinned official clients; see tests/mcp/README.md"]
async fn official_typescript_clients() {
    let server = TestServer::start(Limits::default()).await;
    let address = server.address;
    let token = server.token.clone();
    let script = std::env::var("MCP_INTEROP_SCRIPT").unwrap();
    let result = tokio::task::spawn_blocking(move || {
        std::process::Command::new("node")
            .arg(script)
            .env("MCP_TEST_URL", format!("http://{address}/mcp"))
            .env("MCP_TEST_TOKEN", token)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    server.stop().await;
}

struct FinishGuard(Arc<Fixture>);
impl Drop for FinishGuard {
    fn drop(&mut self) {
        self.0.finished.fetch_add(1, Ordering::SeqCst);
    }
}

async fn wait_finished(fixture: &Fixture, expected: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.finished.load(Ordering::SeqCst) != expected {
            sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn disconnect_semantics_differ_by_protocol_version() {
    let server = TestServer::start(Limits::default()).await;
    let message = modern(
        json!(1),
        "tools/call",
        json!({"name":"fixture","arguments":{"mode":"wait"}}),
    );
    let headers = vec![
        ("Mcp-Protocol-Version".into(), CURRENT_VERSION.into()),
        ("Mcp-Method".into(), "tools/call".into()),
        ("Mcp-Name".into(), "fixture".into()),
    ];
    let mut stream = TcpStream::connect(server.address).await.unwrap();
    stream
        .write_all(
            wire(
                server.address,
                Some(&server.token),
                "POST",
                &headers,
                &message.to_string(),
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    server.fixture.started.notified().await;
    drop(stream);
    wait_finished(&server.fixture, 1).await;

    let session = server.legacy().await;
    let message = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"fixture","arguments":{"mode":"wait"}}});
    let headers = vec![
        ("Mcp-Protocol-Version".into(), LEGACY_VERSION.into()),
        ("Mcp-Session-Id".into(), session.clone()),
    ];
    let mut stream = TcpStream::connect(server.address).await.unwrap();
    stream
        .write_all(
            wire(
                server.address,
                Some(&server.token),
                "POST",
                &headers,
                &message.to_string(),
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    server.fixture.started.notified().await;
    drop(stream);
    sleep(Duration::from_millis(30)).await;
    assert_eq!(server.fixture.finished.load(Ordering::SeqCst), 1);
    server
        .post(
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2}}),
            Some(LEGACY_VERSION),
            Some(&session),
        )
        .await;
    wait_finished(&server.fixture, 2).await;
    server.stop().await;
}

#[tokio::test]
async fn bounded_concurrency_and_shutdown_drain_handlers() {
    let server = TestServer::start(Limits {
        tool_calls: 1,
        ..Limits::default()
    })
    .await;
    let session = server.legacy().await;
    let message = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"fixture","arguments":{"mode":"wait"}}});
    let headers = vec![
        ("Mcp-Protocol-Version".into(), LEGACY_VERSION.into()),
        ("Mcp-Session-Id".into(), session),
    ];
    let mut stream = TcpStream::connect(server.address).await.unwrap();
    stream
        .write_all(
            wire(
                server.address,
                Some(&server.token),
                "POST",
                &headers,
                &message.to_string(),
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    server.fixture.started.notified().await;
    assert_eq!(
        server
            .post(
                modern(json!(2), "tools/call", json!({"name":"fixture"})),
                Some(CURRENT_VERSION),
                None
            )
            .await
            .0,
        429
    );
    let fixture = server.fixture.clone();
    server.stop().await;
    assert_eq!(fixture.finished.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn response_limit_and_credential_rotation() {
    let server = TestServer::start(Limits {
        response_bytes: 1024,
        ..Limits::default()
    })
    .await;
    let (status, _, result) = server
        .post(
            modern(
                json!(1),
                "tools/call",
                json!({"name":"fixture","arguments":{"mode":"large"}}),
            ),
            Some(CURRENT_VERSION),
            None,
        )
        .await;
    assert_eq!(status, 500);
    assert_eq!(result["id"], 1);
    assert_eq!(result["error"]["code"], -32603);
    let session = server.legacy().await;
    let original = server
        .credentials
        .authenticate(&server.token)
        .unwrap()
        .credential;
    server.credentials.revoke(&server.principal);
    server
        .credentials
        .insert(server.principal.clone(), &server.token)
        .unwrap();
    assert_ne!(
        original,
        server
            .credentials
            .authenticate(&server.token)
            .unwrap()
            .credential
    );
    assert_eq!(
        server
            .post(
                json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
                Some(LEGACY_VERSION),
                Some(&session)
            )
            .await
            .0,
        404
    );
    server.stop().await;
}

#[tokio::test]
async fn lifecycle_gates_and_notifications_do_not_execute_tools() {
    let server = TestServer::start(Limits::default()).await;
    let (_, headers, _) = server.post(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"unsupported","capabilities":{},"clientInfo":{"name":"test","version":"1"}
    }}), None, None).await;
    let session = &headers["mcp-session-id"];
    assert_eq!(
        server
            .post(
                json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
                Some(LEGACY_VERSION),
                Some(session)
            )
            .await
            .0,
        400
    );
    server
        .post(
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            Some(LEGACY_VERSION),
            Some(session),
        )
        .await;
    assert_eq!(
        server
            .post(
                json!({"jsonrpc":"2.0","method":"tools/call","params":{"name":"fixture"}}),
                Some(LEGACY_VERSION),
                Some(session)
            )
            .await
            .0,
        202
    );
    assert_eq!(server.fixture.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        raw(
            server.address,
            Some(&server.token),
            "DELETE",
            &[
                ("Mcp-Session-Id".into(), session.clone()),
                ("Mcp-Protocol-Version".into(), "unsupported".into())
            ],
            ""
        )
        .await
        .0,
        400
    );
    assert_eq!(
        raw(
            server.address,
            Some(&server.token),
            "DELETE",
            &[("Mcp-Session-Id".into(), session.clone())],
            ""
        )
        .await
        .0,
        200
    );
    assert_eq!(
        server
            .post(
                json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}),
                Some(LEGACY_VERSION),
                Some(session)
            )
            .await
            .0,
        404
    );
    server.stop().await;
}

#[tokio::test]
async fn transport_headers_versions_and_body_timeout() {
    let server = TestServer::start(Limits {
        body_timeout: Duration::from_millis(100),
        ..Limits::default()
    })
    .await;
    let message = modern(json!(1), "tools/call", json!({"name":"fixture"}));
    let headers = vec![
        ("Mcp-Protocol-Version".into(), CURRENT_VERSION.into()),
        ("Mcp-Method".into(), "tools/call".into()),
        ("Mcp-Name".into(), "=?base64?Zml4dHVyZQ==?=".into()),
    ];
    assert_eq!(
        raw(
            server.address,
            Some(&server.token),
            "POST",
            &headers,
            &message.to_string()
        )
        .await
        .0,
        200
    );
    let mut wrong = headers.clone();
    wrong[2].1 = "wrong".into();
    assert_eq!(
        raw(
            server.address,
            Some(&server.token),
            "POST",
            &wrong,
            &message.to_string()
        )
        .await
        .2["error"]["code"],
        -32020
    );
    wrong[0].1 = "unsupported".into();
    let result = raw(
        server.address,
        Some(&server.token),
        "POST",
        &wrong,
        &message.to_string(),
    )
    .await;
    assert_eq!(result.0, 400);
    assert_eq!(
        result.2["error"]["data"]["supported"],
        json!([CURRENT_VERSION, LEGACY_VERSION])
    );
    assert_eq!(
        raw(
            server.address,
            Some(&server.token),
            "POST",
            &[],
            &message.to_string()
        )
        .await
        .2["error"]["code"],
        -32020
    );
    assert_eq!(
        raw(
            server.address,
            Some(&server.token),
            "POST",
            &[("Content-Encoding".into(), "gzip".into())],
            "{}"
        )
        .await
        .0,
        415
    );
    let long_headers = vec![("X-Large".into(), "x".repeat(20 * 1024))];
    let status = raw(
        server.address,
        Some(&server.token),
        "POST",
        &long_headers,
        "{}",
    )
    .await
    .0;
    assert!(matches!(status, 400 | 431));
    let mut stream = TcpStream::connect(server.address).await.unwrap();
    let request = wire(server.address, Some(&server.token), "POST", &[], "{}");
    stream
        .write_all(request[..request.len() - 2].as_bytes())
        .await
        .unwrap();
    let mut result = String::new();
    tokio::time::timeout(Duration::from_secs(2), stream.read_to_string(&mut result))
        .await
        .unwrap()
        .unwrap();
    assert!(result.starts_with("HTTP/1.1 408"));
    server.stop().await;
}
