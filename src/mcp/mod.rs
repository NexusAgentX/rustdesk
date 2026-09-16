mod credentials;
mod operations;
mod tools;
mod transport;
mod types;

use crate::automation::{
    control::{self, Agent},
    error::{BridgeError, Result},
    gui, sessions, terminals, wire,
};
use hbb_common::{
    config::LocalConfig,
    log,
    tokio::{
        self,
        sync::{mpsc, Semaphore},
    },
};
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Serialize)]
pub struct Settings {
    pub enabled: bool,
    pub port: u16,
    pub approval_required: bool,
}
impl Settings {
    fn load() -> Self {
        Self {
            enabled: LocalConfig::get_option("mcp-enabled") == "Y",
            port: LocalConfig::get_option("mcp-port").parse().unwrap_or(21122),
            approval_required: LocalConfig::get_option("mcp-approval-required") != "N",
        }
    }
    fn save(&self) {
        LocalConfig::set_option(
            "mcp-enabled".into(),
            if self.enabled { "Y" } else { "N" }.into(),
        );
        LocalConfig::set_option("mcp-port".into(), self.port.to_string());
        LocalConfig::set_option(
            "mcp-approval-required".into(),
            if self.approval_required { "Y" } else { "N" }.into(),
        );
    }
}
#[derive(Clone, Serialize)]
struct Status {
    settings: Settings,
    state: String,
    address: Option<String>,
    error: Option<String>,
}
pub(crate) struct Client {
    pub agent: Agent,
    pub initialized: AtomicBool,
    pub created: chrono::DateTime<chrono::Utc>,
    pub name: Mutex<String>,
    pub version: Mutex<String>,
    pub last_communication: Mutex<chrono::DateTime<chrono::Utc>>,
    pub session_id: Mutex<Option<String>>,
    pub cancel: CancellationToken,
    pub calls: Arc<Semaphore>,
    operations: operations::Operations,
    detached: Mutex<HashMap<String, (std::time::Instant, String)>>,
}
impl Client {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            agent: Agent::new(),
            initialized: AtomicBool::new(false),
            created: chrono::Utc::now(),
            name: Mutex::new(String::new()),
            version: Mutex::new(String::new()),
            last_communication: Mutex::new(chrono::Utc::now()),
            session_id: Mutex::new(None),
            cancel: CancellationToken::new(),
            calls: Arc::new(Semaphore::new(16)),
            operations: Default::default(),
            detached: Default::default(),
        })
    }
    pub fn touch(&self) {
        *self.last_communication.lock().unwrap() = chrono::Utc::now();
    }
    pub fn finish(&self) {
        if !self.agent.alive.swap(false, Ordering::AcqRel) {
            return;
        }
        self.cancel.cancel();
        operations::forget(self);
        for session in sessions::list() {
            let control = session.control();
            if control.view().agent_id.as_deref() == Some(&self.agent.id) {
                if let Err(error) = control.release(None, true) {
                    log::warn!("MCP binding cleanup: {}", error.code);
                }
                session.set_capture_enabled(false);
                if let Some(core) = sessions::core(&session.snapshot().session_id) {
                    core.lc.write().unwrap().automation_forget_credentials();
                }
                crate::automation::subscriptions::detach(&session.snapshot().session_id);
                wire::wake(&session.snapshot().session_id);
            }
        }
        state().clients.lock().unwrap().remove(&self.agent.id);
    }
}
pub(crate) struct ServiceState {
    status: Mutex<Status>,
    token: Mutex<Option<String>>,
    pub clients: Mutex<HashMap<String, Arc<Client>>>,
    shutdown: Mutex<Option<CancellationToken>>,
}
fn state() -> &'static ServiceState {
    static STATE: OnceLock<ServiceState> = OnceLock::new();
    STATE.get_or_init(|| ServiceState {
        status: Mutex::new(Status {
            settings: Settings::load(),
            state: "disabled".into(),
            address: None,
            error: None,
        }),
        token: Mutex::new(None),
        clients: Default::default(),
        shutdown: Default::default(),
    })
}
static COMMANDS: OnceLock<mpsc::Sender<(u64, Settings)>> = OnceLock::new();
static GUI_READY: AtomicBool = AtomicBool::new(false);
static LOADED: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicU64 = AtomicU64::new(0);
static CONFIG_LOCK: Mutex<()> = Mutex::new(());

