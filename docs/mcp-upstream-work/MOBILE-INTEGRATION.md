# 移动端实际构建与 MCP 集成

更新：2026-09-18。进行中；不是移动平台验收完成报告。

最新状态：三套含MCP完整Rust原生库均用Rust1.75构建成功；iOS模拟器完整应用已链接、安装并实际运行。iOS26.2上原有AppDelegate路径（无Scene适配）已通过两官方MCP客户端及应用挂起/恢复/请求释放实测；iOS27需要通用Scene迁移，隔离实验通过但未入产品分支。Android ARM64 APK已构建成功，正在模拟器安装/运行。真实设备和远控业务/67能力验收仍未完成。

## 范围与源码

本工作负责尽早打通 Android/iOS 实际构建、应用运行、嵌入 MCP 协议服务和生命周期验证。五平台单 PR、67 项能力范围不变；协议 fixture 验证不等于业务工具实现。

- 产品目录：`/Users/laysath/Projects/rustdesk`，`codex/mcp-upstream`。
- 原版基线：`5278fcab685723099e9950d56cb605807db5aa2a`，`hbb_common` 为 `0eb1759`。
- 隔离源码及证据根：`/Users/laysath/Library/Caches/rustdesk-build/mobile-20260918`，源码在 `source/`，日志在 `logs/`。
- 原版源码复制自 macOS 构建任务的干净快照，不复制 target/build/Pods/cache；六份桥接来自该任务用当前主线重新生成的输出，哈希见其 `logs/generated-sha256.txt`。未复用已归档旧参考分支桥接。

## 工具链与正在执行的准备

- 宿主 ARM64 Mac，24 GiB。与 macOS 原版构建任务协调，本阶段先下载配置，不同时进行大规模 Rust/vcpkg 编译。
- Xcode 27.0 (`27A266a`)，iphoneos27.0/iphonesimulator27.0 SDK 已安装；`xcrun simctl list runtimes` 初始为空。
- `xcodebuild -downloadPlatform iOS` 已实际启动官方 iOS 27.0 (`24A434`) ARM64 runtime 下载（8.05 GB），日志 `logs/ios-runtime-download.log`。下载完成不等于应用运行通过。
- Flutter 3.24.5：`/Users/laysath/Library/Caches/rustdesk-build/flutter-3.24.5`，沿用主线 CI Flutter 补丁。
- 按主线移动 CI 使用 Rust 1.75.0，正在安装 `aarch64-apple-ios`、`aarch64-apple-ios-sim`、`aarch64-linux-android`；日志 `logs/rust-targets-install.log`。
- Android 在 PATH、`~/Library/Android`、Applications、Homebrew、Spotlight 索引中均未发现可用 SDK/Java；通过 Google 官方 repository XML 获取下载版本。安装目录 `~/Library/Android/sdk`。
- Android 配方：NDK r28c (`28.2.13676358`)，compile/target SDK 36，应用最低 API22，cargo-ndk 编译平台 API21。主线 ABI 为 ARM64/armv7/x86_64，优先 ARM64 构建及模拟器；真机 ABI 待设备接入确认。
- Homebrew 安装 OpenJDK17 因 portable-ruby 下载失败，转官方 Adoptium JDK17 ARM64 压缩包，保留失败日志；未变更系统 Java 默认配置。

## 已核实的工程路径

- `flutter/ios/Runner.xcodeproj/project.pbxproj` 的库路径直接指定 `target/aarch64-apple-ios/release/liblibrustdesk.a`，部分配置 `SUPPORTED_PLATFORMS = iphoneos`。需要按 SDK 选择独立模拟器 Rust 产物，同时保持真机路径。
- 现有 `flutter/ios_arm64.sh` 仅编译真机，`ios_x64.sh` 仅编译 Intel 模拟器。ARM64 模拟器需新增独立构建入口。
- `libs/scrap/build.rs` 根据 arch+os 推导 `arm64-ios`，不能直接区分同架构真机与模拟器；先用隔离的 `VCPKG_INSTALLED_ROOT` 与相同 triplet 名验证，避免为新模拟器路径改写所有既有平台。
- FFmpeg overlay 的 iOS 分支固定 `-mios-version-min=8.0 -fembed-bitcode`，需在真实模拟器编译中确认影响，再限定修正范围。

