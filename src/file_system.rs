use std::time::Duration;

use dokan::*;
use log::error;
use widestring::{U16CStr, U16CString};
use winapi::shared::ntstatus::*;
use winapi::um::winnt::{
    self, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
    FILE_ATTRIBUTE_RECALL_ON_OPEN, PSECURITY_DESCRIPTOR,
};

use crate::cache::Cache;
use crate::client::{self, Client};
use crate::models::{self, FileHandle};
use crate::requests::WRITE_AT_END;

const FILE_SUPERSEDE: u32 = 0;
const FILE_OPEN: u32 = 1;
const FILE_CREATE: u32 = 2;
const FILE_OPEN_IF: u32 = 3;
const FILE_OVERWRITE: u32 = 4;
const FILE_OVERWRITE_IF: u32 = 5;

const FILE_DIRECTORY_FILE: u32 = 0x0000_0001;
const FILE_NON_DIRECTORY_FILE: u32 = 0x0000_0040;

pub struct QuestFsHandler {
    volume_name: U16CString,
    client: Client,
    directory_cache: Cache<String, Vec<models::FileInfo>>,
    stat_cache: Cache<String, models::FileInfo>,
}

fn nt(status: i32) -> OperationError {
    OperationError::NtStatus(status)
}

fn convert_file_name(win_file_name: &U16CStr) -> String {
    let raw = win_file_name.to_string_lossy().replace('\\', "/");
    let mut parts: Vec<&str> = Vec::new();
    for comp in raw.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp == ".." {
            parts.pop();
            continue;
        }
        parts.push(comp);
    }
    if parts.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", parts.join("/"))
    }
}

fn parent_path(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        return None;
    }
    match trimmed.rfind('/') {
        Some(0) | None => Some("/".to_string()),
        Some(index) => Some(trimmed[..index].to_string()),
    }
}

fn wants_directory(create_options: u32, file_attributes: u32) -> bool {
    if create_options & FILE_NON_DIRECTORY_FILE != 0 {
        return false;
    }
    create_options & FILE_DIRECTORY_FILE != 0
        || file_attributes & FILE_ATTRIBUTE_DIRECTORY != 0
}

fn convert_attributes(file: &models::FileInfo) -> u32 {
    if models::is_dir_mode(file.mode) {
        // Plain directory. The "recall" flags mark a cloud placeholder that still
        // has to be downloaded; on a folder they make Explorer treat it as
        // offline content, and a reparse-point flag on a folder that is not a
        // real reparse point makes it unreadable.
        return FILE_ATTRIBUTE_DIRECTORY;
    }
    // Files keep the recall flags: Windows then does not read file contents just
    // to draw thumbnails or scan them, which would be very slow over ADB.
    // (Symlinks are already resolved by the daemon, so there is no reparse flag.)
    FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS | FILE_ATTRIBUTE_RECALL_ON_OPEN
}

impl QuestFsHandler {
    pub fn new(client: Client, volume_name: U16CString) -> Self {
        QuestFsHandler {
            volume_name,
            client,
            directory_cache: Cache::new(Duration::from_secs(10), 128),
            stat_cache: Cache::new(Duration::from_secs(3), 1024),
        }
    }

    fn stat_cached(&self, path: &str) -> client::Result<models::FileInfo> {
        if let Some(hit) = self.stat_cache.try_get(&path.to_string()) {
            return Ok(hit);
        }
        let info = self.client.stat_file(path)?;
        self.stat_cache.put(path.to_string(), info.clone());
        Ok(info)
    }

    fn invalidate(&self, path: &str) {
        self.stat_cache.erase(&path.to_string());
        if let Some(parent) = parent_path(path) {
            self.directory_cache.erase(&parent);
            self.stat_cache.erase(&parent);
        }
    }

