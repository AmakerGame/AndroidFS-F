//! Background engine: finds Android devices through ADB, pushes the daemon,
//! mounts / unmounts Dokan drives and publishes the state the GUI shows.

use std::collections::{HashSet, VecDeque};
use std::io::Write;
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use dokan::{Drive, MountFlags};
use log::{debug, error, info, warn};
use widestring::U16CString;
use winapi::um::minwinbase::SYSTEMTIME;
use winapi::um::sysinfoapi::GetLocalTime;

use crate::adb::{self, Invokeable};
use crate::client::Client;
use crate::file_system::QuestFsHandler;
use crate::settings::Settings;

const BASE_PORT: u16 = 15000;
const MAX_PORT: u16 = 16000;
const SERVER_PUSH_PATH: &str = "/data/local/tmp/androidfs_server";
const LOG_CAP: usize = 500;
const POLL_INTERVAL: Duration = Duration::from_millis(1500);

/// 64-bit ARM daemon (arm64-v8a).
const SERVER_ARM64: &[u8] =
    include_bytes!("../target/aarch64-linux-android/release/androidfs_server");
/// 32-bit ARM daemon (armeabi-v7a).
const SERVER_ARM32: &[u8] =
    include_bytes!("../target/armv7-linux-androideabi/release/androidfs_server");

const MOUNT_POINTS: &[&str] = &[
    "Q:", "R:", "S:", "T:", "U:", "V:", "W:", "X:", "Y:", "Z:", "D:", "E:", "F:", "G:", "H:", "I:",
    "J:", "K:", "L:", "M:", "N:", "O:", "P:",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceState {
    Detected,
    Mounting,
    Mounted(String),
    Unmounting,
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct DeviceEntry {
    pub serial: String,
    pub model: String,
    pub abi: String,
    pub state: DeviceState,
    /// Set when the user unmounted the device: no automatic remount until "Mount" is pressed.
    pub manual_hold: bool,
    pub retry_after: Option<Instant>,
}

#[derive(Clone, Debug)]
pub enum Command {
    Refresh,
    Mount(String),
    Unmount(String),
}

pub struct Shared {
    pub devices: Vec<DeviceEntry>,
    pub settings: Settings,
    pub log: VecDeque<String>,
    pub adb_path_in_use: String,
    pub adb_error: Option<String>,
    pub commands: VecDeque<Command>,
    reserved: HashSet<String>,
    pub quit: bool,
}

pub type SharedState = Arc<Mutex<Shared>>;

pub fn lock(shared: &SharedState) -> MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Shared {
    pub fn new(settings: Settings) -> Self {
        Shared {
            devices: Vec::new(),
            settings,
            log: VecDeque::new(),
            adb_path_in_use: String::new(),
            adb_error: None,
            commands: VecDeque::new(),
            reserved: HashSet::new(),
            quit: false,
        }
    }

    pub fn push_log(&mut self, line: String) {
        if self.log.len() >= LOG_CAP {
            self.log.pop_front();
        }
        self.log.push_back(format!("[{}] {}", timestamp(), line));
    }

    fn entry_mut(&mut self, serial: &str) -> Option<&mut DeviceEntry> {
        self.devices.iter_mut().find(|device| device.serial == serial)
    }
}

fn timestamp() -> String {
    unsafe {
        let mut st: SYSTEMTIME = std::mem::zeroed();
        GetLocalTime(&mut st);
        format!("{:02}:{:02}:{:02}", st.wHour, st.wMinute, st.wSecond)
    }
}

fn ui_info(shared: &SharedState, msg: String) {
    info!("{msg}");
    lock(shared).push_log(msg);
}

fn ui_error(shared: &SharedState, msg: String) {
    error!("{msg}");
    lock(shared).push_log(format!("ERROR: {msg}"));
}

fn server_binary_for(abi: &str) -> Option<&'static [u8]> {
    if abi.starts_with("arm64") {
        Some(SERVER_ARM64)
    } else if abi.starts_with("armeabi") {
        Some(SERVER_ARM32)
    } else {
        None
    }
}

fn pick_letter(reserved: &HashSet<String>) -> Option<&'static str> {
    MOUNT_POINTS.iter().copied().find(|letter| {
        !reserved.contains(*letter) && !Path::new(&format!("{letter}\\")).exists()
    })
}

