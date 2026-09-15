#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
build_cache="${RUSTDESK_BUILD_CACHE:-$HOME/Library/Caches/rustdesk-build}"
export FLUTTER_ROOT="${FLUTTER_ROOT:-$build_cache/flutter-3.24.5}"
export VCPKG_ROOT="${VCPKG_ROOT:-$build_cache/vcpkg}"
export PATH="$build_cache/rust-tools/bin:$build_cache/nasm-install/bin:$FLUTTER_ROOT/bin:$HOME/.cargo/bin:$PATH"
export RUSTUP_TOOLCHAIN="${RUSTDESK_RUST_TOOLCHAIN:-1.81.0}"
export MACOSX_DEPLOYMENT_TARGET=12.3
export VCPKG_DEFAULT_TRIPLET=arm64-osx
export VCPKG_DEFAULT_HOST_TRIPLET=arm64-osx
export VCPKG_MAX_CONCURRENCY="${VCPKG_MAX_CONCURRENCY:-8}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-8}"
export COCOAPODS_DISABLE_STATS=true
export CI=true
export LIBCLANG_PATH="${LIBCLANG_PATH:-$(dirname "$(xcrun --find clang)")/../lib}"

cd "$repo_root"
if [[ "$(uname -s)" != Darwin || "$(uname -m)" != arm64 ]]; then
    echo "This build entry point requires macOS ARM64." >&2
    exit 1
fi

check() {
    git submodule status
    rustc --version
    cargo --version
    flutter --version
    pod --version
    "$VCPKG_ROOT/vcpkg" version
    nasm --version
    xcodebuild -version
}

deps() {
    "$VCPKG_ROOT/vcpkg" install --triplet arm64-osx \
        --x-install-root="$VCPKG_ROOT/installed"
    (cd flutter && flutter pub get --enforce-lockfile)
}

bridge() {
    RUST_LOG=info flutter_rust_bridge_codegen \
        --skip-add-mod-to-lib \
        --rust-input src/flutter_ffi.rs \
        --dart-output flutter/lib/generated_bridge.dart \
        --c-output flutter/macos/Runner/bridge_generated.h
}

rust() {
    cargo build --locked --release \
        --features flutter,hwcodec,unix-file-copy-paste,screencapturekit
    cp target/release/liblibrustdesk.dylib target/release/librustdesk.dylib
}

gui() {
    xcodebuild -version
    test -f target/release/liblibrustdesk.dylib
    test -f target/release/service
    (
        cd flutter
        FLUTTER_XCODE_ARCHS=arm64 \
        FLUTTER_XCODE_ONLY_ACTIVE_ARCH=YES \
        FLUTTER_XCODE_MACOSX_DEPLOYMENT_TARGET=12.3 \
            flutter build macos --release --no-pub
    )
    cp target/release/service \
        flutter/build/macos/Build/Products/Release/RustDesk.app/Contents/MacOS/
    local app=flutter/build/macos/Build/Products/Release/RustDesk.app
    codesign --force --sign - "$app/Contents/MacOS/service"
    codesign --force --sign - --preserve-metadata=entitlements "$app"
    codesign --verify --deep --strict "$app"
}

stage="${1:-check}"
case "$stage" in
    check|deps|bridge|rust|gui) "$stage" ;;
    all) check; deps; bridge; rust; gui ;;
    *) echo "Usage: $0 {check|deps|bridge|rust|gui|all}" >&2; exit 2 ;;
esac
