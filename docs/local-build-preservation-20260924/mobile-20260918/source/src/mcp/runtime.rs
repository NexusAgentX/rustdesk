use super::{Credentials, RegisteredTool, Server, ServerConfig};
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
};
use tokio::{runtime::Handle, sync::watch};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Disabled,
    Suspended,
    Starting,
    Running(SocketAddr),
    Failed(String),
}

#[derive(Clone)]
struct Settings {
    config: ServerConfig,
    credentials: Arc<Credentials>,
    tools: Vec<RegisteredTool>,
}

#[derive(Clone)]
struct Desired {
    generation: u64,
    foreground: bool,
    settings: Option<Settings>,
}

struct Current {
    generation: u64,
    cancellation: CancellationToken,
}

struct Inner {
    desired: watch::Sender<Desired>,
    status: watch::Receiver<Status>,
    current: Arc<Mutex<Current>>,
    shutdown: CancellationToken,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.current.lock().unwrap().cancellation.cancel();
    }
}

#[derive(Clone)]
pub struct Service(Arc<Inner>);

impl Service {
    pub fn attach(handle: &Handle) -> Self {
        let (desired, receiver) = watch::channel(Desired {
            generation: 0,
            foreground: true,
            settings: None,
        });
        let (status, status_receiver) = watch::channel(Status::Disabled);
        let current = Arc::new(Mutex::new(Current {
            generation: 0,
            cancellation: CancellationToken::new(),
        }));
        let shutdown = CancellationToken::new();
        handle.spawn(supervise(
            receiver,
            status,
            current.clone(),
            shutdown.clone(),
        ));
        Self(Arc::new(Inner {
            desired,
            status: status_receiver,
            current,
            shutdown,
        }))
    }

    pub fn enable(
        &self,
        config: ServerConfig,
        credentials: Arc<Credentials>,
        tools: Vec<RegisteredTool>,
    ) {
        self.update(|desired| {
            desired.settings = Some(Settings {
                config,
                credentials,
                tools,
            });
            true
        });
    }

    pub fn disable(&self) {
        self.update(|desired| desired.settings.take().is_some());
    }

    pub fn set_foreground(&self, foreground: bool) {
        self.update(|desired| {
            if desired.foreground == foreground {
                return false;
            }
            desired.foreground = foreground;
            true
        });
    }

    pub fn subscribe(&self) -> watch::Receiver<Status> {
        self.0.status.clone()
    }

    fn update(&self, change: impl FnOnce(&mut Desired) -> bool) {
        let mut current = self.0.current.lock().unwrap();
        self.0.desired.send_if_modified(|desired| {
            if !change(desired) {
                return false;
            }
            current.cancellation.cancel();
            current.generation = current.generation.wrapping_add(1);
            desired.generation = current.generation;
            true
        });
    }
}

async fn supervise(
    mut desired: watch::Receiver<Desired>,
    status: watch::Sender<Status>,
    current: Arc<Mutex<Current>>,
    shutdown: CancellationToken,
) {
    loop {
        let snapshot = desired.borrow_and_update().clone();
        let settings = snapshot.settings.filter(|_| snapshot.foreground);
        if let Some(settings) = settings {
            status.send_replace(Status::Starting);
            let binding = Server::bind(settings.config, settings.credentials, settings.tools);
            let server = tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                change = desired.changed() => {
                    if change.is_err() { break; }
                    continue;
                }
                result = binding => result,
            };
            match server {
                Ok(server) => {
                    let address = match server.local_addr() {
                        Ok(address) => address,
                        Err(error) => {
                            status.send_replace(Status::Failed(error.to_string()));
                            if !wait_for_change(&mut desired, &shutdown).await {
                                break;
                            }
                            continue;
                        }
                    };
                    let cancellation = CancellationToken::new();
                    {
                        let mut active = current.lock().unwrap();
                        if active.generation != snapshot.generation {
                            continue;
                        }
                        active.cancellation = cancellation.clone();
                    }
                    status.send_replace(Status::Running(address));
                    let mut task = tokio::spawn(server.run(cancellation.clone()));
                    tokio::select! {
                        biased;
                        _ = shutdown.cancelled() => {},
                        _ = desired.changed() => {},
                        result = &mut task => {
                            let message = match result {
                                Ok(Ok(())) => "MCP listener stopped".to_owned(),
                                Ok(Err(error)) => error.to_string(),
                                Err(error) => error.to_string(),
                            };
                            status.send_replace(Status::Failed(message));
                            if !wait_for_change(&mut desired, &shutdown).await { break; }
                            continue;
                        }
                    }
                    cancellation.cancel();
                    match task.await {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => tracing::error!(%error, "MCP listener cleanup failed"),
                        Err(error) => tracing::error!(%error, "MCP listener task failed"),
                    }
                    if shutdown.is_cancelled() {
                        break;
                    }
                    continue;
                }
                Err(error) => {
                    status.send_replace(Status::Failed(error.to_string()));
                }
            }
        } else {
            status.send_replace(if snapshot.foreground {
                Status::Disabled
            } else {
                Status::Suspended
            });
        }
        if !wait_for_change(&mut desired, &shutdown).await {
            break;
        }
    }
    current.lock().unwrap().cancellation.cancel();
    status.send_replace(Status::Disabled);
}