enum SetupError {
    NoAvailablePort,
    NoAvailableDriveLetter,
    Adb(adb::Error),
    Io(std::io::Error),
    DaemonUnreachable,
    UnsupportedAbi(String),
}

impl From<adb::Error> for SetupError {
    fn from(err: adb::Error) -> Self {
        SetupError::Adb(err)
    }
}

impl From<std::io::Error> for SetupError {
    fn from(err: std::io::Error) -> Self {
        SetupError::Io(err)
    }
}

impl std::fmt::Display for SetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SetupError::Adb(err) => write!(f, "{err}"),
            SetupError::NoAvailablePort => {
                write!(f, "no free TCP port between {BASE_PORT} and {MAX_PORT}")
            }
            SetupError::NoAvailableDriveLetter => write!(f, "no free drive letter"),
            SetupError::DaemonUnreachable => {
                write!(f, "the on-device daemon did not accept a connection")
            }
            SetupError::Io(err) => write!(f, "could not stage the daemon binary: {err}"),
            SetupError::UnsupportedAbi(abi) => write!(
                f,
                "device CPU ABI is {abi}; supported ABIs are arm64-v8a and armeabi-v7a"
            ),
        }
    }
}

pub fn is_safe_android_path(path: &str) -> bool {
    let path = path.trim();
    path.starts_with('/')
        && path.len() < 240
        && path
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '_' | '.' | '-'))
}

fn shell(device: &adb::Device, script: &str) -> Result<std::process::Output, adb::Error> {
    device.invoke_shell_command_result(vec![script.to_string()])
}

fn getprop(device: &adb::Device, key: &str) -> String {
    shell(device, &format!("getprop {key}"))
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default()
}

fn setup(device: adb::Device, shared: &SharedState) -> Result<String, SetupError> {
    let serial = device.serial_number.clone();
    ui_info(shared, format!("Mounting {serial}"));

    let abi = String::from_utf8_lossy(&shell(&device, "getprop ro.product.cpu.abi")?.stdout)
        .trim()
        .to_string();
    let binary: &'static [u8] = if abi.is_empty() {
        SERVER_ARM64
    } else {
        server_binary_for(&abi).ok_or_else(|| SetupError::UnsupportedAbi(abi.clone()))?
    };
    {
        let mut guard = lock(shared);
        if let Some(entry) = guard.entry_mut(&serial) {
            entry.abi = abi.clone();
        }
    }

    let (letter, root) = {
        let mut guard = lock(shared);
        let letter = pick_letter(&guard.reserved).ok_or(SetupError::NoAvailableDriveLetter)?;
        guard.reserved.insert(letter.to_string());
        (letter.to_string(), guard.settings.root.clone())
    };

    let result = setup_reserved(device, shared, &letter, binary, &root);
    if result.is_err() {
        lock(shared).reserved.remove(&letter);
    }
    result
}

