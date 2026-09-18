# 完整 MCP 集成能力清单

更新：2026-09-18。交付为一个完整 PR，不拆出后续功能 PR。基线为参考分支 `2e8ab16a5` 中实际注册的 67 个工具；这些名字用于追踪能力，不约束新 API 必须保留旧拼写或旧参数。

初始化时所有条目均为待实现/待验证，当前逐项状态维护在下表。每个条目需记录新入口、Windows/macOS/Linux/Android/iOS 状态、主线协议或平台边界、自动化证据、运行证据及 feature-off 回归。只有真实系统/协议限制可记为不适用；未实现或未验证必须保留原状态。

实现、构建与运行验证分别记录。用户已确认先完成现有资源能覆盖的测试，确实拿不到设备的运行项标为“待社区补测”，放到最后汇总，由用户请求社区支援；这些资源不作为开发或提交 PR 的前置条件。每个补测项保留系统/架构/硬件需求、精确提交与构建、操作步骤、预期结果和日志要求。功能实现仍完整纳入同一个 PR，已发现缺陷继续修复。

| 功能组 | 参考工具 | 验收重点 |
| --- | --- | --- |
| 会话（9） | `rd_session_list`、`rd_session_open`、`rd_session_attach`、`rd_session_detach`、`rd_session_get`、`rd_session_authenticate`、`rd_session_disconnect`、`rd_session_reconnect`、`rd_session_close` | 四类会话、可见 GUI、远端认证/2FA/确认、重连代次、准确关闭目标 |
| 控制（3） | `rd_control_request`、`rd_control_cancel`、`rd_control_release` | 批准/拒绝/接管；输入、文件、终端、隧道等所有写操作的权限一致 |
| 观察/输入（3） | `rd_screen_capture`、`rd_screen_refresh`、`rd_input_send` | 按显示器帧、布局变化、软硬解、Unicode、按键释放、排队撤销 |
| 显示器（5） | `rd_displays_get`、`rd_display_modes_get`、`rd_display_select`、`rd_display_resolution_set`、`rd_virtual_display_set` | 负坐标/DPI、多个视图、分辨率、虚拟驱动能力与实际反馈 |
| 连接/视图（5） | `rd_connection_settings_get`、`rd_connection_settings_set`、`rd_view_settings_get`、`rd_view_settings_set`、`rd_view_window` | 配置与真实观测分开、桌面窗口与移动页面语义、持久化作用域 |
| 文本剪贴板（5） | `rd_clipboard_settings_get`、`rd_clipboard_settings_set`、`rd_clipboard_read`、`rd_clipboard_write`、`rd_clipboard_type` | 同步权限、缓存新鲜度、文本输入、移动剪贴板访问条件 |
| 文件（8） | `rd_file_list`、`rd_file_manage`、`rd_file_transfer`、`rd_file_jobs`、`rd_file_job_get`、`rd_file_job_cancel`、`rd_file_job_resume`、`rd_file_conflict_resolve` | 本地/远端路径、移动沙盒授权、重名/取消/恢复、进度与完成证据 |
| 文件剪贴板（5） | `rd_file_clipboard_get`、`rd_file_clipboard_set`、`rd_file_clipboard_copy`、`rd_file_clipboard_paste`、`rd_file_clipboard_cancel` | Windows/Linux/macOS 原生机制、移动端实际能力、粘贴/取消的完成状态 |
| 安全/系统（8） | `rd_security_get`、`rd_input_block_set`、`rd_privacy_set`、`rd_session_elevate`、`rd_os_password_input`、`rd_ctrl_alt_del`、`rd_session_lock`、`rd_session_restart` | 主线权限、确认/未知结果、UAC、SAS、隐私与输入屏蔽恢复、凭据不落日志 |
| 聊天（2） | `rd_chat_send`、`rd_chat_read` | 双向历史、游标、去重、有界缓存、GUI 同步 |
| 录制（2） | `rd_recording_get`、`rd_recording_set` | 真正写入/完成/失败、权限撤销、移动保存路径与生命周期 |
| 终端（6） | `rd_terminal_list`、`rd_terminal_create`、`rd_terminal_read`、`rd_terminal_write`、`rd_terminal_resize`、`rd_terminal_close` | 原始字节/ANSI/中文、输出缺口、真实退出状态、多终端、移动 UI |
| 隧道（4） | `rd_tunnel_list`、`rd_tunnel_add`、`rd_tunnel_remove`、`rd_tunnel_authenticate` | loopback、独立认证/2FA、端口占用、二进制数据、撤权/关闭/挂起时清理 |
| 能力/操作（2） | `rd_capabilities_get`、`rd_operation_get` | supported/allowed/ready/unknown 区分、去重、超时和操作结果不混淆 |

