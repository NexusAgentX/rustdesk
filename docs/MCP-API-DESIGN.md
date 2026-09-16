# RustDesk MCP Interface and Service Design

Updated: 2026-09-15. Status: **The initial interface and service design is finalized, using the second revision. The initial bridge, 20 MCP tools, and GUI are implemented; the end-to-end single-display live test passed; dual-display live acceptance testing is pending.** See the [build and development log](MACOS-ARM64-BUILD.md) for current progress.

Based on the [MCP controller requirements](MCP-CONTROLLER-REQUIREMENTS.md). The project owner has approved the tool names, fields, call contracts, and service layers in this design as the basis for initial implementation and validation. The companion product requirements have been synchronized.

Base: RustDesk `1.4.9` / `6c578292e8ebbbec708b76986ba8c4bc7c509747`; the first platform is the local macOS / ARM64 machine. The selected SDK is `rmcp =3.3.0`, with Axum 0.8 for HTTP.

Incremental contracts are documented in [MCP capabilities and operation queries](MCP-CAPABILITIES-OPERATIONS.md) and [file transfer and text clipboard](MCP-FILES-CLIPBOARD.md). The design below retains the initial 20 tools; the incremental documents define subsequent additions.

## 1. Interface design principles

1. Split functionality into sessions, control ownership, screen capture, input, and terminals: **20 tools** in total. Use the `rd_` prefix and English `snake_case`; write concise English tool descriptions and errors, while continuing to localize GUI text.
2. Do not provide an implicit “current session” or “current terminal.” Bound operations take a single `session_ref`. Screenshots default to the primary display, and results always identify the actual selection.
3. Opening or binding does not mean authentication is complete; granting AI control does not mean the peer has authorized it; sending input does not mean the remote application executed it successfully.
4. Return machine-readable states and results. Screenshots also include native MCP images; terminal output preserves raw bytes.
5. Obtain caller identity from the authenticated MCP connection context. Tool arguments cannot specify or impersonate an `agent_id`.
6. Read-only tools never take control automatically. A rejected write must not silently request approval or remain queued for the next grant of control.
7. Provide queries only for existing long-lived objects: session state, approval requests, and terminals. The initial version introduces neither a general task system nor an arbitrary code-execution tool.
8. Common paths return the information needed for the next step: open can submit a password and wait for a bounded interval; input can include a screenshot; terminal writes can include a read. Separate authentication, capture, and read tools remain available.

### Changes from the first design revision

| Scenario | Second revision |
| --- | --- |
| Known device and password | open accepts an optional password, waits up to 10 seconds by default, and returns the actual state; a preceding list or immediate get is not mandatory |
| Target fields on each read/write | One `session_ref` replaces repeated `session_id + binding_id + control_token` arguments; all generations remain tracked internally |
| Ordinary one-off operation | operation_id is optional; provide it for safe retries after a lost response; omission carries no deduplication guarantee |
| Consecutive clicks, typing, and observation | An ordered input batch can include a screenshot; its snapshot identifier can be declared once at batch level |
| Reading output after terminal input | write can optionally wait for and return incremental output, without inferring command completion |
| Default result | A compact session snapshot, operation summary, and necessary errors; detailed diagnostics appear only in full-mode get |

## 2. Objects, identifiers, and isolation boundaries

| Name | Meaning and lifetime |
| --- | --- |
| `agent_id` | Display identifier assigned internally to a logical MCP session; separate from the authentication token and `Mcp-Session-Id` |
| `peer_id` | Remote device ID, as a string; cannot replace a local session ID |
| `session_id` | Local logical remote-control session ID assigned by the bridge, such as `s_…`; corresponds to one actual RustDesk core session |
| `session_ref` | Caller-facing session reference, such as `r_…`; the server associates it with an agent, core session, binding, and control/connection generations, so subsequent operations need only this reference |
| `ui_session_ids` | Set of official Flutter window/view session IDs; used only to correlate state with the GUI, not as MCP operation targets |
| `binding_id` | Internal bridge identifier for a binding; expires on detach or AI departure; no longer an MCP tool argument |
| `control_token` | Internal bridge identifier for a grant of control; expires on revocation; no longer an MCP tool argument |
| `connection_epoch` | Remote connection generation; increments on reconnect to prevent old frames, events, or input from being applied to a new connection |
| `display_id` | Display identifier within the current connection generation; not a local monitor index |
| `snapshot_id` | Identifier for a returned screenshot and its coordinate mapping, such as `f_…`; associated with a binding, connection generation, and display layout |
| `terminal_id` | Terminal instance ID assigned by the bridge, such as `t_…`; maps to its core session and the official numeric terminal ID |
| `approval_id` | Identifier for a takeover approval request, such as `a_…`; used for queries, deduplication, and cancellation |
| `operation_id` | Optional retry identifier supplied by the AI; distinct from the JSON-RPC request ID; calls execute independently when omitted |

### 2.1 Multiple windows share exclusive ownership of one core session

The official `sessions::SESSIONS` registry in `src/flutter.rs` stores core sessions by `(peer_id, ConnType)`, with multiple UI sessions under each core session. Individual window UUIDs therefore cannot be treated as connections that different AIs can own exclusively.

- Assign one `session_id` per actual core session, covering all associated GUI views.
- AI binding and control mode apply to the core session and all its views. Adding a display window does not create another independently ownable unit.
- Exclusivity does not become a device-wide lock. Desktop and terminal connections to the same peer are different kinds of core session and can be bound separately, but the GUI must clearly show ownership.
- Rebinding creates a new `binding_id`. Old calls cannot affect the new binding, even from the same AI using the same `session_id`.
- The GUI settings page shows the relationship as `agent_id → session_id → ui_session_ids / terminals`.

A `session_ref` is a reference, not a new authentication credential: every call still checks Bearer authentication, the logical MCP session, and agent ownership. The server stores its binding and control generations instead of asking the model to assemble those fields.

- open/attach return a reference; get/capture snapshots and control-management results return the current reference. `session_id` remains available for discovery, initial attach, and GUI correlation.
- A control change or remote reconnect creates a new reference. Old references remain readable while their original binding is valid, and reads return the current reference. Old references cannot write or silently execute under the new generation.
- Takeover requests also check the reference generation so an old queued request cannot revive after another human takeover. Reference-expiration errors return a compact current state; the AI can read it and explicitly request control again.
- Release and detach clean up a binding; approval cancellation targets an approval ID. Replaying old requests must not affect other bindings. A late release that crosses into a new control generation returns `CONTROL_EXPIRED` and does not revoke the new grant.
- References are not reused across detach or MCP reconnect. The current reference is never evicted while the binding is valid. Retain at most 64 old references per binding, each for 5 minutes after replacement. References outside retention return `SESSION_REF_EXPIRED`, never an unconditional operation by session ID.

### 2.2 Terminal and desktop connections

The official terminal uses `ConnType::TERMINAL`; `TerminalConnectionManager` shares one terminal connection among multiple terminal tabs for the same device.

- `session.kind` is `desktop` or `terminal`.
- `rd_session_open(kind="terminal")` opens a visible terminal window and creates the first terminal tab through the official flow. The result includes its terminal ID even if authentication is still pending.
- `rd_terminal_create` adds a visible tab to a bound terminal connection; it does not create a hidden shell.
- All tabs on one terminal connection share the AI binding and human/AI control mode. The initial version does not let different AIs own different terminal tabs on the same core connection.
- Desktop capture and image-coordinate input support only `desktop`; terminal tools support only `terminal`. A mismatch returns `WRONG_SESSION_KIND`.

## 3. Common arguments, states, and result format

### 3.1 Argument conventions

All tools accept a JSON object, reject unknown fields, and do not accept arbitrary JSON passthrough to FFI. IDs are opaque strings; the AI need not parse them.

The argument tables below use these abbreviations, expanded into ordinary top-level fields in actual JSON:

