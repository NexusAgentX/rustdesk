# 本地视图、缩放与窗口控制

MCP 控制端复用桌面 Flutter 视图，被控端仍使用原版 RustDesk。以下设置不修改远端分辨率，也不改变 MCP 截图输出尺寸。

## 工具

| 工具 | 用途 |
| --- | --- |
| `rd_view_settings_get` | 从指定 Flutter 视图读取设置、当前缩放、光标选项、全屏、控制条固定状态及本地显示器列表 |
| `rd_view_settings_set` | `change` 指定一项设置和明确值，避免 toggle 重试反转状态 |
| `rd_view_window` | 展示窗口、关闭一个视图、通过原版多屏路径打开指定显示器 |

共有多个视图时，必须提供 `ui_session_id`，从 `rd_displays_get.local_views` 获取。读操作无需 AI 控制权；写操作需要已就绪桌面及 AI 控制权，Flutter 执行前重新校验。写入支持既有 `operation_id` 去重。

## 设置与作用范围

- `scale`：`mode: original/adaptive/custom`。custom 要求 `percent: 5..1000`，其他模式不传 percent。保存设备偏好并立即更新目标视图的画布；实际渲染比例另见 `scale.render_scale`。
- `individual_windows`：`enabled` 保存设备偏好，决定以后通过控制条选择显示器时是否打开独立窗口，不立即创建或关闭已有窗口。
- `use_all_local_displays`：`enabled` 保存设备偏好，在下一次新连接时按原版逻辑应用。启用需要本机至少两块显示器及对端多屏支持；关闭不撤销现有窗口布局。
- `show_remote_cursor`、`follow_remote_cursor`、`follow_remote_focus`、`scale_cursor`：保存设备偏好。支持条件取决于对端平台、版本、光标是否已嵌入画面、当前单屏/全屏显示范围及是否存在多个视图；查询结果将设置值与有效状态分开。
- 启用 `follow_remote_cursor` 同时开启远端光标显示；先关闭跟随后，才能关闭光标显示。禁用跟随保留显示光标的设置。
- `follow_ai_display`：全局持久设置，复用首版跟随 AI 操作屏幕功能。鼠标移动本身不触发跟屏，点击等坐标操作沿用既有规则。
- `toolbar_pinned`：保存全局控制条固定偏好并立即更新目标视图。其他已打开视图保持各自已加载的状态。
- `fullscreen`：控制整个本地系统窗口，影响共享该窗口的其他标签；不是远端桌面的分辨率设置。

除 `scale` 外，各设置均使用 `enabled: true/false`。

## 窗口操作

`action` 是对象：`{"action":"show"}`、`{"action":"close"}` 或 `{"action":"open_display","display_id":"1"}`。

- show 展示并提升目标系统窗口。
- close 只关闭目标桌面视图；最后一个视图关闭时原版会断开逻辑会话。`rd_session_close` 则关闭该会话的全部视图。
- open_display 复用原版显示器窗口路径，可能激活已有窗口或创建新窗口。显示器编号从 0 开始，需在线且支持原版多视图协议。
- open/close 返回 `delivery: sent, confirmed: false`；不能据此宣称窗口已打开/关闭。用 `rd_displays_get` 观察实际视图数量和显示器，必要时读取 `rd_session_get`。
- 等待超时返回 pending。不要盲目重发窗口操作；使用操作去重及状态查询。

## 验证记录

- 原版 Windows RustDesk 1.4.9 双屏：原始/适应/125% 自定义缩放读回通过，远端分辨率未改变；135% 自定义缩放重连后保留，结束后恢复原设置。
- 光标显示与跟随、跟随焦点、缩放光标、AI 跟屏、独立窗口偏好、控制条固定及系统全屏开关读回通过；跟随光标时隐藏光标的冲突被拒绝。
- AI 跟屏关闭时点击副屏不切换本地观看；开启后同样点击切至副屏。
- 原版路径实际创建第二显示器窗口并渲染画面；同一 operation_id 重试未重复创建。多视图未指定目标时报歧义；此时跟随光标被拒绝；关闭新增视图后原会话仍连接。
- 无效缩放参数、不存在的视图、归还控制权后写操作被拒绝；归还后仍可查询。
- 本机只有一块显示器，启用 use_all_local_displays 明确拒绝。需求方决定先发布其余功能，双本地显示器成功路径实测暂缓，继续在 #6 跟踪。
- 新增 Flutter 视图控制文件分析无问题；其他相关文件仅有原有提示。

- 自动测试：54 项 automation 与 11 项 MCP 全部通过；会话、重连、终端实机回归通过，安装版及 ZIP 解压产物的严格签名校验通过。
