<p align="center">
  <img src="assets/icon.png" alt="AndroidFS-F" width="128" height="128">
</p>

<h1 align="center">AndroidFS-F</h1>

<p align="center">
  Mount an Android phone as a regular Windows drive over ADB. No MTP, no extra apps on the phone.
</p>

<p align="center">
  <a href="https://github.com/AmakerGame/AndroidFS-F/actions/workflows/installer.yml"><img alt="Build" src="https://github.com/AmakerGame/AndroidFS-F/actions/workflows/installer.yml/badge.svg"></a>
  <img alt="Version" src="https://img.shields.io/badge/version-0.3.0-3DDC84">
  <img alt="Platform" src="https://img.shields.io/badge/platform-Windows%20%7C%20Android-informational">
  <img alt="License" src="https://img.shields.io/badge/license-GPL--3.0-blue">
</p>

<p align="center">
  English · <a href="README.uk.md">Українська</a>
</p>

---

## Overview

AndroidFS-F is a fork of [AndroidFS](https://github.com/Lauriethefish/AndroidFS). It runs a small daemon on the phone and exposes the phone's storage to Windows through the [Dokan](https://dokan-dev.github.io/) user-mode file system driver. The phone then appears in Explorer with its own drive letter and works with any Windows program.

Version 0.3 adds a desktop application with a device list and controls, support for 32-bit ARM phones, and a set of stability fixes.

## Features

- **Desktop application.** A window with the list of connected devices, status, and one-click **Mount**, **Unmount** and **Open** actions.
- **Automatic mounting.** Plug a phone in and its drive appears; unplug it and the drive is removed cleanly.
- **Wide device support.** The embedded daemon is built for both `arm64-v8a` and `armeabi-v7a`, and the right one is chosen from the phone's CPU.
- **Full read/write access.** Create, copy, rename, move and delete files and folders; large writes are streamed in chunks and failures are reported instead of dropping the drive.
- **Built-in log viewer** and a log file for troubleshooting.
- **Start with Windows**, minimized, from the Settings tab or the installer.
- **Single self-contained installer** that also installs Dokan.

## Requirements

| Component | Requirement |
|-----------|-------------|
| Windows | 10 or 11, 64-bit |
| Driver | [Dokan 1.5.x](https://github.com/dokan-dev/dokany/releases/tag/v1.5.1.1000) (installed by the installer) |
| ADB | `adb.exe` from [Android platform-tools](https://developer.android.com/tools/releases/platform-tools), on `PATH` or set in Settings |
| Phone | Android with **USB debugging** enabled, `arm64-v8a` or `armeabi-v7a` |
| Graphics | OpenGL 2.0 capable GPU or driver (required by the window) |

## Installation

1. Download `androidfs_f_installer.exe` from the latest successful run on the [Actions](https://github.com/AmakerGame/AndroidFS-F/actions) page (artifact **installer**).
2. Run the installer and choose whether to start with Windows. Restart if asked, so that the Dokan driver loads.
3. Make sure `adb.exe` is available (see Requirements).

A portable `androidfs_driver.exe` is published as the **standalone** artifact. It still needs Dokan installed.

## Usage

1. On the phone: open *Settings > About phone*, tap *Build number* seven times, then enable *USB debugging* in *Developer options*.
2. Connect the phone by USB and allow the computer on the phone screen.
3. Start **AndroidFS-F**. The phone appears on the **Devices** tab and is mounted automatically as the first free drive letter, starting at `Q:`.
4. Use **Open** to browse the drive in Explorer, and **Unmount** before unplugging when you want to be safe.

Closing the window unmounts all drives and exits the application.

## Configuration

Settings are available on the **Settings** tab and are stored in `%APPDATA%\AndroidFS-F\settings.ini`.

| Setting | Description | Default |
|---------|-------------|---------|
| Mount automatically | Mount devices as soon as they are detected | On |
| Start with Windows | Launch minimized at sign-in | Off |
| ADB path | Full path to `adb.exe` | Auto-detect |
| Drive root | Android directory shown as the drive | `/storage/emulated/0` |

Auto-detection of `adb.exe` checks `ANDROIDFS_ADB`, `ANDROID_HOME`, `ANDROID_SDK_ROOT`, `%LOCALAPPDATA%\Android\Sdk\platform-tools` and `PATH`. The `ANDROIDFS_ROOT` environment variable is still honoured when no drive root is set in the application.

Command-line options for `androidfs_driver.exe`:

| Option | Effect |
|--------|--------|
| `--background` | Start minimized (used by autostart) |

## Building from source

Prerequisites: stable Rust, [cargo-ndk](https://github.com/bbqsrc/cargo-ndk), the Android NDK (`ANDROID_NDK_HOME` set), Dokan 1.5.x with its development files, and the Windows SDK.

```powershell
rustup target add aarch64-linux-android armv7-linux-androideabi
cargo install cargo-ndk
./build.ps1
```

The script first builds the Android daemon for both ABIs, then builds the Windows application that embeds them with `include_bytes!`. The result is `target/release/androidfs_driver.exe`.

The daemon logic and the wire protocol can be tested without a phone or Windows:

```
cargo test --lib
```

Continuous integration (`.github/workflows/installer.yml`) runs these tests on Linux, builds both daemons, builds the Windows application and compiles the installer with Inno Setup.

## How it works

```
 Windows                                        Android
┌──────────────────────────┐                  ┌───────────────────┐
│ Explorer / any program   │                  │ androidfs_server  │
│          │               │   adb forward    │ (arm64 or armv7)  │
│  Dokan drive (Q:)        │◄────────────────►│ file operations   │
│          │               │   TCP over USB   │ on shared storage │
│  androidfs_driver.exe    │                  └───────────────────┘
└──────────────────────────┘
```

On connection the application pushes the matching daemon to `/data/local/tmp`, starts it as the `shell` user, forwards a local TCP port to it through ADB, and serves Dokan file system calls by exchanging length-prefixed `bincode` messages with the daemon.

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| "ADB is not available" | Install platform-tools and add `adb.exe` to `PATH`, or set its path in Settings |
| Phone not listed | Enable USB debugging, reconnect the cable and accept the key prompt on the phone |
| Status *Failed* with a Dokan message | Install Dokan 1.5.x and restart Windows |
| Window does not open | Update the graphics driver; the window needs OpenGL 2.0 |
| "Already running" message | Another instance is open; find it in the taskbar |

Open the **Log** tab, or `%APPDATA%\AndroidFS-F\androidfs.log`, for details.

## Limitations

- Throughput is bounded by USB debugging, and file access rights are those of the `shell` user.
- Only `arm64-v8a` and `armeabi-v7a` phones are supported.
- File security descriptors and alternate data streams are not supported.

## Changelog

**0.3.0**

- New desktop window with device list, mount / unmount / open actions, settings and log viewer.
- Support for `armeabi-v7a` (32-bit ARM) phones next to `arm64-v8a`.
- Application icon, English documentation, installer options for autostart and a desktop shortcut.
- Settings and log moved to `%APPDATA%\AndroidFS-F`.

**0.2.1**

- Fixed the Windows build (daemon modules are Unix-only) and the GitHub Actions pipeline.
- Write failures such as a full disk are reported to Windows instead of dropping the connection.
- Symbolic links to directories are shown as directories.

**0.2.0**

- Implemented file and folder creation, writing, truncating and safe deletion in the daemon.
- Fixed a panic on reads past the end of a file and wrong status codes for missing files.
- The drive root is the phone's shared storage; ADB failures no longer stop the application.

## License

Released under the GNU General Public License v3.0. See [LICENSE](LICENSE). Based on [AndroidFS](https://github.com/Lauriethefish/AndroidFS) by Lauriethefish; the original readme is kept in [READMEoriginal.md](READMEoriginal.md).
