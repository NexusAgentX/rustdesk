# MCP capability and operation inspection

Implements [implementation task #1](https://github.com/NexusAgentX/rustdesk/issues/1). Only the controller changes; the stock controlled client does not need an update.

## Capability inspection

`rd_capabilities_get({"session_ref":"r_…"})` is read-only and is also available during human control.
It returns `peer` (version, platform, authentication, connection state and known permissions), `capabilities` and `contract`.

Each capability includes:

- `implemented`: implemented by the current MCP service; features available only through the GUI are not counted as MCP capabilities.
- `supported`: whether the session type and negotiated features support it; `null` means undetermined.
- `allowed`: permission status; `null` does not mean authorized. Operations requiring remote permissions check them again at execution time.
- `available`, `blockers`: current availability and reasons, such as `support_unknown`, `unsupported_for_session`, `permission_unknown`, `permission_denied`, `human_control`, `control_transition`, `not_ready` and `session_closed`.
- `tools`, `scope`, `requires_ai_control`: associated tools, scope and whether AI control is required.

These values describe the instant of the query. They are not atomic authorization credentials and do not guarantee an available frame, terminal instance or application result. As capabilities are implemented, document their version, platform, permission, driver and negotiation requirements; do not infer permissions from a version alone.

## Operation inspection

Write calls can supply `operation_id` to use the existing deduplication mechanism. Within the same MCP client, the same ID and parameters execute only once; different parameters return `OPERATION_CONFLICT`.

`rd_operation_get({"operation_id":"example","wait_ms":1000})` only queries; it does not resend the write and needs no additional session reference. It can query only records created by the current logical MCP client. Existing read-authorization checks still apply when a binding becomes invalid. Operation inspection through `rd_session_get` remains available.

- `wait_ms` ranges from 0..30000, defaults to 0, and waits only for local tool execution to finish.
- `data.operation` contains the original call result, including its `operation` metadata; the outer `completed` means only that the query completed.
- Metadata `tool` identifies the original tool. Original arguments and authentication credentials are not returned.

| execution_state | outcome | Meaning |
| --- | --- | --- |
| running | pending | The local tool is still executing |
| finished | reported | The local call finished; the strength of the evidence follows the original tool's contract |
| finished | unknown | The original call ended with pending; no final remote result is available |
| finished | partial | The original tool reported partial execution |
| cancelled | unknown | The call was cancelled; operations already sent cannot be withdrawn |
| failed | unknown | The tool reported an error; this does not guarantee that nothing changed remotely |

For example, after a disconnect request's bounded wait times out, the operation record retains its original pending result. Use `rd_session_get` to observe the actual connection state. Querying neither rewrites an old pending record as remote success nor disconnects again. Likewise, inspect terminal state through terminal queries. Future file tasks should expose their own progress and completion events.

Completed records are retained for 300 seconds, with at most 256 per client. Images and attached output have a separate 30-second cache. Records are cleared when the client ends. Missing, expired or another client's IDs return `OPERATION_EXPIRED`.

## Contract for subsequent settings interfaces

Settings should expose actual values and accept explicit values (for example, `enabled: true`), rather than require callers to guess the current state before toggling. Each setting documents its scope (`session`, `peer_preference`, `global` or `local_window`), whether it takes effect immediately, and whether it persists. This increment defines the contract; individual settings interfaces accompany their respective features.

Writes continue to check the binding, connection epoch and AI control. References must be refreshed after reconnection; queued writes must not continue after control is revoked. Errors retain `code/message/retry/details`. Passwords, verification codes and original authentication arguments are excluded from errors and operation records.

## 0.1.1 validation record (2026-09-16)

- macOS ARM64 release build, Flutter packaging and post-installation signature verification passed.
- 11 MCP tests and 40 automation tests passed, including timeout/expiry, client isolation, duplicate requests and conflicts, unknown/denied permissions, and human-control state.
- Live testing against stock Windows RustDesk 1.4.9 with two displays covered capability/version queries, screenshots, rejection of the wrong session type, input rejection after human takeover, running-operation queries, deduplication, conflict rejection, cross-client operation isolation, interruption of waits on control revocation, unknown outcomes after disconnect, reconnect authentication and first-frame recovery.
- Live terminal testing covered capability inspection, listing, reading, resizing, terminal closure and session closure.
- Tests waited for actual first-frame arrival after authentication and screenshot-cache readiness after rebinding; authentication completion was not treated as image readiness.
- The Python MCP SDK printed a termination notice for HTTP 202 after the service accepted DELETE. Service cleanup and subsequent rebinding worked, and HTTP session-isolation tests passed.
