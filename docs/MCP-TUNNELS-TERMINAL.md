# TCP tunnels and terminals

MCP 0.1.9 adds the `tcp_tunnel` session type and `rd_tunnel_list`, `rd_tunnel_add`, `rd_tunnel_remove` and `rd_tunnel_authenticate`, bringing the total to 67 tools. Only the controller changes; the controlled client remains stock RustDesk.

## TCP tunnels

1. `rd_session_open(peer_id, kind="tcp_tunnel")` opens a visible stock port-forwarding window and prepares a local manager. `ready` means only that the local manager is ready; `connection.authenticated=false`. Each tunnel reports its own remote authentication and connection state.
2. `rd_tunnel_add(session_ref, local_port, remote_host, remote_port, password?, operation_id?)` binds the specified port on `127.0.0.1`. Ports range from 1..65535; stock special RDP/automatic port 0 is not used. Confirmation proves only successful local binding.
3. Only when a local application connects does the tunnel use the stock PORT_FORWARD protocol to connect and authenticate to the controlled client. `remote_host` is resolved/accessed from that machine; for example, `127.0.0.1` denotes the controlled machine itself. Tunnels carry arbitrary TCP bytes without interpreting HTTP, commands or other application protocols.
4. `rd_tunnel_list` queries status, active connection count, cumulative successful connections, authentication challenges and the latest error. States include listening, connecting, awaiting_auth, authenticating, closing and closed; abnormal task/cleanup failure is failed. While a listener remains active, remote-target failures retain `last_error`. Historical successes do not prove the target is currently available.
5. When a password/2FA challenge is required, call `rd_tunnel_authenticate` with that tunnel's current `tunnel_id`, `challenge_id` and credentials. Challenges cannot cross tunnels and credentials are not broadcast to other listeners. Sending a request is not successful authentication. Stock TCP sessions do not support OS-account login.
6. `rd_tunnel_remove` closes the listener and all active forwarding connections, including those awaiting authentication, then confirms after local cleanup. Session closure cleans MCP tunnels before closing the GUI.

### Lifecycle and boundaries

- AI control is required to add, remove or authenticate; queries remain available under human control. Releasing control, detaching, client expiry, disconnect or session closure closes MCP listeners and active connections. Reconnection does not restart them.
- At most 16 MCP listeners per session and 32 active connections per listener. The latest 64 records, including closed entries, remain in memory.
- Each listener has isolated login state. A password may be provided when adding it, or `from_session_ref` during session open may try to inherit a still-valid stock connection token from an authenticated connection to the same peer. A desktop authenticated with a one-use MCP password clears that credential after login and usually has no reusable token. A listener can still be established, but a password must then be supplied through `rd_tunnel_add.password` or a later challenge; passwordless connectivity must not be assumed. TCP sessions reject `session_open.password` to avoid implicitly sharing credentials among listeners.
- Explicit login state remains only in listener memory for subsequent connections and is released on close. An inherited connection token remains in the current GUI session's memory until that session closes. MCP does not persist credentials to disk or modify saved GUI forwarding configuration. Deduplication records do not store original arguments.
- `PORT_IN_USE` explicitly reports local port conflicts. Only loopback listeners are supported. Connections not made directly by IP require the stock encrypted channel; MCP cannot authorize an insecure connection.
- Saved GUI forwarding configuration is listed separately as `configured_gui_forwards`; configuration is not treated as runtime state. MCP status, deletion and lifecycle guarantees in this increment apply to MCP-created tunnels. Existing GUI actions retain stock behavior.
- The visible window shows MCP tunnel ports, targets, states and errors, and retains the human-takeover control.

## Terminal reuse

Continues using `rd_terminal_list/create/read/write/resize/close` without adding another execution interface.

- Terminals are interactive byte streams, preserving ANSI, UTF-8 and raw bytes with text/base64 representations.
- Cursors bind to terminal instance, connection epoch and byte offset. Repeated reads do not consume the cache; overwritten output returns `OUTPUT_GAP`.
- `requested_size` reports the requested dimensions. Process exit codes belong to the whole shell, not invented per-input exit codes or separate stdout/stderr streams.
- Stock platform, version, terminal support and authentication requirements remain authoritative. Explicitly unsupported creation returns `UNSUPPORTED`; incomplete negotiation/authentication returns `NOT_READY`, avoiding invalid tabs. Disconnected terminals cannot accept further writes.

## Validation

74 automation tests and 12 MCP tests passed. New coverage includes port exclusivity, cancellation before binding, isolated challenges, port release on handoff, rejection after closure, and history eviction while preserving a long-lived listener across 70 short-lived listener create/close cycles. Terminal support negotiation, explicit lack of support and preflight rejection after disconnect have automated coverage.

Live testing against stock Windows 1.4.9 verified binary echo (including null and non-UTF-8 bytes), add/remove deduplication, port conflicts, incorrect-password challenges and correction, invalid-challenge rejection, remote-target failure reporting, and cleanup of listeners/active connections on removal, control handoff, disconnect and window closure. Listeners remained closed after reconnect. When the source desktop's one-use credential had been cleared, the tunnel returned a password challenge; explicit authentication then completed echo on the same local connection.

Terminal live testing passed for Chinese/ANSI raw bytes, dual representations, repeated cursor reads, input deduplication, creating a second terminal, resizing, actual shell exit code 7, write rejection after disconnect and cached reads after closure. The temporary echo service ran in the stock terminal and listened only on the controlled machine's loopback interface. It was shut down after verifying process identity, and port closure was confirmed.

The final package also passed basic session, control, deduplication, screenshot and quality-setting disconnect/reconnect regression checks. Signature verification passed for the extracted application.

Successful 2FA, actual rejection by older peers and remote tunnel-permission revocation lack suitable test environments and were deferred with the project owner's authorization. The controlled RustDesk client was not modified, no service was installed and permission protections were not bypassed.
