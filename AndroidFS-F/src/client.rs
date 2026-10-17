use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Mutex, MutexGuard};

use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use dokan::OperationError;
use serde::de::DeserializeOwned;
use winapi::shared::ntstatus::*;

use crate::models::FileHandle;
use crate::requests;
use crate::responses;

const MAX_FRAME: u64 = 16 * 1024 * 1024;

#[derive(Debug)]
pub enum Error {
    IOFailed(std::io::Error),
    ReceivedInvalidData(bincode::Error),
    RequestFailed(responses::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::IOFailed(err) => write!(f, "I/O error talking to the device daemon: {err}"),
            Error::ReceivedInvalidData(err) => write!(f, "daemon sent an invalid response: {err}"),
            Error::RequestFailed(err) => write!(f, "daemon rejected the request: {err:?}"),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::IOFailed(err)
    }
}

impl From<responses::Error> for Error {
    fn from(err: responses::Error) -> Self {
        Error::RequestFailed(err)
    }
}

impl From<bincode::Error> for Error {
    fn from(err: bincode::Error) -> Self {
        Error::ReceivedInvalidData(err)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn convert_error(err: Error) -> OperationError {
    let status = match err {
        Error::IOFailed(_) | Error::ReceivedInvalidData(_) => STATUS_INTERNAL_ERROR,
        Error::RequestFailed(err) => match err {
            responses::Error::FileNotFound => STATUS_OBJECT_NAME_NOT_FOUND,
            responses::Error::NoSuchHandle => STATUS_INVALID_HANDLE,
            responses::Error::FileExists => STATUS_OBJECT_NAME_COLLISION,
            responses::Error::PermissionDenied => STATUS_ACCESS_DENIED,
            responses::Error::DirectoryNotEmpty => STATUS_DIRECTORY_NOT_EMPTY,
            responses::Error::NotADirectory => STATUS_NOT_A_DIRECTORY,
            responses::Error::IsADirectory => STATUS_FILE_IS_A_DIRECTORY,
            responses::Error::CouldNotFindDisk => STATUS_DEVICE_NOT_READY,
            responses::Error::Other => STATUS_INTERNAL_ERROR,
        },
    };
    OperationError::NtStatus(status)
}

pub fn convert_response<T>(resp: Result<T>) -> std::result::Result<T, OperationError> {
    resp.map_err(convert_error)
}

pub struct Client {
    connection: Mutex<TcpStream>,
}

impl Client {
    pub fn new(tcp_stream: TcpStream) -> Client {
        let _ = tcp_stream.set_nodelay(true);
        Client {
            connection: Mutex::new(tcp_stream),
        }
    }

    fn send_keep_connection<T: DeserializeOwned>(
        &self,
        request: requests::Request,
    ) -> Result<(T, MutexGuard<'_, TcpStream>)> {
        let mut connection = self.connection.lock().unwrap();
        let encoded = bincode::serialize(&request)?;
        if encoded.len() as u64 > MAX_FRAME {
            return Err(Error::IOFailed(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "request is too large",
            )));
        }
        connection.write_u64::<BigEndian>(encoded.len() as u64)?;
        connection.write_all(&encoded)?;

        let length = connection.read_u64::<BigEndian>()?;
        if length > MAX_FRAME {
            return Err(Error::IOFailed(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "response is too large",
            )));
        }
        let mut buffer = vec![0u8; length as usize];
        connection.read_exact(&mut buffer)?;
        let deserialized: responses::Result<T> = bincode::deserialize(&buffer)?;
        let data = deserialized?;
        Ok((data, connection))
    }

    fn send<T: DeserializeOwned>(&self, request: requests::Request) -> Result<T> {
        Ok(self.send_keep_connection::<T>(request)?.0)
    }

    pub fn list_files(&self, path: &str) -> Result<responses::ListFiles> {
        self.send(requests::Request::List(path.to_string()))
    }

    pub fn get_free_space(&self) -> Result<responses::FreeSpace> {
        self.send(requests::Request::GetFreeSpace)
    }

    pub fn close_file(&self, handle: FileHandle) -> Result<()> {
        self.send(requests::Request::Close(handle))
    }

    pub fn delete_file(&self, path: String) -> Result<()> {
        self.send(requests::Request::Delete(path))
    }

    pub fn open_file(&self, path: String) -> Result<FileHandle> {
        self.send(requests::Request::Open(requests::OpenFile { path }))
    }

    pub fn create_file(&self, path: String, truncate: bool, exclusive: bool) -> Result<FileHandle> {
        self.send(requests::Request::CreateFile(requests::CreateFile {
            path,
            truncate,
            exclusive,
        }))
    }

    pub fn create_directory(&self, path: String, exclusive: bool) -> Result<()> {
        self.send(requests::Request::CreateDirectory(requests::CreateDirectory {
            path,
            exclusive,
        }))
    }

    pub fn move_file(&self, from: String, to: String, replace_if_exists: bool) -> Result<()> {
        self.send(requests::Request::Move(requests::MoveFile {
            from,
            to,
            replace_if_exists,
        }))
    }

    pub fn read_file(&self, handle: FileHandle, offset: u64, buffer: &mut [u8]) -> Result<u32> {
        let req = requests::Request::Read(requests::ReadFile {
            handle,
            offset,
            len: buffer.len() as u64,
        });
        let (length_read, mut connection) = self.send_keep_connection::<responses::ReadFile>(req)?;
        let length_read = length_read as usize;
        if length_read > buffer.len() {
            return Err(Error::IOFailed(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "daemon returned more bytes than requested",
            )));
        }
        if length_read > 0 {
            connection.read_exact(&mut buffer[..length_read])?;
        }
        Ok(length_read as u32)
    }

    pub fn write_file(&self, handle: FileHandle, offset: u64, data: &[u8]) -> Result<()> {
        let req = requests::Request::Write(requests::WriteFile {
            handle,
            offset,
            len: data.len() as u64,
        });
        let (_, mut connection) = self.send_keep_connection::<()>(req)?;
        connection.write_all(data)?;
        Ok(())
    }

    pub fn set_end_of_file(&self, handle: FileHandle, len: u64) -> Result<()> {
        self.send(requests::Request::SetEndOfFile(requests::SetEndOfFile {
            handle,
            len,
        }))
    }

    pub fn stat_file(&self, path: &str) -> Result<responses::StatFile> {
        self.send(requests::Request::Stat(path.to_string()))
    }
}
