# MCP file transfer and text clipboard

Implements [file transfer #2](https://github.com/NexusAgentX/rustdesk/issues/2) and [text clipboard #4](https://github.com/NexusAgentX/rustdesk/issues/4). Only the controller changes; the stock protocol is retained.

## File-transfer connections

`rd_session_open` supports `kind: "file_transfer"` and opens a visible file-manager window. Lifecycle, authentication, control and session-reference rules match desktop sessions. `from_session_ref` can reuse an authenticated connection's token for the same peer. If the peer rejects the token, call `rd_session_authenticate` using the returned authentication challenge.

| Tool | Purpose |
| --- | --- |
| `rd_file_list` | `location: local/remote`, `path`, `include_hidden`; an empty remote path denotes the home directory |
| `rd_file_transfer` | `direction: upload/download`; 1..32 source/destination path pairs; recursive directory transfer |
| `rd_file_jobs` | State of jobs owned by the current binding |
| `rd_file_job_get` | Fetch a job, wait for changes using `after_revision`, and paginate directories with `offset/limit` |
| `rd_file_job_cancel` | Stop a job; partially written files may remain |
| `rd_file_conflict_resolve` | Explicitly overwrite or skip, optionally applying the choice to later conflicts in that job |

`destination_path` is the full destination path, not its parent directory. Remote paths use the remote OS format. The default conflict policy is `ask`; `overwrite` and `skip` are also available. Files and empty directories use the stock file protocol. Downloading empty directories requires peer support for `ReadEmptyDirs`; lack of support or timeout reports failure instead of silently dropping empty directories.

The transfer tool returns a job, not transfer completion. Query `rd_file_job_get`; terminal states are `completed/failed/cancelled/interrupted`, while `awaiting_conflict` requires a decision. The stock protocol reports a skipped file as `job_error: skipped`; this interface maps it to `completed`, `outcome: skipped`. Control revocation, invalid binding or disconnect interrupts active jobs. Cancellation/interruption cannot roll back files or directories already written. After disconnect, explicitly call `rd_file_job_resume` once reconnected with control restored; see [file management and recovery](MCP-FILE-RECOVERY-DISPLAYS.md).

Directory responses have no native request ID, so only one directory-metadata request may be outstanding per session. Each response returns at most 1000 entries; use `next_offset` to continue reading the cache. Cache limits are 100000 entries/8 MiB per directory and 32 MiB total. At most 128 jobs are retained; records completed over 300 seconds ago are removed when adding or listing jobs. Each recursive transfer handles at most 4096 empty directories. Waits are capped at 30000 ms; timeout does not mean an empty directory or a successful transfer.

## Text clipboard

| Tool | Purpose |
| --- | --- |
| `rd_clipboard_settings_get/set` | Read/set an explicit `enabled` value; scope is `peer_preference`, applied immediately and persisted for the peer |
| `rd_clipboard_read` | Return the most recently received remote text, source, receipt time, connection epoch and revision; optionally wait for a revision change |
| `rd_clipboard_write` | Send text to the peer, with optional `paste: true` and `delay_ms` |
| `rd_clipboard_type` | Send keystrokes for specified text; omit `text` to read the local text clipboard |

The stock protocol cannot actively read the remote clipboard or acknowledge a completed write. `read` always reports source `remote_sync`. If no text has arrived for the current binding and connection, it returns `known: false` rather than substituting local clipboard contents. Cache lifetime is 300 seconds, with at most 32 sessions and 1 MiB per text value. Identical text does not create a new revision. Whether clearing the remote clipboard sends a synchronization event depends on the stock implementation; the cache is not a live query result.

`write` accepts up to 1 MiB of UTF-8, including an empty string. `delivery: sent` means only that the message was handed to the connection. Optional paste waits 200 ms by default and uses Command+V on macOS or Ctrl+V elsewhere. The delay is not an application receipt acknowledgement; paste failure after a write reports partial execution. `type` uses the existing keyboard path, has a 16 KiB limit and does not depend on the text-sync switch. CRLF/LF/CR become Enter and tabs become Tab; actual behavior depends on the focused control.

Text-sync reads and writes require remote clipboard permission and locally enabled synchronization, outside view-only mode. Keystroke input additionally requires keyboard permission. Writes require current AI control; queries do not take control from the human. Caches from different bindings or connection epochs cannot be mixed. See [native file clipboard](MCP-FILE-RECOVERY-DISPLAYS.md) for file clipboard interfaces.

Windows text keystrokes are paced at approximately 10 ms per character to avoid duplicate characters from consecutive stock Unicode injection in modern Notepad. Automatic pacing plus explicit waits must not exceed 30 seconds; excessive input is rejected before sending and must be split or pasted through the clipboard. Ordinary keys and modifiers both use physical key codes to avoid mixing injection methods in rapid shortcuts.

## Validation

- 47 automation tests and 11 MCP tests passed, including directory pagination, binding isolation, interruption wakeups, dangerous-path rejection, Unicode/empty-text caching, physical modifiers and newline handling.
- Stock Windows RustDesk 1.4.9: bidirectional directory/multi-file transfers passed for Chinese/spaced paths, hidden files, binary files, empty files and empty directories. Downloads were verified by SHA-256.
- Manual/automatic overwrite and skip, cancellation, control revocation, disconnect and post-reconnect job state passed; intentional skips were not reported as failures.
- Remote PowerShell independently verified Chinese multiline and empty clipboard text. Sync switching, wait timeout and cache isolation after rebinding passed.
- Remote Notepad testing covered specified-text keystrokes, local-clipboard keystrokes and automatic paste, with copied-back text verified character by character. Clipboard/keyboard rejection in view-only mode, capability reporting and restoration of original settings passed.
- Initial session, screenshot, control, operation deduplication/isolation, disconnect/reconnect and terminal regression checks passed, along with macOS ARM64 build, Flutter packaging and signature verification.
- After clipboard permission was disabled locally on the controlled machine, reads, writes and enabling synchronization returned `PERMISSION_DENIED`; capability queries marked it unavailable while keyboard/mouse permission remained valid. The stock connection manager blocks remote clicks that change permissions, so this test used a person locally changing the permission. Capability queries and clipboard reads recovered after permission was restored.
