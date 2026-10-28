use std::collections::BTreeMap;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use winapi::um::winbase::CREATE_NO_WINDOW;

use crate::daemon_cmd::AccessMode;

/// Overrides for one device; `None` means "use the global setting".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceSetting {
    pub access_mode: Option<AccessMode>,
    pub root: Option<String>,
    pub read_only: Option<bool>,
    /// Preferred drive letter such as `R:`; remembered after the first mount.
    pub letter: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub auto_mount: bool,
    /// Android directory exposed as the drive root. Empty = auto-detect.
    pub root: String,
    /// Custom adb.exe path. Empty = auto-detect.
    pub adb_path: String,
    /// Auto: shell first, root only when needed. Shell: never root. Root: always `su`.
    pub access_mode: AccessMode,
    /// Per-device overrides, keyed by adb serial.
    pub per_device: BTreeMap<String, DeviceSetting>,
    /// Protect the phone: Windows may not write, delete or rename anything.
    pub read_only: bool,
    /// Network devices (`ip:port`) reconnected automatically with `adb connect`.
    pub hosts: Vec<String>,
}

impl Settings {
    /// Access mode, drive root and read-only flag that apply to this device.
    pub fn effective(&self, serial: &str) -> Effective {
        let over = self.per_device.get(serial);
        Effective {
            access_mode: over.and_then(|d| d.access_mode).unwrap_or(self.access_mode),
            root: over.and_then(|d| d.root.clone()).unwrap_or_else(|| self.root.clone()),
            read_only: over.and_then(|d| d.read_only).unwrap_or(self.read_only),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Effective {
    pub access_mode: AccessMode,
    pub root: String,
    pub read_only: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            auto_mount: true,
            root: String::new(),
            adb_path: String::new(),
            access_mode: AccessMode::Auto,
            per_device: BTreeMap::new(),
            read_only: false,
            hosts: Vec::new(),
        }
    }
}

pub fn data_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("AndroidFS-F")
}

pub fn settings_path() -> PathBuf {
    data_dir().join("settings.ini")
}

pub fn log_path() -> PathBuf {
    data_dir().join("androidfs.log")
}

pub fn load() -> Settings {
    let mut settings = Settings::default();
    let Ok(text) = std::fs::read_to_string(settings_path()) else {
        return settings;
    };
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        let key = key.trim();
        if let Some(rest) = key.strip_prefix("device.") {
            if let Some((serial, field)) = rest.rsplit_once('.') {
                let entry = settings.per_device.entry(serial.to_string()).or_default();
                match field {
                    "access" => entry.access_mode = Some(AccessMode::parse(value)),
                    "root" => entry.root = Some(value.to_string()),
                    "readonly" => entry.read_only = Some(value == "true"),
                    "letter" => entry.letter = Some(value.trim().to_string()),
                    _ => {}
                }
            }
            continue;
        }
        match key {
            "auto_mount" => settings.auto_mount = value != "false",
            "root" => settings.root = value.to_string(),
            "adb_path" => settings.adb_path = value.to_string(),
            "access_mode" => settings.access_mode = AccessMode::parse(value),
            "read_only" => settings.read_only = value == "true",
            "host" => {
                if let Some(host) = crate::daemon_cmd::normalize_adb_host(value) {
                    if !settings.hosts.contains(&host) {
                        settings.hosts.push(host);
                    }
                }
            }
            _ => {}
        }
    }
    settings
}

pub fn save(settings: &Settings) -> std::io::Result<()> {
    std::fs::create_dir_all(data_dir())?;
    let mut text = format!(
        "auto_mount={}\nroot={}\nadb_path={}\naccess_mode={}\n",
        settings.auto_mount,
        settings.root.trim(),
        settings.adb_path.trim(),
        settings.access_mode.as_str()
    );
    text.push_str(&format!("read_only={}\n", settings.read_only));
    for host in &settings.hosts {
        text.push_str(&format!("host={host}\n"));
    }
    for (serial, device) in &settings.per_device {
        if let Some(letter) = &device.letter {
            text.push_str(&format!("device.{serial}.letter={letter}\n"));
        }
        if let Some(read_only) = device.read_only {
            text.push_str(&format!("device.{serial}.readonly={read_only}\n"));
        }
        if let Some(mode) = device.access_mode {
            text.push_str(&format!("device.{serial}.access={}\n", mode.as_str()));
        }
        if let Some(root) = &device.root {
            text.push_str(&format!("device.{serial}.root={}\n", root.trim()));
        }
    }
    std::fs::write(settings_path(), text)
}

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "AndroidFS-F";

pub fn autostart_enabled() -> bool {
    Command::new("reg")
        .creation_flags(CREATE_NO_WINDOW)
        .args(["query", RUN_KEY, "/v", RUN_VALUE])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

pub fn set_autostart(enable: bool) -> Result<(), String> {
    let mut cmd = Command::new("reg");
    cmd.creation_flags(CREATE_NO_WINDOW);
    if enable {
        let exe = std::env::current_exe().map_err(|err| err.to_string())?;
        let value = format!("\"{}\" --background", exe.display());
        cmd.args(["add", RUN_KEY, "/v", RUN_VALUE, "/t", "REG_SZ", "/d"])
            .arg(value)
            .arg("/f");
    } else {
        cmd.args(["delete", RUN_KEY, "/v", RUN_VALUE, "/f"]);
    }
    match cmd.output() {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
        Err(err) => Err(err.to_string()),
    }
}