- **S**: `session_id: string`, required.
- **H**: `session_ref: string`, required; resolves the target and binding owner, replacing S and internal binding/control identifiers.
- **O**: `operation_id?: string`, 1–64 ASCII letters, digits, `_`, or `-`; provide for safe retries, omit for ordinary one-off calls.
- **W**: H plus O; the server checks the reference's control and connection generations before executing an AI write.
- `x?: type = value` denotes an optional field and its default; fields without a question mark are required.
- Output timestamps use RFC 3339 UTC. Wait durations are in milliseconds; timeout decisions use a monotonic clock.
- `revision`, frame sequence numbers, and byte offsets are decimal strings to avoid cross-language integer precision loss.

### 3.2 Session state

The default `SessionView` contains `session_id`, `session_ref`, `peer_id`, `kind`, `state`, `control`, and `revision`. The control object contains only mode and approval_required. Add fields as needed:

- Return `auth_challenge` while awaiting authentication, `human_action` while awaiting human intervention, and `approval` while awaiting takeover approval.
- open/attach and get without after_revision return `platform`, compact `displays: [{id,name,primary,width,height}]` or `terminals: [{id,state}]`, and necessary capability checks such as `can_input`, `can_capture`, and `can_use_terminal`. `can_input` means desktop keyboard/mouse input is currently available to the AI, accounting for control mode, authentication, and peer permissions. `can_use_terminal` describes terminal connection capability; writes still require AI control.
- Return a reason for unsupported or unauthorized capabilities; unknown does not mean allowed.
- When progress is blocked, return `next_action: {tool, reason}` suggesting an actionable next step. Do not execute the suggestion automatically or echo passwords.
- Ordinary input, terminal writes, and capture do not repeat the full session. Return only the operation result, current session_ref, and any changed control/connection state.

Only `rd_session_get(detail="full")` returns the complete `SessionState`, retaining the SessionView fields and expanding the following information:

| Field | Shape |
| --- | --- |
| Identity | `session_id`, `peer_id`, `kind`, `ui_session_ids: string[]` |
| GUI | `gui: {registered: boolean, visibility: "visible" / "minimized" / "hidden"}` |
| Connection | `connection: {state, epoch, authenticated: boolean, error: ErrorInfo / null}` |
| AI ownership | `owner: "self" / "other" / "none"`; internal binding IDs are not returned as required call arguments |
| Control | `control: {mode: "human" / "ai", approval_required: boolean, approval: Approval / null}`; the currently usable reference is `session_ref` |
| Capabilities | `capabilities`, each with `supported: boolean / null`, `allowed: boolean / null`, `reason: string / null` |
| Platform | `platform: string / null`, reported by the peer |
| Displays | `layout_revision` and `displays[]`, each with ID, name, origin, remote coordinate dimensions, decoded dimensions, and primary status |
| Terminals | `terminals[]`, including terminal ID, open state, size, and most recent error; output is not included by default |
| Synchronization | `revision`, `updated_at`; state changes increment revision, while video frames do not continuously wake state queries |

Connection states: `connecting`, `awaiting_auth`, `awaiting_human`, `awaiting_frame`, `ready`, `disconnected`, `closed`.

- `awaiting_auth` also provides `auth_challenge: {id, kind, fields}`; `kind` can be `password`, `two_factor`, or `os_login`.
- Other authentication, peer confirmation, or session-selection requirements enter `awaiting_human` and return `human_action: {kind, message}`. The initial version does not invent a generic interface for clicking arbitrary authentication dialogs.
- For `desktop`, `ready` requires completed authentication and an available remote frame. For `terminal`, it requires completed authentication and confirmed terminal capability, with no first-frame requirement. Whether a terminal is actually open is tracked separately in terminal state.
- `ready` does not guarantee permission for every write capability. `allowed: null` means not yet known and must not be treated as allowed.

### 3.3 Common tool result

Each tool defines its own `inputSchema` and `outputSchema`. Results share this top-level structure:

```json
{
  "ok": true,
  "status": "completed",
  "data": {}
}
```

- `ok=false` corresponds to MCP `isError=true`. Normal waiting states, or read timeouts with no new data, can return `ok=true`.
- `status` is `completed`, `pending`, `unchanged`, `partial`, or `failed`. `completed` means only that the action defined by the tool contract has completed; delivery evidence has separate fields.
- `data` is tool-specific. Partial execution still returns progress; errors must not discard the range already sent.
- Calls with a deduplication record return top-level `operation: {id, dedupe_expires_at, replayed?}`. replayed is true when returning the original result; the full OperationState is returned only on explicit query. Calls without operation_id, or rejected before acceptance, do not gain a redundant set of retry fields.
- Failures alone return `error: {code, message, retry: "never" / "after_state_change" / "same_operation", details?}`. Successful results omit error instead of filling the response with null fields.
- MCP `structuredContent` contains this object; the first text block in `content` contains its serialized JSON. Images appear separately as native image blocks. JSON holds only metadata, without duplicating large image base64 payloads.
- Text and structured results use the same compact object, avoiding repeated long explanations. No custom client capability negotiation is added for this purpose yet. Whether clients duplicate the result in model context requires live testing; eliminating that token cost is not promised in advance.
- Errors must help determine the next step, rather than returning only `false`, an empty string, or a low-level log.

Protocol structure errors and unknown tools use JSON-RPC errors. Invalid tool argument content, incomplete authentication, permission denial, and control denial use tool errors. HTTP authentication failures return 401 before MCP dispatch; disallowed Origins return 403.

Attached observations share `Observation: {kind: "screen" / "terminal", status: "completed" / "unchanged" / "failed" / "expired", data?, error?}`. data uses the corresponding standalone read tool's result. An unchanged result may still include the terminal cursor and closed state; failures use the same ErrorInfo. The main operation and observation have separate statuses; MCP isError follows the main operation. Cached observation bodies are stored separately from main-operation deduplication records: a global 32 MiB image budget and 8 MiB terminal-observation budget, each retained for at most 30 seconds. Eviction returns expired without repeating the write.

### 3.4 Retries and partial execution

- Calls with operation_id are deduplicated by `(agent_id, operation_id)`. The same ID must use the same tool and semantic arguments; reuse with different content returns `OPERATION_CONFLICT`. Without this field, two identical calls are independent actions; the service does not infer retries from similar arguments.
- To retry after a lost response, the caller must generate and supply operation_id before the first send. An ID generated in the result cannot recover a result that was itself lost. Orchestration code may generate it automatically; ordinary tool calls may provide it explicitly.
- Repeated calls for the same operation return its original progress or result without resending input. Records remain until 5 minutes after completion and return `dedupe_expires_at`. Each AI has a limit of 256 records; unexpired records are never evicted early. When full, reject calls requiring a new record and provide a retry time. Calls without operation_id still obey ordinary concurrency and queue limits.
- Concurrent duplicates atomically reserve the same record before executing the main operation once. Record queries still check the original agent and binding access. References and state in replayed results belong to the original execution time; they do not assert current control ownership. Use get for fresh state. Non-replayed calls are accepted and recorded only after permission and argument checks.
- Cleanup operations such as release, detach, and cancel are inherently idempotent by binding/approval identifier and cannot be blocked by a full deduplication store. If necessary, omit a new operation record and return the actual post-cleanup state.
- Argument digests use HMAC with a random key held within the service process. Records do not retain plaintext passwords. Sensitive arguments and terminal input are excluded from ordinary logs.
- Exactly-once execution is not guaranteed across MCP reconnect, process restart, or deduplication expiration. Retry an identified call with its original ID within the original logical MCP session. After omission of the ID or a session change, inspect state and screen first rather than blindly resending.
- Input results return `delivery: "not_sent" / "queued" / "sent" / "unknown"`, `completed_actions`, `sent_events`, and optional `failed_action_index`. `sent` means handed to the existing remote-control send path, not confirmed by the remote application.
- Operations with peer responses, such as terminal open/close, require explicit confirmation events. resize/write without peer acknowledgments return only delivery evidence.
- open/authenticate/reconnect/create can wait for a bounded interval directly; use queries only if still incomplete. Each wait is limited to 30 seconds. Ending an HTTP response or reconnecting SSE does not cancel accepted operations. Standard MCP cancellation cancels an active call; the bridge then cancels its unsent portion. Sent portions cannot be rolled back.

