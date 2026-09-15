# macOS ARM64 构建基线

本阶段先验证 RustDesk 1.4.9 的官方 Flutter 客户端，再接入 MCP。产品范围见 [主控客户端需求](MCP-CONTROLLER-REQUIREMENTS.md)，接口契约见 [MCP 设计](MCP-API-DESIGN.md)。

## 固定输入

| 项目 | 版本或提交 |
| --- | --- |
| RustDesk | `1.4.9` / `6c578292e8ebbbec708b76986ba8c4bc7c509747` |
| hbb_common | `7e1c392c62d39c364127307cd408421dd5f8cfb0` |
| Flutter | `3.24.5` / `dec2ee5c1f98f8e84a7d5380c05eb8a3d0a81668` |
| Rust 基线工具链 | `1.81.0`，来自该版本官方 macOS CI |
| vcpkg | `120deac3062162151622ca4860575a33844ba10b` |
| vcpkg triplet | `arm64-osx`，使用仓库原有 manifest 与 overlay ports |
| NASM | `2.16.03`；官方 CI 明确要求 2.x |
| Flutter Rust Bridge 生成器 | `1.80.1`，启用 `uuid` |
| cargo-expand | `1.0.95` |
| macOS 构建目标版本 | `12.3`，与官方 ARM64 CI 一致 |
| Rust 功能 | `flutter,hwcodec,unix-file-copy-paste,screencapturekit` |

Rust 1.81.0 仅用于官方构建基线。后续 rmcp 接入需要至少 Rust 1.88，应在基线通过后单独升级和验证。Rust 和 Dart 依赖分别使用仓库的 Cargo.lock 与 pubspec.lock，不为解决下载问题更新版本。

### 首次解析的锁文件修正

上游 pubspec.lock 与该标签的 pubspec.yaml / Flutter 3.24.5 不一致，首次 `flutter pub get --enforce-lockfile` 报错。使用固定 SDK 执行一次离线 pub get 后重新锁定，共调整 16 个依赖条目：补齐已有 flutter_test 开发依赖及其传递依赖，并将测试链的 async、matcher、test_api 等版本对齐 SDK；vector_math 仅修正为已有的直接依赖分类。所有 Git 依赖提交保持不变，未修改 pubspec.yaml。

此后继续用 `--enforce-lockfile` 验证，避免后续构建隐式刷新依赖。这是构建所需的依赖解析变更；涉及共享 Dart 库的版本变化仍需由后续 GUI 编译和运行验收覆盖。

## 准备环境

需要完整 Xcode；仅安装 Command Line Tools 无法构建 Flutter macOS GUI。Xcode 安装、许可确认及首次初始化完成后，确认 `xcodebuild -version` 与 `xcodebuild -checkFirstLaunchStatus` 均成功。Apple 账户操作由本机用户完成。

构建缓存默认位于 `$HOME/Library/Caches/rustdesk-build`，不修改全局 Flutter 或 Rust 默认版本。脚本支持 `RUSTDESK_BUILD_CACHE`、`FLUTTER_ROOT` 和 `VCPKG_ROOT` 覆盖路径。

```bash
brew install cmake ninja pkg-config cocoapods
git submodule update --init --recursive
rustup toolchain install 1.81.0 --profile minimal --component rustfmt

mkdir -p "$HOME/Library/Caches/rustdesk-build"
git clone --depth 1 --branch 3.24.5 https://github.com/flutter/flutter.git \
  "$HOME/Library/Caches/rustdesk-build/flutter-3.24.5"
```

