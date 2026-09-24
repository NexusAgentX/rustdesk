mod http;
mod model;
mod protocol;
pub mod runtime;
mod security;

pub use self::{
    model::{
        CallContext, Content, Implementation, ProtocolVersion, RpcError, ToolAnnotations,
        ToolDefinition, ToolFuture, ToolHandler, ToolResult, CURRENT_VERSION, LEGACY_VERSION,
    },
    security::Credentials,
};
use std::{
    collections::{BTreeMap, HashMap},
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    net::TcpListener,
    sync::{Notify, Semaphore},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct Limits {
    pub connections: usize,
    pub requests: usize,
    pub tool_calls: usize,
    pub sessions: usize,
    pub body_bytes: usize,
    pub response_bytes: usize,
    pub header_bytes: usize,
    pub header_count: usize,
    pub body_timeout: Duration,
    pub request_timeout: Duration,
    pub session_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            connections: 64,
            requests: 32,
            tool_calls: 16,
            sessions: 64,
            body_bytes: 1024 * 1024,
            response_bytes: 16 * 1024 * 1024,
            header_bytes: 16 * 1024,
            header_count: 32,
            body_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(120),
            session_timeout: Duration::from_secs(3600),
        }
    }
}

#[derive(Clone)]
pub struct ServerConfig {
    pub address: SocketAddr,
    pub info: Implementation,
    pub allowed_origins: Vec<String>,
    pub limits: Limits,
}

impl ServerConfig {
    pub fn local(info: Implementation) -> Self {
        Self {
            address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            info,
            allowed_origins: Vec::new(),
            limits: Limits::default(),
        }
    }
}

#[derive(Clone)]
pub struct RegisteredTool {
    pub definition: ToolDefinition,
    pub handler: Arc<dyn ToolHandler>,
}

struct Session {
    principal: security::Principal,
    ready: bool,
    touched: Instant,
    cancellation: CancellationToken,
}

#[derive(Hash, Eq, PartialEq, Clone)]
struct RequestKey {
    principal: String,
    credential: String,
    session: Option<String>,
    id: String,
}

struct State {
    config: ServerConfig,
    credentials: Arc<Credentials>,
    tools: BTreeMap<String, RegisteredTool>,
    sessions: Mutex<HashMap<String, Session>>,
    active: Mutex<HashMap<RequestKey, CancellationToken>>,
    requests: Arc<Semaphore>,
    calls: Arc<Semaphore>,
    stopped: CancellationToken,
    drained: Notify,
}

pub struct Server {
    listener: TcpListener,
    state: Arc<State>,
}

impl Server {
    /// Binding does not enable remote control or create a runtime. The caller owns both.
    pub async fn bind(
        config: ServerConfig,
        credentials: Arc<Credentials>,
        tools: Vec<RegisteredTool>,
    ) -> io::Result<Self> {
        let limits = &config.limits;
        if !config.address.ip().is_loopback()
            || config.info.name.is_empty()
            || config.info.version.is_empty()
            || limits.connections == 0
            || limits.requests == 0
            || limits.tool_calls == 0
            || limits.sessions == 0
            || limits.body_bytes == 0
            || limits.response_bytes < 1024
            || limits.header_bytes < 8192
            || limits.header_count == 0
            || limits.body_timeout.is_zero()
            || limits.request_timeout.is_zero()
            || limits.session_timeout.is_zero()
            || [limits.connections, limits.requests, limits.tool_calls]
                .iter()
                .any(|limit| *limit > Semaphore::MAX_PERMITS)
            || config
                .allowed_origins
                .iter()
                .any(|origin| !valid_origin(origin))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid MCP server configuration",
            ));
        }
        let mut registry = BTreeMap::new();
        for tool in tools {
            let definition = &tool.definition;
            if definition.name.is_empty()
                || definition.name.len() > 128
                || !definition
                    .name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
                || definition
                    .input_schema
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    != Some("object")
                || unsupported_header_annotation(&definition.input_schema)
                || definition.output_schema.as_ref().is_some_and(|schema| {
                    schema.get("type").and_then(serde_json::Value::as_str) != Some("object")
                })
                || registry.contains_key(&definition.name)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Invalid or duplicate MCP tool definition",
                ));
            }
            registry.insert(definition.name.clone(), tool);
        }
        let listener = TcpListener::bind(config.address).await?;
        Ok(Self {
            listener,
            state: Arc::new(State {
                requests: Arc::new(Semaphore::new(limits.requests)),
                calls: Arc::new(Semaphore::new(limits.tool_calls)),
                config,
                credentials,
                tools: registry,
                sessions: Mutex::new(HashMap::new()),
                active: Mutex::new(HashMap::new()),
                stopped: CancellationToken::new(),
                drained: Notify::new(),
            }),
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    pub async fn run(self, shutdown: CancellationToken) -> io::Result<()> {
        http::serve(self, shutdown).await
    }
}

impl State {
    fn expire_sessions(&self) {
        self.sessions.lock().unwrap().retain(|_, session| {
            let keep = !session.principal.revoked.is_cancelled()
                && session.touched.elapsed() < self.config.limits.session_timeout;
            if !keep {
                session.cancellation.cancel();
            }
            keep
        });
    }
}

#[cfg(test)]
mod tests;

fn valid_origin(origin: &str) -> bool {
    match origin.parse::<hyper::Uri>() {
        Ok(uri) => {
            matches!(uri.scheme_str(), Some("http" | "https"))
                && uri
                    .authority()
                    .is_some_and(|authority| !authority.as_str().contains(['@', '*']))
                && uri.query().is_none()
                && uri.path() == "/"
                && !origin.ends_with('/')
        }
        Err(_) => false,
    }
}

// Routing annotations need additional HTTP validation; do not advertise them until implemented.
fn unsupported_header_annotation(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            map.contains_key("x-mcp-header") || map.values().any(unsupported_header_annotation)
        }
        serde_json::Value::Array(values) => values.iter().any(unsupported_header_annotation),
        _ => false,
    }
}
