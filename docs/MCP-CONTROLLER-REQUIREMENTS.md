# RustDesk MCP controller requirements

Updated: 2026-09-15. Status: **initial implementation complete; the live single-display workflow passed; live dual-display acceptance pending**. See the [build and development record](MACOS-ARM64-BUILD.md) for implementation progress.

This document records the project owner's approved initial product requirements as the development and acceptance baseline. Upstream baseline, controller platform, initial scope, transport, human/AI collaboration rules and MCP interface/service design are finalized. See [MCP interface and service design](MCP-API-DESIGN.md) for exact contracts. Finalized design does not itself mean implementation or builds are complete.

## 1. Project goal

Build a **controller client retaining the full GUI and embedding an MCP service**, based on official RustDesk client source.

- AI uses remote desktop, keyboard/mouse, terminal and related capabilities through MCP.
- A person watches in the normal RustDesk window and can take over the same remote session at any time.
- AI and the person share sessions, images and operation results.
- The controlled machine keeps official RustDesk and can use the existing `rustdesk-launcher` bootstrapper; MCP requires no changes to the controlled client.

## 2. Technical approach

- Use the project owner's specified official stable **RustDesk 1.4.9**, pinned by tag and commit rather than following moving `master` or substituting the latest version at development startup.
- Retain the official Flutter / Dart GUI and Rust remote-control core.
- Add a bridge inside the Rust core, reusing existing session and operation interfaces.
- Use the **official MCP Rust SDK `rmcp`, pinned to `=3.3.0`**. See section 4 for transport and [MCP interface and service design](MCP-API-DESIGN.md) for the MCP layer.
- Embed MCP in the client without a separately started service program.
- YaoxinCS/RustdeskMCP is a reference only, not the source baseline.
- This new project does not provide compatibility or migration for older bridge protocols.
- **MCP tool names, arguments, result formats and AI-facing interaction design have been approved by the project owner.** This document defines underlying capabilities and product behavior; the companion design specifies tool contracts.

### 2.1 Pinned baseline and target platform

| Item | Decision |
| --- | --- |
| Official repository | `https://github.com/rustdesk/rustdesk.git` |
| Stable tag | `1.4.9` |
| Pinned commit | `6c578292e8ebbbec708b76986ba8c4bc7c509747` |
| `libs/hbb_common` submodule | Use the tag's `7e1c392c62d39c364127307cd408421dd5f8cfb0`, without independently following latest |
| Development branch | `codex/mcp-controller`, starting at the pinned commit above |
| First controller platform | Local macOS, Apple Silicon / ARM64 |
| Rust target | `aarch64-apple-darwin` |
| Current acceptance machine | macOS `26.6.2` (`25G83`), model identifier `Mac17,3` |
| Initial scope | Full remote desktop, keyboard/mouse/text, human/AI takeover, interactive terminals, multiple displays and MCP settings |

The OS version records the first acceptance environment; it is not a support commitment for other macOS versions or architectures.

### 2.2 Overall structure

```text
AI client
    ↕ MCP
RustDesk controller client
    ├─ MCP service layer
    ├─ Rust bridge layer
    ├─ Official session, communication and decoding core
    └─ Flutter GUI: viewing, manual operation, takeover
                ↕ Existing RustDesk protocol
         Official RustDesk controlled client
```

## 3. Human/AI collaboration and control

### 3.1 Two independent states

Each session separately tracks:

1. **Whether AI has joined**: whether an AI is bound to the session.
2. **Current control mode**: AI control or human control.

Binding and acquiring control are separate actions. Connection state, AI presence and control mode must be represented independently.

### 3.2 Initial session behavior

- **A new session opened by AI** starts in AI control; the GUI shows that AI is controlling it.
- Human approval governs transitions from human to AI control. An AI-created session still starts as described above without an additional approval wait.
- **AI attaching to an existing human-opened session** preserves its control mode; the GUI shows that AI has joined.
- **AI voluntarily releasing control** remains attached and can continue reading.

### 3.3 Two control modes

| Action | AI control | Human control |
| --- | --- | --- |
| Human views remote image | Allowed | Allowed |
| Human operates peer through GUI | Blocked | Allowed |
| AI reads images, state and terminal output | Allowed | Allowed |
| AI writes remotely | Allowed, subject to actual peer permissions | Rejected with an explicit control error |
| Human clicks takeover | Always available | Already under human control |
| AI calls acquire-control tool | Already under AI control | Acquire directly or request approval according to settings |
| AI calls release-control tool | Switch to human control | Remain under human control |