## 4. Tool catalog and contracts

### 4.1 Sessions: 9 tools

| Tool | Arguments | `data` and key behavior |
| --- | --- | --- |
| `rd_session_list` | `scope?: "all" / "mine" / "available" = "all"`, `cursor?: string`, `limit?: integer = 50` (1–100) | `{sessions: SessionSummary[], next_cursor}`; a minimal discovery summary includes ID, kind, GUI presence, connection state, and ownership; never exposes another AI's binding credentials, frames, or terminal output |
| `rd_session_open` | O, `peer_id: string`, `password?: string`, `kind?: "desktop" / "terminal" = "desktop"`, `force_relay?: boolean = false`, `wait_ms?: integer = 10000` (0–30000) | `{created, session: SessionView, initial_terminal_id?, password_applied?}`; opens or reuses and binds through the GUI, then waits until usable, additional authentication/human action is needed, failure, or timeout; returns current state and challenge |
| `rd_session_attach` | S, O | `{session: SessionView}`; binds an existing session while preserving its control mode; another AI's binding yields `SESSION_BUSY`; no implicit reconnect or focus change |
| `rd_session_detach` | H, O | `{session_id, detached: true}`; allowed under human control; cancels this binding's operations, releases input, returns control to the human, and keeps the GUI; repeated calls for the original binding have no side effects and do not affect later bindings |
| `rd_session_get` | H, `detail?: "summary" / "full" = "summary"`, `after_revision?: string`, `wait_ms?: integer = 0` (0–30000), `operation_id?: string` | `{session: SessionView / SessionState, operation?: OperationState}`; compact by default, with full capabilities, displays, and GUI diagnostics only in full mode; bounded wait if revision is unchanged, returning `unchanged` on timeout; also queries short-lived close records for the original caller |
| `rd_session_authenticate` | W, `challenge_id: string`, `credentials: AuthCredentials`, `wait_ms?: integer = 10000` (0–30000) | `{session: SessionView, delivery}`; submits the current challenge and waits for the actual result, returning immediately on the next challenge; if still processing at timeout, returns pending; an FFI return is not authentication success |
| `rd_session_disconnect` | W | `{session: SessionView}`; stops the peer connection while keeping a visible GUI container; cancels writes and approvals, releases input, and switches to human control; distinct from detach or closing the window |
| `rd_session_reconnect` | W, `force_relay?: boolean = false`, `wait_ms?: integer = 10000` (0–30000) | `{session: SessionView}`; explicitly reconnects with a bounded wait, updates the connection generation, and returns the current reference; revokes the old control grant, requiring another explicit takeover after reconnection |
| `rd_session_close` | W | `{session_id, closed: boolean, remaining_views: integer}`; closes every GUI view and the core connection for this logical session; use get for final confirmation when pending; detach does not count as close |

`SessionSummary` contains only `session_id`, `peer_id`, `kind`, `gui_registered`, `connection_state`, and `owner`. Pagination cursors are tied to the list revision. A changed list invalidates the cursor with `CURSOR_EXPIRED`; list again.

Reuse rules for `rd_session_open`:

1. Look up the core session by normalized peer ID and connection kind; reuse it if present. An existing binding owned by this AI retains its mode. A newly bound, previously unbound session retains human control. A session bound by another AI returns a busy error.
2. Create a core session only when none exists. **Only genuinely new sessions** default to AI control, preventing open from bypassing approval settings on existing sessions.
3. Keep a reservation while creation is pending. GUI creation and MCP binding share a correlation ID so concurrent opens cannot create competing duplicate connections.
4. Explicit GUI creation failure or expiry of the separate 30-second creation deadline returns `GUI_UNAVAILABLE`, leaving no connection that the AI can control invisibly. A window may be minimized; it must be registered and available for a human to open and observe, not permanently in the foreground.
5. If an external path produces multiple unmergeable core connections for the same device/kind, return `AMBIGUOUS_SESSION` and require list + attach to select one; do not choose randomly.

Password and wait rules for `rd_session_open`:

- A known peer_id can be opened directly without list. The server handles duplicate detection, binding, and state reporting.
- password is used only for this connection's password authentication and is submitted automatically at most once, only for a genuinely new session or one already under this agent's AI control. It is neither persisted nor echoed and is not used for OS login or 2FA.
- Reusing a human-controlled session preserves that mode, does not use the supplied password, and does not request takeover automatically. Return `password_applied=false` and the required control/authentication guidance. Discard that password when the call ends; do not silently submit it after some future grant of control.
- An authenticated session ignores an unnecessary password and explicitly returns `password_applied=false`. A wrong password stops automatic attempts and returns the actual authentication failure and current challenge; use authenticate afterward.
- wait_ms is this call's wait budget, not the connection's lifetime. Timeout keeps the session or creation reservation and returns pending with session_ref; there is no automatic close or implicit retry. Pending GUI creation must be queryable, and input is forbidden before registration completes. The separate GUI creation deadline still follows the reuse rules.
- desktop waits for the first authenticated frame or a blocking state. A new terminal waits for its first terminal to open or a blocking state; the AI does not need another list call to discover the result.
- Validate arguments before accepting open, then recheck binding and control generation before submitting the password. Human takeover during the wait stops unsubmitted passwords and deferred AI actions. Exhausting the wait budget alone does not cancel an accepted authentication intent. The password stays in memory only until that intent completes, fails, or is cancelled, then is discarded.

`AuthCredentials` is a mutually exclusive structure tagged by `kind`:

- `{"kind":"password","password":"…"}`.
- `{"kind":"two_factor","code":"…"}`.
- `{"kind":"os_login","username":"…","password":"…"}`.

A challenge accepts only its corresponding credential type. Initial MCP submissions neither save passwords nor trust the device. Errors must not echo credentials. Authentication failure leaves the GUI available for human handling.

### 4.2 Control: 3 tools

| Tool | Arguments | `data` and key behavior |
| --- | --- | --- |
| `rd_control_request` | H, O, `reason?: string` (at most 200 characters), `wait_ms?: integer = 0` (0–30000) | `{session_ref, control, approval?}`; returns pending immediately by default, optionally waiting for approval; returns a new reference when approval is unnecessary or granted; returns the current reference if this AI already controls the session and the reference is valid |
| `rd_control_cancel` | H, O, `approval_id: string` | `{approval}`; cancels an unfinished approval request; an expired, rejected, or approved request returns its existing outcome without undoing an effective grant |
| `rd_control_release` | H, O | `{session_ref, control, released_inputs}`; requires no human approval; cancels pending approval, queued work, and AI-held input, switches to human control, retains the AI binding, and returns a new reference usable for reads |

`Approval`: `{id, state, created_at, expires_at, remaining_ms, reason}`, with states `pending`, `approved`, `rejected`, `cancelled`, `expired`. Query changes through `rd_session_get`.

- Each binding allows at most one pending request, with a 60-second timeout. A new operation ID still reuses the pending approval without extending it.
- Final request records are retained for 5 minutes. If an old ID's result is no longer available, return `APPROVAL_EXPIRED` rather than treating it as a new request.
- The AI cannot claim human approval through arguments. Only trusted local GUI events can approve.
- AI control may be granted while awaiting authentication or disconnected so the AI can authenticate/reconnect afterward. Ordinary input still requires a `ready` peer and valid permissions. This avoids a cycle in which authentication requires control and control requires authentication; the rule is also reflected in product requirements section 3.9. A disconnection event still revokes the grant and approval request current at that time, requiring a new explicit request afterward.

### 4.3 Screen capture: 1 tool

| Tool | Arguments | `data` and key behavior |
| --- | --- | --- |
| `rd_screen_capture` | H, `display_id?: string = "primary"`, `after_frame_seq?: string`, `wait_ms?: integer = 0` (0–30000), `max_width?: integer = 1600`, `max_height?: integer = 1600` (each 1–3840) | `{session_ref, frame?: FrameInfo, image_content_index?}`, plus a PNG image block when a frame is available; readable under human control; primary resolves to the current remote primary display on each call and returns the actual display_id |

