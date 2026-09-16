//! Loopback TCP listeners using the stock RustDesk port-forward handshake.
//! Each listener owns isolated login state and cancels all accepted connections on close.
use super::{
    control::Permit,
    error::{BridgeError, Result},
    sessions,
};
use crate::{
    client::{self, Data, Interface, LoginConfigHandler},
    ui_session_interface::{InvokeUiSession, Session},
};
use hbb_common::{
    futures::{SinkExt, StreamExt},
    message_proto::{self, *},
    protobuf::Message as _,
    rendezvous_proto::ConnType,
    tokio::{
        self,
        net::TcpListener,
        sync::{mpsc, oneshot, watch},
        task::JoinSet,
    },
    tokio_util::codec::{BytesCodec, Framed},
    Stream,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, RwLock,
    },
    time::Duration,
};

pub type Shared = Arc<Mutex<VecDeque<Arc<Mutex<Row>>>>>;
#[derive(Serialize)]
pub struct Row {
    id: String,
    local_address: String,
    local_port: u16,
    remote_host: String,
    remote_port: u16,
    state: &'static str,
    active_connections: usize,
    successful_connections: u64,
    last_error: Option<String>,
    auth_challenge: Option<sessions::AuthChallenge>,
    connection_epoch: u64,
}
#[derive(Clone)]
pub enum Credentials {
    Password(String),
    TwoFactor(String),
}
#[derive(Clone)]
pub enum Command {
    CloseAll,
    Add {
        local_port: u16,
        remote_host: String,
        remote_port: u16,
        password: Option<String>,
    },
    Remove {
        id: String,
    },
    Authenticate {
        id: String,
        challenge: String,
        credentials: Credentials,
    },
}
#[derive(Clone)]
pub struct Envelope {
    permit: Permit,
    command: Command,
    active: Arc<AtomicBool>,
    reply: Arc<Mutex<Option<oneshot::Sender<Result<Value>>>>>,
}
impl Envelope {
    fn check(&self) -> Result<()> {
        self.permit.check()?;
        if !self.active.load(Ordering::Acquire) {
            return Err(BridgeError::new(
                "CANCELLED",
                "Tunnel request was cancelled before applying",
            ));
        }
        Ok(())
    }
    fn complete(&self, result: Result<Value>) {
        if let Some(reply) = self.reply.lock().unwrap().take() {
            let _receiver_closed = reply.send(result);
        }
    }
}
fn session(permit: &Permit, write: bool) -> Result<sessions::SessionHandle> {
    if write {
        permit.check()?;
    } else {
        permit.read_check()?;
    }
    let session = sessions::get(&permit.authority.session_id)
        .ok_or_else(|| BridgeError::new("SESSION_CLOSED", "Tunnel session closed"))?;
    if session.snapshot().kind != sessions::SessionKind::TcpTunnel {
        return Err(BridgeError::new(
            "WRONG_SESSION_KIND",
            "Open kind=tcp_tunnel first",
        ));
    }
    if write && session.snapshot().state != sessions::ConnectionState::Ready {
        return Err(BridgeError::new(
            "NOT_READY",
            "Local tunnel manager is not running",
        ));
    }
    Ok(session)
}
pub fn gui_state(session: &sessions::SessionHandle) -> Value {
    if session.snapshot().kind != sessions::SessionKind::TcpTunnel {
        return Value::Null;
    }
    json!(session
        .tunnels()
        .lock()
        .unwrap()
        .iter()
        .map(|row| json!(*row.lock().unwrap()))
        .collect::<Vec<_>>())
}
pub fn list(permit: &Permit) -> Result<Value> {
    let session = session(permit, false)?;
    let s = session.snapshot();
    let rows = session
        .tunnels()
        .lock()
        .unwrap()
        .iter()
        .map(|r| json!(*r.lock().unwrap()))
        .collect::<Vec<_>>();
    let configured = sessions::core(&s.session_id)
        .map(|c| c.lc.read().unwrap().port_forwards.clone())
        .unwrap_or_default();
    Ok(
        json!({"tunnels":rows,"scope":"session","manager_running":s.state==sessions::ConnectionState::Ready,
        "configured_gui_forwards":configured,"gui_forward_observation":"Saved GUI forwards are separate; their runtime state is not inferred from configuration.",
        "semantics":"Listening is local only. Authentication and remote target connection occur when a local client connects. Returning AI control, detaching or disconnecting closes MCP listeners and all their streams. No automatic restart.",
        "credentials":"Per-listener login state stays in memory until close; never saved by MCP. Passwords/challenges are not broadcast to other listeners."}),
    )
}
fn validate(command: &Command) -> Result<()> {
    match command {
        Command::Add {
            local_port,
            remote_host,
            remote_port,
            password,
        } => {
            if *local_port == 0 || *remote_port == 0 {
                return Err(BridgeError::invalid(
                    "Ports must be 1..65535; automatic/RDP port zero is not supported",
                ));
            }
            if remote_host.is_empty()
                || remote_host.len() > 253
                || remote_host
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control())
            {
                return Err(BridgeError::invalid(
                    "remote_host must be an IP address or hostname without whitespace",
                ));
            }
            if password
                .as_ref()
                .is_some_and(|p| p.is_empty() || p.len() > 16384)
            {
                return Err(BridgeError::invalid("Password must be 1..16384 bytes"));
            }
        }
        Command::Authenticate { credentials, .. } => {
            let valid = match credentials {
                Credentials::Password(p) => !p.is_empty() && p.len() <= 16384,
                Credentials::TwoFactor(p) => !p.is_empty() && p.len() <= 256,
            };
            if !valid {
                return Err(BridgeError::invalid(
                    "Authentication value exceeds its limit",
                ));
            }
        }
        _ => {}
    }
    Ok(())
}
pub async fn command(permit: Permit, command: Command, wait_ms: u64) -> Result<Value> {
    super::api::wait_budget(wait_ms)?;
    validate(&command)?;
    session(&permit, true)?;
    let core = sessions::core(&permit.authority.session_id)
        .ok_or_else(|| BridgeError::new("GUI_UNAVAILABLE", "Tunnel GUI unavailable"))?;
    let (tx, rx) = oneshot::channel();
    let active = Arc::new(AtomicBool::new(true));
    struct Guard(Arc<AtomicBool>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    let _guard = Guard(active.clone());
    let sender = core
        .sender
        .read()
        .unwrap()
        .clone()
        .ok_or_else(|| BridgeError::new("NOT_READY", "Tunnel manager unavailable"))?;
    sender
        .send(Data::AutomationTunnel(Envelope {
            permit,
            command,
            active,
            reply: Arc::new(Mutex::new(Some(tx))),
        }))
        .map_err(|_| BridgeError::new("NOT_READY", "Tunnel manager closed"))?;
    tokio::time::timeout(Duration::from_millis(wait_ms.max(1000)), rx)
        .await
        .map_err(|_| {
            BridgeError::new(
                "DELIVERY_UNKNOWN",
                "Tunnel operation acknowledgement timed out; inspect tunnel list before retrying",
            )
        })?
        .map_err(|_| {
            BridgeError::new(
                "DELIVERY_UNKNOWN",
                "Tunnel manager ended before acknowledgement",
            )
        })?
}
struct Active {
    row: Arc<Mutex<Row>>,
    permit: Permit,
    stop: watch::Sender<bool>,
    auth: mpsc::UnboundedSender<Envelope>,
    task: tokio::task::JoinHandle<()>,
}
pub struct Manager {
    store: Shared,
    active: Vec<Active>,
    session: sessions::SessionHandle,
    closing: bool,
}
impl Manager {
    pub fn new(session: sessions::SessionHandle) -> Self {
        Self {
            store: session.tunnels(),
            active: Vec::new(),
            session,
            closing: false,
        }
    }
    pub async fn handle<T: InvokeUiSession>(
        &mut self,
        core: &Session<T>,
        key: &str,
        token: &str,
        request: Envelope,
    ) {
        let result = self.apply(core, key, token, &request).await;
        if !matches!(request.command, Command::Authenticate { .. }) || result.is_err() {
            request.complete(result);
        }
    }
    async fn apply<T: InvokeUiSession>(
        &mut self,
        core: &Session<T>,
        key: &str,
        token: &str,
        request: &Envelope,
    ) -> Result<Value> {
        request.check()?;
        if self.closing {
            return Err(BridgeError::new("NOT_READY", "Tunnel manager is closing"));
        }
        validate(&request.command)?;
        match &request.command {
            Command::CloseAll => {
                self.close().await;
                if self
                    .store
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|r| r.lock().unwrap().state != "closed")
                {
                    return Err(BridgeError::new(
                        "CLEANUP_FAILED",
                        "Some tunnel tasks did not finish cleanly",
                    ));
                }
                Ok(json!({"confirmed":true}))
            }
            Command::Add {
                local_port,
                remote_host,
                remote_port,
                password,
            } => {
                if self.active.len() >= 16 {
                    return Err(BridgeError::new(
                        "LIMIT_EXCEEDED",
                        "At most 16 MCP listeners per session",
                    ));
                }
                // TcpListener uses exclusive binding; do not use stock SO_REUSEPORT here.
                let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, *local_port))
                    .await
                    .map_err(|e| {
                        BridgeError::new(
                            if e.kind() == std::io::ErrorKind::AddrInUse {
                                "PORT_IN_USE"
                            } else {
                                "LISTEN_FAILED"
                            },
                            &e.to_string(),
                        )
                    })?;
                request.check()?;
                let row = Arc::new(Mutex::new(Row {
                    id: format!("tunnel_{}", uuid::Uuid::new_v4()),
                    local_address: "127.0.0.1".into(),
                    local_port: *local_port,
                    remote_host: remote_host.clone(),
                    remote_port: *remote_port,
                    state: "listening",
                    active_connections: 0,
                    successful_connections: 0,
                    last_error: None,
                    auth_challenge: None,
                    connection_epoch: self.session.snapshot().connection_epoch,
                }));
                let result = json!({"tunnel":*row.lock().unwrap(),"confirmed":true,"confirmation":"local_listener_bound","remote_connected":false});
                let mut child = core.clone();
                let mut lc = LoginConfigHandler::default();
                let (force_relay, conn_token) = {
                    let parent = core.lc.read().unwrap();
                    (parent.force_relay, parent.get_conn_token())
                };
                lc.initialize(
                    core.get_id(),
                    ConnType::PORT_FORWARD,
                    None,
                    force_relay,
                    None,
                    None,
                    conn_token,
                );
                lc.port_forward = (remote_host.clone(), i32::from(*remote_port));
                lc.automation_authentication(Some(request.permit.clone()), password.is_some());
                child.lc = Arc::new(RwLock::new(lc));
                child.password = String::new();
                let (stop, stopped) = watch::channel(false);
                let (auth, rx) = mpsc::unbounded_channel();
                let task = tokio::spawn(listen(
                    child,
                    listener,
                    row.clone(),
                    request.permit.clone(),
                    password.clone(),
                    key.to_owned(),
                    token.to_owned(),
                    stopped,
                    rx,
                ));
                self.store.lock().unwrap().push_back(row.clone());
                {
                    let mut rows = self.store.lock().unwrap();
                    while rows.len() > 64 {
                        // A live listener must remain discoverable even after many short-lived ones.
                        if let Some(index) = rows
                            .iter()
                            .position(|r| matches!(r.lock().unwrap().state, "closed" | "failed"))
                        {
                            rows.remove(index);
                        } else {
                            break;
                        }
                    }
                }
                self.active.push(Active {
                    row,
                    permit: request.permit.clone(),
                    stop,
                    auth,
                    task,
                });
                Ok(result)
            }
            Command::Remove { id } => {
                if let Some(index) = self
                    .active
                    .iter()
                    .position(|a| a.row.lock().unwrap().id == *id)
                {
                    let active = self.active.remove(index);
                    stop(active).await?;
                }
                let rows = self.store.lock().unwrap();
                let row = rows
                    .iter()
                    .find(|r| r.lock().unwrap().id == *id)
                    .ok_or_else(|| {
                        BridgeError::new(
                            "TUNNEL_NOT_FOUND",
                            "Tunnel is not retained in this session",
                        )
                    })?;
                Ok(json!({"tunnel":*row.lock().unwrap(),"confirmed":true}))
            }
            Command::Authenticate { id, .. } => {
                let active = self
                    .active
                    .iter()
                    .find(|a| a.row.lock().unwrap().id == *id)
                    .ok_or_else(|| {
                        BridgeError::new("TUNNEL_NOT_FOUND", "No active listener with this ID")
                    })?;
                check_auth(request, &active.row)?;
                active.auth.send(request.clone()).map_err(|_| {
                    BridgeError::new("NOT_READY", "Listener authentication queue closed")
                })?;
                // The handshake owns the acknowledgement. Returning early must not consume it.
                Ok(json!({"queued":true}))
            }
        }
    }
    pub async fn poll(&mut self) {
        let mut error = None;
        let mut i = 0;
        while i < self.active.len() {
            if self.active[i].permit.check().is_err() || self.active[i].task.is_finished() {
                let active = self.active.remove(i);
                if let Err(e) = stop(active).await {
                    hbb_common::log::error!("Tunnel cleanup: {}", e.message);
                    error = Some(e.message);
                }
            } else {
                i += 1;
            }
        }
        if let Some((generation, _)) = self.session.control().pending() {
            // All MCP listeners with old permits have now completed cleanup.
            self.session
                .control()
                .complete_transition(generation, error);
        }
    }
    pub async fn close(&mut self) {
        self.closing = true;
        for active in self.active.drain(..) {
            if let Err(e) = stop(active).await {
                hbb_common::log::error!("Tunnel close: {}", e.message);
            }
        }
    }
}
async fn stop(active: Active) -> Result<()> {
    active.stop.send_replace(true);
    match active.task.await {
        Ok(()) => Ok(()),
        Err(error) => {
            let mut row = active.row.lock().unwrap();
            row.state = "failed";
            row.last_error = Some(error.to_string());
            Err(BridgeError::new("CLEANUP_FAILED", &error.to_string()))
        }
    }
}
fn check_auth(request: &Envelope, row: &Arc<Mutex<Row>>) -> Result<()> {
    request.check()?;
    let Command::Authenticate {
        challenge,
        credentials,
        ..
    } = &request.command
    else {
        return Err(BridgeError::invalid("Expected authentication"));
    };
    let kind = match credentials {
        Credentials::Password(_) => "password",
        Credentials::TwoFactor(_) => "two_factor",
    };
    if !row
        .lock()
        .unwrap()
        .auth_challenge
        .as_ref()
        .is_some_and(|c| c.id == *challenge && c.kind == kind)
    {
        return Err(BridgeError::new(
            "AUTH_CHALLENGE_CHANGED",
            "Read this tunnel's current challenge before submitting",
        ));
    }
    Ok(())
}
struct StreamGuard(Arc<Mutex<Row>>);
impl Drop for StreamGuard {
    fn drop(&mut self) {
        let mut row = self.0.lock().unwrap();
        row.active_connections = row.active_connections.saturating_sub(1);
    }
}
async fn listen<T: InvokeUiSession>(
    core: Session<T>,
    listener: TcpListener,
    row: Arc<Mutex<Row>>,
    permit: Permit,
    mut password: Option<String>,
    key: String,
    token: String,
    mut stopped: watch::Receiver<bool>,
    mut auth: mpsc::UnboundedReceiver<Envelope>,
) {
    let mut streams = JoinSet::new();
    loop {
        tokio::select! {
            _=stopped.changed()=>break,
            Some(result)=streams.join_next(), if !streams.is_empty()=> {
                let mut r=row.lock().unwrap();
                match result {Ok(Ok(()))=>{},Ok(Err(e))=>r.last_error=Some(e),Err(e)=>r.last_error=Some(e.to_string())}
            }
            request=auth.recv()=>{if let Some(r)=request {r.complete(Err(BridgeError::new("AUTH_CHALLENGE_CHANGED","No authentication is waiting")));}}
            accepted=listener.accept()=>{
                if permit.check().is_err() {break;}
                let (socket,_)=match accepted {Ok(v)=>v,Err(e)=>{row.lock().unwrap().last_error=Some(e.to_string());break;}};
                if streams.len()>=32 {row.lock().unwrap().last_error=Some("At most 32 active streams per listener".into());continue;}
                row.lock().unwrap().state="connecting";
                let result=tokio::select! {
                    _=stopped.changed()=>break,
                    result=connect(&core,&row,&permit,password.take(),&key,&token,&mut auth)=>result,
                };
                match result {
                    Ok(stream)=>{
                        let mut r=row.lock().unwrap();r.state="listening";r.active_connections+=1;r.successful_connections+=1;r.auth_challenge=None;r.last_error=None;drop(r);
                        let guard=StreamGuard(row.clone());
                        streams.spawn(async move {let _guard=guard;forward(Framed::new(socket,BytesCodec::new()),stream).await});
                    }
                    Err(error)=>{let mut r=row.lock().unwrap();r.state="listening";r.last_error=Some(error.chars().take(1024).collect());r.auth_challenge=None;}
                }
            }
        }
    }
    drop(listener);
    row.lock().unwrap().state = "closing";
    streams.abort_all();
    while streams.join_next().await.is_some() {}
    core.lc.write().unwrap().automation_forget_credentials();
    let mut r = row.lock().unwrap();
    r.state = "closed";
    r.active_connections = 0;
    r.auth_challenge = None;
}
async fn connect<T: InvokeUiSession>(
    core: &Session<T>,
    row: &Arc<Mutex<Row>>,
    permit: &Permit,
    password: Option<String>,
    key: &str,
    token: &str,
    auth: &mut mpsc::UnboundedReceiver<Envelope>,
) -> std::result::Result<Stream, String> {
    permit.check().map_err(|e| e.message)?;
    let ((mut stream, _, _, _, _), (feedback, server)) = client::Client::start(
        &core.get_id(),
        key,
        token,
        ConnType::PORT_FORWARD,
        core.clone(),
    )
    .await
    .map_err(|e| e.to_string())?;
    if !stream.is_secured() && !crate::common::is_direct_ip_access(&core.get_id()) {
        return Err("INSECURE_CONNECTION: encrypted peer connection required".into());
    }
    let _keep_alive = client::hc_connection(feedback, server, token).await;
    let mut password = password;
    loop {
        tokio::select! {
            request=auth.recv()=>{
                let Some(request)=request else {return Err("Authentication queue closed".into());};
                let result=match check_auth(&request,row) {
                    Err(error)=>Err(error),
                    Ok(())=>{
                        core.lc.write().unwrap().automation_authentication(Some(permit.clone()),false);
                        match &request.command {
                            Command::Authenticate {credentials:Credentials::Password(password),..}=>{
                                core.handle_login_from_ui(String::new(),String::new(),password.clone(),false,&mut stream).await;
                                core.lc.write().unwrap().automation_auth_result.take().unwrap_or_else(||Err(BridgeError::new("DELIVERY_UNKNOWN","Login send result unavailable")))
                            }
                            Command::Authenticate {credentials:Credentials::TwoFactor(code),..}=>{
                                let mut message=Message::new();message.set_auth_2fa(Auth2FA {code:code.clone(),..Default::default()});
                                stream.send(&message).await.map_err(|e|BridgeError::new("DELIVERY_UNKNOWN",&e.to_string()))
                            }
                            _=>Err(BridgeError::invalid("Unsupported authentication")),
                        }
                    }
                };
                if result.is_ok() {row.lock().unwrap().auth_challenge=None;row.lock().unwrap().state="authenticating";}
                request.complete(result.map(|_|json!({"delivery":"sent","confirmed":false})));
            }
            incoming=tokio::time::timeout(Duration::from_secs(60),stream.next())=>{
                permit.check().map_err(|e|e.message)?;
                let bytes=incoming.map_err(|_|"Remote login timed out".to_owned())?.ok_or_else(||"Peer disconnected during login".to_owned())?.map_err(|e|e.to_string())?;
                let message=Message::parse_from_bytes(&bytes).map_err(|e|e.to_string())?;
                match message.union {
                    Some(message_proto::message::Union::Hash(hash))=>{
                        core.handle_hash(password.take().as_deref().unwrap_or(""),hash,&mut stream).await;
                        if core.lc.read().unwrap().get_conn_token().is_none() {
                            let mut r=row.lock().unwrap();r.state="awaiting_auth";r.auth_challenge=Some(sessions::AuthChallenge{id:format!("tunnel_auth_{}",uuid::Uuid::new_v4()),kind:"password".into(),fields:vec!["password".into()]});
                        }
                    }
                    Some(message_proto::message::Union::LoginResponse(response))=>match response.union {
                        Some(login_response::Union::PeerInfo(_))=>{stream.set_raw();return Ok(stream);}
                        Some(login_response::Union::Error(error))=>{
                            let kind=match error.as_str() {client::LOGIN_MSG_PASSWORD_EMPTY|client::LOGIN_MSG_PASSWORD_WRONG=>Some("password"),client::REQUIRE_2FA|client::LOGIN_MSG_2FA_WRONG=>Some("two_factor"),_=>None};
                            let Some(kind)=kind else {return Err(error);};
                            let mut r=row.lock().unwrap();r.state="awaiting_auth";r.last_error=Some(error);r.auth_challenge=Some(sessions::AuthChallenge{id:format!("tunnel_auth_{}",uuid::Uuid::new_v4()),kind:kind.into(),fields:vec![if kind=="password" {"password"}else{"code"}.into()]});
                        }
                        _=>{}
                    }
                    Some(message_proto::message::Union::TestDelay(delay))=>core.handle_test_delay(delay,&mut stream).await,
                    _=>{}
                }
            }
        }
    }
}
async fn forward(
    mut local: Framed<tokio::net::TcpStream, BytesCodec>,
    mut remote: Stream,
) -> std::result::Result<(), String> {
    loop {
        tokio::select! {
            bytes=local.next()=>match bytes {Some(Ok(bytes))=>remote.send_bytes(bytes.into()).await.map_err(|e|e.to_string())?,Some(Err(e))=>return Err(e.to_string()),None=>return Ok(())},
            bytes=remote.next()=>match bytes {Some(Ok(bytes))=>local.send(bytes).await.map_err(|e|e.to_string())?,Some(Err(e))=>return Err(e.to_string()),None=>return Ok(())},
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::control::Agent;
    use super::*;
    fn setup() -> (Manager, Session<crate::flutter::FlutterHandler>, Permit) {
        let session = sessions::SessionHandle::new(
            "mcp-tunnel-test".into(),
            sessions::SessionKind::TcpTunnel,
            Default::default(),
        );
        let agent = Agent::new();
        session.control().attach(&agent, true).unwrap();
        if let Some((generation, _)) = session.control().pending() {
            session.control().complete_transition(generation, None);
        }
        let reference = session.control().view().session_ref.unwrap();
        let permit = session.control().resolve(&agent, &reference, true).unwrap();
        let core = Session::default();
        core.lc.write().unwrap().initialize(
            "mcp-tunnel-test".into(),
            ConnType::PORT_FORWARD,
            None,
            false,
            None,
            None,
            None,
        );
        (Manager::new(session), core, permit)
    }
    fn envelope(permit: &Permit, command: Command) -> Envelope {
        Envelope {
            permit: permit.clone(),
            command,
            active: Arc::new(AtomicBool::new(true)),
            reply: Default::default(),
        }
    }
    fn free_port() -> u16 {
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        socket.local_addr().unwrap().port()
    }
    #[tokio::test]
    async fn exclusive_binding_remove_and_handover_release_ports() {
        let (mut manager, core, permit) = setup();
        let port = free_port();
        let command = Command::Add {
            local_port: port,
            remote_host: "127.0.0.1".into(),
            remote_port: 80,
            password: None,
        };
        let first = manager
            .apply(&core, "", "", &envelope(&permit, command.clone()))
            .await
            .unwrap();
        assert_eq!(first["confirmation"], "local_listener_bound");
        assert_eq!(first["remote_connected"], false);
        assert_eq!(
            manager
                .apply(&core, "", "", &envelope(&permit, command.clone()))
                .await
                .unwrap_err()
                .code,
            "PORT_IN_USE"
        );
        let id = first["tunnel"]["id"].as_str().unwrap().to_owned();
        let stopped = manager
            .apply(&core, "", "", &envelope(&permit, Command::Remove { id }))
            .await
            .unwrap();
        assert_eq!(stopped["tunnel"]["state"], "closed");
        assert_eq!(stopped["tunnel"]["active_connections"], 0);
        manager
            .apply(&core, "", "", &envelope(&permit, command))
            .await
            .unwrap();
        manager.session.control().release(None, false).unwrap();
        manager.poll().await;
        assert!(manager.active.is_empty());
        assert!(std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).is_ok());
    }
    #[tokio::test]
    async fn cancelled_add_never_binds_and_challenges_are_listener_scoped() {
        let (mut manager, core, permit) = setup();
        let port = free_port();
        let request = envelope(
            &permit,
            Command::Add {
                local_port: port,
                remote_host: "localhost".into(),
                remote_port: 80,
                password: None,
            },
        );
        request.active.store(false, Ordering::Release);
        assert_eq!(
            manager
                .apply(&core, "", "", &request)
                .await
                .unwrap_err()
                .code,
            "CANCELLED"
        );
        assert!(std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).is_ok());
        let row = Arc::new(Mutex::new(Row {
            id: "one".into(),
            local_address: "127.0.0.1".into(),
            local_port: port,
            remote_host: "localhost".into(),
            remote_port: 80,
            state: "awaiting_auth",
            active_connections: 0,
            successful_connections: 0,
            last_error: None,
            connection_epoch: 1,
            auth_challenge: Some(sessions::AuthChallenge {
                id: "first".into(),
                kind: "password".into(),
                fields: vec!["password".into()],
            }),
        }));
        let request = envelope(
            &permit,
            Command::Authenticate {
                id: "one".into(),
                challenge: "another-listener".into(),
                credentials: Credentials::Password("temporary".into()),
            },
        );
        assert_eq!(
            check_auth(&request, &row).unwrap_err().code,
            "AUTH_CHALLENGE_CHANGED"
        );
        let request = envelope(
            &permit,
            Command::Authenticate {
                id: "one".into(),
                challenge: "first".into(),
                credentials: Credentials::TwoFactor("123456".into()),
            },
        );
        assert_eq!(
            check_auth(&request, &row).unwrap_err().code,
            "AUTH_CHALLENGE_CHANGED"
        );
        let request = envelope(
            &permit,
            Command::Authenticate {
                id: "one".into(),
                challenge: "first".into(),
                credentials: Credentials::Password("temporary".into()),
            },
        );
        assert!(check_auth(&request, &row).is_ok());
    }
    #[tokio::test]
    async fn bounded_history_keeps_live_listeners_and_close_fences_new_work() {
        let (mut manager, core, permit) = setup();
        let command = Command::Add {
            local_port: free_port(),
            remote_host: "localhost".into(),
            remote_port: 80,
            password: None,
        };
        let first = manager
            .apply(&core, "", "", &envelope(&permit, command))
            .await
            .unwrap();
        let first_id = first["tunnel"]["id"].as_str().unwrap();
        let port = free_port();
        for _ in 0..70 {
            let add = Command::Add {
                local_port: port,
                remote_host: "localhost".into(),
                remote_port: 80,
                password: None,
            };
            let value = manager
                .apply(&core, "", "", &envelope(&permit, add))
                .await
                .unwrap();
            let id = value["tunnel"]["id"].as_str().unwrap().to_owned();
            manager
                .apply(&core, "", "", &envelope(&permit, Command::Remove { id }))
                .await
                .unwrap();
        }
        assert_eq!(manager.store.lock().unwrap().len(), 64);
        assert!(manager.store.lock().unwrap().iter().any(|r| {
            let r = r.lock().unwrap();
            r.id == first_id && r.state == "listening"
        }));
        manager
            .apply(&core, "", "", &envelope(&permit, Command::CloseAll))
            .await
            .unwrap();
        assert!(manager.active.is_empty());
        let add = Command::Add {
            local_port: port,
            remote_host: "localhost".into(),
            remote_port: 80,
            password: None,
        };
        assert_eq!(
            manager
                .apply(&core, "", "", &envelope(&permit, add))
                .await
                .unwrap_err()
                .code,
            "NOT_READY"
        );
    }
    #[test]
    fn targets_and_passwords_are_bounded() {
        for (local, host, remote) in [
            (0, "localhost", 80),
            (80, "localhost", 0),
            (80, "bad host", 80),
            (80, "", 80),
        ] {
            assert!(validate(&Command::Add {
                local_port: local,
                remote_host: host.into(),
                remote_port: remote,
                password: None
            })
            .is_err());
        }
    }
}