## 协议及生命周期接口协调

协议代理拥有 `src/mcp/`、Cargo、`src/lib.rs`，移动任务不重复实现协议。

约定接口：`ServerConfig::local(Implementation { name, version })`，`Credentials::new(capacity)`，`issue/revoke`，`Server::bind(config, credentials, tools).await`，`local_addr()`，`run(self, CancellationToken).await`。调用方提供已有 Tokio runtime，不隐藏创建 runtime。停服取消监听、请求、连接；挂起取消，恢复重新 bind/run。真实业务授权及输入释放尚需接入，不能用空钩子宣称完成。

协议 ready 前先完成原版构建；随后通过真实协议层验证发现/初始化、tools/list/tools/call、凭据、客户端可达性、停止/恢复/连接释放。测试专用工具只证明协议路径。

## 当前结果与缺口

初始阶段尚无完整目标构建；后续结果见下方阶段证据及页首最新状态。尚无移动应用/MCP实际运行通过证据。

iOS 无真机，已授权使用模拟器及真机目标编译。设备后台挂起、内存回收、设备网络、签名安装和同机 AI 客户端体验需单列真机补测，最后由用户请求社区支持。Android 手机尚未连接，SDK准备完成后提供精确 USB 调试与授权步骤；当前不以设备未接入阻塞构建。

## 主分支改动与回归面

初始阶段仅新增本记录；后续产品变更及回归面逐项记录在下方“已接入主分支的最小移动生命周期钩子”及后续构建变更。隔离快照实验不改变主分支运行路径。

## 阶段证据：原生模拟器构建与运行时（2026-09-18）

- Rust1.75 的 iOS 真机、ARM64模拟器、Android ARM64目标已安装完成；Google commandline-tools 23.0已安装。
- 为避免与 macOS任务争夺Flutter全局锁，移动任务使用仓库外克隆的 `mobile-20260918/flutter-3.24.5`。首次Flutter iOS构建实际执行但仍在下载iOS artifacts，原先共享SDK下载进程主动结束，不影响macOS任务。
- vcpkg `arm64-ios-simulator` 全部基线依赖安装成功：`logs/ios-vcpkg-build-01.log`。但实际对象检查及clang链接探针证实FFmpeg和VPX仍输出真机对象，错误为 `building for iOS-simulator, but linking ... built for iOS`，详见 `logs/ffmpeg-simulator-link-probe.log`。不能将vcpkg绿色退出码当模拟器可用。
- 隔离快照仅修正FFmpeg模拟器编译选项，并给VPX增加仅模拟器应用的configure补丁；重新完整构建成功，`logs/ios-vcpkg-build-02.log`。两库对象已显示 `LC_BUILD_VERSION / IOSSIMULATOR`，相同链接探针通过，`logs/ffmpeg-simulator-link-probe-fixed.log`。设备路径仍保留原参数。产品分支尚未回写这些修正，待完整链接继续确认。
- Xcode条件库路径隔离修改已用 `xcodebuild -showBuildSettings` 分别确认模拟器选择 `aarch64-apple-ios-sim`、真机保留 `aarch64-apple-ios`；证据 `logs/ios-simulator-settings.log`、`logs/ios-device-settings.log`。
- 完整Rust模拟器构建实际执行 `cargo +1.75.0 build --locked --features flutter,hwcodec --release --target aarch64-apple-ios-sim --lib`。在 `libsodium-sys 0.2.7` build.rs失败：`Unknown iOS build target: aarch64-apple-ios-sim`。正使用该crate已有的 `SODIUM_LIB_DIR` 接入独立simulator libsodium，未升级Rust或依赖。日志 `logs/ios-rust-build-01.log`。
- 主代理批准修改 `src/flutter.rs` 的MCP feature路径：改用容量1 Tokio channel异步等待，同样使用既有current_thread runtime；非MCP及桌面仍保留std channel和阻塞接收。增加生命周期runtime注册钩子，未创建运行时。
- 新增 `src/mcp/runtime.rs`，由现有Handle驱动服务监督器，默认关闭，显式enable才监听，挂起同步取消请求/连接，清理完成后恢复重新绑定；凭据与Server复用协议模块。尚未接真实业务控制权及按键释放，不声称已有业务安全验收。
- 3项Rust1.75宿主runtime测试已通过，覆盖真实HTTP tools/list、显式开启/挂起/恢复/关闭、空闲连接释放、drop关闭监听、绑定失败及重试。`logs/mcp-runtime-tests-02.log`。第一轮测试把macOS关闭连接的ConnectionReset误当失败，已接受EOF或ConnectionReset并保留监听端口不可重连断言。这里只是宿主协议/生命周期证据，不是Android/iOS运行通过。

