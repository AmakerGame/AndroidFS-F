//! Background engine: finds Android devices through ADB, pushes the daemon,
//! mounts / unmounts Dokan drives and publishes the state the GUI shows.

use std::collections::{HashSet, VecDeque};
use std::io::Write;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use dokan::{Drive, MountFlags};
use log::{debug, error, info, warn};
use widestring::U16CString;
use winapi::um::minwinbase::SYSTEMTIME;
use winapi::um::sysinfoapi::GetLocalTime;

use crate::adb::{self, Invokeable};
use crate::client::Client;
use crate::daemon_cmd::{
    self, Access, AccessMode, SuStyle, DAEMON_PORT, KILL_SCRIPT, SERVER_PATH, SU_STYLES,
};
use crate::file_system::QuestFsHandler;
use crate::responses;
use crate::settings::Settings;

pub use crate::daemon_cmd::is_safe_android_path;

const BASE_PORT: u16 = 15000;
const MAX_PORT: u16 = 16000;
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

/// What the daemon reported once the drive was mounted; shown in the device list.
#[derive(Clone, Debug)]
pub struct MountInfo {
    /// "shell (no root)", "root (su -c)", ...
    pub access: String,
    /// Android folder that is shown as the drive.
    pub root_dir: String,
    pub entries: Option<u32>,
    pub note: Option<String>,
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
    pub info: Option<MountInfo>,
    /// The `su` flavour that worked on this device (remembered to clean up root daemons).
    pub su_style: Option<SuStyle>,
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
    DaemonUnreachable(String),
    DaemonExited(String),
    StaleDaemon,
    NoRoot,
    MountFailed(String),
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
            SetupError::DaemonUnreachable(detail) => write!(
                f,
                "the on-device daemon did not answer ({detail}). Check the log; the phone may block running files from /data/local/tmp"
            ),
            SetupError::DaemonExited(output) => {
                write!(f, "the on-device daemon stopped right after starting: {output}")
            }
            SetupError::StaleDaemon => write!(
                f,
                "an old androidfs_server (probably started as root) still holds the port on the phone; reboot the phone or stop it with: su -c killall androidfs_server"
            ),
            SetupError::NoRoot => write!(
                f,
                "root access mode is selected, but the device did not grant root (su). Approve the request on the phone or switch the access mode to Auto"
            ),
            SetupError::MountFailed(detail) => write!(f, "{detail}"),
            SetupError::Io(err) => write!(f, "could not stage the daemon binary: {err}"),
            SetupError::UnsupportedAbi(abi) => write!(
                f,
                "device CPU ABI is {abi}; supported ABIs are arm64-v8a and armeabi-v7a"
            ),
        }
    }
}

fn shell(device: &adb::Device, script: &str) -> Result<std::process::Output, adb::Error> {
    device.invoke_shell_command_result(vec![script.to_string()])
}

fn getprop(device: &adb::Device, key: &str) -> String {
    shell(device, &format!("getprop {key}"))
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default()
}

fn output_text(out: &std::process::Output) -> String {
    let mut text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let err = String::from_utf8_lossy(&out.stderr);
    let err = err.trim();
    if !err.is_empty() {
        if !text.is_empty() {
            text.push_str(" | ");
        }
        text.push_str(err);
    }
    if text.chars().count() > 600 {
        text = text.chars().take(600).collect::<String>() + "...";
    }
    text
}

/// True when the ADB daemon on the device already runs as root (userdebug
/// builds, `adb root`, many TV boxes), so no `su` is needed.
fn shell_is_root(device: &adb::Device) -> bool {
    shell(device, "id -u")
        .ok()
        .and_then(|out| daemon_cmd::parse_uid(&String::from_utf8_lossy(&out.stdout)))
        == Some(0)
}

