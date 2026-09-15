# RustDesk MCP 接口与服务设计草案

更新时间：2026-09-15。状态：**第一版设计提案，待需求方审阅定稿，尚未实现**。

依据：[MCP 主控客户端需求](MCP-CONTROLLER-REQUIREMENTS.md)。已确认按能力拆分工具；下列具体命名、字段和调用契约供需求方定稿，不因为写入文档而自动成为已批准接口。

底座：RustDesk `1.4.9` / `6c578292e8ebbbec708b76986ba8c4bc7c509747`；首个平台为本机 macOS / ARM64。SDK 采用已选定的 `rmcp =3.3.0`，HTTP 使用 Axum 0.8 系列。

## 1. 接口设计原则

1. 按会话、控制权、画面、输入、终端拆分，共提议 **20 个工具**。使用 `rd_` 前缀和英文 `snake_case`，工具描述和错误说明使用简明英文，GUI 文案继续本地化。
2. 不设隐式的“当前会话”“当前显示器”“当前终端”。每次操作明确指定目标。
3. 打开或绑定不等于认证完成，取得 AI 控制权不等于远端已经授权，发送输入不等于远端应用执行成功。
4. 状态和结果返回可解析的结构；截图同时返回 MCP 原生图片，终端保留原始字节。
5. 请求身份取自已认证的 MCP 连接上下文，工具参数不能指定或伪造 `ai_id`。
6. 只读工具不自动接管；写入被拒绝后不暗中请求批准、不排队等到下次接管。
7. 只为已有长生命周期对象提供查询：会话状态、批准请求、终端。首版不引入通用任务系统或任意代码执行工具。

## 2. 对象、标识与隔离边界

| 名称 | 含义与有效期 |
| --- | --- |
| `ai_id` | 服务内部为一次逻辑 MCP 会话分配的展示标识；与认证令牌、`Mcp-Session-Id` 分离 |
| `peer_id` | 远端设备 ID，字符串；不能替代本地会话 ID |
| `session_id` | 桥接层分配的本地逻辑远控会话 ID，如 `s_…`；对应一个实际 RustDesk 核心会话 |
| `ui_session_ids` | 官方 Flutter 的窗口/视图会话 ID 集合，仅在状态中用于对应 GUI；不作为 MCP 操作目标 |
| `binding_id` | 一次 AI 绑定的不可复用标识，如 `b_…`；解绑或 AI 离开后失效 |
| `control_token` | 一次 AI 控制权授予的不可复用标识，如 `c_…`；控制权撤销后失效，不能单独用于认证 |
| `connection_epoch` | 远端连接代次；重连后递增，防止把旧画面、事件或输入用于新连接 |
| `display_id` | 当前连接代次内的显示器标识；不是本机屏幕编号 |
| `snapshot_id` | 一张已返回截图及其坐标映射的标识，如 `f_…`；与绑定、连接代次、显示器布局关联 |
| `terminal_id` | 桥接层分配的终端实例 ID，如 `t_…`；映射到所属核心会话及官方终端数字 ID |
| `approval_id` | 一次接管批准请求的标识，如 `a_…`；用于查询、去重及取消 |
| `operation_id` | AI 为一次有副作用的调用提供的重试标识；与 JSON-RPC 请求 ID 不同 |

### 2.1 一个核心会话的多个窗口共同独占

官方 `src/flutter.rs` 的 `sessions::SESSIONS` 按 `(peer_id, ConnType)` 保存核心会话，核心会话下另有多个 UI session。因此不能把每个窗口 UUID 都当成可由不同 AI 独占的连接。

- 一个实际核心会话只分配一个 `session_id`，包含全部对应 GUI 视图。
- AI 的绑定和控制模式作用于这个核心会话及其全部视图。增加显示器窗口不创建新的 AI 独占单元。
- 独占不升级为设备级锁。同一远端的桌面连接与终端连接是不同类型的核心会话，可以分别绑定，但 GUI 要清楚显示归属。
- AI 重新绑定产生新的 `binding_id`；旧调用即使来自同一个 AI、使用相同 `session_id`，也不能作用于新绑定。
- GUI 设置页使用 `ai_id → session_id → ui_session_ids / terminals` 展示关系。

### 2.2 终端与桌面连接

官方终端使用 `ConnType::TERMINAL`；`TerminalConnectionManager` 让同一设备的多个终端标签共享终端连接。

- `session.kind` 为 `desktop` 或 `terminal`。
- `rd_session_open(kind="terminal")` 打开可见终端窗口，并按官方流程创建首个终端标签；结果带回该终端的 ID，即使连接仍在等待认证。
- `rd_terminal_create` 在已绑定的终端连接下增加可见标签，不创建隐藏 shell。
- 同一终端连接下的全部标签共享 AI 绑定和人机控制模式。首版不让不同 AI 分别占用同一核心连接的不同终端标签。
- 桌面截图和图像坐标输入仅支持 `desktop`；终端工具仅支持 `terminal`。类型不匹配返回 `WRONG_SESSION_KIND`。

## 3. 公共参数、状态和返回格式

### 3.1 参数约定

所有工具的参数都是 JSON object，拒绝未知字段，不接受任意透传到 FFI 的 JSON。ID 均为不透明字符串，不要求 AI 解析。

下文参数表使用以下缩写，实际 JSON 中展开为普通顶层字段：

- **S**：`session_id: string`，必填。
- **B**：S 加 `binding_id: string`，必须属于当前 AI。
- **O**：`operation_id: string`，1～64 个 ASCII 字母、数字、`_` 或 `-`，必填。
- **W**：B、O 加 `control_token: string`，用于执行 AI 写入。
- `x?: type = value` 表示可选字段及默认值；未标问号的字段必填。
- 时间戳输出 RFC 3339 UTC；等待时长单位为毫秒，超时判定用单调时钟。
- `revision`、帧序号和字节偏移用十进制字符串，避免跨语言整数精度丢失。

### 3.2 会话状态

`SessionState` 至少包含：

| 字段 | 形状 |
| --- | --- |
| 标识 | `session_id`, `peer_id`, `kind`, `ui_session_ids: string[]` |
| GUI | `gui: {registered: boolean, visibility: "visible" / "minimized" / "hidden"}` |
| 连接 | `connection: {state, epoch, authenticated: boolean, error: ErrorInfo / null}` |
| AI 归属 | `owner: "self" / "other" / "none"`；仅自己的绑定返回 `binding_id` |
| 控制 | `control: {mode: "human" / "ai", approval_required: boolean, approval: Approval / null}`；仅当前拥有 AI 控制权者返回 `control_token` |
| 能力 | `capabilities`，每项含 `supported: boolean / null`, `allowed: boolean / null`, `reason: string / null` |
| 平台 | `platform: string / null`，来自远端报告 |
| 显示器 | `layout_revision` 与 `displays[]`，每项含 ID、名称、原点、远端坐标尺寸、解码尺寸、是否主屏 |
| 终端 | `terminals[]`，含终端 ID、打开状态、尺寸与最近错误，不默认夹带输出 |
| 同步 | `revision`, `updated_at`；状态变更递增 revision，视频帧不造成状态查询持续唤醒 |

