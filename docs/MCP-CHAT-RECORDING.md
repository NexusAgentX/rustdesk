# Text chat and session recording

MCP 0.1.8 adds `rd_chat_send`, `rd_chat_read`, `rd_recording_set` and `rd_recording_get`, bringing the total to 63 tools. It uses the stock protocol and recorder; the controlled client needs no changes.

## Text chat

- `rd_chat_send(session_ref, text, operation_id?)`: sends 1..16384 bytes of UTF-8 text through a ready desktop connection. Requires AI control, independently of keyboard/mouse permission. Returns sent; the stock protocol has no delivery/read receipts.
- `rd_chat_read(session_ref, cursor?, max_messages?, wait_ms?)`: reads the visible core session's in-memory message cache. It includes observed incoming messages and messages successfully sent by the local GUI/MCP. It neither reads old GUI history nor writes to disk or accesses the system clipboard.
- Returns 50 messages by default, with a range of 1..100. The default wait is 0, up to 30 seconds. `next_cursor` supports incremental reads; `gap` reports cache eviction. A cursor from another session or outside the valid range is an error.
- Each session retains at most 256 messages / 256 KiB. Each incoming message is limited to 16 KiB, with UTF-8-safe truncation reported by `truncated`. Messages preserve order and connection epoch. Reconnection retains the current session's cache; disconnected reads immediately return cached messages. The cache is reclaimed with the session after closure.

## Session recording

- `rd_recording_set(session_ref, enabled, wait_ms?, operation_id?)`: explicitly starts/stops the stock screen recorder and requires AI control. Starting requires remote recording permission; stopping does not require that permission to remain enabled. Starting requests a keyframe. Wait defaults to 10 seconds, with a maximum of 30 seconds; a timeout does not cancel recording.
- `rd_recording_get(session_ref)`: reads local recording state, the current output directory and at most 64 file records. Each file includes its path, display, recording epoch, stock frame-write event count (not total media frame count), final byte count and writing/finalized/discarded/failed state.
- Confirming a start requires observation of an actual video-frame write. Confirming a stop requires the local recording switch to be off and every active writer to be finalized. Still inspect each file's state and error.
- Uses the controller's stock video directory without changing the global directory setting. Output files remain on the controller; file records remain in session memory. Codec/resolution changes or timestamps moving backward (for example, after a video-stream refresh) may split the output into multiple files.
- Records video only, without audio. The stock recorder deletes files shorter than one second or containing no valid video; the interface explicitly marks them discarded instead of claiming they are usable.
- Preserves stock recording behavior: recording continues after AI control is released and is finalized on explicit stop, remote recording-permission revocation or connection termination. Both local and remote sides display the stock recording status.
- Creation/write/muxing failures expose errors. Late results from an old connection or recording file cannot overwrite a new recording's state.

## Validation

69 automation tests and 12 MCP tests passed. Coverage includes chat ordering, cursors, connection-epoch changes, UTF-8 truncation and eviction, as well as recording finalization, discarded files, failure retention, old-epoch isolation and history limits.

Live testing against stock Windows 1.4.9 with two displays verified:

- Sending multiple Chinese messages, ordering, operation deduplication, incremental reads and empty waits; receipt was visually confirmed in the controlled client's chat window.
- Starting/stopping recording on two displays, operation deduplication, output files and short-clip discard. All valid outputs from this round were inspected with ffprobe and fully decoded with ffmpeg.
- Chat and recording writes are rejected under human control; reads remain available.
- Disconnect turns recording off and finalizes writers. Chat remains readable offline and survives reconnection; recording does not restart automatically.

Regression checks passed for basic sessions, control, asynchronous operation deduplication, connection-quality state across disconnect, and terminal open/close/read/resize. Multi-window reads explicitly selected `ui_session_id`.

The controlled client's connection manager blocked remote interaction with chat replies and permission switches. Under the project owner's instruction to defer blocked live tests, successful incoming remote chat and recording-permission revocation remain untested on a live peer. Stock protections were not bypassed. Automated tests cover retaining state after recorder-reported failures; disk failures were not induced on the test machine.
