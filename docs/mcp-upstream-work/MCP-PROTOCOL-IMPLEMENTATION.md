# MCP 协议与 HTTP 模块实施记录

更新：2026-09-18。分支 `codex/mcp-upstream`；本记录对应尚未提交的模块代码。路线为 D09，自实现协议适配，HTTP/JSON 使用 Hyper/Serde。**这里交付的是可嵌入的协议模块及测试，不是整个五平台、67 项业务能力 PR 已完成。**

## 文件与接入边界

| 文件 | 职责 |
| --- | --- |
| `src/mcp/mod.rs` | 配置、资源上限、工具注册、loopback bind/run；不会创建 runtime |
| `src/mcp/model.rs` | 工具描述、文本/结构化/图片结果、RPC 错误、handler/context 契约 |
| `src/mcp/protocol.rs` | 按版本分开的初始化/发现、会话、分发、请求取消、错误语义 |
| `src/mcp/http.rs` | Hyper HTTP/1 服务、认证入口、Host/Origin、消息大小与超时、连接关闭 |
| `src/mcp/security.rs` | 客户端独立凭据、主体/凭据代次、撤权、哈希存储与常量时间哈希比较 |
| `src/mcp/runtime.rs` | 由移动集成代理编写：接入调用方 Handle，默认 Disabled，启停/挂起/恢复监督器；其应用接入另见 MOBILE-INTEGRATION.md |
| `src/mcp/tests.rs` | 仅测试注册的 fixture 和真实 TCP/HTTP 边界测试 |
| `tests/mcp/interop.mjs`、`README.md` | 固定官方客户端互操作与正式复现说明 |
| 根 Cargo.toml/Cargo.lock、src/lib.rs | 可选 `mcp` feature、必要直接依赖、两行 cfg 模块声明 |

本任务没有改远控业务、Flutter 业务 UI、生成桥接、hbb_common、通用 core trait 或参考分支。`src/flutter.rs` 等由移动代理独立拥有。正式应用只有调用方注册的工具会公开；空 registry 返回空列表，不伪造 67 个成功工具。

## 固定协议要求与实际选择

来源以版本化规范及同版本官方 schema 为准：