## 阶段证据：完整模拟器 Rust 库（2026-09-18）

- **完整原版Rust库已编译/链接成功**：`logs/ios-rust-build-04.log`，Rust1.75，`flutter,hwcodec`，`aarch64-apple-ios-sim`。产物 `liblibrustdesk-baseline-simulator.a`（约175MiB），SHA256见 `logs/ios-baseline-rust-sha256.txt`。这尚不是Flutter应用链接或模拟器运行通过。
- 重现环境已保存 `env-ios-simulator.sh`。实际命令：在隔离 `source/` 中 source该脚本后 `cargo build --locked --features flutter,hwcodec --release --target aarch64-apple-ios-sim --lib`。
- libsodium最终使用crate内置1.0.18源码，不升级加密依赖。通过`SODIUM_USE_PKG_CONFIG=1`及分别设置宿主/目标的`PKG_CONFIG_LIBDIR_<target>`解决build.rs也依赖hbb_common导致的宿主混链。先前全局`SODIUM_LIB_DIR`方案失败于macOS build-script混入simulator对象，保留 `logs/ios-rust-build-03.log`。
- bindgen旧版直接把Rust `aarch64-apple-ios-sim` 传给Clang，报invalid target；使用明确Clang `arm64-apple-ios13.0-simulator`、SDK/sysroot及clang resource include环境参数解决，不修改依赖源码。
- `vcpkg libsodium 1.0.22`尝试因缺autotools失败后弃用；转回crate自带源码1.0.18实际交叉编译成功。失败日志 `logs/ios-libsodium-build.log`，成功日志 `logs/ios-libsodium-bundled-{configure,build}.log`。
- 正在构建同快照的`flutter,hwcodec,mcp`模拟器Rust库，日志 `logs/ios-rust-mcp-build.log`。此时复制主分支明确文件及重新生成桥接，不复制旧参考桥接。
- 真机native目标`arm64-ios`已独立启动，安装位置 `vcpkg/installed-device`、日志 `logs/ios-device-vcpkg-build.log`，不与simulator库混用。
- runtime测试现4项全通过，增加真实`tools/call`等待fixture，确认挂起状态公布前handler已析构且监听关闭：`logs/mcp-runtime-tests-04.log`。fixture仅用于协议生命周期，不算67项能力。
- 使用真实`src/flutter.rs::async_tasks`模块提取的隔离harness（只有业务online查询为stub）验证：online查询等待及队列满时，真实MCP initialize仍HTTP200；容量1队列保留，第三个try_send报满；stop后监听关闭。Rust1.75通过：`logs/async-runner-tests-02.log`；feature-off原代码路径编译通过：`logs/async-runner-feature-off-check.log`。宿主测试不能替代移动运行。
- Android platform-tools37.0.1已安装，`adb devices -l`为空。手机接入步骤：数据线连接Mac，开启开发者选项/USB调试，保持解锁并接受本机RSA授权弹窗；不需要账号、root或更改系统安全策略。JDK17、NDKr28c继续下载。NDK改为官方URL分段下载并校验Google repository记录的SHA1，以缩短单连接慢下载；原始日志保留。

## 已接入主分支的最小移动生命周期钩子

