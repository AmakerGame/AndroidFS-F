pub mod adb_parse;
pub mod daemon_cmd;
pub mod models;
pub mod requests;
pub mod responses;

// The on-device daemon only makes sense on Unix (Android). These modules use
// `std::os::unix` and `libc::statvfs`, so they must not be compiled for the
// Windows driver build.
#[cfg(unix)]
pub mod fs_ops;
#[cfg(unix)]
mod server;

#[cfg(windows)]
mod adb;
#[cfg(windows)]
mod cache;
#[cfg(windows)]
mod client;
#[cfg(windows)]
mod file_system;
#[cfg(windows)]
mod gui;
#[cfg(windows)]
mod service;
#[cfg(windows)]
mod settings;

pub fn server_main() {
    #[cfg(unix)]
    {
        server::run();
    }
    #[cfg(not(unix))]
    {
        eprintln!("androidfs_server only runs on the Android device (Unix).");
        std::process::exit(1);
    }
}

pub fn driver_main() {
    #[cfg(windows)]
    {
        gui::run();
    }
    #[cfg(not(windows))]
    {
        eprintln!("androidfs_driver only runs on Windows (it mounts a Dokan drive).");
        std::process::exit(1);
    }
}
