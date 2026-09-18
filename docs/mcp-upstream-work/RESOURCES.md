# RustDesk MCP 测试资源与访问配置

更新：2026-09-18。资源清单是覆盖目标，实际准备和测试状态记录在 [验证记录](VALIDATION.md)。一个 PR 完整实现 MCP 集成，Windows/macOS/Linux/Android/iOS 都属于功能范围，参考实现 67 项工具能力全部安排验收。下述配置主要用于运行测试；构建优先由 GitHub Actions 和现有 Mac 完成。

## 采用的资源方案

用户希望日常测试集中在当前 Mac 的虚拟机，按需轮流开机；已有 Android 手机和若干 Windows 真机，可以安排双显示器 Windows 真机，暂时没有 iPhone。采用此方案，完整单 PR 的功能范围保持不变。

用户已确认：先把现有资源能完成的实现与测试做充分，确实拿不到的设备和相应测试放到最后，由用户请求社区支援。下表及后续资源清单表示测试覆盖目标，不是要求用户全部配齐的前置清单；缺资源项明确记录为“待社区补测”。

本机已核实为 ARM64、24 GiB 内存、约 598 GiB 可用磁盘，已安装 UTM 4.7.5 与 Xcode 27.0。默认只运行一台桌面 VM 或一个 iOS 模拟器；需要双端时先用 Mac 与一台 VM 配对。安装版/便携版环境使用干净磁盘副本或快照轮换，不要求两台同时运行。大规模 Rust/Flutter 编译尽量交给 CI，避免与虚拟机争内存。

| 名称建议 | 本机运行方式 | 建议配置 | 用途 |
| --- | --- | --- | --- |
| rd-win11-arm64 | UTM ARM64 虚拟化 | 4 vCPU / 8 GiB / 80 GiB | 日常 Windows 流程、原生 ARM64、安装版/便携版状态分别保存 |
| rd-linux-arm64 | Ubuntu 24.04 ARM64 + GNOME，UTM 虚拟化 | 4 vCPU / 4–6 GiB / 50 GiB | 分别登录 Xorg、Wayland，日常 Linux 流程与 ARM64 |
| rd-win7-x64 | UTM x86_64 全系统模拟 | 默认单核 / 3–4 GiB / 40 GiB | Win7 SP1 实际启动、旧系统 API、基础功能回归；不用于性能测量 |
| rd-win-x64 | Windows 10/11 x64，按需添加 UTM 模拟环境 | 默认单核 / 4–6 GiB / 70 GiB | x64 软件功能调试；耗时较长的回归可集中用已有真机 |
| rd-linux-x64 | Linux x64，按需添加 UTM 模拟环境 | 默认单核 / 4 GiB / 40 GiB | x64 运行差异；CI 同时覆盖 x64 原生构建/测试 |
| iOS Simulator | Xcode 模拟 iPhone/iPad，轮流运行 | 一次一个设备，使用宿主资源 | iOS 完整功能开发、GUI、MCP 和远端操作测试；补齐原生库的模拟器构建 |
| Android 手机 | 用户已有，USB + adb | 型号/系统/ABI 接入后核实 | 全量能力、后台/网络切换、文件授权、录制保存；模拟器仅补版本和 ABI |
| Windows 真机 | 用户按需提供，至少一次双显示器 | 现有配置，接入后核实 x64/GPU | 原生 x64、硬解/VRAM、真实双屏/DPI、驱动、输入屏蔽/隐私恢复 |