- `src/flutter.rs`：仅MCP + Android/iOS使用异步容量1 channel，注册服务到原有runtime；无MCP保留原接收路径。必要性是原有同步recv会饿死同线程监听服务。
- `src/flutter_ffi.rs`：新增同步 `main_mcp_set_foreground` 薄接口，仅MCP+Android/iOS调用生命周期服务。同步返回防止前后台回调在FFI线程池重排；feature-off无行为。
- `flutter/lib/main.dart`：复用现有全局WidgetsBindingObserver，在移动应用初始化、生命周期变化和dispose传递foreground状态。桌面不触发这些新调用；iOS/Android离开resumed立即挂起MCP。
- `src/mcp/runtime.rs`：新功能模块、默认Disabled、显式启用后绑定、挂起/关闭/Drop取消并等待旧请求清理后重启；接收已有Handle，不创建runtime。真实业务授权/按键释放仍待控制权模块接入。
- 六份桥接已在主分支按FRB1.80.1/cargo-expand1.0.95/Rust1.81/Flutter3.24.5重新生成（ignored生成文件），日志 `logs/main-bridge-generation-02.log`。首次失败因环境RUST_LOG不受FRB接受，显式设info后成功。pub get产生的无关pubspec.lock变化已恢复，解析diff仅留证据。
- `flutter analyze lib/main.dart` 仅6项原有deprecated/unused提示，无新增接口错误；日志 `logs/main-dart-analyze.log`。

上述已有路径变更均是接入移动生命周期及复用既有runtime所需，未调整会话/输入/渲染等旧路径，未做无关格式或清理。尚未宣告业务控制权限已可用。

## 阶段证据：真机目标、Android 目标与构建修正

- **iOS真机目标完整Rust库（含MCP）构建成功**：`logs/ios-device-rust-mcp-build.log`，Rust1.75，`flutter,hwcodec,mcp`，`aarch64-apple-ios`，产物 `source/target/aarch64-apple-ios/release/liblibrustdesk.a`（约184MiB）。重现环境 `env-ios-device.sh`。没有iPhone，没有签名/设备运行验收。
- **iOS模拟器目标完整Rust库（含MCP）构建成功**：`logs/ios-rust-mcp-build.log`，同Rust/features，`aarch64-apple-ios-sim`。
- 已把经过实际对象检查/链接探针与原生构建验证的最小修正写回主分支：`flutter/ios/Runner.xcodeproj/project.pbxproj`按SDK选Rust库并允许simulator；`res/vcpkg/ffmpeg/portfile.cmake`只新增simulator分支，真机原分支逐行保留；`res/vcpkg/libvpx/portfile.cmake`只在simulator加载新`arm64-ios-simulator.patch`。这些修改不可避免，因为旧路径生成真机对象并被simulator链接拒绝。其它目标不加载新VPX补丁。
- 新增 `flutter/ios_simulator.sh`，沿用现有ios_arm64.sh形状，编译aarch64-apple-ios-sim并允许附加feature参数。与既有脚本一样，调用前需配置native依赖。当前完整可复现实验环境在 `env-ios-simulator.sh`：独立vcpkg facade映射simulator库到依赖build.rs固定的arm64-ios目录；crypto宿主/目标PKG_CONFIG目录分开；显式Clang simulator triple。后续正式开发文档需保留这些配置，不把单个cargo命令声称为全自动环境安装。
- Java17.0.20.1+1已安装到证据根，不修改系统默认Java。commandline-tools23.0实际附带Android CLI1.0.16261425；旧sdkmanager是其兼容脚本。NDKr28c官方包已校验Google repository的SHA1并解压；clang是包含ARM64的通用Mach-O。cargo-ndk3.1.2按主线版本安装成功。
- Android ARM64原生依赖实际运行主线`./flutter/build_android_deps.sh arm64-v8a`，日志 `logs/android-native-build.log`。随后实际运行 `cargo ndk --platform 21 --target aarch64-linux-android build --locked --release --features flutter,hwcodec,mcp --lib`，日志 `logs/android-rust-mcp-build.log`、环境`env-android.sh`。并非桌面check替代。
- Android API36/build-tools36/emulator/API35 ARM64 Google APIs镜像正用官方CLI安装，日志`logs/android-sdk-install.log`。
- iOS runtime标准xcodebuild下载仍较慢，额外使用本机Apple公开MobileAsset目录的同一官方URL分段下载；按目录SHA256校验后用系统AppleArchive提取，再通过simctl标准签名验证安装。脚本`download-ios-runtime.py`、日志`logs/ios-runtime-ranged-download.log`。未改系统安全策略、未使用第三方runtime。下载/解压尚不等于安装/运行完成。

### 回归面收窄