fn setup_reserved(
    device: adb::Device,
    shared: &SharedState,
    letter: &str,
    binary: &'static [u8],
    root_setting: &str,
) -> Result<String, SetupError> {
    let mut chosen_port = None;
    for port in BASE_PORT..=MAX_PORT {
        match device.invoke_result(vec![
            "forward".to_string(),
            format!("tcp:{port}"),
            "tcp:12345".to_string(),
        ]) {
            Ok(_) => {
                chosen_port = Some(port);
                break;
            }
            Err(err) => debug!("port {port} unavailable: {err}"),
        }
    }
    let port = chosen_port.ok_or(SetupError::NoAvailablePort)?;
    debug!("Forwarded tcp:{port} -> tcp:12345");

    let cleanup_forward = |device: &adb::Device, port: u16| {
        let _ = device.invoke_result(vec![
            "forward".to_string(),
            "--remove".to_string(),
            format!("tcp:{port}"),
        ]);
    };

    if let Err(err) = (|| -> Result<(), SetupError> {
        let _ = shell(&device, "killall androidfs_server >/dev/null 2>&1 || true");
        std::thread::sleep(Duration::from_millis(200));
        let _ = shell(&device, "rm -f /data/local/tmp/androidfs_server");

        let mut staged = tempfile::NamedTempFile::new()?;
        staged.write_all(binary)?;
        staged.flush()?;
        let staged_path = staged
            .path()
            .to_str()
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "temp path is not utf-8")
            })?
            .to_string();
        device.invoke_result(vec![
            "push".to_string(),
            staged_path,
            SERVER_PUSH_PATH.to_string(),
        ])?;
        shell(&device, "chmod 755 /data/local/tmp/androidfs_server")?;
        Ok(())
    })() {
        cleanup_forward(&device, port);
        return Err(err);
    }

    let root = if root_setting.trim().is_empty() {
        std::env::var("ANDROIDFS_ROOT").unwrap_or_default()
    } else {
        root_setting.to_string()
    };

    let serial = device.serial_number.clone();
    let letter_owned = letter.to_string();
    let device_for_daemon = device.clone();
    let shared_for_daemon = shared.clone();
    std::thread::spawn(move || {
        debug!("Starting on-device daemon for {serial}");
        let mut command = String::from("/data/local/tmp/androidfs_server");
        if !root.trim().is_empty() {
            if is_safe_android_path(&root) {
                command.push(' ');
                command.push_str(root.trim());
                info!("{serial}: using root {}", root.trim());
            } else {
                warn!("{serial}: ignoring unsafe root {root:?}");
            }
        }
        let run = shell(&device_for_daemon, &command);
        if let Err(err) = &run {
            error!("Daemon for {serial} stopped with an error: {err}");
        } else {
            info!("Daemon for {serial} exited");
        }
        let still_mounted = {
            let guard = lock(&shared_for_daemon);
            guard.devices.iter().any(|device| {
                device.serial == serial
                    && matches!(&device.state, DeviceState::Mounted(l) if *l == letter_owned)
            })
        };
        if still_mounted {
            if !dokan::unmount(U16CString::from_str(&letter_owned).unwrap()) {
                warn!("Failed to unmount {letter_owned} for {serial}");
            }
            info!("Device {serial} disconnected");
        }
    });

    let stream = match connect_with_retry(port) {
        Ok(stream) => stream,
        Err(_) => {
            let _ = shell(&device, "killall androidfs_server >/dev/null 2>&1 || true");
            cleanup_forward(&device, port);
            return Err(SetupError::DaemonUnreachable);
        }
    };

    let label = volume_label(&device.serial_number);
    let volume_name =
        U16CString::from_str(&label).unwrap_or_else(|_| U16CString::from_str("Android").unwrap());
    let mount_point = U16CString::from_str(letter).unwrap();
    let serial = device.serial_number.clone();
    let letter_for_mount = letter.to_string();
    let shared_for_mount = shared.clone();
    let device_for_cleanup = device.clone();
    {
        let mut guard = lock(shared);
        if let Some(entry) = guard.entry_mut(&device.serial_number) {
            entry.state = DeviceState::Mounted(letter.to_string());
            entry.retry_after = None;
        }
    }
    std::thread::spawn(move || {
        let handler = QuestFsHandler::new(Client::new(stream), volume_name);
        let mut failure: Option<String> = None;
        match Drive::new()
            .mount_point(&mount_point)
            .flags(MountFlags::CASE_SENSITIVE)
            .thread_count(0)
            .mount(&handler)
        {
            Ok(_) => debug!("Mount thread for {serial} exited"),
            Err(err) => {
                error!("Mount error for {serial}: {err}");
                failure = Some(format!("Dokan could not mount the drive: {err}. Is Dokan 1.5 installed?"));
            }
        }
        let _ = device_for_cleanup.invoke_result(vec![
            "forward".to_string(),
            "--remove".to_string(),
            format!("tcp:{port}"),
        ]);
        {
            let mut guard = lock(&shared_for_mount);
            guard.reserved.remove(&letter_for_mount);
            if let Some(entry) = guard.entry_mut(&serial) {
                match &failure {
                    Some(msg) => {
                        entry.state = DeviceState::Failed(msg.clone());
                        entry.retry_after = Some(Instant::now() + Duration::from_secs(15));
                    }
                    None => {
                        entry.state = DeviceState::Detected;
                        if !entry.manual_hold {
                            entry.retry_after = Some(Instant::now() + Duration::from_secs(5));
                        }
                    }
                }
            }
        }
        match failure {
            Some(msg) => ui_error(&shared_for_mount, format!("{serial}: {msg}")),
            None => ui_info(&shared_for_mount, format!("{serial} unmounted")),
        }
    });

    Ok(letter.to_string())
}

