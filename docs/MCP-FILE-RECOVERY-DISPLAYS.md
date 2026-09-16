# MCP file management, file clipboard and display management

Implements [file management and recovery #3](https://github.com/NexusAgentX/rustdesk/issues/3) and [display management #5](https://github.com/NexusAgentX/rustdesk/issues/5). The controller reuses the stock protocol; the controlled client needs no changes.

## File management and recovery

- `rd_file_manage`: `create_directory`, `rename`, `remove_file`, `remove_directory`, with `location` set to `local/remote`. Requires absolute paths and rejects roots and `.`/`..` components. Rename uses a single `new_name` within the same parent directory.
- Deleting a nonempty directory requires explicit `recursive: true`. The controller enumerates entries, including hidden files, and deletes children before parents without following enumerated symlinks. Recursion is capped at 10000 entries and 64 levels. Unlike stock recursive empty-directory deletion, this interface checks each step's result.
- Management operations return queryable jobs recording completed steps. Cancellation/interruption does not roll back changes. Stock rename may replace an existing destination according to OS semantics.
- `rd_file_job_resume`: resumes a retained `interrupted/failed/cancelled` transfer within the same binding, creating a new job and retaining the old record. Requires stock resume support on the peer (1.4.2 onward), a ready connection and AI control.
- Resume determines the byte offset using stock size/modification-time summaries and any remaining partial file, not a content hash. Cancellation may already have removed partial files. Without reusable partial data, transfer restarts or enters conflict handling. Completed jobs are retained for 300 seconds, do not survive controller process restart, and are not automatically resumed in the GUI behind the control checks.

## Native file clipboard

| Tool | Behavior |
| --- | --- |
| `rd_file_clipboard_get/set` | Query or explicitly set `enabled`, a peer preference independent of the text-sync switch |
| `rd_file_clipboard_copy` | With `paths`: set the local system file clipboard and publish the file list; without `paths`: send Copy to the current remote selection |
| `rd_file_clipboard_paste` | Without `local_directory`: send Paste to the remote focus; with it: download the current binding's most recent file-clipboard offer into an existing local directory |
| `rd_file_clipboard_cancel` | Cancel the current binding's local native paste, not the remote application's Paste |

Requires a desktop session, file and keyboard permissions, enabled file copy/paste and a session outside view-only mode. Remote selection depends on file-manager focus; receiving a stock file list requires the corresponding local desktop tab to be active. As with the toolbar, the file clipboard is a **global resource on the local machine**, affecting local applications and other enabled connections.

`paths` contains 1..128 existing absolute file/directory paths. Direct local paste currently requires macOS with native file-clipboard support enabled at build time; other platforms explicitly return unsupported. Remote file offers expire after 300 seconds and cannot be reused across binding or connection epochs.

Local paste state appears in `get.settings.local_paste`, including job ID, state, progress and error. Only one paste can be active globally. Waits are capped at 30000 ms; the background job limit is 10 minutes. Stock behavior automatically disambiguates duplicate destination names. Cancellation, loss of control or connection invalidation stops the job; completed files may remain.

Sending Copy/Paste proves only that input was sent, not that the application finished. Default Paste delay is 500 ms, up to 30000 ms. In sequential GUI actions, explicitly wait between activating a window, entering an address bar and selecting files, and independently inspect destination files.

## Displays and resolutions

| Tool | Behavior |
| --- | --- |
| `rd_displays_get` | Display IDs, names, positions, capture dimensions, scale, original resolutions, known modes, local windows and capture selection |
| `rd_display_modes_get` | Read cached peer modes without side effects; unknown modes return `known: false` |
| `rd_display_select` | `display_id` is a numeric string or `all`; `target: capture/local_view` |
| `rd_display_resolution_set` | `mode: set/restore_original/fit_local`; `set` requires width and height |
| `rd_virtual_display_set` | `action: add/remove/remove_all`; calls the stock virtual-display operation |

Capture selection applies to the current AI binding; local-view selection applies to one window. Multiple windows require `ui_session_id`. Actual subscriptions are the union of AI and GUI requirements; a later `rd_screen_capture` also adds its requested display. Explicit local-view selection does not depend on the follow-AI-action-display switch.

The stock peer reports supported modes on connection and single-display selection. If modes are unknown, switch away and back through `local_view`; reselecting an already selected display does not make the stock peer report modes again. With only one display, reconnect to obtain the connection-time report. Reading modes never silently switches displays. MCP single-display selection does not apply saved custom-resolution preferences. Changing selection, resolution or layout invalidates old screenshot coordinates; capture again.

Physical-display custom settings can choose only peer-reported modes. Restore may use the peer's original dimensions without a supported-mode cache. An original resolution of `0x0` is the stock virtual-display custom-mode marker, not a restorable physical resolution. `fit_local` uses the controller's primary display dimensions and requires an exact match in a physical display's supported list. It does not silently choose a near match. `scale` distinguishes capture pixels from OS mode dimensions.

Resolution and virtual displays affect the remote machine and require AI control, keyboard permission and a session outside view-only mode. `confirmed: true` means matching state was observed. Timeout returns `pending`, not proof of failure or permission to blindly repeat the operation.

### Virtual displays and drivers

The peer must be an installed Windows client reporting `rustdesk_idd` or `amyuni_idd`, with privacy mode off. RustDesk IDD add/remove uses `index: 1..4`. Amyuni takes no index, adds/removes one display per call and supports at most four. remove_all takes no index either.

The stock protocol does not reliably report driver installation, so `driver_installed` remains unknown. **Adding a virtual display may trigger stock driver installation**; the project owner explicitly approved this, superseding the initial restriction against implicit driver installation. Results are observed through peer-reported virtual-display counts or IDs; unconfirmed results are not claimed as success.

## Validation record

- Automated tests: 53 automation and 11 MCP tests passed. Flutter analysis reported zero errors/warnings (8 existing info notices). The final ARM64 application extracted from the package and the installed copy both passed strict signature checks; live session, reconnect and terminal regression checks passed.

- Stock Windows RustDesk 1.4.9 with two displays: 1920×1080 and 2560×1440, with the second display at `(1920, -563)`.
- File management: local/remote directory creation, rename, rejection of nonempty deletion without recursion, and recursive deletion including hidden files passed. A local root symlink was rejected; child symlinks were removed without touching external targets.
- A 64 MiB download was interrupted at 1,179,648 bytes, retaining a partial file. A new resume job after reconnect completed with a SHA-256 matching the source.
- File clipboard: bidirectional transfers of Chinese/spaced filenames, directories, empty files and empty directories passed. Remote checks used SHA-256; local checks compared bytes and directory structure. Disabling, invalid paths, rejecting old offers after rebinding, cancellation and interruption on release of control passed.
- Displays: independent AI capture/local-view selection, single/all-display selection, actual dual-display GUI rendering, negative-coordinate screenshots and rejection of old coordinates after selection/resolution changes passed. Queries remained available after control release; writes were rejected.
- Secondary display 2560×1440 → 3840×2160 → original 2560×1440 passed. Unsupported sizes and fit-local requests without an exact match were rejected. Primary-display viewing and original resolution were restored.
- Regression checks covered retaining cached modes when layout messages follow mode messages, and preserving installation state across incremental platform updates. Restoring original dimensions remained possible when supported modes were unknown.
- Windows APIs confirmed effective DPI values of 144 and 96 (150%/100%). RustDesk capture `scale` was 1.0 on both and does not represent Windows desktop scaling. Mouse movement to 10 points across both displays used scaled screenshot coordinates, followed by independent DPI-aware `GetCursorPos` readback. Error was at most 1 pixel per axis, including the display with negative coordinates.
- **Deferred**: the peer lacked an installed system service and reported no virtual-display support. The project owner chose to release the other features and defer live testing in a supported environment. No service or driver was installed and no virtual display was added/removed. Parameter and support checks have automated coverage; successful operations and custom virtual-display resolutions still need a suitable environment.
