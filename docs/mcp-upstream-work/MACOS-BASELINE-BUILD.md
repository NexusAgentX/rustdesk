# macOS ARM64 原版主线构建基线

更新：2026-09-18。任务 A01 / 验证矩阵 B03：**通过**。主线原版 macOS ARM64 的 Rust Release、Flutter Release 与完整应用构建成功，CI 光标尺寸测试 16 项通过。没有执行应用启动或远控运行测试；不代表运行矩阵 E01 已验收。

## 源码与隔离

- 产品基线：`5278fcab685723099e9950d56cb605807db5aa2a`，hbb_common：`0eb175963c950cba8d2f4f2a480f49db27b42c39`。
- 工作目录：`/Users/laysath/Library/Caches/rustdesk-build/macos-baseline-5278fcab-20260918/source`，由上述提交 `git archive` 创建全新快照，不复用旧 Rust/Flutter 构建输出。
- 证据根：`/Users/laysath/Library/Caches/rustdesk-build/macos-baseline-5278fcab-20260918`；环境变量脚本为 `env.sh`，日志目录为 `logs/`。
- 主工作区五个现存旧分支 bridge 生成文件先复制到证据根的 `old-generated-backup/`；原 iOS header 不存在，未产生虚假备份；基线在独立源码快照生成，不依赖这些旧接口。
- 主工作区产品源码、用户已有修改和全局 Rust 默认版本不变；不触碰已安装 RustDesk 或已有远控会话。

## 工具与配方

- 本机 macOS 26.6.2 (25G83)、ARM64，Xcode 27.0 (27A266a)、SDK 27.0。
- Rust/Cargo 1.81.0，Flutter 3.24.5 (`dec2ee5c1f`)、Dart 3.5.4。
- flutter_rust_bridge_codegen 1.80.1、cargo-expand 1.0.95、NASM 2.16.03、CMake 4.4.3、CocoaPods 1.17.0、pkg-config 3.0.7。
- 沿用 `.github/workflows/flutter-build.yml` ARM64 的 `hwcodec,flutter,unix-file-copy-paste,screencapturekit`、部署目标 12.3；构建不进行发布签名。
- 主线 bridge CI 使用 Linux / Rust 1.75 / Flutter 3.22.3；本机使用相同 codegen 和 cargo-expand，在 macOS / Rust 1.81 / Flutter 3.24.5 生成；桥接、Rust 与 Dart 实际编译均通过。未声称与 Linux CI 的桥接文件逐字节相同。

## 执行记录

1. 原有 vcpkg 仓库为 shallow clone，HEAD `120deac3062162151622ca4860575a33844ba10b`，缺少 manifest 指定的 `9e593bb18ea69cc5095e012465dcd675a822ed0d`。已 fetch 精确 commit，并在证据根 `vcpkg/` 创建固定 commit 的独立 worktree、bootstrap 对应工具 2026-07-27，使用独立 installed/buildtrees；此为环境/缓存问题，不修改产品 manifest。
2. 已保存环境日志 `logs/environment.log`，`flutter pub get` 已通过；16 项锁定依赖由 Flutter 3.24.5 解析调整，差异保存为 `logs/flutter-lock-resolution.diff`，未改主工作区锁文件。
3. 继承的 `RUST_LOG` 值触发 codegen 1.80.1 仅接受 debug/info 的断言；重试显式设 `RUST_LOG=info`，属于本机环境。补齐 Cargo git 依赖后桥接生成通过；按 CI 复制 macOS header 至 iOS，现有六份新桥接，SHA-256 见 `logs/generated-sha256.txt`。生成结果不含 automation/MCP 接口。
4. 首次 cursor test 在桥接生成完成前触发，因 `generated_bridge.dart` 尚不存在失败（`logs/flutter-cursor-size-test.log`）；这是执行顺序问题，不计作源码缺陷。桥接完成后重跑 **16 项测试全部通过**，日志 `logs/flutter-cursor-size-test-after-bridge.log`。

5. 为并行编译通用 Rust 依赖，在 vcpkg 尚未安装完成时启动首次 Release。它停在 `hwcodec` / `magnum-opus` 缺少尚未安装的 FFmpeg / Opus 头文件，日志 `logs/build-attempt-1.log`；这是依赖准备顺序导致的预期失败，vcpkg 完成后重试，不算上游源码问题。Cargo.lock 与主线一致。

6. FFmpeg 7.1.1 的 vcpkg 下载在 8.5 MiB 停滞，使用 `curl -L --connect-timeout 15 --max-time 180 --retry 2 https://codeload.github.com/FFmpeg/FFmpeg/tar.gz/refs/tags/n7.1.1` 下载同一归档。SHA-512 与 overlay 要求的 `6b9a5ee501be41d6abc7579a106263b31f787321cbc45dedee97abf992bf8236cdb2394571dd256a74154f4a20018d429ae7e7f0409611ddc4d6f529d924d175` 一致；只终止本任务卡住的 x-download 子进程，将已验证归档放入下载缓存后重跑。证据 `logs/ffmpeg-download-retry.log`、`logs/vcpkg-install-retry.log`。

7. 固定 vcpkg baseline 的全部 12 项依赖安装通过，最后重试阶段耗时 1.4 分钟；版本清单 `logs/vcpkg-installed.txt`。随后执行依赖齐备的第二次完整 Release 构建，日志 `logs/build-attempt-2.log`。

