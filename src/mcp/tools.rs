use super::{types::*, Client};
use crate::automation::{
    api, auth,
    control::Mode,
    error::{BridgeError, Result},
    gui, input, screen,
    sessions::{self, ConnectionState, SessionKind},
    terminals,
};
use hbb_common::tokio;
use rmcp::model::{CallToolResult, ContentBlock, Tool, ToolAnnotations};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::{json, Map, Value};
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub(super) struct Reply {
    pub value: Value,
    pub png: Option<Vec<u8>>,
}
impl Reply {
    pub fn success(data: Value) -> Self {
        Self {
            value: json!({"ok":true,"status":"completed","data":data}),
            png: None,
        }
    }
    pub fn status(mut self, status: &str) -> Self {
        self.value["status"] = json!(status);
        self
    }
    pub fn error(error: BridgeError) -> Self {
        Self {
            value: json!({"ok":false,"status":"failed","data":{},"error":error}),
            png: None,
        }
    }
    pub fn result(self) -> CallToolResult {
        let mut content = vec![ContentBlock::text(self.value.to_string())];
        if let Some(png) = self.png {
            content.push(ContentBlock::image(crate::encode64(png), "image/png"));
        }
        let mut result = CallToolResult::success(content);
        result.is_error = Some(self.value["ok"] != true);
        result.structured_content = Some(self.value);
        result
    }
}
fn definition<T: JsonSchema + 'static>(
    name: &'static str,
    description: &'static str,
    read: bool,
) -> Tool {
    let mut annotations = ToolAnnotations::default();
    annotations.read_only_hint = Some(read);
    annotations.destructive_hint = Some(!read);
    annotations.open_world_hint = Some(true);
    Tool::new(name, description, Arc::new(Map::new()))
        .with_input_schema::<T>()
        .with_raw_output_schema(Arc::new(output_schema(name)))
        .with_annotations(annotations)
}
pub fn definitions() -> Vec<Tool> {
    vec![
    definition::<Read>("rd_capabilities_get","Inspect implemented MCP capabilities for this session, including peer version, permission, control and readiness blockers. Unknown support is not permission. Does not take control.",true),
    definition::<OperationGet>("rd_operation_get","Query this MCP client's retained operation_id without replaying a write. Optional wait_ms waits for local execution, not remote application completion. Original pending results have unknown final outcome; inspect current session or terminal state. Records expire five minutes after execution and when this MCP client ends.",true),
    definition::<List>("rd_session_list","Discover visible desktop and terminal sessions. Multiple MCP agents may connect; ownership is exclusive per core session.",true),
    definition::<Open>("rd_session_open","Open a visible GUI session or reuse and attach to an existing one. Only newly created sessions start under AI control. Optional password is single-use connection authentication, never OS login.",false),
    definition::<Attach>("rd_session_attach","Attach to an existing core session without changing its control mode. Returns session_ref; fails if another agent owns it.",false),
    definition::<Write>("rd_session_detach","Detach this binding, cancel queued input and return control to the human. Keep the GUI and remote connection.",false),
    definition::<Get>("rd_session_get","Read current session state and session_ref. Optionally wait for a revision change or query an operation_id. Old references remain read-only within the binding.",true),
    definition::<Authenticate>("rd_session_authenticate","Submit credentials for the current authentication challenge under AI control. Does not save passwords or trust devices.",false),
    definition::<WaitWrite>("rd_session_disconnect","Disconnect the remote connection while retaining the visible GUI container. Revokes AI control. wait_ms defaults to 10000; timeout returns pending; query session_get for completion.",false),
    definition::<Reconnect>("rd_session_reconnect","Explicitly reconnect the visible session. Preserves the authorized AI control through reconnect and authentication unless the human takes over. Refresh session_ref after reconnecting.",false),
    definition::<WaitWrite>("rd_session_close","Close all GUI views of this logical session and its remote connection. Other sessions remain open. wait_ms defaults to 10000; timeout returns pending; query session_get for completion.",false),
    definition::<ControlRequest>("rd_control_request","Explicitly request AI control. Human approval is required by default. Repeated pending requests do not extend the deadline.",false),
    definition::<ControlCancel>("rd_control_cancel","Cancel one pending approval request. Returns an already-final result without undoing a completed control grant.",false),
    definition::<Write>("rd_control_release","Voluntarily return control to the human, invalidate queued AI input and release held keys and buttons. Keep the binding for reading.",false),
    definition::<Capture>("rd_screen_capture","Read remote decoded pixels as a native PNG image block, including geometry and snapshot_id for input. Available under human control. Specify actual display_id with after_frame_seq. wait_ms is a maximum wait for a qualifying frame, not a fixed delay; cached frames may return immediately.",true),
    definition::<Input>("rd_input_send","Send 1..32 ordered input actions under AI control. Coordinate actions require a recent snapshot_id. Keys use exact names such as KeyL, Digit1, Enter and ArrowLeft. Example Ctrl+L: {\"type\":\"shortcut\",\"modifiers\":[\"Control\"],\"key\":\"KeyL\"}. Use text actions for literal text. Insert {\"type\":\"wait\",\"duration_ms\":500} before subsequent actions when the application needs time; total batch delays must be <=30000 ms. Use operation_id for safe retries. Optional capture.delay_ms delays observation after sending (default 0, max 30000 ms); capture.wait_ms then waits at most for a frame newer than the pre-input frame (default 1000, max 30000 ms), returning immediately if available. Neither guarantees remote application completion. Observation failures never replay input.",false),
    definition::<Read>("rd_terminal_list","List terminal instances in this terminal connection without creating one.",true),
    definition::<TerminalCreate>("rd_terminal_create","Create a visible GUI terminal tab before requesting a remote interactive shell. Requires a terminal connection and AI control.",false),
    definition::<TerminalRead>("rd_terminal_read","Incrementally read bounded raw terminal output, including ANSI and carriage returns. Cursors count bytes; OUTPUT_GAP reports recoverable oldest_cursor. Shell exit status is not a command exit status.",true),
    definition::<TerminalWrite>("rd_terminal_write","Send exact UTF-8 text to an open interactive terminal without adding a newline. Optional read is a separate observation, not a command-completion guarantee.",false),
    definition::<TerminalResize>("rd_terminal_resize","Request PTY rows 1..500 and cols 1..1000. Delivery is transport evidence, not remote size acknowledgement.",false),
    definition::<TerminalClose>("rd_terminal_close","Close one terminal instance and observe its remote closing state. Preserve other terminals.",false),
]
}
fn parse<T: DeserializeOwned>(args: &Map<String, Value>) -> Result<T> {
    serde_json::from_value(Value::Object(args.clone()))
        .map_err(|_| BridgeError::invalid("Arguments do not match this tool's schema"))
}
fn wait(ms: Option<u64>, default: u64) -> Result<u64> {
    let ms = ms.unwrap_or(default);
    api::wait_budget(ms)?;
    Ok(ms)
}
fn current(session: &sessions::SessionHandle) -> Result<String> {
    session
        .control()
        .view()
        .session_ref
        .ok_or_else(|| BridgeError::new("BINDING_EXPIRED", "Binding ended"))
}

