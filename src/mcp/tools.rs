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
    definition::<Read>("rd_displays_get","Read remote display topology, original dimensions, known modes, AI capture selection, local view selection and stock virtual-display support. Display IDs are valid only for the reported layout_revision; driver_installed is unknown because the stock protocol does not report it.",true),
    definition::<DisplayModes>("rd_display_modes_get","Read cached supported modes for one online display without changing it. If known=false, switch from another display to this display with target=local_view; the stock peer reports modes only when the selected display changes. Reconnect if no other display exists. Current geometry is in capture pixels; modes are the stock OS resolution values and scale is reported separately.",true),
    definition::<ViewGet>("rd_view_settings_get","Read actual settings and local-window state from one desktop GUI view, including local screens, scaling, cursor preferences, toolbar pin and fullscreen. Multiple views require ui_session_id from displays_get. Does not change remote resolution or MCP image size.",true),
    definition::<ViewSet>("rd_view_settings_set","Set one explicit local-view setting under AI control. Scaling/cursor/display-window preferences persist per peer; use_all_local_displays applies on the next fresh connection; individual_windows changes subsequent toolbar selections without creating/closing windows. follow_ai_display and toolbar pin persist globally; fullscreen affects the whole local OS window, including other tabs. Custom scale percent 5..1000. Read back settings and effective/support fields.",false),
    definition::<ViewWindow>("rd_view_window","Show the target local OS window, close only its desktop view, or open_display using the stock monitor-window path (may reuse an existing tab/window). Closing the last view disconnects the logical session; session_close instead closes every view. Show/fullscreen affect the OS window shared by tabs. Window open/close returns sent with unknown completion; use displays_get to observe topology. Requires AI control.",false),
    definition::<DisplaySelect>("rd_display_select","Select numeric display_id or all for target=capture (this binding's AI capture subscription) or local_view (one GUI window). Effective capture is the union of AI and GUI needs; subsequent screen_capture calls add their requested display. With multiple GUI views specify ui_session_id from displays_get. Local viewing is independent of the follow-AI preference. Selection invalidates old screenshot coordinates. wait_ms observes local GUI application, minimum 1000/default 10000; capture has no remote acknowledgement. Selecting one local display requests its supported modes without applying saved resolution preferences.",false),
    definition::<DisplayResolutionSet>("rd_display_resolution_set","Request mode=set with width/height, restore_original, or fit_local (exact local main-display size). Setting physical screens requires a reported supported mode; restore_original uses the original resolution reported by the peer even when the mode cache is unknown; custom sizes require a virtual display reported with original 0x0. Requires keyboard permission and AI control. This changes the remote machine's display; wait_ms default 10000 observes geometry, with confirmed=false meaning unknown outcome. Does not save a new peer resolution preference.",false),
    definition::<VirtualDisplaySet>("rd_virtual_display_set","Add, remove, or remove_all stock Windows virtual displays. Requires installed peer reporting an IDD implementation, keyboard permission, AI control and privacy mode off. Adding MAY INSTALL A DRIVER through the stock peer. RustDesk IDD add/remove requires index 1..4; Amyuni adds/removes one and requires index omitted; remove_all always omits index. wait_ms default 10000 observes reported counts/indices; false confirmation is unknown outcome, not proof of failure.",false),
    definition::<Read>("rd_clipboard_settings_get","Read text clipboard synchronization preference, effective state, and permission for a desktop session. Does not read the local clipboard.",true),
    definition::<ClipboardSet>("rd_clipboard_settings_set","Set text synchronization enabled explicitly. Persists the peer preference and sends the option to the connected peer; requires AI control. Disabling does not clear clipboard text.",false),
    definition::<ClipboardRead>("rd_clipboard_read","Read the latest text received from this binding's remote clipboard synchronization, optionally waiting for a changed revision. Unknown means no observed text; this is not an active remote clipboard pull. Cache expires after five minutes and across binding/connection changes.",true),
    definition::<ClipboardWrite>("rd_clipboard_write","Send up to 1 MiB of UTF-8 text to the remote clipboard without changing the local clipboard. Requires enabled synchronization and AI control. Optional paste sends Ctrl+V (Command+V on macOS) after delay_ms (default 200, max 30000); neither sending nor delay proves the application pasted successfully.",false),
    definition::<ClipboardType>("rd_clipboard_type","Type up to 16 KiB of explicit text, or if omitted read the local system clipboard and type its text, using the existing input path. This corresponds to Send clipboard keystrokes; it does not set the remote clipboard. Windows types one character every 10 ms; total input pacing/waits must fit 30 seconds, otherwise split the text or use clipboard paste. Requires keyboard permission and AI control.",false),
    definition::<FileList>("rd_file_list","List a local or remote directory through a visible file_transfer session. Returns a directory job with up to 1000 entries; use file_job_get next_offset for further pages. Empty remote path requests the remote home directory. One directory request per session at a time; timeout does not mean an empty directory.",true),
    definition::<Write>("rd_file_clipboard_cancel","Request cancellation of this binding's native local file paste. Completed files may remain; file_clipboard_get reports its final state. Does not cancel a remote application's Paste operation.",false),
    definition::<Read>("rd_file_clipboard_get","Read file copy-paste settings, platform support and whether a recent remote file offer was received for this binding. Native file clipboard is shared by local applications and enabled desktop sessions.",true),
    definition::<ClipboardSet>("rd_file_clipboard_set","Set explicit enabled for file copy-paste in the peer preference. Requires file and keyboard permissions and AI control; independent of text clipboard synchronization.",false),
    definition::<FileClipboardCopy>("rd_file_clipboard_copy","With paths, replace the local SYSTEM clipboard with 1..128 existing absolute file/directory paths; native synchronization publishes them to enabled sessions. Without paths, send Copy to the remote focused selection; select this local desktop tab first. File-manager selection determines files; consult file_clipboard_get for a received remote offer. Copy delivery does not prove a file was selected.",false),
    definition::<FileClipboardPaste>("rd_file_clipboard_paste","Paste native clipboard files. With local_directory, request native download of recently copied remote files into an existing absolute local directory (macOS only); existing names are disambiguated by the native paste implementation. Without it, send Paste to remote focus after delay_ms (default 500). wait_ms observes local paste only, default 10000. Clipboard is global; remote application paste result is unknown until independently checked.",false),
    definition::<FileJobWrite>("rd_file_job_resume","Explicitly resume a retained interrupted, failed, or cancelled transfer after restoring the file connection and AI control. Creates a new job in the same binding; original remains unchanged. Stock size/mtime digest and surviving partial files determine byte-offset resume; otherwise files restart or follow conflict policy. This is not content-hash validation. Jobs expire after 300 seconds and do not survive controller restart. Cancel may already have removed partial files.",false),
    definition::<FileManage>("rd_file_manage","Create directories, rename within the same parent, remove files, or remove directories locally or remotely. Requires absolute non-root paths and AI control in a file_transfer session. recursive defaults false; true deletes all descendants including hidden files, without following listed symlinks. Deletion is permanent; cancellation cannot undo completed steps. new_name is a single name, and rename may replace an existing destination according to the OS. Returns a tracked job; inspect its final state.",false),
    definition::<FileTransfer>("rd_file_transfer","Upload or download 1..32 files/directories using the stock file-transfer protocol. destination_path is the complete target path, not just its parent. Returns tracked jobs, not a claim of completion. Default conflict policy is ask; use file_conflict_resolve. Partial files can remain after cancellation. Requires AI control and a file_transfer session.",false),
    definition::<Read>("rd_file_jobs","List this binding's retained directory and transfer jobs without changing them.",true),
    definition::<FileJobGet>("rd_file_job_get","Read one file job, optionally waiting for completion or a revision change. Page completed directory entries using offset and limit. Job completion is based on protocol events; interrupted/cancelled transfers may leave partial files.",true),
    definition::<FileJobWrite>("rd_file_job_cancel","Cancel one tracked file job under AI control. Cancels the local transfer and requests remote cleanup, without deleting partial destination files.",false),
    definition::<FileConflict>("rd_file_conflict_resolve","Resolve a pending same-name conflict with explicit overwrite=true or false (skip). Optionally apply to all remaining conflicts in this job.",false),
    definition::<Read>("rd_capabilities_get","Inspect implemented MCP capabilities for this session, including peer version, permission, control and readiness blockers. Unknown support is not permission. Does not take control.",true),
    definition::<OperationGet>("rd_operation_get","Query this MCP client's retained operation_id without replaying a write. Optional wait_ms waits for local execution, not remote application completion. Original pending results have unknown final outcome; inspect current session or terminal state. Records expire five minutes after execution and when this MCP client ends.",true),
    definition::<List>("rd_session_list","Discover visible desktop, terminal and file-transfer sessions. Multiple MCP agents may connect; ownership is exclusive per core session.",true),
    definition::<Open>("rd_session_open","Open a visible GUI session or reuse and attach to an existing one. Only newly created sessions start under AI control. Optional password is single-use connection authentication, never OS login. kind supports desktop, terminal and file_transfer. Optional from_session_ref reuses an authenticated same-peer connection token when available; unsupported token reuse still requires authentication.",false),
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
        "rd_view_settings_get" | "rd_view_settings_set" | "rd_view_window" => {
            use crate::automation::views::{self, Command};
            let (reference, view, command, ms) = match name {
                "rd_view_settings_get" => {let p:ViewGet=parse(args)?;(p.session_ref,p.ui_session_id,Command::Get,p.wait_ms)},
                "rd_view_settings_set" => {let p:ViewSet=parse(args)?;(p.session_ref,p.ui_session_id,Command::Set{change:p.change},p.wait_ms)},
                _ => {let p:ViewWindow=parse(args)?;(p.session_ref,p.ui_session_id,Command::Window{action:p.action},p.wait_ms)},
            };
            let (_,permit)=api::resolve(agent,&reference,name!="rd_view_settings_get")?;
            let value=views::request(permit,view,command,wait(ms,10000)?).await?;
            let confirmed=value["confirmed"]==true;
            Ok(Reply::success(value).status(if confirmed {"completed"}else{"pending"}))
        }
        "rd_displays_get" => {
            let p:Read=parse(args)?;let (_,permit)=api::resolve(agent,&p.session_ref,false)?;
            Ok(Reply::success(crate::automation::displays::get(&permit)?))
        }
        "rd_display_modes_get" => {
            let p:DisplayModes=parse(args)?;let (_,permit)=api::resolve(agent,&p.session_ref,false)?;
            Ok(Reply::success(crate::automation::displays::modes(&permit,&p.display_id)?))
        }
        "rd_display_select" => {
            let p:DisplaySelect=parse(args)?;let (_,permit)=api::resolve(agent,&p.session_ref,true)?;
            let value=crate::automation::displays::select(permit,&p.display_id,p.target,p.ui_session_id,wait(p.wait_ms,10000)?).await?;
            let confirmed=value["confirmed"]==true;Ok(Reply::success(value).status(if confirmed {"completed"}else{"pending"}))
        }
        "rd_display_resolution_set" => {
            let p:DisplayResolutionSet=parse(args)?;let (_,permit)=api::resolve(agent,&p.session_ref,true)?;
            let value=crate::automation::displays::resolution(permit,&p.display_id,p.mode,p.width,p.height,wait(p.wait_ms,10000)?).await?;
            let confirmed=value["confirmed"]==true;Ok(Reply::success(value).status(if confirmed {"completed"}else{"pending"}))
        }
        "rd_virtual_display_set" => {
            let p:VirtualDisplaySet=parse(args)?;let (_,permit)=api::resolve(agent,&p.session_ref,true)?;
            let value=crate::automation::displays::virtual_change(permit,p.action,p.index,wait(p.wait_ms,10000)?).await?;
            let confirmed=value["confirmed"]==true;Ok(Reply::success(value).status(if confirmed {"completed"}else{"pending"}))
        }
        "rd_clipboard_settings_get" => {
            let p: Read = parse(args)?;
            let (_, permit) = api::resolve(agent, &p.session_ref, false)?;
            Ok(Reply::success(
                json!({"settings":crate::automation::text_clipboard::settings(&permit)?}),
            ))
        }
        "rd_clipboard_settings_set" => {
            let p: ClipboardSet = parse(args)?;
            let (_, permit) = api::resolve(agent, &p.session_ref, true)?;
            Ok(Reply::success(
                json!({"settings":crate::automation::text_clipboard::set_enabled(permit,p.enabled).await?}),
            ))
        }
        "rd_clipboard_read" => {
            let p: ClipboardRead = parse(args)?;
            let (_, permit) = api::resolve(agent, &p.session_ref, false)?;
            Ok(Reply::success(
                json!({"clipboard":crate::automation::text_clipboard::read(&permit,p.after_revision,wait(p.wait_ms,0)?).await?}),
            ))
        }
        "rd_clipboard_write" => {
            let p: ClipboardWrite = parse(args)?;
            let (session, permit) = api::resolve(agent, &p.session_ref, true)?;
            let delay = wait(p.delay_ms, 200)?;
            let mut actions = vec![];
            if p.paste.unwrap_or(false) {
                actions = vec![
                    input::Action::Wait { duration_ms: delay },
                    input::Action::Shortcut {
                        modifiers: vec![
                            if session.snapshot().platform.as_deref() == Some("Mac OS") {
                                input::Modifier::Meta
                            } else {
                                input::Modifier::Control
                            },
                        ],
                        key: input::KeyName::KeyV,
                    },
                ];
                input::validate(&permit, &actions, None)?;
            }
            crate::automation::text_clipboard::write(permit.clone(), p.text).await?;
            let paste = if actions.is_empty() {
                Value::Null
            } else {
                let progress = input::send(permit, actions, None, async {
                    tokio::select! {_=cancel.cancelled()=>{},_=client.cancel.cancelled()=>{}}
                })
                .await;
                match progress {
                    Ok(p) => {
                        json!({"delivery":p.delivery,"sent_events":p.sent_events,"error":p.error})
                    }
                    Err(e) => json!({"error":e}),
                }
            };
            let failed = !paste.is_null() && !paste["error"].is_null();
            let mut reply = Reply::success(json!({"delivery":"sent","paste":paste}));
            if failed {
                reply.value["ok"] = json!(false);
                reply.value["error"] = paste["error"].clone();
                reply = reply.status("partial");
            }
            Ok(reply)
        }
        "rd_clipboard_type" => {
            let p: ClipboardType = parse(args)?;
            let (session, permit) = api::resolve(agent, &p.session_ref, true)?;
            if session.snapshot().kind != SessionKind::Desktop {
                return Err(BridgeError::new(
                    "WRONG_SESSION_KIND",
                    "Typing requires a desktop session",
                ));
            }
            permit.check()?;
            let text = match p.text {
                Some(t) => t,
                None => crate::automation::text_clipboard::local_text().await?,
            };
            let progress = input::send(permit, vec![input::Action::Text { text }], None, async {
                tokio::select! {_=cancel.cancelled()=>{},_=client.cancel.cancelled()=>{}}
            })
            .await?;
            let mut reply = Reply::success(
                json!({"delivery":progress.delivery,"sent_events":progress.sent_events}),
            );
            if let Some(error) = progress.error {
                reply.value["ok"] = json!(false);
                reply.value["error"] = json!(error);
                reply = reply.status(if progress.sent_events > 0 {
                    "partial"
                } else {
                    "failed"
                });
            }
            Ok(reply)
        }
        "rd_file_list" => {
            let p: FileList = parse(args)?;
            let ms = wait(p.wait_ms, 10000)?;
            let (_, permit) = api::resolve(agent, &p.session_ref, false)?;
            let id = crate::automation::files::directory(
                permit.clone(),
                p.path,
                matches!(p.location, FileLocation::Local),
                p.include_hidden.unwrap_or(false),
            )
            .await?;
            let job = crate::automation::files::get(&permit, &id, None, ms, 0, 1000).await?;
            Ok(Reply::success(json!({"job":job})))
        }
        "rd_file_clipboard_cancel" => {
            let p: Write = parse(args)?;
            let (_, permit) = api::resolve(agent, &p.session_ref, true)?;
            Ok(Reply::success(crate::automation::file_clipboard::cancel_local(&permit)?))
        }
        "rd_file_clipboard_get" => {
            let p: Read = parse(args)?;
            let (_, permit) = api::resolve(agent, &p.session_ref, false)?;
            Ok(Reply::success(json!({"settings":crate::automation::file_clipboard::settings(&permit)?})))
        }
        "rd_file_clipboard_set" => {
            let p: ClipboardSet = parse(args)?;
            let (_, permit) = api::resolve(agent, &p.session_ref, true)?;
            Ok(Reply::success(json!({"settings":crate::automation::file_clipboard::set_enabled(permit,p.enabled).await?})))
        }
        "rd_file_clipboard_copy" => {
            let p: FileClipboardCopy = parse(args)?;
            let (_, permit) = api::resolve(agent, &p.session_ref, true)?;
            Ok(Reply::success(crate::automation::file_clipboard::copy(permit,p.paths).await?))
        }
        "rd_file_clipboard_paste" => {
            let p: FileClipboardPaste = parse(args)?;
            let (_, permit) = api::resolve(agent, &p.session_ref, true)?;
            let ms = wait(p.wait_ms,10000)?;
            let delay = wait(p.delay_ms,500)?;
            let result = if let Some(path) = p.local_directory { crate::automation::file_clipboard::paste_local(permit,path,ms).await? }
                else { crate::automation::file_clipboard::paste_remote(permit,delay).await? };
            Ok(Reply::success(result))
        }
        "rd_file_job_resume" => {
            let p: FileJobWrite = parse(args)?;
            let (_, permit) = api::resolve(agent, &p.session_ref, true)?;
            let id = crate::automation::files::resume(permit.clone(), &p.job_id).await?;
            Ok(Reply::success(json!({"job":crate::automation::files::get(&permit,&id,None,0,0,1).await?})))
        }
        "rd_file_manage" => {
            let p: FileManage = parse(args)?;
            let ms = wait(p.wait_ms, 0)?;
            let (_, permit) = api::resolve(agent, &p.session_ref, true)?;
            let id = crate::automation::files::manage(permit.clone(), p.action, p.path, matches!(p.location, FileLocation::Local), p.recursive.unwrap_or(false), p.new_name)?;
            Ok(Reply::success(json!({"job":crate::automation::files::get(&permit,&id,None,ms,0,1).await?})))
        }
        "rd_file_transfer" => {
            let p: FileTransfer = parse(args)?;
            let (_, permit) = api::resolve(agent, &p.session_ref, true)?;
            if p.items.is_empty() || p.items.len() > 32 {
                return Err(BridgeError::invalid("items must contain 1..32 paths"));
            }
            for item in &p.items {
                crate::automation::files::validate_path(&item.source_path, false)?;
                crate::automation::files::validate_path(&item.destination_path, false)?;
            }
            let policy = match p.conflict {
                Some(ConflictPolicy::Overwrite) => "overwrite",
                Some(ConflictPolicy::Skip) => "skip",
                _ => "ask",
            };
            let mut jobs = Vec::new();
            for item in p.items {
                match crate::automation::files::transfer(
                    permit.clone(),
                    item.source_path,
                    item.destination_path,
                    matches!(p.direction, TransferDirection::Download),
                    p.include_hidden.unwrap_or(false),
                    policy,
                )
                .await
                {
                    Ok(id) => {
                        jobs.push(crate::automation::files::get(&permit, &id, None, 0, 0, 1).await?)
                    }
                    Err(error) => {
                        let mut reply = Reply::error(error);
                        reply.value["data"] = json!({"jobs":jobs});
                        if !jobs.is_empty() {
                            reply = reply.status("partial");
                        }
                        return Ok(reply);
                    }
                }
            }
            Ok(Reply::success(json!({"jobs":jobs})))
        }
        "rd_file_jobs" => {
            let p: Read = parse(args)?;
            let (_, permit) = api::resolve(agent, &p.session_ref, false)?;
            Ok(Reply::success(
                json!({"jobs":crate::automation::files::list(&permit)?}),
            ))
        }
        "rd_file_job_get" => {
            let p: FileJobGet = parse(args)?;
            let (_, permit) = api::resolve(agent, &p.session_ref, false)?;
            Ok(Reply::success(
                json!({"job":crate::automation::files::get(&permit,&p.job_id,p.after_revision,wait(p.wait_ms,0)?,p.offset.unwrap_or(0),p.limit.unwrap_or(1000)).await?}),
            ))
        }
        "rd_file_job_cancel" => {
            let p: FileJobWrite = parse(args)?;
            let (_, permit) = api::resolve(agent, &p.session_ref, true)?;
            crate::automation::files::cancel(permit.clone(), &p.job_id).await?;
            Ok(Reply::success(
                json!({"job":crate::automation::files::get(&permit,&p.job_id,None,0,0,1).await?}),
            ))
        }
        "rd_file_conflict_resolve" => {
            let p: FileConflict = parse(args)?;
            let (_, permit) = api::resolve(agent, &p.session_ref, true)?;
            crate::automation::files::confirm(
                permit.clone(),
                &p.job_id,
                p.overwrite,
                p.apply_to_remaining.unwrap_or(false),
            )
            .await?;
            Ok(Reply::success(
                json!({"job":crate::automation::files::get(&permit,&p.job_id,None,0,0,1).await?}),
            ))
        }
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
            let summaries=all.iter().filter_map(|s|{let snapshot=s.snapshot();let owner=s.control().view().agent_id;let mine=owner.as_deref()==Some(&agent.id);if matches!(p.scope,Some(Scope::Mine))&&!mine||matches!(p.scope,Some(Scope::Available))&&owner.is_some()&&!mine{return None;}Some(json!({"session_id":snapshot.session_id,"peer_id":snapshot.peer_id,"kind":snapshot.kind.name(),"gui_registered":!snapshot.ui_session_ids.is_empty(),"connection_state":api::state_name(snapshot.state),"owner":if mine{"self"}else if owner.is_some(){"other"}else{"none"}}))}).collect::<Vec<_>>();
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
            let kind = match p.kind {
                Some(Kind::Terminal) => SessionKind::Terminal,
                Some(Kind::FileTransfer) => SessionKind::FileTransfer,
                _ => SessionKind::Desktop,
            };
            let token = if let Some(source) = p.from_session_ref {
                let (source, permit) = api::resolve(agent, &source, false)?;
                permit.read_check()?;
                let snapshot = source.snapshot();
                if snapshot.peer_id != p.peer_id.trim() || !snapshot.authenticated {
                    return Err(BridgeError::invalid(
                        "Source session must be authenticated to the same peer",
                    ));
                }
                sessions::core(&snapshot.session_id).and_then(|core| core.get_conn_token())
            } else {
                None
            };
            let (session, created) = gui::reserve(
                agent,
                p.peer_id.trim(),
                kind,
                p.password.clone(),
                token,
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
        "rd_displays_get" => shape!(Read),
        "rd_display_modes_get" => shape!(DisplayModes),
        "rd_view_settings_get" => shape!(ViewGet),
        "rd_view_settings_set" => shape!(ViewSet),
        "rd_view_window" => shape!(ViewWindow),
        "rd_display_select" => shape!(DisplaySelect),
        "rd_display_resolution_set" => shape!(DisplayResolutionSet),
        "rd_virtual_display_set" => shape!(VirtualDisplaySet),
        "rd_session_open" => shape!(Open),
        "rd_session_attach" => shape!(Attach),
        "rd_session_get" => shape!(Get),
        "rd_session_list" => shape!(List),
        "rd_capabilities_get" | "rd_file_jobs" | "rd_clipboard_settings_get" => shape!(Read),
        "rd_clipboard_settings_set" => shape!(ClipboardSet),
        "rd_clipboard_read" => shape!(ClipboardRead),
        "rd_clipboard_write" => shape!(ClipboardWrite),
        "rd_clipboard_type" => shape!(ClipboardType),
        "rd_file_list" => shape!(FileList),
        "rd_file_clipboard_cancel" => shape!(Write),
        "rd_file_clipboard_get" => shape!(Read),
        "rd_file_clipboard_set" => shape!(ClipboardSet),
        "rd_file_clipboard_copy" => shape!(FileClipboardCopy),
        "rd_file_clipboard_paste" => shape!(FileClipboardPaste),
        "rd_file_job_resume" => shape!(FileJobWrite),
        "rd_file_manage" => shape!(FileManage),
        "rd_file_transfer" => shape!(FileTransfer),
        "rd_file_job_get" => shape!(FileJobGet),
        "rd_file_job_cancel" => shape!(FileJobWrite),
        "rd_file_conflict_resolve" => shape!(FileConflict),
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
                | "rd_displays_get"
                | "rd_display_modes_get"
                | "rd_capabilities_get"
                | "rd_file_clipboard_get"
                | "rd_file_jobs"
                | "rd_file_list"
                | "rd_file_job_get"
                | "rd_clipboard_read"
                | "rd_clipboard_settings_get"
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
        "rd_displays_get" => &[("displays","array"),("local_views","array"),("capture_selection","object"),("virtual_displays","object"),("layout_revision","string"),("remote_current_display","string")],
        "rd_display_modes_get" => &[("display_id","string"),("known","boolean"),("modes","array|null"),("current","object"),("original","object|null"),("custom_supported","boolean"),("unknown_hint","string")],
        "rd_view_settings_get" | "rd_view_settings_set" | "rd_view_window" => &[("confirmed","boolean"),("delivery","string"),("state","object"),("scope","string"),("ui_session_id","string"),("request_id","string"),("hint","string")],
        "rd_display_select" | "rd_display_resolution_set" | "rd_virtual_display_set" => &[("delivery","string"),("confirmed","boolean"),("state","object"),("requested","object"),("scope","string"),("driver_installation_may_occur","boolean")],
        "rd_file_clipboard_cancel" => &[("job_id","string"),("delivery","string"),("partial_files_may_remain","boolean")],
        "rd_file_clipboard_copy" => &[("delivery","string"),("clipboard_scope","string"),("remote_delivery","string"),("application_result","string")],
        "rd_file_clipboard_paste" => &[("delivery","string"),("clipboard_scope","string"),("application_result","string"),("paste","object|null")],
        "rd_file_clipboard_get" | "rd_file_clipboard_set" | "rd_clipboard_settings_get" | "rd_clipboard_settings_set" => &[("settings", "object")],
        "rd_clipboard_read" => &[("clipboard", "object")],
        "rd_clipboard_write" => &[("delivery", "string"), ("paste", "object|null")],
        "rd_clipboard_type" => &[("delivery", "string"), ("sent_events", "integer")],
        "rd_file_job_resume" | "rd_file_manage" | "rd_file_list" | "rd_file_job_get" | "rd_file_job_cancel" | "rd_file_conflict_resolve" => {
            &[("job", "object")]
        }
        "rd_file_jobs" | "rd_file_transfer" => &[("jobs", "array")],
        "rd_capabilities_get" => &[
            ("session_ref", "string"),
            ("peer", "object"),
            ("capabilities", "object"),
            ("contract", "object"),
        ],
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