8. 第二次构建的 **Rust 完整 Release 通过**（3m02s）。新 dylib 为 ARM64，`LC_BUILD_VERSION` 为 minos 12.3 / SDK 27.0，见 `logs/rust-dylib-inspection.txt`。随后 Flutter/Xcode 失败：Pods 的原始部署目标 10.13/10.14 低于本机 Xcode 27 支持下限 12.0。主线 CI 使用 macos-14，不是同一 Xcode 环境；用进程级 `FLUTTER_XCODE_MACOSX_DEPLOYMENT_TARGET=12.3` 让全部 Xcode targets 沿用主线 ARM64 最低版本，第三次完整构建 **退出码 0，Rust + Flutter 应用通过**（`logs/build-attempt-3.log`）。此调整没有改 Pod/产品代码。

## 当前复现命令

在证据根执行（该目录已保存原始源码 tar、部署目标调整前四个配置文件及独立 vcpkg worktree）：

```sh
source ./env.sh
python3 "$BASELINE_ROOT/apply-ci-config.py"
cd "$BASELINE_ROOT/source/flutter"
flutter pub get
cd ..
RUST_LOG=info flutter_rust_bridge_codegen \
  --rust-input ./src/flutter_ffi.rs \
  --dart-output ./flutter/lib/generated_bridge.dart \
  --c-output ./flutter/macos/Runner/bridge_generated.h
cp ./flutter/macos/Runner/bridge_generated.h ./flutter/ios/Runner/bridge_generated.h
"$VCPKG_ROOT/vcpkg" install --triplet arm64-osx \
  --x-install-root="$VCPKG_ROOT/installed"
cd flutter
flutter test --timeout 60s test/cursor_size_test.dart
cd ..
python3 build.py --flutter --hwcodec --unix-file-copy-paste --screencapturekit
```

构建入口另存为证据根 `run-build.sh`。四个临时配置调整与 CI 一致：`build.py`、`Cargo.toml`、`flutter/macos/Podfile`、`flutter/macos/Runner.xcodeproj/project.pbxproj` 的最低 macOS 版本改为 12.3。SDK 中已有的 dropdown-menu/filter 和 scheduler 补丁已核对与主线 CI 要求相符，另存 `logs/flutter-sdk-patches.diff`。`env.sh` 包含 Xcode 27 所需的 `FLUTTER_XCODE_MACOSX_DEPLOYMENT_TARGET=12.3`。

构建结束后已恢复隔离快照的四个 CI 配置文件、`flutter/pubspec.lock` 和 `flutter/macos/Podfile.lock`；按原始两个 git archive 逐文件校验，所有受追踪源码恢复一致。实际构建使用的六个文件另存 `build-inputs/`，差异为 `logs/build-inputs.diff`，恢复核验为 `logs/snapshot-changes-restored.txt`。重新构建应先运行上面的 `apply-ci-config.py`，或者使用会自动执行它的 `run-build.sh`。

## 产物与证据

- 完整应用：`/Users/laysath/Library/Caches/rustdesk-build/macos-baseline-5278fcab-20260918/source/flutter/build/macos/Build/Products/Release/RustDesk.app`。
- Rust 输出：同一 `source/target/release/` 下的 `liblibrustdesk.dylib`、`rustdesk`、`service`；CI 入口已将 `service` 复制进 app。
- 应用版本 1.5.0、build 68。主 executable、服务和核心 dylib 全部为 ARM64；应用 plist 和 Mach-O 的最低版本为 12.3，SDK 27.0。
- Xcode 默认生成 ad-hoc 签名，`TeamIdentifier=not set`；这里的 unsigned 指没有开发者身份签名/公证。没有发布、安装到 `/Applications`、启动应用或更改系统安全策略。
- `logs/app-inspection.txt`：包结构、版本、架构、部署目标、签名类别及链接库核对。
- `logs/app-manifest.json`：应用所有文件的 SHA-256、大小及符号链接目标；核心文件摘要另存 `logs/app-key-sha256.json`。
- 核心 dylib SHA-256：`a2e641c3f46ff4e5348e1ab9729b01670c55dd0f24dcdeb7809a43700fcd9b9e`。
- 全部构建日志、归档、工具链脚本、生成文件与产物均保留在证据根；没有依赖主工作区旧缓存或参考分支产物。

## 结论与限制

未发现阻止本次原版构建的上游产品源码缺陷。已解决的是旧/shallow vcpkg 缓存、下载停滞、继承日志变量、步骤准备顺序，以及本机 Xcode 27 对旧 Pods 部署目标的约束。主线 CI 最低目标 12.3 保持不变；没有放宽或升级产品依赖来绕过错误。现有弃用/未使用警告未作为本任务顺带修改。

下一步可使用这份应用开展 E01 的 GUI 与原有远控行为基线验证；本报告不把构建通过或单元测试通过当作远控运行通过。

## 回归面

本任务在主工作区只新增本报告；没有修改任何受追踪产品文件或既有运行路径。独立快照的临时构建配置和依赖锁文件均已恢复。共享缓存只新增精确依赖与下载，不修改默认工具链。未写回主目录的 ignored 桥接/产物；旧文件后续清理由独立任务负责。