连接状态：`connecting`, `awaiting_auth`, `awaiting_human`, `awaiting_frame`, `ready`, `disconnected`, `closed`。

- `awaiting_auth` 同时给出 `auth_challenge: {id, kind, fields}`；`kind` 可为 `password`, `two_factor`, `os_login`。
- 其他认证、远端确认或会话选择需求进入 `awaiting_human`，返回 `human_action: {kind, message}`；首版不伪造通用“点击任意认证对话框”接口。
- `desktop` 的 `ready` 需要认证完成及可用远端帧；`terminal` 的 `ready` 需要认证完成及终端能力确认，不要求屏幕首帧。终端是否真正打开另看终端状态。
- `ready` 不保证全部写入能力获准；`allowed: null` 表示尚未获知，不能当作允许。

### 3.3 统一工具结果

每个工具定义自己的 `inputSchema` 和 `outputSchema`。输出顶层结构统一为：

```json
{
  "ok": true,
  "status": "completed",
  "data": {},
  "error": null
}
```

- `ok=false` 对应 MCP `isError=true`。正常等待状态或读取超时但没有新数据可以 `ok=true`。
- `status` 为 `completed`, `pending`, `unchanged`, `partial`, `failed`。`completed` 仅表示本工具约定的动作完成，具体发送证据另有字段。
- `data` 按工具类型定义；发生部分执行时仍返回进度，不能因错误丢掉已发送范围。
- 已接受的有副作用调用另返回顶层 `operation: OperationState`，包含重试 ID、进度和去重到期时间；读取结果及接受前的校验失败可以省略该字段。
- `error` 为 `null` 或 `{code, message, retry: "never" / "after_state_change" / "same_operation", details}`。
- MCP `structuredContent` 放上述对象；`content` 的第一个 text block 放其序列化 JSON。图像另外放原生 image block，JSON 只放元数据，不重复嵌入大段图像 base64。
- 错误信息必须可用于决定下一步，不能只返回 `false`、空字符串或底层日志。

协议结构错误、未知工具使用 JSON-RPC 错误；工具参数内容无效、认证未完成、权限和控制权拒绝等使用工具错误。HTTP 认证失败在 MCP 分发前返回 401；不允许的 Origin 返回 403。

### 3.4 重试与部分执行

- 所有带 O 的调用按 `(ai_id, operation_id)` 去重，相同 ID 必须使用相同工具和语义参数；重用 ID 配不同内容返回 `OPERATION_CONFLICT`。
- 同一操作的重复调用返回原进度或结果，不重新发输入。去重记录保存到完成后 5 分钟，返回 `dedupe_expires_at`；每个 AI 最多 256 条，未到期记录不提前逐出，满时拒绝新的写入并给出可重试时间。
- 让出、解绑和取消等清理动作本身按绑定/批准标识幂等，不因去重记录容量耗尽而被阻止；必要时不新增操作记录，返回清理后的真实状态。
- 参数摘要使用服务进程内随机密钥的 HMAC，记录不保留密码明文；敏感参数和终端输入不写普通日志。
- 不承诺跨 MCP 重连、进程重启或去重有效期外的恰好一次执行。响应丢失时在原逻辑 MCP 会话内用原 ID 重试；跨会话后先读取状态和画面，不能盲目补发。
- 输入结果返回 `delivery: "not_sent" / "queued" / "sent" / "unknown"`、`completed_actions`、`sent_events` 及可选 `failed_action_index`。`sent` 只表示交给已有远控发送路径，不是远端应用确认。
- 打开/关闭终端等有远端响应的操作以明确事件确认；无远端应答的 resize/write 只返回发送证据。
- 长等待使用查询工具，每次最多 30 秒。HTTP 响应结束或 SSE 重连不取消已接受操作；标准 MCP cancellation 取消尚在执行的调用，桥接层再取消未发送部分，已发送部分不可回滚。

## 4. 工具清单与契约

### 4.1 会话：9 个

| 工具 | 参数 | `data` 和关键行为 |
| --- | --- | --- |
| `rd_session_list` | `scope?: "all" / "mine" / "available" = "all"`, `cursor?: string`, `limit?: integer = 50`（1～100） | `{sessions: SessionSummary[], next_cursor}`；发现用的最小摘要含 ID、类型、GUI 是否存在、连接状态、占用状态；不给其他 AI 的绑定凭据、画面或终端输出 |
| `rd_session_open` | O, `peer_id: string`, `kind?: "desktop" / "terminal" = "desktop"`, `force_relay?: boolean = false` | `{created, session: SessionState, initial_terminal_id: string / null}`；通过 GUI 打开或复用可见会话并绑定，返回后用 get 等待认证/连接；不在该工具中传密码 |
| `rd_session_attach` | S, O | `{session: SessionState}`；绑定已存在会话，保留控制模式；被其他 AI 占用则 `SESSION_BUSY`；不隐式重连或抢焦点 |
| `rd_session_detach` | B, O | `{session_id, detached: true}`；可在人类控制时执行，取消本绑定操作、释放输入、归还人类，保留 GUI；对原绑定重复调用无副作用，不影响后来建立的新绑定 |
| `rd_session_get` | B, `after_revision?: string`, `wait_ms?: integer = 0`（0～30000）, `operation_id?: string` | `{session: SessionState, operation: OperationState / null}`；查询自己绑定的会话及可选操作记录；revision 未变化时有界等待，超时返回 `unchanged`；也可查询原调用者的短期关闭记录 |
| `rd_session_authenticate` | W, `challenge_id: string`, `credentials: AuthCredentials` | `{session: SessionState, delivery}`；只处理当前挑战，不把 FFI 调用返回当作登录成功，后续 get 读取认证结果 |
| `rd_session_disconnect` | W | `{session: SessionState}`；停止远端连接并保留可见 GUI 容器，取消写入及批准、释放输入并转人类模式；不等同于解绑或关窗口 |
| `rd_session_reconnect` | W, `force_relay?: boolean = false` | `{session: SessionState}`；显式重连，连接代次更新；重连过程中撤销旧控制 token，恢复连接后须再次显式接管 |
| `rd_session_close` | W | `{session_id, closed: boolean, remaining_views: integer}`；关闭此逻辑会话对应全部 GUI 视图及核心连接；返回 pending 时用 get 查最终关闭结果，不将解绑当作关闭 |

