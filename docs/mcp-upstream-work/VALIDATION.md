# 构建与运行验证记录

更新：2026-09-18。所有结果对应明确提交、配置和环境。原版 macOS 完整应用构建、协议模块测试/互操作和三套移动原生库已有通过结果；iOS 应用已在隔离兼容修正后启动，移动 MCP 生命周期尚未通过。

## 记录规则

- 构建模式分为 F0（未编译 MCP）、F1（编译 MCP 但运行关闭）、F2（MCP 启用）。MCP feature 名为 `mcp`，具体组合见每次执行记录。
- 构建表记录 F0 与含 MCP 的构建结果；运行表分别记录 F1/F2。F0 的原有行为回归也需要运行证据。
- 构建成功不等于运行通过；原版参考分支的结果不计入新分支验收。
- 状态使用 `未执行`、`进行中`、`通过`、`失败`、`待社区补测`。平台不适用必须附主线/协议/系统依据，不能依据控制端系统错误禁用远端能力。
- 已有通过结果只在相关代码、依赖、构建环境改变或有新问题时重跑；新的失败保留记录，并另记修复后的结果。
- “现有资源”优先完成，拿不到的设备测试最后汇总。代码缺陷继续修复，不转成资源缺口。

## 构建矩阵

工具链、特性和打包参数以主线对应配方为准；执行时填写精确版本。下表是覆盖计划，不能当作已配置完成的 CI。

| ID | 消费者 / 目标 | F0 构建 | 含 MCP 构建 | 证据 / 说明 |
| --- | --- | --- | --- | --- |
| B01 | Windows Flutter x86_64 | 未执行 | 未执行 | 保留 Win7 所需工具链及链接兼容 |
| B02 | Windows Flutter aarch64 | 未执行 | 未执行 | 按主线 ARM64 Flutter 配方 |
| B03 | macOS Flutter aarch64 | 通过 | 未执行 | V003：原版完整 Rust + Flutter Release，未运行 |
| B04 | macOS Flutter x86_64 | 未执行 | 未执行 | CI 构建，真机运行另列 |
| B05 | Linux Flutter x86_64 | 未执行 | 未执行 | 发行包与图形依赖按配方 |
| B06 | Linux Flutter aarch64 | 未执行 | 未执行 | 主线 ARM64 配方 |
| B07 | Android aarch64 | 未执行 | 进行中 | V005：含 MCP 完整原生库通过，APK 未完成 |
| B08 | Android armv7 | 未执行 | 未执行 | 核实对应发行配方与最低系统目标 |
| B09 | Android x86_64 | 未执行 | 未执行 | 核实对应发行配方 |
| B10 | iOS aarch64 真机 | 未执行 | 进行中 | V005：含 MCP 完整 Rust 静态库通过，未签名安装 |
| B11 | iOS aarch64 模拟器 | 进行中 | 进行中 | V005：原版及含 MCP 原生库通过；应用已链接安装，经隔离 UIScene 修正启动；具体构建身份见移动报告 |
| B12 | Windows Sciter i686 | 未执行 | 待确认构建适用性 | 保留原有消费者，专门的历史工具链 |
| B13 | Linux Sciter x86_64 / armv7 | 未执行 | 待确认构建适用性 | 执行时按各架构拆开记录 |
| B14 | Web 客户端 / 桥接 | 未执行 | 待确认构建适用性 | 既有客户端回归，不声称浏览器内提供本地 MCP 监听 |
| B15 | F-Droid / 其他发行包 | 未执行 | 待确认构建适用性 | 执行时列出具体配方、权限和 feature 组合 |

## 运行环境及覆盖