During AI control:

- The remote image in the GUI is read-only.
- Moving over it shows a read-only cursor.
- The GUI sends no human mouse, keyboard or text input to the peer.
- Local scaling, display switching, connection information, session closure and takeover remain available.

### 3.4 GUI status

The remote-session UI provides a clear status banner and reliable takeover entry.

| State | UI |
| --- | --- |
| AI absent, human control | Normal manual-session UI |
| AI attached, human control | “AI has joined · You are in control” |
| AI attached, AI control | “AI is controlling”, with a “Take control” button |
| AI requesting control, awaiting approval | Show the request and “Allow / Deny”; retain human control until approval |

Exact wording and styling may change during UI design, but state must remain clear and synchronized in real time.

### 3.5 Control transitions

#### Human takeover

- The person takes control through the remote-session banner without AI consent.
- Stop subsequent AI writes and cancel queued writes not yet executed.
- Release AI-held keys and mouse buttons.
- Restore manual GUI operation.

#### AI requests control

MCP provides an acquire-control tool. Settings include whether AI control requires human approval, **enabled by default**:

- **Approval disabled**: an explicit tool call can directly acquire control from human mode.
- **Approval required**: display a GUI request and switch only after the person allows it. Denial or pending approval retains human control.

AI must explicitly request control; reconnecting, reading or retrying writes does not acquire it automatically.

#### AI releases control

MCP provides a release-control tool:

- Switch directly to human control without approval.
- Stop queued AI writes and release held AI input state.
- Restore GUI manual operation and update the banner.
- Keep the AI attached with read access.
- Repeated release while already under human control succeeds.
- Acquiring control again requires an explicit tool call and follows approval settings.

### 3.6 Execution constraints

- Control is per session, with GUI and bridge state consistent.
- Rust bridge logic enforces restrictions; disabled GUI controls or MCP argument checks alone are insufficient.
- Queued operations must recheck control immediately before sending, so old requests cannot continue after human takeover.
- Transitions handle keys/buttons still held by the previous controller to prevent stuck input.
- Session closure, disconnect and MCP stop clean up AI input state.
- Actions already sent cannot be withdrawn. For example, a terminal command already started does not automatically stop on takeover.

“AI writes” includes remote state changes: keyboard/mouse, text, terminal input and later file changes, clipboard writes and system actions. Control-management operations such as acquire/release follow their own rules.

The initial design uses a **read-only image plus an explicit takeover button**. Automatically taking over when human input is detected is not required for the first version.

### 3.7 Multiple AI clients and session exclusivity

- **Multiple AI clients may connect to MCP simultaneously; each local RustDesk session allows only one AI binding at a time.** Exclusivity applies to remote-control sessions, not the entire controller or MCP service.
- Different AIs can bind different sessions; one AI can bind multiple sessions. Each independently tracks ownership, presence and control.
- Identity is the authenticated, initialized logical MCP session, not TCP-connection or tool-call count. Concurrent HTTP requests within a logical session are one AI; different logical sessions using the same access token are different AIs.
- Other AIs cannot attach to an occupied session and receive an explicit busy error, without being prevented from connecting to MCP or binding other sessions. Reattaching to one's own session creates no duplicate binding and does not change control mode.
- Occupancy checks and ownership updates must be atomic so only one concurrent attachment succeeds. Session operations check ownership; knowing a local session ID alone does not authorize operations on another AI's binding.
- Human takeover or voluntary AI release does not detach the AI or relinquish exclusive binding ownership. Other AIs cannot steal it through acquire, release or detach calls.
- Explicit detachment or logical MCP session termination/expiry releases ownership. Another AI may then explicitly attach, preserving the session's human-control mode; acquiring control still follows approval settings.
- An AI can remain connected to MCP after detaching its final RustDesk session without occupying other sessions. Processes sharing logical-session credentials are the same client; the first version does not distinguish multiple models/agents inside them.

### 3.8 Departure, disconnect and service stop