`SessionSummary` 仅含 `session_id`, `peer_id`, `kind`, `gui_registered`, `connection_state`, `owner`。分页 cursor 绑定列表修订号，列表变化导致 cursor 失效时返回 `CURSOR_EXPIRED`，重新列举。

`rd_session_open` 的复用规则：

1. 按规范化远端 ID 和连接类型查找同一核心会话，已有则复用。自己的绑定保持原模式，未绑定会话新绑定后保持人类模式，其他 AI 已绑定则返回占用错误。
2. 没有核心会话才创建，**只有真正新建**的会话默认由 AI 控制，避免用 open 绕过已有会话的批准设置。
3. 保留创建中的预约，GUI 创建与 MCP 绑定使用同一关联 ID；并发 open 不能生成竞争的重复连接。
4. GUI 未能完成注册时返回 `GUI_UNAVAILABLE`，不留下可被 AI 隐形操控的连接。窗口可以最小化，已注册且可供人类打开观看即可，不要求永远前台置顶。
5. 若外部路径导致同一设备/类型出现多个不可合并的核心连接，返回 `AMBIGUOUS_SESSION`，要求用 list + attach 指定，不随机选择。

`AuthCredentials` 是带 `kind` 的互斥结构：

- `{"kind":"password","password":"…"}`。
- `{"kind":"two_factor","code":"…"}`。
- `{"kind":"os_login","username":"…","password":"…"}`。

挑战只接受对应类型；首版 MCP 提交不保存密码、不勾选信任设备。操作返回错误不得回显凭据。认证失败保留 GUI 处理入口。

### 4.2 控制权：3 个

| 工具 | 参数 | `data` 和关键行为 |
| --- | --- | --- |
| `rd_control_request` | B, O, `reason?: string`（最多 200 字符） | `{control, approval}`；默认返回 pending 的批准请求；无需批准则直接授予并返回 `control_token`；已由本 AI 控制时返回原 token |
| `rd_control_cancel` | B, O, `approval_id: string` | `{approval}`；取消尚未完成的批准请求；过期、拒绝或已批准则返回已有结果，不回退已经生效的控制权 |
| `rd_control_release` | B, O | `{control, released_inputs}`；无须人类批准，取消待批准请求、队列和 AI 按住的输入，切到人类模式，保留 AI 绑定 |

`Approval`：`{id, state, created_at, expires_at, remaining_ms, reason}`，状态为 `pending`, `approved`, `rejected`, `cancelled`, `expired`。通过 `rd_session_get` 获取变化。

- 每个绑定最多一个 pending 请求，60 秒超时；即使提供新的 operation ID，pending 期间也复用原批准请求且不延长。
- 请求最终记录保留 5 分钟；旧 ID 结果不再存在时明确返回 `APPROVAL_EXPIRED`，不误当新请求。
- AI 不能通过参数声称人类已批准。只有本地 GUI 可信事件能批准。
- **提议澄清原需求**：授予 AI 控制权允许发生在等待认证或已断线状态；控制权只允许调用后续认证/重连，普通输入仍须等远端 ready 和权限通过。否则“人类打开的未认证会话”会出现必须先认证才能接管、必须先接管才能认证的循环。第 11 节列出此待定稿差异。

### 4.3 画面：1 个

| 工具 | 参数 | `data` 和关键行为 |
| --- | --- | --- |
| `rd_screen_capture` | B, `display_id: string`, `after_frame_seq?: string`, `wait_ms?: integer = 0`（0～30000）, `max_width?: integer = 1600`, `max_height?: integer = 1600`（各 1～3840） | `{frame: FrameInfo / null, image_content_index: integer / null}`，有图时另附 PNG image block；可在人类控制时读取 |

`FrameInfo` 包含：`snapshot_id`, `display_id`, `connection_epoch`, `layout_revision`, `frame_seq`, `received_at`, `age_ms`, `is_stale`, `is_new`, `image_width`, `image_height`, `remote_rect: {x,y,width,height}`, `cursor_composited: false`, `mapping_expires_at`。

- 只导出远端解码像素。默认 PNG，等比缩小到指定边界，不放大、不裁剪、不拼接显示器。
- `after_frame_seq` 有值时等待序号更大的解码帧，超时返回 `unchanged` 且不冒充新图；没有任何可用帧时返回 `NO_FRAME`。断线可以读取缓存，但明确标记断线和陈旧，不能拿它执行新连接的输入。
- 未指定 after 时返回最新可用图。`is_new` 在没有比较序号时为 `null`；`is_stale` 表示接收后超过 2 秒或已断线，不声称知道远端实际拍摄时间。
- 桌面静止可能没有新帧。读取可请求官方视频刷新，但不能改变人机控制权；仍没有新帧则如实返回。
- 不额外合成 RustDesk 独立光标通道，`cursor_composited=false`。远端原始视频本身已经包含的光标像素不做图像识别或擦除。
- PNG 上限 8 MiB，超限返回 `IMAGE_TOO_LARGE` 并建议降低尺寸；不返回被截断的图片。
- `snapshot_id` 的坐标映射保留 30 秒，每绑定最多 64 个，重连或布局变化立即失效；这是映射有效期，不代表画面内容 30 秒内没有变化。

### 4.4 输入：1 个

| 工具 | 参数 | `data` 和关键行为 |
| --- | --- | --- |
| `rd_input_send` | W, `actions: InputAction[]`（1～32） | `{delivery, completed_actions, sent_events, failed_action_index, held_keys, held_buttons}`；按数组顺序执行；一次失败或控制权变更停止剩余项，返回部分发送情况 |

同一会话只允许一个输入批次在执行，最多再排队一个批次；更多请求返回 `INPUT_BUSY`。不同会话可并行。一个批次总时长上限 5 秒，不提供任意长时间 sleep。

`InputAction` 以 `type` 区分，非所属字段一律不接受：

