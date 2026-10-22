use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use winapi::um::winbase::CREATE_NO_WINDOW;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub auto_mount: bool,
    /// Android directory exposed as the drive root. Empty = auto-detect.
    pub root: String,
    /// Custom adb.exe path. Empty = auto-detect.
    pub adb_path: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            auto_mount: true,
            root: String::new(),
            adb_path: String::new(),
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
        match key.trim() {
            "auto_mount" => settings.auto_mount = value != "false",
            "root" => settings.root = value.to_string(),
            "adb_path" => settings.adb_path = value.to_string(),
            _ => {}
        }
    }
    settings
}

pub fn save(settings: &Settings) -> std::io::Result<()> {
    std::fs::create_dir_all(data_dir())?;
    let text = format!(
        "auto_mount={}\nroot={}\nadb_path={}\n",
        settings.auto_mount,
        settings.root.trim(),
        settings.adb_path.trim()
    );
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