最小化检查发现桌面多Flutter engine可能重复initialize并替换原有online runner，若在这里绑定桌面MCP服务会造成服务随旧runner退出重置。因此本任务将`src/flutter.rs`的新异步channel与runtime注册严格限制为`feature=mcp && (Android || iOS)`，桌面及feature-off继续旧路径。桌面主engine的稳定runtime/幂等启动由桌面集成另行接入，本移动任务不擅自扩大共享改造。此前宿主实际runner测试验证的是同一异步路径逻辑（收窄前可在宿主激活）；最终移动cfg由真实移动目标编译验证。

## 阶段证据：Android 完整原生库

- **Android ARM64 完整 Rust 共享库（含 MCP）已成功**：`logs/android-rust-mcp-build-02.log`，Rust1.75 + cargo-ndk3.1.2 + NDKr28c，`--platform 21 --target aarch64-linux-android --features flutter,hwcodec,mcp`。`file`确认ELF64 ARM aarch64；动态依赖为标准NDK/系统库及libc++_shared。真实产物`source/target/aarch64-linux-android/release/liblibrustdesk.so`。
- 第一轮失败来自Homebrew libclang23与NDK19资源头混用；环境改为NDK自带libclang和`-resource-dir`后通过，无产品依赖或源码绕过。最终移动专用cfg同步后再次增量编译记录`logs/android-rust-mcp-final-build.log`。
- Android应用打包已开始准备，原版Gradle8.11.1 wrapper下载遇Java SSL握手EOF（`logs/gradle-version-02.log`）；正在用官方Gradle分发包正常下载/校验解决，没有关闭TLS验证。模拟器安装显式指定`mac_arm64`以避开新版Android CLI自身x86_64/Rosetta自动选错宿主架构的问题。
- iOS应用构建仍在Flutter官方iOS artifacts下载阶段；官方runtime约8GB继续下载。上述原生库通过不等于应用链接或模拟器运行通过。

## 阶段证据：模拟器环境与应用打包推进

- 三套最终移动cfg的完整Rust目标均已增量构建通过：`logs/android-rust-mcp-final-build.log`、`logs/ios-rust-mcp-final-build.log`、`logs/ios-device-rust-mcp-final-build.log`。产物与相关源码SHA256在`logs/mobile-final-{native,source}-sha256.txt`。
- iOS27 ARM64 runtime官方8,053,063,680字节asset的SHA256已匹配Apple元数据；原始`aa extract`对该metadata/patch格式返回0但未产出文件，未将其当成功安装。显式`aea decrypt`后以`aa patch`重建出DMG，再执行`xcrun simctl runtime add`；系统报告`Ready`。日志`ios-runtime-{ranged-download-03,decrypt,extract-patch,import}.log`。原先重复的xcodebuild下载已停止。
- 独立`RustDesk MCP validation` iPhone16模拟器已实际boot完成：`logs/ios-simulator-boot.log`；UDID在证据根`ios-simulator-udid.txt`，截图`logs/ios-simulator-booted.png`。这仅证明运行环境，不是RustDesk应用运行验收。
- Flutter3.24.5全部iOS引擎已下载；release包505,313,793字节按Google Storage公开MD5核对后解压，依Flutter缓存规则设置执行权限/许可证/stamp。`logs/flutter-ios-ranged-download.log`，不升级引擎、不伪造缺失cache。真实iOS构建现进入`pod install`：`logs/ios-build-03.log`。
- Gradle8.11.1分发包已通过官方SHA256；仅隔离快照wrapper引用相同版本本地已校验zip，以避开Java下载握手中断。首次完整Gradle实际执行后builder-model7.3.0 jar下载TLS EOF；从同一官方Google Maven下载，核对既有Gradle module的SHA256后填充其内容寻址缓存，再次真实APK构建`logs/android-apk-build-03.log`。尚未产出APK。
- 隔离测试应用使用Android applicationIdSuffix和iOS独立bundle ID `com.carriez.flutterHbb.mcp.validation`、显示名RustDesk MCP Test；这些验证身份改动不写入产品分支，不覆盖正式应用。

## 阶段证据：实际 iOS 应用与 MCP 生命周期