    fn open_existing(
        &self,
        path: String,
        info: models::FileInfo,
        create_disposition: u32,
        create_options: u32,
    ) -> Result<CreateFileInfo<FileHandle>, OperationError> {
        let is_dir = models::is_dir_mode(info.mode);
        if is_dir && create_options & FILE_NON_DIRECTORY_FILE != 0 {
            return Err(nt(STATUS_FILE_IS_A_DIRECTORY));
        }
        if !is_dir && create_options & FILE_DIRECTORY_FILE != 0 {
            return Err(nt(STATUS_NOT_A_DIRECTORY));
        }

        match create_disposition {
            FILE_CREATE => Err(nt(STATUS_OBJECT_NAME_COLLISION)),
            FILE_OPEN | FILE_OPEN_IF => {
                if is_dir {
                    Ok(CreateFileInfo {
                        context: 0,
                        is_dir: true,
                        new_file_created: false,
                    })
                } else {
                    let handle = client::convert_response(self.client.open_file(path))?;
                    Ok(CreateFileInfo {
                        context: handle,
                        is_dir: false,
                        new_file_created: false,
                    })
                }
            }
            FILE_SUPERSEDE | FILE_OVERWRITE | FILE_OVERWRITE_IF => {
                if is_dir {
                    return Err(nt(STATUS_FILE_IS_A_DIRECTORY));
                }
                let handle =
                    client::convert_response(self.client.create_file(path.clone(), true, false))?;
                self.invalidate(&path);
                Ok(CreateFileInfo {
                    context: handle,
                    is_dir: false,
                    new_file_created: false,
                })
            }
            _ => Err(nt(STATUS_INVALID_PARAMETER)),
        }
    }

    fn create_missing(
        &self,
        path: String,
        create_disposition: u32,
        want_dir: bool,
    ) -> Result<CreateFileInfo<FileHandle>, OperationError> {
        match create_disposition {
            FILE_OPEN | FILE_OVERWRITE => Err(nt(STATUS_OBJECT_NAME_NOT_FOUND)),
            FILE_CREATE | FILE_OPEN_IF | FILE_OVERWRITE_IF | FILE_SUPERSEDE => {
                if want_dir {
                    let exclusive = create_disposition == FILE_CREATE;
                    client::convert_response(self.client.create_directory(path.clone(), exclusive))?;
                    self.invalidate(&path);
                    Ok(CreateFileInfo {
                        context: 0,
                        is_dir: true,
                        new_file_created: true,
                    })
                } else {
                    let exclusive = create_disposition == FILE_CREATE;
                    let truncate = matches!(
                        create_disposition,
                        FILE_OVERWRITE_IF | FILE_SUPERSEDE | FILE_OVERWRITE
                    );
                    let handle = client::convert_response(self.client.create_file(
                        path.clone(),
                        truncate,
                        exclusive,
                    ))?;
                    self.invalidate(&path);
                    Ok(CreateFileInfo {
                        context: handle,
                        is_dir: false,
                        new_file_created: true,
                    })
                }
            }
            _ => Err(nt(STATUS_INVALID_PARAMETER)),
        }
    }
}