| type | 字段及含义 |
| --- | --- |
| `move` | `snapshot_id`, `x`, `y`：截图像素坐标，整数 |
| `button_down` / `button_up` | `button: "left" / "middle" / "right"`；可选整体 `position: {snapshot_id,x,y}`，先移动再按下/释放；省略位置保持当前远端指针位置 |
| `click` | `snapshot_id`, `x`, `y`, `button?="left"`, `count?: 1 / 2 = 1`；展开为移动及按下/释放，双击间隔固定 100 ms |
| `drag` | `button?="left"`, `points: {snapshot_id,x,y}[]`（2～64）, `duration_ms?: integer = 500`（1～3000）；按路径拖动并释放，允许不同点来自不同显示器 |
| `scroll` | `horizontal?: integer = 0`, `vertical?: integer = 0`（各 -100～100，不能同时为 0），可选 `position`；单位为逻辑滚轮刻度，正数向右/向下，由桥接层适配远端协议 |
| `key_down` / `key_up` | `key: KeyName` |
| `key_press` | `key: KeyName`，展开按下和释放 |
| `shortcut` | `modifiers: Modifier[]`, `key: KeyName`；按修饰键、主键，再反序释放本动作新按下的键 |
| `text` | `text: string`，UTF-8 最多 16 KiB；按原文输入，不解析命令、不自动追加回车、不借用本机剪贴板 |
| `release_all` | 无其他字段；释放当前 AI 跟踪的全部键和按钮；人类接管后的清理由内部路径执行，不要求再获得 AI token |

`KeyName` 首版支持：`KeyA`～`KeyZ`、`Digit0`～`Digit9`、`F1`～`F12`、`Enter`, `Tab`, `Escape`, `Backspace`, `Delete`, `Insert`, `Space`, `ArrowUp`, `ArrowDown`, `ArrowLeft`, `ArrowRight`, `Home`, `End`, `PageUp`, `PageDown`，以及 `ControlLeft/Right`, `ShiftLeft/Right`, `AltLeft/Right`, `MetaLeft/Right`。

`Modifier` 为 `Control`, `Shift`, `Alt`, `Meta`，默认映射左侧修饰键；不根据主控 macOS 自动把 Control 改成 Meta。文字与标点优先使用 text，未支持的键明确报错。

- 完整批次先做静态校验；执行每个实际输入事件前再次检查 AI 身份、绑定、control token、远端连接代次、权限和截图布局映射。
- AI 下发的事件携带控制代次直到真正序列化发送处。不能只在 MCP 收到请求或入队时检查，否则官方发送队列里的旧输入仍可能在接管后发出。
- 接管清理是高优先级本地操作，不排在普通输入队列后面。已经交给网络的字节不能撤回，明确区分这一边界。
- 跟踪按键使用集合；重复 down 不增加释放计数。快捷键不能释放更早显式按住的修饰键。
- 批次失败或取消时尽力释放本 AI 全部按住状态，并在结果中反映发送/清理失败；正常完成的显式 down 可跨调用保持，直到显式 up、release 或生命周期清理。

坐标换算采用截图对应显示器的远端原点和尺寸，例如横坐标：

```text
remote_x = display_origin_x + min(remote_width - 1,
    floor((image_x + 0.5) * remote_width / image_width))
```

纵坐标同理。先检查图内范围，不将越界输入悄悄钳制到边缘。远端原点可以为负；坐标尺寸取官方远端坐标空间，不假定 GUI 缩放、Retina 像素与远端坐标一一相等。

### 4.5 终端：6 个

| 工具 | 参数 | `data` 和关键行为 |
| --- | --- | --- |
| `rd_terminal_list` | B | `{terminals: TerminalState[]}`；只列出该终端连接的实例，不自动创建 |
| `rd_terminal_create` | W, `rows?: integer = 24`, `cols?: integer = 80` | `{terminal: TerminalState}`；先创建可见 GUI 标签再请求远端打开，状态可为 opening，后续 list/read 获得 opened/error |
| `rd_terminal_read` | B, `terminal_id: string`, `cursor?: string`, `format?: "text" / "base64" / "both" = "text"`, `max_bytes?: integer = 16384`（1～65536）, `wait_ms?: integer = 0`（0～30000） | `{terminal, stream_epoch, start_offset, end_offset, next_cursor, oldest_cursor, has_more, text, text_lossy, data_base64}`；未请求的表示字段为 null；从 cursor 增量读取，不消耗 GUI 缓冲；首个 cursor 省略时从当前保留的最早字节开始 |
| `rd_terminal_write` | W, `terminal_id: string`, `text: string`（UTF-8 最多 16 KiB） | `{terminal_id, bytes_sent, delivery}`；原样发送，支持转义字符，不自动换行，不等待命令结束 |
| `rd_terminal_resize` | W, `terminal_id: string`, `rows: integer`, `cols: integer` | `{terminal_id, requested_size, delivery}`；rows 为 1～500，cols 为 1～1000，区分请求尺寸与远端已确认尺寸 |
| `rd_terminal_close` | W, `terminal_id: string` | `{terminal: TerminalState}`；关闭指定实例，其他实例保留，等待远端 closed 事件确认；关闭最后一个标签可能触发官方 GUI 关闭连接 |

create 的 rows/cols 使用与 resize 相同的范围。`TerminalState`：`{terminal_id, state, requested_size, pid, shell_exit_code, error, stream_epoch}`，状态为 `opening`, `open`, `closing`, `closed`, `failed`。未确认信息为 null。

- 每个终端保留 4 MiB 原始输出环形缓存，全局总额 64 MiB；压力下只淘汰最早原始字节，不截断或改写后续字节。GUI 使用独立缓冲。
- cursor 绑定终端实例、连接/输出代次和字节偏移；已被覆盖则 `OUTPUT_GAP`，返回可恢复的 oldest cursor，AI 显式从那里继续。绝不悄悄跳过丢失内容。
- 在旧 cursor 上重复读得到相同仍保留的数据；没有新输出时可长轮询，超时为 `unchanged`。终端关闭事件即使没有输出也应唤醒读取。
- 缓存始终保留原始字节。默认 text 用 UTF-8 解码并保留 ANSI 与回车，便于 AI 阅读；遇到非法字节或分片切在字符中间时以替代字符呈现，并标记 `text_lossy=true`。需要精确字节时用原 cursor 和 base64/both 重读，不把解码文本冒充原始字节。
- base64 表示包含完整 ANSI、回车和任意非 UTF-8 字节。首版不剥离终端控制序列、不做屏幕重建，也不提供“执行一条命令并返回退出码”工具。cursor 始终按原始字节计数，切换 format 不改变读取位置。
- `shell_exit_code` 仅来自远端终端关闭事件；连接断开不猜测退出码。协议中的重连回放可能包含旧内容，新 `stream_epoch` 明确标记 replay，不声称输出天然不重复。
- 已关闭实例的最后输出和关闭结果可供原绑定调用者读取 60 秒，随后 `TERMINAL_EXPIRED`。这是只读完成记录，不保留活跃会话独占；其他 AI 重新绑定不能读取旧绑定的完成记录。
- 核心 GUI 会话关闭后，`rd_session_get` 同样保留 60 秒的只读关闭记录；通过原 AI 身份和原 binding ID 访问，超时后返回 `SESSION_CLOSED`。读取记录不能重新赋予控制权。
- AI 解绑或 MCP 会话结束立即撤销它对完成记录的访问；终端连接仍在 GUI 中运行时，不因 AI 离开自动关闭 shell。
- 终端 GUI 的键盘/粘贴、AI 输入和尺寸改变共同遵守控制模式。AI 控制时，窗口尺寸变化只更新本地布局，不能悄悄覆盖 AI 请求的 PTY 尺寸；切到人类控制后按当前 GUI 尺寸同步。

