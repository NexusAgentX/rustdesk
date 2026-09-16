# Preparing the MCP upstream proposal

Updated: 2026-09-16. The reviewed [proposal text](MCP-UPSTREAM-PROPOSAL.md) was published in upstream's Feature Request category as [discussion #16246](https://github.com/rustdesk/rustdesk/discussions/16246). The evaluation entry point is the released desktop app, connected to the evaluator's own MCP agent and devices. A video or separate reproduction script is optional supplementary material, not a prerequisite for the initial discussion.

## Maintainer evaluation

The macOS Apple Silicon package already contains the MCP service and all 67 tools. Evaluation follows the normal product flow:

1. Download `RustDesk-macOS-arm64-mcp-v0.1.9.zip` and `SHA256SUMS.txt` from [MCP v0.1.9](https://github.com/NexusAgentX/rustdesk/releases/tag/mcp-v0.1.9). Verify the download, extract it, and install/open the included `RustDesk.app`.
2. Open **Settings → MCP**, enable the service, and select **Copy AI connection configuration**. Human approval for AI control is enabled by default.
3. Add the copied configuration to an MCP-capable agent client on the same Mac. The client must support authenticated Streamable HTTP. For clients using another configuration format, enter the copied URL and Authorization header in that client's format. The local MCP token belongs in the evaluator's own agent configuration.
4. Use the normal RustDesk GUI to connect to a computer the evaluator owns or is authorized to test, with stock RustDesk on that computer. Use the evaluator's own RustDesk server configuration and authentication as usual.
5. Ask the agent to inspect the connected session, capture the screen, request control, and perform a small task. Watch the same session in the GUI and use human takeover to check that agent writes stop.

The app copies a configuration with this shape; the real local address and token are filled in by the app:

```json
{
  "mcpServers": {
    "rustdesk": {
      "url": "http://127.0.0.1:21122/mcp",
      "headers": {
        "Authorization": "Bearer <LOCAL_MCP_TOKEN>"
      }
    }
  }
}
```

A suggested first task, after preparing a blank, unsaved text editor on the evaluator's own test computer:

> Use RustDesk to find my existing desktop session to the test computer. Attach to it and show me the primary display. Request AI control through RustDesk, then type “RustDesk MCP demo” into the blank editor I have prepared. I will use human takeover afterward; report the resulting control state and stop writing.

The agent calls the packaged MCP tools directly. The evaluator supplies their own agent and target computer; the project owner's test machine is an internal validation environment. No Python client script or Rust/Flutter toolchain is required to try the package.

Feedback should identify the controller platform, agent client, remote RustDesk version/platform, attempted action, and observed result. Source builds are available for maintainers who want to inspect or modify the implementation.

## Pinned evaluation version

| Item | Verified information |
| --- | --- |
| Public repository | [NexusAgentX/rustdesk](https://github.com/NexusAgentX/rustdesk) |
| Evaluation commit | `a0df1515f706d97a06f59ae7cf0c26410f7e7e76` |
| Upstream base | RustDesk 1.4.9, `6c578292e8ebbbec708b76986ba8c4bc7c509747` |
| Implementation scale | At the evaluation commit, 128 files changed: 19,606 insertions and 75 deletions relative to the upstream base; includes implementation, tests, Flutter integration, localization, documentation, and build/dependency changes |
| Implemented tools | 67 unique tools registered in the [released tool catalog](https://github.com/NexusAgentX/rustdesk/blob/a0df1515f706d97a06f59ae7cf0c26410f7e7e76/src/mcp/tools.rs) |
| Release tag | `mcp-v0.1.9`; the annotated tag was dereferenced through the GitHub API and confirmed to point to the evaluation commit |
| Evaluation build | [MCP v0.1.9](https://github.com/NexusAgentX/rustdesk/releases/tag/mcp-v0.1.9) |
| Published assets | `RustDesk-macOS-arm64-mcp-v0.1.9.zip`, `SHA256SUMS.txt` |
| Validated controller platform | macOS ARM64; a 12.3 deployment target does not mean every macOS version has been tested |
| Contributor build tools | MCP uses Rust 1.97.1 and Flutter 3.24.5; rmcp 3.3.0 declares a minimum Rust version of 1.88. These are source-build requirements, not package-installation requirements |

## Evaluation checks: 2026-09-16

- Downloaded the published 28,260,106-byte ZIP and verified SHA-256 `6265ffa92742b5ccd14b57297c43b817807e85881e7a9fb2b5a0573eae57fad4` against both `SHA256SUMS.txt` and GitHub asset metadata.
- Extracted the app and passed `codesign --verify --deep --strict`. Its main executable is ARM64, and its bundle reports RustDesk 1.4.9. The signature is ad-hoc, without a Developer ID team; Apple notarization is not established by this check.
- Confirmed that the running controller uses the installed `RustDesk.app`. Its main executable, Rust dynamic library, and Flutter application binary match the corresponding downloaded binaries byte for byte.
- Confirmed that the connected MCP client exposes 67 RustDesk tools and can call session discovery directly.
- Reran the automation and MCP test suites from `ff80407bb2980e348da959d1670b99b3b54fa0ff`, which differs from the evaluation commit only in documentation: **74 automation tests and 12 MCP tests passed, with zero failures**.

These checks verify package integrity, correspondence with the installed binaries, MCP discovery, and the automated suites. They do not establish a reproducible source build. Fresh remote desktop validation was paused at authentication; earlier live-test records below remain the evidence for remote behavior.

## Evidence inventory

| Behavior or claim | Evidence | Status |
| --- | --- | --- |
| Existing implementation scale and all 67 tools | Diff between the pinned base and evaluation commit; registrations in [`src/mcp/tools.rs`](https://github.com/NexusAgentX/rustdesk/blob/a0df1515f706d97a06f59ae7cf0c26410f7e7e76/src/mcp/tools.rs) | Verified against the released tool catalog |
| Packaged app ready for evaluator-owned agent setup | Release download and package checks above; **Copy AI connection configuration** in the app | Package and configuration entry point verified |
| Stock Windows peer, visible desktop, capture, and exact text input | [Baseline live record](MACOS-ARM64-BUILD.md#mcp-live-integration-record-2026-09-15) and [final-version regression](MCP-TUNNELS-TERMINAL.md#validation) | Historical live-test evidence |
| Approval request, human takeover, queue cancellation, and held-key release | [Baseline live record](MACOS-ARM64-BUILD.md#mcp-live-integration-record-2026-09-15) | Historical live-test evidence |
| Manual session remains usable after MCP stops | [Baseline live record](MACOS-ARM64-BUILD.md#mcp-live-integration-record-2026-09-15) | Historical live-test evidence |
| Dual displays, negative coordinates, and mixed DPI | [Display increment record](MCP-FILE-RECOVERY-DISPLAYS.md#validation-record) | Historical live-test evidence |
| 74 automation tests and 12 MCP tests | Fresh evaluation checks above | Rerun successfully on 2026-09-16 |
| Existing build path with MCP disabled | [Integration build record](MACOS-ARM64-BUILD.md#mcp-integration-build-and-validation) records successful `cargo check --locked` on Rust 1.81.0 | Revalidate after porting to master; do not extrapolate to every platform |
| Virtual-display creation, successful 2FA, and some successful security-action paths | [Display limitations](MCP-FILE-RECOVERY-DISPLAYS.md#validation-record), [tunnel limitations](MCP-TUNNELS-TERMINAL.md#validation), and [security-action limitations](MCP-SECURITY-ACTIONS.md#validation) | Suitable environments still needed for the documented success paths |

Early overview documents still say dual-display acceptance is pending; later display records include dual-display live testing. Public descriptions should cite the specific record and scope.

## Source builds for contributors

The primary trial path is the packaged app above. Contributors who want to build or modify the pinned implementation can fetch it separately:

```bash
git clone https://github.com/NexusAgentX/rustdesk.git rustdesk-mcp-evaluation
cd rustdesk-mcp-evaluation
git checkout --detach a0df1515f706d97a06f59ae7cf0c26410f7e7e76
git submodule update --init --recursive
```

Configure Xcode, Flutter, vcpkg, NASM, the generator, and Flutter patches according to the [build log](MACOS-ARM64-BUILD.md#environment-preparation), and install Rust 1.97.1 for MCP. From the repository root:

```bash
RUSTDESK_MCP=1 scripts/build-macos-arm64.sh all
```

The expected app is `flutter/build/macos/Build/Products/Release/RustDesk.app`. A fresh-directory source reproduction remains unverified. The commands below, run in Bash, were used for the fresh automated test checks:

```bash
(
  set -e
  export RUSTDESK_MCP=1
  source scripts/build-macos-arm64.sh check
  cargo test --locked --release --lib \
    --features mcp,hwcodec,unix-file-copy-paste,screencapturekit automation::
  cargo test --locked --release --lib \
    --features mcp,hwcodec,unix-file-copy-paste,screencapturekit mcp::
)
```

## Upstream interest and next step

The [maintainer response](https://github.com/rustdesk/rustdesk/discussions/16246#discussioncomment-18467383) on 2026-09-16 invited the contribution to proceed and noted that Rust 1.75 is retained for Windows 7 users. In the [follow-up decision](https://github.com/rustdesk/rustdesk/discussions/16246#discussioncomment-18467622), the maintainer requested a single PR first, with splitting considered later if needed, and deferred additional testing requirements until seeing the PR. We [acknowledged the next step](https://github.com/rustdesk/rustdesk/discussions/16246#discussioncomment-18467722) and marked the initial maintainer response as the answer while keeping the discussion open. How the toolchain constraint applies to optional MCP builds remains unresolved.

- [x] Review the proposal text with the project owner.
- [x] Verify the pinned source, release tag, implementation scale, and 67 registered tools.
- [x] Translate release notes and branch-specific documentation into English.
- [x] Verify the downloaded package and its correspondence with the installed application binaries.
- [x] Rerun 74 automation tests and 12 MCP tests successfully.
- [x] Document package installation and connection to the evaluator's own agent and devices.
- [x] Publish the proposal in Feature Request: [discussion #16246](https://github.com/rustdesk/rustdesk/discussions/16246), posted on 2026-09-16. The published title and body were read back and verified against the reviewed proposal.
- [x] Review the initial maintainer response: proceed with the contribution while accounting for the Rust 1.75/Windows 7 constraint.
- [x] Ask about PR organization and maintainer/community testing expectations.
- [x] Confirm the contribution format: one PR first, with splitting considered later if needed.
- [x] Acknowledge the next step and mark the initial maintainer response as the answer, keeping the discussion open.
- [ ] Prepare a single PR against current master, accounting for the existing toolchain constraints and documenting test results, validated platforms, and known limitations.
- [ ] Agree on additional testing or evaluation builds after the maintainer reviews the PR.

A video, additional internal live testing, or a standalone reproduction script can supplement later discussion if useful. They do not block the initial proposal or packaged evaluation. The next step is to resolve the toolchain approach and prepare the upstream adaptation as a single PR; further testing expectations will follow maintainer review.
