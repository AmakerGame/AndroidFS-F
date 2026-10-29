# Changelog

## 0.5.0

### Added
- Network devices: `Network device` field runs `adb connect`; addresses are remembered and reconnected in the background.
- Read-only mode (global default or per device).
- Per-device settings: access mode, drive root, drive letter, drive name, hide dot-files, removable drive, read-only.
- Stable drive letters: a device keeps its letter between runs.
- SD card / USB drive as drive root (`@external`).
- Disk settings: hide dot-files, show as removable drive, open Explorer after mounting, folder refresh interval.
- Original file dates are kept when copying to the phone.
- Remount button, Open adb shell, Copy diagnostics.
- Hints for devices adb lists as unauthorized or offline.

### Fixed
- Free space was taken from the root file system when the drive root was `/` or `/storage`, so Windows refused to copy.
- Explorer could freeze forever when the phone stopped answering; requests now time out after 90 s and a lost connection is detected immediately.
- A panic in one request no longer breaks the whole drive.
- Failed mounts were retried every 15 s forever; retries now back off up to 5 minutes.
- Unreachable saved network hosts no longer delay mounting.
- The log file no longer grows without limit (rolls over at 2 MB).
- Drive letters could change between runs.

## 0.4.0

First release.

- Desktop application with device list, Mount / Unmount / Open, Settings, Log, About.
- Root and non-root devices (Automatic / Without root / Root) with `su -c`, `su 0`, `su root` and `adb root` support.
- Fixed a drive that mounted but looked empty in Explorer (the host connected before the daemon was listening; a token handshake is now required).
- Folders no longer carry cloud-placeholder attributes; one over-long name no longer aborts a listing; unreadable entries are still listed.
- Wider shared-storage detection, `adb forward --no-rebind`, post-mount self-check in the log.
