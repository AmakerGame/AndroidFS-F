pub mod adb_parse;
pub mod fs_ops;
pub mod models;
pub mod requests;
pub mod responses;

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
mod service;

pub fn server_main() {
    server::run();
}

pub fn driver_main() {
    #[cfg(windows)]
    {
        service::run();
    }
    #[cfg(not(windows))]
    {
        eprintln!("androidfs_driver only runs on Windows (it mounts a Dokan drive).");
        std::process::exit(1);
    }
}
