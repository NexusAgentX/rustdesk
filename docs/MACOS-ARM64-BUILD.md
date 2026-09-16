# macOS ARM64 build baseline

This stage validates the official RustDesk 1.4.9 Flutter client before integrating MCP. See [controller requirements](MCP-CONTROLLER-REQUIREMENTS.md) for product scope and [MCP design](MCP-API-DESIGN.md) for interface contracts.

## Pinned inputs

| Item | Version or commit |
| --- | --- |
| RustDesk | `1.4.9` / `6c578292e8ebbbec708b76986ba8c4bc7c509747` |
| hbb_common | `7e1c392c62d39c364127307cd408421dd5f8cfb0` |
| Flutter | `3.24.5` / `dec2ee5c1f98f8e84a7d5380c05eb8a3d0a81668` |
| Baseline Rust toolchain | `1.81.0`, from this version's official macOS CI |
| vcpkg | `120deac3062162151622ca4860575a33844ba10b` |
| vcpkg triplet | `arm64-osx`, using the repository's existing manifest and overlay ports |
| NASM | `2.16.03`; official CI explicitly requires 2.x |
| Flutter Rust Bridge generator | `1.80.1`, with `uuid` enabled |
| cargo-expand | `1.0.95` |
| macOS deployment target | `12.3`, matching official ARM64 CI |
| Rust features | `flutter,hwcodec,unix-file-copy-paste,screencapturekit` |

Rust 1.81.0 is only for the official build baseline. Later rmcp integration needs at least Rust 1.88 and should be upgraded and validated separately after the baseline passes. Rust and Dart dependencies use the repository's Cargo.lock and pubspec.lock respectively; download problems are not grounds for updating versions.

### Initial lockfile correction

Upstream pubspec.lock was inconsistent with the tag's pubspec.yaml / Flutter 3.24.5, causing the first `flutter pub get --enforce-lockfile` to fail. One offline pub get with the pinned SDK regenerated the lockfile, adjusting 16 dependency entries: completing the existing flutter_test development dependency and its transitive dependencies, and aligning async, matcher, test_api and other test-chain versions with the SDK. vector_math only changed classification to reflect its existing direct dependency. Git dependency commits and pubspec.yaml were unchanged.

Continue validating with `--enforce-lockfile` to prevent implicit dependency refreshes. This resolution change is required for the build; changes to shared Dart library versions still need subsequent GUI compilation and runtime validation.

## Environment preparation

Full Xcode is required; Command Line Tools alone cannot build the Flutter macOS GUI. After installation, license acceptance and initial setup, verify that `xcodebuild -version` and `xcodebuild -checkFirstLaunchStatus` both succeed. Apple account interactions are performed by the local user.

The build cache defaults to `$HOME/Library/Caches/rustdesk-build` without changing global Flutter or Rust defaults. Paths can be overridden with `RUSTDESK_BUILD_CACHE`, `FLUTTER_ROOT` and `VCPKG_ROOT`.

```bash
brew install cmake ninja pkg-config cocoapods
git submodule update --init --recursive
rustup toolchain install 1.81.0 --profile minimal --component rustfmt

mkdir -p "$HOME/Library/Caches/rustdesk-build"
git clone --depth 1 --branch 3.24.5 https://github.com/flutter/flutter.git \
  "$HOME/Library/Caches/rustdesk-build/flutter-3.24.5"
```

