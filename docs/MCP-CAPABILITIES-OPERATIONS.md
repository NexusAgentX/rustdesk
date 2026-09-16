# MCP 能力与操作查询

对应 [实施任务 #1](https://github.com/NexusAgentX/rustdesk/issues/1)。仅改变控制端，原版被控端无需更新。

## 能力查询

`rd_capabilities_get({"session_ref":"r_…"})` 是只读操作，人工控制期间也可调用。
返回 `peer`（版本、平台、认证、连接状态、已知权限）、`capabilities` 和 `contract`。

每项能力包含：

- `implemented`：当前 MCP 已实现；此列表不把仅在 GUI 中存在的功能当作 MCP 能力。
- `supported`：会话类型及已协商功能是否支持；`null` 表示尚未确定。
- `allowed`：权限；`null` 不代表授权。需要远端权限的操作在执行时仍重新验证。
- `available`、`blockers`：当前条件判断和原因，如 `support_unknown`、`unsupported_for_session`、`permission_unknown`、`permission_denied`、`human_control`、`control_transition`、`not_ready`、`session_closed`。
- `tools`、`scope`、`requires_ai_control`：关联工具、作用范围、是否需要 AI 控制权。

这些是查询瞬间的状态，不是原子授权凭据，也不保证已有可用帧、终端实例或应用结果。每项能力在后续实现时应补充其版本、平台、权限、驱动和协商条件，不应仅凭版本猜测权限。

## 操作查询

写调用主动传 `operation_id` 可使用既有去重机制；同一 MCP 客户端内相同 ID 与参数只执行一次，不同参数返回 `OPERATION_CONFLICT`。

`rd_operation_get({"operation_id":"example","wait_ms":1000})` 只查询，不重发写操作；无需额外会话引用。只能查询当前逻辑 MCP 客户端创建的记录，绑定失效时仍受原有读权限校验约束。`rd_session_get` 原有的操作查询也保留。

- `wait_ms` 为 0..30000，默认 0，仅等待本地工具执行结束。
- 返回 `data.operation` 为原始调用结果，内含 `operation` 元数据；外层 `completed` 只表示查询完成。
- 元数据 `tool` 标识原始工具；不返回原始参数或认证凭据。

| execution_state | outcome | 含义 |
| --- | --- | --- |
| running | pending | 本地工具仍在执行 |
| finished | reported | 本地调用已完成，结果的证据强度以原始工具契约为准 |
| finished | unknown | 原始调用以 pending 结束，尚无最终远端结果 |
| finished | partial | 原始工具报告部分执行 |
| cancelled | unknown | 调用被取消，已经发送的操作不能撤回 |
| failed | unknown | 工具报告错误；不能据此保证远端未发生任何变化 |

例如断开请求的有界等待超时后，操作记录保留原始 pending 结果；使用 `rd_session_get` 观察实际连接状态。查询不会将旧的 pending 记录改写为远端成功，也不会重新断开。终端同理由终端状态查询观察。后续文件任务应查询文件任务自身的进度和完成事件。

完成记录保留 300 秒，每个客户端最多 256 条；图片/附带输出另有 30 秒缓存。客户端结束后记录清理。不存在、超期或其他客户端的 ID 返回 `OPERATION_EXPIRED`。

## 后续设置接口约定

设置应读取实际值并接受明确值（例如 `enabled: true`），不能要求调用方猜测当前状态后 toggle。每个设置说明作用范围：`session`、`peer_preference`、`global` 或 `local_window`，以及是否立即生效、是否持久化。此批次只制定契约，具体设置接口随对应功能实现。

写入继续检查绑定、连接代次和 AI 控制权；重连后刷新引用，控制权撤销后不得继续发送队列中的写入。错误沿用 `code/message/retry/details`，不在错误或操作记录中加入密码、验证码或原始认证参数。

## 0.1.1 验证记录（2026-09-16）

- macOS ARM64 release 构建、Flutter 打包及安装后的签名校验通过。
- 11 项 MCP 测试、40 项 automation 测试通过；包含超时/过期、跨客户端隔离、重复请求与冲突、未知/拒绝权限及人工控制状态。
- 原版 Windows RustDesk 1.4.9 双显示器实测：能力与版本查询、截图、错误会话类型拒绝、人工接管后的输入拒绝、运行中操作查询、重复调用去重、冲突拒绝、另一客户端不可查询操作、控制权撤销中断等待、断开后的未知结果、重连认证和首帧恢复。
- 终端连接实测：能力查询、列表、读取、调整大小、关闭终端及关闭会话。
- 测试按实际状态等待认证后的首帧和重新绑定后的截图缓存，不将认证完成当作画面已就绪。
- Python MCP SDK 对服务接受 DELETE 后返回的 HTTP 202 打印终止提示；服务清理与后续重新绑定正常，HTTP 会话隔离测试通过。