- [2025 生命周期](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle)、[2025 HTTP](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)、[2025 取消](https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/cancellation)。
- [2026 版本协商](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning)、[2026 HTTP](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http)、[2026 工具](https://modelcontextprotocol.io/specification/2026-07-28/server/tools)。
- 官方 schema：[2025 JSON Schema](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/schema/2025-11-25/schema.json)、[2026 JSON Schema](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/schema/2026-07-28/schema.json)。实验目录保存了两份下载副本。

| 项目 | 2025-11-25 | 2026-07-28 |
| --- | --- | --- |
| 入口 | `initialize` 协商该 legacy 版本；不支持的初始化版本返回本服务可接受的 legacy 版本，让客户端决定是否继续 | 不要求 initialize；`server/discover`，每次请求独立版本/能力元数据 |
| 正常状态 | 成功初始化发出随机 session ID，收到 `notifications/initialized` 才开放工具；ping 可在初始化通知前使用 | 无协议 session；不会创建或回显 session ID |
| 版本/header | 后续验证版本；已有会话缺版本 header 时沿用协商值；会话缺失 400、失效/跨凭据 404 | 必须有版本、Mcp-Method，以及适用的 Mcp-Name；与正文相符；解码 Base64 sentinel；不符 400/-32020；不支持版本 400/-32022 |
| 列表/结果 | `tools/list`、`tools/call`，传统结果结构 | `resultType: complete`、服务端信息 `_meta`；发现和列表 `cacheScope: private`、`ttlMs: 0` |
| 未知 RPC | JSON-RPC -32601 | HTTP 404 + JSON-RPC -32601 |
| 会话终止 | DELETE 删除绑定凭据的会话并取消活动请求；有上限和空闲过期 | DELETE 405；无 session 功能 |
| 通知 | initialized/cancelled；接受的通知返回 202 空体；不会把无 ID tools/call 执行为工具 | 核心 HTTP 目前无客户端通知；不声明扩展通知；接受的未知通知只返回 202，无工具副作用 |
| 取消 | 显式 `notifications/cancelled` 按凭据、session、request ID 定位；TCP 断开不自动取消 | 本实现选择 JSON 响应，JSON 请求断开也撤销未完成 handler（项目策略）；规范的 SSE 断流取消规则没有被冒充为 JSON 的强制要求 |

共同 JSON-RPC 支持字符串/整数 ID、格式错误/信封错误/参数错误/未知方法/内部错误，拒绝 batch、null/小数 ID；使用 Serde 默认递归深度限制及有界 body。工具执行失败用 `isError: true`，RPC 失败用 `error`。图片通过 MIME + Base64 内容返回，结构化结果选择两版都接受的 object 子集。不存在服务端主动 RPC 请求，所以意外客户端 response 被拒绝。

功能声明只包括 `tools: {}`。选择规范允许的单 JSON 响应方式：无 SSE、GET 返回 405；未声明 listChanged、resources、prompts、subscriptions、sampling、elicitation、logging、roots、tasks、MRTR、SSE 重放或 2024 旧 SSE。长业务操作可返回独立 operation handle，但需要业务层明确实现取消/去重与权限，不能把它们伪装成 MCP Tasks。

registry 在 bind 时拒绝重复/非法工具名、非 object 输入/输出 schema 及尚未支持的 `x-mcp-header` 自定义路由标注，避免公开承诺无法验证的 header。业务 handler 负责参数与 outputSchema 的实际语义校验；协议层不是通用 JSON Schema 引擎。

## 安全与资源策略

- 仅允许 loopback 地址；Host 必须为 localhost/127.0.0.1/::1 加实际端口，拒绝重复安全 header。Origin 默认全部拒绝；调用方可显式配置精确的 HTTP(S) origin 白名单，不接受 `null`、通配符、userinfo、路径/query。无 Origin 的非浏览器客户端仍需凭据。
- 每个请求，包括 GET/DELETE，都认证 Bearer 凭据。401 有 WWW-Authenticate；凭据不在 URL、日志或错误信息中。库仅存 SHA-256 哈希；`issue` 返回的原始 token 只交调用方安全保管。token 使用两个随机 UUID v4 的随机位，不使用可猜测时间/计数器。
- 每次 insert 建立随机 credential 代次；相同 principal ID 撤销后重新登记也不能继承旧协议 session/取消权。`CallContext` 包括 principal、credential、协议版本、取消 token，后续业务句柄应绑定前两者及业务控制代次。可伪造的 clientInfo 不能用来识别主体。
- 这是本地预配置凭据认证，**没有实现或宣称 OAuth 授权发现/流程**。[MCP 授权规范](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization) 将该能力列为可选、HTTP 实现 SHOULD 遵循其流程；需要 OAuth-only 客户端时必须另行完成集成并验证，不能宣称它们已经可用。两款实测官方客户端均使用明确配置的 Authorization header。loopback 和 Host/Origin 不提供应用间隔离，不能替代令牌与用户控制授权。
- 默认上限：64 连接、32 同时 HTTP 请求、16 工具调用、64 legacy sessions；body 1 MiB、response 16 MiB、HTTP buffer 16 KiB、32 headers；header/body 10 秒、单次 handler 120 秒、session 空闲 1 小时。配置为零或超出 semaphore 可接受范围会被拒绝。HTTP 由 Hyper 解析，输入流经过 Limited，而非先无限读完。序列化通过有界 writer，超限返回小型内部错误。Serde JSON 默认深度限制保留。
- 连接或调用达到上限时拒绝，不创建无界等待任务。legacy 断流后 worker 仍占用工具并发槽且有超时。撤权/停服/session DELETE 或过期取消相应工作。停服等待活动 handler future drop 后才返回，恢复不会在旧 handler 尚未退出时重新监听。
- 业务 handler 必须保持 future 构造无副作用，实际执行前复查控制权，在取消/drop 时停止队列并释放其资源。不得阻塞 Tokio；编码/阻塞系统调用应使用受限的 spawn_blocking 或业务执行器。协议层只能约束接收和发送，无法阻止一个错误业务 handler 先在内部分配超大截图，业务层仍需单独限制图片/历史/队列和内存。

真实远端认证、用户授予 AI 控制权、人工接管、已按下按键释放、操作句柄去重/过期均尚待业务接入，不能用 fixture 或取消 token 代替验收。

## 依赖、兼容与最小差异

可选直接依赖为 Hyper 1.7 (`server,http1`，无默认 feature)、hyper-util 0.1.17 (`tokio`，无默认 feature)、http-body-util 0.1.3、Tokio 1.44 (`net,rt,macros,io-util,sync,time`)、tokio-util 0.7 (`rt`)、base64 0.22、subtle 2.6、tracing 0.1。复用已有 serde/serde_json、bytes、sha2、uuid。

实际 lock 保留原包版本：hyper 1.7.0 / hyper-util 0.1.17 / tokio 1.44.2 / tokio-util 0.7.15 / serde 1.0.228 / serde_json 1.0.118。新增一个 Hyper HTTP/1 server 需要的传递包 `httpdate 1.0.3`；Cargo.lock 总共 14 行新增，无已有包升级、删除或 blanket update。先手动补根依赖边，再用 `cargo +1.75.0 metadata --offline` 让 Cargo 正常补必要锁项；最初 `--locked` 检出缺 httpdate，补完后复验通过。

`mcp` 默认关闭；src/lib.rs 只有 cfg 模块声明。产品 metadata 的默认 root 依赖里只有原有 dev Tokio，没有新增 MCP 可选直接依赖；打开 mcp 才出现这些依赖。协议源码不改任何旧远控业务路径。移动代理的 Flutter runtime feature 分支需另做其相应回归；本记录不替它声称完整应用 feature-off 运行通过。

## 实测与复现

实验根目录：`/Users/laysath/.codex/tmp/rustdesk-mcp-protocol/`。所有 target、npm node_modules、日志和 token 测试运行环境都在仓库外。工具：Rust/Cargo **1.75.0**，宿主 aarch64-apple-darwin；格式化使用已安装 rustfmt 1.81.0（1.75 未安装 rustfmt，没有额外安装/升级全局工具链）；Node **24.19.0**。

隔离 `harness/Cargo.toml` 使用主线精确依赖，`harness/src/lib.rs` 直接 `#[path = "/Users/laysath/Projects/rustdesk/src/mcp/mod.rs"] pub mod mcp;`，没有复制一套实现。features default=mcp，模块整个在 cfg(mcp) 下。该 harness 避开 RustDesk 图形/编解码原生依赖，**不是完整产品 cargo check/build**。

| 检查 | 结果 / 证据范围 |
| --- | --- |
| harness Rust 1.75 `test --locked` | **15 通过、0 失败、1 ignored**；ignored 是需外部客户端的测试，已单独实际运行；包含移动代理 3 个 runtime 测试 |
| 官方客户端测试 `official_typescript_clients -- --ignored --nocapture` | **1 通过**；真实 loopback HTTP：sdk 1.30.0 → 2025-11-25；client 2.0.0 → 2026-07-28 |
| harness `check --locked --no-default-features` | **通过**；模块不编入；不是应用所有 native 依赖验收 |
| harness `check --locked --target i686-pc-windows-msvc` | **通过**；没有 Windows 链接/运行或 Win7 验证 |
| harness `check --locked --target aarch64-linux-android` | **通过**；没有 APK/Android 网络或生命周期运行证据 |
| harness `check --locked --target aarch64-apple-ios-sim` | **通过**；没有 simulator app 链接/运行证据 |
| harness `check --locked --target aarch64-apple-ios` | **通过**；没有 iOS 真机链接/签名/运行证据 |
| 根工作区 `metadata --locked --offline --format-version 1`，及追加 `--features mcp` | **均通过**，工作区解析/锁文件一致；不是 cargo check |
| diff minimization / `git diff --check` | 已检查自有共享改动均必要、无已有代码重构；通过 |

模块测试实际覆盖两版生命周期、工具文本/结构化/图片、工具失败和 RPC 失败、无凭据/错误凭据/Host/Origin、明确允许 Origin、跨主体 session、token 撤销与重新插入、body 上限、session 过期/删除、并发工具上限、重复活动 ID、显式取消、两版断流差异、超时、停服等待 drop、响应上限、header 超限、body 慢请求、编码 Mcp-Name、版本/header 不符、通知无工具副作用。已完成请求 ID 不提供永久去重；业务幂等必须另实现。

官方测试同时检查新版 wire `resultType/cacheScope`、legacy 初始化通知、新版无 initialize、真实工具结果、解码 PNG 签名/尺寸/压缩数据，以及 isError 和 RPC error 的区别。固定依赖及完整 npm 锁文件保存在 `clients/package.json` / `clients/package-lock.json`；产品不依赖 Node 或这两个 SDK。没有运行 conformance 全能力套件，也没有设置 expected-failures 冒充通过。

主要命令（`R` 只是复现用局部路径变量）：

```sh
R=/Users/laysath/.codex/tmp/rustdesk-mcp-protocol
CARGO_TARGET_DIR="$R/target" cargo +1.75.0 test --locked --manifest-path "$R/harness/Cargo.toml"
MCP_CLIENT_DIRECTORY="$R/clients" \
MCP_INTEROP_SCRIPT=/Users/laysath/Projects/rustdesk/tests/mcp/interop.mjs \
CARGO_TARGET_DIR="$R/target" cargo +1.75.0 test --locked \
  --manifest-path "$R/harness/Cargo.toml" official_typescript_clients -- --ignored --nocapture
CARGO_TARGET_DIR="$R/target" cargo +1.75.0 check --locked --no-default-features \
  --manifest-path "$R/harness/Cargo.toml"
CARGO_TARGET_DIR="$R/check-i686-pc-windows-msvc" cargo +1.75.0 check --locked \
  --manifest-path "$R/harness/Cargo.toml" --target i686-pc-windows-msvc
CARGO_TARGET_DIR="$R/product-target" cargo +1.75.0 metadata --locked --offline --format-version 1
CARGO_TARGET_DIR="$R/product-target" cargo +1.75.0 metadata --locked --offline --features mcp --format-version 1
```

另三个交叉目标使用同样命令及各自独立 target 目录。日志为 `test.log`、`interop.log`、`feature-off.log`、`check-<target>.log`、`product-metadata*.log/json`。没有将凭据写入日志。

过程中发现并修正：官方新版 SDK 聚合 listTools 后去掉缓存字段，初次断言 API 对象导致失败，改为捕获并校验真实响应字段；Hyper 1.7 对超大 header 可返回 400，边界测试改为接受明确拒绝的 400/431；移动 runtime 测试曾假设停服只会 EOF，移动代理修正 macOS ConnectionReset 关闭情形。这些失败没有被当成成功，也未降低业务验收要求。

## 后续接入契约与未完成项

```rust
let credentials = Credentials::new(max_clients);
let (principal, token) = credentials.issue()?; // 调用方保护 token，不记录日志
let server = Server::bind(ServerConfig::local(info), credentials, registered_tools).await?;
let address = server.local_addr()?;
server.run(shutdown_token).await?; // 调用方现有 Tokio runtime
```

移动 runtime Service 接收现有 Handle；显式 enable 才 bind，后台/停用取消，恢复重绑；详细 API/实际应用接入由移动报告负责。桌面服务/UI/配置持久化也应复用这个边界，不另造独立隐藏 runtime。

剩余：67 项业务 handler、控制权与输入释放、真实平台生命周期、凭据安全存储/管理 UI、最终客户端兼容矩阵（含是否需要 OAuth）、整个产品及 feature-off 构建/运行、Linux/各桌面架构/Win7 与移动模拟器和实机。上述交叉 check 不等于这些工作已完成。缺设备可最后社区补测；未写代码或失败不能归入缺设备。
