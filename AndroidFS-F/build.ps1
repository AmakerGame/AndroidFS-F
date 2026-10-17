# Builds the on-device daemon first, then the Windows driver that embeds it.
# Requires: stable Rust, cargo-ndk, ANDROID_NDK_HOME, the arm64 Android target,
# and Dokan 1.5.x (the library the driver links against).
$ErrorActionPreference = "Stop"

if (-not $env:ANDROID_NDK_HOME) {
    Write-Error "Set ANDROID_NDK_HOME to your Android NDK directory."
}

rustup target add aarch64-linux-android | Out-Host
cargo ndk -t arm64-v8a build --release --bin androidfs_server
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

cargo build --release --bin androidfs_driver
exit $LASTEXITCODE
