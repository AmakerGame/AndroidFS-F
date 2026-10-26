use serde::{Deserialize, Serialize};

use crate::models::FileInfo;

pub type Result<T> = std::result::Result<T, Error>;

/// Bumped whenever the wire protocol changes incompatibly.
pub const PROTOCOL_VERSION: u32 = 2;

pub type ListFiles = Vec<FileInfo>;
pub type StatFile = FileInfo;
pub type ReadFile = u32;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct FreeSpace {
    pub total_bytes: u64,
    pub free_bytes: u64,
}

/// Answer to [`crate::requests::Request::Hello`].
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub protocol: u32,
    /// The token the host passed on the daemon command line.
    pub token: String,
    /// Unix user id the daemon runs as (0 = root, 2000 = adb shell).
    pub uid: u32,
    /// Directory that is exposed as the Windows drive.
    pub root: String,
    /// Number of entries in `root`, or `None` when it could not be listed.
    pub root_entries: Option<u32>,
    /// True when the requested root was unreadable and another one was used.
    pub root_fallback: bool,
    /// Human-readable remark (why a fallback happened, what to try).
    pub note: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum Error {
    FileNotFound,
    NoSuchHandle,
    FileExists,
    PermissionDenied,
    CouldNotFindDisk,
    Other,
    DirectoryNotEmpty,
    IsADirectory,
    NotADirectory,
}
