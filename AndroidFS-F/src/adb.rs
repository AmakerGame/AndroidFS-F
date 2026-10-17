use std::fmt::Display;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Output};

use log::trace;
use winapi::um::winbase::CREATE_NO_WINDOW;

use crate::adb_parse;

#[derive(Clone, Debug)]
pub struct DebugBridge {
    pub adb_path: String,
}

#[derive(Clone, Debug)]
pub struct Device {
    debug_bridge: DebugBridge,
    pub serial_number: String,
}

#[derive(Clone, Debug)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
}

#[derive(Clone, Debug)]
pub enum ErrorKind {
    LaunchFailed,
    NonSuccessExitCode,
    ParseFailure,
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

pub trait Invokeable {
    fn invoke(&self, command: Vec<String>) -> std::result::Result<Output, Error>;

    fn invoke_result(&self, command: Vec<String>) -> std::result::Result<Output, Error> {
        let result = self.invoke(command)?;
        if result.status.success() {
            Ok(result)
        } else {
            let stdout = String::from_utf8_lossy(&result.stdout);
            let stderr = String::from_utf8_lossy(&result.stderr);
            Err(Error {
                kind: ErrorKind::NonSuccessExitCode,
                message: format!("adb exited with {}: {stdout} {stderr}", result.status),
            })
        }
    }

    fn invoke_shell_command_result(
        &self,
        mut command: Vec<String>,
    ) -> std::result::Result<Output, Error> {
        command.insert(0, "shell".to_string());
        self.invoke_result(command)
    }
}

impl Invokeable for Device {
    fn invoke(&self, mut command: Vec<String>) -> std::result::Result<Output, Error> {
        command.insert(0, "-s".to_string());
        command.insert(1, self.serial_number.clone());
        self.debug_bridge.invoke(command)
    }
}

impl DebugBridge {
    pub fn locate() -> Self {
        DebugBridge {
            adb_path: locate_adb_path(),
        }
    }

    pub fn get_devices(&self) -> std::result::Result<Vec<Device>, Error> {
        let output = self.invoke_result(vec!["devices".to_string()])?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(adb_parse::parse_device_serials(&stdout)
            .into_iter()
            .map(|serial_number| Device {
                debug_bridge: self.clone(),
                serial_number,
            })
            .collect())
    }
}

impl Invokeable for DebugBridge {
    fn invoke(&self, command: Vec<String>) -> std::result::Result<Output, Error> {
        trace!("Invoking ADB ({}): {:?}", self.adb_path, command);
        match Command::new(&self.adb_path)
            .creation_flags(CREATE_NO_WINDOW)
            .args(&command)
            .output()
        {
            Ok(output) => Ok(output),
            Err(err) => Err(Error {
                kind: ErrorKind::LaunchFailed,
                message: format!(
                    "could not run adb at \"{}\": {err}. Install Android platform-tools and add adb.exe to PATH, or set ANDROIDFS_ADB.",
                    self.adb_path
                ),
            }),
        }
    }
}

fn locate_adb_path() -> String {
    if let Some(explicit) = std::env::var_os("ANDROIDFS_ADB") {
        return PathBuf::from(explicit).to_string_lossy().into_owned();
    }

    let mut candidates = Vec::new();
    for var in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Some(value) = std::env::var_os(var) {
            candidates.push(
                PathBuf::from(value)
                    .join("platform-tools")
                    .join("adb.exe"),
            );
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(
            PathBuf::from(local)
                .join("Android")
                .join("Sdk")
                .join("platform-tools")
                .join("adb.exe"),
        );
    }
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        candidates.push(
            PathBuf::from(profile)
                .join("AppData")
                .join("Local")
                .join("Android")
                .join("Sdk")
                .join("platform-tools")
                .join("adb.exe"),
        );
    }
    candidates.push(PathBuf::from(r"C:\Android\platform-tools\adb.exe"));
    candidates.push(PathBuf::from(r"C:\platform-tools\adb.exe"));

    for candidate in candidates {
        if candidate.is_file() {
            return candidate.to_string_lossy().into_owned();
        }
    }
    "adb.exe".to_string()
}
