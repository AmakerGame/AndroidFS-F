use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dokan::{Drive, MountFlags};
use log::*;
use widestring::U16CString;

use crate::adb::{self, Invokeable};
use crate::client::Client;
use crate::file_system::QuestFsHandler;

const BASE_PORT: u16 = 15000;
const MAX_PORT: u16 = 16000;
const SERVER_PUSH_PATH: &str = "/data/local/tmp/androidfs_server";
const SERVER_EXECUTABLE: &[u8] =
    include_bytes!("../target/aarch64-linux-android/release/androidfs_server");

const MOUNT_POINTS: &[&str] = &[
    "Q:", "R:", "S:", "T:", "U:", "V:", "W:", "X:", "Y:", "Z:", "D:", "E:", "F:", "G:", "H:", "I:",
    "J:", "K:", "L:", "M:", "N:", "O:", "P:",
];

struct MountTable {
    mounted: HashMap<String, String>,
    reserved: HashSet<String>,
    retry_after: HashMap<String, Instant>,
}

impl MountTable {
    fn new() -> Self {
        MountTable {
            mounted: HashMap::new(),
            reserved: HashSet::new(),
            retry_after: HashMap::new(),
        }
    }

    fn pick_letter(&self) -> Option<&'static str> {
        MOUNT_POINTS.iter().copied().find(|letter| {
            !self.reserved.contains(*letter)
                && !Path::new(&format!("{letter}\\")).exists()
        })
    }
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
            SetupError::NoAvailablePort => write!(f, "no free TCP port between {BASE_PORT} and {MAX_PORT}"),
            SetupError::NoAvailableDriveLetter => write!(f, "no free drive letter"),
            SetupError::DaemonUnreachable => {
                write!(f, "the on-device daemon did not accept a connection")
            }
            SetupError::Io(err) => write!(f, "could not stage the daemon binary: {err}"),
            SetupError::UnsupportedAbi(abi) => write!(
                f,
                "device CPU ABI is {abi}; this build only includes the arm64 daemon"
            ),
        }
    }
}

fn is_safe_android_path(path: &str) -> bool {
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

fn setup(device: adb::Device, table: Arc<Mutex<MountTable>>) -> Result<String, SetupError> {
    info!("Mounting {}", device.serial_number);

    let abi = String::from_utf8_lossy(
        &shell(&device, "getprop ro.product.cpu.abi")?.stdout,
    )
    .trim()
    .to_string();
    if !abi.is_empty() && !abi.starts_with("arm64") {
        return Err(SetupError::UnsupportedAbi(abi));
    }

    let letter = {
        let mut guard = table.lock().unwrap();
        let letter = guard.pick_letter().ok_or(SetupError::NoAvailableDriveLetter)?;
        guard.reserved.insert(letter.to_string());
        letter.to_string()
    };

    let result = setup_reserved(device, table.clone(), &letter);
    if result.is_err() {
        table.lock().unwrap().reserved.remove(&letter);
    }
    result
}

fn setup_reserved(
    device: adb::Device,
    table: Arc<Mutex<MountTable>>,
    letter: &str,
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
        let _ = shell(
            &device,
            "killall androidfs_server >/dev/null 2>&1 || true",
        );
        std::thread::sleep(Duration::from_millis(200));
        let _ = shell(&device, "rm -f /data/local/tmp/androidfs_server");

        let mut staged = tempfile::NamedTempFile::new()?;
        staged.write_all(SERVER_EXECUTABLE)?;
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

    let serial = device.serial_number.clone();
    let letter_owned = letter.to_string();
    let device_for_daemon = device.clone();
    let table_for_daemon = table.clone();
    std::thread::spawn(move || {
        debug!("Starting on-device daemon for {serial}");
        let mut command = String::from("/data/local/tmp/androidfs_server");
        if let Ok(root) = std::env::var("ANDROIDFS_ROOT") {
            if is_safe_android_path(&root) {
                command.push(' ');
                command.push_str(root.trim());
                info!("{serial}: using ANDROIDFS_ROOT {}", root.trim());
            } else {
                warn!("{serial}: ignoring unsafe ANDROIDFS_ROOT {root:?}");
            }
        }
        let run = shell(&device_for_daemon, &command);
        if let Err(err) = &run {
            error!("Daemon for {serial} stopped with an error: {err}");
        } else {
            info!("Daemon for {serial} exited");
        }
        let mut guard = table_for_daemon.lock().unwrap();
        if guard.mounted.contains_key(&serial) {
            drop(guard);
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
    let table_for_mount = table.clone();
    let device_for_cleanup = device.clone();
    {
        let mut guard = table.lock().unwrap();
        guard
            .mounted
            .insert(device.serial_number.clone(), letter.to_string());
        guard.retry_after.remove(&device.serial_number);
    }
    std::thread::spawn(move || {
        let handler = QuestFsHandler::new(Client::new(stream), volume_name);
        match Drive::new()
            .mount_point(&mount_point)
            .flags(MountFlags::CASE_SENSITIVE)
            .thread_count(0)
            .mount(&handler)
        {
            Ok(_) => debug!("Mount thread for {serial} exited"),
            Err(err) => error!("Mount error for {serial}: {err}"),
        }
        let _ = device_for_cleanup.invoke_result(vec![
            "forward".to_string(),
            "--remove".to_string(),
            format!("tcp:{port}"),
        ]);
        let mut guard = table_for_mount.lock().unwrap();
        if let Some(used) = guard.mounted.remove(&serial) {
            guard.reserved.remove(&used);
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

fn init_logging(console: bool) {
    let mut builder = env_logger::Builder::new();
    builder.parse_default_env();
    if std::env::var_os("RUST_LOG").is_none() {
        builder.filter_level(LevelFilter::Info);
    }
    if console {
        builder.init();
        return;
    }
    let path = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("androidfs.log")))
        .unwrap_or_else(|| std::path::PathBuf::from("androidfs.log"));
    match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => {
            builder.target(env_logger::Target::Pipe(Box::new(file))).init();
        }
        Err(_) => {
            builder.init();
        }
    }
}

pub fn run() {
    let console = std::env::var("ANDROIDFS_CONSOLE").is_ok();
    if !console {
        unsafe {
            winapi::um::wincon::FreeConsole();
        }
    }
    init_logging(console);
    info!("AndroidFS-F 0.2 started");

    let adb = adb::DebugBridge::locate();
    info!("Using adb at {}", adb.adb_path);
    let table = Arc::new(Mutex::new(MountTable::new()));

    loop {
        match adb.get_devices() {
            Ok(devices) => {
                let due: Vec<adb::Device> = {
                    let guard = table.lock().unwrap();
                    devices
                        .into_iter()
                        .filter(|device| {
                            if guard.mounted.contains_key(&device.serial_number) {
                                return false;
                            }
                            match guard.retry_after.get(&device.serial_number) {
                                Some(when) => Instant::now() >= *when,
                                None => true,
                            }
                        })
                        .collect()
                };
                for device in due {
                    let serial = device.serial_number.clone();
                    match setup(device, table.clone()) {
                        Ok(drive) => info!("Mounted {serial} as {drive}"),
                        Err(err) => {
                            error!("Failed to mount {serial}: {err}");
                            let mut guard = table.lock().unwrap();
                            let fails = guard.retry_after.contains_key(&serial);
                            let wait = if fails { 15 } else { 5 };
                            guard
                                .retry_after
                                .insert(serial, Instant::now() + Duration::from_secs(wait));
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(750));
            }
            Err(err) => {
                error!("{err}");
                std::thread::sleep(Duration::from_secs(3));
            }
        }
    }
}