| ID | 环境 | 准备状态 | 运行状态 | 重点 |
| --- | --- | --- | --- | --- |
| E01 | 当前 macOS ARM64 | 主机可用，原版新应用已构建 | 未执行 | 全量 MCP、GUI、人工接管与默认路径 |
| E02 | Windows 11 ARM64 VM | 待配置 | 未执行 | Windows 控制端/被控端、安装版与便携版 |
| E03 | Linux ARM64 VM | 待配置 | 未执行 | X11/Wayland 分别验证、桌面会话与权限 |
| E04 | Android 手机 | 用户可提供，待 USB/adb 接入和 ABI 确认 | 未执行 | 全量适用能力、授权、挂起/恢复、网络切换 |
| E05 | iOS 模拟器 | iOS 27 runtime Ready，模拟器已 boot | 进行中 | 原版启动失败；隔离 UIScene 适配后显示首页，MCP 实际生命周期未验收 |
| E06 | Windows 真机（可双显示器） | 用户可提供，待接入与架构确认 | 未执行 | 原生 x64/GPU、双屏/DPI/热插拔、驱动与隐私/输入恢复 |
| E07 | Win7 / x64 / x86 模拟环境 | 按需配置 | 未执行 | 旧系统 API、架构差异；模拟性能不代表产品性能 |
| E08 | iPhone/iPad | 当前缺资源 | 待社区补测 | 真机挂起、资源回收、设备网络、签名安装与权限 |
| E09 | Intel Mac 真机 | 当前缺资源 | 待社区补测 | x86_64 实际 GUI/原生运行路径 |

每次运行记录控制端、被控端和连接方向。按能力清单覆盖正向功能、拒绝/取消/失败与恢复；多客户端、人工接管、排队输入撤销及断连按键释放需要明确结果。关闭虚拟化软件的自动剪贴板同步后再测 RustDesk 剪贴板和文件传输。

## 已有执行证据

### V001 — 起始主线的工作区清单解析

- 日期：2026-09-18，前序准备阶段执行。
- 提交：`5278fcab685723099e9950d56cb605807db5aa2a`。
- hbb_common：`0eb175963c950cba8d2f4f2a480f49db27b42c39`。
- 环境：本机 macOS ARM64，Rust 1.75.0。
- 命令：`cargo +1.75.0 metadata --no-deps --locked`。
- 结果：通过；工作区清单可解析。
- 范围：未编译依赖或应用、未实现 MCP；不能推导任何平台的完整构建或运行通过。
- 证据保存：保留前序执行摘要，原始完整输出未归档；后续执行按下述格式保存结果。

### V002 — Rust 1.75 SDK 调查及隔离 HTTP 栈验证

- 日期：2026-09-18，子代理 `mcp_rust175_research` 完成。
- 报告：[MCP-SDK-RESEARCH.md](MCP-SDK-RESEARCH.md)，包含 E01–E12 的命令、依赖组合、结果及来源。
- 实验位置：`/Users/laysath/.codex/tmp/mcp-rust175-research/experiments/`，各实验保留锁文件及日志；未修改产品 Cargo.toml/Cargo.lock。
- 实际成功范围：部分旧协议 SDK 的宿主 cargo check；主线锁定版本的 Hyper/Tokio/Serde 组合在 Rust 1.75 下通过一次本机 loopback HTTP POST/JSON 往返及 Windows i686 cargo check。
- 实际失败与限制：现代 SDK 候选有 MSRV、源码或依赖组合问题；未穷尽所有历史依赖 pins。没有完整 MCP 客户端互操作、Windows 链接/运行、Win7 或移动端运行证据。
- 结论用途：支持依赖选型和集中协议适配层评估；不改变上方产品构建矩阵或 67 项能力的待实现/待验证状态。

### V003 — 原版 macOS 完整构建通过

