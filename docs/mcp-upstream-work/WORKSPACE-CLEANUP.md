# 旧分支工作区残留清理

日期：2026-09-18。范围仅为 `/Users/laysath/Projects/rustdesk` 从 `codex/mcp-controller` 切换至 `codex/mcp-upstream` 后遗留的 ignored 生成文件和构建输出。未实现产品功能、未运行完整构建、未提交 Git。

## 结果

- 已将 28 个明确列出的文件或目录移至仓库外归档，合计约 **11.0 GiB**（`du -sk` 合计 11,512,332 KiB）。使用同一文件系统的 `os.rename`，没有复制大型产物或删除数据；因此释放了主工作区路径，但没有释放磁盘容量。
- 五份现存旧桥接已移出；六个预期桥接路径目前全部不存在，等待按当前主线重新生成。清理不代表构建通过。
- 旧 `build/macos-arm64/logs` 和 `validation` 整体保留，84 个历史证据文件移出前后 SHA-256 一致。
- 所有受追踪文件移出前后 SHA-256 相同；主仓库 HEAD、子模块提交和参考工作区状态未变。未修改源码、配置、锁文件或既有文档。

归档根目录：

`/Users/laysath/Projects/rustdesk-workspace-archive/20260918-154318/`

原路径与归档路径的映射为：`<主工作区>/<相对路径>` → `<归档根>/files/<相对路径>`。精确移出清单、各路径 ignore 规则与容量见 [manifest.json](/Users/laysath/Projects/rustdesk-workspace-archive/20260918-154318/manifest.json)。

## 清理范围与归属证据

每个移出项均通过 `git ls-files -- <path>` 确认没有受追踪文件，并通过 `git check-ignore -v <path>` 确认忽略规则。目录递归检查不含 `.git` 元数据；遍历不跟随符号链接。Flutter 插件注册文件和配置内容带生成文件标记；版本文件包含旧构建日期 `2026-09-16 14:44`。旧桥接实际含 automation/MCP API。

| 类别 | 原相对路径 |
| --- | --- |
| Rust 构建缓存及旧版本生成文件 | `target/`、`src/version.rs` |
| 历史构建与验证证据 | `build/`，内部仅有 `macos-arm64/logs/`、`macos-arm64/validation/` |
| Flutter/Dart 缓存和插件清单 | `flutter/build/`、`flutter/.dart_tool/`、`flutter/.flutter-plugins`、`flutter/.flutter-plugins-dependencies` |
| Rust/Dart/macOS 旧桥接 | `src/bridge_generated.rs`、`src/bridge_generated.io.rs`、`flutter/lib/generated_bridge.dart`、`flutter/lib/generated_bridge.freezed.dart`、`flutter/macos/Runner/bridge_generated.h` |
| Android 注册生成文件 | `flutter/android/app/src/main/java/io/flutter/plugins/GeneratedPluginRegistrant.java`，仅移出该文件，保留其父目录 |
| iOS 生成文件 | `flutter/ios/Flutter/Generated.xcconfig`、`flutter/ios/Flutter/flutter_export_environment.sh`、`flutter/ios/Runner/GeneratedPluginRegistrant.h`、`flutter/ios/Runner/GeneratedPluginRegistrant.m` |
| macOS 工程缓存 | `flutter/macos/Pods/`、`flutter/macos/Flutter/ephemeral/`、`flutter/macos/Flutter/GeneratedPluginRegistrant.swift` |
| Linux/Windows 工程缓存 | 各自 `flutter/<平台>/flutter/ephemeral/`、`generated_plugin_registrant.cc`、`generated_plugin_registrant.h`、`generated_plugins.cmake` |

