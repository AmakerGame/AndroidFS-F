use serde::{Deserialize, Serialize};

use crate::models::FileHandle;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum Request {
    List(ListFiles),
    Stat(StatFile),
    Delete(DeleteFile),
    CreateFile(CreateFile),
    CreateDirectory(CreateDirectory),
    Open(OpenFile),
    Close(CloseFile),
    Move(MoveFile),
    GetFreeSpace,
    Read(ReadFile),
    Write(WriteFile),
    SetEndOfFile(SetEndOfFile),
    /// Handshake: asks the daemon who it is (token, user id, drive root).
    /// Appended last so the wire format of the other requests is unchanged.
    Hello,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ReadFile {
    pub handle: FileHandle,
    pub offset: u64,
    pub len: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct WriteFile {
    pub handle: FileHandle,
    pub offset: u64,
    pub len: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct SetEndOfFile {
    pub handle: FileHandle,
    pub len: u64,
}

pub type CloseFile = FileHandle;
pub type ListFiles = String;
pub type StatFile = String;
pub type DeleteFile = String;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct CreateFile {
    pub path: String,
    /// When true, an existing file is truncated to zero length.
    pub truncate: bool,
    /// When true, fail with [`crate::responses::Error::FileExists`] if the path exists.
    pub exclusive: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct CreateDirectory {
    pub path: String,
    pub exclusive: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct OpenFile {
    pub path: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct MoveFile {
    pub from: String,
    pub to: String,
    pub replace_if_exists: bool,
}

/// Sentinel offset meaning "append / write at the current end of the file".
pub const WRITE_AT_END: u64 = u64::MAX;