- 日期：2026-09-18，A01/B03；详细环境、命令及日志见 [MACOS-BASELINE-BUILD.md](MACOS-BASELINE-BUILD.md)。
- 基线：独立 upstream `5278fcab685723099e9950d56cb605807db5aa2a` 快照及固定子模块；Rust 1.81、Flutter 3.24.5、Xcode 27。
- 通过：桥接生成、16 项光标测试、Rust Release 和 Flutter Release，构建退出码 0。主程序、service、dylib 均 ARM64/minos 12.3。
- Xcode 27 需要进程级 `FLUTTER_XCODE_MACOSX_DEPLOYMENT_TARGET=12.3`；详细临时环境调整见报告。Rust Cargo.lock 与基线一致，Flutter 解析变化仅隔离保留。
- 仅 ad-hoc 签名；未启动/安装或远控实测，不计 E01 通过，也不包含 MCP。

### V004 — MCP 模块测试与官方客户端互操作

- 日期：2026-09-18，A02/B01/V01；对应尚未提交的新模块，报告 [MCP-PROTOCOL-IMPLEMENTATION.md](MCP-PROTOCOL-IMPLEMENTATION.md)。
- Rust 1.75 同源码隔离 harness 的 15 项测试通过；官方 TS sdk 1.30.0 与 client 2.0.0 分别验证两版协议通过。
- Windows i686、Android ARM64、iOS 真机/模拟器目标 cargo check 通过；这些 check 不代表平台链接或运行。工作区 feature on/off 锁文件解析通过，仅新增 httpdate 1.0.3，未升级已有包。
- 真实业务工具、业务控制权、OAuth 和完整应用运行不在此结论内；fixture 不计入 67 项能力完成数。移动 runtime 后续新增测试另见 V005，不重复累计。

### V005 — 移动原生库与应用启动进展

- 日期：2026-09-18，A03/B07/B10/B11；含未提交移动修正，精确源码哈希、命令与日志见 [MOBILE-INTEGRATION.md](MOBILE-INTEGRATION.md)。
- Rust 1.75 的 Android ARM64 共享库、iOS ARM64 真机及模拟器静态库均已完整构建成功，features 为 flutter,hwcodec,mcp。原版 iOS 模拟器 Rust 库也通过。
- 移动 runtime 的 4 项宿主测试通过；真实 async runner 隔离验证证明 online 查询阻塞/队列满时 HTTP 仍响应，stop 关闭监听。不是移动运行或真实业务验收。
- iOS 27 模拟器已启动，应用实际链接/安装成功；子代理报告启动崩溃要求 UIScene，隔离快照最小 UIScene 适配后已实际显示中文连接首页，Rust 初始化/NAT 响应正常（logs/ios-app-scene.png）；该适配未写回产品分支，应用内 MCP 请求与挂起/恢复仍待实测。Android APK 打包遇官方 Maven TLS 下载中断，继续处理。
- 尚无移动 MCP 应用实际运行通过证据；无 iPhone 真机，Android 手机未接入。

## 后续执行条目

每次有新结果追加条目，关联上方构建/环境 ID 和 CAPABILITIES 的 C/N 编号。模板：

```text
记录 ID / 日期：
任务 / 能力 ID：
源码提交 / 未提交差异说明（如有）：
控制端系统 / 架构 / 构建模式 / 工具链：
被控端系统 / 版本 / 架构 / 权限（如有）：
命令或操作步骤：
预期结果：
实际结果与状态：
日志 / 构建 / 截图证据位置：
相关问题 / 修复 / 后续验证：
```

## 社区补测队列

| ID | 缺口 | 当前替代验证计划 | 求助准备状态 |
| --- | --- | --- | --- |
| G01 | iPhone/iPad 真机运行 | iOS 真机 Rust 库已构建；完整应用及模拟器功能验收仍待完成 | 最后整理；尚无可分发测试构建 |
| G02 | Intel Mac 真机运行 | CI x86_64 构建 + ARM64 Mac 流程测试，均待执行 | 最后整理；尚无可分发测试构建 |

现有资源测试完成后再确定剩余精确缺口。每项材料需包含提交、可用构建、所需系统/架构/硬件、步骤、预期结果和日志要求，交给用户请求社区测试；收到结果后照常记录原始环境与证据。
