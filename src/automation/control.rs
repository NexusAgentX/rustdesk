use super::error::{BridgeError, Result};
use hbb_common::tokio::sync::watch;
use serde::Serialize;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicI64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use uuid::Uuid;

static APPROVAL_REQUIRED: AtomicBool = AtomicBool::new(true);

#[derive(Clone)]
pub struct Agent {
    pub id: String,
    pub alive: Arc<AtomicBool>,
    deadline: Arc<AtomicI64>,
}

impl Agent {
    pub fn new() -> Self {
        Self {
            id: format!("agent_{}", Uuid::new_v4()),
            alive: Arc::new(AtomicBool::new(true)),
            deadline: Arc::new(AtomicI64::new(i64::MAX)),
        }
    }
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
            && chrono::Utc::now().timestamp_millis() < self.deadline.load(Ordering::Acquire)
    }
    pub fn renew(&self) {
        self.deadline.store(
            chrono::Utc::now().timestamp_millis() + 30_000,
            Ordering::Release,
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Human,
    Ai,
}

#[derive(Clone, Debug, Serialize)]
pub struct Approval {
    pub id: String,
    pub state: String,
    pub reason: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub remaining_ms: u64,
    #[serde(skip)]
    deadline: Instant,
    #[serde(skip)]
    generation: u64,
}

struct Reference {
    value: String,
    generation: u64,
    created: Instant,
}
struct Binding {
    id: String,
    agent: Agent,
    references: VecDeque<Reference>,
}

struct State {
    binding: Option<Binding>,
    generation: u64,
    epoch: u64,
    mode: Mode,
    pending: Option<(u64, Mode)>,
    approval: Option<Approval>,
    history: VecDeque<Approval>,
    revision: u64,
    release_error: Option<String>,
    released: Option<(u64, ReleasedInputs)>,
    // Once installed, queued unmarked GUI writes cannot cross a handover.
    installed: bool,
    fenced: bool,
    reconnecting: bool,
}

pub struct Authority {
    pub session_id: String,
    state: Mutex<State>,
    changes: watch::Sender<u64>,
}

#[derive(Clone, Serialize)]
pub struct ReleasedInputs {
    pub keys: usize,
    pub buttons: usize,
}

#[derive(Clone, Serialize)]
pub struct ControlView {
    pub mode: Mode,
    pub approval_required: bool,
    pub transitioning: bool,
    pub agent_id: Option<String>,
    pub session_ref: Option<String>,
    pub approval: Option<Approval>,
    pub revision: String,
    pub release_error: Option<String>,
    pub released_inputs: Option<ReleasedInputs>,
}

#[derive(Clone)]
pub struct Permit {
    pub authority: Arc<Authority>,
    pub generation: u64,
    pub epoch: u64,
    binding: Option<String>,
    pub(crate) human: bool,
}

impl Permit {
    pub fn binding_id(&self) -> &str {
        self.binding.as_deref().unwrap_or("")
    }
    pub fn read_check(&self) -> Result<()> {
        let state = self.authority.state.lock().unwrap();
        if state
            .binding
            .as_ref()
            .is_some_and(|b| Some(&b.id) == self.binding.as_ref() && b.agent.is_alive())
        {
            Ok(())
        } else {
            Err(BridgeError::new(
                "BINDING_EXPIRED",
                "Binding ended or MCP client disconnected",
            ))
        }
    }
    pub fn check(&self) -> Result<()> {
        let state = self.authority.state.lock().unwrap();
        if state.generation != self.generation
            || state.epoch != self.epoch
            || state.pending.is_some()
        {
            return Err(BridgeError::new(
                "CONTROL_EXPIRED",
                "Control or connection changed before sending",
            ));
        }
        if self.human {
            if state.mode != Mode::Human {
                return Err(BridgeError::new(
                    "HUMAN_CONTROL_REQUIRED",
                    "AI controls this session",
                ));
            }
        } else {
            let valid = state.binding.as_ref().is_some_and(|binding| {
                Some(&binding.id) == self.binding.as_ref() && binding.agent.is_alive()
            });
            if !valid || state.mode != Mode::Ai {
                return Err(BridgeError::new(
                    "HUMAN_CONTROL",
                    "Request AI control before writing",
                ));
            }
        }
        Ok(())
    }
}

impl Authority {
    pub fn new(session_id: String) -> Self {
        let (changes, _) = watch::channel(0);
        Self {
            session_id,
            changes,
            state: Mutex::new(State {
                binding: None,
                generation: 0,
                epoch: 0,
                mode: Mode::Human,
                pending: None,
                approval: None,
                history: VecDeque::new(),
                revision: 0,
                release_error: None,
                released: None,
                installed: false,
                fenced: false,
                reconnecting: false,
            }),
        }
    }

    fn notify(&self, state: &mut State) {
        state.revision += 1;
        self.changes.send_replace(state.revision);
    }

    fn rotate(state: &mut State) {
        state.generation += 1;
        if let Some(binding) = &mut state.binding {
            if let Some(current) = binding.references.back_mut() {
                current.created = Instant::now();
            }
            binding
                .references
                .retain(|r| r.created.elapsed() < Duration::from_secs(300));
            while binding.references.len() >= 64 {
                binding.references.pop_front();
            }
            binding.references.push_back(Reference {
                value: format!("r_{}", Uuid::new_v4()),
                generation: state.generation,
                created: Instant::now(),
            });
        }
    }

    fn end_approval(state: &mut State, outcome: &str) {
        if let Some(mut approval) = state.approval.take() {
            approval.state = outcome.to_owned();
            approval.remaining_ms = 0;
            approval.deadline = Instant::now();
            state.history.push_back(approval);
            while state.history.len() > 64 {
                state.history.pop_front();
            }
        }
    }

    pub fn tick(&self) {
        let mut state = self.state.lock().unwrap();
        if state
            .approval
            .as_ref()
            .is_some_and(|a| Instant::now() >= a.deadline)
        {
            Self::end_approval(&mut state, "expired");
            self.notify(&mut state);
        }
        state
            .history
            .retain(|a| a.deadline.elapsed().as_secs() < 300);
    }

    pub fn view(&self) -> ControlView {
        self.tick();
        let state = self.state.lock().unwrap();
        ControlView {
            mode: state.mode,
            approval_required: APPROVAL_REQUIRED.load(Ordering::Acquire),
            transitioning: state.pending.is_some(),
            agent_id: state.binding.as_ref().map(|b| b.agent.id.clone()),
            session_ref: state
                .binding
                .as_ref()
                .and_then(|b| b.references.back())
                .map(|r| r.value.clone()),
            approval: state
                .approval
                .clone()
                .or_else(|| state.history.back().cloned())
                .map(|mut a| {
                    if a.state == "pending" {
                        a.remaining_ms = a
                            .deadline
                            .saturating_duration_since(Instant::now())
                            .as_millis() as u64;
                    }
                    a
                }),
            revision: state.revision.to_string(),
            release_error: state.release_error.clone(),
            released_inputs: state
                .released
                .as_ref()
                .filter(|(generation, _)| *generation == state.generation)
                .map(|(_, value)| value.clone()),
        }
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    pub fn attach(&self, agent: &Agent, created: bool) -> Result<()> {
        if !agent.is_alive() {
            return Err(BridgeError::new("BINDING_EXPIRED", "MCP session has ended"));
        }
        let mut state = self.state.lock().unwrap();
        if let Some(binding) = &state.binding {
            return if binding.agent.id == agent.id {
                Ok(())
            } else {
                Err(BridgeError::new(
                    "SESSION_BUSY",
                    "Another agent owns this core session",
                ))
            };
        }
        state.history.clear();
        state.binding = Some(Binding {
            id: Uuid::new_v4().to_string(),
            agent: agent.clone(),
            references: VecDeque::new(),
        });
        state.installed = true;
        Self::rotate(&mut state);
        if created {
            state.fenced = true;
            state.pending = Some((state.generation, Mode::Ai));
        }
        self.notify(&mut state);
        Ok(())
    }

    pub fn resolve(
        self: &Arc<Self>,
        agent: &Agent,
        reference: &str,
        write: bool,
    ) -> Result<Permit> {
        let state = self.state.lock().unwrap();
        let binding = state
            .binding
            .as_ref()
            .filter(|b| b.agent.id == agent.id && agent.is_alive())
            .ok_or_else(|| {
                BridgeError::new(
                    "SESSION_REF_EXPIRED",
                    "Binding is absent or belongs to another MCP session",
                )
            })?;
        let current = binding.references.back().map(|r| r.value.as_str()) == Some(reference);
        let entry = binding
            .references
            .iter()
            .find(|r| {
                r.value == reference && (current || r.created.elapsed() < Duration::from_secs(300))
            })
            .ok_or_else(|| {
                BridgeError::new("SESSION_REF_EXPIRED", "Session reference has expired")
            })?;
        if write && entry.generation != state.generation {
            return Err(BridgeError::new(
                "CONTROL_EXPIRED",
                "Read the session to obtain its current reference",
            ));
        }
        Ok(Permit {
            authority: self.clone(),
            generation: entry.generation,
            epoch: state.epoch,
            binding: Some(binding.id.clone()),
            human: false,
        })
    }

    pub fn human_permit(self: &Arc<Self>) -> Option<Permit> {
        let state = self.state.lock().unwrap();
        if !state.installed {
            return None;
        }
        Some(Permit {
            authority: self.clone(),
            generation: state.generation,
            epoch: state.epoch,
            binding: None,
            human: true,
        })
    }

    pub fn binding_id(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap()
            .binding
            .as_ref()
            .map(|b| b.id.clone())
    }

    pub fn rejects_unmarked(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.installed && (state.fenced || state.mode != Mode::Human || state.pending.is_some())
    }

    pub fn installed(&self) -> bool {
        self.state.lock().unwrap().installed
    }

    pub fn request(&self, permit: &Permit, reason: String) -> Result<()> {
        self.tick();
        let mut state = self.state.lock().unwrap();
        if permit.generation != state.generation
            || state.binding.as_ref().map(|b| &b.id) != permit.binding.as_ref()
        {
            return Err(BridgeError::new(
                "CONTROL_EXPIRED",
                "Control request belongs to an old generation",
            ));
        }
        if state.mode == Mode::Ai || state.pending.is_some() || state.approval.is_some() {
            return Ok(());
        }
        if APPROVAL_REQUIRED.load(Ordering::Acquire) {
            let now = chrono::Utc::now();
            state.approval = Some(Approval {
                id: format!("a_{}", Uuid::new_v4()),
                state: "pending".into(),
                reason,
                created_at: now,
                expires_at: now + chrono::Duration::seconds(60),
                remaining_ms: 60_000,
                deadline: Instant::now() + Duration::from_secs(60),
                generation: state.generation,
            });
        } else {
            Self::rotate(&mut state);
            state.fenced = true;
            state.pending = Some((state.generation, Mode::Ai));
        }
        self.notify(&mut state);
        Ok(())
    }

    pub fn approve(&self, id: &str, accept: bool) -> Result<()> {
        self.tick();
        let mut state = self.state.lock().unwrap();
        let pending = state
            .approval
            .as_ref()
            .is_some_and(|a| a.id == id && a.generation == state.generation)
            && state.binding.as_ref().is_some_and(|b| b.agent.is_alive());
        if !pending {
            return Err(BridgeError::new(
                "APPROVAL_EXPIRED",
                "Approval is no longer pending",
            ));
        }
        Self::end_approval(&mut state, if accept { "approved" } else { "rejected" });
        if accept {
            Self::rotate(&mut state);
            state.fenced = true;
            state.pending = Some((state.generation, Mode::Ai));
        }
        self.notify(&mut state);
        Ok(())
    }

    pub fn cancel_approval(&self, permit: &Permit, id: &str) -> Result<Approval> {
        self.tick();
        let mut state = self.state.lock().unwrap();
        if state.binding.as_ref().map(|b| &b.id) != permit.binding.as_ref() {
            return Err(BridgeError::new("SESSION_REF_EXPIRED", "Binding ended"));
        }
        if state.approval.as_ref().is_some_and(|a| a.id == id) {
            Self::end_approval(&mut state, "cancelled");
            self.notify(&mut state);
        }
        state
            .history
            .iter()
            .find(|a| a.id == id)
            .cloned()
            .ok_or_else(|| BridgeError::new("APPROVAL_EXPIRED", "Approval record has expired"))
    }

    pub fn release(&self, permit: Option<&Permit>, detach: bool) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        if let Some(permit) = permit {
            if state.binding.as_ref().map(|b| &b.id) != permit.binding.as_ref() {
                return Err(BridgeError::new("SESSION_REF_EXPIRED", "Binding ended"));
            }
            if !detach && state.generation != permit.generation {
                return Err(BridgeError::new(
                    "CONTROL_EXPIRED",
                    "A newer control grant exists",
                ));
            }
        }
        Self::end_approval(&mut state, "cancelled");
        state.reconnecting = false;
        Self::rotate(&mut state);
        state.fenced = true;
        state.pending = Some((state.generation, Mode::Human));
        if detach {
            state.binding = None;
        }
        self.notify(&mut state);
        Ok(())
    }

    pub fn pending(&self) -> Option<(u64, Mode)> {
        self.state.lock().unwrap().pending
    }

    pub fn record_released(&self, generation: u64, keys: usize, buttons: usize) {
        let mut state = self.state.lock().unwrap();
        if state.generation == generation {
            state.released = Some((generation, ReleasedInputs { keys, buttons }));
        }
    }
    pub fn complete_transition(&self, generation: u64, error: Option<String>) {
        let mut state = self.state.lock().unwrap();
        if let Some((expected, mode)) = state.pending {
            if expected == generation {
                if state.released.as_ref().map(|(g, _)| *g) != Some(generation) {
                    state.released = Some((
                        generation,
                        ReleasedInputs {
                            keys: 0,
                            buttons: 0,
                        },
                    ));
                }
                state.mode = mode;
                state.pending = None;
                state.release_error = error;
                self.notify(&mut state);
            }
        }
    }

    pub fn first_connection(&self, epoch: u64) {
        let mut state = self.state.lock().unwrap();
        state.epoch = epoch;
        self.notify(&mut state);
    }

    pub fn prepare_reconnect(&self, permit: &Permit) -> Result<()> {
        permit.check()?;
        let mut state = self.state.lock().unwrap();
        if state.generation != permit.generation {
            return Err(BridgeError::new("CONTROL_EXPIRED", "Control changed before reconnect"));
        }
        state.reconnecting = true;
        Ok(())
    }

    pub fn disconnected(&self, epoch: u64, closed: bool) {
        let mut state = self.state.lock().unwrap();
        Self::end_approval(&mut state, "cancelled");
        let preserve = state.reconnecting && !closed && state.mode == Mode::Ai;
        if closed || epoch > state.epoch {
            state.reconnecting = false;
        }
        state.epoch = epoch;
        state.fenced = true;
        Self::rotate(&mut state);
        state.mode = if preserve { Mode::Ai } else { Mode::Human };
        state.pending = None;

        self.notify(&mut state);
    }
}

pub fn set_approval_required(required: bool) {
    APPROVAL_REQUIRED.store(required, Ordering::Release);
    for session in super::sessions::list() {
        let control = session.control();
        let mut state = control.state.lock().unwrap();
        Authority::end_approval(&mut state, "cancelled");
        control.notify(&mut state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_reconnect_preserves_control_but_human_takeover_wins() {
        let (authority, agent, permit) = joined(true);
        authority.prepare_reconnect(&permit).unwrap();
        authority.disconnected(0, false);
        authority.disconnected(1, false);
        assert!(matches!(authority.view().mode, Mode::Ai));
        assert!(permit.check().is_err());
        let reference = authority.view().session_ref.unwrap();
        let current = authority.resolve(&agent, &reference, true).unwrap();
        authority.prepare_reconnect(&current).unwrap();
        authority.release(None, false).unwrap();
        authority.disconnected(2, false);
        assert!(matches!(authority.view().mode, Mode::Human));
    }

    fn joined(created: bool) -> (Arc<Authority>, Agent, Permit) {
        let authority = Arc::new(Authority::new(Uuid::new_v4().to_string()));
        let agent = Agent::new();
        authority.attach(&agent, created).unwrap();
        if let Some((generation, _)) = authority.pending() {
            authority.complete_transition(generation, None);
        }
        let reference = authority.view().session_ref.unwrap();
        let permit = authority.resolve(&agent, &reference, true).unwrap();
        (authority, agent, permit)
    }
    #[test]
    fn binding_is_exclusive_per_session_not_per_agent_connection() {
        let (authority, agent, _) = joined(false);
        let other = Agent::new();
        assert_eq!(
            authority.attach(&other, false).unwrap_err().code,
            "SESSION_BUSY"
        );
        let second = Authority::new("second".into());
        assert!(second.attach(&other, false).is_ok());
        assert!(authority.attach(&agent, false).is_ok());
        assert_eq!(authority.view().mode, Mode::Human);
    }
    #[test]
    fn takeover_invalidates_queued_writes_before_cleanup() {
        let (authority, agent, permit) = joined(true);
        assert!(permit.check().is_ok());
        let old = authority.view().session_ref.unwrap();
        authority.release(None, false).unwrap();
        assert!(permit.check().is_err());
        assert!(authority.resolve(&agent, &old, false).is_ok());
        assert_eq!(
            authority.resolve(&agent, &old, true).err().unwrap().code,
            "CONTROL_EXPIRED"
        );
        let (generation, _) = authority.pending().unwrap();
        authority.complete_transition(generation, None);
        assert_eq!(authority.view().mode, Mode::Human);
    }
    #[test]
    fn dead_agent_cannot_read_or_write_even_before_cleanup() {
        let (_, agent, permit) = joined(true);
        agent.alive.store(false, Ordering::Release);
        assert!(permit.read_check().is_err());
        assert!(permit.check().is_err());
    }
    #[test]
    fn expired_heartbeat_blocks_resume_before_watchdog_runs() {
        let (_, agent, permit) = joined(true);
        agent
            .deadline
            .store(chrono::Utc::now().timestamp_millis() - 1, Ordering::Release);
        assert!(permit.check().is_err());
        assert!(permit.read_check().is_err());
    }
    #[test]
    fn pending_requests_reuse_deadline_and_late_approval_cannot_grant() {
        let (authority, agent, permit) = joined(false);
        authority.request(&permit, "first".into()).unwrap();
        let first = authority.view().approval.unwrap();
        authority.request(&permit, "duplicate".into()).unwrap();
        let second = authority.view().approval.unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(first.expires_at, second.expires_at);
        authority.release(None, false).unwrap();
        assert!(authority.approve(&first.id, true).is_err());
        assert!(authority
            .resolve(&agent, &authority.view().session_ref.unwrap(), false)
            .is_ok());
    }
    #[test]
    fn completion_of_old_cleanup_never_overwrites_a_newer_takeover() {
        let (authority, _, _) = joined(true);
        authority.release(None, false).unwrap();
        let (old, _) = authority.pending().unwrap();
        authority.release(None, false).unwrap();
        let (current, _) = authority.pending().unwrap();
        authority.complete_transition(old, None);
        assert!(authority.view().transitioning);
        authority.complete_transition(current, None);
        assert!(!authority.view().transitioning);
    }
    #[test]
    fn detached_binding_cannot_access_later_binding_by_same_agent() {
        let (authority, agent, permit) = joined(true);
        authority.release(Some(&permit), true).unwrap();
        authority.attach(&agent, false).unwrap();
        assert!(permit.read_check().is_err());
        assert!(permit.check().is_err());
        assert!(authority.release(Some(&permit), true).is_err());
    }
    #[test]
    fn a_new_binding_does_not_inherit_previous_approval_history() {
        let (authority, _, permit) = joined(false);
        authority.request(&permit, "private reason".into()).unwrap();
        authority.release(Some(&permit), true).unwrap();
        let other = Agent::new();
        authority.attach(&other, false).unwrap();
        assert!(authority.view().approval.is_none());
    }
    #[test]
    fn current_reference_survives_retention_but_old_references_do_not() {
        let (authority, agent, _) = joined(false);
        let reference = authority.view().session_ref.unwrap();
        authority
            .state
            .lock()
            .unwrap()
            .binding
            .as_mut()
            .unwrap()
            .references
            .back_mut()
            .unwrap()
            .created = Instant::now() - Duration::from_secs(301);
        assert!(authority.resolve(&agent, &reference, false).is_ok());
        authority.release(None, false).unwrap();
        assert!(authority.resolve(&agent, &reference, false).is_ok());
        authority
            .state
            .lock()
            .unwrap()
            .binding
            .as_mut()
            .unwrap()
            .references
            .front_mut()
            .unwrap()
            .created = Instant::now() - Duration::from_secs(301);
        assert!(authority.resolve(&agent, &reference, false).is_err());
    }
}