/// Finds out which `su` flavour (if any) gives a root shell. May block until
/// the user approves the request on the phone, hence the timeout.
fn probe_root(device: &adb::Device, shared: &SharedState) -> Option<SuStyle> {
    let serial = device.serial_number.clone();
    for style in SU_STYLES {
        let script = daemon_cmd::wrap(Access::Root(style), "id -u");
        match device.shell_timeout(&script, Duration::from_secs(15)) {
            Ok(out) if out.status.success() => {
                if daemon_cmd::parse_uid(&String::from_utf8_lossy(&out.stdout)) == Some(0) {
                    ui_info(shared, format!("{serial}: root access granted ({})", style.label()));
                    if let Some(entry) = lock(shared).entry_mut(&serial) {
                        entry.su_style = Some(style);
                    }
                    return Some(style);
                }
            }
            Ok(_) => {}
            Err(err) => {
                if matches!(err.kind, adb::ErrorKind::TimedOut) {
                    ui_info(
                        shared,
                        format!("{serial}: su did not answer; approve the root request on the phone"),
                    );
                    return None;
                }
                debug!("{serial}: su probe ({}) failed: {err}", style.label());
            }
        }
    }
    ui_info(shared, format!("{serial}: no root access (su not available or denied)"));
    None
}

/// Stops leftover daemons. The root variant is only attempted when `su` is known to work.
fn kill_daemons(device: &adb::Device, su: Option<SuStyle>) {
    let _ = shell(device, KILL_SCRIPT);
    if let Some(style) = su {
        let script = daemon_cmd::wrap(Access::Root(style), KILL_SCRIPT);
        let _ = device.shell_timeout(&script, Duration::from_secs(8));
    }
}

struct Session {
    stream: TcpStream,
    hello: responses::Hello,
    access: Access,
    /// Set when the host stops the daemon on purpose, so its watcher thread
    /// does not unmount the drive that replaced it.
    stop: Arc<AtomicBool>,
}

enum HandshakeFailure {
    NotReady(String),
    WrongDaemon,
}

/// Connect + `Hello`. With `adb forward`, connecting succeeds even before the
/// daemon listens (adb accepts locally and drops the connection later), so this
/// only counts as success when the daemon answers with the expected token.
fn try_handshake(port: u16, token: &str) -> Result<(TcpStream, responses::Hello), HandshakeFailure> {
    let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(1500))
        .map_err(|err| HandshakeFailure::NotReady(err.to_string()))?;
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));
    match Client::handshake(&mut stream) {
        Ok(hello) => {
            if hello.protocol != responses::PROTOCOL_VERSION || hello.token != token {
                return Err(HandshakeFailure::WrongDaemon);
            }
            let _ = stream.set_read_timeout(None);
            let _ = stream.set_write_timeout(None);
            Ok((stream, hello))
        }
        Err(err) => Err(HandshakeFailure::NotReady(err.to_string())),
    }
}

