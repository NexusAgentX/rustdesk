# MCP SDK 与 Rust 1.75 技术选型研究

日期：2026-09-18。研究任务 A02 / Q01；**本文件是选型证据与建议，不是已接受的架构决策或实现验收**。只修改本报告，没有修改产品源码、Cargo.toml、Cargo.lock、工具链或共享项目状态文档。

后续决策（2026-09-18）：用户已接受自行实现集中 MCP 协议适配层，见 [D09](DECISIONS.md)。以下保留研究提交时的证据和建议；支持版本集合、产品集成及互操作仍待落实，研究实验不升级为产品验收结果。

开发目录：`/Users/laysath/Projects/rustdesk`，`codex/mcp-upstream`。参考实现现位于 `/Users/laysath/.codex/worktrees/rustdesk-mcp-reference/rustdesk`，`codex/mcp-controller`。研究期间工作区发生迁移，以下实验始终保存在仓库外。

## 结论

**找到能用 Rust 1.75 编译的旧 MCP SDK，但尚未找到同时满足 RustDesk 所需现代 HTTP 服务端、维护质量和 Rust 1.75 的可直接采用 SDK。** “社区没有任何 Rust 1.75 SDK”不准确；“兼容版本仍需自行补齐现代协议与传输，所以不能直接解决本项目问题”才是本次证据支持的判断。

建议采用主线已锁定的 Hyper / Tokio / Serde 组合，在 RustDesk 新功能模块内维护集中的 MCP 协议适配层；不手写 HTTP 解析器，不移植整套 SDK，不全局升级 Rust。这个组合已经通过 **Rust 1.75 的本机实际 HTTP 往返测试**和 **Windows i686 目标的 cargo check**。仍需完整协议互操作、产品集成、目标平台链接与运行验证，不能把依赖实验记成五平台实现通过。

