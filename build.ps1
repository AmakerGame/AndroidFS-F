# Builds the on-device daemons (arm64 + armv7) first, then the Windows app that embeds them.
# The Windows app targets x86_64-win7-windows-msvc (runs on Windows 7 SP1 and newer),
# which needs a nightly toolchain with rust-src.
# Requires: nightly Rust (rustup toolchain install nightly --component rust-src), cargo-ndk, ANDROID_NDK_HOME, the Android targets,
# and Dokan 1.5.x (the library the driver links against).
$ErrorActionPreference = "Stop"

if (-not $env:ANDROID_NDK_HOME) {
    Write-Error "Set ANDROID_NDK_HOME to your Android NDK directory."
}

rustup target add aarch64-linux-android armv7-linux-androideabi | Out-Host
cargo +nightly ndk -t arm64-v8a -t armeabi-v7a build --release --bin androidfs_server
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

cargo +nightly build --release --bin androidfs_driver --target x86_64-win7-windows-msvc -Zbuild-std
exit $LASTEXITCODE