impl<'a, 'b: 'a> FileSystemHandler<'a, 'b> for QuestFsHandler {
    type Context = FileHandle;

    fn create_file(
        &'b self,
        win_file_name: &U16CStr,
        _security_context: &DOKAN_IO_SECURITY_CONTEXT,
        _desired_access: winnt::ACCESS_MASK,
        file_attributes: u32,
        _share_access: u32,
        create_disposition: u32,
        create_options: u32,
        _info: &mut OperationInfo<'a, 'b, Self>,
    ) -> Result<CreateFileInfo<Self::Context>, OperationError> {
        let path = convert_file_name(win_file_name);
        let want_dir = wants_directory(create_options, file_attributes);

        if path == "/" {
            if create_options & FILE_NON_DIRECTORY_FILE != 0 {
                return Err(nt(STATUS_FILE_IS_A_DIRECTORY));
            }
            if create_disposition != FILE_OPEN && create_disposition != FILE_OPEN_IF {
                return Err(nt(STATUS_ACCESS_DENIED));
            }
            return Ok(CreateFileInfo {
                context: 0,
                is_dir: true,
                new_file_created: false,
            });
        }

        match self.stat_cached(&path) {
            Ok(info) => self.open_existing(path, info, create_disposition, create_options),
            Err(client::Error::RequestFailed(crate::responses::Error::FileNotFound)) => {
                self.create_missing(path, create_disposition, want_dir)
            }
            Err(err) => Err(client::convert_error(err)),
        }
    }

    fn cleanup(
        &'b self,
        win_file_name: &U16CStr,
        info: &OperationInfo<'a, 'b, Self>,
        _context: &'a Self::Context,
    ) {
        if !info.delete_on_close() {
            return;
        }
        let path = convert_file_name(win_file_name);
        if path == "/" {
            return;
        }
        match self.client.delete_file(path.clone()) {
            Ok(()) => self.invalidate(&path),
            Err(err) => error!("delete-on-close failed for {path}: {err}"),
        }
    }

    fn close_file(
        &'b self,
        _file_name: &U16CStr,
        _info: &OperationInfo<'a, 'b, Self>,
        context: &'a Self::Context,
    ) {
        if *context == 0 {
            return;
        }
        if let Err(err) = self.client.close_file(*context) {
            error!("could not close handle {context}: {err}");
        }
    }

    fn read_file(
        &'b self,
        _: &U16CStr,
        offset: i64,
        buffer: &mut [u8],
        _info: &OperationInfo<'a, 'b, Self>,
        context: &'a Self::Context,
    ) -> Result<u32, OperationError> {
        if *context == 0 {
            return Err(nt(STATUS_INVALID_DEVICE_REQUEST));
        }
        let offset = if offset < 0 { 0 } else { offset as u64 };
        client::convert_response(self.client.read_file(*context, offset, buffer))
    }

    fn write_file(
        &'b self,
        win_file_name: &U16CStr,
        offset: i64,
        buffer: &[u8],
        info: &OperationInfo<'a, 'b, Self>,
        context: &'a Self::Context,
    ) -> Result<u32, OperationError> {
        if *context == 0 {
            return Err(nt(STATUS_INVALID_DEVICE_REQUEST));
        }
        let offset = if info.write_to_eof() {
            WRITE_AT_END
        } else if offset < 0 {
            0
        } else {
            offset as u64
        };
        client::convert_response(self.client.write_file(*context, offset, buffer))?;
        self.invalidate(&convert_file_name(win_file_name));
        Ok(buffer.len() as u32)
    }

    fn flush_file_buffers(
        &'b self,
        _file_name: &U16CStr,
        _info: &OperationInfo<'a, 'b, Self>,
        _context: &'a Self::Context,
    ) -> Result<(), OperationError> {
        Ok(())
    }

    fn get_file_information(
        &'b self,
        win_file_name: &U16CStr,
        _info: &OperationInfo<'a, 'b, Self>,
        _context: &'a Self::Context,
    ) -> Result<FileInfo, OperationError> {
        let path = convert_file_name(win_file_name);
        let file_info = client::convert_response(self.stat_cached(&path))?;
        Ok(FileInfo {
            attributes: convert_attributes(&file_info),
            creation_time: file_info.creation_time,
            last_access_time: file_info.last_accessed,
            last_write_time: file_info.last_modified,
            file_size: file_info.size,
            number_of_links: 1,
            file_index: file_info.ino,
        })
    }

    fn find_files(
        &'b self,
        win_file_name: &U16CStr,
        mut fill_find_data: impl FnMut(&FindData) -> Result<(), FillDataError>,
        _info: &OperationInfo<'a, 'b, Self>,
        _context: &'a Self::Context,
    ) -> Result<(), OperationError> {
        let path = convert_file_name(win_file_name);
        let files = if let Some(cached) = self.directory_cache.try_get(&path) {
            cached
        } else {
            let listed = client::convert_response(self.client.list_files(&path))?;
            self.directory_cache.put(path, listed.clone());
            listed
        };

        for file in &files {
            let Ok(file_name) = U16CString::from_str(file.name.as_str()) else {
                continue;
            };
            let result = fill_find_data(&FindData {
                attributes: convert_attributes(file),
                creation_time: file.creation_time,
                last_access_time: file.last_accessed,
                last_write_time: file.last_modified,
                file_size: file.size,
                file_name,
            });
            match result {
                Ok(()) => {}
                // One over-long name must not abort the whole listing
                // (the `?` operator would turn it into an error for the folder).
                Err(FillDataError::NameTooLong) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }

    fn set_file_attributes(
        &'b self,
        _file_name: &U16CStr,
        _file_attributes: u32,
        _info: &OperationInfo<'a, 'b, Self>,
        _context: &'a Self::Context,
    ) -> Result<(), OperationError> {
        Ok(())
    }

    fn set_file_time(
        &'b self,
        _file_name: &U16CStr,
        _creation_time: FileTimeInfo,
        _last_access_time: FileTimeInfo,
        _last_write_time: FileTimeInfo,
        _info: &OperationInfo<'a, 'b, Self>,
        _context: &'a Self::Context,
    ) -> Result<(), OperationError> {
        Ok(())
    }

    fn delete_file(
        &'b self,
        win_file_name: &U16CStr,
        _info: &OperationInfo<'a, 'b, Self>,
        _context: &'a Self::Context,
    ) -> Result<(), OperationError> {
        let path = convert_file_name(win_file_name);
        let info = client::convert_response(self.stat_cached(&path))?;
        if models::is_dir_mode(info.mode) {
            return Err(nt(STATUS_FILE_IS_A_DIRECTORY));
        }
        Ok(())
    }

    fn delete_directory(
        &'b self,
        win_file_name: &U16CStr,
        info: &OperationInfo<'a, 'b, Self>,
        _context: &'a Self::Context,
    ) -> Result<(), OperationError> {
        let path = convert_file_name(win_file_name);
        if path == "/" {
            return Err(nt(STATUS_ACCESS_DENIED));
        }
        let stat = client::convert_response(self.stat_cached(&path))?;
        if !models::is_dir_mode(stat.mode) {
            return Err(nt(STATUS_NOT_A_DIRECTORY));
        }
        if info.delete_on_close() {
            let children = client::convert_response(self.client.list_files(&path))?;
            if !children.is_empty() {
                return Err(nt(STATUS_DIRECTORY_NOT_EMPTY));
            }
        }
        Ok(())
    }

    fn move_file(
        &'b self,
        file_name: &U16CStr,
        new_file_name: &U16CStr,
        replace_if_existing: bool,
        _info: &OperationInfo<'a, 'b, Self>,
        _context: &'a Self::Context,
    ) -> Result<(), OperationError> {
        let from = convert_file_name(file_name);
        let to = convert_file_name(new_file_name);
        let result = client::convert_response(self.client.move_file(from.clone(), to.clone(), replace_if_existing));
        self.invalidate(&from);
        self.invalidate(&to);
        result
    }

    fn set_end_of_file(
        &'b self,
        win_file_name: &U16CStr,
        offset: i64,
        _info: &OperationInfo<'a, 'b, Self>,
        context: &'a Self::Context,
    ) -> Result<(), OperationError> {
        if *context == 0 {
            return Err(nt(STATUS_INVALID_DEVICE_REQUEST));
        }
        let len = if offset < 0 { 0 } else { offset as u64 };
        client::convert_response(self.client.set_end_of_file(*context, len))?;
        self.invalidate(&convert_file_name(win_file_name));
        Ok(())
    }

    fn set_allocation_size(
        &'b self,
        _file_name: &U16CStr,
        _alloc_size: i64,
        _info: &OperationInfo<'a, 'b, Self>,
        _context: &'a Self::Context,
    ) -> Result<(), OperationError> {
        Ok(())
    }

    fn get_disk_free_space(
        &'b self,
        _info: &OperationInfo<'a, 'b, Self>,
    ) -> Result<DiskSpaceInfo, OperationError> {
        let free_space = client::convert_response(self.client.get_free_space())?;
        Ok(DiskSpaceInfo {
            byte_count: free_space.total_bytes,
            free_byte_count: free_space.free_bytes,
            available_byte_count: free_space.free_bytes,
        })
    }

    fn get_volume_information(
        &'b self,
        _info: &OperationInfo<'a, 'b, Self>,
    ) -> Result<VolumeInfo, OperationError> {
        Ok(VolumeInfo {
            name: self.volume_name.clone(),
            serial_number: 0x4146_5321,
            max_component_length: 255,
            fs_flags: winnt::FILE_CASE_PRESERVED_NAMES
                | winnt::FILE_CASE_SENSITIVE_SEARCH
                | winnt::FILE_UNICODE_ON_DISK,
            fs_name: U16CString::from_str("AndroidFS").unwrap(),
        })
    }

    fn get_file_security(
        &'b self,
        _file_name: &U16CStr,
        _security_information: u32,
        _security_descriptor: PSECURITY_DESCRIPTOR,
        _buffer_length: u32,
        _info: &OperationInfo<'a, 'b, Self>,
        _context: &'a Self::Context,
    ) -> Result<u32, OperationError> {
        Err(nt(STATUS_NOT_IMPLEMENTED))
    }

    fn set_file_security(
        &'b self,
        _file_name: &U16CStr,
        _security_information: u32,
        _security_descriptor: PSECURITY_DESCRIPTOR,
        _buffer_length: u32,
        _info: &OperationInfo<'a, 'b, Self>,
        _context: &'a Self::Context,
    ) -> Result<(), OperationError> {
        Err(nt(STATUS_NOT_IMPLEMENTED))
    }

    fn find_streams(
        &'b self,
        _file_name: &U16CStr,
        _fill_find_stream_data: impl FnMut(&FindStreamData) -> Result<(), FillDataError>,
        _info: &OperationInfo<'a, 'b, Self>,
        _context: &'a Self::Context,
    ) -> Result<(), OperationError> {
        Err(nt(STATUS_NOT_IMPLEMENTED))
    }
}
