# Input blocking, secure attention, elevation, OS login and privacy mode

Uses the stock protocol and changes only the controller. MCP 0.1.7 adds six tools; headless account authentication reuses `rd_session_authenticate` rather than introducing a separate authentication interface.

## Interfaces

- `rd_security_get(session_ref)`: reads SAS support, available privacy implementations, permissions, portable-service status and peer feedback for input blocking/privacy/elevation. A null value is unknown, not off. After disconnect, `fresh=false`; reconnection clears old observations. `sequence` is the local state revision and `observed_at` is the receipt time of peer feedback.
- `rd_input_block_set(session_ref, enabled, wait_ms?, operation_id?)`: controls physical keyboard/mouse blocking on Windows. Enabling requires keyboard and input-blocking permissions. Disabling can be requested for recovery even after permission revocation, although the stock peer may reject it. Stock success has no acknowledgement; only failures are reported. With no feedback, the result remains unknown, and another explicit set is allowed after the observation window. Default wait is 10 seconds, range 0..30000 ms.
- `rd_privacy_set(session_ref, enabled, implementation?, wait_ms?, operation_id?)`: enabling explicitly chooses an implementation from the query result and checks peer support, keyboard/privacy permissions and toolbar display restrictions. Disabling may omit implementation, using the observed implementation or stock default. May change remote display topology. Default wait is 10 seconds.
- `rd_ctrl_alt_del(session_ref, operation_id?)`: sends the stock dedicated command to Linux or Windows reporting SAS support. Requires keyboard/mouse permission and a session outside view-only mode. It is not an ordinary three-key shortcut.
- `rd_session_elevate(session_ref, request, wait_ms?, operation_id?)`: requests Windows portable-client elevation. `request` is `{mode:"direct"}` or `{mode:"logon",username,password}`. An installed peer or one already reporting SAS/a running portable service returns NOT_NEEDED. Direct elevation may require local UAC confirmation on the controlled machine; the tool does not bypass it. An empty elevation response means the process was initiated; a portable-service-running notification confirms startup.
- `rd_os_password_input(session_ref, password, activate?, operation_id?)`: types a one-use password into the focused OS login field and presses Enter. It neither saves nor returns the password. Default activate=false; true first presses Enter and waits 1200 ms to open the login field. Inspect the login screen before calling to avoid typing into another application. Uses existing serialized input, length/duration limits and control revocation. Returns sent, not proof of successful login.

For headless Linux/OS-account challenges, read `auth_challenge` through `rd_session_get` and call `rd_session_authenticate` with `credentials:{kind:"os_login",username,password}`. If challenge fields include connection_password, explicitly supply the RustDesk connection password as `connection_password`. Account login, OS password entry, elevation and the RustDesk connection password are separate flows; sensitive values are excluded from results and operation records.

## Feedback, concurrency and recovery

Input blocking and privacy use `BackNotification`; elevation uses `ElevationResponse` and `PortableServiceRunning`. Optimistic GUI checkbox state is not used as evidence.

- Current control and applicable permissions are checked before accepting a write and again before sending it.
- `confirmed=true` means matching peer feedback was received. Ctrl+Alt+Del and OS password entry have no success acknowledgements and always report only delivery.
- Stock notifications lack request IDs, so only one enable request of each category may await a result. Privacy/elevation retain pending state after timeout. Input blocking reports failures only, so its request lock is released after the observation window; silence is never treated as success. An explicit disable can request recovery; reconnection clears old pending state.
- Remote failures return REMOTE_ACTION_FAILED with outcome distinctions. Insufficient permission returns PERMISSION_DENIED; unsupported actions return UNSUPPORTED. Elevation errors do not echo peer-supplied free text that may contain account information.
- Releasing/revoking AI control sends disable requests for AI-enabled input blocking and privacy. Explicit disconnect performs cleanup first. Sending disable does not prove recovery; inspect fresh peer feedback. A network break, controller crash or peer rejection can prevent cleanup; actual disconnect handling remains the stock peer's responsibility.
- An observed successful disable is not sent again on a control transition. Feedback from an old connection cannot modify new state.

## Validation

2026-09-16: 64 automation tests and 12 MCP tests passed, as did the macOS ARM64 release build and signature verification. The peer was stock Windows 1.4.9 portable, with two displays.

Live testing verified:

- Discovery of all six new tools, 59 total; capability queries distinguish unknown, unsupported, permission and human-control conditions.
- Repeated explicit input-block enable/disable, deduplication, unknown outcomes without acknowledgements, and allowing the next request after the observation window. A remote disable failure returned REMOTE_ACTION_FAILED/off_failed. Actual physical input blocking was not confirmed by a person at the peer.
- Privacy exclude_from_capture enable/disable, deduplication and successful disable feedback after control release. Disconnect set fresh=false; reconnect cleared observations and subsequent switching produced new feedback.
- Input blocking, privacy, elevation, Ctrl+Alt+Del and password entry all returned HUMAN_CONTROL during human control.
- A one-use password was entered after inspecting the Windows password field, and the normal desktop was observed afterward. Repeating operation_id did not type again. Another attempt disconnected without subsequent frames and was not counted as a successful login.
- With a portable service already running, both elevation modes returned NOT_NEEDED. With SAS=false, Ctrl+Alt+Del returned UNSUPPORTED.
- Regression checks passed for basic sessions/control/deduplication/screenshots, quality state across disconnect, and terminal list/create/read/resize/close.

Deferred with the project owner's authorization: successful headless Linux login, both elevation paths on non-elevated Windows, successful Ctrl+Alt+Del on a SAS-capable peer, physical confirmation of input blocking and recovery after disconnect, and live permission revocation. The connection manager's separate protection prevented remote clicks on permission buttons even with remote configuration changes enabled; queries confirmed that permissions had not changed. Automated tests cover retaining denial state, isolating old-connection feedback, concurrent-request tokens, error states and cleanup messages on a real stream.

The stock connection manager's local protection, elevation/UAC conditions and missing protocol acknowledgements were not bypassed. Test limitations are not recorded as successes.