`FrameInfo` contains `snapshot_id`, `display_id`, `connection_epoch`, `layout_revision`, `frame_seq`, `received_at`, `age_ms`, `is_stale`, `is_new`, `image_width`, `image_height`, `remote_rect: {x,y,width,height}`, `cursor_composited: false`, and `mapping_expires_at`.

If no primary display can be identified, return a display-selection error and the available list; do not choose randomly. Comparing after_frame_seq requires the actual display_id from the previous result, not the potentially changed primary alias, to avoid comparing sequence numbers across displays.

- Export only decoded remote pixels. Use PNG by default, scaling down proportionally to fit the requested bounds; never upscale, crop, or stitch displays together.
- With `after_frame_seq`, wait for a decoded frame with a greater sequence number. Timeout returns `unchanged` rather than pretending an old image is new. If no frame is available, return `NO_FRAME`. Disconnected sessions may expose cached frames, explicitly marked disconnected and stale; those frames cannot be used for input on a new connection.
- Without after, return the latest available image. `is_new` is `null` without a comparison sequence. `is_stale` means received more than 2 seconds ago or disconnected; it does not claim knowledge of the peer's actual capture time.
- A static desktop may produce no new frames. A read may request the official video refresh, but cannot change human/AI control. If no new frame arrives, report that honestly.
- Do not composite RustDesk's separate cursor channel; set `cursor_composited=false`. Cursor pixels already present in the peer's video are not detected or erased.
- PNGs are limited to 8 MiB. Exceeding the limit returns `IMAGE_TOO_LARGE` with a suggestion to reduce dimensions; never return a truncated image.
- `snapshot_id` coordinate mappings live for 30 seconds, at most 64 per binding, and expire immediately on reconnect or layout change. Mapping validity does not imply that screen content stays unchanged for 30 seconds.

### 4.4 Input: 1 tool

| Tool | Arguments | `data` and key behavior |
| --- | --- | --- |
| `rd_input_send` | W, `actions: InputAction[]` (1–32), `snapshot_id?: string`, `capture?: CaptureOptions` | `{session_ref, delivery, completed_actions, sent_events, failed_action_index?, held_keys?, held_buttons?, observation?}`; executes in array order; the top-level snapshot_id is shared by coordinate actions in the batch; may include one screenshot after sending |

Each session allows one executing input batch and at most one additional queued batch; further requests return `INPUT_BUSY`. Different sessions may run concurrently. A batch is limited to 5 seconds; arbitrary long sleeps are not supported.

- Coordinate actions prefer their explicitly specified snapshot_id, otherwise inheriting the batch-level value. Reject when neither is present. Keyboard/text-only batches need no snapshot ID; cross-display drags may still specify one per point.
- `CaptureOptions` is `{display_id?: string = "primary", wait_ms?: integer = 1000, max_width?: integer = 1600, max_height?: integer = 1600}`. display_id can also be an actual ID; ranges match the standalone capture tool. Omitting capture disables automatic capture.
- Attached capture observes after actual sending finishes, waiting for the selected display's sequence number to advance beyond its value at send completion. Timeout explicitly returns unchanged and does not claim the image proves successful execution. Observation waits do not occupy the input send queue.
- If the main operation succeeds but the attached capture fails, retain the main operation's completed/sent result and report the observation error separately in `observation`. Suggest retrying capture alone, never the entire click batch.
- Observation failure, no new frame, or human takeover never resends input. The original binding remains readable after human takeover; detach stops observation. Partial execution may also return an observation, while retaining the main operation's partial status.
- Deduplication guarantees only that the main operation is not repeated. If a retry's original image has been evicted from the bounded cache, return `observation.status="expired"`; do not resend input or present a new image as the original result. The extra encoded-image cache is globally limited to 32 MiB and 30 seconds of retention.

`InputAction` is tagged by `type`; fields belonging to other variants are rejected:

| type | Fields and meaning |
| --- | --- |
| `move` | `snapshot_id?`, `x`, `y`: integer screenshot pixel coordinates |
| `button_down` / `button_up` | `button: "left" / "middle" / "right"`; optional complete `position: {snapshot_id?,x,y}`, moving before press/release; omission keeps the current remote pointer position |
| `click` | `snapshot_id?`, `x`, `y`, `button?="left"`, `count?: 1 / 2 = 1`; expands to move and press/release, with a fixed 100 ms double-click interval |
| `drag` | `button?="left"`, `points: {snapshot_id?,x,y}[]` (2–64), `duration_ms?: integer = 500` (1–3000); drags along the path and releases; points may come from different displays |
| `scroll` | `horizontal?: integer = 0`, `vertical?: integer = 0` (each -100–100, not both zero), optional `position`; units are logical wheel ticks, positive right/down, adapted by the bridge to the peer protocol |
| `key_down` / `key_up` | `key: KeyName` |
| `key_press` | `key: KeyName`, expanded to press and release |
| `shortcut` | `modifiers: Modifier[]`, `key: KeyName`; presses modifiers and the main key, then releases keys newly pressed by this action in reverse order |
| `text` | `text: string`, at most 16 KiB UTF-8; enters text verbatim without command parsing, automatic Enter, or use of the local clipboard |
| `release_all` | No other fields; releases all keys and buttons tracked for the current AI; cleanup after human takeover uses an internal path and requires no new AI control grant |

The initial `KeyName` set supports `KeyA`–`KeyZ`, `Digit0`–`Digit9`, `F1`–`F12`, `Enter`, `Tab`, `Escape`, `Backspace`, `Delete`, `Insert`, `Space`, `ArrowUp`, `ArrowDown`, `ArrowLeft`, `ArrowRight`, `Home`, `End`, `PageUp`, `PageDown`, and `ControlLeft/Right`, `ShiftLeft/Right`, `AltLeft/Right`, `MetaLeft/Right`.

`Modifier` is `Control`, `Shift`, `Alt`, or `Meta`, mapping to the left modifier by default. A macOS controller does not automatically replace Control with Meta. Prefer text for text and punctuation; unsupported keys return explicit errors.

- Statically validate the complete batch first. Before every actual input event, recheck AI identity, binding, internal control generation, remote connection generation, permissions, and screenshot layout mapping.
- AI events carry their control generation through to actual serialization and sending. Checking only when MCP receives or queues a request would allow old input in the official send queue to escape after takeover.
- Takeover cleanup is a high-priority local action, never queued behind ordinary input. Bytes already handed to the network cannot be recalled; distinguish that boundary explicitly.
- Track held keys as a set; duplicate down events do not increase a release count. A shortcut cannot release modifiers explicitly held by an earlier action.
- On batch failure or cancellation, make a best effort to release every input held by this AI and report delivery/cleanup failures. A successfully completed explicit down may remain held across calls until an explicit up, release, or lifecycle cleanup.

Coordinate conversion uses the remote origin and dimensions of the screenshot's display. For example, the horizontal coordinate is:

```text
remote_x = display_origin_x + min(remote_width - 1,
    floor((image_x + 0.5) * remote_width / image_width))
```

The vertical coordinate follows the same rule. Check image bounds first; do not silently clamp out-of-range input to the edge. Remote origins may be negative. Dimensions use the official remote coordinate space, without assuming a one-to-one relationship among GUI scaling, Retina pixels, and remote coordinates.

### 4.5 Terminals: 6 tools