| Event | AI presence and control | GUI sessions and pending operations |
| --- | --- | --- |
| AI voluntarily releases control | Retain AI presence; switch to human control | Keep session, cancel pending AI writes and release AI input |
| AI detaches one RustDesk session | Clear its AI presence; return human control | Keep session, cancel its takeover requests/pending writes and release input |
| AI ends its MCP session or is declared lost | Clear only that AI's bindings/presence, return their sessions to human control and release ownership | Keep GUI sessions, cancel that AI's takeover requests/pending writes and release input; other AIs are unaffected |
| MCP stops or token resets | End all logical MCP sessions, clear bindings and return human control | Keep GUI sessions; old sessions/late requests cannot operate; token reset also invalidates the old token |
| Peer disconnects while AI's MCP session remains valid | Retain AI presence for surviving GUI sessions; switch to human control | Show disconnected, cancel takeover requests/pending writes and clear input tracking |
| RustDesk GUI session closes | Remove its binding and control state | Cancel its takeover requests/pending writes and clean input |

- Cleanup first blocks future writes, then best-effort releases keys/buttons through any surviving remote connection. If already disconnected, do not claim release messages were delivered. Clear local tracking to prevent replay after reconnect.
- Reconnecting AI or reattaching to an existing session does not restore old control, approval or input queues. Explicitly reacquire control under approval rules.
- Stopping/restarting MCP retains the saved token but requires new logical MCP sessions. Only explicit token reset replaces credentials.
- Remote reconnection also does not automatically restore AI control; explicit takeover is required.
- Cleanup is idempotent; late completion or approval cannot revive bindings/control.
- Cleanup uses the original AI identity and binding. If another AI has rebound the session, late cleanup by the old AI cannot remove or change the new binding/control.

#### Detecting a lost client

- Use the stateful Streamable HTTP transport selected in section 4. An HTTP request ending, TCP closure or temporary SSE reconnect does not itself mean the AI left.
- Probe each logical MCP session independently using standard MCP `ping`, by default every 15 seconds with up to 15 seconds for a reply. Timeout ends that session and cleans only that AI's bindings/operations. Under normal operation the target detection bound is approximately 30 seconds.
- A responding AI is not detached merely for having no tool calls. Probes/timing run independently of tools and cannot be blocked by long terminal reads or approval waits.
- Explicit departure, service stop and token reset clean up immediately without waiting for probe timeout.
- AI process crashes are not guaranteed to be detected instantly; human takeover remains available during detection. After host sleep, expired liveness state is cleaned before new writes are accepted.

### 3.9 Takeover approval requests

- Each remote session has at most one pending takeover request, owned by the current logical MCP session and binding.
- **Approval expires 60 seconds after request creation.** Expiry ends the request and removes its prompt, retaining human control; it never means implicit approval.
- Repeated requests by the same AI while pending reuse the request and remaining time, without stacking dialogs or extending expiry.
- Approval switches control only if the AI, binding, GUI session, request and control epoch remain valid; otherwise it ends with an explicit reason. Control may be granted while awaiting authentication or disconnected so AI can explicitly authenticate/reconnect. Ordinary input still requires an operable peer and permission. A disconnect revokes then-current control and pending approval; another explicit request is required afterward.
- Denial or AI cancellation retains human control. Cancelling a finished request does not change control; use release after an approval has taken effect.
- Expired, rejected and cancelled are distinct outcomes. Another attempt requires a new explicit request; the service neither retries nor reopens approval prompts automatically.
- Explicit human takeover, AI release/detach, MCP session termination, remote disconnect, GUI closure, MCP stop and token reset cancel related pending requests.
- Changing the approval-required setting cancels pending requests. The new value applies only to later explicit acquisition; it does not auto-approve an old request.
- Racing timeout, cancellation and approval have exactly one final outcome. Late clicks/responses cannot change a finished request.
- This section defines product behavior; arguments, request IDs and result formats follow [MCP interface and service design](MCP-API-DESIGN.md).

## 4. MCP settings page

Add an **MCP tab** to settings.

| Item | Requirement |
| --- | --- |
| Service switch | Start/stop the embedded MCP service |
| Runtime state | Show disabled, starting, running, failed and failure reason |
| Address | Show the actual listener address with copy support |
| Credentials | Generate, copy and reset the token |
| Connection configuration | Provide copyable AI-client configuration |
| AI session list | Show each connected AI's identity, connection state and bound RustDesk sessions; see 4.3 |
| Approval | Configure whether AI acquisition requires human approval; required by default |
| Listening port | Default `21122`, editable and persistent |
| Persistence | Save enablement and related settings with consistent restart behavior |

### 4.1 Transport and SDK selection