async fn wait_for_change(
    desired: &mut watch::Receiver<Desired>,
    shutdown: &CancellationToken,
) -> bool {
    tokio::select! {
        biased;
        _ = shutdown.cancelled() => false,
        change = desired.changed() => change.is_ok(),
    }
}

static APP_SERVICE: OnceLock<Mutex<Option<Service>>> = OnceLock::new();
static APP_FOREGROUND: AtomicBool = AtomicBool::new(true);

pub fn set_app_foreground(foreground: bool) {
    APP_FOREGROUND.store(foreground, Ordering::SeqCst);
    if let Some(service) = app_service() {
        service.set_foreground(foreground);
    }
}

pub fn app_service() -> Option<Service> {
    APP_SERVICE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap()
        .clone()
}

pub struct Registration(Service);

pub fn register_current() -> Registration {
    let service = Service::attach(&Handle::current());
    if let Some(previous) = APP_SERVICE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap()
        .replace(service.clone())
    {
        previous.0.shutdown.cancel();
        previous.0.current.lock().unwrap().cancellation.cancel();
    }
    service.set_foreground(APP_FOREGROUND.load(Ordering::SeqCst));
    enable_validation_fixture(&service);
    Registration(service)
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.0 .0.shutdown.cancel();
        self.0 .0.current.lock().unwrap().cancellation.cancel();
        let mut current = APP_SERVICE.get_or_init(|| Mutex::new(None)).lock().unwrap();
        if current
            .as_ref()
            .is_some_and(|service| Arc::ptr_eq(&service.0, &self.0 .0))
        {
            current.take();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::{Implementation, CURRENT_VERSION};
    use serde_json::json;
    use std::time::Duration;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
        time::timeout,
    };

    async fn wait(
        status: &mut watch::Receiver<Status>,
        accept: impl Fn(&Status) -> bool,
    ) -> Status {
        timeout(Duration::from_secs(5), async {
            loop {
                let value = status.borrow_and_update().clone();
                if accept(&value) {
                    return value;
                }
                status.changed().await.unwrap();
            }
        })
        .await
        .unwrap()
    }

    fn config() -> ServerConfig {
        ServerConfig::local(Implementation {
            name: "runtime-test".into(),
            version: "1".into(),
        })
    }

    async fn tools(address: SocketAddr, token: &str) {
        let body = json!({"jsonrpc":"2.0", "id":1, "method":"tools/list", "params":{
            "_meta":{"io.modelcontextprotocol/protocolVersion":CURRENT_VERSION,
                "io.modelcontextprotocol/clientCapabilities":{}}
        }})
        .to_string();
        let mut stream = TcpStream::connect(address).await.unwrap();
        let request = format!("POST /mcp HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: {CURRENT_VERSION}\r\nMcp-Method: tools/list\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        timeout(Duration::from_secs(5), stream.read_to_string(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let body: serde_json::Value =
            serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["result"]["tools"], json!([]));
    }

    #[tokio::test]
    async fn explicit_enable_suspend_resume_disable_and_drop() {
        let service = Service::attach(&Handle::current());
        let mut status = service.subscribe();
        assert_eq!(*status.borrow(), Status::Disabled);
        let credentials = Credentials::new(1);
        let (_, token) = credentials.issue().unwrap();
        service.enable(config(), credentials, vec![]);
        let Status::Running(first) = wait(&mut status, |s| matches!(s, Status::Running(_))).await
        else {
            unreachable!()
        };
        tools(first, &token).await;
        let mut idle_connection = TcpStream::connect(first).await.unwrap();
        service.set_foreground(false);
        wait(&mut status, |s| *s == Status::Suspended).await;
        assert!(TcpStream::connect(first).await.is_err());
        let mut byte = [0];
        match timeout(Duration::from_secs(5), idle_connection.read(&mut byte))
            .await
            .unwrap()
        {
            Ok(0) => {}
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
            result => panic!("Suspended listener retained a connection: {result:?}"),
        }
        service.set_foreground(true);
        let Status::Running(second) = wait(&mut status, |s| matches!(s, Status::Running(_))).await
        else {
            unreachable!()
        };
        tools(second, &token).await;
        service.disable();
        wait(&mut status, |s| *s == Status::Disabled).await;
        assert!(TcpStream::connect(second).await.is_err());
        service.set_foreground(false);
        wait(&mut status, |s| *s == Status::Suspended).await;
        service.set_foreground(true);
        wait(&mut status, |s| *s == Status::Disabled).await;
        drop(service);
    }

    #[tokio::test]
    async fn dropping_enabled_service_closes_listener() {
        let service = Service::attach(&Handle::current());
        let mut status = service.subscribe();
        service.enable(config(), Credentials::new(1), vec![]);
        let Status::Running(address) = wait(&mut status, |s| matches!(s, Status::Running(_))).await
        else {
            unreachable!()
        };
        drop(service);
        wait(&mut status, |s| *s == Status::Disabled).await;
        assert!(TcpStream::connect(address).await.is_err());
    }

    #[tokio::test]
    async fn failed_bind_is_reported_and_can_be_retried() {
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let service = Service::attach(&Handle::current());
        let mut status = service.subscribe();
        let mut configuration = config();
        configuration.address = occupied.local_addr().unwrap();
        service.enable(configuration.clone(), Credentials::new(1), vec![]);
        wait(&mut status, |s| matches!(s, Status::Failed(_))).await;
        drop(occupied);
        service.enable(configuration, Credentials::new(1), vec![]);
        wait(&mut status, |s| matches!(s, Status::Running(_))).await;
        service.disable();
        wait(&mut status, |s| *s == Status::Disabled).await;
    }
    #[tokio::test]
    async fn suspension_waits_for_active_handler_cleanup() {
        use crate::mcp::{CallContext, ToolDefinition, ToolFuture, ToolHandler};
        use serde_json::{Map, Value};
        use std::sync::atomic::{AtomicBool, Ordering};
        use tokio::sync::Notify;

        struct Pending {
            started: Notify,
            dropped: AtomicBool,
        }
        struct Guard(Arc<Pending>);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.dropped.store(true, Ordering::SeqCst);
            }
        }
        impl ToolHandler for Arc<Pending> {
            fn call(&self, _: CallContext, _: Map<String, Value>) -> ToolFuture {
                let pending = self.clone();
                Box::pin(async move {
                    let _guard = Guard(pending.clone());
                    pending.started.notify_one();
                    std::future::pending().await
                })
            }
        }
        let pending = Arc::new(Pending {
            started: Notify::new(),
            dropped: AtomicBool::new(false),
        });
        let tool = RegisteredTool {
            definition: ToolDefinition {
                name: "pending".into(),
                description: "Lifecycle test only".into(),
                input_schema: json!({"type":"object"}),
                output_schema: None,
                annotations: None,
            },
            handler: Arc::new(pending.clone()),
        };
        let service = Service::attach(&Handle::current());
        let mut status = service.subscribe();
        let credentials = Credentials::new(1);
        let (_, token) = credentials.issue().unwrap();
        service.enable(config(), credentials, vec![tool]);
        let Status::Running(address) = wait(&mut status, |s| matches!(s, Status::Running(_))).await
        else {
            unreachable!()
        };
        let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
            "name":"pending","arguments":{},"_meta":{"io.modelcontextprotocol/protocolVersion":CURRENT_VERSION,
                "io.modelcontextprotocol/clientCapabilities":{}}
        }}).to_string();
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(format!("POST /mcp HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: {CURRENT_VERSION}\r\nMcp-Method: tools/call\r\nMcp-Name: pending\r\nContent-Length: {}\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        timeout(Duration::from_secs(5), pending.started.notified())
            .await
            .unwrap();
        service.set_foreground(false);
        wait(&mut status, |s| *s == Status::Suspended).await;
        assert!(pending.dropped.load(Ordering::SeqCst));
        assert!(TcpStream::connect(address).await.is_err());
    }
}