## 5. 错误分类与 AI 下一步

| 错误码 | 意义 | AI 应采取的动作 |
| --- | --- | --- |
| `INVALID_ARGUMENT` | 参数内容或范围不合法 | 修正参数；未发送输入 |
| `SESSION_NOT_FOUND`, `SESSION_CLOSED` | 会话不存在或已关闭 | 重新 list；不复用旧目标 |
| `SESSION_BUSY`, `AMBIGUOUS_SESSION` | 被别的 AI 绑定，或目标不唯一 | 选择明确且可绑定的会话，不创建隐藏重复连接绕过占用 |
| `BINDING_REQUIRED`, `BINDING_EXPIRED`, `NOT_SESSION_OWNER` | 无绑定、旧绑定、绑定不属于调用者 | 显式 attach 或停止；不提供他人的内容 |
| `HUMAN_CONTROL`, `CONTROL_EXPIRED` | 人类控制或旧控制 token | 停止写入；需要时显式 request control |
| `AUTH_REQUIRED`, `AUTH_FAILED`, `AUTH_CHALLENGE_CHANGED` | 未认证、认证失败或挑战已变化 | 读取当前状态，在获得控制权后提交当前挑战，或让人处理 |
| `HUMAN_ACTION_REQUIRED` | 需要本地或远端人工确认 | 告知具体原因并等待，不把其他窗口弹窗当作成功 |
| `NOT_READY`, `DISCONNECTED` | 连接暂不可用 | get 等待；重连必须显式执行 |
| `PERMISSION_DENIED`, `UNSUPPORTED`, `WRONG_SESSION_KIND` | 未授权、功能不支持或连接类型错误 | 遵从实际能力；不循环尝试相同写入 |
| `NO_FRAME`, `DISPLAY_CHANGED`, `SNAPSHOT_EXPIRED`, `IMAGE_TOO_LARGE` | 无画面、布局变化、坐标映射过期或图片过大 | 重新查显示器并截图，或减小返回尺寸 |
| `INPUT_BUSY`, `LIMIT_EXCEEDED` | 输入队列或资源达到上限 | 按返回的等待提示重试；不扩大队列 |
| `OPERATION_CONFLICT` | 重试 ID 对应另一组参数 | 新动作使用新 ID；原动作的未知结果先查询 |
| `APPROVAL_EXPIRED`, `CURSOR_EXPIRED` | 批准记录或列表游标失效 | 重新查询；重新申请批准须显式新动作 |
| `TERMINAL_NOT_FOUND`, `TERMINAL_NOT_OPEN`, `TERMINAL_EXPIRED` | 终端不可用或完成记录已过期 | list 查看真实状态，不重新写入旧终端 |
| `OUTPUT_GAP`, `OUTPUT_EPOCH_CHANGED` | 缓存覆盖或重连输出代次变化 | 报告丢失/回放，从服务返回的新游标显式继续 |
| `GUI_UNAVAILABLE`, `SERVICE_STOPPING`, `INTERNAL_ERROR` | GUI 或服务不可用 | 保留诊断编号，不回显内部秘密；不默认执行成功 |

所有失败均给出 `delivery` 或明确“未发送”，让 AI 区分安全重试和可能重复执行。读取型错误本身不触发接管。

`OperationState` 为 `{operation_id, tool, status, delivery, completed_actions, sent_events, error, dedupe_expires_at}`；不适用的字段为 null。可通过原调用去重查询，或对已知会话使用 `rd_session_get(operation_id=...)` 查询；无记录时明确表示未知，不能返回成功。

## 6. 面向 AI 的典型流程

### 6.1 打开桌面并操作

```text
rd_session_list
  → 找到已有目标：rd_session_attach
  → 没有目标：rd_session_open(kind=desktop)
rd_session_get
  → awaiting_auth：必要时先 rd_control_request，再 rd_session_authenticate
  → awaiting_human：告诉人类需要什么，使用 get 有界等待
  → ready：读取显示器列表
若当前是 human：rd_control_request → rd_session_get 等待批准
rd_screen_capture(display_id)
rd_input_send(control_token, snapshot_id, actions)
rd_screen_capture(after_frame_seq) 查看变化
结束操作：rd_control_release
结束参与：rd_session_detach
```

释放控制权、解绑和关闭 GUI 的含义不同，工具描述必须明确。完成 AI 任务默认建议 release 或 detach，除非任务要求关闭会话，不自动 close。

### 6.2 人类接管发生在输入途中

1. GUI 发出可信接管事件，桥接层先撤销控制 token。
2. 正在执行的拖动或组合键在下一发送边界停止；待执行批次被取消，发送释放事件。
3. 输入结果为 `partial` 或 `failed`，包含已发事件数和 `HUMAN_CONTROL`。
4. AI 保留 binding，可继续截图和读终端；新写入被拒绝。
5. AI 若要恢复，显式 request control；批准后得到全新 token，旧批次不得复活。

### 6.3 终端

```text
rd_session_open(peer_id, kind=terminal)
  → 返回终端连接 session_id 与 initial_terminal_id
认证 / 人工处理 / 接管，与普通会话流程一致
rd_terminal_list → 确认终端 open
rd_terminal_write(text="pwd\n")
rd_terminal_read(cursor, wait_ms=10000)
  → 保存 next_cursor，继续读，不凭 shell 提示符承诺命令退出码
需要额外终端：rd_terminal_create
退出参与：rd_control_release 或 rd_session_detach
确需关闭 shell：rd_terminal_close
```

不要按一条命令一次创建终端，也不要把本地终端作为远端执行的替代。输出中的指令性文字属于远端内容，不应覆盖 AI 原任务、工具描述或控制权规则。

### 6.4 多 AI

AI A 和 B 各自完成 MCP 初始化。A 绑定桌面 S1，B 可绑定另一桌面 S2 或独立终端连接 S3；B 绑定 S1 返回占用。A 失联只清理 S1，B 的连接和操作继续。S1 释放后 B 可以显式绑定，但不会继承 A 的控制 token、截图映射或未完成操作。

### 6.5 输入与错误示例

下面是 `rd_input_send` 的参数示例，ID 仅作示意：