重要协议更新：官方 `latest` 当前指向 **2026-07-28**，已取消协议级初始化会话，改为逐请求版本/能力元数据；2025-11-25 是需要考虑的客户端兼容版本，不能继续称作最新协议。建议一个集中的适配层支持 2026-07-28 与 2025-11-25 两种语义，随后依据实际客户端矩阵决定是否加 2025-03-26 / 2025-06-18。不能给所有版本共用一个“断流即取消”或 `initialize` 状态机。[官方版本入口](https://modelcontextprotocol.io/specification/latest)、[版本变化](https://modelcontextprotocol.io/specification/2026-07-28/changelog)、[兼容规则](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning)

用户约定的**同一个 PR、五平台 MCP 服务端、全部 67 项参考能力**不变。完整 RustDesk MCP 集成不等于实现 MCP SDK 的全部可选功能；未提供的可选能力不能声明为支持。

## 调查方法与证据位置

- 优先采用 crates.io 发布元数据、下载包中的规范化 `Cargo.toml` / 源码，以及项目官方仓库。README 自报 MSRV、协议覆盖或 conformance 通过率只视为声明。
- 最新发布版本与是否 yanked 取自 `https://crates.io/api/v1/crates/<crate>`。包源码取自 `https://crates.io/api/v1/crates/<crate>/<version>/download`。本次获取的 JSON 与解包源码保存在下述目录。
- GitHub 仓库维护状态取自仓库 API 的 `archived`、`pushed_at`；最近 push 不等于承诺维护旧版本。
- 实验根目录：`/Users/laysath/.codex/tmp/mcp-rust175-research/`。`sources/` 是发布源码，`experiments/` 含每项 Cargo.toml、Cargo.lock、源码、编译日志，`spec/` 保存查阅的官方规范 Markdown。
- 真正编译与测试都使用已有 `/Users/laysath/.cargo/bin/cargo +1.75.0`；rustc 为 `1.75.0 (82e1608df 2023-12-21)`，宿主 `aarch64-apple-darwin`。未安装或升级全局工具链。
- 某些宽泛依赖在 Cargo 1.75 新解析时拉入 edition 2024。为了区分“传递依赖漂移”与“库源码本身不兼容”，额外用**已有** Cargo 1.98 的 MSRV fallback 仅生成隔离项目锁文件，转换本地锁文件头到 v3，再由 Cargo/rustc 1.75 `--locked` 真正编译。该步骤不是声称 Cargo 1.75 能自动选择旧依赖，更不用于改动产品锁文件。
- 不做穷尽式库搜索；重点覆盖官方 SDK、几款独立成熟候选、明确声明 1.75 的候选，以及旧版可编译库。缺少 `rust-version` 不作为兼容证据。

## 候选比较

| 候选与检查版本 | 许可、维护快照 | 声明 / 实际 Rust 1.75 证据 | 协议与服务端覆盖、结论 |
| --- | --- | --- | --- |
| 官方 `rmcp` 3.4.0 / 参考使用的 3.3.0 | Apache-2.0；3.4.0 发布于 2026-09-15，活跃 | 两者声明 Rust 1.88、edition 2024；不能使用 Cargo 1.75 | 现代 Streamable HTTP、2026-07-28 与旧版兼容，能力丰富；工具链直接排除。非必要的 client/auth feature 关闭也无法解决 edition |
| `rmcp` 0.1.0 / 0.1.5 / 0.2.0 | 旧版 MIT/Apache-2.0；0.1.0 已 yanked | 包源码均 edition 2024；0.1.5 由 Cargo 1.75 实际解析失败 | 0.1 系列主要旧传输；0.2 引入 Streamable HTTP；降到这些版本仍无效 |
| `rust-mcp-sdk` 2.0.0 / 1.1.0 | MIT；2.0.0 发布于 2026-08-27，仓库活跃 | 声明 Rust 1.80；2.0 的 base64 0.23、time、schema/transport 等仍需单独审计，未声称实际最低版本等于 1.80 | 2.x 面向 2026-07-28，1.x 面向 2025-11-25，HTTP、SSE、会话/认证等较完整；高于 1.75，直接排除 |
| `rust-mcp-sdk` 0.1.0 | MIT；历史版本，不代表当前维护线 | 无 MSRV 声明；默认解析发生 schema / transport 类型不一致；固定 transport 0.1.0、schema 0.2.1 后 **1.75 check 通过** | 发布 README 明确仅 stdio、SSE 尚未提供；不能直接支持移动端嵌入 HTTP |
| `rust-mcp-sdk` 0.3.0 / 0.5.3 / 0.7.2 | MIT；历史版本 | 无 MSRV 声明；内置 HTTP 的 Axum 0.8 早期版本（至 0.8.4）声明支持 1.75，不能仅凭 major/minor 排除；0.7.2 关闭 Axum 后仍在 transport 0.6.2 编译失败，见 E11；0.5.3 HTTP 的实际依赖组合失败，见 E12 | 0.3/0.4.7 仅旧 SSE；0.5/0.7 有 Streamable HTTP、至 2025-06-18 的 schema，不能仅因 manifest 未声明 MSRV 判定兼容；未穷尽历史依赖组合 |
| `mcpkit` 0.1.0 / 当前 0.7.0 | MIT OR Apache-2.0；当前发布 2026-07-27 | 0.1.0 明确声明 1.75，**实际失败**：兼容传递依赖固定后 `mcpkit-transport` 的递归 async 函数 E0733；0.7.0 已要求 1.85 / edition 2024 | 0.1.0 宣称 2025-11-25；发布 HTTP POST 实际 echo，SSE/DELETE 有占位处理。不能靠小幅依赖锁定作为完整服务端使用 |
| `mcp-core` 0.1.50 | Apache-2.0；最后发布 2025-05-01，仓库最近 push 亦为 2025-05-01，未归档 | 无声明；默认新解析失败；兼容锁文件下 **1.75 core check 通过**，未验证可选 SSE feature | schema 有 2024-11-05 / 2025-03-26；提供 stdio 和 Actix 旧 HTTP+SSE，无现代 Streamable HTTP。能作为旧协议参考，不建议加进产品再自己改造 |
| `mcp-sdk` 0.0.3（AntigmaLabs） | Apache-2.0；最后发布 2025-01-20，仓库最近 push 2025-07-23 | 无声明；默认新解析失败；兼容锁文件下 **1.75 check 通过** | 发布 schema 的版本常量为 2024-11-05，旧 stdio 路线；现代协议/HTTP/安全适配仍需自行承担 |
| `mcp-protocol-sdk` 0.5.1 / 0.1.0 | MIT；仓库已归档，最后 push 2025-08-11 | 0.5.1 要求 1.85、edition 2024；0.3/0.4 声明 1.82；0.1.0 已 yanked，Cargo 新解析拒绝选择，未强行绕过 | 0.1 schema 为 2024-11-05；选择归档/yanked 旧版再维护不优于集中适配层 |
| `pmcp` 2.20.0 / 0.1.0 | MIT；当前发布 2026-09-05，仓库活跃 | 2.20.0 声明 1.91，最早 0.1.0 声明 1.80；不满足 1.75 | 现代全功能 SDK，但工具链排除；搜索结果里旧 Docker 示例的 rust:1.75 不能当证据 |
| `ra0x3/mcpkit-rs` | 官方 SDK 的独立 WASM fork，README 自称早期开发；最近 push 2026-03-10 | README 写 1.75+，但工作区 Cargo.toml 实际 `edition = "2024"` | 不因宣传文本采用；WASM 运行时也不解决 RustDesk 五平台嵌入需求 |
| `mcp-server` 0.1.0 | MIT；2025-02-27 的历史发布 | edition 2021，无 MSRV；未做额外编译 | 旧官方 SDK 前身依赖 mcp-spec / mcp-macros 0.1，stdio / 旧协议，未发现现代 HTTP 路线；不进一步投入 |

来源：

- [rmcp 3.4.0 manifest](https://docs.rs/crate/rmcp/3.4.0/source/Cargo.toml)、[rmcp 0.1.5 manifest](https://docs.rs/crate/rmcp/0.1.5/source/Cargo.toml)、[官方仓库](https://github.com/modelcontextprotocol/rust-sdk)。
- [rust-mcp-sdk 2.0.0 manifest](https://docs.rs/crate/rust-mcp-sdk/2.0.0/source/Cargo.toml)、[0.1.0 README](https://docs.rs/crate/rust-mcp-sdk/0.1.0/source/README.md)、[0.7.2 manifest](https://docs.rs/crate/rust-mcp-sdk/0.7.2/source/Cargo.toml)、[社区仓库](https://github.com/rust-mcp-stack/rust-mcp-sdk)。
- [mcpkit 0.1.0 manifest](https://docs.rs/crate/mcpkit/0.1.0/source/Cargo.toml)、[mcpkit transport 0.1.0 HTTP 源码](https://docs.rs/crate/mcpkit-transport/0.1.0/source/src/http.rs)、[递归函数](https://docs.rs/crate/mcpkit-transport/0.1.0/source/src/middleware/rate_limit.rs)。
- [mcp-core 源码](https://docs.rs/crate/mcp-core/0.1.50/source/)、[mcp-core 仓库](https://github.com/stevohuncho/mcp-core)、[mcp-sdk 0.0.3](https://docs.rs/crate/mcp-sdk/0.0.3/source/)、[AntigmaLabs 仓库](https://github.com/AntigmaLabs/mcp-sdk)。
- [mcp-protocol-sdk manifest](https://docs.rs/crate/mcp-protocol-sdk/0.5.1/source/Cargo.toml)、[归档仓库](https://github.com/mcp-rust/mcp-protocol-sdk)、[pmcp 2.20.0 manifest](https://docs.rs/crate/pmcp/2.20.0/source/Cargo.toml)、[pmcp 0.1.0 manifest](https://docs.rs/crate/pmcp/0.1.0/source/Cargo.toml)。
- [mcpkit-rs manifest](https://github.com/ra0x3/mcpkit-rs/blob/main/Cargo.toml)、[mcp-server 0.1.0](https://docs.rs/crate/mcp-server/0.1.0/source/)。

### 为什么 mcpkit 0.1.0 不能按其 1.75 声明直接采用

排除了新依赖解析问题后，1.75 实际编译到 `mcpkit-transport-0.1.0/src/middleware/rate_limit.rs:209` 失败。其 `check()` 使用 `Box::pin(self.check()).await` 递归；这仍不被 Rust 1.75 接受。声明值不足以证明源码兼容。

更重要的是 HTTP 实现本身：发布源码 `http.rs` 的 POST handler 返回收到的消息，尚未路由到 MCP handler；GET 只发一个 `connected` 事件；DELETE 未实际删除会话。即使维护一个兼容补丁，也仍需替换关键 HTTP 逻辑并重新验证协议语义。这里不是对当前 0.7.0 质量作结论，而是对唯一明确声称 1.75 的 **0.1.0 发布包**作结论。

## 可复现实验

以下命令均在 `experiments/<目录>` 执行，日志保存在同目录。没有使用 `--ignore-rust-version`，也没有修改下载的库源码来强行通过。

统一真正编译命令：

```sh
/Users/laysath/.cargo/bin/cargo +1.75.0 check --locked --target aarch64-apple-darwin
```

对无锁文件的初次尝试省略 `--locked`。兼容解析实验先执行以下步骤，随后执行上面的 1.75 编译；已有锁文件留存，可直接重跑 1.75：

```sh
/Users/laysath/.cargo/bin/cargo +1.98.0 generate-lockfile \
  --config 'resolver.incompatible-rust-versions="fallback"'
# 仅隔离实验目录：将生成的 Cargo.lock 顶层 version = 4 改为 version = 3。
```

| ID | 目录、配置 | 结果及意义 |
| --- | --- | --- |
| E01 | `rmcp-0.1.0`，`=0.1.0` / no-default / server；另直接解析下载包 0.1.5 | 0.1.0 已 yanked，新解析拒绝；0.1.5 `cargo +1.75.0 metadata --no-deps` 明确报 edition2024。不能把 yanked 失败当源码编译失败，两份证据分开 |
| E02 | `mcpkit-0.1.0-http`，no-default + `server,http,tokio-runtime` | 初次解析在 idna_adapter 1.2.2 的 edition2024 失败 |
| E03 | `mcpkit-0.1.0-http-resolved`，同 feature，兼容锁文件 | 实际编译到 SDK transport 后 E0733 失败。关键解析版本含 axum 0.7.9、reqwest 0.12.28、idna_adapter 1.2.0、uuid 1.20.0、async-lock 3.4.1；已跨过传递依赖失败点 |
| E04 | `mcp-core-0.1.50` / `mcp-sdk-0.0.3`，默认 features | 两者初次新解析均卡在 idna_adapter 1.2.2 / edition2024 |
| E05 | `mcp-core-0.1.50-resolved` / `mcp-sdk-0.0.3-resolved`，兼容锁文件 | **两者 1.75 check 通过**。该证据仅限宿主库编译；mcp-core 的可选 SSE 未启用；没有 MCP 客户端互操作证据 |
| E06 | `rust-mcp-sdk-0.1.0-base`，默认 features | 传递 transport 自动选到 0.1.2，与 SDK schema 的消息 trait 类型不一致，产生 14 个 E0277 |
| E07 | `rust-mcp-sdk-0.1.0-pinned`，额外精确固定 `rust-mcp-transport =0.1.0`、`rust-mcp-schema =0.2.1` | **1.75 check 通过**；stdio-only 历史版本，不是现代 HTTP 路线 |
| E08 | `mcp-protocol-sdk-0.1.0-http`，no-default + http | 0.1.0 已 yanked，新解析不能选择；未绕过、未编译源码 |
| E09 | `http-stack`，下表主线锁定版本，复制主线 Cargo.lock 到隔离项目后由 Cargo 1.75 保留所需依赖 | **1.75 test 通过，1 个本机 loopback HTTP POST / JSON 往返测试**；运行时仅由实验测试入口提供，库函数不创建运行时；不是 MCP 协议测试 |
| E10 | `http-stack`，同锁文件，`cargo +1.75.0 check --locked --target i686-pc-windows-msvc` | **通过**；仅交叉编译检查，无 Windows 链接、运行或 Win7 证据 |
| E11 | `rust-mcp-sdk-0.7.2-http`，no-default + `server,streamable-http,2025-06-18`，兼容锁文件 | 1.75 编译在 rust-mcp-transport 0.6.2 的 `utils/streamable_http_stream.rs:196` 因 E0716（临时值引用生命周期）失败；已避开 Axum，仍不能直接编译；未修改库源码 |
| E12 | `rust-mcp-sdk-0.5.3-http`，no-default + `server,hyper-server,2025_06_18`，兼容锁文件 | 解析出 Axum 0.8.4 / axum-server 0.7.2 / hyper-util 0.1.20，1.75 编译在 axum-server 的 HTTP2 executor trait bounds 因 E0277 / E0599 失败；这是该组合的失败，不是证明所有历史 pins 均不可能通过 |

实验最初有两次 feature 名写错（`mcpkit/tokio`、`rust-mcp-sdk 0.1.0/server`），已纠正重跑；对应原始日志保留，但不作为淘汰候选的证据。短暂下载重试不影响上述完成项，成功与源码失败均以各自日志末尾为准。

编译通过的旧 SDK 未运行协议互操作测试；失败 SDK 未改源码或无限向下穷举依赖。结论因此是“当前没有经过本项目约束验证的可直接采用现代 SDK”，不是“数学上不存在可修补/锁定的组合”。例如 rust-mcp-sdk 0.5.3 或许能通过进一步固定旧 Hyper 家族解决编译，但其协议停留在 2025-06-18，安全修复与当前协议回移仍需维护；建议仅在决定接受这项长期成本后继续投入。

### 可复用的 HTTP/JSON 依赖组合

| 包 | 实测版本 | 本次启用 feature / 用途 |
| --- | --- | --- |
| hyper | 1.7.0 | no-default，server + http1；HTTP 解析、连接状态、响应 |
| hyper-util | 0.1.17 | no-default，tokio；已有 Tokio IO 适配 |
| tokio | 1.44.2 | net、rt、macros、io-util、sync、time；产品接入现有运行时 |
| http-body-util | 0.1.3 | Limited / Full；限制读入字节，组合响应体 |
| bytes | 1.11.1 | 有界消息体缓冲 |
| serde / serde_json | 1.0.228 / 1.0.118 | JSON-RPC 信封与工具参数/结果 |
| base64 | 0.22.1 | 截图 image 内容编码 |

这些版本已经存在于调查时的主线 Cargo.lock。产品里仍需显式选择直接依赖与 feature，并检查 feature 合并对旧消费者的影响；“锁文件里有”不等于可以依赖另一个 crate 的私有依赖。不得大范围重新锁定工作区来照搬实验成功。Hyper 原生支持服务端 HTTP，选择它能避免新加另一套大框架；本实验没有验证安全策略、SSE 或完整协议。

## 五平台和旧 Windows 适用性

| 平台 / 消费者 | 当前证据 | 仍需验证 |
| --- | --- | --- |
| macOS ARM64 | HTTP 栈使用 Rust 1.75 编译并运行 loopback 往返；旧 SDK 的上述成功/失败发生于此 | RustDesk 完整构建按主线 1.81，UI / 服务生命周期 / 沙盒；Intel Mac 与最低系统版本 |
| Windows x86 / x64 / ARM64 | HTTP 栈 i686-msvc 的 1.75 check 通过；SDK 有 MSRV 硬阻碍的候选不会因目标改变而解决 | 完整链接、网络运行、安装/便携版、各架构；Win7 实机/VM上的系统 API 和依赖行为，不能从 Rust 版本或 check 推定 |
| Linux x64 / ARM64 | HTTP/JSON 部分无平台专用协议设计；这只是源码结构判断 | 编译、链接、运行与 X11/Wayland 会话集成；本报告未跨编译这些目标 |
| Android | 应复用同一嵌入式 TCP/JSON 服务；不依赖进程启动或本机 stdio 客户端 | ABI、NDK、应用私有凭据、前后台、端口可达性、网络切换；未实测 |
| iOS 真机 / 模拟器 | 同一协议模块原则上可用；无须把一个 SDK 的 server process 假设映射为另起进程 | 原生依赖、设备/模拟器分开构建、前后台挂起、同机其他 App 可达性；未实测 |
| Sciter/Web / feature-off | 集中模块可 cfg 隔离；不借此声称已有新增服务支持 | 检查未启用 feature 仍能解析依赖，Web 无 TCP 监听，旧代码路径保持；未实测 |

没有找到上述候选针对 Rust 1.75 + Win7 + Android/iOS 的完整共同保证。SDK 选择不能替代系统验证。已有 `mio` / Tokio 路径也应沿主线验证，而不是为新协议假定现代 Windows API 均可用。

## 推荐的集中协议适配范围

### 共同层：本项目必须实现

1. JSON-RPC 2.0：UTF-8；字符串/整数 request ID 正确回显；请求、通知、响应分类；格式错误、未知方法、参数错误、内部错误分别处理；无 ID 的通知不产生 JSON-RPC 响应。拒绝不属于所支持 MCP 版本的 batch 数组。解析失败不泄漏内部路径或凭据。
2. 工具层：稳定、确定顺序的 `tools/list`，完整 67 项能力映射；参数 schema 与真实校验一致；`tools/call` 返回文本、结构化结果及截图 `image` / Base64 / MIME。工具执行失败使用 `isError`；协议错误使用 JSON-RPC error，两者不混用。工具声明正确的副作用提示，但提示不能替代授权。
3. 状态边界：MCP 传输状态、经认证的客户端身份、远控会话/控制权、长任务 operation ID 分开。2026 无协议会话也不意味着没有应用会话；客户端名称可伪造，不能当身份。所有句柄绑定凭据主体与控制代次，不能猜中 ID 就跨客户端访问。
4. 生命周期：启动、显式停止、凭据撤销、App 挂起、人工接管、远端断开必须撤销写权限、取消未执行写操作并释放按键；服务停止不破坏人工远控。参数中的操作去重标识不能只依赖 JSON-RPC request ID，重试与断流不可重复执行破坏性动作。
5. 资源限制：HTTP header/body、JSON 深度/容器规模、同时连接/请求、截图尺寸/编码并发、结果体积、任务队列、会话数、SSE 队列、超时、操作与聊天历史全部有界。body streaming 读取时限制，不能读取任意大字符串后再检查。高耗时编码离开 UI / 网络线程。

工具语义依据：[2025 tools](https://modelcontextprotocol.io/specification/2025-11-25/server/tools)、[2026 tools](https://modelcontextprotocol.io/specification/2026-07-28/server/tools)。完整文件/终端/录制/隧道等能力仍由 RustDesk 业务工具实现，并不要求使用 MCP Resources、Sampling 或 Tasks 扩展才算实现。

### 2025-11-25 兼容层

- `initialize` 必须走版本/能力协商；客户端发来支持版本就返回同版本，否则返回服务端支持版本，不能盲目 echo 未实现版本。收到 `notifications/initialized` 后进入正常生命周期。实现该版本的 `ping` 与所支持通知语义。
- 单一 `/mcp` 的 POST 接受单条 JSON-RPC；接收通知/响应时成功返回 HTTP 202 空体；请求响应为 JSON 或 SSE。验证后续 `MCP-Protocol-Version`；初始握手不错误要求该后续 header。缺省版本/已协商版本与 unsupported header 按规范处理。
- GET 要么 SSE，要么正确返回 405；仅 JSON 回应是合法 Streamable HTTP 模式。是否开启独立 GET 流取决于是否真的提供异步 list-change 等通知，不因“Streamable”名称就额外实现所有可选流机制。
- 协议 session ID 是可选；本项目若用它管理兼容客户端生命周期，就落实加密随机 ID、凭据绑定、过期 404、缺失/无效会话、DELETE 撤销与清理，不把 session ID 当认证凭据。
- SSE 重放也是可选。只有实现 per-stream 有界重放、正确 `Last-Event-ID` 归属/过期语义后才声明可恢复。断流在此版本不自动表示取消；应处理 `notifications/cancelled`，不能取消 initialize，且与业务任务显式取消区分。

依据：[2025 lifecycle](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle)、[2025 HTTP / SSE](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)、[2025 cancellation](https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/cancellation)。

### 2026-07-28 当前协议层

- 必须实现 `server/discover`；不强制 `initialize` / `notifications/initialized`。逐请求读取版本、能力与客户端信息，响应按新版 schema 提供 `resultType`，列表结果提供必要缓存字段；对授权/设备相关结果使用私有缓存范围及保守有效期。
- 检查 HTTP header 与正文 `_meta` / method / name 一致，包括适用的标准路由 header 和编码规则；版本不支持与 header 不一致分别使用规范错误。不能用 2025 握手产生的状态覆盖新版单请求语义。
- POST 响应为 JSON 或请求内 SSE；新版不再用 GET 独立流、Mcp-Session-Id、Last-Event-ID。如果产品确需持续变化通知，使用新版 `subscriptions/listen` 并正确限定通知类型；不提供该能力时不宣告。
- 新版 SSE 响应流断开必须按该请求取消处理；取消不是回滚已生效动作。后台文件传输等若设计为返回业务 operation handle，就通过明确工具语义继续/取消，不让传输断开悄悄等于“业务永远继续”或“无条件重启”。
- 不为本项目新增已弃用的 Roots/Sampling/Logging 功能；Tasks、MCP Apps、MRTR 等只在实际使用和正确协商时实现/声明，不为 SDK 对等而无限扩张范围。

依据：[2026 versioning](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning)、[2026 HTTP](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http)、[2026 cancellation](https://modelcontextprotocol.io/specification/2026-07-28/basic/patterns/cancellation)。

### 认证与本地端点

- 协议要求验证存在的 Origin，非法值拒绝；本地服务建议只监听 loopback。项目进一步要求默认关闭、每请求认证、Host/端口白名单、严格 Origin 白名单、无凭据跨域不允许，避免 DNS rebinding。实际 IPv4/IPv6 绑定与移动访问方式分开验证。
- 初始建议使用用户明确配置的本地凭据，安全保存、可轮换/撤销、不出现在 URL、工具结果或日志中。该方案是 RustDesk 本地端点认证，**不是宣称完整实现 MCP OAuth 授权规范**。HTTP OAuth 标准的集成属于单独需定案的客户端互操作问题；不能伪造 OAuth discovery 来迎合只支持 OAuth 的客户端。
- MCP Authorization 本身可选，HTTP 实现采用它时应遵循该规范；如果最终客户端矩阵需要标准 OAuth，则应选成熟 OAuth 组件，并在同一 PR 完成需要的流程与测试，不能手写弱化的 OAuth 或把远端 RustDesk 密码复用为 MCP token。
- 本地 MCP 凭据认证、用户对 AI 控制授权、原版远端认证三者分别验证，SDK 的认证功能不能替代控制代次的最终输入检查。

依据：[当前授权规范](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization)、[HTTP 安全要求](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http)。

## 互操作与验收建议（本报告尚未执行）

1. 固定官方 TypeScript / Python 或 Rust SDK 的明确版本/提交，在仓库外或 CI 作为测试客户端运行。测试客户端可使用较新运行时，不进入 RustDesk 产品依赖和发行包。
2. 用官方客户端覆盖每个宣告的协议版本：发现或初始化、工具列表、文本/结构化/image 结果、工具错误、未知方法、非法参数、通知无回应、并发请求、超时、取消、版本/header 校验。图片至少解码检查尺寸/格式，不仅检查字符串非空。
3. 分别测 2025 和 2026 的断流/取消规则。HTTP/JSON 与 POST SSE、如果启用则 GET/SSE 或 subscriptions、session 过期、DELETE/重连、凭据撤销均记录实际报文与预期结果。跨主体不能复用会话或 operation handle。
4. 使用[官方 conformance](https://github.com/modelcontextprotocol/conformance) 的冻结协议版本要求集和适用场景。其“SDK 全能力 fixture”不是 RustDesk 产品应无条件暴露所有工具/Resources/Prompts 的依据；保留真实通过/失败/不适用理由，不能用 expected-failures 的绿色退出码冒充全部通过。固定工具版本，不用未固定的 latest 作为验收证据。
5. 协议互操作通过后，与五平台的 67 项能力清单相连；feature-off / enabled-but-off / enabled-on、Win7 与移动 lifecycle 分开测试。缺设备由社区补测，但代码失败和未实现不能归为缺设备。

推荐将协议 DTO、版本适配、HTTP 策略、工具分发与业务授权保持清晰边界，便于将来在主线允许提高 MSRV 后替换为官方 SDK。不要为这种未来替换给整个 RustDesk 增加新的通用 trait 或改造现有非 MCP 调用方。

## 备选与未决事项

- **备选：维护一个旧 SDK 分支。** 只有当上游愿意持续维护 1.75 且能减少本项目代码、提供完整 HTTP/互操作证据时才重新考虑。当前 mcpkit 的编译和 HTTP 缺口、其他旧 SDK 的协议缺口意味着我们会同时承担旧 SDK fork 与 RustDesk 适配两层维护，不是更省事的默认选择。
- **暂不推荐：独立新版 Rust sidecar。** 桌面可以绕开 app MSRV，却引入多进程发行/认证/IPC；Android/iOS 的嵌入式部署与生命周期更复杂，不能据此把移动端拿掉。
- A02 还不能标“实现完成”：需由主代理接受选型、固定支持协议集合、完成官方客户端实测及全工作区依赖解析检查。本报告验证的是可行依赖基础。
- 需要正式确定本地认证与客户端配置体验，特别是只支持 OAuth 的客户端、移动同机 AI App 与跨设备访问；不能在依赖选型阶段假定这些客户端均可连通。
- 需要检查所选固定依赖的已知安全通告与主线锁文件策略；本次没有执行完整依赖安全审计，也不建议用无限期冻结版本替代补丁维护。
- MSRV 证据不等于最旧 OS / ABI 证据；尚未执行 Linux、Android、iOS 目标编译和 Windows 实机运行。缺少设备不影响继续开发，但最终报告必须保留这些区分。