// Isolated validation snapshot only. Never copied into the product branch.
static VALIDATION_STARTED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static VALIDATION_DROPPED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
struct ValidationGuard;
impl Drop for ValidationGuard {
    fn drop(&mut self) {
        VALIDATION_DROPPED.fetch_add(1, Ordering::SeqCst);
    }
}
struct ValidationHandler;
impl super::ToolHandler for ValidationHandler {
    fn call(&self, _: super::CallContext, args: serde_json::Map<String, serde_json::Value>) -> super::ToolFuture {
        Box::pin(async move {
            if args.get("action").and_then(|v| v.as_str()) == Some("pending") {
                let _guard = ValidationGuard;
                VALIDATION_STARTED.fetch_add(1, Ordering::SeqCst);
                return std::future::pending().await;
            }
            Ok(super::ToolResult::text(serde_json::json!({
                "fixture": true,
                "started": VALIDATION_STARTED.load(Ordering::SeqCst),
                "dropped": VALIDATION_DROPPED.load(Ordering::SeqCst),
                "args": args
            }).to_string()))
        })
    }
}
fn enable_validation_fixture(service: &Service) {
    if option_env!("MCP_MOBILE_VALIDATION") != Some("enabled") { return; }
    let mut config = ServerConfig::local(super::Implementation {
        name: "rustdesk-mobile-lifecycle-fixture".into(), version: "0-test-only".into()
    });
    config.address.set_port(37173);
    let credentials = Credentials::new(1);
    credentials.insert("mobile-validation".into(), "mobile-validation-fixture-not-a-production-credential-20260918").unwrap();
    service.enable(config, credentials, vec![RegisteredTool {
        definition: super::ToolDefinition {
            name: "mobile_validation_fixture".into(),
            description: "Protocol/lifecycle fixture only; no remote control capability".into(),
            input_schema: serde_json::json!({"type":"object","properties":{"action":{"type":"string"}}}),
            output_schema: None, annotations: None,
        },
        handler: Arc::new(ValidationHandler),
    }]);
}
