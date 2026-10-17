use std::time::SystemTime;

use serde::{Deserialize, Serialize};

pub type FileHandle = u32;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct FileInfo {
    pub creation_time: SystemTime,
    pub last_modified: SystemTime,
    pub last_accessed: SystemTime,
    pub name: String,
    pub size: u64,
    pub mode: u32,
    pub ino: u64,
}

pub fn is_dir_mode(mode: u32) -> bool {
    mode & 0xF000 == 0x4000
}

pub fn is_link_mode(mode: u32) -> bool {
    mode & 0xF000 == 0xA000
}
