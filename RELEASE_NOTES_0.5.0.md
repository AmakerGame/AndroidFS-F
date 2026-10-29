# AndroidFS-F 0.5.0

Mount an Android phone, tablet or TV box as a Windows drive over ADB, with or without root.

## New
- **Network devices:** enter `ip` or `ip:port` and press Connect. Addresses are remembered and reconnected automatically.
- **Read-only mode,** globally or per device.
- **Per-device settings** (Devices tab > *Access for this device*): access mode, drive root, drive letter, drive name, hide dot-files, removable drive.
- **Stable drive letters:** each device keeps its letter between runs.
- **SD card / USB drive** as the drive root (`SD / USB` button).
- **Disk settings:** hide dot-files, show as removable drive, open Explorer after mounting, folder refresh interval.
- **File dates are preserved** when copying to the phone.
- **Remount** button to apply changed settings; **Open adb shell**; **Copy diagnostics** in the Log tab.
- Clear hints for *unauthorized* and *offline* devices.

## Fixed
- Windows refused to copy when the drive root was `/` or `/storage` (free space came from the root file system).
- Explorer froze forever when the phone stopped answering; requests now time out after 90 s.
- A panic in one request could break the whole drive.
- Failed mounts were retried every 15 s forever; retries now back off up to 5 minutes.
- Unreachable saved network hosts delayed mounting.
- The log file grew without limit; it now rolls over at 2 MB.

## Requirements
Windows 7 SP1 or newer (64-bit), Dokan 1.5.x (installed by the installer), `adb.exe`, Android with USB debugging, `arm64-v8a` or `armeabi-v7a`.

## Downloads
- `androidfs_f_installer.exe`: installer (includes Dokan)
- `androidfs_driver.exe`: portable build (Dokan must be installed)

## Known limitations
x86 / x86_64 devices are not supported. Without root, access is limited to what the ADB `shell` user may read. Security descriptors and alternate data streams are not supported. Changed settings apply after Remount.
