# 刷新、截图、锁屏、重启和结束后锁定

仅修改控制端，使用原版协议。所有写操作要求 AI 控制权，并沿用会话引用校验与 operation_id 去重。没有远端回执的操作明确返回 delivery=sent、confirmed=false。

## 工具

- `rd_screen_refresh(session_ref, display_id?, operation_id?)`：请求刷新指定屏幕，默认 primary。旧版对端不支持分屏刷新时请求全部屏幕，返回 effective_target。要观察新画面，用既有截图的 frame_seq 再调用截图接口。
- `rd_session_lock(session_ref, operation_id?)`：原版 LockScreen 系统命令，需要键鼠权限且非浏览模式。返回发送结果，需另看截图确认锁屏。解锁使用正常的系统登录流程。
- `rd_session_restart(session_ref, operation_id?)`：Windows/Linux/macOS 原版重启命令，需要重启权限。可能终止应用并断连；没有重启成功回执。通过会话查询与显式重连检查恢复，不能把断连视为重启成功。便携版未运行服务时可能需要被控端本地重新打开 RustDesk。
- `rd_connection_settings_set` 新增 `change: {setting: "lock_after_end", enabled: true/false}`。非 Android、键鼠权限已允许、非浏览模式时可用。保存设备偏好并通知当前对端；连接结束后由原版对端执行锁定，不把本地偏好读回当成实际锁定完成。

LockScreen 和 Ctrl+Alt+Del 是一次性协议命令，不计为按住的按键，归还控制权时不会补发一次命令。

## 连接错误

打开、重连及会话查询的默认摘要和完整详情均返回 `connection.error`。连接建立失败（包括远端离线）、传输错误、超时、对端断开及远端关闭原因保留原始错误文本，最多 1024 个字符；断开后仍可查询。新一轮连接清除旧错误，旧连接迟到的错误不能污染新连接。主动正常断开不凭空生成错误。错误文本只是连接层观察，不代表重启是否完成。

## 截图与保存

复用 `rd_screen_capture`，没有增加第二个截图工具：

| source | 语义与限制 |
| --- | --- |
| `decoded`（默认） | 既有视频解码画面，支持 max_width/max_height、after_frame_seq，返回输入映射 snapshot_id。只读时可在人类控制下使用。默认 wait_ms=0，缓存可立即返回；断连缓存明确标记陈旧。 |
| `remote_original` | 与原版控制条相同的远端原始 PNG 请求，对端 >=1.4.0，需要 AI 控制权。默认 wait_ms=10000，范围 1..30000；不能传缩放或帧序号。不返回输入映射 snapshot_id，remote_capture 报告 PNG 尺寸、显示器及接收时间。超时只表示未收到结果，已发送请求无法撤回。 |

两种来源都返回原生 PNG 内容块，限制 8 MiB。原始 PNG 太大时明确报错，可改用 decoded 的尺寸上限。原始截图按请求 ID、逻辑会话分别收取；迟到响应不会污染 GUI 截图缓存或弹出保存框。

可加 `save_path`（绝对路径，结尾 .png）把**返回的同一份 PNG 字节**保存到控制端，需要 AI 控制权。父目录必须存在；成功返回路径、字节数、SHA-256。先写临时文件，再无覆盖地发布完整文件，已有文件、目标符号链接均不覆盖。返回 unchanged 且没有图片时不会创建文件。错误分别报告 FILE_EXISTS、SAVE_FAILED；取消时已发送请求或已完成保存不能回滚。

为保存或远端原始请求使用 operation_id 可避免重复执行。操作元数据保留 5 分钟，图片沿用共享的 32 MiB/30 秒缓存；图片过期后重放仍返回保存结果，image_status=expired、image_content_index=null，不重新保存或重新截图。工具因可选写操作标记为非只读，但 decoded 且不保存仍允许只读控制。

## 验证记录

- 原版 Windows RustDesk 1.4.9 双屏同时请求原始 PNG，分别得到 1920×1080、2560×1440；请求/响应与保存结果正确对应。保存文件与返回图片逐字节一致，SHA-256 一致。
- decoded 保存及 1000 像素宽缩放通过，保留输入映射；相同 operation_id 重放原始截图不重复发送或保存。
- 已有文件和目标符号链接均返回 FILE_EXISTS 且内容不变；不存在的父目录返回 SAVE_FAILED；相对路径、原始截图混用缩放/帧序号及零等待被拒绝。
- 刷新指定显示器后通过 after_frame_seq 取得新帧；结束后锁定的显式设置与原偏好恢复通过。
- 归还控制权后 decoded 只读截图可用，原始请求、保存、刷新、锁屏、重启被拒绝。
- 被控端本地撤销键鼠及重启权限后，锁屏、重启、结束后锁定返回 PERMISSION_DENIED 且偏好不变；刷新和原始截图仍正常。
- 原始请求 1 ms 超时返回 SCREENSHOT_TIMEOUT，同 operation_id 重放保持原结果，后续新请求成功；实际 GUI 未弹出迟到响应的保存对话框。
- 恢复权限后实际锁屏成功，GUI 与 MCP 图片均看到 Windows 锁屏；相同操作 ID 重放未重复发送。使用提供的系统登录凭据解锁后恢复桌面。
- 启用结束后锁定并断开，重连后实际看到 Windows 锁屏；设备偏好保留，随后已恢复关闭。断连状态下刷新、锁屏、重启及原始截图请求被拒绝。
- 重启请求返回 sent/confirmed=false，相同 operation_id 重放未重复发送；随后对端断开；便携版经被控端本地重新打开后连接恢复，系统启动时间由 9 月 15 日 04:32 更新为 9 月 16 日 16:42（北京时间），确认实际重启成功。
- 自动测试：59 项 automation、12 项 MCP 全部通过；新增 Flutter 代码分析无问题。连接失败原因在打开、默认/完整查询和重连中实测可见，正常 Windows 会话 error=null。基础会话、控制权、去重、截图、断连/重连、画质状态及终端回归通过。

### 大图片客户端设置

实测锁屏 PNG 约 1.26 MB，Base64 后约 1.68 MB。Python 测试客户端所用 httpx2 默认 SSE 事件上限 1 MiB，会报 SSEError，SDK 随后尝试恢复流。将测试客户端 SSE 接收上限设为 16 MiB 后正常；Codex 原生 MCP 同尺寸图片也正常。服务端保持 8 MiB PNG 上限，客户端应为 Base64 及 JSON 开销留足空间。

### 窗口管理待办

复测发现关闭最后一个桌面标签后立即打开另一会话，可能停留在 GUI 未注册的 connecting 占位状态；重启本地客户端后可连接，已有桌面标签时重复验证正常。单独在窗口管理 issue 跟踪，不将该状态误报为对端离线。
