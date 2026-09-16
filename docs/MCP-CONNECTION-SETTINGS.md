# 画质、编码、声音与连接诊断

控制端复用原版会话设置和解码统计，不修改被控端。工具 `rd_connection_settings_get` 读取设置与观测，`rd_connection_settings_set` 用 `change` 修改一项设置。多个桌面视图时指定 `ui_session_id`；单视图可省略。

写入需要就绪桌面和 AI 控制权，沿用 `operation_id` 去重。返回 `confirmed: true` 表示本地设置应用/读回完成，**不表示远端编码、FPS、声音或图像质量已经匹配**；`remote_effect_confirmed` 明确为 false。实际协商结果从 metrics 读取。

## 设置

| change.setting | 参数及限制 |
| --- | --- |
| `quality` | `preset: best/balanced/low/custom`；custom 必填 `quality: 10..2000`，可传 `fps: 5..120`；其他预设不传 quality/fps |
| `codec` | `preference: auto/vp8/vp9/av1/h264/h265`；仅接受 available_codecs 中的选择，集合取原版控制端解码及对端编码支持交集 |
| `true_color` | `enabled`；启用要求对端 >=1.2.4，当前已观测编码为 VP9 或 AV1 |
| `audio_muted` | `enabled`；需要远端音频权限，静音不等于撤销对端权限 |
| `quality_overlay` | `enabled`；保存设备偏好并更新目标视图的本地质量浮层 |

公共服务器的非直连会话不允许自定义 FPS，质量上限为 100；旧对端 <1.2.0 不允许自定义 FPS，<1.2.2 质量上限为 100。接口遵守控制条限制，拒绝超限，不悄悄降级。自定义质量但省略 fps 时，不另发 FPS 修改。

设置为设备持久偏好。预设切换沿用原版对 FPS 的处理；`settings.custom_fps` 是保存的自定义偏好，与当前实际 FPS 不同。编码偏好与实际编码也分别返回。支持列表不能保证硬件编码/解码器随后一定初始化成功；以实际观测为准。

## 指标与诊断

- speed、FPS、delay、目标码率、codec、chroma 均带 `value/known/observed_at/age_ms/fresh/unit`。
- speed 保留原版带单位的格式；FPS 保留当前视图的原版表示，显示全部屏幕时可能包含多个数值；delay 为毫秒；目标码率保留原版字段值及单位标记。
- 缺失值表示未知。速度、帧率、延迟和目标码率超过 10 秒标为陈旧；编码/色度是协商状态，保留到更新或重连。断开时指标不标为 fresh，重连会清空上一连接的观测。
- connection 返回连接是否就绪、加密、直连/中继、传输类型、是否使用公共服务器、对端平台/版本、原版权限覆盖及浏览模式。外层 session 使用已有 MCP 会话状态与控制权结构。
- `permission_overrides` 是原版实际收到的权限消息。桌面对端在登录时只发送被拒绝的音频权限；已认证桌面未报告拒绝时，音频可用。查询结果不授予权限。
- 质量浮层展示属于本地 UI，静音和编码选项通过原版协议生效；静态画面的实际 FPS 可能低于目标，属于正常观测。

## 验证记录

- 原版 Windows RustDesk 1.4.9、自建服务器中继连接：best/balanced/low 和自定义质量 65、目标 FPS 24 设置及读回通过；结束后恢复原设置。
- VP8、VP9、AV1、H264、H265 逐项切换，实际协商编码和后续新截图均验证通过；VP9 真彩色实际观测为 4:4:4。
- 质量低于下限、FPS 超限、预设混用自定义参数、未知编码枚举均被拒绝；静音与质量浮层开关读回通过，本地 GUI 实际显示质量浮层。
- 被控端本地撤销音频权限后，设置接口返回 PERMISSION_DENIED，原静音偏好保持不变；恢复权限后静音开关再次可用，最终恢复原设置。
- 六项指标返回时间戳和新鲜度；重连后保留设备偏好，已知指标的观测时间均来自新连接。
- 本次环境支持全部列出的编码；有效但双方不支持的编码、公共中继限制和旧版本限制未做对应环境实测，保留原版能力判断与限制。
- 断连后 connection.ready=false、codecs_known=false，所有指标 fresh=false，设置写入被拒绝；重新认证连接后恢复正常查询。
- 自动测试：55 项 automation、11 项 MCP 全部通过。Flutter 新文件分析无问题，相关旧文件仅有原有提示。基础会话、控制权、截图、操作去重、断连重连及终端回归通过。
- 安装版及发布 ZIP 解压产物严格签名校验通过。
