# Proposal: optional local MCP support for visible RustDesk sessions

Draft for the upstream Feature Request discussion. Prepared on 2026-09-16; the demonstration recording is pending.

## Motivation

I have built a controller-side prototype that lets an MCP client observe and operate a RustDesk session while a person watches the same session in the normal Flutter GUI and can take control. The intended use is assisted remote troubleshooting and desktop tasks on machines the user is authorized to access.

Would RustDesk consider an optional local MCP integration? I would like to agree on the architecture and a small initial scope before preparing upstream PRs.

## Working prototype

The [source snapshot](https://github.com/NexusAgentX/rustdesk/tree/a0df1515f706d97a06f59ae7cf0c26410f7e7e76) is based on RustDesk 1.4.9 and currently targets a macOS ARM64 controller. Controlled machines use the existing RustDesk protocol and an unmodified client. A [macOS ARM64 build and checksum file](https://github.com/NexusAgentX/rustdesk/releases/tag/mcp-v0.1.9) are available for optional evaluation. Documented packaging uses ad-hoc signing; Developer ID signing and notarization have not been validated.

The prototype separates the MCP transport and tools (`src/mcp`) from session observation, input authority and GUI integration (`src/automation`). It uses the official Rust MCP SDK and Streamable HTTP. The Cargo features and the service setting are off by default; the listener binds to loopback and requires a Bearer token, with Host and Origin validation.

Existing visible sessions retain human control when an agent attaches. Switching them to agent control requires local approval by default. Human takeover invalidates queued agent input and releases held keys and buttons. An agent may continue observing while the human controls the session. In the current prototype, a genuinely new session opened by the agent starts in agent control after normal remote authentication; this policy is open for upstream discussion.

## Evidence and limitations

The [recorded desktop validation](MACOS-ARM64-BUILD.md#mcp-live-integration-record-2026-09-15) covers captures, text and keyboard input, local approval, human takeover during input, reconnect behavior and continued manual use after stopping MCP. Later [display validation](MCP-FILE-RECOVERY-DISPLAYS.md#validation-record) covers a stock Windows 1.4.9 peer with two displays, negative coordinates and mixed Windows DPI. The [latest validation record](MCP-TUNNELS-TERMINAL.md#validation) reports 74 automation tests and 12 MCP tests passing. These are existing project records; an independent reproduction and a short public demonstration are still being prepared.

Other controller platforms have not been validated. Some advanced paths, including successful 2FA and virtual-display creation, still need suitable test environments. The [SDK requires Rust 1.88 or later](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.3.0/Cargo.toml); the prototype was built with 1.97.1, while [upstream's current macOS build workflow](https://github.com/rustdesk/rustdesk/blob/0ac2e7fb5269b9dbdab85f7213b30d76ba2d49f9/.github/workflows/flutter-build.yml) uses 1.81. Toolchain and dependency choices need agreement. A contribution would also need adapting to current master, including the move to `libs/base`.

## Suggested contribution scope

The prototype includes additional terminal, file and tunnel tools. For an initial contribution, I propose a smaller read-only increment: optional service and authentication, GUI settings and binding indication, listing/attaching visible desktop sessions, and bounded screenshots. Desktop input and human takeover would follow together; terminal and other capabilities could be separate changes.

Each increment would include relevant tests and a description of effects on existing runtime paths, including behavior with the feature disabled.

## Questions for maintainers

1. Is an optional embedded MCP service a direction you would consider, or would you prefer a separate integration with a narrower client API?
2. Would an experimental macOS-first increment with the scope above be useful?
3. What SDK, Rust toolchain and CI constraints should an upstream implementation follow?
