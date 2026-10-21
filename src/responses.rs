use serde::{Deserialize, Serialize};

use crate::models::FileInfo;

pub type Result<T> = std::result::Result<T, Error>;

pub type ListFiles = Vec<FileInfo>;
pub type StatFile = FileInfo;
pub type ReadFile = u32;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct FreeSpace {
    pub total_bytes: u64,
    pub free_bytes: u64,
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