Apple Silicon 上 ARM64 来宾可以硬件虚拟化，x86/x64 来宾需要模拟；安装多个系统可行，但跨架构环境不适合作为主要编译环境。UTM 对跨架构强制多核也有正确性提示，先采用默认 CPU 配置。[UTM 架构说明](https://docs.getutm.app/settings-qemu/system/)

Windows ARM64 中运行 x64 程序只能增加应用兼容性证据，不能替代 x64 Windows 原生环境，尤其是驱动和系统组件相关路径。[微软 ARM 兼容性说明](https://learn.microsoft.com/en-us/windows/arm/apps-on-arm-troubleshooting-x86)

VM 用完后关机释放内存。初期为系统镜像、干净副本和移动 SDK 预留约 150–250 GiB，按实际增长管理缓存。虚拟磁盘配置值不等于初始占用；以 UTM 实际磁盘格式和宿主占用为准。

## 访问方案

部署与收集日志走 SSH，桌面行为通过原版 RustDesk 和虚拟机控制台观察。测试期间轮换被测控制端和原版被控端，至少保留一条不依赖被测 RustDesk 的恢复通道。

| 环境 | 管理入口 | 桌面/设备入口 |
| --- | --- | --- |
| Linux | OpenSSH，专用用户 `rdtest`，能 sudo 安装测试依赖 | 已登录桌面 + RustDesk；虚拟机控制台恢复 |
| Windows 10/11 | 内置 OpenSSH Server，专用测试管理员，SSH 中执行 PowerShell | 已登录交互桌面 + RustDesk；保留普通用户供权限测试 |
| Windows 7 | 虚拟机宿主/控制台是必备入口；如已有可用 SSH 可附带 | 原版 RustDesk；必要时通过控制台/共享介质传递测试包 |
| Android | 本机或设备宿主上的 adb；远程宿主提供 SSH | 模拟器或 USB 真机 |
| iOS | 当前采用本机 Xcode Simulator / simctl | 模拟器控制台；借到真机后增加 USB 配对和开发签名 |

Windows 7 不要求安装 Windows 10/11 的内置 OpenSSH 功能；[微软说明](https://learn.microsoft.com/en-us/windows-server/administration/openssh/openssh_install_firstuse)的最低系统条件不包括 Win7。

SSH 只负责部署和管理。Windows GUI 应用需要启动在已登录的交互桌面中；SSH 启动的非交互进程不能当作 GUI 验证。Linux Wayland 也要使用真实用户桌面会话，不能用 Xvfb 的成功结果代替。

本机 VM 优先使用 Mac 可访问的 UTM 共享网络，必要时配置每台不同的 SSH 转发端口；UTM 控制台可在 RustDesk 中断后恢复。Windows 真机使用内网或已有 VPN；如需跳板，配置 `ProxyJump`。MCP 继续绑定被测设备的 loopback；测试客户端在设备上运行，或经 SSH/USB 转发访问，不需要放开 MCP 的公网监听。

## SSH 公钥

测试专用公钥单独分发，具体访问配置保留在仓库外。本文只保留通用配置步骤。

Linux：追加到测试用户 `~/.ssh/authorized_keys`，目录权限 700、文件 600，归该用户所有。

Windows：启用并启动 OpenSSH Server。若测试账号属于 Administrators，公钥写入 `C:\ProgramData\ssh\administrators_authorized_keys`，ACL 只授予 Administrators 和 SYSTEM；普通用户则使用 `C:\Users\<用户名>\.ssh\authorized_keys`。非英文 Windows 可以按 SID 配置 ACL。[微软密钥配置说明](https://learn.microsoft.com/en-us/windows-server/administration/openssh/openssh_keymanagement)

SSH 使用本任务公钥。当前只运行 iOS 模拟器，不要求用户现在准备真机签名证书；以后借到真机时再配置开发签名，账号可由用户直接在 Xcode 配置。

## 移动端接入验证

Android 的首轮外部测试可使用 adb 端口转发，将开发机客户端接到设备内的 loopback MCP 服务。[adb 端口转发](https://developer.android.com/tools/adb#forwardports) 提供该机制。测试端口按可用情况配置，避免与当前参考版服务冲突。

iOS 使用模拟器验证完整功能实现：UI、服务启停、MCP 请求、远端会话/截图/输入、终端/文件等适用功能及生命周期回调。同时保留真机目标的独立编译和链接检查，不能仅通过模拟器构建就声称真机二进制有效。

当前主线的 Xcode 工程直接链接 `target/aarch64-apple-ios/release/liblibrustdesk.a`，部分配置的 `SUPPORTED_PLATFORMS` 只有 `iphoneos`。需要补 `aarch64-apple-ios-sim` 和相应 C/C++ 依赖、插件及按 SDK 选择库的构建路径；不是下载模拟器就立即能运行，实际打通后记录证据。

模拟器与真机在资源、性能和硬件特性上有差异。[Apple 当前说明](https://developer.apple.com/documentation/xcode/running-your-app-on-simulated-or-physical-devices)要求用真机验证设备特性。因此将真机后台挂起/恢复时序、系统内存回收、设备网络/USB 接入、签名安装及相关权限行为列为待补运行证据。模拟器中触发生命周期回调不能证明真机同样运行；同宿主的网络连通也不能证明真机客户端可达。

没有 iPhone 不阻止完整实现和 PR 提交，也不缩减 iOS 功能范围。不要求采购或借齐设备；现有资源验证完成后，准备可复现的测试说明，由用户请求社区测试者补测。PR 按实际证据标注模拟器与真机各自状态；设备相关合并要求由上游判断，不把缺少证据标成通过。开发签名到真机测试时再安排。

设备在远端时，优先提供连接设备的开发宿主，使用 SSH 访问宿主上的 adb/Xcode/转发工具。

## 虚拟机准备细节

- 配置一个可登录的测试桌面用户，保留安装、重启和快照恢复能力；提供干净系统快照。
- 图形桌面保持可运行；锁屏、休眠和注销由测试用例主动触发。
- 安装原版 RustDesk 作为基线，后续测试构建由我部署；提供所用服务器配置方式及测试设备 ID。连接凭据在访问建立后单独配置。
- 先单显示器即可；至少一个 Windows/Linux 被控环境后续能够设置双显示器、不同 DPI 和负坐标布局。
- 保留默认 UAC/权限机制，测试中需要区分普通用户和提权行为。
- 若网络经过 NAT，确保测试节点之间能按原版 RustDesk 的正常方式连接，能从宿主访问控制台和重启节点。

- 验证剪贴板和文件传输时，关闭 UTM/Simulator 与宿主的自动剪贴板同步，并使用来宾磁盘上的独立测试目录，避免虚拟化工具的共享功能让 RustDesk 测试出现假成功。[UTM 剪贴板共享](https://docs.getutm.app/settings-qemu/sharing/)

## 第二批资源

| 资源 | 优先级与作用 |
| --- | --- |
| Windows 7/10 x86 | 本机按需增加 UTM 模拟环境，覆盖 32 位 Sciter；必要时用 x64 Windows 真机上的 x86 VM |
| Intel Mac | CI 先覆盖编译；拿不到设备时，x86_64 真实 GUI 验证留到最后请求社区补测 |
| 双显示器 Windows 真机 | 用户已可提供；集中测试 x64、硬解/VRAM、多屏/混合 DPI/热插拔、物理输入与驱动 |
| Android 较旧系统与第二种 ABI | 补主线最低部署目标、armv7 与当前系统差异 |
| iPhone/iPad | 补模拟器无法提供的设备运行证据；当前缺资源，最后请求社区补测 |

这些资源按需启用。若本机 x64 模拟明显拖慢测试，可集中预约 Windows 真机，或在其上运行 x64 Linux/旧 Windows 虚拟机，并继续从 Mac 管理控制台。

## 完整功能需要的测试条件

- Windows 安装版节点允许安装上游原版虚拟显示驱动，可创建/移除显示器，保留系统快照；支持显示器和 DPI 调整。
- Windows 便携版节点保留正常 UAC，有普通用户和测试管理员，用于提权成功、拒绝与恢复；管理员安装版不能代替这一条路径。
- 至少一个被控节点能按原版 RustDesk 配置 2FA、连接确认和各项远端权限；我通过虚拟机控制台或你提供的本地配合完成权限切换，验证真实成功/拒绝与运行时撤销。
- Linux 节点有可登录的测试系统账户，可验证主线仍提供的系统账户认证路径；实际不受支持的旧路径按主线能力记录。
- 文件传输使用独立测试目录，允许创建、覆盖、重命名、删除测试文件，准备可中断的大文件与重名目录；我生成测试数据。移动端提供文件选择器/应用目录/授权撤销测试条件。
- 能在被控节点启动本地 TCP 回显服务及终端，验证任意字节、隧道认证和端口释放；端口与进程由测试脚本创建并清理。
- 录制与聊天使用测试会话；磁盘留有视频输出空间，测试可重连、撤销权限和退出应用。
- 至少一次输入屏蔽/隐私模式的测试需要通过宿主控制台观察真实效果和恢复。若虚拟机无法提供等价的本地输入/GPU 行为，则补真机证据。

参考实现中未验证的成功路径全部重新列入本次测试清单。现有资源能测的先完成，确实缺资源的注明原因并留到最后请求社区补测；缺资源不改变实现范围。

## 社区补测交付

完成现有资源覆盖后，再汇总剩余缺口。每项提供精确提交及可用构建、所需系统/架构/硬件、操作步骤、预期结果、需要收集的日志和已有验证证据，方便社区成员直接复测。已发现的代码缺陷继续修复，不归入单纯的资源缺口。求助材料由我整理、用户发布；当前不发送社区消息。

## 配好后交付的信息

每个节点在仓库外提供以下信息，仓库内仅用节点别名关联测试结果：

```text
节点名称：
系统版本 / 架构：
SSH 主机或 IP / 端口 / 用户名：
跳板（如需）：
SSH 主机公钥指纹：
已安装任务公钥：是 / 否
GUI 用户 / 当前桌面类型：
RustDesk 版本 / 设备 ID / 服务器配置方式：
控制台与快照恢复方式：
是否可安装、重启、创建测试用户：
可用时间：
GPU / 显示器（如有）：
```

移动设备补充：型号、系统版本、连接的开发宿主、USB/adb/Xcode 配对状态；以后借到 iOS 真机时，再补可用的开发签名 Team。拿到这些信息后，我完成 SSH 配置、环境检查、测试包部署、MCP 客户端安装和自动化脚本。