合计 67 项。还需验收非工具能力：服务配置/启停、凭据轮换、认证/Host/Origin 校验、协议版本协商/生命周期、客户端断开/取消、资源上限、五平台设置和控制 UI、构建特性与桥接生成、安装包、文档和既有功能回归。

移动端不能只通过协议握手就记为完成；文件、录制、终端、隧道、剪贴板和安全控制都要在适用场景运行。对远端 Windows/Linux/macOS 的能力支持不能依据 MCP 控制端手机的操作系统错误禁用。

## 逐项执行状态

每个平台格按“实现/运行验证”记录：`待/未` = 待实现、未执行；实现还可记 `进行中`、`完成`；验证还可记 `通过`、`失败`、`待社区`。真实平台限制写 `不适用` 并链接依据。共享逻辑完成后仍逐平台确认桥接与生命周期；构建结果在 [验证记录](VALIDATION.md) 中独立记录。

填写“新入口/证据”时引用实际源码、提交及验证记录编号；合并工具或改名仍保留对应能力行。Windows/macOS/Linux/Android/iOS 列指 MCP 控制端，远端系统及权限条件在证据中注明。

| ID | 参考能力 | Windows | macOS | Linux | Android | iOS | 新入口/证据 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| C01 | `rd_session_list` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C02 | `rd_session_open` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C03 | `rd_session_attach` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C04 | `rd_session_detach` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C05 | `rd_session_get` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C06 | `rd_session_authenticate` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C07 | `rd_session_disconnect` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C08 | `rd_session_reconnect` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C09 | `rd_session_close` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C10 | `rd_control_request` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C11 | `rd_control_cancel` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C12 | `rd_control_release` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C13 | `rd_screen_capture` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C14 | `rd_screen_refresh` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C15 | `rd_input_send` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C16 | `rd_displays_get` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C17 | `rd_display_modes_get` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C18 | `rd_display_select` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C19 | `rd_display_resolution_set` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C20 | `rd_virtual_display_set` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C21 | `rd_connection_settings_get` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C22 | `rd_connection_settings_set` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C23 | `rd_view_settings_get` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C24 | `rd_view_settings_set` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C25 | `rd_view_window` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C26 | `rd_clipboard_settings_get` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C27 | `rd_clipboard_settings_set` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C28 | `rd_clipboard_read` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C29 | `rd_clipboard_write` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C30 | `rd_clipboard_type` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C31 | `rd_file_list` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C32 | `rd_file_manage` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C33 | `rd_file_transfer` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C34 | `rd_file_jobs` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C35 | `rd_file_job_get` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C36 | `rd_file_job_cancel` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C37 | `rd_file_job_resume` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C38 | `rd_file_conflict_resolve` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C39 | `rd_file_clipboard_get` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C40 | `rd_file_clipboard_set` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C41 | `rd_file_clipboard_copy` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C42 | `rd_file_clipboard_paste` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C43 | `rd_file_clipboard_cancel` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C44 | `rd_security_get` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C45 | `rd_input_block_set` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C46 | `rd_privacy_set` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C47 | `rd_session_elevate` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C48 | `rd_os_password_input` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C49 | `rd_ctrl_alt_del` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C50 | `rd_session_lock` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C51 | `rd_session_restart` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C52 | `rd_chat_send` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C53 | `rd_chat_read` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C54 | `rd_recording_get` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C55 | `rd_recording_set` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C56 | `rd_terminal_list` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C57 | `rd_terminal_create` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C58 | `rd_terminal_read` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C59 | `rd_terminal_write` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C60 | `rd_terminal_resize` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C61 | `rd_terminal_close` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C62 | `rd_tunnel_list` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C63 | `rd_tunnel_add` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C64 | `rd_tunnel_remove` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C65 | `rd_tunnel_authenticate` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C66 | `rd_capabilities_get` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |
| C67 | `rd_operation_get` | 待/未 | 待/未 | 待/未 | 待/未 | 待/未 | — |

## 配套交付

| ID | 交付项 | 状态 | 证据 |
| --- | --- | --- | --- |
| N01 | 服务配置、启停、凭据和客户端配置说明 | 待开始 | — |
| N02 | 五平台设置与控制状态 UI、本地化 | 待开始 | — |
| N03 | 协议协商、HTTP 传输、认证与资源限制 | 待开始 | — |
| N04 | 构建特性、桥接生成、发行包及原有消费者回归 | 待开始 | — |
| N05 | 正式使用文档、平台限制及社区复测说明 | 待开始 | — |
