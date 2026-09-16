# Proposal: upstreaming an optional local MCP integration for RustDesk

Reviewed draft for the upstream Feature Request discussion. Updated on 2026-09-16; a packaged build is available for evaluation with your own MCP agent and devices.

## Motivation

I have implemented and released a controller-side MCP integration with **67 MCP tools**. It lets an MCP client observe and operate RustDesk sessions while a person watches the same sessions in the normal Flutter GUI and can take control. The intended use is assisted remote troubleshooting and desktop tasks on machines the user is authorized to access.

I would like to contribute this work upstream. Would RustDesk consider an optional local MCP integration? I am looking for feedback on the architecture, toolchain constraints, and how to divide the existing implementation into reviewable contributions.

## Existing implementation and release

The released [MCP v0.1.9 source snapshot](https://github.com/NexusAgentX/rustdesk/tree/a0df1515f706d97a06f59ae7cf0c26410f7e7e76) is based on RustDesk 1.4.9 and currently targets a macOS ARM64 controller. The [diff from that base](https://github.com/NexusAgentX/rustdesk/compare/6c578292e8ebbbec708b76986ba8c4bc7c509747...a0df1515f706d97a06f59ae7cf0c26410f7e7e76) adds approximately **20,000 lines: 19,606 additions and 75 deletions across 128 files**, including implementation, automated tests, Flutter integration, localization, documentation, and build/dependency changes.

Controlled machines use the existing RustDesk protocol and an unmodified client. A [macOS ARM64 build and checksum file](https://github.com/NexusAgentX/rustdesk/releases/tag/mcp-v0.1.9) are available for evaluation. Documented packaging uses ad-hoc signing; Developer ID signing and notarization have not been validated.

## Try it with your own MCP agent

The MCP service and all 67 tools are included in the desktop application. To evaluate it:

1. Download the macOS Apple Silicon package from the release above, verify it with `SHA256SUMS.txt`, and open the included `RustDesk.app`.
2. In **Settings → MCP**, enable the service and select **Copy AI connection configuration**.
3. Add that configuration to your own agent client on the same Mac. The client needs authenticated Streamable HTTP support; clients with a different configuration format can use the copied endpoint and Bearer header.
4. Connect to one of your own test computers through the normal RustDesk GUI, using stock RustDesk on that computer. Ask your agent to inspect the session, capture the screen, request control, and perform a small task while you observe the GUI and exercise human takeover.

Evaluation uses your own agent, devices, and normal RustDesk authentication. No separate client script or Rust/Flutter build environment is needed. The [evaluation guide](https://github.com/NexusAgentX/rustdesk/blob/cde87cd29663299f7417f842cc1d768c41bfab03/docs/MCP-UPSTREAM-PREPARATION.md#maintainer-evaluation) includes a suggested first task.

## Implemented capabilities

All **67 tools are registered in the [released tool catalog](https://github.com/NexusAgentX/rustdesk/blob/a0df1515f706d97a06f59ae7cf0c26410f7e7e76/src/mcp/tools.rs)**. Their implemented scope is:

| Area | Tools | Implemented capabilities |
| --- | ---: | --- |
| Sessions and authentication | 9 | Discover, open, attach, detach, inspect, authenticate, disconnect, reconnect, and close visible desktop, file-transfer, terminal, and TCP-tunnel sessions |
| Control and inspection | 5 | Request, cancel, and release AI control; inspect capabilities and operation records |
| Keyboard, mouse, and desktop actions | 5 | Ordered keyboard/mouse input, screenshot-based coordinates, capture and refresh, remote lock and restart |
| Displays and local views | 8 | Display topology and modes, capture/view selection, resolution and virtual-display requests, scaling, cursor settings, and local window control |
| Connection quality and audio | 2 | Quality/FPS and codec preferences, true color, audio settings, and observed connection metrics |
| Privacy and security | 6 | Security-state queries, input blocking, privacy mode, elevation requests, OS password input, and secure attention |
| File transfer and management | 8 | Directory browsing, bidirectional transfers, progress, cancellation, conflict handling, transfer resumption, and file management |
| Text clipboard | 5 | Synchronization settings, reading, writing/pasting, and sending text as keystrokes |
| Native file clipboard | 5 | Settings and state, copy, paste, and cancellation |
| Chat and recording | 4 | Text-chat send/read and video-recording control/state |
| Interactive terminals | 6 | List, create, read, write, resize, and close visible terminal tabs |
| TCP tunnels | 4 | List, add, remove, and authenticate managed loopback listeners |

These are implemented interfaces; peer/platform support and live-test coverage vary by capability, as described below and in the [release notes](https://github.com/NexusAgentX/rustdesk/releases/tag/mcp-v0.1.9).

The implementation separates MCP transport and tools (`src/mcp`) from session observation, input authority and GUI integration (`src/automation`). It uses the official Rust MCP SDK and Streamable HTTP. The Cargo features and the service setting are off by default; the listener binds to loopback and requires a Bearer token, with Host and Origin validation.

Existing visible sessions retain human control when an agent attaches. Switching them to agent control requires local approval by default. Human takeover invalidates queued agent input and releases held keys and buttons. An agent may continue observing while the human controls the session. In the current implementation, a genuinely new session opened by the agent starts in agent control after normal remote authentication; this policy is open for upstream discussion.

## Evidence and limitations

The [recorded desktop validation](https://github.com/NexusAgentX/rustdesk/blob/cde87cd29663299f7417f842cc1d768c41bfab03/docs/MACOS-ARM64-BUILD.md#mcp-live-integration-record-2026-09-15) covers captures, text and keyboard input, local approval, human takeover during input, reconnect behavior and continued manual use after stopping MCP. Later [display validation](https://github.com/NexusAgentX/rustdesk/blob/cde87cd29663299f7417f842cc1d768c41bfab03/docs/MCP-FILE-RECOVERY-DISPLAYS.md#validation-record) covers a stock Windows 1.4.9 peer with two displays, negative coordinates and mixed Windows DPI. These remain the historical live-test records.

In a [fresh evaluation on 2026-09-16](https://github.com/NexusAgentX/rustdesk/blob/cde87cd29663299f7417f842cc1d768c41bfab03/docs/MCP-UPSTREAM-PREPARATION.md#evaluation-checks-2026-09-16), **74 automation tests and 12 MCP tests passed**. The downloaded release ZIP matched both its checksum file and GitHub's asset digest; the extracted ARM64 app passed strict signature verification. Its main executable, Rust library, and Flutter application binary matched the installed app used for evaluation. This round verified the package and automated tests; the remote desktop paths above were not rerun.

Other controller platforms have not been validated. Some advanced paths, including successful 2FA and virtual-display creation, still need suitable test environments. The [SDK requires Rust 1.88 or later](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.3.0/Cargo.toml); the released implementation was built with 1.97.1, while [upstream's macOS build workflow at the checked revision](https://github.com/rustdesk/rustdesk/blob/0ac2e7fb5269b9dbdab85f7213b30d76ba2d49f9/.github/workflows/flutter-build.yml) uses 1.81. Toolchain and dependency choices need agreement. A contribution would also need adapting to current master, including the move to `libs/base`.

## Suggested contribution scope

I would like to contribute the existing implementation in reviewable increments, with maintainers helping decide the final scope and order. One possible first PR would extract and adapt the read-only foundation: optional service and authentication, GUI settings and binding indication, listing/attaching visible desktop sessions, and bounded screenshots. Desktop input and human takeover would follow together. The remaining implemented capabilities above could then be grouped into separate changes according to maintainer priorities.

This is a proposed integration sequence for the existing 67-tool implementation. Each increment would include relevant tests and a description of effects on existing runtime paths, including behavior with the feature disabled.

## Questions for maintainers

1. Is an optional embedded MCP service a direction you would consider, or would you prefer a separate integration with a narrower client API?
2. Would an experimental macOS-first increment with the scope above be useful?
3. What SDK, Rust toolchain and CI constraints should an upstream implementation follow?