fn volume_label(serial: &str) -> String {
    let mut label = format!("Android {serial}");
    if label.len() > 31 {
        label.truncate(31);
    }
    label
}

fn connect_with_retry(port: u16) -> std::io::Result<TcpStream> {
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
    let mut last = None;
    for attempt in 0..40 {
        match TcpStream::connect(address) {
            Ok(stream) => return Ok(stream),
            Err(err) => {
                last = Some(err);
                if attempt == 0 {
                    std::thread::sleep(Duration::from_millis(300));
                } else {
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
        }
    }
    Err(last.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::TimedOut, "daemon did not come up")
    }))
}

fn mount_now(shared: &SharedState, bridge: &adb::DebugBridge, serial: &str) {
    {
        let mut guard = lock(shared);
        let Some(entry) = guard.entry_mut(serial) else {
            return;
        };
        if matches!(
            entry.state,
            DeviceState::Mounted(_) | DeviceState::Mounting | DeviceState::Unmounting
        ) {
            return;
        }
        entry.state = DeviceState::Mounting;
        entry.manual_hold = false;
    }
    match setup(bridge.device(serial), shared) {
        Ok(drive) => ui_info(shared, format!("Mounted {serial} as {drive}\\")),
        Err(err) => {
            let wait = match err {
                SetupError::UnsupportedAbi(_) => 3600,
                _ => 15,
            };
            {
                let mut guard = lock(shared);
                if let Some(entry) = guard.entry_mut(serial) {
                    entry.state = DeviceState::Failed(err.to_string());
                    entry.retry_after = Some(Instant::now() + Duration::from_secs(wait));
                }
            }
            ui_error(shared, format!("Failed to mount {serial}: {err}"));
        }
    }
}

fn unmount_one(shared: &SharedState, serial: &str) {
    let letter = {
        let mut guard = lock(shared);
        match guard.entry_mut(serial) {
            Some(entry) => {
                entry.manual_hold = true;
                let current = entry.state.clone();
                if let DeviceState::Mounted(letter) = current {
                    entry.state = DeviceState::Unmounting;
                    Some(letter)
                } else {
                    None
                }
            }
            None => None,
        }
    };
    if let Some(letter) = letter {
        ui_info(shared, format!("Unmounting {letter}"));
        if !dokan::unmount(U16CString::from_str(&letter).unwrap()) {
            {
                let mut guard = lock(shared);
                if let Some(entry) = guard.entry_mut(serial) {
                    entry.state = DeviceState::Mounted(letter.clone());
                    entry.manual_hold = false;
                }
            }
            ui_error(shared, format!("Could not unmount {letter}; close programs using the drive and retry"));
        }
    }
}

