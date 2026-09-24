export PATH=/Users/laysath/.cargo/bin:/Users/laysath/Library/Caches/rustdesk-build/nasm-install/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin
export RUSTUP_TOOLCHAIN=1.75.0
export VCPKG_ROOT=/Users/laysath/Library/Caches/rustdesk-build/mobile-20260918/native-simulator
export VCPKG_DEFAULT_TRIPLET=arm64-ios
export VCPKG_TARGET_TRIPLET=arm64-ios
export SDKROOT="$(xcrun --sdk iphonesimulator --show-sdk-path)"
export IPHONEOS_DEPLOYMENT_TARGET=13.0
export LIBCLANG_PATH=/opt/homebrew/opt/llvm/lib
export BINDGEN_EXTRA_CLANG_ARGS="--target=arm64-apple-ios13.0-simulator -isysroot $SDKROOT -isystem $(xcrun clang -print-resource-dir)/include"
export SODIUM_USE_PKG_CONFIG=1
export PKG_CONFIG_ALLOW_CROSS=1
export PKG_CONFIG_ALL_STATIC=1
export PKG_CONFIG_LIBDIR_aarch64_apple_ios_sim=/Users/laysath/Library/Caches/rustdesk-build/mobile-20260918/libsodium-simulator/lib/pkgconfig
export PKG_CONFIG_LIBDIR_aarch64_apple_darwin=/Users/laysath/Library/Caches/rustdesk-build/macos-baseline-5278fcab-20260918/source/target/release/build/libsodium-sys-c4ce05bbedd9e62a/out/installed/lib/pkgconfig
export CARGO_BUILD_JOBS=2