| Item | Decision |
| --- | --- |
| Transport | Stateful MCP Streamable HTTP |
| MCP protocol version | Pin `2025-11-25` and negotiate only that version |
| Default address | `http://127.0.0.1:21122/mcp` |
| Listener scope | Initially bind only `127.0.0.1` |
| Official MCP Rust SDK | `rmcp = "=3.3.0"`, using its Streamable HTTP server and session management |
| HTTP integration | Axum `0.8` hosts the SDK service; `Cargo.lock` pins exact resolved dependency versions during integration |
| Authentication | Locally generated random Bearer token in the `Authorization` header |
| Lifecycle | Embedded in the GUI process, reusing the existing Tokio runtime |

- The selected SDK requires Rust at least `1.88`. Validate RustDesk 1.4.9 integration during the first build; selection alone is not evidence of build compatibility.
- Use one `/mcp` endpoint for requests, event streams and explicit session termination. Clients keep a GET/SSE channel for server messages such as standard `ping` and respond promptly. This initial connection requirement needs client-support validation during MCP integration.
- No legacy HTTP+SSE dual-endpoint compatibility layer or separate bridge-service process.
- Port conflicts produce startup failure with a reason; do not silently choose another port. The user changes the port and restarts. UI and copied configuration reflect the effective address.
- Every MCP HTTP method checks the token; logical-session IDs do not replace authentication. Generate tokens with cryptographically secure randomness; exclude them from URLs and ordinary logs.
- Validate `Origin`. Authenticated native clients without `Origin` may connect; reject disallowed origins even with a loopback-only listener.
- Store the token in protected local credential storage, using Keychain on macOS. Ordinary configuration contains only nonsecret settings.
- Token reset immediately invalidates old tokens and MCP sessions, cleaning up under section 3.8.

These decisions define transport, authentication and lifecycle boundaries. Tool definitions and service layering follow the companion design document.

### 4.2 Stop behavior

After MCP is disabled:

- Stop accepting new calls.
- Stop AI writes not yet executed.
- Release AI-held keys and mouse buttons.
- Keep normal GUI remote-control sessions usable; stopping MCP does not disconnect them.

Credentials must not appear in ordinary logs. Unauthorized calls cannot operate sessions.

### 4.3 AI session list

MCP settings show individual logical AI sessions so a person can see connected AIs, their remote-session bindings and who controls each session.

| Information | Display requirement |
| --- | --- |
| AI client | Client-supplied name/version; show unknown client when missing, without inferring model or chat-task identity |
| AI session identifier | A separate display ID per logical session, distinguishing same-name clients; do not expose protocol-session credentials |
| Connection state | Actual connected/liveness-checking state, distinct from remote RustDesk connection state |
| Time | Connection creation and most recent communication; communication includes liveness probes and is not labeled latest tool call |
| Bound remote sessions | Per AI: remote device ID, local RustDesk session ID and peer connection state; show “No remote sessions attached” when empty |
| Control mode | AI/human mode per binding, not one mode for the entire AI connection |
| Takeover request | Pending approval and remaining lifetime per binding |

- One row per logical MCP session; multiple HTTP requests or SSE connections by that AI do not duplicate rows.
- Update promptly on connect, attach/detach, control transitions and approval changes, synchronized with session banners and bridge state.
- When an AI leaves/is lost, perform section 3.8 cleanup and remove its entry without affecting others. Connection history is not required initially.
- Show an empty state when MCP runs with no clients. Clear old entries after MCP stop/token-reset cleanup.

## 5. Sessions and authentication

The bridge supports:

- Listing sessions while distinguishing **remote device IDs** and **local session IDs**.
- Opening visible remote desktop sessions.
- Attaching existing sessions without unnecessary duplicate connections.
- Submitting passwords and recognizing other authentication/human-confirmation requirements.
- Reading connection state, peer platform, actual permissions and display information.
- Disconnect/reconnect.
- Returning recognizable errors; existing display dimensions do not establish authentication success.

Connection state distinguishes at least:

- Connecting.
- Awaiting authentication.
- Awaiting human action.
- Awaiting first frame.
- Ready for operation.
- Disconnected.
- Closed.

## 6. Remote images

- Export **decoded remote images**, not screenshots of the local RustDesk window.
- Isolate caches by session and display.
- Return dimensions, display ID, frame sequence/time and other required metadata.
- Distinguish no image, old cache and a new image.
- Reading must not steal or corrupt GUI render buffers.
- Support multiple displays and display changes.
- Explicitly and consistently report cursor inclusion.

The initial version may prioritize CPU-readable pixels. GPU texture paths must provide usable pixel output or readback, avoiding sessions where the GUI works but the API can never capture.

Validate images after minimization, window switching and reconnection.

