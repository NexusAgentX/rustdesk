# 阻止输入、安全注意序列、提权、系统登录与隐私模式

使用原版协议，仅修改控制端。MCP 0.1.7 新增六个工具；无头账户认证复用既有 `rd_session_authenticate`，不另造认证接口。

## 接口

- `rd_security_get(session_ref)`：读取 SAS 支持、隐私实现列表、权限、便携服务状态、阻止输入/隐私/提权的远端反馈。值为空表示未知，不代表关闭。断开后 `fresh=false`；重连清除上次观测。`sequence` 是本地状态版本，`observed_at` 是远端反馈的接收时间。
- `rd_input_block_set(session_ref, enabled, wait_ms?, operation_id?)`：Windows 现场键鼠输入开关。原版成功时不回执，只通知失败，因此无反馈时保持结果未知；观察窗口结束后允许下一次显式设置。开启要求键鼠和阻止输入权限，关闭允许请求恢复，即使权限已经撤销；原版对端仍可能拒绝关闭。默认等待 10 秒，范围 0..30000 ms。
- `rd_privacy_set(session_ref, enabled, implementation?, wait_ms?, operation_id?)`：开启时明确选择查询到的实现，检查对端功能、键鼠/隐私权限和控制条显示器限制。关闭时可省略实现，使用已观测实现或原版默认。可能改变远端屏幕拓扑。默认等待 10 秒。
- `rd_ctrl_alt_del(session_ref, operation_id?)`：Linux 或报告 SAS 支持的 Windows 使用原版专用命令；要求 AI 控制权及键鼠权限，非浏览模式。不是普通三个按键组合。
- `rd_session_elevate(session_ref, request, wait_ms?, operation_id?)`：Windows 便携版提权。`request` 为 `{mode:"direct"}` 或 `{mode:"logon",username,password}`。安装版、已经报告 SAS/便携服务运行的对端返回 NOT_NEEDED。直接方式可能需要被控端本地确认 UAC；工具不替用户绕过确认。空提权响应只表示流程已启动，收到便携服务运行通知后才确认启动成功。
- `rd_os_password_input(session_ref, password, activate?, operation_id?)`：把临时密码输入当前聚焦的系统登录框，再按 Enter；密码不保存、不返回。默认 activate=false；true 先按 Enter 并等待 1200 ms 打开登录框。调用前须观察登录界面，避免写入其他程序。使用既有串行键盘输入、长度/时长限制和控制权撤销机制；返回 sent，不能当作登录成功。

无头 Linux/系统账户挑战：通过 `rd_session_get` 读取当前 `auth_challenge`，使用 `rd_session_authenticate`，`credentials:{kind:"os_login",username,password}`；若挑战 fields 包含 connection_password，还必须明确提交 RustDesk 连接密码 `connection_password`。账户登录、系统密码输入、权限提升与 RustDesk 连接密码是独立流程，敏感值不写入结果及操作记录。

## 反馈、并发与恢复

阻止输入和隐私模式采用 `BackNotification`，提权采用 `ElevationResponse` 和 `PortableServiceRunning`。不使用 GUI 的乐观勾选状态。

- 写入前、实际发送前都检查当前控制权和适用权限。
- `confirmed=true` 仅指收到符合目标的远端状态反馈；Ctrl+Alt+Del、系统密码输入没有成功回执，始终仅报告发送。
- 原版通知没有请求 ID，因此每类功能仅容许一个正在等待的开启请求。隐私/提权超时保留 pending；阻止输入因原版只通知失败，在观察窗口结束后释放请求锁，但绝不把沉默当作成功。明确关闭可用于恢复，重连会清除旧待结果状态。
- 远端失败用 REMOTE_ACTION_FAILED 及 outcome 区分；权限不足用 PERMISSION_DENIED，不支持用 UNSUPPORTED。提权错误不回显对端可能包含账户内容的自由文本。
- 归还/撤销 AI 控制权时，对 AI 请求开启的阻止输入及隐私模式发送关闭命令；主动断开也先执行清理。发送关闭不证明已恢复，需随后查询 fresh 的远端反馈。网络中断、客户端崩溃或对端拒绝时无法保证清理成功，实际断线清理由原版被控端处理。
- 已观察到关闭成功后，不在控制权切换时重复关闭。旧连接反馈不能修改新连接状态。

## 验证

2026-09-16：64 项 automation、12 项 MCP 自动测试通过；macOS ARM64 release 构建及签名校验通过。被控端为原版 Windows 1.4.9 便携版、双屏。

实机已验证：
- 六个新工具可发现，总数 59；能力查询区分未知、不支持、权限与人工控制。
- 阻止输入连续显式开/关、操作去重、无回执时保持未知且不锁死下一次调用；远端关闭失败返回 REMOTE_ACTION_FAILED/off_failed。成功后的物理键鼠拦截效果没有现场确认。
- 隐私模式 exclude_from_capture 开启、关闭、去重，以及归还控制权后的关闭成功回执；断开后 fresh=false，重连清除旧观测，再次开关取得新回执。
- 人工控制时，阻止输入、隐私、提权、Ctrl+Alt+Del、密码输入均返回 HUMAN_CONTROL。
- 观察 Windows 密码框后输入单次密码并确认回到正常桌面；重复 operation_id 不再次输入。另一次尝试中断线且无后续帧，未算作登录成功。
- 已运行便携服务时，两种提权方式均返回 NOT_NEEDED；SAS=false 时 Ctrl+Alt+Del 返回 UNSUPPORTED。
- 基础会话/控制权/操作去重/截图/画质断连状态及终端 list/create/read/resize/close 回归通过。

按需求方授权暂缓：无头 Linux 登录成功路径、未提权 Windows 的两种提权成功路径、SAS 支持端的 Ctrl+Alt+Del 成功路径、阻止输入及断线恢复的现场物理验证。远端权限撤销实测也暂缓：连接管理器采用独立的远程输入保护，开启远程配置修改后，权限按钮仍未接受远程点击；查询确认权限未改变。自动测试覆盖权限拒绝状态保留、旧连接反馈隔离、并发请求令牌、错误状态、以及真实流上的关闭清理报文。

原版连接管理器的本地保护、提权/UAC 条件和协议缺少回执均未绕过；测试限制不记作成功。