| Tool | Arguments | `data` and key behavior |
| --- | --- | --- |
| `rd_terminal_list` | H | `{session_ref, terminals: TerminalState[]}`; lists instances on this terminal connection without creating any |
| `rd_terminal_create` | W, `rows?: integer = 24`, `cols?: integer = 80`, `wait_ms?: integer = 10000` (0–30000) | `{session_ref, terminal: TerminalState}`; creates a visible GUI tab before requesting the peer to open it, then waits for opened/error within a bounded interval; returns opening if incomplete, with read used for further waiting |
| `rd_terminal_read` | H, `terminal_id: string`, `cursor?: string`, `format?: "text" / "base64" / "both" = "text"`, `max_bytes?: integer = 16384` (1–65536), `wait_ms?: integer = 0` (0–30000) | `{session_ref, terminal, stream_epoch, start_offset, end_offset, next_cursor, oldest_cursor, has_more, text?, text_lossy?, data_base64?}`; omits unrequested representations; reads incrementally from cursor without consuming GUI buffers; omitting the initial cursor starts at the oldest currently retained byte |
| `rd_terminal_write` | W, `terminal_id: string`, `text: string` (at most 16 KiB UTF-8), `read?: TerminalReadOptions` | `{session_ref, terminal_id, bytes_sent, delivery, observation?}`; sends verbatim without adding a newline; can wait for some output, but does not wait for or infer command completion |
| `rd_terminal_resize` | W, `terminal_id: string`, `rows: integer`, `cols: integer` | `{terminal_id, requested_size, delivery}`; rows is 1–500 and cols is 1–1000; distinguishes requested size from peer-confirmed size |
| `rd_terminal_close` | W, `terminal_id: string` | `{terminal: TerminalState}`; closes the specified instance while retaining others, awaiting the peer's closed event for confirmation; closing the last tab may cause the official GUI to close the connection |

create uses the same rows/cols ranges as resize. `TerminalState`: `{terminal_id, state, requested_size, pid, shell_exit_code, error, stream_epoch}`, with states `opening`, `open`, `closing`, `closed`, `failed`. Unconfirmed information is null.

`TerminalReadOptions` is `{cursor?: string, wait_ms?: integer = 1000, max_bytes?: integer = 16384, format?: "text" / "base64" / "both" = "text"}`, with the same ranges as standalone read. Omitting read performs only the write. Supplying read without cursor records the current output end before sending and starts there; returned output is not guaranteed to come only from this input. If a cursor exists, keep using it to avoid skipping earlier output.

The attached read appears in `observation`, containing a normal read result or a separate read error. A read timeout/failure does not turn a successful send into a failed write. Continue with read afterward, without resending input. Retries with operation_id reuse the original output or explicitly report observation expired; they never write again.

- Each terminal retains a 4 MiB raw-output ring buffer, with a global 64 MiB budget. Under pressure, evict only the oldest raw bytes, without truncating or rewriting subsequent bytes. The GUI uses an independent buffer.
- A cursor binds the terminal instance, connection/output generation, and byte offset. Overwritten data returns `OUTPUT_GAP` with a recoverable oldest cursor; the AI explicitly resumes there. Never silently skip lost content.
- Repeated reads at an old cursor return the same retained data. With no new output, long polling may return `unchanged` on timeout. A terminal-close event must wake readers even without output.
- The buffer always preserves raw bytes. Default text decoding uses UTF-8 while preserving ANSI and carriage returns for AI readability. Invalid bytes or a split inside a character appear as replacement characters and set `text_lossy=true`. For exact bytes, reread from the original cursor with base64/both; decoded text is not presented as raw bytes.
- base64 includes all ANSI, carriage returns, and arbitrary non-UTF-8 bytes. The initial version neither strips terminal control sequences nor reconstructs a screen, and offers no “execute one command and return its exit code” tool. Cursors always count raw bytes; changing format does not change the read position.
- `shell_exit_code` comes only from a peer terminal-close event; a lost connection does not imply an exit code. Protocol replay on reconnect may include old content; a new `stream_epoch` explicitly marks replay rather than claiming output is inherently unique.
- Final output and close results remain readable to the original bound caller for 60 seconds after an instance closes, then return `TERMINAL_EXPIRED`. These are read-only completion records and do not retain active exclusive session ownership. A newly bound AI cannot read the previous binding's completion records.
- After a core GUI session closes, `rd_session_get` likewise retains a read-only close record for 60 seconds, accessible with the original AI identity and session_ref. After expiration it returns `SESSION_CLOSED`. Reading a record cannot restore control.
- AI detach or MCP session termination immediately revokes access to completion records. If the terminal connection remains running in the GUI, AI departure does not automatically close the shell.
- Terminal GUI keyboard/paste, AI input, and resizing all follow control mode. Under AI control, window resizing updates only local layout and cannot silently override the AI's requested PTY size. Switching to human control synchronizes the current GUI size.

## 5. Error categories and the AI's next step

| Error code | Meaning | Recommended AI action |
| --- | --- | --- |
| `INVALID_ARGUMENT` | Invalid argument content or range | Correct the arguments; no input was sent |
| `SESSION_NOT_FOUND`, `SESSION_CLOSED` | Session absent or closed | List again; do not reuse the old target |
| `SESSION_BUSY`, `AMBIGUOUS_SESSION` | Another AI owns the binding, or the target is ambiguous | Select a specific available session; do not create hidden duplicate connections to bypass ownership |
| `BINDING_REQUIRED`, `BINDING_EXPIRED`, `NOT_SESSION_OWNER` | Missing, expired, or another caller's binding | Explicitly attach or stop; another caller's content is not exposed |
| `SESSION_REF_EXPIRED` | Reference outside its retention window | Use list to find this agent's binding and attach to obtain the current reference; lost control is not restored |
| `HUMAN_CONTROL`, `CONTROL_EXPIRED` | Human control or a reference to an old control generation | Stop writing; explicitly request control if needed |
| `AUTH_REQUIRED`, `AUTH_FAILED`, `AUTH_CHALLENGE_CHANGED` | Authentication missing, failed, or changed | Read current state and submit the current challenge after obtaining control, or ask a human to handle it |
| `HUMAN_ACTION_REQUIRED` | Local or remote human confirmation required | Report the specific reason and wait; a dialog in another window is not evidence of success |
| `NOT_READY`, `DISCONNECTED` | Connection temporarily unavailable | Wait through get; reconnect must be explicit |
| `PERMISSION_DENIED`, `UNSUPPORTED`, `WRONG_SESSION_KIND` | Permission absent, feature unsupported, or wrong connection kind | Respect actual capabilities; do not repeatedly attempt the same write |
| `NO_FRAME`, `DISPLAY_CHANGED`, `SNAPSHOT_EXPIRED`, `IMAGE_TOO_LARGE` | Missing frame, changed layout, expired mapping, or oversized image | Query displays and capture again, or reduce output dimensions |
| `INPUT_BUSY`, `LIMIT_EXCEEDED` | Input queue or resource limit reached | Retry according to the returned wait guidance; do not expand the queue |
| `OPERATION_CONFLICT` | Retry ID belongs to different arguments | Use a new ID for a new action; first query the unknown outcome of the original action |
| `APPROVAL_EXPIRED`, `CURSOR_EXPIRED` | Approval record or list cursor expired | Query again; a new approval request must be an explicit new action |
| `TERMINAL_NOT_FOUND`, `TERMINAL_NOT_OPEN`, `TERMINAL_EXPIRED` | Terminal unavailable or completion record expired | Use list for actual state; do not write again to an old terminal |
| `OUTPUT_GAP`, `OUTPUT_EPOCH_CHANGED` | Buffer overwrite or changed reconnect output generation | Report loss/replay and explicitly resume from the new server-provided cursor |
| `GUI_UNAVAILABLE`, `SERVICE_STOPPING`, `INTERNAL_ERROR` | GUI or service unavailable | Retain the diagnostic identifier without exposing internal secrets; do not assume execution succeeded |

Failures involving side effects provide `delivery` or an explicit “not sent” statement so the AI can distinguish safe retries from potential duplicate execution. Read errors never trigger takeover by themselves.

`OperationState` is `{operation_id, tool, status, delivery?, completed_actions?, sent_events?, error?, dedupe_expires_at}`, omitting inapplicable fields. Query by replaying the original deduplicated call, or use `rd_session_get(operation_id=...)` for a known session. A missing record must explicitly mean unknown, never success.

## 6. Typical AI workflows

### 6.1 Open and operate a desktop