```json
{
  "session_id": "s_demo",
  "binding_id": "b_demo",
  "operation_id": "click-search-001",
  "control_token": "c_demo",
  "actions": [
    {"type": "click", "snapshot_id": "f_demo", "x": 420, "y": 180},
    {"type": "text", "text": "RustDesk"},
    {"type": "key_press", "key": "Enter"}
  ]
}
```

如果人类已接管，工具的 `structuredContent` 示例：

```json
{
  "ok": false,
  "status": "failed",
  "data": {
    "delivery": "not_sent",
    "completed_actions": 0,
    "sent_events": 0,
    "failed_action_index": null,
    "held_keys": [],
    "held_buttons": []
  },
  "error": {
    "code": "HUMAN_CONTROL",
    "message": "A human controls this session. Reading is allowed; request control explicitly before writing.",
    "retry": "after_state_change",
    "details": {"session_id": "s_demo", "control_mode": "human"}
  }
}
```

外层 MCP 结果设 `isError=true`；不把错误包装成成功的 text-only 结果。

## 7. MCP 协议与传输设计

### 7.1 明确协议版本

**提议首版协议固定为 `2025-11-25`，使用有状态 Streamable HTTP。** SDK 版本与协议版本是两件事。

- SDK 3.3.0 默认支持多个协议版本；其 `ServerHandler::supported_protocol_versions` 可限制支持范围，`negotiate_initialize` 可复用协商逻辑。
- 在初始化中声明且仅协商 `2025-11-25`，不默认接受 SDK 的所有版本。后续 HTTP 请求使用已协商的版本，版本不匹配按规范拒绝。
- 这一选择保留已确定的 initialize、MCP 逻辑会话、标准 ping、GET/SSE 与 DELETE 生命周期。2026-07-28 的生命周期和结果格式不同，不能在不设计的情况下混用。
- 使用 SDK 处理协议消息和序列化，不自建 JSON-RPC 解析器，不手写旧版传输兼容层。

### 7.2 HTTP 与身份

```text
127.0.0.1:21122/mcp
  → HTTP 大小限制 / Origin 检查 / Bearer 认证
  → rmcp Streamable HTTP 会话管理
  → 每逻辑 MCP 会话一个 ClientContext
  → 参数解析与工具路由
  → 独立 Rust 桥接层
  → 官方 RustDesk 连接、解码和输入路径
```

- `POST /mcp`：初始化、请求、通知及客户端对服务端 ping 的应答。
- `GET /mcp`：已初始化客户端的服务端事件通道；维持活性探测，不用 TCP 连接数统计 AI。
- `DELETE /mcp`：结束这一逻辑 AI 会话，仅清理它的绑定。
- 认证在 POST、GET、DELETE 之前执行。生成的 MCP 会话 ID 绑定 `ClientContext`；相同令牌可以初始化多个独立 AI，不相互复用身份。
- 除初始化外，每次调用必须属于已建立且未失效的 MCP 逻辑会话；首版不开放无会话的直接工具调用或新的 discover 生命周期入口。
- 缺失 Origin 的原生已认证客户端可连接；首版只允许精确的本地 MCP 服务 Origin（若有 Origin），拒绝其他来源，不开放通配 CORS。浏览器直连不作为首版连接保证。
- HTTP 请求体上限 256 KiB；每 AI 最多 16 个正在执行的工具调用，读取等待也计入。内部 ping/控制撤销不受此并发限制阻塞。
- AI 主动让出、解绑和取消批准使用独立的短操作通道，不因自己的长轮询占满普通调用名额而无法执行清理。
- 未完成初始化的临时会话最多等待 10 秒；未完成握手不进入 GUI 的已连接 AI 列表、不允许绑定。
- ClientContext 含内部 AI ID、客户端上报名称/版本、创建时间、最近通信、取消标志与绑定集合。元数据做长度限制并作为纯文本显示，不作为授权依据。

### 7.3 SDK 与工具注册

- 用 `rmcp::ServerHandler` 及 SDK 工具路由实现 20 个具名工具。一个服务实例共享 Bridge，一个 handler 上下文绑定一个 AI 身份。
- 参数结构使用 `serde` 的未知字段拒绝和 `schemars` 生成输入 schema；动作/凭据使用显式标签枚举。输出按工具的具体数据类型生成 schema，成功和错误都必须满足自己的输出约束。
- 只声明实际实现的 tools 能力；首版不声明 resources、prompts、sampling、elicitation、tasks 等能力。批准在本地 GUI 完成，状态通过工具查询，不要求 AI 客户端支持弹窗询问。
- tools 列表在运行期间固定；远端能力变化体现为会话能力和工具错误，不频繁增删工具。
- list/get/capture/terminal read 类工具标记只读；写入工具保守标记有副作用。`idempotentHint` 不因为短期 operation 去重就宣称永久幂等。工具注解只是提示，不能代替权限检查。
- server instructions 简述：先发现/绑定、区分 ID、遵守控制权、截图坐标来源、发送与执行的区别，以及断线后不能盲重试。

### 7.4 状态和取消传播

- 桥接层用可订阅的最新状态快照服务 GUI 和工具，不以 MCP 推送成功作为状态成立条件。
- `rd_session_get(after_revision)`、`rd_screen_capture(after_frame_seq)`、`rd_terminal_read(cursor)` 分别等待状态、帧和输出；没有变化返回 unchanged，不使用固定 sleep 猜结果。
- 订阅后再比对修订号，避免“读完状态、开始等待之间”丢失事件。队列丢事件时回到最新状态快照，不能靠无界日志补齐。
- 同一调用的 MCP cancellation、AI 失联、控制权撤销、服务关闭分别关联不同取消范围。取消一次读取不解绑 AI，取消批准查询也不取消独立存在的批准请求。
- request control 已返回 pending 后，批准请求是独立状态；显式 control cancel、让出或生命周期清理才能取消它。
- AI 失联或显式结束只清理自己的上下文；MCP 停止和全局令牌重置撤销全部上下文。清理校验绑定代次，不影响后来的绑定。

## 8. Rust 桥接与 Flutter 集成

### 8.1 模块边界

提议新增模块，具体文件拆分可在实现时按大小调整：

```text
src/mcp/
  mod.rs          服务启停与共享状态
  transport.rs    HTTP 认证、rmcp 会话生命周期
  tools.rs        工具路由及参数/结果转换
  types.rs        MCP 对外 schema

src/automation/
  mod.rs          独立于 MCP 的桥接入口
  sessions.rs     核心会话、GUI 视图、AI 绑定及状态
  control.rs      控制权、批准和取消代次
  input.rs        有序输入及按住状态
  frames.rs       独立像素缓存与坐标映射
  terminals.rs    终端状态、原始输出和游标
  operations.rs   有界去重记录
```

