use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::models::{FileHandle, FileInfo};
use crate::requests::{self, WRITE_AT_END};
use crate::responses::{self, Error};

const MAX_IO: u64 = 8 * 1024 * 1024;

struct OpenedFile {
    file: File,
    read_only: bool,
}

pub struct FsService {
    root: PathBuf,
    handles: std::collections::HashMap<FileHandle, OpenedFile>,
    next_handle: u32,
}

impl FsService {
    pub fn new(root: PathBuf) -> Self {
        FsService {
            root,
            handles: std::collections::HashMap::new(),
            next_handle: 1,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Prefer the shared storage users actually want in Explorer.
    /// `ANDROIDFS_ROOT` overrides detection. `/` is the fallback (full shell view).
    pub fn detect_root() -> PathBuf {
        if let Ok(explicit) = std::env::var("ANDROIDFS_ROOT") {
            let explicit = explicit.trim();
            if !explicit.is_empty() {
                return PathBuf::from(explicit);
            }
        }
        for candidate in ["/storage/emulated/0", "/sdcard", "/mnt/sdcard"] {
            if Path::new(candidate).is_dir() {
                return PathBuf::from(candidate);
            }
        }
        PathBuf::from("/")
    }

    pub fn resolve_path(root: &Path, raw: &str) -> responses::Result<PathBuf> {
        if raw.contains('\0') {
            return Err(Error::PermissionDenied);
        }
        let mut out = root.to_path_buf();
        for comp in raw.split(['/', '\\']) {
            if comp.is_empty() || comp == "." {
                continue;
            }
            if comp == ".." {
                return Err(Error::PermissionDenied);
            }
            out.push(comp);
        }
        Ok(out)
    }

    fn resolve(&self, raw: &str) -> responses::Result<PathBuf> {
        Self::resolve_path(&self.root, raw)
    }

    fn alloc_handle(&mut self) -> responses::Result<FileHandle> {
        let start = self.next_handle;
        loop {
            let id = self.next_handle;
            self.next_handle = self.next_handle.wrapping_add(1);
            if self.next_handle == 0 {
                self.next_handle = 1;
            }
            if !self.handles.contains_key(&id) {
                return Ok(id);
            }
            if self.next_handle == start {
                return Err(Error::Other);
            }
        }
    }

    fn insert_handle(&mut self, file: File, read_only: bool) -> responses::Result<FileHandle> {
        let id = self.alloc_handle()?;
        self.handles.insert(
            id,
            OpenedFile {
                file,
                read_only,
            },
        );
        Ok(id)
    }

    pub fn list_files(&self, raw: &str) -> responses::Result<Vec<FileInfo>> {
        let path = self.resolve(raw)?;
        let meta = fs::symlink_metadata(&path).map_err(to_response_error)?;
        if meta.file_type().is_symlink() {
            // Listing a symlink: follow one level so Explorer can open link-directories,
            // but only if the target stays... we already resolved the link path itself.
            // symlink_metadata says it's a link; use metadata() to see the target type.
        }
        if !meta.is_dir() && !fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false) {
            return Err(Error::NotADirectory);
        }
        let entries = fs::read_dir(&path).map_err(to_response_error)?;
        let mut files = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == "." || name == ".." {
                continue;
            }
            let meta = match fs::symlink_metadata(entry.path()) {
                Ok(meta) => meta,
                Err(_) => continue,
            };
            files.push(metadata_to_file_info(name, &meta));
        }
        files.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(files)
    }

    pub fn stat_file(&self, raw: &str) -> responses::Result<FileInfo> {
        let path = self.resolve(raw)?;
        let meta = fs::symlink_metadata(&path).map_err(to_response_error)?;
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/".to_string());
        Ok(metadata_to_file_info(name, &meta))
    }

