# AndroidFS-F

**AndroidFS-F** is a fork of the original [AndroidFS](https://github.com/Lauriethefish/AndroidFS) project. It mounts an Android device as a drive in Windows Explorer using ADB.

A connected phone or tablet appears in "My Computer" as a separate volume (default drive letter `Q:`), and you work with its files just like local ones — without MTP or separate file manager programs.

> **Status:** This project is in an alpha state. It is recommended for experienced users.

---

## 📦 Prerequisites

Before using AndroidFS-F, make sure you have the following software installed:

### 1. Dokan Library
A driver that allows creating virtual drives in Windows. **Required component** — AndroidFS-F will not work without it.

- Download: [Dokan Releases](https://github.com/dokan-dev/dokany/releases)
- Recommended version: **1.5.1** (the one used for testing the original)

### 2. ADB (Android Debug Bridge)
A tool for communicating with an Android device. Required for the driver to work.

- Download: [Android Platform Tools](https://developer.android.com/tools/releases/platform-tools)
- After downloading, add the folder containing `adb.exe` to your `PATH` environment variable.

### 3. USB drivers for your device
For ADB to work correctly on Windows, you may need official USB drivers from your device manufacturer (Samsung, Xiaomi, Google, etc.).

---

## 🚀 Installation

1. Go to the [**Releases**](../../releases) page of this repository.
2. Download the latest installer.
3. Run the installer and follow the prompts.
4. **Restart your computer** — this is required to register the driver.

---

## 📱 Usage

1. Enable **USB debugging** on your Android device:
   - Settings → About phone → tap "Build number" 7 times
   - Settings → Developer options → enable "USB debugging"
2. Connect the device to your computer via USB.
3. Confirm the debugging authorization prompt on your phone screen.
4. Open "Explorer" — the device will appear as drive `Q:`.

Devices are automatically added and removed when connected or disconnected.

### Debugging (if needed)

To enable the console window for diagnostics, set the following environment variables:

```
ANDROIDFS_CONSOLE=1
RUST_LOG=DEBUG
```

A restart is required for the change to take effect.

---

## ⚙️ How it works

AndroidFS-F uses a "server" executable that is automatically pushed to the device upon connection. The driver on the PC communicates with it via sockets and `adb forward` — this avoids having to pull files to temporary locations each time they are opened or edited.

For fast operation, file stats and directory listings are cached — this significantly speeds up navigation, especially since Windows often makes multiple requests to the same directory when opening a folder in Explorer.

---

## ⚠️ Known limitations

- Performance is limited by USB debugging bandwidth.
- Some features may vary depending on the Android version and device manufacturer.

---

## 🛠 Compilation from source

If you want to build AndroidFS-F yourself:

1. Install [Dokan](https://github.com/dokan-dev/dokany/releases/tag/v1.5.1.1000) (v1.5).
2. Install [rustup](https://rustup.rs/).
3. Run `rustup install nightly`.
4. Add the Android target:
   ```
   rustup target add aarch64-linux-android
   ```
5. Download the [Android NDK](https://developer.android.com/ndk/downloads) and set `ANDROID_NDK_HOME` to its path.
6. Run:
   ```
   ./build.ps1
   ```

---

## 📄 License

This project is distributed under the **GNU General Public License v3.0 (GPLv3)**.

AndroidFS-F is a fork of the original [AndroidFS](https://github.com/Lauriethefish/AndroidFS) by Lauriethefish. Because the original project is licensed under GPLv3, this fork also remains under GPLv3. See the [`LICENSE`](LICENSE) file for the full license text.