fn poll_devices(shared: &SharedState, bridge: &adb::DebugBridge) {
    let devices = match bridge.get_devices() {
        Ok(devices) => devices,
        Err(err) => {
            let message = err.to_string();
            let changed = {
                let mut guard = lock(shared);
                let changed = guard.adb_error.as_deref() != Some(message.as_str());
                guard.adb_error = Some(message.clone());
                changed
            };
            if changed {
                ui_error(shared, format!("ADB: {message}"));
            }
            return;
        }
    };
    let serials: Vec<String> = devices.into_iter().map(|d| d.serial_number).collect();
    {
        let mut guard = lock(shared);
        guard.adb_error = None;
        guard.devices.retain(|d| {
            serials.contains(&d.serial)
                || matches!(
                    d.state,
                    DeviceState::Mounted(_) | DeviceState::Mounting | DeviceState::Unmounting
                )
        });
        for serial in &serials {
            if !guard.devices.iter().any(|d| d.serial == *serial) {
                guard.devices.push(DeviceEntry {
                    serial: serial.clone(),
                    model: String::new(),
                    abi: String::new(),
                    state: DeviceState::Detected,
                    manual_hold: false,
                    retry_after: None,
                });
                guard.push_log(format!("Device detected: {serial}"));
            }
        }
    }

    let missing: Vec<String> = {
        let guard = lock(shared);
        guard
            .devices
            .iter()
            .filter(|d| d.model.is_empty())
            .map(|d| d.serial.clone())
            .collect()
    };
    for serial in missing {
        let device = bridge.device(&serial);
        let model = getprop(&device, "ro.product.model");
        let maker = getprop(&device, "ro.product.manufacturer");
        let abi = getprop(&device, "ro.product.cpu.abi");
        let name = if model.is_empty() {
            maker
        } else if maker.is_empty() || model.to_lowercase().starts_with(&maker.to_lowercase()) {
            model
        } else {
            format!("{maker} {model}")
        };
        let mut guard = lock(shared);
        if let Some(entry) = guard.entry_mut(&serial) {
            entry.model = if name.is_empty() {
                "Android device".to_string()
            } else {
                name
            };
            if entry.abi.is_empty() {
                entry.abi = abi;
            }
        }
    }

    let now = Instant::now();
    let (auto_mount, candidates): (bool, Vec<String>) = {
        let guard = lock(shared);
        (
            guard.settings.auto_mount,
            guard
                .devices
                .iter()
                .filter(|d| {
                    matches!(d.state, DeviceState::Detected | DeviceState::Failed(_))
                        && !d.manual_hold
                        && d.retry_after.map_or(true, |t| now >= t)
                })
                .map(|d| d.serial.clone())
                .collect(),
        )
    };
    if auto_mount {
        for serial in candidates {
            mount_now(shared, bridge, &serial);
        }
    }
}

pub fn spawn_engine(shared: SharedState) {
    std::thread::spawn(move || engine_loop(shared));
}

fn engine_loop(shared: SharedState) {
    let mut adb_key = lock(&shared).settings.adb_path.clone();
    let mut bridge = adb::DebugBridge::locate_with(&adb_key);
    lock(&shared).adb_path_in_use = bridge.adb_path.clone();
    ui_info(&shared, format!("Using adb at {}", bridge.adb_path));

    let mut last_poll = Instant::now() - Duration::from_secs(60);
    let mut force_poll = true;
    loop {
        let (quit, adb_wanted) = {
            let guard = lock(&shared);
            (guard.quit, guard.settings.adb_path.clone())
        };
        if quit {
            return;
        }
        if adb_wanted != adb_key {
            adb_key = adb_wanted;
            bridge = adb::DebugBridge::locate_with(&adb_key);
            lock(&shared).adb_path_in_use = bridge.adb_path.clone();
            ui_info(&shared, format!("Using adb at {}", bridge.adb_path));
            force_poll = true;
        }

        let commands: Vec<Command> = lock(&shared).commands.drain(..).collect();
        for command in commands {
            match command {
                Command::Refresh => force_poll = true,
                Command::Mount(serial) => mount_now(&shared, &bridge, &serial),
                Command::Unmount(serial) => unmount_one(&shared, &serial),
            }
        }

        if force_poll || last_poll.elapsed() >= POLL_INTERVAL {
            force_poll = false;
            last_poll = Instant::now();
            poll_devices(&shared, &bridge);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Unmounts every drive and stops the engine. Called when the window closes.
pub fn shutdown(shared: &SharedState) {
    let letters: Vec<String> = {
        let mut guard = lock(shared);
        guard.quit = true;
        guard
            .devices
            .iter()
            .filter_map(|d| match &d.state {
                DeviceState::Mounted(letter) => Some(letter.clone()),
                _ => None,
            })
            .collect()
    };
    for letter in letters {
        let _ = dokan::unmount(U16CString::from_str(&letter).unwrap());
    }
    for _ in 0..40 {
        let busy = lock(shared).devices.iter().any(|d| {
            matches!(
                d.state,
                DeviceState::Mounted(_) | DeviceState::Unmounting | DeviceState::Mounting
            )
        });
        if !busy {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