MCP 层不直接锁 Flutter 会话内部字段、不编码 RustDesk 键鼠消息。桥接层接收通用调用者身份和结构化操作，GUI 接管入口也直接调用桥接层。

首版只在 Flutter + macOS 主控构建启用内嵌服务；后台被控服务、连接管理器进程及每个子窗口不分别监听端口。构建开关和服务关闭时，原 GUI 路径保持可用。

### 8.2 可见 GUI 会话的创建

macOS 原生入口注册多窗口 Flutter controller；锁文件固定的多窗口插件使用进程内原生窗口管理。首版按同一进程的 Rust 核心注册表设计，Dart 多窗口使用各自 isolate，不能共用 Dart 对象。

1. Rust 桥接层建立带超时的 GUI 创建请求，发送到主 Flutter 事件通道。
2. 主窗口通过现有 `RustDeskMultiWindowManager` 创建/激活正常桌面或终端界面。
3. 目标界面在官方 FFI 注册核心会话/视图时，带回请求关联 ID，桥接层完成关联及绑定。
4. GUI 注册未确认前，不允许 AI 发送远端输入；请求超时或失联时，迟到的 GUI 回调只可留下正常人类会话，不授予失效 AI 控制权。
5. 通过本地 FFI 回调使设置页和对应远控视图都订阅同一个控制状态。UI 发出的旧输入也需在 AI 接管边界失效，不能只在界面层取消焦点。

所有为 AI 创建对象而延后的 GUI 回调，包括终端认证完成后的自动 open，都携带原请求及控制代次；人类接管或 AI 离开后不能继续以原 AI 授权启动 shell。正常人工创建路径不受这些 AI 请求的取消影响。

实机验证须确认各窗口加载同一 Rust 库实例及会话注册表。这里不为未支持平台预建 IPC 框架。

### 8.3 输入队列的最终检查

官方输入存在 `Data` 消息及 IO loop 发送队列。只在 MCP 侧设队列无法保证接管后的旧输入停止。

- 为新功能提供带 `{caller, binding_id, control_generation, connection_epoch}` 的输入信封，持续携带到真正远端发送前。
- 优先在客户端 `Data` 增加仅新功能使用的变体及 IO loop 薄分支；普通人工会话的已有 `Data::Message` 路径不经新抽象重写。
- 同一核心会话的最终发送器串行处理 AI 事件与控制切换；每个未发送事件可丢弃。接管完成的线性化边界是旧发送授权已失效并安排释放，此后不再开始旧授权的发送。
- GUI 输入仅在有 AI 绑定且控制模式为 AI 时拦截；人类控制或 MCP 未启用时保留现有输入实现。不能为了此功能更改所有 `Interface` 调用签名。
- 给已有人工会话首次安装绑定时，在发送器中设置交接边界；此前未带新功能标识的 GUI 输入须在边界前完成或被清除，之后的新 GUI 输入按控制代次处理。不能先宣布 AI 已接管再让旧人工输入穿过队列。
- 等待网络或队列时不持有全局锁；截图编码用 `spawn_blocking`，不得阻塞控制撤销和心跳。

### 8.4 画面缓存与多屏

- 在 `FlutterHandler::on_rgba` 分发到 GUI/纹理渲染之前复制到桥接快照；不调用 `session_next_rgba` 消耗 GUI 缓冲，不使用交换走 GUI 像素的方式。
- 每个显示器保留最新完整 CPU 帧，使用共享只读引用交给截图编码；网络/解码线程不等待图片编码。
- 每帧缓存上限 128 MiB、总像素缓存预算 512 MiB；压力下淘汰最久未读取显示器的旧缓存，返回无帧或请求刷新，不能干扰 GUI 缓冲。编码同时最多两个任务。
- AI 读取某显示器时需要的视频订阅与 GUI 当前需要的订阅取并集；GUI 切屏/最小化不取消 AI 正在使用的画面来源，解绑后释放 AI 自己的订阅。
- 首版 macOS 的 AI 会话必须选择 CPU 可读解码输出，GUI 仍可把这些像素交给纹理渲染。软件解码作为明确回退，不能仅挂在软件渲染分支而漏掉纹理渲染。
- 对已有 GPU-only 会话，桥接层在绑定时准备同会话 CPU 输出或切换到软件解码；切换期间显式表示 awaiting_frame。无法提供任何像素路径必须报告能力失败，不能把截图永久不可用的绑定视为验收通过。
- 显示器添加、移除、原点/尺寸/旋转改变，更新布局修订号并使旧映射失效。连接代次和显示器标识防止旧编号复用。

### 8.5 终端事件

- 在官方 `handle_terminal_response` 收到 opened/data/closed/error 时，以相同事件更新桥接状态；保留原 GUI 事件流。
- data 需要解压后保留完整字节，再分别送 GUI 和桥接环形缓存；不从 Flutter xterm 渲染文本反向还原原始输出。
- 若借用现有 FFI 的 void 返回值，只能表示请求入口被调用；桥接必须补齐目标检查、发送失败和远端事件关联。
- 终端打开必须只由一个已关联路径发出，避免 GUI 的自动 open 与 MCP 工具各发一次。创建请求、官方 terminal 数字 ID 和桥接 terminal ID 一一对应。

## 9. 服务、配置与资源生命周期

- `disabled → starting → running / failed → stopping → disabled`。starting 只有成功绑定监听地址且路由和取消机制就绪后才进入 running；GUI 可以把 stopping 显示为关闭中。
- 启用状态持久化；应用启动完成 GUI 事件注册后再自动启服务。关闭设置页或最小化主窗口不停止服务，退出整个 GUI 主控进程才停止。
- 端口冲突为 failed，不自动换端口。改变端口或重置令牌通过受控停止/重启处理，保持配置和实际运行状态一致。
- stop 先拒绝新写入并使所有控制代次失效，再取消等待/排队工作、释放输入、移除绑定、结束 MCP 会话和监听。释放失败要可见，不阻止最终停服；不能关掉 RustDesk GUI 远控。
- 所有资源使用上限：输入队列、操作记录、像素缓存、终端字节、状态等待者；超限明确返回错误，不依赖进程耗尽后报错。
- 所有活性和批准计时都来自独立定时器，主机恢复时先处理过期项。中途出现 SDK 传输断开但逻辑会话尚有效时，以活性探测规则决定收尾。
- 32 字节随机 Bearer 令牌保存在 Keychain。运行日志只记录工具名、显示用 AI ID、会话 ID、耗时、结果码和关联编号；不记录参数/结果全文、密码、token、截图或终端正文。

## 10. 验证计划与回归范围

### 10.1 契约及并发验证