    pub fn create_directory(&self, req: &requests::CreateDirectory) -> responses::Result<()> {
        let path = self.resolve(&req.path)?;
        if path == self.root {
            return Err(Error::PermissionDenied);
        }
        match fs::create_dir(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if !req.exclusive && path.is_dir() {
                    Ok(())
                } else if path.is_dir() {
                    Err(Error::FileExists)
                } else {
                    Err(Error::NotADirectory)
                }
            }
            Err(err) => Err(to_response_error(err)),
        }
    }

    pub fn create_file(&mut self, req: &requests::CreateFile) -> responses::Result<FileHandle> {
        let path = self.resolve(&req.path)?;
        if path == self.root {
            return Err(Error::IsADirectory);
        }
        if path.is_dir() {
            return Err(Error::IsADirectory);
        }
        let mut opts = OpenOptions::new();
        opts.read(true).write(true);
        if req.exclusive {
            opts.create_new(true);
        } else {
            opts.create(true).truncate(req.truncate);
        }
        match opts.open(&path) {
            Ok(file) => self.insert_handle(file, false),
            Err(err) => Err(to_response_error(err)),
        }
    }

    pub fn open_file(&mut self, req: &requests::OpenFile) -> responses::Result<FileHandle> {
        let path = self.resolve(&req.path)?;
        let meta = fs::symlink_metadata(&path).map_err(to_response_error)?;
        if meta.is_dir() {
            return Err(Error::IsADirectory);
        }
        match OpenOptions::new().read(true).write(true).open(&path) {
            Ok(file) => self.insert_handle(file, false),
            Err(write_err) => match OpenOptions::new().read(true).open(&path) {
                Ok(file) => self.insert_handle(file, true),
                Err(_) => Err(to_response_error(write_err)),
            },
        }
    }

    pub fn close(&mut self, handle: FileHandle) -> responses::Result<()> {
        match self.handles.remove(&handle) {
            Some(_) => Ok(()),
            None => Err(Error::NoSuchHandle),
        }
    }

    pub fn delete(&self, raw: &str) -> responses::Result<()> {
        let path = self.resolve(raw)?;
        if path == self.root {
            return Err(Error::PermissionDenied);
        }
        let meta = fs::symlink_metadata(&path).map_err(to_response_error)?;
        let result = if meta.is_dir() {
            fs::remove_dir(&path)
        } else {
            fs::remove_file(&path)
        };
        result.map_err(to_response_error)
    }

    pub fn move_file(&self, req: &requests::MoveFile) -> responses::Result<()> {
        let from = self.resolve(&req.from)?;
        let to = self.resolve(&req.to)?;
        if from == self.root || to == self.root {
            return Err(Error::PermissionDenied);
        }
        if to.exists() {
            if !req.replace_if_exists {
                return Err(Error::FileExists);
            }
            let meta = fs::symlink_metadata(&to).map_err(to_response_error)?;
            let rm = if meta.is_dir() {
                fs::remove_dir(&to)
            } else {
                fs::remove_file(&to)
            };
            rm.map_err(to_response_error)?;
        }
        fs::rename(&from, &to).map_err(to_response_error)
    }

    pub fn read_at(&mut self, handle: FileHandle, offset: u64, len: u64) -> responses::Result<Vec<u8>> {
        if len > MAX_IO {
            return Err(Error::Other);
        }
        let opened = self.handles.get_mut(&handle).ok_or(Error::NoSuchHandle)?;
        let file_len = opened.file.metadata().map_err(to_response_error)?.len();
        if offset >= file_len || len == 0 {
            return Ok(Vec::new());
        }
        let to_read = std::cmp::min(len, file_len - offset) as usize;
        opened
            .file
            .seek(SeekFrom::Start(offset))
            .map_err(to_response_error)?;
        let mut buf = vec![0u8; to_read];
        let mut filled = 0;
        while filled < to_read {
            match opened.file.read(&mut buf[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(to_response_error(err)),
            }
        }
        buf.truncate(filled);
        Ok(buf)
    }

    pub fn write_at(&mut self, handle: FileHandle, offset: u64, data: &[u8]) -> responses::Result<u32> {
        if data.len() as u64 > MAX_IO {
            return Err(Error::Other);
        }
        let opened = self.handles.get_mut(&handle).ok_or(Error::NoSuchHandle)?;
        if opened.read_only {
            return Err(Error::PermissionDenied);
        }
        let seek = if offset == WRITE_AT_END {
            SeekFrom::End(0)
        } else {
            SeekFrom::Start(offset)
        };
        opened.file.seek(seek).map_err(to_response_error)?;
        opened.file.write_all(data).map_err(to_response_error)?;
        Ok(data.len() as u32)
    }

    pub fn set_end_of_file(&mut self, handle: FileHandle, len: u64) -> responses::Result<()> {
        let opened = self.handles.get_mut(&handle).ok_or(Error::NoSuchHandle)?;
        if opened.read_only {
            return Err(Error::PermissionDenied);
        }
        opened.file.set_len(len).map_err(to_response_error)
    }

    pub fn free_space(&self) -> responses::Result<responses::FreeSpace> {
        disk_space(&self.root)
            .or_else(|| disk_space(Path::new("/storage/emulated/0")))
            .or_else(|| disk_space(Path::new("/")))
            .ok_or(Error::CouldNotFindDisk)
    }
}

fn metadata_to_file_info(name: String, metadata: &fs::Metadata) -> FileInfo {
    use std::os::unix::fs::MetadataExt;
    FileInfo {
        name,
        size: metadata.len(),
        last_accessed: unwrap_or_epoch(metadata.accessed()),
        last_modified: unwrap_or_epoch(metadata.modified()),
        creation_time: unwrap_or_epoch(metadata.created()),
        mode: metadata.mode(),
        ino: metadata.ino(),
    }
}

fn unwrap_or_epoch(result: std::io::Result<SystemTime>) -> SystemTime {
    result.unwrap_or(SystemTime::UNIX_EPOCH)
}