pub fn install_runtime() {
    let (tx, mut rx) = mpsc::channel::<(u64, Settings)>(16);
    if COMMANDS.set(tx).is_err() {
        return;
    }
    tokio::spawn(async move {
        let mut running: Option<tokio::task::JoinHandle<()>> = None;
        while let Some((generation, settings)) = rx.recv().await {
            if generation != GENERATION.load(Ordering::Acquire) {
                continue;
            }
            let existing = state().status.lock().unwrap().clone();
            control::set_approval_required(settings.approval_required);
            if settings.enabled
                && existing.state == "running"
                && existing.settings.port == settings.port
            {
                state().status.lock().unwrap().settings = settings;
                continue;
            }
            stop_now();
            if let Some(mut task) = running.take() {
                if tokio::time::timeout(Duration::from_secs(3), &mut task)
                    .await
                    .is_err()
                {
                    task.abort();
                }
            }
            control::set_approval_required(settings.approval_required);
            {
                let mut status = state().status.lock().unwrap();
                status.settings = settings.clone();
                status.state = if settings.enabled {
                    "starting"
                } else {
                    "disabled"
                }
                .into();
                status.address = None;
                status.error = None;
            }
            if !settings.enabled {
                continue;
            }
            let token =
                match tokio::task::spawn_blocking(|| credentials::load_or_create(false)).await {
                    Ok(Ok(token)) => token,
                    _ => {
                        failed("Could not load MCP credential file");
                        continue;
                    }
                };
            if generation != GENERATION.load(Ordering::Acquire) {
                continue;
            }
            *state().token.lock().unwrap() = Some(token.clone());
            let shutdown = CancellationToken::new();
            *state().shutdown.lock().unwrap() = Some(shutdown.clone());
            match transport::listen(settings.port, token, shutdown.clone()).await {
                Ok(task) => {
                    if generation != GENERATION.load(Ordering::Acquire) {
                        shutdown.cancel();
                        running = Some(task);
                        continue;
                    }
                    let mut status = state().status.lock().unwrap();
                    status.state = "running".into();
                    status.address = Some(format!("http://127.0.0.1:{}/mcp", settings.port));
                    running = Some(task);
                }
                Err(error) => failed(&error.message),
            }
        }
    });
    tokio::spawn(async {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            gui::tick();
            terminals::tick();
            crate::automation::screen::tick();
            operations::tick();
            for session in sessions::list() {
                let control = session.control();
                control.tick();
                if let Some((generation, _)) = control.pending() {
                    if matches!(
                        session.snapshot().state,
                        sessions::ConnectionState::Connecting
                            | sessions::ConnectionState::Disconnected
                            | sessions::ConnectionState::Closed
                    ) {
                        control.complete_transition(generation, None);
                    } else {
                        wire::wake(&session.snapshot().session_id);
                    }
                }
            }
        }
    });
    if GUI_READY.load(Ordering::Acquire) {
        gui_ready();
    }
}

pub fn gui_ready() {
    GUI_READY.store(true, Ordering::Release);
    if COMMANDS.get().is_some() && !LOADED.swap(true, Ordering::AcqRel) {
        let settings = Settings::load();
        if configure(settings.enabled, settings.port, settings.approval_required).is_err() {
            LOADED.store(false, Ordering::Release);
            failed("MCP configuration queue is full");
        }
    }
}
fn failed(error: &str) {
    let mut status = state().status.lock().unwrap();
    status.state = "failed".into();
    status.error = Some(error.to_owned());
    status.address = None;
}
fn stop_now() {
    {
        let mut status = state().status.lock().unwrap();
        status.state = "disabled".into();
        status.address = None;
    }
    if let Some(shutdown) = state().shutdown.lock().unwrap().take() {
        shutdown.cancel();
    }
    let clients = state()
        .clients
        .lock()
        .unwrap()
        .values()
        .cloned()
        .collect::<Vec<_>>();
    for client in clients {
        client.finish();
    }
}
pub fn configure(enabled: bool, port: u16, approval_required: bool) -> Result<()> {
    if port == 0 {
        return Err(BridgeError::invalid("Port must be 1..65535"));
    }
    let settings = Settings {
        enabled,
        port,
        approval_required,
    };
    let commands = COMMANDS
        .get()
        .ok_or_else(|| BridgeError::new("SERVICE_UNAVAILABLE", "GUI async runtime is not ready"))?;
    let _config = CONFIG_LOCK.lock().unwrap();
    let slot = commands
        .try_reserve()
        .map_err(|_| BridgeError::new("LIMIT_EXCEEDED", "Configuration queue is full"))?;
    let generation = GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
    let previous = state().status.lock().unwrap().clone();
    if !enabled || previous.settings.port != port {
        stop_now();
    }
    settings.save();
    slot.send((generation, settings));
    Ok(())
}
pub fn credential(reset: bool) -> Result<String> {
    if reset {
        stop_now();
    }
    let token = credentials::load_or_create(reset)?;
    *state().token.lock().unwrap() = Some(token.clone());
    if reset {
        let settings = Settings::load();
        configure(settings.enabled, settings.port, settings.approval_required)?;
    }
    Ok(token)
}
pub fn settings_json() -> String {
    let status = state().status.lock().unwrap().clone();
    let clients = state()
        .clients
        .lock()
        .unwrap()
        .values()
        .filter(|client| client.initialized.load(Ordering::Acquire))
        .cloned()
        .collect::<Vec<_>>();
    let sessions = sessions::list();
    let agents = clients.iter().map(|client| serde_json::json!({
        "agent_id":client.agent.id, "name":*client.name.lock().unwrap(), "version":*client.version.lock().unwrap(),
        "connected_at":client.created, "last_communication":*client.last_communication.lock().unwrap(),
        "sessions":sessions.iter().filter(|s| s.control().view().agent_id.as_deref() == Some(&client.agent.id)).map(|s| {
            let snapshot = s.snapshot(); serde_json::json!({"session_id":snapshot.session_id,"peer_id":snapshot.peer_id,"ui_session_ids":snapshot.ui_session_ids,"control":s.control().view().mode,"connection_state":format!("{:?}",snapshot.state),"terminals":terminals::list(&snapshot.session_id)})
        }).collect::<Vec<_>>()
    })).collect::<Vec<_>>();
    serde_json::json!({"available":true,"status":status,"agents":agents}).to_string()
}