- 每个工具的合法/非法参数与输出结构按 schema 验证；未知字段、越界坐标、错误会话类型不能发送事件。
- 两个 AI 并发绑定同一个核心会话，仅一方成功；多个 GUI 视图不能绕过独占。
- 重放 operation ID 不重复点击、输入、创建终端；不同参数复用 ID 被拒绝；缓存过期/服务重启后不宣称能去重。
- 在批次开始前、拖动中间、最后发送前、批准与取消竞争时注入接管，断言撤销边界之后没有旧代次输入开始发送，并准确报告已发送范围。
- 解绑后立即重新绑定，旧清理与旧批准到达时不得修改新归属。
- 测试只读状态下读取继续、写入拒绝、AI 主动让出、默认批准和无需批准两种设置。
- 新的 HTTP 连接和 GET 重建不算新 AI；一个 AI 失联不影响其他 AI。心跳不能被长轮询、满输入队列或图片编码阻塞。

### 10.2 画面和终端实机验证

- 本机 macOS / ARM64 连接官方被控端，确认 CPU/纹理两种 GUI 渲染可截图，多屏负坐标、缩放、旋转、切屏、最小化、重连都映射正确。
- 对比 GUI 图像消费前后的桥接像素，检查没有抢占 GUI 缓冲；静止画面、未首帧、旧缓存、新帧各自语义正确。
- 终端保持 ANSI、非 UTF-8 和分片 UTF-8 原始字节；重复读、缓存覆盖、回放、关闭无输出及 shell 退出码均可识别。
- 终端重连不重复创建 shell；关闭单一标签、最后一个标签和 AI 解绑的行为分别验证。
- 非法 token、重置前 token、外部 Origin、其他 AI 的 binding 以及旧 control token 均不能写入。

### 10.3 预计必须触及的既有路径

| 既有文件/路径 | 必要变更原因与边界 |
| --- | --- |
| `src/lib.rs` / 客户端启动路径 | 注册新模块，在 GUI 主控生命周期启停服务；不在每个窗口重复监听 |
| `src/flutter_ffi.rs` | 新增设置、状态订阅、GUI 创建确认和接管的薄接口；不直接把全部 FFI 导出成 MCP |
| `src/flutter.rs` | 会话/视图关联、解码帧复制和终端事件旁路；保留原 GUI 渲染/事件消费 |
| `src/client.rs`、`src/client/io_loop.rs` | 新 AI 输入信封和最终发送授权检查，连接/权限事件薄钩子；原人工消息路径尽量不变 |
| `src/ui_session_interface.rs` | 仅在桥接确需复用发送/认证能力时补薄入口；不改共享 trait 签名迫使无关调用者传占位参数 |
| Flutter 设置页及新 MCP 小组件 | 展示服务和 AI 连接列表；调用相同桥接状态 |
| Flutter 远控/输入模型及工具栏 | AI 控制时只读和人类接管入口，清理人类已按住状态；没有 AI 绑定时保持现有交互 |
| Flutter 多窗口管理及终端页面/模型 | 创建可见对象和关联确认、终端输入及尺寸门控；不替换正常人工创建流程 |

不修改被控端协议、不为此目的更新 hbb_common 子模块。上述是未来实现需要验证的最小范围，不能作为提前扩大修改范围的许可。本轮文档设计不改变任何运行路径。

## 11. 定稿前的明确选择

已确认：**按能力拆分工具**。

本稿提出以下具体选择，需需求方审阅：

1. 20 个工具及 `rd_` 命名，输入采用一个带类型动作数组的工具；鉴权凭据使用独立 authenticate，避免在每个操作携带密码。
2. `session_id` 表示核心连接，多个 GUI 视图共同独占；终端独立连接，其全部终端标签共享控制权。
3. 写入显式携带 `binding_id`、`control_token` 和重试用 `operation_id`，换取旧调用不复活和响应丢失时的可控重试。
4. 截图 PNG 默认边界 1600×1600，输入使用该截图坐标和 snapshot ID；终端默认返回保留 ANSI 的 UTF-8 文本，可选 base64 原始字节，并显式标记有损解码。
5. `2025-11-25` 协议搭配已选 SDK 3.3.0，限制协商范围，不混入其他生命周期。
6. **原需求第 3.9 节需澄清**：允许在未认证/断线时先取得控制权，以便 AI 提交认证或重连；实际普通输入仍受连接状态与权限检查。本稿没有静默改写原需求这条规则。

定稿后再由 Rust 类型生成机器可校验的完整 schema，并按第 10 节验证。现在的工具表、字段类型、边界和例子用于接口审阅，不是已注册可调用的工具。

## 12. 核查依据

### 固定 RustDesk 源码

- [会话 FFI 与终端接口](https://github.com/rustdesk/rustdesk/blob/6c578292e8ebbbec708b76986ba8c4bc7c509747/src/flutter_ffi.rs)
- [核心会话注册表、RGBA 与终端事件](https://github.com/rustdesk/rustdesk/blob/6c578292e8ebbbec708b76986ba8c4bc7c509747/src/flutter.rs)
- [IO loop 与实际发送路径](https://github.com/rustdesk/rustdesk/blob/6c578292e8ebbbec708b76986ba8c4bc7c509747/src/client/io_loop.rs)
- [终端连接共享](https://github.com/rustdesk/rustdesk/blob/6c578292e8ebbbec708b76986ba8c4bc7c509747/flutter/lib/desktop/pages/terminal_connection_manager.dart)
- [终端 GUI 生命周期](https://github.com/rustdesk/rustdesk/blob/6c578292e8ebbbec708b76986ba8c4bc7c509747/flutter/lib/desktop/pages/terminal_page.dart)
- [macOS 窗口初始化](https://github.com/rustdesk/rustdesk/blob/6c578292e8ebbbec708b76986ba8c4bc7c509747/flutter/macos/Runner/MainFlutterWindow.swift)
- [锁定依赖中的 macOS 多窗口管理](https://github.com/rustdesk-org/rustdesk_desktop_multi_window/blob/b47e8385e5a75d38319ad706a64b0ead3108b093/macos/Classes/MultiWindowManager.swift)

### 官方 MCP 与 SDK

- [2025-11-25 工具与结构化结果](https://modelcontextprotocol.io/specification/2025-11-25/server/tools)
- [2025-11-25 Streamable HTTP](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)
- [2025-11-25 cancellation](https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/cancellation)
- [SDK 3.3.0 ServerHandler 与协商范围](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.3.0/crates/rmcp/src/handler/server.rs)
- [2026-07-28 工具与生命周期说明，用于识别版本差异](https://modelcontextprotocol.io/specification/2026-07-28/server/tools)
