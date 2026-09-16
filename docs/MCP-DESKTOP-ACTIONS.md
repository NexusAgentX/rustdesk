# Refresh, screenshots, screen lock, restart and lock after session end

Only the controller changes, using the stock protocol. All writes require AI control and retain session-reference validation and operation_id deduplication. Operations without remote acknowledgements explicitly return delivery=sent and confirmed=false.

## Tools

- `rd_screen_refresh(session_ref, display_id?, operation_id?)`: requests a refresh for the specified display, defaulting to primary. If an older peer does not support per-display refresh, requests all displays and returns effective_target. To observe a new frame, call the screenshot interface with an existing screenshot's frame_seq.
- `rd_session_lock(session_ref, operation_id?)`: sends the stock LockScreen system command; requires keyboard/mouse permission and a session outside view-only mode. Returns delivery status; inspect another screenshot to confirm the lock. Unlock through the normal operating-system login flow.
- `rd_session_restart(session_ref, operation_id?)`: sends the stock Windows/Linux/macOS restart command; requires restart permission. May terminate applications and disconnect; no restart-success acknowledgement exists. Use session queries and explicit reconnection to check recovery; disconnect is not proof of a completed restart. A portable client without a running service may need RustDesk reopened locally on the controlled machine.
- `rd_connection_settings_set` adds `change: {setting: "lock_after_end", enabled: true/false}`. Available for non-Android peers with keyboard/mouse permission outside view-only mode. Saves the peer preference and notifies the current peer. The stock peer locks after connection termination; reading back the local preference does not confirm the actual lock.

LockScreen and Ctrl+Alt+Del are one-shot protocol commands, not held keys; releasing control does not send them again.

## Connection errors

Default summaries and full details for open, reconnect and session queries return `connection.error`. Connection establishment failures (including an offline peer), transport errors, timeouts, peer disconnects and remote closure reasons retain the original error text, up to 1024 characters, and remain queryable after disconnect. A new connection attempt clears old errors; late errors from an old connection cannot contaminate the new one. A normal explicit disconnect does not invent an error. Error text is a connection-layer observation, not proof of whether a restart finished.

## Screenshots and saving

Reuses `rd_screen_capture` instead of adding a second screenshot tool:

| source | Semantics and limits |
| --- | --- |
| `decoded` (default) | Existing decoded video image; supports max_width/max_height and after_frame_seq, and returns input mapping snapshot_id. Read-only use is allowed under human control. Default wait_ms=0 allows immediate cache returns; disconnected caches are explicitly stale. |
| `remote_original` | The same original remote PNG request as the stock toolbar, requiring peer >=1.4.0 and AI control. Default wait_ms=10000, range 1..30000; scaling and frame sequence are not accepted. Returns no input mapping snapshot_id; remote_capture reports PNG dimensions, display and receipt time. A timeout means no result arrived; the sent request cannot be withdrawn. |

Both sources return a native PNG content block, capped at 8 MiB. Oversized original PNGs produce an explicit error; use decoded with size limits instead. Original screenshots are matched by request ID and logical session. Late responses neither contaminate the GUI screenshot cache nor open a save dialog.

Optional `save_path` (an absolute path ending in .png) saves **the same PNG bytes returned by the tool** on the controller and requires AI control. The parent directory must exist. Success returns path, byte count and SHA-256. A temporary file is written first, then the complete file is published without overwriting an existing file or destination symlink. An unchanged result without an image creates no file. Errors distinguish FILE_EXISTS and SAVE_FAILED. Cancellation cannot roll back an already-sent request or completed save.

Use operation_id for saving or original remote requests to avoid duplicate execution. Operation metadata is retained for 5 minutes; images use the shared 32 MiB/30-second cache. After image expiry, replay still returns the save result with image_status=expired and image_content_index=null, without saving or capturing again. Optional writes make the tool's annotation non-read-only, but decoded capture without saving remains available under read-only control.

## Validation record

- Concurrent original PNG requests to both displays of stock Windows RustDesk 1.4.9 returned 1920×1080 and 2560×1440 images with correctly matched requests, responses and save results. Saved files matched returned images byte for byte and by SHA-256.
- Saving decoded images and scaling to 1000 pixels wide passed, retaining input mappings. Replaying original screenshots with the same operation_id did not resend or resave.
- Existing files and destination symlinks returned FILE_EXISTS without changing content. Missing parent directories returned SAVE_FAILED. Relative paths, original requests mixed with scaling/frame sequence, and zero wait for original requests were rejected.
- Refreshing a selected display yielded a new frame through after_frame_seq. Explicit lock-after-end settings and restoration of the original preference passed.
- After control was released, read-only decoded capture remained available; original requests, saves, refresh, lock and restart were rejected.
- After keyboard/mouse and restart permissions were revoked locally on the controlled machine, lock, restart and lock-after-end returned PERMISSION_DENIED without changing preferences. Refresh and original screenshots still worked.
- An original request with a 1 ms timeout returned SCREENSHOT_TIMEOUT. Replay with the same operation_id retained that result; a subsequent new request succeeded. No GUI save dialog appeared for the late response.
- After restoring permission, an actual lock succeeded and both GUI and MCP images showed the Windows lock screen. Replay with the same operation ID did not resend. The desktop recovered after unlocking with the supplied OS login credentials.
- Enabling lock-after-end and disconnecting produced a visible Windows lock screen on reconnection. The peer preference persisted and was subsequently restored to off. Refresh, lock, restart and original screenshot requests were rejected while disconnected.
- Restart returned sent/confirmed=false, and replay with the same operation_id did not resend. The peer then disconnected; after reopening the portable client locally, the connection recovered. OS boot time changed from September 15 at 04:32 to September 16 at 16:42 (Beijing time), confirming an actual restart.
- Automated tests: all 59 automation and 12 MCP tests passed. New Flutter code had no analysis issues. Connection failure reasons were visible in open, default/full queries and reconnect; a normal Windows session returned error=null. Basic sessions, control, deduplication, screenshots, disconnect/reconnect, quality state and terminal regression checks passed.

### Client settings for large images

A tested lock-screen PNG was about 1.26 MB, or 1.68 MB after Base64 encoding. The Python test client's httpx2 default SSE event limit of 1 MiB caused SSEError, followed by SDK stream-recovery attempts. Raising the test client's SSE receive limit to 16 MiB worked; native Codex MCP also handled images of this size. The server retains its 8 MiB PNG cap; clients must allow for Base64 and JSON overhead.

### Window-management follow-up

Retesting found that closing the last desktop tab and immediately opening another session can leave a connecting placeholder with no registered GUI. Restarting the local client restores connectivity; repeated checks with an existing desktop tab worked. This is tracked separately in the window-management issue and must not be reported as the peer being offline.