## 7. Keyboard, mouse and text

The bridge supports:

- Mouse movement, press, release, click, drag and scroll.
- Keyboard press/release and shortcuts.
- Text input.
- Correct mapping of image coordinates to the remote desktop.
- Multiple display origins, negative coordinates and image scaling.
- Ordered input within each session.
- Tracking AI-held keys/buttons.
- Checking both current control and permissions actually granted by the peer.

After human takeover, AI writes must fail explicitly, not disappear silently or wait for later control to be resent.

## 8. Remote terminals

- Open visible terminal sessions in the normal GUI.
- Create, list, resize and close terminals.
- Send input and read output.
- Obtain open results, errors and closure events.
- Bound output caches to prevent unlimited growth.
- Preserve raw output; higher layers decide whether to add processed text.
- Terminal writes follow control restrictions; AI may still read during human control.

**The underlying terminal is interactive and does not promise a separate exit code for every command.** Distinguish shell exit codes from per-command exit codes.

## 9. Later extensions

After the initial core workflow passes, consider:

| Category | Capabilities |
| --- | --- |
| File management | Directory reads, upload/download, create, delete, rename |
| File jobs | Progress, overwrite confirmation, completion, failure, cancellation, recovery |
| Port forwarding | Create, list and close TCP tunnels |
| System actions | Lock, restart, request elevation |
| Session controls | Resolution, quality, privacy mode, recording |
| Clipboard | Explicit read/write with clear relationships among sessions and the local clipboard |

Permission to call a capability does not mean the peer supports or authorizes it; the bridge must expose actual restrictions.

## 10. Code responsibilities

### Rust bridge

Responsible for:

- Session lookup and lifecycle.
- Calling official operation interfaces.
- Collecting connection, permission, image, terminal and file events.
- Caches and job state.
- Input ordering and human takeover.
- AI presence and control management.
- Converting underlying failures into recognizable results.

Keep it as independent of MCP as practical so the GUI, tests or other interfaces can reuse it.

### MCP layer

Handles official SDK initialization, transport, authentication, service lifecycle, argument validation, bridge calls and response conversion.

Do not reimplement remote-control logic in MCP tool handlers.

### Flutter layer

Handles MCP settings/runtime state, connection information, AI session lists, presence banners, control mode, read-only interaction, takeover buttons and approval UI.

Preserve existing manual remote-control flows where practical.

### Official entry points

These references were found during the earlier **1.4.9** inspection. Recheck call semantics at the pinned commit during implementation; this list is not completed bridge-validation evidence.

| File | Purpose |
| --- | --- |
| `src/flutter_ffi.rs` | Connection, authentication, keyboard/mouse, terminal, files and other controller operations |
| `src/ui_session_interface.rs` | `Session<T>`, `InvokeUiSession` callbacks and session operations |
| `src/flutter.rs` | GUI events, image buffers and terminal responses |
| `src/client/io_loop.rs` | Communication events and decoded-frame output |
| `flutter/lib/desktop/pages/desktop_setting_page.dart` | Settings-page reference entry |

Existing functions include `session_add_sync`, `session_login`, `session_send_mouse`, `session_input_key`, `session_open_terminal` and `session_send_files`. These are internal interfaces, not a stable external SDK.

References:

