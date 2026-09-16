# Preparing the MCP upstream proposal

Updated: 2026-09-16. Current stage: assembling verifiable evidence. See [MCP-UPSTREAM-PROPOSAL.md](MCP-UPSTREAM-PROPOSAL.md) for the draft proposal.

## Pinned evaluation version

| Item | Verified information |
| --- | --- |
| Public repository | [NexusAgentX/rustdesk](https://github.com/NexusAgentX/rustdesk) |
| Evaluation commit | `a0df1515f706d97a06f59ae7cf0c26410f7e7e76` |
| Upstream base | RustDesk 1.4.9, `6c578292e8ebbbec708b76986ba8c4bc7c509747` |
| Implementation scale | At the evaluation commit, 128 files changed: 19,606 insertions and 75 deletions relative to the upstream base; includes implementation, tests, Flutter integration, localization, documentation, and build/dependency changes |
| Implemented tools | 67 unique tools registered in `src/mcp/tools.rs`; the proposal includes their complete capability breakdown |
| Release tag | `mcp-v0.1.9`; the annotated tag was dereferenced through the GitHub API and confirmed to point to the evaluation commit |
| Optional evaluation build | [MCP v0.1.9](https://github.com/NexusAgentX/rustdesk/releases/tag/mcp-v0.1.9) |
| Published assets | `RustDesk-macOS-arm64-mcp-v0.1.9.zip`, `SHA256SUMS.txt` |
| Validated controller platform | macOS ARM64; a 12.3 deployment target does not mean every macOS version has been tested |
| Build tools | MCP uses Rust 1.97.1 and Flutter 3.24.5; rmcp 3.3.0 declares a minimum Rust version of 1.88 |

GitHub asset metadata reports ZIP SHA-256 `6265ffa92742b5ccd14b57297c43b817807e85881e7a9fb2b5a0573eae57fad4`. This round checked source, tag, and release metadata. Download verification, launch checks, and correspondence between the binary and its source build remain for the next validation round. A tag pointing to the correct commit does not itself prove that the uploaded binary was built from that commit.

Existing scripts use ad-hoc signing. Developer ID signing and notarization have not been validated. The initial proposal uses source and a demonstration as its main entry points, with the existing download as optional evaluation material.

## Four claims the proposal needs to demonstrate

1. MCP calls can read actual remote frames and complete a small, observable task.
2. The human and agent share the same visible session; agent writes are rejected after human takeover.
3. The implementation reuses the stock controlled client and existing protocol; ordinary manual remote control remains usable after MCP stops.
4. The boundaries are explicit: current platform, SDK/toolchain requirements, historical test evidence, and uncovered paths are described separately.

The first discussion presents the released 67-tool implementation and its approximately 20,000 added lines, then seeks feedback on direction, contribution order, and build constraints. The demonstration uses the existing implementation. A first PR containing only read-only sessions and capture would extract a reviewable increment from that work; maintainers would help choose the final scope and order of subsequent contributions.

## Evidence inventory

The results below come from existing records and had not been rerun when preparing the proposal. Before publishing the demonstration, rerun the relevant paths on the pinned version and save redacted results.

| Behavior to demonstrate | Existing evidence | Preparation for this presentation |
| --- | --- | --- |
| Existing implementation scale and all 67 tools | Diff between the pinned base and evaluation commit; registrations in [`src/mcp/tools.rs`](https://github.com/NexusAgentX/rustdesk/blob/a0df1515f706d97a06f59ae7cf0c26410f7e7e76/src/mcp/tools.rs) | Present the capability breakdown and downloadable release before discussing PR scope; distinguish implemented interfaces from fully validated success paths |
| Stock Windows peer, visible desktop, capture, and exact text input | [Baseline live record](MACOS-ARM64-BUILD.md#mcp-live-integration-record-2026-09-15) and [final-version regression](MCP-TUNNELS-TERMINAL.md#validation) | Record calls, GUI, and remote results for the same session |
| Approval request, human takeover, queue cancellation, and held-key release | [Baseline live record](MACOS-ARM64-BUILD.md#mcp-live-integration-record-2026-09-15) | Demonstrate rejected writes after takeover and successful human input |
| Manual session remains usable after MCP stops | [Baseline live record](MACOS-ARM64-BUILD.md#mcp-live-integration-record-2026-09-15) | Retest with the final evaluation build |
| Dual displays, negative coordinates, and mixed DPI | [Display increment record](MCP-FILE-RECOVERY-DISPLAYS.md#validation-record) | Use as supporting evidence; keep the main video to a single-display flow |
| 74 automation tests and 12 MCP tests | [Latest test record](MCP-TUNNELS-TERMINAL.md#validation) | Save fresh results from the pinned commit |
| Existing build path with MCP disabled | [Integration build record](MACOS-ARM64-BUILD.md#mcp-integration-build-and-validation) records successful `cargo check --locked` on Rust 1.81.0 | Revalidate after porting to master; do not extrapolate to every platform |
| Virtual-display creation, successful 2FA, and some successful security-action paths | [Display limitations](MCP-FILE-RECOVERY-DISPLAYS.md#validation-record), [tunnel limitations](MCP-TUNNELS-TERMINAL.md#validation), and [security-action limitations](MCP-SECURITY-ACTIONS.md#validation) | List honestly; exclude these features from the initial demonstration |

Early overview documents still say dual-display acceptance is pending; the later display increment record includes dual-display live testing. Public descriptions should cite the specific record and scope rather than generalizing an early status or partial test result to the entire feature set.

## Two-to-three-minute demonstration script

Before recording, prepare a stock Windows peer that is authorized for testing, with a blank, unsaved text editor open. Record the controller commit, actual running application path, and peer version. Complete connection authentication before showing public footage. Hide real device IDs, addresses, credentials, tokens, and private content in other windows.

Recording layout: show the local RustDesk GUI alongside MCP calls/results. Keep the actual result of each step; label human actions as performed by the demonstrator.

| Time | Action | Evidence to capture |
| --- | --- | --- |
| 0:00–0:20 | Show the controller version, stock peer version, and connected desktop; enable MCP | Running service and normal GUI session; omit token footage |
| 0:20–0:45 | Call `rd_session_list`, select and `rd_session_attach` to the target, then call `rd_screen_capture` | Returned session reference and actual PNG; agent binding appears in the GUI while control remains Human |
| 0:45–1:05 | Call `rd_control_request`; the demonstrator approves locally; read the current `session_ref` | pending before approval, AI control and a new reference afterward |
| 1:05–1:35 | Locate the editor from a fresh screenshot, use `rd_input_send` to click and enter fixed text, then capture again | Calls correspond to actual editor changes; `sent` alone cannot prove input completion |
| 1:35–2:05 | The demonstrator clicks human takeover; the agent then attempts more input | Writing through the old reference returns `CONTROL_EXPIRED`; writing after reading the current reference returns `HUMAN_CONTROL`; rejected text does not appear |
| 2:05–2:25 | The demonstrator enters `Human in control` through the GUI | Human input succeeds and the agent does not automatically reacquire control |
| 2:25–2:45 | Disable MCP and continue typing through the same GUI session | MCP stops while manual remote control remains usable |

Fixed demonstration text: `RustDesk MCP demo / 你好` (the Chinese text intentionally tests Unicode input). Rejected text: `BLOCKED_AFTER_TAKEOVER`. Use a distinct `operation_id` for every intended new input, including rejection checks, to avoid replaying a previous operation's deduplicated result. Use a newly returned `snapshot_id` for screenshot coordinates; do not hard-code machine-specific positions.

This tool sequence is a demonstration script that can be followed manually. A standalone runnable MCP client script, actual video, and redacted logs still need to be produced. Genuinely new sessions opened by the agent default to AI control in the implementation; this demonstration attaches to an existing session to show the approval flow. The proposal explicitly describes both policies.

## Reproducing from source

Fetch the pinned source into a new evaluation directory:

```bash
git clone https://github.com/NexusAgentX/rustdesk.git rustdesk-mcp-evaluation
cd rustdesk-mcp-evaluation
git checkout --detach a0df1515f706d97a06f59ae7cf0c26410f7e7e76
git submodule update --init --recursive
```

First configure full Xcode, the pinned Flutter version, vcpkg, NASM, the generator, and Flutter patches according to the [build log's pinned inputs and environment preparation](MACOS-ARM64-BUILD.md#environment-preparation). Install Rust 1.97.1 for MCP. Then run from the checked-out repository root:

```bash
RUSTDESK_MCP=1 scripts/build-macos-arm64.sh all
```

The expected app is `flutter/build/macos/Build/Products/Release/RustDesk.app`. Explicitly launch and verify that application path during evaluation so another installed RustDesk is not mistaken for the pinned build. The build entry point has been used previously; the fresh-directory reproduction procedure above still needs an independent run.

To rerun tests, reuse the script's configured environment in a subshell and execute both suites:

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

Run these commands in Bash. Record the actual command, source SHA, platform, pass/fail counts, and limitations. Use the new test counts; historical records do not replace fresh output.

## Progress and next steps

- [x] Pin the source SHA and verify the public repository, release tag, and asset metadata.
- [x] Verify the 128-file, +19,606/-75-line diff and 67 registered tools at the pinned evaluation commit; include their capability breakdown in the proposal.
- [x] Write an English proposal draft and evidence inventory with sources.
- [x] Write the demonstration flow, expected results, and source-reproduction entry point.
- [ ] Verify the evaluation package and running instance; rerun the core demonstration paths and automated tests.
- [ ] Produce a runnable minimal MCP reproduction script and redacted video.
- [ ] Update the proposal with fresh results, video, and reproduction-script links.
- [ ] Submit the proposal to [Feature Request](https://github.com/rustdesk/rustdesk/discussions/categories/feature-request) and follow up on scope feedback.

This stage is complete when maintainers can quickly understand the use case, see actual operation, find pinned source and an evaluation build, and clearly identify the limitations. A full port to current master and PR breakdown follow feedback on the direction.