pub fn to_response_error(err: std::io::Error) -> Error {
    use std::io::ErrorKind;
    match err.kind() {
        ErrorKind::NotFound => Error::FileNotFound,
        ErrorKind::AlreadyExists => Error::FileExists,
        ErrorKind::PermissionDenied => Error::PermissionDenied,
        ErrorKind::IsADirectory => Error::IsADirectory,
        ErrorKind::NotADirectory => Error::NotADirectory,
        ErrorKind::DirectoryNotEmpty => Error::DirectoryNotEmpty,
        ErrorKind::ReadOnlyFilesystem => Error::PermissionDenied,
        _ => match err.raw_os_error() {
            Some(2) => Error::FileNotFound,
            Some(1) | Some(13) | Some(30) => Error::PermissionDenied,
            Some(17) => Error::FileExists,
            Some(21) => Error::IsADirectory,
            Some(20) => Error::NotADirectory,
            Some(39) => Error::DirectoryNotEmpty,
            _ => Error::Other,
        },
    }
}

fn disk_space(path: &Path) -> Option<responses::FreeSpace> {
    use std::os::unix::ffi::OsStrExt;
    let mut bytes = path.as_os_str().as_bytes().to_vec();
    if bytes.contains(&0) {
        return None;
    }
    bytes.push(0);
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(bytes.as_ptr() as *const libc::c_char, &mut st) != 0 {
            return None;
        }
        let unit = if st.f_frsize == 0 {
            st.f_bsize as u64
        } else {
            st.f_frsize as u64
        };
        if unit == 0 {
            return None;
        }
        Some(responses::FreeSpace {
            total_bytes: st.f_blocks as u64 * unit,
            free_bytes: st.f_bavail as u64 * unit,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_root() -> PathBuf {
        let n = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("androidfs-{}-{}", std::process::id(), n));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn resolve_rejects_parent_escape() {
        let root = Path::new("/storage/emulated/0");
        let ok = FsService::resolve_path(root, "/DCIM/Camera").unwrap();
        assert_eq!(ok, PathBuf::from("/storage/emulated/0/DCIM/Camera"));
        assert!(matches!(
            FsService::resolve_path(root, "/DCIM/../../data"),
            Err(Error::PermissionDenied)
        ));
        assert_eq!(
            FsService::resolve_path(root, "/").unwrap(),
            PathBuf::from("/storage/emulated/0")
        );
    }

    #[test]
    fn create_write_read_truncate_delete() {
        let root = temp_root();
        let mut svc = FsService::new(root.clone());

        let handle = svc
            .create_file(&requests::CreateFile {
                path: "/notes/nope.txt".into(),
                truncate: false,
                exclusive: true,
            })
            .unwrap_err();
        assert_eq!(handle, Error::FileNotFound);

        svc.create_directory(&requests::CreateDirectory {
            path: "/notes".into(),
            exclusive: true,
        })
        .unwrap();

        let h = svc
            .create_file(&requests::CreateFile {
                path: "/notes/a.txt".into(),
                truncate: false,
                exclusive: true,
            })
            .unwrap();
        assert_eq!(svc.write_at(h, 0, b"hello").unwrap(), 5);
        svc.close(h).unwrap();

        let again = svc
            .create_file(&requests::CreateFile {
                path: "/notes/a.txt".into(),
                truncate: false,
                exclusive: true,
            })
            .unwrap_err();
        assert_eq!(again, Error::FileExists);

        let h = svc
            .open_file(&requests::OpenFile {
                path: "/notes/a.txt".into(),
            })
            .unwrap();
        assert_eq!(svc.read_at(h, 0, 64).unwrap(), b"hello");
        assert!(svc.read_at(h, 100, 10).unwrap().is_empty());
        svc.write_at(h, WRITE_AT_END, b"!").unwrap();
        svc.set_end_of_file(h, 4).unwrap();
        assert_eq!(svc.read_at(h, 0, 64).unwrap(), b"hell");
        svc.close(h).unwrap();

        let listed = svc.list_files("/notes").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "a.txt");
        assert_eq!(listed[0].size, 4);
        assert!(!crate::models::is_dir_mode(listed[0].mode));

        let stat = svc.stat_file("/notes").unwrap();
        assert!(crate::models::is_dir_mode(stat.mode));

        svc.move_file(&requests::MoveFile {
            from: "/notes/a.txt".into(),
            to: "/notes/b.txt".into(),
            replace_if_exists: false,
        })
        .unwrap();
        assert!(matches!(svc.stat_file("/notes/a.txt"), Err(Error::FileNotFound)));
        svc.delete("/notes/b.txt").unwrap();
        assert_eq!(svc.delete("/notes").unwrap(), ());

        let space = svc.free_space().unwrap();
        assert!(space.total_bytes > 0);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn refuses_to_delete_mount_root_and_nonempty_dir() {
        let root = temp_root();
        let mut svc = FsService::new(root.clone());
        assert_eq!(svc.delete("/"), Err(Error::PermissionDenied));
        svc.create_directory(&requests::CreateDirectory {
            path: "/keep".into(),
            exclusive: true,
        })
        .unwrap();
        svc.create_file(&requests::CreateFile {
            path: "/keep/x".into(),
            truncate: false,
            exclusive: true,
        })
        .unwrap();
        assert_eq!(svc.delete("/keep"), Err(Error::DirectoryNotEmpty));
        let _ = fs::remove_dir_all(&root);
    }
}