pub async fn call(
    client: Arc<Client>,
    name: &str,
    args: Map<String, Value>,
    cancel: CancellationToken,
) -> CallToolResult {
    if !client
        .initialized
        .load(std::sync::atomic::Ordering::Acquire)
        || !client.agent.is_alive()
    {
        return Reply::error(BridgeError::new(
            "BINDING_EXPIRED",
            "MCP logical session is not active",
        ))
        .result();
    }
    let Ok(_slot) = client.calls.clone().try_acquire_owned() else {
        return Reply::error(BridgeError::new(
            "LIMIT_EXCEEDED",
            "Too many concurrent calls for this MCP client",
        ))
        .result();
    };
    let result = super::operations::execute(client, name, args, cancel).await;
    result.unwrap_or_else(Reply::error).result()
}
pub(super) async fn dispatch(
    client: &Client,
    name: &str,
    args: &Map<String, Value>,
    cancel: &CancellationToken,
) -> Result<Reply> {
    let agent = &client.agent;
    match name {
        "rd_capabilities_get" => {
            let p: Read = parse(args)?;
            let (session, permit) = api::resolve(agent, &p.session_ref, false)?;
            permit.read_check()?;
            Ok(Reply::success(super::capabilities::view(&session)))
        }
        "rd_operation_get" => {
            let p: OperationGet = parse(args)?;
            let ms = wait(p.wait_ms, 0)?;
            Ok(Reply::success(json!({"operation":super::operations::wait_query(client, &p.operation_id, ms).await?})))
        }
        "rd_session_list" => {
            let p: List = parse(args)?;
            let limit = p.limit.unwrap_or(50);
            if !(1..=100).contains(&limit) {
                return Err(BridgeError::invalid("limit must be 1..100"));
            }
            let mut all = sessions::list()
                .into_iter()
                .filter(|s| s.snapshot().state != ConnectionState::Closed)
                .collect::<Vec<_>>();
            all.sort_by_key(|s| s.snapshot().session_id);
            let revision = crate::encode64(sha2::Sha256::digest(
                all.iter()
                    .map(|s| format!("{}:{}", s.snapshot().session_id, api::revision(s)))
                    .collect::<Vec<_>>()
                    .join("|")
                    .as_bytes(),
            ));
            let offset = if let Some(cursor) = p.cursor {
                let (r, o) = cursor
                    .rsplit_once(':')
                    .ok_or_else(|| BridgeError::invalid("Invalid list cursor"))?;
                if r != revision {
                    return Err(BridgeError::new(
                        "CURSOR_EXPIRED",
                        "Session list changed; restart listing",
                    ));
                }
                o.parse::<usize>()
                    .map_err(|_| BridgeError::invalid("Invalid list cursor"))?
            } else {
                0
            };
            let summaries=all.iter().filter_map(|s|{let snapshot=s.snapshot();let owner=s.control().view().agent_id;let mine=owner.as_deref()==Some(&agent.id);if matches!(p.scope,Some(Scope::Mine))&&!mine||matches!(p.scope,Some(Scope::Available))&&owner.is_some()&&!mine{return None;}Some(json!({"session_id":snapshot.session_id,"peer_id":snapshot.peer_id,"kind":if snapshot.kind==SessionKind::Desktop{"desktop"}else{"terminal"},"gui_registered":!snapshot.ui_session_ids.is_empty(),"connection_state":api::state_name(snapshot.state),"owner":if mine{"self"}else if owner.is_some(){"other"}else{"none"}}))}).collect::<Vec<_>>();
            let next = offset.checked_add(limit as usize).ok_or_else(|| {
                BridgeError::new(
                    "CURSOR_EXPIRED",
                    "List cursor is outside the current result",
                )
            })?;
            Ok(Reply::success(
                json!({"sessions":summaries.iter().skip(offset).take(limit as usize).collect::<Vec<_>>(),"next_cursor":(next<summaries.len()).then(||format!("{revision}:{next}"))}),
            ))
        }
        "rd_session_open" => {
            let p: Open = parse(args)?;
            let ms = wait(p.wait_ms, 10000)?;
            let kind = if matches!(p.kind, Some(Kind::Terminal)) {
                SessionKind::Terminal
            } else {
                SessionKind::Desktop
            };
            let (session, created) = gui::reserve(
                agent,
                p.peer_id.trim(),
                kind,
                p.password.clone(),
                p.force_relay.unwrap_or(false),
            )?;
            let mut intent = gui::OpenGuard::new(session.clone(), created);
            api::transition(&session);
            let reference = current(&session)?;
            let permit = session.control().resolve(agent, &reference, false)?;
            let mut applied = false;
            if !created && session.control().view().mode == Mode::Ai {
                let snapshot = session.snapshot();
                if let (Some(password), Some(challenge)) = (p.password, snapshot.auth_challenge) {
                    if !snapshot.authenticated && challenge.kind == "password" {
                        auth::submit(
                            permit.clone(),
                            challenge.id,
                            auth::Credentials::Password(password),
                        )
                        .await?;
                        applied = true;
                    }
                }
            }
            let started = tokio::time::Instant::now();
            let mut done = api::wait_session(&session, &permit, None, ms).await?;
            let initial = terminals::list(&session.snapshot().session_id)
                .first()
                .map(|t| t.terminal_id.clone());
            if session.snapshot().state == ConnectionState::Ready && kind == SessionKind::Terminal {
                if let Some(id) = &initial {
                    let remaining = ms.saturating_sub(started.elapsed().as_millis() as u64);
                    done = terminals::wait_open(&permit, id, remaining).await?.state != "opening";
                }
            }
            intent.finish();
            Ok(Reply::success(json!({"created":created,"session":api::view(&session,false),"initial_terminal_id":initial,"password_applied":if created {gui::password_applied(&session.snapshot().session_id)} else {applied}})).status(if done{"completed"}else{"pending"}))
        }
        "rd_session_attach" => {
            let p: Attach = parse(args)?;
            let session = sessions::get(&p.session_id)
                .filter(|s| s.snapshot().state != ConnectionState::Closed)
                .ok_or_else(|| {
                    BridgeError::new("SESSION_NOT_FOUND", "Visible session does not exist")
                })?;
            session.control().attach(agent, false)?;
            session.set_capture_enabled(session.snapshot().kind == SessionKind::Desktop);
            Ok(Reply::success(json!({"session":api::view(&session,false)})))
        }
        "rd_session_get" => {
            let p: Get = parse(args)?;
            let ms = wait(p.wait_ms, 0)?;
            let (session, permit) = api::resolve(agent, &p.session_ref, false)?;
            let changed = if let Some(after) = p.after_revision.as_deref() {
                api::wait_session(&session, &permit, Some(after), ms).await?
            } else {
                true
            };
            {
                let mut reply = Reply::success(
                    json!({"session":api::view(&session,matches!(p.detail,Some(Detail::Full)))}),
                )
                .status(if changed { "completed" } else { "unchanged" });
                if let Some(id) = p.operation_id {
                    reply.value["data"]["operation"] = super::operations::query(client, &id)?;
                }
                Ok(reply)
            }
        }
        "rd_session_detach" => {
            let p: Write = parse(args)?;
            {
                let mut detached = client.detached.lock().unwrap();
                detached.retain(|_, (at, _)| at.elapsed().as_secs() < 300);
                if let Some((_, id)) = detached.get(&p.session_ref) {
                    return Ok(Reply::success(json!({"session_id":id,"detached":true})));
                }
            }
            let (session, permit) = api::resolve(agent, &p.session_ref, false)?;
            session.control().release(Some(&permit), true)?;
            {
                let mut detached = client.detached.lock().unwrap();
                if detached.len() >= 256 {
                    if let Some(oldest) = detached
                        .iter()
                        .min_by_key(|(_, (at, _))| *at)
                        .map(|(key, _)| key.clone())
                    {
                        detached.remove(&oldest);
                    }
                }
                detached.insert(
                    p.session_ref,
                    (
                        std::time::Instant::now(),
                        session.snapshot().session_id.clone(),
                    ),
                );
            }
            session.set_capture_enabled(false);
            crate::automation::subscriptions::detach(&session.snapshot().session_id);
            api::transition(&session);
            Ok(Reply::success(
                json!({"session_id":session.snapshot().session_id,"detached":true}),
            ))
        }
        "rd_control_release" => {
            let p: Write = parse(args)?;
            let (session, permit) = api::resolve(agent, &p.session_ref, true)?;
            session.control().release(Some(&permit), false)?;
            api::transition(&session);
            Ok(Reply::success(json!({"session_ref":current(&session)?,"control":session.control().view(),"released_inputs":session.control().view().released_inputs})).status(if session.control().view().transitioning{"pending"}else{"completed"}))
        }
        "rd_control_request" => {
            let p: ControlRequest = parse(args)?;
            let ms = wait(p.wait_ms, 0)?;
            let reason = p.reason.unwrap_or_default();
            if reason.chars().count() > 200 {
                return Err(BridgeError::invalid("reason exceeds 200 characters"));
            }
            let (session, permit) = api::resolve(agent, &p.session_ref, true)?;
            let mut changed = session.control().subscribe();
            session.control().request(&permit, reason)?;
            api::transition(&session);
            let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
            while ms > 0 {
                permit.read_check()?;
                let c = session.control().view();
                if c.mode == Mode::Ai && !c.transitioning
                    || c.approval.as_ref().is_some_and(|a| a.state != "pending")
                {
                    break;
                }
                if tokio::time::timeout_at(deadline, changed.changed())
                    .await
                    .is_err()
                {
                    break;
                }
            }
            let c = session.control().view();
            Ok(Reply::success(
                json!({"session_ref":c.session_ref,"control":c,"approval":c.approval}),
            )
            .status(
                if !c.transitioning
                    && (c.mode == Mode::Ai
                        || c.approval.as_ref().is_some_and(|a| a.state != "pending"))
                {
                    "completed"
                } else {
                    "pending"
                },
            ))
        }
        "rd_control_cancel" => {
            let p: ControlCancel = parse(args)?;
            let (session, permit) = api::resolve(agent, &p.session_ref, false)?;
            Ok(Reply::success(
                json!({"approval":session.control().cancel_approval(&permit,&p.approval_id)?}),
            ))
        }
        "rd_screen_capture" => {
            let p: Capture = parse(args)?;
            let (_, permit) = api::resolve(agent, &p.session_ref, false)?;
            let ms = wait(p.wait_ms, 0)?;
            let after = p
                .after_frame_seq
                .map(|v| {
                    v.parse::<u64>()
                        .map_err(|_| BridgeError::invalid("Invalid frame sequence"))
                })
                .transpose()?;
            let observation = screen::capture(
                &permit,
                p.display_id.as_deref().unwrap_or("primary"),
                after,
                ms,
                p.max_width.unwrap_or(1600),
                p.max_height.unwrap_or(1600),
            )
            .await?;
            Ok(Reply {
                value: json!({"ok":true,"status":if observation.unchanged{"unchanged"}else{"completed"},"data":observation.data}),
                png: observation.png,
            })
        }
        "rd_input_send" => {
            let p: Input = parse(args)?;
            let (session, permit) = api::resolve(agent, &p.session_ref, true)?;
            if session.snapshot().kind != SessionKind::Desktop {
                return Err(BridgeError::new(
                    "WRONG_SESSION_KIND",
                    "Keyboard and mouse input require a desktop session",
                ));
            }
            if let Some(c) = &p.capture {
                wait(c.wait_ms, 1000)?;
                if c.delay_ms.unwrap_or(0) > 30_000 {
                    return Err(BridgeError::invalid("capture.delay_ms must be 0..30000"));
                }
                if !(1..=3840).contains(&c.max_width.unwrap_or(1600))
                    || !(1..=3840).contains(&c.max_height.unwrap_or(1600))
                {
                    return Err(BridgeError::invalid("Invalid capture dimensions"));
                }
            }
            let observation_cursor = p
                .capture
                .as_ref()
                .map(|c| {
                    let id =
                        screen::display(&session, c.display_id.as_deref().unwrap_or("primary"))?;
                    let sequence = session
                        .read_frame(id, None)
                        .map(|f| f.frame.stamp.sequence)
                        .unwrap_or(0);
                    Ok::<_, BridgeError>((id, sequence))
                })
                .transpose()?;
            let progress = input::send(permit.clone(), p.actions, p.snapshot_id, async {
                tokio::select! {_=cancel.cancelled()=>{},_=client.cancel.cancelled()=>{}}
            })
            .await?;
            let mut reply = Reply::success(
                json!({"session_ref":session.control().view().session_ref,"delivery":progress.delivery,"completed_actions":progress.completed_actions,"sent_events":progress.sent_events,"held_keys":progress.held_keys,"held_buttons":progress.held_buttons}),
            );
            if let Some(error) = progress.error {
                reply.value["ok"] = json!(false);
                reply.value["status"] = json!(if progress.sent_events > 0 {
                    "partial"
                } else {
                    "failed"
                });
                reply.value["error"] = json!(error);
                reply.value["data"]["failed_action_index"] = json!(progress.failed_action_index);
            }
            if let Some(c) = p.capture {
                let result = async {
                    let (id, sequence) = observation_cursor.ok_or_else(|| {
                        BridgeError::new("INTERNAL_ERROR", "Missing observation cursor")
                    })?;
                    // Keep the pre-input cursor so results received during sending or
                    // the delay remain eligible. The surrounding select cancels this delay.
                    if let Some(delay) = c.delay_ms.filter(|ms| *ms > 0) {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    let after = Some(sequence);
                    screen::capture(
                        &permit,
                        &id.to_string(),
                        after,
                        c.wait_ms.unwrap_or(1000),
                        c.max_width.unwrap_or(1600),
                        c.max_height.unwrap_or(1600),
                    )
                    .await
                };
                match tokio::select! {biased; _=cancel.cancelled()=>Err(BridgeError::new("CANCELLED","Observation cancelled after input")),_=client.cancel.cancelled()=>Err(BridgeError::new("BINDING_EXPIRED","Agent disconnected after input")),result=result=>result}
                {
                    Ok(o) => {
                        reply.png = o.png;
                        reply.value["data"]["observation"] = json!({"kind":"screen","status":if o.unchanged{"unchanged"}else{"completed"},"data":o.data});
                    }
                    Err(e) => {
                        reply.value["data"]["observation"] =
                            json!({"kind":"screen","status":"failed","error":e})
                    }
                }
            }
            Ok(reply)
        }
        "rd_session_authenticate" => {
            let p: Authenticate = parse(args)?;
            let ms = wait(p.wait_ms, 10000)?;
            let (session, permit) = api::resolve(agent, &p.session_ref, true)?;
            let credentials = match p.credentials {
                Credentials::Password { password } => auth::Credentials::Password(password),
                Credentials::TwoFactor { code } => auth::Credentials::TwoFactor(code),
                Credentials::OsLogin { username, password } => {
                    auth::Credentials::OsLogin(username, password)
                }
            };
            let challenge = p.challenge_id.clone();
            auth::submit(permit.clone(), p.challenge_id, credentials).await?;
            let changed = api::wait_until(&session, &permit, ms, |s| {
                s.authenticated
                    || matches!(
                        s.state,
                        ConnectionState::Disconnected
                            | ConnectionState::Closed
                            | ConnectionState::AwaitingHuman
                    )
                    || s.auth_challenge.as_ref().is_some_and(|c| c.id != challenge)
            })
            .await?;
            let snapshot = session.snapshot();
            if changed
                && !snapshot.authenticated
                && snapshot.state == ConnectionState::AwaitingAuth
                && snapshot.last_error.is_some()
            {
                let mut reply = Reply::error(BridgeError::new(
                    "AUTH_FAILED",
                    "Remote authentication failed; inspect the current challenge",
                ));
                reply.value["data"] =
                    json!({"session":api::view(&session,false),"delivery":"sent"});
                return Ok(reply);
            }
            Ok(
                Reply::success(json!({"session":api::view(&session,false),"delivery":"sent"}))
                    .status(if changed { "completed" } else { "pending" }),
            )
        }
        "rd_session_disconnect" => {
            let p: WaitWrite = parse(args)?;
            let ms = wait(p.wait_ms, 10000)?;
            let (session, permit) = api::resolve(agent, &p.session_ref, true)?;
            api::disconnect(permit.clone())?;
            let done = api::wait_until(&session, &permit, ms, |s| matches!(s.state, ConnectionState::Disconnected | ConnectionState::Closed)).await?;
            Ok(Reply::success(json!({"session":api::view(&session,false)})).status(if done { "completed" } else { "pending" }))
        }
        "rd_session_reconnect" => {
            let p: Reconnect = parse(args)?;
            let ms = wait(p.wait_ms, 10000)?;
            let (session, permit) = api::resolve(agent, &p.session_ref, true)?;
            let epoch = session.snapshot().connection_epoch;
            // End the old IO loop before starting a new one: both loops share login state.
            if session.snapshot().state == ConnectionState::Ready {
                session.control().prepare_reconnect(&permit)?;
                api::disconnect(permit.clone())?;
                if !api::wait_until(&session, &permit, 10000, |s| s.state == ConnectionState::Disconnected).await? {
                    return Err(BridgeError::new("NOT_READY", "Previous connection has not disconnected yet"));
                }
            }
            let reference = current(&session)?;
            let (_, permit) = api::resolve(agent, &reference, true)?;
            api::reconnect(&permit, p.force_relay.unwrap_or(false))?;
            let _ = api::wait_until(&session, &permit, ms, |s| {
                s.connection_epoch > epoch
                    && matches!(
                        s.state,
                        ConnectionState::Ready
                            | ConnectionState::AwaitingAuth
                            | ConnectionState::AwaitingHuman
                            | ConnectionState::Disconnected
                            | ConnectionState::Closed
                    )
            })
            .await?;
            Ok(
                Reply::success(json!({"session":api::view(&session,false)})).status(
                    if session.snapshot().state == ConnectionState::Ready {
                        "completed"
                    } else {
                        "pending"
                    },
                ),
            )
        }
        "rd_session_close" => {
            let p: WaitWrite = parse(args)?;
            let ms = wait(p.wait_ms, 10000)?;
            let (session, permit) = api::resolve(agent, &p.session_ref, true)?;
            gui::close(permit.clone())?;
            let done = api::wait_until(&session, &permit, ms, |s| s.state == ConnectionState::Closed && s.ui_session_ids.is_empty()).await?;
            Ok(Reply::success(json!({"session_id":session.snapshot().session_id,"closed":done,"remaining_views":session.snapshot().ui_session_ids.len(),"session":api::view(&session,false)})).status(if done { "completed" } else { "pending" }))
        }
        "rd_terminal_list" => {
            let p: Read = parse(args)?;
            let (session, permit) = api::resolve(agent, &p.session_ref, false)?;
            api::terminal_session(&session)?;
            let terminals = terminals::list(&session.snapshot().session_id)
                .into_iter()
                .filter(|t| terminals::authorize(&permit, &t.terminal_id).is_ok())
                .collect::<Vec<_>>();
            Ok(Reply::success(
                json!({"session_ref":current(&session)?,"terminals":terminals}),
            ))
        }
        "rd_terminal_create" => {
            let p: TerminalCreate = parse(args)?;
            let ms = wait(p.wait_ms, 10000)?;
            let (session, permit) = api::resolve(agent, &p.session_ref, true)?;
            api::terminal_session(&session)?;
            let terminal =
                terminals::create(permit.clone(), p.rows.unwrap_or(24), p.cols.unwrap_or(80))?;
            let mut intent =
                terminals::IntentGuard::new(&session.snapshot().session_id, &terminal.terminal_id);
            let terminal = terminals::wait_open(&permit, &terminal.terminal_id, ms).await?;
            intent.finish();
            Ok(
                Reply::success(json!({"session_ref":current(&session)?,"terminal":terminal}))
                    .status(if terminal.state == "opening" {
                        "pending"
                    } else {
                        "completed"
                    }),
            )
        }
        "rd_terminal_read" => {
            let p: TerminalRead = parse(args)?;
            let (session, permit) = api::resolve(agent, &p.session_ref, false)?;
            api::terminal_session(&session)?;
            let (output, unchanged) = terminals::wait_read(
                &permit,
                &p.terminal_id,
                p.cursor.as_deref(),
                p.max_bytes.unwrap_or(16384),
                p.format.as_ref().map(OutputFormat::name).unwrap_or("text"),
                wait(p.wait_ms, 0)?,
            )
            .await?;
            let mut data = serde_json::to_value(output).map_err(|_| {
                BridgeError::new("INTERNAL_ERROR", "Could not encode terminal output")
            })?;
            data["session_ref"] = json!(current(&session)?);
            Ok(Reply::success(data).status(if unchanged { "unchanged" } else { "completed" }))
        }
        "rd_terminal_write" => {
            let p: TerminalWrite = parse(args)?;
            let (session, permit) = api::resolve(agent, &p.session_ref, true)?;
            api::terminal_session(&session)?;
            if p.text.len() > 16384 {
                return Err(BridgeError::invalid("Terminal text exceeds 16 KiB"));
            }
            if let Some(r) = &p.read {
                wait(r.wait_ms, 1000)?;
                if !(1..=65536).contains(&r.max_bytes.unwrap_or(16384)) {
                    return Err(BridgeError::invalid("Invalid read max_bytes"));
                }
            }
            terminals::ready(&permit, &p.terminal_id)?;
            let cursor = if let Some(read) = &p.read {
                Some(read.cursor.clone().unwrap_or(terminals::end_cursor(
                    &session.snapshot().session_id,
                    &p.terminal_id,
                )?))
            } else {
                None
            };
            let bytes = p.text.len();
            let sent = tokio::select! {biased; _=cancel.cancelled()=>Err(BridgeError::new("CANCELLED","Terminal write cancelled; in-flight delivery is unknown")),_=client.cancel.cancelled()=>Err(BridgeError::new("BINDING_EXPIRED","Agent disconnected during terminal write")),result=terminals::write(permit.clone(),&p.terminal_id,p.text)=>result };
            if let Err(error) = sent {
                let delivery = if matches!(
                    error.code,
                    "CANCELLED" | "BINDING_EXPIRED" | "DELIVERY_UNKNOWN"
                ) {
                    "unknown"
                } else {
                    "not_sent"
                };
                let mut reply = Reply::error(error);
                reply.value["data"] = json!({"session_ref":session.control().view().session_ref,"terminal_id":p.terminal_id,"bytes_sent":0,"delivery":delivery});
                return Ok(reply);
            }
            let mut reply = Reply::success(
                json!({"session_ref":session.control().view().session_ref,"terminal_id":p.terminal_id,"bytes_sent":bytes,"delivery":"sent"}),
            );
            if let Some(read) = p.read {
                let read_future = terminals::wait_read(
                    &permit,
                    &p.terminal_id,
                    cursor.as_deref(),
                    read.max_bytes.unwrap_or(16384),
                    read.format
                        .as_ref()
                        .map(OutputFormat::name)
                        .unwrap_or("text"),
                    read.wait_ms.unwrap_or(1000),
                );
                let result = tokio::select! {biased;_=cancel.cancelled()=>Err(BridgeError::new("CANCELLED","Observation cancelled after terminal input")),_=client.cancel.cancelled()=>Err(BridgeError::new("BINDING_EXPIRED","Agent disconnected after terminal input")),result=read_future=>result};
                reply.value["data"]["observation"] = match result {
                    Ok((output, unchanged)) => {
                        json!({"kind":"terminal","status":if unchanged{"unchanged"}else{"completed"},"data":output})
                    }
                    Err(error) => json!({"kind":"terminal","status":"failed","error":error}),
                };
            }
            Ok(reply)
        }
        "rd_terminal_resize" => {
            let p: TerminalResize = parse(args)?;
            terminals::validate_size(p.rows, p.cols)?;
            let (session, permit) = api::resolve(agent, &p.session_ref, true)?;
            api::terminal_session(&session)?;
            terminals::ready(&permit, &p.terminal_id)?;
            terminals::resize(permit, &p.terminal_id, p.rows, p.cols).await?;
            Ok(Reply::success(
                json!({"terminal_id":p.terminal_id,"requested_size":{"rows":p.rows,"cols":p.cols},"delivery":"sent"}),
            ))
        }
        "rd_terminal_close" => {
            let p: TerminalClose = parse(args)?;
            let (session, permit) = api::resolve(agent, &p.session_ref, true)?;
            api::terminal_session(&session)?;
            terminals::ready(&permit, &p.terminal_id)?;
            terminals::close(permit.clone(), &p.terminal_id).await?;
            let (output, _) = terminals::wait_read(
                &permit,
                &p.terminal_id,
                Some(&terminals::end_cursor(
                    &session.snapshot().session_id,
                    &p.terminal_id,
                )?),
                1,
                "base64",
                1000,
            )
            .await?;
            Ok(Reply::success(json!({"terminal":output.terminal})).status(
                if output.terminal.state == "closed" {
                    "completed"
                } else {
                    "pending"
                },
            ))
        }
        _ => Err(BridgeError::invalid("Unknown tool")),
    }
}
use sha2::Digest;

pub(super) fn preflight(client: &Client, name: &str, args: &Map<String, Value>) -> Result<()> {
    macro_rules! shape {
        ($ty:ty) => {{
            let _: $ty = parse(args)?;
        }};
    }
    match name {
        "rd_session_open" => shape!(Open),
        "rd_session_attach" => shape!(Attach),
        "rd_session_get" => shape!(Get),
        "rd_session_list" => shape!(List),
        "rd_capabilities_get" => shape!(Read),
        "rd_operation_get" => shape!(OperationGet),
        "rd_session_detach" | "rd_control_release" => shape!(Write),
        "rd_session_disconnect" | "rd_session_close" => shape!(WaitWrite),
        "rd_session_authenticate" => shape!(Authenticate),
        "rd_session_reconnect" => shape!(Reconnect),
        "rd_control_request" => shape!(ControlRequest),
        "rd_control_cancel" => shape!(ControlCancel),
        "rd_screen_capture" => shape!(Capture),
        "rd_input_send" => shape!(Input),
        "rd_terminal_list" => shape!(Read),
        "rd_terminal_create" => shape!(TerminalCreate),
        "rd_terminal_read" => shape!(TerminalRead),
        "rd_terminal_write" => shape!(TerminalWrite),
        "rd_terminal_resize" => shape!(TerminalResize),
        "rd_terminal_close" => shape!(TerminalClose),
        _ => return Err(BridgeError::invalid("Unknown tool")),
    }
    if let Some(ms) = args.get("wait_ms").and_then(Value::as_u64) {
        api::wait_budget(ms)?;
    }
    if let Some(reference) = args.get("session_ref").and_then(Value::as_str) {
        let read = matches!(
            name,
            "rd_session_get"
                | "rd_capabilities_get"
                | "rd_screen_capture"
                | "rd_terminal_list"
                | "rd_terminal_read"
                | "rd_control_cancel"
                | "rd_session_detach"
        );
        let (_, permit) = api::resolve(&client.agent, reference, !read)?;
        if !read && !matches!(name, "rd_control_request" | "rd_control_release") {
            permit.check()?;
        }
        if name == "rd_input_send" {
            let p: Input = parse(args)?;
            input::validate(&permit, &p.actions, p.snapshot_id.as_deref())?;
        }
    }
    Ok(())
}

fn output_schema(name: &str) -> Map<String, Value> {
    let fields: &[(&str, &str)] = match name {
        "rd_capabilities_get" => &[("session_ref", "string"), ("peer", "object"), ("capabilities", "object"), ("contract", "object")],
        "rd_operation_get" => &[("operation", "object")],
        "rd_session_list" => &[("sessions", "array"), ("next_cursor", "string|null")],
        "rd_session_open" => &[
            ("created", "boolean"),
            ("session", "object"),
            ("initial_terminal_id", "string|null"),
            ("password_applied", "boolean"),
        ],
        "rd_session_attach" | "rd_session_disconnect" | "rd_session_reconnect" => {
            &[("session", "object")]
        }
        "rd_session_get" => &[("session", "object"), ("operation", "object")],
        "rd_session_authenticate" => &[("session", "object"), ("delivery", "string")],
        "rd_session_detach" => &[("session_id", "string"), ("detached", "boolean")],
        "rd_session_close" => &[
            ("session_id", "string"),
            ("closed", "boolean"),
            ("remaining_views", "integer"),
            ("session", "object"),
        ],
        "rd_control_request" => &[
            ("session_ref", "string"),
            ("control", "object"),
            ("approval", "object|null"),
        ],
        "rd_control_cancel" => &[("approval", "object")],
        "rd_control_release" => &[
            ("session_ref", "string"),
            ("control", "object"),
            ("released_inputs", "object|null"),
        ],
        "rd_screen_capture" => &[
            ("session_ref", "string"),
            ("frame", "object"),
            ("image_content_index", "integer"),
        ],
        "rd_input_send" => &[
            ("session_ref", "string"),
            ("delivery", "string"),
            ("completed_actions", "integer"),
            ("sent_events", "integer"),
            ("failed_action_index", "integer"),
            ("held_keys", "array"),
            ("held_buttons", "array"),
            ("observation", "object"),
        ],
        "rd_terminal_list" => &[("session_ref", "string"), ("terminals", "array")],
        "rd_terminal_create" => &[("session_ref", "string"), ("terminal", "object")],
        "rd_terminal_close" => &[("terminal", "object")],
        "rd_terminal_resize" => &[
            ("terminal_id", "string"),
            ("requested_size", "object"),
            ("delivery", "string"),
        ],
        "rd_terminal_write" => &[
            ("session_ref", "string"),
            ("terminal_id", "string"),
            ("bytes_sent", "integer"),
            ("delivery", "string"),
            ("observation", "object"),
        ],
        "rd_terminal_read" => &[
            ("session_ref", "string"),
            ("terminal", "object"),
            ("stream_epoch", "string"),
            ("start_offset", "string"),
            ("end_offset", "string"),
            ("next_cursor", "string"),
            ("oldest_cursor", "string"),
            ("has_more", "boolean"),
            ("text", "string"),
            ("text_lossy", "boolean"),
            ("data_base64", "string"),
        ],
        _ => &[],
    };
    let properties = fields
        .iter()
        .map(|(key, kind)| {
            let mut property = Map::new();
            property.insert(
                "type".into(),
                if kind.contains('|') {
                    json!(kind.split('|').collect::<Vec<_>>())
                } else {
                    json!(kind)
                },
            );
            ((*key).into(), Value::Object(property))
        })
        .collect::<Map<_, _>>();
    let schema = json!({"type":"object","required":["ok","status","data"],"additionalProperties":false,"properties":{"ok":{"type":"boolean"},"status":{"enum":["completed","pending","unchanged","partial","failed"]},"data":{"type":"object","additionalProperties":false,"properties":properties},"error":{"type":"object","required":["code","message","retry"],"properties":{"code":{"type":"string"},"message":{"type":"string"},"retry":{"enum":["never","after_state_change","same_operation"]},"details":{}}},"operation":{"type":"object"}}});
    match schema {
        Value::Object(object) => object,
        _ => Map::new(),
    }
}