现存五份桥接与构建代理之前保存在 `/Users/laysath/Library/Caches/rustdesk-build/macos-baseline-5278fcab-20260918/old-generated-backup/` 下的同路径备份逐一 SHA-256 相同。`flutter/ios/Runner/bridge_generated.h` 在本次清理开始时和已有备份中均不存在；此前“六份备份”的描述应以本次核对的五份为准。详见 [桥接核对记录](/Users/laysath/Projects/rustdesk-workspace-archive/20260918-154318/bridge-backup-verification.json)。

## 并行构建及保留范围

开始前已与 `macos_baseline_build` 确认：其源码、vcpkg worktree、构建和日志均在 `/Users/laysath/Library/Caches/rustdesk-build/macos-baseline-5278fcab-20260918/`，不使用主目录 ignored 产物，也不写回主目录桥接；该证据根完整保留。主目录 `lsof` 盘点未发现被移出路径的打开文件。未停止任何编译或应用进程，也未操作系统安装的 RustDesk。

以下项目未清理：

- 参考工作区 `/Users/laysath/.codex/worktrees/rustdesk-mcp-reference/rustdesk`：参考分支、HEAD、子模块和 Git 状态前后相同。
- 所有 Git 元数据、`libs/hbb_common` 内容与提交；主仓库全部受追踪文件，包括 `flutter/pubspec.lock`、macOS/iOS 的 `Podfile.lock`。
- `flutter/android/local.properties`：本地 SDK 路径配置，不属于旧业务接口或构建输出；仍指向保留的共享 Flutter SDK。这是清理后唯一剩余的 ignored 文件。
- `/Users/laysath/Library/Caches/rustdesk-build`、`~/.cargo`、`~/.pub-cache` 等共享工具链缓存，以及其他仓库外实验、认证文件、用户文件和研究报告。
- 构建代理的 `docs/mcp-upstream-work/MACOS-BASELINE-BUILD.md`；本子任务只新增本报告。

268 个符号链接随所属目录原样移动，没有跟随链接清理共享缓存；原本存在的仓库外目标仍存在。部分旧构建链接使用主目录的绝对路径，移出后会失效，这是归档产物的预期状态，不能把归档当作可直接运行的构建目录。符号链接清单见 [symlinks.json](/Users/laysath/Projects/rustdesk-workspace-archive/20260918-154318/symlinks.json)。

## 验证与回归面

- [before.json](/Users/laysath/Projects/rustdesk-workspace-archive/20260918-154318/before.json)：受追踪文件哈希、原 Git 状态、主仓库及参考工作区提交、保护目录信息。
- [verification.json](/Users/laysath/Projects/rustdesk-workspace-archive/20260918-154318/verification.json)：受追踪文件变化列表为空；主仓库、子模块、参考工作区与保护目录检查均通过；六个桥接生成路径均不存在。
- [historical-evidence-sha256.json](/Users/laysath/Projects/rustdesk-workspace-archive/20260918-154318/historical-evidence-sha256.json)：84 个历史日志、截图、实验文件的哈希。
- [cleanup.py](/Users/laysath/Projects/rustdesk-workspace-archive/20260918-154318/cleanup.py)：精确白名单与移出/验证逻辑，供审计；不能直接重复执行已完成的移动。

清理前的预检曾因 Android 父目录本身不匹配忽略规则而停止，未移动任何文件；改为只移动被明确忽略的 `GeneratedPluginRegistrant.java`。移动后的初次链接验证将指向旧主目录的绝对链接误当成应继续可用的链接，修正为检查链接内容不变及共享外部目标存在后全部通过。以上过程未丢失文件或修改共享目标。

回归面仅为上述 ignored 生成和构建路径：后续构建必须重新解析 Flutter 依赖、生成桥接/插件注册及平台 ephemeral 文件，并重新构建 Rust/Flutter 产物，避免复用旧分支接口。已有产品源码和运行时逻辑没有改变。本次未运行应用构建或功能测试；当前主线构建结果由构建代理独立记录。

清理已完成，主目录可以写入按当前主线重新生成的文件。没有需要用户处理的遗留项；归档保留供查证，不自动删除。
