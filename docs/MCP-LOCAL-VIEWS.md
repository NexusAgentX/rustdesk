# Local views, scaling and window control

The MCP controller reuses desktop Flutter views, while the controlled client remains stock RustDesk. These settings change neither remote resolution nor MCP screenshot output dimensions.

## Tools

| Tool | Purpose |
| --- | --- |
| `rd_view_settings_get` | Read settings, current scaling, cursor options, fullscreen, toolbar pinning and local displays from the specified Flutter view |
| `rd_view_settings_set` | Specify one setting and explicit value through `change`, avoiding state reversal when retrying a toggle |
| `rd_view_window` | Show a window, close one view or open a selected display through the stock multi-display path |

When multiple views exist, provide `ui_session_id` from `rd_displays_get.local_views`. Reads do not require AI control. Writes require a ready desktop and AI control, checked again before Flutter executes them. Writes support existing `operation_id` deduplication.

## Settings and scope

- `scale`: `mode: original/adaptive/custom`. custom requires `percent: 5..1000`; other modes do not accept percent. Saves the peer preference and immediately updates the target view's canvas. Actual rendering scale is reported separately in `scale.render_scale`.
- `individual_windows`: `enabled` saves a peer preference governing whether future toolbar display selections open separate windows. Does not immediately create or close existing windows.
- `use_all_local_displays`: `enabled` saves a peer preference applied through stock behavior on the next new connection. Enabling requires at least two local displays and peer multi-display support. Disabling does not undo an existing window layout.
- `show_remote_cursor`, `follow_remote_cursor`, `follow_remote_focus`, `scale_cursor`: saved peer preferences. Support depends on peer platform/version, whether the cursor is embedded in video, the current single/all-display scope and whether multiple views exist. Queries distinguish configured values from effective state.
- Enabling `follow_remote_cursor` also enables remote cursor display. Turn following off before hiding the cursor. Disabling following preserves the cursor-display setting.
- `follow_ai_display`: persistent global setting reusing the initial AI-action display-following feature. Mouse movement alone does not trigger following; clicks and other coordinate actions follow the existing rules.
- `toolbar_pinned`: saves the global toolbar-pinning preference and immediately updates the target view. Other open views retain their own loaded state.
- `fullscreen`: controls the entire local OS window, including other tabs sharing it; it does not set remote desktop resolution.

All settings except `scale` use `enabled: true/false`.

## Window actions

`action` is an object: `{"action":"show"}`, `{"action":"close"}` or `{"action":"open_display","display_id":"1"}`.

- show displays and raises the target OS window.
- close closes only the target desktop view. Closing the last view disconnects the logical session through stock behavior. `rd_session_close` closes all views of that session.
- open_display reuses the stock display-window path and may activate an existing window or create a new one. Display IDs start at 0; the connection must be online and support the stock multi-view protocol.
- open/close return `delivery: sent, confirmed: false`; this does not confirm that the window opened/closed. Observe actual view counts and displays through `rd_displays_get`, and read `rd_session_get` if needed.
- Wait timeout returns pending. Do not blindly resend window actions; use operation deduplication and state queries.

## Validation record

- Stock Windows RustDesk 1.4.9 with two displays: original/adaptive/125% custom scaling readbacks passed without changing remote resolution. 135% custom scaling survived reconnect; original settings were restored afterward.
- Cursor display/following, focus following, cursor scaling, AI display following, separate-window preference, toolbar pinning and system fullscreen switches read back correctly. Hiding the cursor while following it was rejected as a conflict.
- With AI display following off, clicking the secondary display did not change local viewing. With it enabled, the same click switched to that display.
- The stock path created a second-display window and rendered its image. Retrying the same operation_id did not create another window. Multiple views without an explicit target produced an ambiguity error; cursor following was rejected in this state. Closing the new view kept the original session connected.
- Invalid scaling, nonexistent views and writes after releasing control were rejected; queries remained available after release.
- Only one local display was available, so enabling use_all_local_displays was explicitly rejected. The project owner chose to release the other features and defer the successful two-local-display path, tracked in #6.
- The new Flutter view-control file had no analysis issues; related existing files had only pre-existing notices.

- Automated tests: all 54 automation and 11 MCP tests passed. Live session, reconnect and terminal regression checks passed, as did strict signature verification for the installed application and the application extracted from the ZIP.