```text
Known device: rd_session_open(peer_id, password?)
Existing local session ID: rd_session_attach(session_id)
Use rd_session_list only when target discovery is needed
  → ready: use the returned session_ref and display information directly
  → pending: rd_session_get(session_ref, after_revision, wait_ms)
  → awaiting_auth: request control first if needed, then authenticate
  → awaiting_human: explain the required human action; use get for a bounded wait
If currently human-controlled: rd_control_request(session_ref, wait_ms)
  → pending: wait for approval through rd_session_get; use the new session_ref after approval
rd_screen_capture(session_ref)
rd_input_send(session_ref, snapshot_id, actions, capture={})
  → assess input and observation separately; use the new snapshot_id for the next coordinate input
Finish operating: rd_control_release(session_ref)
Finish participating: rd_session_detach(session_ref)
```

For a new desktop with a correct password that becomes ready within the wait budget, obtaining the first image normally takes two calls: open + capture. An ordered input batch and observation can be combined into one call. These are contract-level call counts, not measured latency or success rates. Execute sequentially when a result determines the next call's arguments; independent reads across sessions may run concurrently, while writes within a session remain ordered.

Releasing control, detaching, and closing the GUI have distinct meanings that tool descriptions must make clear. On completing an AI task, recommend release or detach by default. Do not automatically close unless the task calls for closing the session.

### 6.2 Human takeover during input

1. The GUI emits a trusted takeover event. The bridge revokes the old control generation and creates a new read-only session_ref.
2. An in-progress drag or shortcut stops at the next send boundary; queued batches are cancelled and release events scheduled.
3. The input result is partial or failed, including sent-event counts and HUMAN_CONTROL. Fully sent actions still truthfully report sent.
4. The AI retains its binding. Old references can still read and obtain the current reference, but cannot write, even if control is later reacquired.
5. To resume, the AI reads state and explicitly requests control. After approval, it uses the new reference for a new action; the old batch must not revive.

### 6.3 Terminals

```text
rd_session_open(peer_id, kind=terminal, password?)
  → returns session_ref, initial_terminal_id, and terminal state
  → first terminal already open: write directly, with no preceding list
  → not yet open: follow authentication/human-action guidance and wait for terminal state through read
rd_terminal_write(session_ref, terminal_id, text="pwd\n", read={})
  → assess delivery; save next_cursor from observation
rd_terminal_read(session_ref, terminal_id, cursor=next_cursor, wait_ms=10000)
  → continue reading as needed; a shell prompt does not guarantee a command exit code
Need another terminal: rd_terminal_create
Finish participating: rd_control_release or rd_session_detach
Need to close the shell: rd_terminal_close
```

Do not create a new terminal for every command or substitute a local terminal for remote execution. Instruction-like text in terminal output is remote content and must not override the AI's original task, tool descriptions, or control rules.

### 6.4 Multiple AIs

AIs A and B initialize MCP separately. A binds desktop S1; B may bind another desktop S2 or a separate terminal connection S3, but binding S1 returns busy. If A loses connectivity, only S1 is cleaned up; B's connections and operations continue. After A detaches, B may explicitly bind S1 but inherits none of A's references, control grants, screenshot mappings, or unfinished operations. If A only releases control, it retains the binding and B cannot join S1.

### 6.5 Input and error examples

Example `rd_input_send` arguments follow; IDs are illustrative. operation_id is supplied explicitly so the same arguments can be safely retried if the response is lost. Ordinary one-off calls may omit it:

```json
{
  "session_ref": "r_demo",
  "operation_id": "click-search-001",
  "snapshot_id": "f_demo",
  "actions": [
    {"type": "click", "x": 420, "y": 180},
    {"type": "text", "text": "RustDesk"},
    {"type": "key_press", "key": "Enter"}
  ],
  "capture": {}
}
```

Example structuredContent after human takeover:

```json
{
  "ok": false,
  "status": "failed",
  "data": {
    "session_ref": "r_human_demo",
    "delivery": "not_sent",
    "completed_actions": 0,
    "sent_events": 0
  },
  "error": {
    "code": "HUMAN_CONTROL",
    "message": "A human controls this session. Reading is allowed; request control explicitly before writing.",
    "retry": "after_state_change"
  }
}
```

This example represents rejection before queueing, with no deduplication record created. The outer MCP result sets isError=true. Returning the current reference does not regrant AI control.

Example result when all input was sent but no new frame arrived:

```json
{
  "ok": true,
  "status": "completed",
  "data": {
    "session_ref": "r_demo",
    "delivery": "sent",
    "completed_actions": 3,
    "sent_events": 7,
    "observation": {"kind": "screen", "status": "unchanged"}
  },
  "operation": {
    "id": "click-search-001",
    "dedupe_expires_at": "2026-09-15T10:05:00Z"
  }
}
```

sent_events is illustrative; the actual count comes from real sends in the underlying path. The AI continues observing with capture and does not repeat input merely to obtain a screenshot.

## 7. MCP protocol and transport design

### 7.1 Explicit protocol version

**The initial protocol is pinned to `2025-11-25`, using stateful Streamable HTTP.** SDK version and protocol version are separate choices.

- SDK 3.3.0 supports multiple protocol versions by default. `ServerHandler::supported_protocol_versions` can restrict the supported range, and `negotiate_initialize` provides reusable negotiation logic.
- Declare and negotiate only `2025-11-25` during initialization instead of accepting every SDK-supported version. Subsequent HTTP requests use the negotiated version; reject mismatches as required by the specification.
- This choice retains the established initialize, logical MCP session, standard ping, GET/SSE, and DELETE lifecycles. The 2026-07-28 lifecycle and result format differ and must not be mixed in without a design change.
- Use the SDK for protocol messages and serialization; do not build a custom JSON-RPC parser or handwritten legacy transport compatibility layer.

### 7.2 HTTP and identity

```text
127.0.0.1:21122/mcp
  → HTTP size limits / Origin checks / Bearer authentication
  → rmcp Streamable HTTP session management
  → one ClientContext per logical MCP session
  → argument parsing and tool routing
  → independent Rust bridge
  → official RustDesk connection, decoding, and input paths
```

- `POST /mcp`: initialization, requests, notifications, and client responses to server pings.
- `GET /mcp`: server event channel for initialized clients; maintains liveness probing. AI counts are not based on TCP connection counts.
- `DELETE /mcp`: ends this logical AI session and cleans up only its bindings.
- Authenticate before POST, GET, and DELETE. Generated MCP session IDs bind to `ClientContext`. The same token can initialize multiple independent AIs without sharing identities.
- Except for initialization, every call must belong to an established, unexpired logical MCP session. The initial version offers neither sessionless direct tool calls nor a new discover lifecycle entry point.
- Authenticated native clients without Origin may connect. If Origin is present, the initial version permits only the exact local MCP service Origin, rejects all others, and enables no wildcard CORS. Direct browser connections are not guaranteed in the initial version.
- HTTP bodies are limited to 256 KiB. Each AI may have at most 16 active tool calls, including read waits. Internal ping and control revocation are not blocked by this concurrency limit.
- AI release, detach, and approval cancellation use a separate short-operation path so long polling cannot consume all ordinary call slots and prevent cleanup.
- Temporary uninitialized sessions may wait at most 10 seconds. Incomplete handshakes do not appear in the GUI's connected-AI list and cannot bind sessions.
- ClientContext contains the internal `agent_id`, client-reported name/version, creation time, last communication, cancellation flag, and binding set. Metadata has length limits, is displayed as plain text, and is never authorization evidence.

### 7.3 SDK and tool registration