- [Official RustDesk source](https://github.com/rustdesk/rustdesk)
- [1.4.9 controller interface](https://github.com/rustdesk/rustdesk/blob/1.4.9/src/flutter_ffi.rs)
- [1.4.9 session abstraction](https://github.com/rustdesk/rustdesk/blob/1.4.9/src/ui_session_interface.rs)
- [YaoxinCS/RustdeskMCP](https://github.com/YaoxinCS/RustdeskMCP)
- [Yao bridge implementation](https://github.com/YaoxinCS/RustdeskMCP/blob/abbd7d1/src/agent_bridge.rs)
- [Official RustDesk 1.4.9 release](https://github.com/rustdesk/rustdesk/releases/tag/1.4.9)
- [Pinned baseline commit](https://github.com/rustdesk/rustdesk/commit/6c578292e8ebbbec708b76986ba8c4bc7c509747)
- [Official Rust SDK 3.3.0 release](https://github.com/modelcontextprotocol/rust-sdk/releases/tag/rmcp-v3.3.0)
- [SDK 3.3.0 toolchain requirements](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.3.0/Cargo.toml)
- [MCP Streamable HTTP specification](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)
- [MCP ping specification](https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/ping)

## 11. Initial acceptance criteria

The complete scope below belongs to the first version, including terminals and multiple displays. The initial acceptance environment is local macOS / ARM64.

### Core capabilities

- [ ] The official baseline builds and supports normal manual remote control.
- [ ] Settings start/stop MCP and show actual state, address and credentials.
- [ ] AI opens or attaches visible sessions and completes authentication.
- [ ] AI reads remote images, sends input and observes updates.
- [ ] Multi-display/scaled input coordinates are correct.
- [ ] Screenshots do not disturb GUI rendering.
- [ ] Terminal input, output, errors and closure state work.
- [ ] Disconnect, permission denial and authentication failure are explicit.
- [ ] Image behavior after minimize, window switching and reconnect is validated.
- [ ] The official peer and existing bootstrapper complete core flows without modification.

### Human/AI collaboration

- [ ] AI-created sessions default to AI control.
- [ ] Attaching existing sessions preserves control mode and displays AI presence.
- [ ] In AI mode, the GUI remote-interaction area is read-only with mouse/keyboard restrictions enforced.
- [ ] Human takeover is always available through an explicit control.
- [ ] Takeover stops queued AI writes without stuck keys.
- [ ] AI writes fail under human control while reads remain available.
- [ ] AI can release control; the GUI immediately restores manual operation.
- [ ] AI stays attached after release and can keep reading.
- [ ] AI acquisition obeys the approval-required setting.
- [ ] Approval is required by default; AI-created sessions still start in AI control.
- [ ] Approval expires after 60 seconds; repeated requests neither stack nor extend it; denial, cancellation and late approval are handled correctly.
- [ ] Multiple AIs connect simultaneously and bind different sessions; one AI can manage multiple sessions with independent control.
- [ ] Only one concurrent AI attachment to a session succeeds; others receive busy. Reattachment by the same AI preserves control mode.
- [ ] Human takeover or AI release preserves AI binding ownership; other AIs cannot bypass ownership checks.
- [ ] Detach/loss cleans only that AI's bindings. Others can explicitly attach afterward; old requests and late cleanup cannot affect the new binding.
- [ ] Detach, lost AI, MCP stop and token reset clear relevant presence and return human control while preserving GUI sessions.
- [ ] Peer reconnect or AI reattachment does not automatically restore control, old approval or old writes.
- [ ] GUI and bridge control state remain consistent.

### Service and access control

- [ ] Settings distinguish multiple logical AI sessions, including same-name clients; connections with no bindings remain visible.
- [ ] The AI session list accurately shows each bound device ID, local session ID, connection state, control mode and pending approval, synchronized with session banners.
- [ ] Concurrent HTTP/SSE connections from one AI do not duplicate entries. Departure removes only that AI; MCP stop/token reset clears old connections.
- [ ] Stopping MCP preserves normal GUI sessions while cleaning pending AI writes/input.
- [ ] Unauthorized calls cannot operate sessions.
- [ ] Credentials do not appear in ordinary logs.
- [ ] Saved configuration and client restart yield consistent service behavior.
- [ ] Port conflicts report actual failure without automatic port changes; copied configuration reflects the effective address.
- [ ] Token reset immediately invalidates old credentials and logical sessions.
- [ ] Long tool calls do not block liveness probes; temporary HTTP/SSE changes do not immediately detach; probe expiry performs cleanup.

## 12. Finalized scope and subsequent work

Initial product requirements and [MCP interface and service design](MCP-API-DESIGN.md) are jointly finalized. The second interface design is the implementation baseline: 20 capability-specific tools, agent_id and session_ref, optional passwords, bounded waits, input with attached images, and terminal writes with attached reads.

The documents respectively define product behavior and implementation contracts. Section 3.9 reflects that acquisition does not require completed authentication or an online peer. Later changes to finalized behavior/contracts require corresponding documentation updates and project-owner confirmation.

First build, Rust types/schema generation, SDK integration, implementation and live peer validation are subsequent execution tasks. Acceptance boxes remain unchecked; finalizing a design is not implementation completion.

## 13. Implementation sequence and estimate

Suggested order:

1. Build the official baseline.
2. Visible sessions and screenshots.
3. Working keyboard/mouse flow.
4. Human/AI takeover.
5. Terminals.
6. Complete settings and failure-case validation.

The earlier estimate of **3,000–4,000 handwritten implementation lines** is only a planning reference, excluding tests, generated code and upstream source. Reassess it against image paths, platform differences and control implementation.