- Xcode27实际完整模拟器应用链接成功：`logs/ios-build-05.log`。本机命令增加`FLUTTER_XCODE_IPHONEOS_DEPLOYMENT_TARGET=15.0`、`FLUTTER_XCODE_ARCHS=arm64`、`FLUTTER_XCODE_ONLY_ACTIVE_ARCH=YES`；前者解决Xcode27拒绝旧部署目标，后两者限定本次ARM64验证。产品部署版本未改。
- 该应用安装到iOS27成功，但**原版AppDelegate工程启动失败**：`Runner-2026-09-18-171810.ips`、`logs/ios-app-system-log.txt`，UIKit明确`UIScene life cycle is required for apps built with this SDK`。触发条件限定为本次**使用iOS27 SDK构建、在iOS27 runtime启动**；不代表旧SDK发布包在iOS27必然失败。[Apple说明](https://developer.apple.com/documentation/uikit/transitioning-to-the-uikit-scene-based-life-cycle)、[Flutter说明](https://docs.flutter.dev/release/breaking-changes/uiscenedelegate)。
- 仅在隔离快照AppDelegate/Info.plist增加Scene适配后，Flutter3.24.5实际启动到RustDesk连接首页：`logs/ios-build-06-scene-experiment.log`、`logs/ios-app-scene-launch.log`、`logs/ios-app-scene.png`。该通用迁移未进入产品分支，插件与其他生命周期尚未全面回归。
- 隔离快照Rust runtime通过编译环境`MCP_MOBILE_VALIDATION=enabled`显式启用测试fixture、固定loopback测试端口；fixture文件`mobile-validation-fixture.rs`没有写入产品分支。主分支默认关闭不变。
- **真实应用进程内协议/生命周期通过（Scene隔离包）**：官方SDK1.30.0完成2025-11-25 initialize/initialized，官方client2.0.0完成2026-07-28 server/discover；均tools/list及fixture tools/call成功。脚本通过simctl切到系统设置，pending tools/call被取消、HTTP监听关闭；返回RustDesk重新发现/调用成功，fixture计数证明旧handler Drop完成。`mobile-protocol-validation.mjs`及`logs/ios-mobile-protocol-lifecycle-01.log`。仅证明协议/生命周期，不计67业务、不证明远控输入释放。
- SDK27/新系统兼容问题保持隔离；标准xcodebuild命令实际找到了iOS26.2 runtime，官方asset SHA256已验证并经simctl导入Ready：`logs/ios26-runtime-{availability,ranged-download,import}.log`。继续测试无Scene改造的同SDK构建在旧runtime能否运行，不预设成功。
- Android已创建ARM64 AVD；Gradle实际走到自动安装多个旧插件所需SDK，platform32归档损坏被解压拒绝：`logs/android-apk-build-05.log`。当前正使用官方SDK管理器重新安装，尚无APK/应用运行成功。

## 阶段证据：无 Scene 适配的兼容对照与 Android APK

- **iOS26.2无Scene适配对照通过**：还原原版AppDelegate/Info.plist，以Xcode27编译的同一Flutter3.24.5工程正常启动。`logs/ios-build-08-no-scene-fixture.log`、`logs/ios26-app-no-scene-launch.log`、`logs/ios26-app-no-scene.png`。这是实测兼容组合；不改变SDK27+iOS27要求的结论。
- 两官方客户端、两协议版本及挂起/恢复清理在该原有生命周期路径再次通过：`logs/ios26-mobile-protocol-lifecycle.log`。不依赖隔离Scene转发生命周期消息。
- 换回与主分支相同的普通MCP原生库，重新应用链接、安装、启动；默认没有测试端口监听：`logs/ios-build-09-default-disabled.log`、`logs/ios26-app-default-disabled.log`、`logs/ios26-default-disabled-check.log`。通过Device Hub UI在远程ID框输入123、清除，并往返设置/连接页，截图`logs/ios26-id-input.png`。没有实际远控目标，未发起连接；不能将页面可操作算完整远控通过。
- **Android ARM64 APK实际完整构建通过**：`logs/android-apk-build-06.log`，产物`rustdesk-android-default-disabled.apk`，含主分支MCP库且默认关闭，隔离应用ID。NDK/rustlib/Flutter/Java/Gradle版本不升级，网络失败通过原官方依赖和校验恢复。随后准备fixture APK及启动ARM64模拟器；此时尚无Android服务运行验收。