- Implement the 20 named tools through `rmcp::ServerHandler` and SDK tool routing. A service instance shares one Bridge; each handler context binds one AI identity.
- Argument structures reject unknown fields through `serde`, with `schemars` generating input schemas. Actions and credentials use explicitly tagged enums. Generate output schemas from each tool's concrete data types; both success and error results must satisfy their output constraints.
- Advertise only implemented tools capabilities. The initial version declares no resources, prompts, sampling, elicitation, tasks, or similar capabilities. Approval occurs in the local GUI and is queried through tools; the AI client need not support approval dialogs.
- The tools list stays fixed while running. Changes in peer capabilities appear in session capabilities and tool errors, not frequent tool additions/removals.
- Mark list/get/capture/terminal-read tools as read-only and conservatively mark writes as having side effects. Short-lived operation deduplication does not justify permanent idempotency claims through `idempotentHint`. Tool annotations are hints, never substitutes for permission checks.
- Server instructions briefly explain: open directly with a known peer_id or attach with an existing session_id; use session_ref afterward and operation_id as needed; respect control ownership, screenshot coordinate origins, and the distinction between delivery and execution; do not blindly retry writes with unknown outcomes.

### 7.4 State and cancellation propagation

- The bridge provides subscribable current-state snapshots to the GUI and tools. State validity does not depend on successful MCP push delivery.
- `rd_session_get(after_revision)`, `rd_screen_capture(after_frame_seq)`, and `rd_terminal_read(cursor)` wait for state, frames, and output respectively. Return unchanged when nothing changes; do not guess results using fixed sleeps.
- Subscribe before comparing revisions to avoid losing an event between reading state and starting a wait. If a queue drops events, return to the latest state snapshot rather than filling gaps from an unbounded log.
- MCP call cancellation, AI disconnection, control revocation, and service shutdown have distinct cancellation scopes. Cancelling a read does not detach the AI; cancelling an approval query does not cancel the independently existing approval request.
- Once request control returns pending, the approval request is independent state. Only explicit control cancel, release, or lifecycle cleanup cancels it.
- AI disconnection or explicit termination cleans up only that AI's context. MCP shutdown or global token reset revokes every context. Cleanup checks binding generations and does not affect later bindings.

## 8. Rust bridge and Flutter integration

### 8.1 Module boundaries

Responsibilities of the new modules follow; actual file boundaries may be adjusted for size during implementation:

```text
src/mcp/
  mod.rs          service lifecycle and shared state
  transport.rs    HTTP authentication and rmcp session lifecycle
  tools.rs        tool routing and argument/result conversion
  types.rs        public MCP schemas

src/automation/
  mod.rs          MCP-independent bridge entry point
  sessions.rs     core sessions, GUI views, AI bindings, and state
  control.rs      control ownership, approvals, and cancellation generations
  input.rs        ordered input and held-input state
  frames.rs       independent pixel cache and coordinate mappings
  terminals.rs    terminal state, raw output, and cursors
  operations.rs   bounded deduplication records
```

The MCP layer does not directly lock Flutter session internals or encode RustDesk keyboard/mouse messages. The bridge accepts generic caller identities and structured operations; GUI takeover calls the bridge directly as well.

The initial embedded service is enabled only in Flutter + macOS controller builds. The background controlled service, connection-manager process, and individual child windows do not each listen on a port. Existing GUI paths remain usable when the build option or service is disabled.

### 8.2 Creating visible GUI sessions

The native macOS entry point registers multi-window Flutter controllers. The multi-window plugin pinned in the lockfile uses in-process native window management. The initial design uses the Rust core registry within one process; Dart windows use separate isolates and cannot share Dart objects.

1. The Rust bridge creates a GUI creation request with a timeout and sends it to the main Flutter event channel.
2. The main window creates/activates a normal desktop or terminal UI through the existing `RustDeskMultiWindowManager`.
3. When the target UI registers its core session/view through official FFI, it returns the request correlation ID and the bridge completes association and binding.
4. AI remote input is forbidden until GUI registration is confirmed. After GUI creation timeout, call cancellation, or AI disconnection, a late GUI callback may leave a normal human session but cannot grant control to an expired AI.
5. Local FFI callbacks let the settings page and corresponding remote-control views subscribe to the same control state. Old UI input must also expire at the AI takeover boundary; clearing focus in the UI alone is insufficient.

Every deferred GUI callback that creates an object for an AI, including automatic open after terminal authentication, carries the original request and control generation. Human takeover or AI departure prevents it from starting a shell under the old AI grant. Normal human-created paths are unaffected by cancellation of these AI requests.

Live testing must confirm that all windows load the same Rust library instance and session registry. Do not prebuild an IPC framework for unsupported platforms here.

### 8.3 Final checks in the input queue

Official input uses `Data` messages and an IO-loop send queue. A queue only on the MCP side cannot ensure old input stops after takeover.

- Add an input envelope carrying `{caller, binding_id, control_generation, connection_epoch}` through to the actual remote send boundary.
- Prefer a new client `Data` variant used only by this feature and a small IO-loop branch. Do not rewrite the existing `Data::Message` path for ordinary human sessions through a new abstraction.
- The final sender for a core session serializes AI events and control changes; any unsent event can be discarded. Takeover's linearization boundary is the point where the old send authorization is invalidated and release is scheduled. No send under the old authorization starts afterward.
- Gate GUI input only when an AI binding exists and control mode is AI. Keep the existing input implementation for human control or disabled MCP. Do not change every `Interface` signature for this feature.
- When first installing a binding on an existing human session, establish a handoff boundary in the sender. Earlier GUI input without the new feature's identifiers must finish before that boundary or be cleared; subsequent GUI input follows control generations. Do not announce AI control while old human input can still pass through the queue.
- Hold no global lock while awaiting the network or a queue. Use `spawn_blocking` for screenshot encoding so control revocation and heartbeats cannot be blocked.

### 8.4 Frame caching and multiple displays

- Copy frames into bridge snapshots in `FlutterHandler::on_rgba` before GUI/texture dispatch. Do not consume GUI buffers through `session_next_rgba` or swap pixels away from the GUI.
- Keep the latest complete CPU frame per display and pass shared read-only references to screenshot encoding. Network/decoder threads do not wait for image encoding.
- Limit each cached frame to 128 MiB, with a total pixel-cache budget of 512 MiB. Under pressure, evict old caches for the least recently read displays, then report no frame or request refresh without disturbing GUI buffers. Allow at most two concurrent encoding tasks.
- Take the union of video subscriptions needed by AI reads and current GUI views. GUI display switching/minimization must not cancel a source in use by the AI; detach releases only the AI's subscriptions.
- Initial macOS AI sessions must select CPU-readable decoded output; the GUI may still use those pixels for texture rendering. Software decoding is an explicit fallback. Do not hook only the software-rendering branch and miss texture rendering.
- For an existing GPU-only session, prepare CPU output in the same session or switch to software decoding on bind. Explicitly report awaiting_frame during the transition. If no pixel path is available, report a capability failure; a binding with permanently unavailable capture cannot pass acceptance.
- Display addition/removal or changes in origin, size, or rotation update the layout revision and invalidate old mappings. Connection generations and display identifiers prevent reuse of stale indices.

### 8.5 Terminal events

- When official `handle_terminal_response` receives opened/data/closed/error, use the same event to update bridge state while preserving the original GUI event flow.
- Decompress data and retain the complete bytes, then forward them separately to the GUI and bridge ring buffer. Do not reconstruct raw output from Flutter xterm's rendered text.
- Reusing an existing void-returning FFI entry point proves only that the entry point was called. The bridge must add target validation, send-failure handling, and correlation with peer events.
- Only one correlated path may send terminal open, preventing both GUI automatic open and the MCP tool from issuing it. The creation request, official numeric terminal ID, and bridge terminal ID have a one-to-one correspondence.

## 9. Service, configuration, and resource lifecycle

- `disabled → starting → running / failed → stopping → disabled`. starting becomes running only after binding the listener successfully and making routing and cancellation ready. The GUI may display stopping as shutting down.
- Persist the enabled setting. Autostart the service only after application startup has registered GUI events. Closing settings or minimizing the main window does not stop it; exiting the entire GUI controller process does.
- A port conflict is failed; do not select another port automatically. Port changes and token resets use a controlled stop/restart, keeping configuration consistent with actual runtime state.
- stop first rejects new writes and invalidates all control generations, then cancels waiting/queued work, releases input, removes bindings, and ends MCP sessions and the listener. Release failures remain visible but do not prevent final shutdown. Do not close RustDesk GUI remote-control sessions.
- Bound all resources: input queues, operation records, pixel caches, terminal bytes, and state waiters. Return explicit limit errors instead of relying on process exhaustion.
- All liveness and approval timers use independent timers. Process expired entries first after host resume. An SDK transport interruption while the logical session remains valid is resolved by the liveness rules.
- Store the 32-byte random Bearer token in Keychain. Runtime logs contain only tool names, display `agent_id`, session IDs, durations, result codes, and correlation identifiers; exclude full arguments/results, passwords, tokens, screenshots, and terminal content.