Check out vcpkg at the commit above, then run `bootstrap-vcpkg.sh -disableMetrics`. NASM 2.16.03 source is available in the [official release directory](https://www.nasm.us/pub/nasm/releasebuilds/2.16.03/); build it locally and install it under `nasm-install` in the cache.

Install generators under the cache to avoid replacing other projects' tools:

```bash
cargo +1.81.0 install cargo-expand --version 1.0.95 --locked \
  --root "$HOME/Library/Caches/rustdesk-build/rust-tools"
cargo +1.81.0 install flutter_rust_bridge_codegen --version 1.80.1 \
  --features uuid --locked \
  --root "$HOME/Library/Caches/rustdesk-build/rust-tools"
```

The build script sets `RUST_LOG=info` specifically for the old bridge generator, which accepts only info/debug and panics if it inherits other levels. Generation disables automatic modification of lib.rs and uses the existing official module declaration.

Apply the following two official RustDesk CI adjustments only to this dedicated Flutter SDK:

1. Apply `.github/patches/flutter_3.24.4_dropdown_menu_enableFilter.diff` from the repository.
2. Comment out `_setFramesEnabledState(false);` in `packages/flutter/lib/src/scheduler/binding.dart`, matching the official macOS frame-scheduling adjustment.

These steps follow `.github/workflows/flutter-build.yml` and `.github/workflows/bridge.yml` at the pinned baseline. Official CI's default bridge artifacts use Flutter 3.22.3. Local generation initially uses the client's 3.24.5 and must pass subsequent Rust and Flutter compilation; compatibility is not assumed in advance.

## Staged execution

Run from the repository root:

```bash
scripts/build-macos-arm64.sh check
scripts/build-macos-arm64.sh deps
scripts/build-macos-arm64.sh bridge
scripts/build-macos-arm64.sh rust
scripts/build-macos-arm64.sh gui
```

Alternatively, `all` runs every stage in order. Stop on failure; a successful final copy command is not proof of build success. Stages can be rerun individually. Diagnostic logs go to the ignored `build/macos-arm64/logs/` directory.

The script selects ARM64, macOS 12.3 and the Rust toolchain through the process environment without rewriting official build.py, Cargo.toml, Podfile or Xcode projects. The expected GUI output is `flutter/build/macos/Build/Products/Release/RustDesk.app`; it does not automatically install or replace `/Applications/RustDesk.app`.

After copying the Rust service helper, the script reapplies local ad-hoc signing and verifies the entire application, preserving the main executable's existing entitlements. This signature is for local development validation, not Developer ID signing or notarized distribution.

If the first build needs a manual `pod install`, wait for `flutter precache --macos` to finish first; otherwise the Podfile post-install fails because the release FlutterMacOS.xcframework is missing. In the normal gui stage, Flutter handles download and Pods installation order.

Local launch check:

```bash
open -n "$PWD/flutter/build/macos/Build/Products/Release/RustDesk.app" --args --no-server
```

`--no-server` avoids starting a new controlled-side service when none exists; it does not isolate the official client's configuration or existing IPC service. Verify the process's full executable path to confirm that the local build is running.

## Baseline acceptance

- [x] Native dependencies, bridge generation, Rust compilation and Flutter GUI build succeeded.
- [x] The local build launches without missing dynamic libraries or startup crashes.
- [ ] Validate visible manual remote control, keyboard/mouse, screenshot observation and terminals against an official controlled client.
- [ ] Record test devices, actual tool versions, commands, artifacts and failure limitations.

This stage adds a build entry point and records without changing existing remote-control code. Aligning pubspec.lock with the SDK changes some shared Dart dependency resolutions, requiring GUI compilation/runtime validation. Submodule initialization retains pinned gitlinks without introducing upstream commit changes.

## Execution record: 2026-09-15

- Host: macOS 26.6.2 / ARM64, Xcode 27.0 (27A266a), macOS 27.0 SDK. Xcode license acceptance and initial setup completed; the script's check stage passed.
- Native dependencies: vcpkg manifest installation succeeded. Spot checks of libyuv, Opus and FFmpeg static libraries confirmed ARM64 slices.
- Tools: Flutter 3.24.5 / Dart 3.5.4, Rust 1.81.0, CocoaPods 1.17.0, CMake 4.4.3, Ninja 1.13.2 and NASM 2.16.03. Bridge generator and cargo-expand versions are listed above.
- Dart dependencies: offline `pub get --enforce-lockfile` passed after correcting the lockfile. Every Git dependency retained its original commit.
- Bridge: generation succeeded after Cargo dependencies were available, producing Rust, Dart and macOS C headers. ffigen reported SDK-header nullability warnings, with no compilation errors.
- Rust: release compilation with pinned 1.81.0 succeeded in 6 minutes 59 seconds. Existing unused-code and other warnings remained; there were no compilation errors.
- GUI: Flutter release build succeeded. The main executable, Rust dynamic library and service are all ARM64; the main executable and Rust library both have an actual minimum OS version of 12.3. CocoaPods installation succeeded without plugin-version changes. Lockfile footer differences caused only by the CocoaPods tool version were not retained.
- Packaging: the first signature check after copying service found that the new file was outside the sealed resources. After the script reapplied local signing, `codesign --verify --deep --strict` passed.
- Launch: the local artifact started with `--no-server`, with its process path verified inside the workspace. The normal home screen and connection entry were visible; the previously installed client remained running. No additional screen-recording permission was granted.
- Two live remote-control checks follow. They cover basic terminal GUI flow; multi-display behavior, reconnect, abnormal authentication and MCP bridging remain unaccepted at this stage.

Downloads encountered TLS interruptions. Retrying the same URLs/commits reused caches without changing pinned versions. Detailed logs remain locally in `build/macos-arm64/logs/`, outside version control.

### First live check: Windows 7 test device

After the user connected and authenticated, the local build displayed the remote desktop. Established TCP connections of the local process further confirmed session ownership; the device password was not recorded. The exact peer version and provenance were not verified, so full compatibility with an official peer cannot be claimed.

| Check | Result and scope |
| --- | --- |
| Connection and image | Desktop, Start menu and Notepad updated visibly in response to actions. Authentication was performed by the user; incorrect passwords were not tested. |
| Mouse | Start-menu clicks worked at original size and fit-to-window. Dragging Notepad in fit-to-window moved it as expected. |
| Keyboard | Automated ordinary letters, digits, Enter and Backspace worked. The user confirmed uppercase letters, digits and punctuation with a physical keyboard. |
| Automation input differences | The current computer-use tool's typeText / Shift combinations lost some uppercase letters and punctuation; cause unknown. This is not established as a RustDesk manual-keyboard fault or acceptance evidence for future MCP input. |
| Minimize/restore | Restoring through the native window menu showed an updated remote image. Bridge reads while minimized were not tested; no bridge existed yet. |
| Terminal | The GUI entry opened a separate window; connection returned `Remote terminal is not supported by the remote side`, then returned to the desktop after acknowledgement. No usable shell was created, so I/O, resizing and exit events could not be accepted. |
| Multiple displays | No display-switching entry was available. Multiple displays, negative coordinates and different scales were not covered. |
| Other | Scroll wheel, disconnect/reconnect, permission denial and authentication failure remain to be tested. |

On Windows, the pinned source advertises terminal support based on portable-pty ConPTY detection (src/server/connection.rs). The current dependency requires Windows 10 October 2018 or later. This Windows 7 device reported terminals unsupported, consistent with that restriction. Further checks need a terminal-capable peer and multiple displays.

Tests typed only into a new unsaved Notepad document, without writing files or changing remote system configuration. During cleanup the computer-use tool returned `noWindowsAvailable` twice, although RustDesk remained running and its remote-control window was readable. The temporary Notepad was left for the user to close. This tool-location problem is not recorded as a client crash. The session view changed from original size to fit-to-window.

### Second live check: terminal-capable Windows device

After switching machines and connecting, the user opened Terminal (beta) from the session toolbar and a visible PowerShell window appeared. The device name indicated Windows 10; its exact OS build, RustDesk version and provenance were not verified.

| Check | Result and scope |
| --- | --- |
| Open and I/O | The first terminal showed a PowerShell prompt; echo and subsequent prompts worked, including Chinese command-error output. |
| Size synchronization | mode con initially reported 167 columns × 45 rows. Shrinking the local terminal window changed it to 126 columns × 35 rows, confirming propagation to the peer. |
| Multiple terminals | The plus button created a second terminal; each had its own tab and prompt. |
| Shell isolation | The first terminal ran sv rdtest first and read first using gv rdtest. In the second terminal, gv rdtest reported the variable missing. |
| Shell exit | exit 7 in the second terminal closed its tab automatically. The first could still read rdtest and was unaffected. The GUI closed too quickly to verify the numeric exit code 7, so exit-code delivery is not accepted by this check. |
| GUI closure | Closing the remaining terminal window returned to the connected desktop. The remote process tree was not inspected, so cleanup of all child processes is not proven. |
| Output history | Scrolling could revisit the first echo and earlier size-query results. Cache capacity and truncation were not stress-tested. |
| Exact text | Computer-use input still lost punctuation, inserted spaces and failed to paste. Exact bytes, Chinese input and the MCP path need independent validation. |

No file or system-configuration modification commands were run: only console-state queries, echo, temporary shell variables and closing test terminals. Both test terminals were closed; the desktop remained connected, using fit-to-window.

Two differences were observed after returning to the desktop. A remote error briefly mentioned QueryFullProcessImageNameW, but its full text was not captured and it cannot be attributed to terminal closure. Computer-use screenshots showed black blocks that partly remained after scaling changes and refresh, while the user explicitly confirmed the manually viewed image was normal. These blocks are recorded as a discrepancy between tool screenshots and human viewing, with cause unknown, not as a RustDesk rendering failure. They cannot validate future bridge frame export.

## Bridge development: session observation and CPU frame cache

This section records the first stage (`df637a04d`). Layout isolation and PNG export follow in the next section.

The new `automation` Cargo feature compiles the bridge only on macOS and is off by default. It depends on Flutter but starts no HTTP service, exposes no AI writes and adds no GUI controls. Compiling source does not automatically give these capabilities to an already-running client.

### Integrated internal paths

- Allocate a distinct `session_id` using the shared connection state of each actual core session. Multiple GUI UUIDs for one core map to one record; removing the last view removes the record and rejects late callbacks. Remote and local IDs are not interchangeable.
- The IO loop holds an observation handle for a fixed connection epoch. Reconnect, authentication success/failure, permissions, display layout and disconnect update the bridge snapshot. Desktop authentication still waits for a valid first frame; terminal readiness depends on authentication and terminal support. Unknown permissions stay unknown.
- State revisions and frame revisions use separate watch channels; continuous frames do not repeatedly wake state queries. Subscribe before reading the snapshot to avoid missed updates while waiting.
- CPU pixels are copied only after explicitly enabling internal frame subscription, stored by session and display in independent memory. Metadata retains format, stride, connection/layout epochs, frame sequence, monotonic time and whether a cursor is embedded. Reading does not consume the GUI buffer.
- Limits are 128 MiB per frame and 512 MiB total pixels. Eviction follows most recent read time; frames still held by readers count toward the budget. Allocation failure returns a capacity error while ordinary GUI rendering continues.
- After disconnect, old cache from the same epoch remains readable and is explicitly stale; reconnect clears it. Without a comparison cursor, new/old comparison is unknown. Cache older than 2 seconds is also stale.

The frame hook sits in `Remote::new_video_thread`'s decode callback, immediately before `FlutterHandler::on_rgba`. This covers both software rendering and GUI paths that upload CPU pixels into textures. This location captures the corresponding IO loop's connection epoch without modifying the shared `InvokeUiSession` trait; old decoder threads cannot populate a new connection's bridge cache.

### Boundaries of this stage

This is internal observation infrastructure, not yet acceptance-ready screenshot or session tools:

- No agent binding, session_ref, approval/takeover, send-queue gating, visible-session creation, terminal raw-byte cache or MCP service is integrated yet.
- Complete authentication-challenge classification, human-confirmation events, connection-error details and GUI visibility remain incomplete. AI write permission cannot be inferred from current observations.
- No image encoding, screenshot mapping, union of display subscriptions or GPU-only-to-CPU switching is integrated. Texture-only output explicitly reports TextureOnly; such sessions cannot currently bind as AI sessions with screenshots.
- Layout changes clear the cache, while frames record the layout revision at callback receipt. Full isolation of queued old frames during layout changes remains unfinished, so no input-coordinate mappings are exposed to AI yet.
- Raw pixels may already contain a cursor embedded by the peer, which is reported honestly. A consistent public screenshot cursor rule is needed before shipping the capture tool.
- No bridge image has yet been exported from a running live session. Earlier manual desktop and terminal results validate only the official baseline.

### Build and validation

```bash
RUSTDESK_AUTOMATION=1 scripts/build-macos-arm64.sh rust
scripts/build-macos-arm64.sh gui
scripts/build-macos-arm64.sh automation-tests
```

Without `RUSTDESK_AUTOMATION=1`, the rust stage keeps the original four features. The bridge adds no third-party dependencies and still uses Rust 1.81.0.

- `cargo check --locked` passed with automation enabled and disabled.
- All 11 `automation::` release unit tests passed: snapshots survive GUI buffer swaps, leased frames count toward limits, unread updates do not evict recently read displays, authentication and first-frame readiness are separate, expired callbacks are rejected, layout invalidation and disconnected staleness work, terminal readiness needs no frame, invalid pixels/texture-only frames fail explicitly, CPU-to-texture-only transition clears cache and wakes readers, state/frame waits are separate, and multiple GUI views share lifecycle.
- Rust release with automation and Flutter GUI builds passed. The GUI artifact was approximately 62.3 MB; packaged `codesign --verify --deep --strict` passed. The running client was not restarted, so this round includes no live remote-control validation of the new bridge.

### Regression surface in existing paths

| File | Required path change |
| --- | --- |
| Cargo.toml, src/lib.rs | Declare the new default-off feature and macOS module entry. |
| src/flutter.rs | Call observation hooks when GUI views register, reuse or remove sessions; rendering is unchanged. |
| src/client/io_loop.rs | Add an observer and thin connection/permission/layout/decode hooks used only by the new feature; ordinary protocol messages and input sending are not rewritten. |
| scripts/build-macos-arm64.sh | Explicit environment selection enables the feature; default build selection is unchanged. |

All runtime hooks compile out with the feature disabled. New logic resides in src/automation. Shared traits, the Session structure, official FFI signatures, Flutter/Dart code, peer protocol and submodules are unchanged.

## Bridge development: layout isolation and PNG export

### Implemented

- Each video-decoding thread retains its own layout revision. After a stock layout event changes geometry, old incremental frames are cleared and a layout boundary enters the decode-message queue. Consuming the boundary resets the decoder, waits for a keyframe and requests a stock refresh. Old callbacks carry old revisions and cannot refill the new-layout cache.
- Old keyframes are still consumed in queue order, and GUI callbacks retain their existing entry. New boundary handling runs only in macOS automation builds with an observed session. The shared MediaData enum gains only a cfg-gated internal message; existing message structures and callback signatures are unchanged.
- New internal async `automation::capture::capture` reads an enabled frame cache by actual display ID, exports PNG and returns dimensions, remote rectangle, connection/layout epochs, frame sequence, receipt time, age, new/old and disconnect markers. If the comparison cursor has not advanced, it returns None instead of presenting the same image as new.
- Handles BGRA / RGBA and row padding, encoding opaque desktop RGB pixels. Default proportional downscaling fits 1600 × 1600; limits can be 1–3840. No upscaling or cropping. PNG output exceeding 8 MiB fails completely instead of returning truncated data.
- Does not composite the separate cursor: `cursor_composited=false`. Cursor pixels already embedded in video are retained and annotated. Local window screenshots are not read.
- Encoding runs in spawn_blocking, with at most two jobs globally. Cancelled calls retain a slot until their encoding job exits. Capture enablement, session validity and connection/layout epochs are rechecked after encoding.
- Pure geometry maps screenshot pixel centers to remote coordinates, including negative origins, scaling and out-of-bounds rejection. This neither grants input permission nor sends input.

### Validation and scope

- `cargo check --locked` passed with automation disabled.
- All 19 final release tests passed. Rust release with automation and Flutter GUI builds passed; the GUI artifact was approximately 62.3 MB and packaged `codesign --verify --deep --strict` passed. The running client was not restarted.
- Tests used independent pixel buffers and simulated session events, covering decoded PNG colors, padding, dimension limits, oversized failures, display isolation, negative coordinates, old-layout callback rejection and stale disconnected state. No images were exported from a live running peer, so full multi-display or GUI runtime regression acceptance cannot be claimed.
- The fixed local build enables hwcodec and disables vram. VP8 / VP9 / AV1 and H264 / H265 RAM decode paths output CPU pixels; subsequent GUI texture rendering does not consume the bridge copy. GPU-only switching/readback is unimplemented. TextureOnly in other feature combinations is not capture-ready.
- Unioned display subscriptions, primary-display resolution, bounded waits and stock refresh policy, agent-bound snapshot_id / mapping expiry and MCP image blocks still need integration. Current internal functions are not MCP tools and expose no AI input.

### Regression surface for this round

| File | Behavior change and necessity |
| --- | --- |
| src/client.rs | Add an automation layout-boundary branch to the decode loop, resetting the decoder and waiting for a keyframe. The boundary must be consumed in the decoding thread to stop old decoded results repopulating a cleared cache. |
| src/client/io_loop.rs | Forward layout events to decoder threads and request refresh when needed. Decode callbacks carry the revision actually applied by that thread. New state is automation-only; existing video-message sending, GUI rendering and input paths are not rewritten. |
| src/automation/sessions.rs | Check layout revision on frame receipt and session validity after export, rejecting stale images. |
| src/automation/mod.rs | Register the new capture and decode-layout modules. |

No new dependencies, FFI / Dart changes, peer-protocol changes or submodule updates in this round. Disabling automation still compiles the original paths.


## MCP integration build and validation

MCP uses a separate `mcp` feature; default builds retain the official path. The macOS ARM64 MCP build uses Rust 1.97.1, official `rmcp =3.3.0` and Axum 0.8, reusing the Flutter/native dependencies and pinned upstream commit above.

```bash
RUSTDESK_MCP=1 scripts/build-macos-arm64.sh bridge
RUSTDESK_MCP=1 scripts/build-macos-arm64.sh rust
RUSTDESK_MCP=1 scripts/build-macos-arm64.sh gui
```

Reproduce the two test groups under the MCP build configuration:

```bash
source scripts/build-macos-arm64.sh check
RUSTUP_TOOLCHAIN=1.97.1 cargo test --locked --release --lib --features mcp,hwcodec,unix-file-copy-paste,screencapturekit automation::
RUSTUP_TOOLCHAIN=1.97.1 cargo test --locked --release --lib --features mcp,hwcodec,unix-file-copy-paste,screencapturekit mcp::
```

Rust 1.97.1 stripping can produce a Mach-O LINKEDIT string table without 8-byte alignment, rejected by the Xcode 27 linker; see [Rust issue #157750](https://github.com/rust-lang/rust/issues/157750). The script overrides `profile.release.package.rustdesk.strip="none"` only for the RustDesk package in MCP builds, leaving other platforms and the official baseline's release profile unchanged. The actual dynamic-library string-table alignment was verified as 0 modulo 8, and a minimal dynamic-library load passed.

Final source validation:

- 38 bridge tests passed: connection/authentication separation, frame cache and multi-display geometry, connection/layout isolation, control epochs, raw terminal output and cache overwrite, physical key mapping, immediate wakeup of waiting input on takeover, key release and rejection of old queued sends. Completed records remained readable within retention after session closure.
- 5 MCP tests passed: strict arguments, exact Bearer matching, real HTTP initialization and two-client isolation, input/output schemas for 20 tools, Origin/body limits, concurrent operation-ID retries and original-result replay. Heartbeats and cancellation notifications remained usable when tool-response capacity was exhausted.
- Flutter static analysis had no errors, with existing upstream warnings/notices remaining. The official path with the new features disabled passed `cargo check --locked` using Rust 1.81.0.
- Final Rust release (library and executable) and Flutter GUI builds passed; artifact size was approximately 71.5 MB and `codesign --verify --deep --strict` passed. After restarting the final artifact, real HTTP initialization, session listing, settings-page running status and agent lists all worked.
- Checks of changed files, integration scripts and build logs found zero occurrences of the temporary password. Test connections were cleaned up, MCP was stopped at final delivery and default human approval remained enabled.

The user supplied a terminal-capable Windows 10 single-display peer for this round. Its temporary password was not saved in documentation, scripts or ordinary logs. No real dual-display device was available; multi-display evidence at this stage comes only from automated geometry/cache-isolation tests.


### MCP live integration record (2026-09-15)

These results came from the local MCP build connected to the user's Windows 10 single-display test machine. Temporary passwords were used only in authentication requests, not saved in scripts, this document or ordinary logs. Test text went into unsaved Notepad; terminal operations used echo, temporary shell variables, read-only version queries and explicit exit. Temporary Notepad was closed without saving. Read-only remote queries reported Windows 10.0.19045 and RustDesk 1.4.9+67; binary hashes were not verified.

| Check | Observed result |
| --- | --- |
| Service and agents | GUI start/stop matched actual listeners. Two MCP clients connected simultaneously; settings showed each name, version, agent_id, bound core sessions and GUI / terminal instances. |
| Session exclusivity | A second agent attaching to an occupied desktop got SESSION_BUSY but could independently open a terminal connection to the same peer. After detachment, another agent could bind with Human mode preserved; old references could not read the new binding. |
| Visible desktop and images | AI created a visible desktop returning authenticated / ready. Screenshots were remotely decoded PNGs, 980 × 606, with actual frame sequence/time. Reads and GUI display coexisted without consuming GUI render buffers. |
| Input | MCP opened Run and Notepad. Exact text ABC abc 123 . : and Chinese displayed correctly. Physical Shift + A / release Shift + B produced Ab. The remote Chinese input method still interpreted physical keys according to its own rules. |
| Scaled coordinates | Menu coordinates in a 490 × 303 screenshot mapped correctly to the 980 × 606 desktop; clicks hit the intended menu. |
| Retries | Text input retried with the same operation_id was not resent. Retrying a terminal counter increment produced only RD_COUNTER=1. |
| Human takeover | GUI clicks/text were blocked in AI mode. After takeover, AI writes returned HUMAN_CONTROL while reads remained available. Held Shift was released. During a sustained drag, a real GUI takeover stopped the active batch after partial execution and rejected the queued batch before execution; both returned CONTROL_EXPIRED. One key and one mouse button were released, with release_error null. |
| Approval | Release, GUI approval and old-reference CONTROL_EXPIRED were observed. Repeated requests retained the same approval ID/deadline; unapproved requests became expired after 60 seconds. Cancellation was repeatable and GUI denial returned rejected. Takeover/approval buttons remained clickable while the terminal password dialog was open. |
| Minimize | GUI state became minimized; new remote frames continued to arrive with updated timestamps and sequences. |
| Terminal | Visible creation, exact input, raw ANSI / UTF-8 / base64 output, PTY resize to 100 × 30, second-instance creation and independent closure passed. The other instance remained usable. exit 7 produced shell_exit_code=7 in the actual close event; this is not a per-command exit code. |
| Authentication and reconnect | A wrong password returned awaiting_auth / Wrong Password. Authenticating with a wrong password returned AUTH_FAILED; the current challenge's correct password authenticated and opened the terminal. Explicit disconnect returned disconnected / Human; old terminals became closed, with unknown exit codes null. Reconnect did not reuse the temporary password and returned new authentication challenges and control references. |
| Stop and departure | Stopping MCP closed its listener, cleared agents and removed banners; the original GUI terminal still accepted manual echo afterstop. DELETE ended the logical MCP session, revoked bindings and retained the GUI. Clients with no GET event stream and no ping responses received HTTP 404 after lease expiry; the session could be rebound and stayed Human. Persisting enablement, exiting and restarting restored running automatically. |

Integration found and fixed the following: official desktop peers report only denied permissions before authentication, so successful PeerInfo must apply protocol defaults while retaining explicit denials; child-window visibility must use desktop_multi_window; drag delays and queued waits must wake on control changes; terminal authentication dialogs must stay within the content area so the takeover entry remains usable. The live checks above were repeated on the updated build. Another fix prevents immediate eviction of a long-lived current reference on closure: retention starts when it is replaced. Automated tests cover retiring long-lived references and closed records; live reads after closing a newly created terminal connection confirmed Closed and GUI registered=false.

### Existing-path regression surface of MCP integration

This review uses the final diff. New remote-control logic stays in src/automation; MCP transport and tool adaptation stay in src/mcp. Required changes to existing files follow.

| File | Change and necessity |
| --- | --- |
| Cargo.toml / Cargo.lock, src/lib.rs | Add default-off, macOS-only mcp dependencies/modules. SDK resolution changes shared async-trait / serde_json versions, requiring feature-off compilation checks too. Submodule gitlinks remain unchanged. |
| src/flutter.rs | Reuse the Flutter async runner, using multithreaded Tokio in MCP builds and preventing duplicate startup. Restore persisted MCP enablement only after main-GUI event registration. Feature-off builds retain the original runner. |
| src/client.rs | Add cfg-gated internal messages and temporary-authentication markers. Final login send checks control; AI passwords stay out of stock persistence/reconnect paths. Human authentication uses the original function. |
| src/client/io_loop.rs | Apply input gating, release and authentication hooks at the actual send point; observe actual permissions/authentication/terminal events. Copy terminal bytes for the bridge while the GUI receives the original responses. |
| src/ui_session_interface.rs | Carry control epochs when manual input enters the queue; clear temporary AI credentials on reconnect so old queued input cannot execute after takeover. |
| src/flutter_ffi.rs | Add only MCP settings/control, visible-session and terminal-mount interfaces plus a thin terminal-open hook. Existing operation signatures are unchanged. |
| src/automation/sessions.rs / mod.rs | Extend the observation layer with authentication challenges, permissions, terminals and closed records. Reclaim state whose GUI core is gone, preventing stale references from remaining live. |
| flutter/lib/main.dart, utils/multi_window_manager.dart, models/model.dart | Route internal open/close requests to actual GUI windows using reserved request IDs without passwords. Ordinary session creation retains its entry point. |
| flutter/lib/desktop/pages/remote_page.dart / remote_tab_page.dart | Add session banners, read-only areas and a session-matched close branch, preserving existing image/toolbar implementations. |
| flutter/lib/desktop/pages/terminal_page.dart / terminal_tab_page.dart / terminal_connection_manager.dart | Forward creation requests, confirm mounted tabs and display the control bar. MCP-build authentication dialogs stay within content so takeover remains clickable. Close tools remove only the matching core's views without rewriting manual tab closure. |
| flutter/lib/desktop/pages/desktop_setting_page.dart | Add a settings tab only in MCP-capable builds; its component is separate. |
| src/lang/*.rs | Append only new UI keys; Chinese is translated, other languages retain empty fallback values, and Italian entries are not translated. |
| scripts/build-macos-arm64.sh | Explicit MCP builds select their toolchain and stripping workaround; default remains the official Rust 1.81.0 GUI. |

Live dual-display switching, mixed origins/scaling and display hotplug have not yet been accepted at this stage. The current release target is a local macOS ARM64 development artifact; other controller platforms and formal signing/notarized distribution have not been validated.
