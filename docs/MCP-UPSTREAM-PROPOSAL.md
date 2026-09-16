# Proposal: optional local MCP integration for RustDesk

Would RustDesk be open to an optional, controller-side MCP integration? I have a working implementation and would like feedback on this direction and any blocking architectural concerns before adapting it for upstream.

It lets an AI agent observe and operate remote sessions while a person watches in the normal Flutter GUI and can take control. The intended use is assisted troubleshooting and desktop tasks on machines the user is authorized to access. **Controlled machines use stock RustDesk; no protocol changes are required.**

## Current implementation

The [macOS ARM64 release](https://github.com/NexusAgentX/rustdesk/releases/tag/mcp-v0.1.9), based on RustDesk 1.4.9, includes 67 tools covering sessions, screenshots and input, displays, files and clipboard, chat, recording, terminals, and TCP tunnels. The [source diff](https://github.com/NexusAgentX/rustdesk/compare/6c578292e8ebbbec708b76986ba8c4bc7c509747...a0df1515f706d97a06f59ae7cf0c26410f7e7e76) is approximately 20,000 added lines including tests, UI, localization, documentation, and build changes.

- **Local and optional:** Cargo features and the service setting are off by default. The embedded service uses the official Rust MCP SDK and Streamable HTTP, binds to loopback, and requires a Bearer token with Host and Origin validation.
- **Human control:** Attaching to an existing visible session preserves human control; agent control requires local approval by default. Human takeover invalidates queued agent input and releases held keys/buttons. Observation can continue during human control.
- **Policy for discussion:** A genuinely new session opened by the agent currently starts in agent control after normal remote authentication.

MCP transport/tools (`src/mcp`) are separated from session observation, input authority, and GUI integration (`src/automation`).

## Try it

Download the [release and checksum file](https://github.com/NexusAgentX/rustdesk/releases/tag/mcp-v0.1.9), open `RustDesk.app`, then enable **Settings → MCP** and select **Copy AI connection configuration**. Add it to an agent client on the same Mac that supports authenticated Streamable HTTP. Connect to your own test computer through the RustDesk GUI, then ask the agent to capture the screen and request control while you observe and try human takeover. No build environment or separate client script is needed.

The [evaluation guide and test records](https://github.com/NexusAgentX/rustdesk/blob/dbacaa1b0f48da8001c87dc854fc809c1b83460c/docs/MCP-UPSTREAM-PREPARATION.md#maintainer-evaluation) provide details. Recorded live tests cover core desktop operations, approval/takeover, reconnect, and multi-display behavior. A fresh check passed 74 automation tests and 12 MCP tests and verified the release package; it did not rerun those live paths.

**Limitations:** Only macOS ARM64 controllers have been validated. Advanced paths such as successful 2FA and virtual-display creation need further testing. Packaging uses ad-hoc signing; Developer ID signing and notarization have not been validated.

## Upstream contribution

If this direction is welcome, I would like to adapt the implementation to current master and prepare an upstream PR promptly. I can work with maintainers on the scope and split the changes as needed for review.

The current implementation uses the Rust MCP SDK (`rmcp` 3.3.0), which [requires Rust 1.88 or later](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.3.0/Cargo.toml), while the [checked upstream macOS workflow](https://github.com/rustdesk/rustdesk/blob/0ac2e7fb5269b9dbdab85f7213b30d76ba2d49f9/.github/workflows/flutter-build.yml) uses Rust 1.81. Porting also needs to account for the move to `libs/base`.

1. Is this integration of interest, and are there blocking concerns with the local-service architecture or control policy?
2. Could a Rust toolchain update be considered, or should the integration target the existing toolchain?