## 10. Validation plan and regression scope

### 10.1 Contract and concurrency validation

- Validate legal/illegal arguments and result structures for every tool against schemas. Unknown fields, out-of-range coordinates, and wrong session kinds must send no events.
- Only one of two AIs concurrently binding the same core session succeeds. Multiple GUI views cannot bypass exclusivity.
- Replaying an operation ID does not repeat clicks, typing, or terminal creation. Reusing an ID with different arguments is rejected. Do not claim deduplication after cache expiration or service restart.
- Identical calls without operation_id execute independently; concurrent duplicates with an explicit ID execute once. Replayed results must not be interpreted as current control grants.
- open submits its optional password once. Reusing a human-controlled session neither submits nor retains it for later. Wait-budget exhaustion and GUI creation failure return pending and an explicit error respectively.
- After a control change, old session_refs can read but not write. Reapproval does not let old batches, releases, or takeover requests affect the new generation. Reference eviction must not invalidate the current reference.
- Verify batch-level screenshot mapping inheritance, per-action overrides, and cross-display paths. Capture failure, timeout, or cache expiration after successful input never resends input. Attached terminal reads follow the same rule.
- summary and full agree on the same state. Omitted optional fields satisfy schemas; native images correctly correspond to observation metadata.
- Inject takeover before a batch, midway through a drag, before the final send, and during approval/cancellation races. Assert that no old-generation input starts sending after the revocation boundary and that the sent range is accurately reported.
- Detach and immediately rebind. Late cleanup and approvals must not change the new ownership.
- Test continued reads and rejected writes in read-only mode, voluntary AI release, and both approval-required and approval-free settings.
- A new HTTP connection or rebuilt GET stream does not count as a new AI. One AI's disconnection does not affect others. Heartbeats cannot be blocked by long polling, a full input queue, or image encoding.

### 10.2 Live screen and terminal validation

- Connect the local macOS / ARM64 controller to a stock peer. Confirm capture with both CPU and texture GUI rendering and correct mapping for multiple displays, negative coordinates, scaling, rotation, display switching, minimization, and reconnect.
- Compare bridge pixels before and after GUI consumption to verify that GUI buffers are not consumed. Check semantics for static screens, no first frame, stale caches, and new frames.
- Preserve raw ANSI, non-UTF-8, and fragmented UTF-8 terminal bytes. Detect repeated reads, buffer overwrite, replay, close without output, and shell exit codes.
- Terminal reconnect must not create duplicate shells. Separately verify closing one tab, closing the last tab, and AI detach.
- Invalid tokens, pre-reset tokens, external Origins, another AI's session_ref, and old-control-generation references must not write.

### 10.3 Existing paths expected to require changes

| Existing file/path | Necessary change and boundary |
| --- | --- |
| `src/lib.rs` / client startup | Register new modules and start/stop the service with the GUI controller lifecycle; do not listen separately in every window |
| `src/flutter_ffi.rs` | Add thin interfaces for settings, state subscriptions, GUI creation confirmation, and takeover; do not export the entire FFI as MCP |
| `src/flutter.rs` | Associate sessions/views, copy decoded frames, and mirror terminal events; preserve original GUI rendering/event consumption |
| `src/client.rs`, `src/client/io_loop.rs` | Add AI input envelopes, final send-authorization checks, and thin connection/permission hooks; preserve the human message path as far as possible |
| `src/ui_session_interface.rs` | Add thin entry points only where the bridge needs existing send/authentication capabilities; do not change shared trait signatures and force unrelated callers to supply placeholders |
| Flutter settings page and new MCP widgets | Display the service and connected-AI list using the same bridge state |
| Flutter remote-control/input models and toolbar | Read-only behavior under AI control, human takeover entry points, and cleanup of human-held input; preserve existing interaction when no AI is bound |
| Flutter multi-window management and terminal pages/models | Create visible objects and confirm association; gate terminal input and resizing; preserve normal human creation flows |

Do not change the controlled-side protocol or update the hbb_common submodule for this purpose. This is the minimum expected scope to validate during implementation, not permission to expand changes prematurely. This documentation design round changes no runtime paths.

## 11. Final decisions and implementation boundaries

The project owner approved splitting tools by capability and the following design:

1. 20 tools with `rd_` names. open supports an optional password and a default 10-second bounded wait; authenticate handles subsequent challenges.
2. `session_id` represents a core connection with exclusive ownership shared across its GUI views. Terminals use a separate connection whose tabs all share control.
3. Bound tools take only session_ref; the server associates binding and control generations. operation_id is optional, but must be supplied in advance for safe retries after a lost response.
4. Capture defaults to the primary display and a 1600×1600 PNG boundary. Input batches share snapshot_id and may attach a screenshot. Terminal write may attach an incremental read, returning UTF-8 text with ANSI preserved by default and optional raw bytes. Failure of an attached read never resends the main operation.
5. Default results are compact; full-mode get provides detailed state. Distinguish unknown information from fields that need not be returned.
6. Use protocol `2025-11-25` with the selected SDK 3.3.0, restrict negotiation, and do not mix in other lifecycles.
7. Allow control acquisition before authentication or while disconnected so the AI can authenticate/reconnect. Ordinary input still checks connection state and permissions. Product requirements section 3.9 reflects this rule.

During implementation, generate complete machine-checkable schemas from Rust types and validate according to section 10. The tool tables, field types, boundaries, and examples are the approved implementation contract; at the time of design finalization, the tools had not yet been registered or implemented. Subsequent contract changes should update the relevant documents and receive project-owner confirmation.

## 12. References checked

### Pinned RustDesk source

- [Session FFI and terminal interfaces](https://github.com/rustdesk/rustdesk/blob/6c578292e8ebbbec708b76986ba8c4bc7c509747/src/flutter_ffi.rs)
- [Core session registry, RGBA, and terminal events](https://github.com/rustdesk/rustdesk/blob/6c578292e8ebbbec708b76986ba8c4bc7c509747/src/flutter.rs)
- [IO loop and actual send path](https://github.com/rustdesk/rustdesk/blob/6c578292e8ebbbec708b76986ba8c4bc7c509747/src/client/io_loop.rs)
- [Shared terminal connections](https://github.com/rustdesk/rustdesk/blob/6c578292e8ebbbec708b76986ba8c4bc7c509747/flutter/lib/desktop/pages/terminal_connection_manager.dart)
- [Terminal GUI lifecycle](https://github.com/rustdesk/rustdesk/blob/6c578292e8ebbbec708b76986ba8c4bc7c509747/flutter/lib/desktop/pages/terminal_page.dart)
- [macOS window initialization](https://github.com/rustdesk/rustdesk/blob/6c578292e8ebbbec708b76986ba8c4bc7c509747/flutter/macos/Runner/MainFlutterWindow.swift)
- [macOS multi-window management in the pinned dependency](https://github.com/rustdesk-org/rustdesk_desktop_multi_window/blob/b47e8385e5a75d38319ad706a64b0ead3108b093/macos/Classes/MultiWindowManager.swift)

### Official MCP and SDK references

- [2025-11-25 tools and structured results](https://modelcontextprotocol.io/specification/2025-11-25/server/tools)
- [2025-11-25 Streamable HTTP](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)
- [2025-11-25 cancellation](https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/cancellation)
- [SDK 3.3.0 ServerHandler and negotiation range](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.3.0/crates/rmcp/src/handler/server.rs)
- [2026-07-28 tool and lifecycle documentation, used to identify version differences](https://modelcontextprotocol.io/specification/2026-07-28/server/tools)