vcpkg checkout 必须使用上表提交，然后执行 `bootstrap-vcpkg.sh -disableMetrics`。NASM 2.16.03 可从 [官方发布目录](https://www.nasm.us/pub/nasm/releasebuilds/2.16.03/) 下载源码，在本机编译并安装到缓存下的 `nasm-install`。

生成器安装到缓存下，避免覆盖其他项目工具：

```bash
cargo +1.81.0 install cargo-expand --version 1.0.95 --locked \
  --root "$HOME/Library/Caches/rustdesk-build/rust-tools"
cargo +1.81.0 install flutter_rust_bridge_codegen --version 1.80.1 \
  --features uuid --locked \
  --root "$HOME/Library/Caches/rustdesk-build/rust-tools"
```

构建脚本为旧版桥接生成器单独设置 `RUST_LOG=info`；该版本仅接受 info/debug，继承其他日志级别会直接 panic。生成时禁用自动修改 lib.rs，使用官方已经存在的模块声明。

Flutter SDK 需要应用官方 RustDesk CI 的两项处理，仅作用于这份专用 SDK：

1. 应用仓库中的 `.github/patches/flutter_3.24.4_dropdown_menu_enableFilter.diff`。
2. 注释 `packages/flutter/lib/src/scheduler/binding.dart` 中的 `_setFramesEnabledState(false);` 调用，对应官方 macOS CI 的帧调度处理。

依据为固定底座的 `.github/workflows/flutter-build.yml` 和 `.github/workflows/bridge.yml`。官方 CI 的默认桥接产物由 Flutter 3.22.3 生成；本地先用客户端对应的 3.24.5 生成，必须通过后续 Rust 和 Flutter 编译验证，不能预先认定产物兼容。

## 分阶段执行

从仓库根目录运行：

```bash
scripts/build-macos-arm64.sh check
scripts/build-macos-arm64.sh deps
scripts/build-macos-arm64.sh bridge
scripts/build-macos-arm64.sh rust
scripts/build-macos-arm64.sh gui
```

也可以使用 `all` 顺序执行全部阶段。失败即停止，不把最后一条复制命令成功误报为构建成功。各阶段可单独重跑，诊断日志存放在忽略目录 `build/macos-arm64/logs/`。

脚本用进程环境指定 ARM64、macOS 12.3 和 Rust 工具链；不改写官方 build.py、Cargo.toml、Podfile 或 Xcode 工程。GUI 阶段的预期产物为 `flutter/build/macos/Build/Products/Release/RustDesk.app`，不自动安装或替换 `/Applications/RustDesk.app`。

复制 Rust 辅助程序 service 后，脚本重新执行本地 ad-hoc 签名并校验整个应用包，保留主程序原有 entitlements。此签名供本机开发验证使用，不是 Developer ID 签名或公证发布。

首次构建如需手动执行 `pod install`，必须先等待 `flutter precache --macos` 完成，否则 Podfile 的 post-install 会因缺少 release FlutterMacOS.xcframework 失败。正常 gui 阶段由 Flutter 处理下载和 Pods 安装顺序。

本机启动检查使用：

```bash
open -n "$PWD/flutter/build/macos/Build/Products/Release/RustDesk.app" --args --no-server
```

`--no-server` 避免在没有现成服务时启动新的被控服务；它不隔离官方客户端的配置或已有 IPC 服务。用进程的完整可执行文件路径确认运行的是本地构建产物。

## 基线验收

- [x] 原生依赖、桥接生成、Rust 编译和 Flutter GUI 构建均成功。
- [x] 本地构建产物能启动，无缺失动态库或启动崩溃。
- [ ] 使用官方被控端完成可见的人工远控、键鼠、截图观察及终端验证。
- [ ] 记录测试设备、实际工具版本、命令、产物和失败限制。

本阶段新增构建入口和记录，未修改既有远控源代码；pubspec.lock 对齐 SDK 改变了部分共享 Dart 依赖的解析版本，其影响需由 GUI 编译和运行验收覆盖。子模块初始化保持固定 gitlink，不引入上游提交变化。

## 执行记录：2026-09-15

- 本机：macOS 26.6.2 / ARM64，Xcode 27.0（27A266a），macOS 27.0 SDK；Xcode 许可及首次初始化完成，构建脚本 check 阶段通过。
- 原生依赖：vcpkg manifest 安装成功；抽查 libyuv、Opus、FFmpeg 静态库均含 ARM64 架构。
- 工具：Flutter 3.24.5 / Dart 3.5.4、Rust 1.81.0、CocoaPods 1.17.0、CMake 4.4.3、Ninja 1.13.2、NASM 2.16.03；桥接生成器和 cargo-expand 使用上表版本。
- Dart 依赖：修正锁文件后，离线 `pub get --enforce-lockfile` 通过；Git 依赖提交全部保持原值。
- 桥接：Cargo 依赖补齐后生成成功，Rust、Dart 和 macOS C 头文件均已输出；ffigen 报告 SDK 头文件的 nullability 警告，未报告编译错误。
- Rust：固定 1.81.0 工具链的 release 编译成功，耗时 6 分 59 秒；有既有未使用代码等警告，无编译错误。
- GUI：Flutter release 构建成功；主程序、Rust 动态库和 service 均为 ARM64，主程序与 Rust 动态库的实际最低系统版本均为 12.3。CocoaPods 安装成功，插件版本未变；不保留仅由 CocoaPods 工具版本产生的锁文件尾注差异。
- 打包：复制 service 后的首次签名校验发现新增文件未纳入密封资源；构建脚本补齐本地签名后，`codesign --verify --deep --strict` 通过。
- 启动：本地产物以 `--no-server` 启动，进程路径确认来自工作区；观察到正常主界面与连接入口，原已安装客户端进程仍在运行。未授予额外屏幕录制权限。
- 两次远控实测见下节；已覆盖基础终端 GUI 流程，多屏、重连、异常认证及 MCP 桥接行为仍未验收。

下载中出现过 TLS 中断，按相同 URL/提交重试并复用缓存；不因此更新锁定版本。详细日志保存在本机 `build/macos-arm64/logs/`，不纳入版本控制。

### 首次远控实测：Windows 7 测试设备

用户完成连接与认证后，在本地构建客户端中观察到远端桌面。通过本地产物进程的已建立 TCP 连接进一步核对会话所在进程；未记录设备密码。被控端的准确版本和来源尚未核验，不能据此认定官方被控端兼容验收全部通过。

| 验证项 | 结果与范围 |
| --- | --- |
| 连接和画面 | 已观察到可见桌面、开始菜单和记事本随操作更新；认证由用户完成，未测错误密码。 |
| 鼠标 | 开始菜单点击、原始尺寸及适应窗口下的点击正常；适应窗口下拖动记事本后，窗口位置按预期改变。 |
| 键盘 | 自动化普通字母、数字、回车、退格可用；用户以实体键盘确认大写字母、数字及标点全部正常。 |
| 自动化输入差异 | 当前电脑操作工具的 typeText / Shift 组合输入未完整保留大写及部分标点，原因未定位。不能将此现象判定为 RustDesk 人工键盘故障，也不能据此验收未来 MCP 输入。 |
| 最小化与恢复 | 通过原生窗口菜单恢复后可见更新后的远端画面；未验证最小化期间的桥接帧读取，当前尚无桥接实现。 |
| 终端 | GUI 终端入口能打开独立窗口；连接返回 `Remote terminal is not supported by the remote side`，确认后返回桌面会话。未创建可用 Shell，不能验收终端输入输出、尺寸调整和退出事件。 |
| 多显示器 | 当前没有多屏切换入口，未覆盖多屏、负坐标及不同屏幕缩放。 |
| 其他 | 滚轮、断线重连、权限拒绝和认证失败待测。 |

固定源码在 Windows 上通过 portable-pty 的 ConPTY 能力检测决定是否公布终端支持（src/server/connection.rs）；当前依赖要求 Windows 10 October 2018 或更新系统。该 Windows 7 设备报告不支持终端，符合此限制。后续需要支持终端的被控端与多屏环境补验。

测试只在新建、未保存的记事本文档中输入文本，没有写入文件或改变远端系统配置。收尾时电脑操作工具两次返回 `noWindowsAvailable`，尽管 RustDesk 进程仍在且可读取远控窗口；临时记事本未关闭，留给用户处理。此工具定位问题不记为客户端崩溃。会话显示方式由原始尺寸切为适应窗口。

### 第二次远控实测：支持终端的 Windows 测试设备

用户换机并完成桌面连接后，从会话工具栏打开“终端 (beta)”，出现可见 PowerShell 窗口。设备名标识为 Windows 10；具体系统构建号与被控端版本、来源尚未核验。

| 验证项 | 结果与范围 |
| --- | --- |
| 打开与输入输出 | 第一个终端出现 PowerShell 提示符；echo 回显及后续提示符正常，命令错误的中文输出可显示。 |
| 尺寸同步 | mode con 初次返回 167 列 × 45 行；拖动缩小本机终端窗口后返回 126 列 × 35 行，确认尺寸已传到远端。 |
| 多终端 | 使用加号创建第二个终端，两个终端分别有独立标签和提示符。 |
| Shell 隔离 | 第一终端执行 sv rdtest first 并用 gv rdtest 读到 first；第二终端执行 gv rdtest 返回变量不存在。 |
| Shell 主动退出 | 第二终端执行 exit 7 后自动关闭标签；第一终端仍能读取 rdtest，未受影响。GUI 自动关闭太快，未直接核验退出码数值 7；不能声称退出码传递已验收。 |
| GUI 主动关闭 | 关闭剩余终端窗口后返回仍在连接的桌面会话；没有检查远端进程树，不能据此证明所有子进程均已回收。 |
| 输出历史 | 用滚轮能回看首条回显及此前尺寸查询结果；未做缓存容量或截断压力测试。 |
| 精确文本 | 电脑操作工具输入仍有标点丢失、额外空格和粘贴无效果的问题；精确字节输入、中文输入及 MCP 路径待独立验证。 |

测试未执行文件或系统配置修改命令，仅查询控制台状态、回显、设置 Shell 临时变量及关闭测试终端。两份测试终端已关闭，桌面连接保留；显示方式切为适应窗口。

返回桌面后观察到两个差异：远端曾短暂出现含 QueryFullProcessImageNameW 字样的错误框，未取得完整正文，不能归因于终端关闭；电脑操作工具截图中出现黑块，切换缩放和刷新画面后仍有残留，但用户明确确认人工观看的画面正常。黑块记录为工具截图与人工观看的差异，原因尚未定位，不判定为 RustDesk 显示故障。这些截图不能作为未来桥接帧导出的验收证据。

## 桥接开发：会话观察与 CPU 帧缓存

本节记录第一阶段（`df637a04d`）；后续的布局隔离与 PNG 导出进展见下一节。

新增 `automation` Cargo feature，仅在 macOS 下编译桥接模块，默认关闭。它依赖 Flutter，但不启动 HTTP 服务、不提供 AI 写入，也没有新的 GUI 控件。运行中的既有客户端不会因源码编译自动获得这些能力。

### 已接入的内部路径

- 按实际核心会话的共享连接状态分配独立 `session_id`；同核心的多个 GUI UUID 归到同一记录，最后一个视图关闭后移除记录并拒绝迟到回调。远端 ID 与本地 ID 不混用。
- IO loop 持有固定连接代次的观察句柄；重连、认证成功/失败、权限、显示器布局和断线事件更新桥接快照。桌面认证完成后仍等待有效首帧；终端就绪由认证与终端能力决定。未知权限保留为未知。
- 状态修订与帧修订使用独立 watch 通道；连续视频帧不持续唤醒状态查询。订阅后再读快照，避免等待时丢失更新。
- 仅显式启用内部帧订阅后复制 CPU 像素，按会话与显示器保存；像素使用独立内存，携带格式、stride、连接/布局代次、帧序号、单调时间及光标是否已嵌入等原始元数据。读取不会消费 GUI 缓冲。
- 每帧上限 128 MiB，总像素上限 512 MiB；缓存按最近读取时间淘汰，仍被读取者持有的帧继续计入预算。无法分配时返回容量错误，原 GUI 渲染继续。
- 断线后允许读取同代次旧缓存并明确标为陈旧，重连开始时清空缓存；没有比较游标时，新旧比较值为未知。超过 2 秒的缓存也标为陈旧。

帧钩子实际放在 `Remote::new_video_thread` 的解码回调中，紧邻并先于 `FlutterHandler::on_rgba` 调用，因此同样覆盖软渲染和 CPU 像素上传纹理两条 GUI 路径。选择该位置是为了捕获对应 IO loop 的连接代次，无需修改共享 `InvokeUiSession` trait；旧解码线程不能把帧交给新连接的桥接缓存。

### 本阶段边界

这只是内部观察基础，尚不能按首版截图或会话工具验收：

- 未接入 agent 绑定、session_ref、批准与接管、发送队列门控、可见会话创建请求、终端原始字节缓存或 MCP 服务。
- 认证挑战的完整分类、人工确认事件、连接错误详情和 GUI 可见性仍待补齐；不能从当前观察状态推导 AI 写入许可。
- 未接入图像编码、截图映射、显示器订阅并集或 GPU-only 到 CPU 路径的切换。纹理独占输出会明确标记 TextureOnly；当前不支持将此类会话绑定成可截图 AI 会话。
- 布局变化会清空已有缓存；帧中的布局修订记录回调接收时的布局。布局变化期间解码队列中旧帧的完整隔离尚待实现，所以当前不向 AI 提供可用于输入的坐标映射。
- 原始像素可能已有远端嵌入光标，当前如实标注；对外截图的统一光标规则需在截图工具上线前实现。
- 还未对运行中的远控会话执行桥接导图验证；此前的人工远控及终端结果只属于官方底座验证。

### 构建与验证

```bash
RUSTDESK_AUTOMATION=1 scripts/build-macos-arm64.sh rust
scripts/build-macos-arm64.sh gui
scripts/build-macos-arm64.sh automation-tests
```

不设置 `RUSTDESK_AUTOMATION=1` 时，rust 阶段继续使用原先四项构建 features。桥接无新增第三方依赖，仍使用 Rust 1.81.0。

- `cargo check --locked`：分别开启 automation 与关闭 automation，均通过。
- `automation::` release 单元测试：11 项全部通过。覆盖 GUI 缓冲交换不影响快照、帧租用计入预算、未读取的视频更新不挤占最近读取的显示器、认证与首帧分离、过期连接回调拒绝、布局缓存失效与断线陈旧标记、终端无首帧就绪、非法像素与纯纹理明确失败、CPU 转纯纹理时清理旧缓存并唤醒读取、分离的状态/帧等待、多 GUI 视图共同生命周期。
- 含 automation 的 Rust release 与 Flutter GUI 构建均通过；GUI 产物约 62.3 MB，打包后的 `codesign --verify --deep --strict` 通过。未重启正在运行的客户端，因此本轮不包含新桥接的实际远控验证。

### 既有路径的回归范围

| 文件 | 必须变更的路径 |
| --- | --- |
| Cargo.toml、src/lib.rs | 声明默认关闭的新 feature 和 macOS 模块入口。 |
| src/flutter.rs | GUI 视图注册、复用及移除时调用观察钩子；不修改渲染实现。 |
| src/client/io_loop.rs | 添加仅新 feature 使用的观察句柄和连接/权限/布局/解码事件薄钩子；普通协议消息与输入发送实现未改写。 |
| scripts/build-macos-arm64.sh | 显式环境变量可启用新 feature，默认构建选择不变。 |

关闭 feature 时所有运行时钩子均不参与编译。新增逻辑位于 src/automation；未变更共享 trait、Session 结构、官方 FFI 签名、Flutter/Dart 代码、被控端协议或子模块。

## 桥接开发：布局隔离与 PNG 导出

### 已实现

- 为每个视频解码线程保留独立的布局修订。官方布局事件改变几何信息后，先清空旧增量帧队列，再向解码消息队列放入布局边界；消费边界时重置解码器并等待关键帧，同时通过官方接口请求刷新。旧回调携带旧修订，不能重新填充新布局的桥接缓存。
- 旧关键帧仍按原队列顺序消费，GUI 回调保持原入口；新的边界处理仅在 macOS automation 构建且存在观察会话时触发。对共享 MediaData 枚举只增加 cfg 限定的内部消息，未改动现有消息结构或回调签名。
- 新增内部异步 `automation::capture::capture`：按实际显示器 ID 读取已启用的帧缓存，导出 PNG，并返回尺寸、远端矩形、连接/布局代次、帧序号、接收时间、年龄、新旧与断线标记。比较游标没有更新时返回 None，不重新包装成新画面。
- 处理 BGRA / RGBA 和行填充；按远端桌面的不透明 RGB 像素编码。默认等比缩小至 1600 × 1600 边界，可设置 1～3840，不放大、不裁剪；PNG 输出超过 8 MiB 时整体失败，不返回截断图像。
- 不合成独立光标，`cursor_composited=false`；原视频已嵌入的光标像素保留并标记。不会读取本机窗口截图。
- 编码在 spawn_blocking 中进行，全局最多两个编码任务；取消调用后，未退出的编码任务仍持有名额。编码结束再次核对截图开关、会话状态及连接/布局代次。
- 提供纯几何的截图像素中心到远端坐标换算，覆盖负原点、缩放和越界拒绝；不代表已取得输入权限，也不直接发送输入。

### 验证与范围

- 关闭 automation 的 `cargo check --locked` 通过。
- 最终 release 测试 19 项全部通过；含 automation 的 Rust release 与 Flutter GUI 构建通过，GUI 产物约 62.3 MB，打包后的 `codesign --verify --deep --strict` 通过。当前运行中的客户端未重启。
- 测试使用独立像素缓冲和模拟会话事件；覆盖 PNG 解码后的颜色、行填充、尺寸边界、超限失败、双屏隔离、负坐标、旧布局回调拒绝，以及断线陈旧状态。尚未通过运行中的真实远端会话导出图片，不能据此验收完整多屏或 GUI 运行时回归。
- 本机固定构建启用 hwcodec、不启用 vram。源码的 VP8 / VP9 / AV1 与 H264 / H265 RAM 解码路径输出 CPU 像素；GUI 后续使用纹理渲染不消耗桥接副本。GPU-only 绑定切换或回读未实现；不能将其他 features 组合中的 TextureOnly 状态视为可截图。
- 仍需接入显示器订阅并集、主屏解析、有界等待和官方刷新策略、agent 绑定的 snapshot_id / 映射过期管理及 MCP image block。当前内部函数不暴露为 MCP 工具，不提供 AI 输入。

### 本轮既有路径的回归范围

| 文件 | 行为变化及必要性 |
| --- | --- |
| src/client.rs | 解码消息循环新增 automation 布局边界分支，负责重置解码器与等待关键帧；必须在解码线程消费消息的位置建立边界，防止缓存清空后旧解码结果回流。 |
| src/client/io_loop.rs | 布局事件同步到各解码线程，必要时请求刷新；解码回调携带线程实际应用的布局修订。新增状态只用于 automation，未改写原有视频消息发送、GUI 渲染或输入路径。 |
| src/automation/sessions.rs | 接收帧时检查布局修订，导图完成时复查会话有效性；用于拒绝过期画面。 |
| src/automation/mod.rs | 注册新增的导图与解码布局模块。 |

本轮无新增依赖、FFI / Dart 变更、被控端协议变更或子模块更新。automation 关闭时仍编译原路径。


## MCP 集成构建与验证

MCP 使用独立的 `mcp` 功能开关，默认构建仍保留官方路径。macOS ARM64 的 MCP 构建使用 Rust 1.97.1、官方 `rmcp =3.3.0` 和 Axum 0.8，复用上面的 Flutter、原生依赖及固定官方提交。

```bash
RUSTDESK_MCP=1 scripts/build-macos-arm64.sh bridge
RUSTDESK_MCP=1 scripts/build-macos-arm64.sh rust
RUSTDESK_MCP=1 scripts/build-macos-arm64.sh gui
```

复现 MCP 构建配置下的两组测试：

```bash
source scripts/build-macos-arm64.sh check
RUSTUP_TOOLCHAIN=1.97.1 cargo test --locked --release --lib --features mcp,hwcodec,unix-file-copy-paste,screencapturekit automation::
RUSTUP_TOOLCHAIN=1.97.1 cargo test --locked --release --lib --features mcp,hwcodec,unix-file-copy-paste,screencapturekit mcp::
```

Rust 1.97.1 的符号剥离会产生未按 8 字节对齐的 Mach-O LINKEDIT 字符串表，被 Xcode 27 链接器拒绝，详见 [Rust 上游问题 #157750](https://github.com/rust-lang/rust/issues/157750)。脚本仅对 MCP 构建中的 RustDesk 包覆盖 `profile.release.package.rustdesk.strip="none"`，不修改其他平台或官方基线的 release 配置。已验证实际动态库字符串表对齐为 0（模 8），最小动态库加载也通过。

最终源代码验证：

- 桥接测试 38 项通过：连接与认证状态分离、帧缓存和多屏几何、重连/布局隔离、控制权代次、终端原始输出及缓存覆盖、实体键映射，以及接管后立即唤醒等待中的输入、释放按键并拒绝旧发送队列；关闭会话仍可在保留期内读取完成记录。
- MCP 测试 5 项通过：严格参数、精确 Bearer 匹配、真实 HTTP 初始化与双客户端隔离、20 个工具输入/输出 schema、Origin/正文限制、并发 operation ID 重试及原始结果重放；工具响应容量耗尽时心跳和取消通知仍可处理。
- Flutter 静态分析无 error；仍有上游既有警告和提示。关闭新功能的官方路径使用 Rust 1.81.0，`cargo check --locked` 通过。
- 最终 Rust release（动态库与程序）和 Flutter GUI 构建通过，产物约 71.5 MB，`codesign --verify --deep --strict` 通过。重启最终产物后，实际 HTTP 初始化、会话列举、设置页 running 状态及 agent 列表均正常。
- 对变更文件、联调脚本与构建日志检查临时密码，匹配数为 0。测试连接已清理，MCP 服务在最终交付时关闭，默认人工批准仍保持开启。

本轮被控端由用户提供，为支持终端的 Windows 10 单屏设备。设备临时密码不保存到文档、脚本或普通日志。真实双显示器验收尚无设备，当前多屏证据仅来自自动化几何与缓存隔离测试。


### MCP 实机联调记录（2026-09-15）

以下结果来自本地 MCP 构建连接用户提供的 Windows 10 单屏测试机。临时密码只用于认证请求，没有保存到测试脚本、本文档或普通日志；测试文本仅输入未保存的记事本，终端使用回显、Shell 临时变量、只读版本查询和显式退出。测试结束已关闭临时记事本且未保存文件。远端只读查询报告 Windows 10.0.19045、RustDesk 1.4.9+67；没有核验其二进制哈希。

| 验证项 | 已观察结果 |
| --- | --- |
| 服务与 agent | GUI 启停对应真实监听；两个 MCP 客户端同时连接，设置页分别显示名称、版本、agent_id、绑定的核心会话与 GUI / 终端实例。 |
| 会话独占 | 第二个 agent 绑定已占用的桌面返回 SESSION_BUSY；仍可独立打开同设备的终端连接。解绑后另一 agent 可绑定，保留 Human 模式；原引用不可读取新绑定。 |
| 可见桌面与图片 | AI 创建可见桌面，返回 authenticated / ready；截图为远端解码 PNG，980 × 606，包含真实帧序号和时间。读取与 GUI 同时显示，未消费 GUI 渲染缓冲。 |
| 输入 | MCP 打开运行窗口及记事本；精确文本 ABC abc 123 . : 和中文显示正确；实体 Shift + A / 释放 Shift + B 得到 Ab。远端中文输入法仍按自己的规则处理实体键。 |
| 缩放坐标 | 490 × 303 截图上的菜单坐标正确映射到 980 × 606 桌面；点击命中预期菜单。 |
| 重试 | 同一 operation_id 重试文本输入不重复发送；终端计数自增重试只得到 RD_COUNTER=1。 |
| 人工接管 | AI 模式 GUI 点击和文字输入被阻挡；人工接管后 AI 写入返回 HUMAN_CONTROL，读取仍可用。保持按住的 Shift 被释放。持续拖动测试中实际点击 GUI 接管，活动批次部分执行后停止，排队批次执行前被拒绝，二者均返回 CONTROL_EXPIRED；释放一个键与一个鼠标按钮，release_error 为 null。 |
| 接管批准 | 主动让出、GUI 批准、旧引用 CONTROL_EXPIRED 均已观察；重复申请保留相同批准 ID 与截止时间，未批准请求 60 秒后 expired；取消可重复调用，GUI 拒绝返回 rejected。终端密码弹窗显示时，提示条上的接管与批准按钮仍可点击。 |
| 最小化 | GUI 状态变为 minimized；最小化后仍收到新的远端帧，时间与帧序号均更新。 |
| 终端 | 可见终端创建、精确输入、原始 ANSI / UTF-8 / base64 输出、调整 PTY 为 100 × 30、第二实例创建与单独关闭均通过；另一个实例保持可用。Shell 执行 exit 7 后，关闭事件实际返回 shell_exit_code=7；此字段不解释为单条命令退出码。 |
| 认证与重连 | 错误密码返回 awaiting_auth / Wrong Password；认证工具提交错误密码返回 AUTH_FAILED，提交当前挑战的正确密码后认证并打开终端。显式断开返回 disconnected / Human，旧终端记录 closed、未知退出码保持 null。重连后临时密码不被复用，返回新的认证挑战和控制引用。 |
| 停止与离开 | MCP 停止后监听关闭、agent 清空、提示条消失，原 GUI 终端仍能执行人工输入 echo afterstop。DELETE 结束 MCP 逻辑会话后也撤销绑定，保留 GUI；无 GET 事件流且不回答 ping 的客户端租约到期后返回 HTTP 404，原会话可重新绑定且保持 Human。启用状态保存后退出并重启，服务自动恢复为 running。 |

联调发现并修复：官方桌面端在认证前只报告权限拒绝，需要在成功 PeerInfo 后应用协议允许默认值并保留显式拒绝；子窗口可见性应使用 desktop_multi_window；拖动延时与排队等待需要被控制权变化唤醒；终端认证弹窗应限制在内容区，不能覆盖可靠的接管入口。这些修复均已使用新版进行上述实机复测。另修复长期有效的当前引用在关闭时立即被淘汰的问题：旧引用保留期从被替换时开始计算，自动化测试覆盖长期引用的退休计时及关闭记录；新创建终端连接关闭后，实机读取确认 Closed、GUI registered=false。

### MCP 集成的既有路径回归范围

本轮检查以最终 diff 为准；新增远控逻辑留在 src/automation，MCP 传输和工具适配留在 src/mcp。既有文件的必要变化如下。

| 文件 | 变化及必要性 |
| --- | --- |
| Cargo.toml / Cargo.lock、src/lib.rs | 增加默认关闭、仅 macOS 使用的 mcp 依赖与模块；SDK 引入共享 async-trait / serde_json 解析版本变化，需同时验证 feature off 编译。子模块 gitlink 不变。 |
| src/flutter.rs | 复用原 Flutter 异步运行器，MCP 构建使用多线程 Tokio 并防止重复启动；主 GUI 注册事件后才恢复持久化 MCP 启用状态。关闭 feature 保留原运行器。 |
| src/client.rs | 新增 cfg 限定的内部消息与临时认证标记；最终登录发送检查控制权，AI 密码不进入官方保存或重连路径。人工认证沿原函数执行。 |
| src/client/io_loop.rs | 在实际发送位置执行输入门控、释放与认证钩子；观察实际权限、认证、终端事件。终端字节复制供桥接缓存，GUI 仍接收原有响应。 |
| src/ui_session_interface.rs | 人工输入进入队列时携带控制代次，重连清理 AI 临时凭据；防止排队旧输入在接管后继续执行。 |
| src/flutter_ffi.rs | 只新增 MCP 设置、控制权、可见会话与终端挂载接口，以及终端打开的薄钩子；原操作函数签名不变。 |
| src/automation/sessions.rs / mod.rs | 扩充已有桥接观察层的认证挑战、权限、终端及关闭记录；回收失去 GUI 核心的状态，避免过期引用存活。 |
| flutter/lib/main.dart、utils/multi_window_manager.dart、models/model.dart | 将内部打开 / 关闭请求送到真实 GUI 窗口，传递不含密码的保留请求 ID；普通会话创建继续走原入口。 |
| flutter/lib/desktop/pages/remote_page.dart / remote_tab_page.dart | 添加会话提示条、只读区域和匹配会话的关闭分支，原画面与工具栏实现保留。 |
| flutter/lib/desktop/pages/terminal_page.dart / terminal_tab_page.dart / terminal_connection_manager.dart | 传递创建请求、确认标签已挂载、显示控制条；MCP 构建的认证弹窗限制在内容区，保证接管按钮可点击。关闭工具只移除对应核心的视图，人工标签关闭流程不改写。 |
| flutter/lib/desktop/pages/desktop_setting_page.dart | 支持 MCP 的构建才新增设置 Tab，设置组件独立。 |
| src/lang/*.rs | 仅追加新界面键；中文提供翻译，其他语言保留空值回退，意大利语条目不改译。 |
| scripts/build-macos-arm64.sh | 显式 MCP 构建选择对应工具链与符号剥离修正；默认仍为 Rust 1.81.0 官方 GUI。 |

真实双屏的屏幕切换、不同原点与缩放组合、显示器热插拔尚未实机验收。当前发布目标仅为本机 macOS ARM64 开发产物；没有验证其他主控平台或正式签名、公证分发。