/// Starts the daemon with the given access and waits until it really answers.
fn start_daemon(
    device: &adb::Device,
    shared: &SharedState,
    letter: &str,
    port: u16,
    access: Access,
    root_setting: &str,
) -> Result<Session, SetupError> {
    let serial = device.serial_number.clone();
    let su = match access {
        Access::Root(style) => Some(style),
        Access::Shell { .. } => None,
    };
    kill_daemons(device, su);
    std::thread::sleep(Duration::from_millis(250));

    let root_arg = if root_setting.trim().is_empty() {
        None
    } else if is_safe_android_path(root_setting) {
        Some(root_setting.trim())
    } else {
        warn!("{serial}: ignoring unsafe root {root_setting:?}");
        None
    };
    let token = daemon_cmd::make_token();
    let command = daemon_cmd::wrap(access, &daemon_cmd::server_command(root_arg, &token));
    debug!("{serial}: starting daemon: {command}");

    let stop = Arc::new(AtomicBool::new(false));
    let exit_text: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    {
        let device = device.clone();
        let shared = shared.clone();
        let stop = stop.clone();
        let exit_text = exit_text.clone();
        let serial = serial.clone();
        let letter = letter.to_string();
        std::thread::spawn(move || {
            let text = match shell(&device, &command) {
                Ok(out) => output_text(&out),
                Err(err) => err.to_string(),
            };
            info!("Daemon for {serial} exited: {text}");
            *exit_text.lock().unwrap_or_else(|p| p.into_inner()) = Some(text);
            if stop.load(Ordering::SeqCst) {
                return;
            }
            let still_mounted = {
                let guard = lock(&shared);
                guard.devices.iter().any(|entry| {
                    entry.serial == serial
                        && matches!(&entry.state, DeviceState::Mounted(l) if *l == letter)
                })
            };
            if still_mounted {
                if !dokan::unmount(U16CString::from_str(&letter).unwrap()) {
                    warn!("Failed to unmount {letter} for {serial}");
                }
                ui_info(&shared, format!("Device {serial} disconnected"));
            }
        });
    }

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut last_detail: String;
    loop {
        if let Some(text) = exit_text.lock().unwrap_or_else(|p| p.into_inner()).clone() {
            return Err(SetupError::DaemonExited(if text.is_empty() {
                "no output".to_string()
            } else {
                text
            }));
        }
        match try_handshake(port, &token) {
            Ok((stream, hello)) => {
                return Ok(Session {
                    stream,
                    hello,
                    access,
                    stop,
                });
            }
            Err(HandshakeFailure::WrongDaemon) => {
                stop.store(true, Ordering::SeqCst);
                return Err(SetupError::StaleDaemon);
            }
            Err(HandshakeFailure::NotReady(detail)) => last_detail = detail,
        }
        if Instant::now() >= deadline {
            stop.store(true, Ordering::SeqCst);
            kill_daemons(device, su);
            return Err(SetupError::DaemonUnreachable(last_detail));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Like `start_daemon`, and when a stale daemon (e.g. one started as root by an
/// earlier run) still owns the port, clears it once and tries again.
fn start_daemon_clearing_stale(
    device: &adb::Device,
    shared: &SharedState,
    letter: &str,
    port: u16,
    access: Access,
    root_setting: &str,
) -> Result<Session, SetupError> {
    match start_daemon(device, shared, letter, port, access, root_setting) {
        Err(SetupError::StaleDaemon) => {
            ui_info(
                shared,
                format!("{}: stopping an old daemon", device.serial_number),
            );
            let known = lock(shared)
                .entry_mut(&device.serial_number)
                .and_then(|entry| entry.su_style);
            let style = known.or_else(|| probe_root(device, shared));
            kill_daemons(device, style);
            std::thread::sleep(Duration::from_millis(600));
            start_daemon(device, shared, letter, port, access, root_setting)
        }
        other => other,
    }
}

fn stop_session(device: &adb::Device, session: Session) {
    session.stop.store(true, Ordering::SeqCst);
    let su = match session.access {
        Access::Root(style) => Some(style),
        Access::Shell { .. } => None,
    };
    drop(session.stream);
    kill_daemons(device, su);
    std::thread::sleep(Duration::from_millis(300));
}

/// Picks shell or root according to the access mode, and in Auto mode moves to
/// root when the shell user cannot read the chosen folder (the typical cause of
/// a drive that mounts but looks empty in Explorer).
fn launch_daemon(
    device: &adb::Device,
    shared: &SharedState,
    letter: &str,
    port: u16,
    mode: AccessMode,
    root_setting: &str,
) -> Result<Session, SetupError> {
    let serial = device.serial_number.clone();
    let shell_access = Access::Shell {
        uid_zero: shell_is_root(device),
    };

    let start_root = |device: &adb::Device| -> Result<Session, SetupError> {
        let style = probe_root(device, shared).ok_or(SetupError::NoRoot)?;
        start_daemon_clearing_stale(device, shared, letter, port, Access::Root(style), root_setting)
    };

    if mode == AccessMode::Root && !shell_access.is_root() {
        return start_root(device);
    }

    let session = match start_daemon_clearing_stale(device, shared, letter, port, shell_access, root_setting) {
        Ok(session) => session,
        Err(err) if mode == AccessMode::Auto => {
            ui_info(shared, format!("{serial}: starting without root failed ({err}); trying root"));
            return start_root(device).map_err(|_| err);
        }
        Err(err) => return Err(err),
    };

    let unreadable = session.hello.root_fallback || session.hello.root_entries.is_none();
    if mode == AccessMode::Auto && !session.access.is_root() && unreadable {
        ui_info(
            shared,
            format!(
                "{serial}: the shell user cannot read the selected folder ({}); looking for root",
                session.hello.note.clone().unwrap_or_default()
            ),
        );
        if let Some(style) = probe_root(device, shared) {
            stop_session(device, session);
            match start_daemon_clearing_stale(device, shared, letter, port, Access::Root(style), root_setting) {
                Ok(rooted) => return Ok(rooted),
                Err(err) => {
                    ui_error(shared, format!("{serial}: root start failed ({err}); continuing without root"));
                    return start_daemon_clearing_stale(device, shared, letter, port, shell_access, root_setting);
                }
            }
        }
    }
    Ok(session)
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

    let (letter, root, mode) = {
        let mut guard = lock(shared);
        let letter = pick_letter(&guard.reserved).ok_or(SetupError::NoAvailableDriveLetter)?;
        guard.reserved.insert(letter.to_string());
        (
            letter.to_string(),
            guard.settings.root.clone(),
            guard.settings.access_mode,
        )
    };

    let result = setup_reserved(device, shared, &letter, binary, &root, mode);
    if result.is_err() {
        lock(shared).reserved.remove(&letter);
    }
    result
}

fn forward_free_port(device: &adb::Device) -> Result<u16, SetupError> {
    for port in BASE_PORT..=MAX_PORT {
        // --no-rebind: never steal a port that another device (or another
        // instance) already forwards; a plain `forward` silently replaces it.
        match device.invoke_result(vec![
            "forward".to_string(),
            "--no-rebind".to_string(),
            format!("tcp:{port}"),
            format!("tcp:{DAEMON_PORT}"),
        ]) {
            Ok(_) => return Ok(port),
            Err(err) => debug!("port {port} unavailable: {err}"),
        }
    }
    Err(SetupError::NoAvailablePort)
}

fn remove_forward(device: &adb::Device, port: u16) {
    let _ = device.invoke_result(vec![
        "forward".to_string(),
        "--remove".to_string(),
        format!("tcp:{port}"),
    ]);
}

fn push_daemon(device: &adb::Device, binary: &[u8]) -> Result<(), SetupError> {
    kill_daemons(device, None);
    std::thread::sleep(Duration::from_millis(200));
    let _ = shell(device, &format!("rm -f {SERVER_PATH}"));

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
    device.invoke_result(vec!["push".to_string(), staged_path, SERVER_PATH.to_string()])?;
    shell(device, &format!("chmod 755 {SERVER_PATH}"))?;
    Ok(())
}

fn setup_reserved(
    device: adb::Device,
    shared: &SharedState,
    letter: &str,
    binary: &'static [u8],
    root_setting: &str,
    mode: AccessMode,
) -> Result<String, SetupError> {
    let port = forward_free_port(&device)?;
    debug!("Forwarded tcp:{port} -> tcp:{DAEMON_PORT}");

    let root = if root_setting.trim().is_empty() {
        std::env::var("ANDROIDFS_ROOT").unwrap_or_default()
    } else {
        root_setting.to_string()
    };

    if let Err(err) = push_daemon(&device, binary) {
        remove_forward(&device, port);
        return Err(err);
    }

    let session = match launch_daemon(&device, shared, letter, port, mode, &root) {
        Ok(session) => session,
        Err(err) => {
            remove_forward(&device, port);
            return Err(err);
        }
    };

    let serial = device.serial_number.clone();
    let hello = session.hello.clone();
    let access = session.access;
    let access_label = access.label();
    ui_info(
        shared,
        format!(
            "{serial}: daemon runs as {access_label}, uid {}, folder {}, {}",
            hello.uid,
            hello.root,
            match hello.root_entries {
                Some(count) => format!("{count} entries"),
                None => "folder cannot be listed".to_string(),
            }
        ),
    );
    if let Some(note) = &hello.note {
        ui_info(shared, format!("{serial}: {note}"));
    }
    if hello.root_entries.is_none() || hello.root_fallback {
        let hint = if access.is_root() {
            "Even root cannot read this folder; pick another Drive root in Settings."
        } else {
            "Without root this folder is not readable. Set the access mode to Root (or Auto on a rooted phone), or pick another Drive root in Settings."
        };
        ui_error(shared, format!("{serial}: {hint}"));
    } else if hello.root_entries == Some(0) {
        ui_info(
            shared,
            format!("{serial}: the folder {} is empty on the phone", hello.root),
        );
    }

    let stream = session.stream;
    let label = volume_label(&serial);
    let volume_name =
        U16CString::from_str(&label).unwrap_or_else(|_| U16CString::from_str("Android").unwrap());
    let mount_point = U16CString::from_str(letter).unwrap();
    let letter_for_mount = letter.to_string();
    let shared_for_mount = shared.clone();
    let device_for_cleanup = device.clone();
    let serial_for_mount = serial.clone();
    let finished = Arc::new(AtomicBool::new(false));
    let mount_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    {
        let finished = finished.clone();
        let mount_error = mount_error.clone();
        std::thread::spawn(move || {
            let serial = serial_for_mount;
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
                    failure = Some(format!(
                        "Dokan could not mount the drive: {err}. Is Dokan 1.5 installed? (restart Windows after installing it)"
                    ));
                }
            }
            remove_forward(&device_for_cleanup, port);
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
                            entry.info = None;
                            if !entry.manual_hold {
                                entry.retry_after = Some(Instant::now() + Duration::from_secs(5));
                            }
                        }
                    }
                }
            }
            match &failure {
                Some(msg) => ui_error(&shared_for_mount, format!("{serial}: {msg}")),
                None => ui_info(&shared_for_mount, format!("{serial} unmounted")),
            }
            *mount_error.lock().unwrap_or_else(|p| p.into_inner()) = failure;
            finished.store(true, Ordering::SeqCst);
        });
    }

    // Do not report "Mounted" before Windows can actually see the drive.
    let drive_path = format!("{letter}\\");
    let ready_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if finished.load(Ordering::SeqCst) {
            let detail = mount_error
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone()
                .unwrap_or_else(|| "the drive was unmounted right after mounting".to_string());
            return Err(SetupError::MountFailed(detail));
        }
        if Path::new(&drive_path).exists() {
            break;
        }
        if Instant::now() >= ready_deadline {
            warn!("{serial}: drive {drive_path} did not appear within 10 s");
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    {
        let mut guard = lock(shared);
        if let Some(entry) = guard.entry_mut(&serial) {
            entry.state = DeviceState::Mounted(letter.to_string());
            entry.retry_after = None;
            entry.info = Some(MountInfo {
                access: access_label,
                root_dir: hello.root.clone(),
                entries: hello.root_entries,
                note: hello.note.clone(),
            });
        }
    }

    // Self-check from the Windows side: what does Explorer's own listing see?
    match std::fs::read_dir(&drive_path) {
        Ok(entries) => {
            let seen = entries.filter(|entry| entry.is_ok()).count();
            ui_info(shared, format!("{serial}: {drive_path} lists {seen} entries"));
            if seen == 0 && hello.root_entries.unwrap_or(0) > 0 {
                ui_error(
                    shared,
                    format!(
                        "{serial}: the daemon sees {} entries but Windows lists none; see {} for details",
                        hello.root_entries.unwrap_or(0),
                        crate::settings::log_path().display()
                    ),
                );
            }
        }
        Err(err) => ui_error(shared, format!("{serial}: cannot list {drive_path}: {err}")),
    }

    Ok(letter.to_string())
}

fn volume_label(serial: &str) -> String {
    let mut label = format!("Android {serial}");
    if label.len() > 31 {
        label.truncate(31);
    }
    label
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
                    info: None,
                    su_style: None,
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
