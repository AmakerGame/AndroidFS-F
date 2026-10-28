use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use std::os::unix::fs::MetadataExt;

use crate::models::{self, FileHandle, FileInfo};
use crate::requests::{self, WRITE_AT_END};
use crate::responses::{self, Error};

const MAX_IO: u64 = 8 * 1024 * 1024;

/// Shared-storage folders on different Android versions and TV boxes.
const ROOT_CANDIDATES: &[&str] = &[
    "/storage/emulated/0",
    "/sdcard",
    "/storage/self/primary",
    "/mnt/sdcard",
    "/storage/emulated/legacy",
    "/storage/sdcard0",
    "/mnt/shell/emulated/0",
    "/mnt/runtime/default/emulated/0",
    "/data/media/0",
];

/// The folder that is served, and why it was chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootChoice {
    pub path: PathBuf,
    /// The requested folder could not be read, so another one was picked.
    pub fallback: bool,
    pub note: Option<String>,
}

pub fn can_list(path: &Path) -> bool {
    fs::read_dir(path).is_ok()
}

/// Entry count of a folder, `None` when it cannot be listed.
pub fn count_entries(path: &Path) -> Option<u32> {
    fs::read_dir(path)
        .ok()
        .map(|entries| entries.filter(|entry| entry.is_ok()).count() as u32)
}

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

    /// First readable folder that holds the user's shared storage. The list
    /// covers phones (Android 4 to 15) and TV boxes; `ANDROIDFS_ROOT` overrides
    /// detection. `/` is the last resort (whole filesystem).
    pub fn detect_root() -> PathBuf {
        if let Ok(explicit) = std::env::var("ANDROIDFS_ROOT") {
            let explicit = explicit.trim();
            if !explicit.is_empty() {
                return PathBuf::from(explicit);
            }
        }
        // `read_dir`, not `is_dir`: a folder that exists but may not be listed
        // by this user (permission denied) must be skipped, otherwise the drive
        // mounts fine and Explorer shows it empty.
        ROOT_CANDIDATES
            .iter()
            .map(Path::new)
            .find(|candidate| can_list(candidate))
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"))
    }

    /// Picks the folder to serve. `requested` is what the user configured
    /// (`None` = auto-detect). An unreadable request falls back to detection and
    /// says so, so the host can retry as root or tell the user what happened.
    /// First SD card or USB drive in `dir` (normally `/storage`): a readable
    /// folder that is not the internal storage or one of its aliases.
    pub fn find_removable_in(dir: &Path) -> Option<PathBuf> {
        let mut found: Vec<PathBuf> = fs::read_dir(dir)
            .ok()?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                !name.is_empty()
                    && !name.starts_with('.')
                    && !matches!(name, "emulated" | "self" | "sdcard0" | "knox-emulated")
                    && path.is_dir()
                    && can_list(path)
            })
            .collect();
        found.sort();
        found.into_iter().next()
    }

    pub fn choose_root(requested: Option<&Path>) -> RootChoice {
        if requested == Some(Path::new(crate::daemon_cmd::EXTERNAL_ROOT)) {
            return match Self::find_removable_in(Path::new("/storage")) {
                Some(path) => RootChoice {
                    path,
                    fallback: false,
                    note: None,
                },
                None => {
                    let path = Self::detect_root();
                    RootChoice {
                        path,
                        fallback: true,
                        note: Some("no SD card or USB drive found; showing internal storage".into()),
                    }
                }
            };
        }
        if let Some(requested) = requested {
            return match fs::read_dir(requested) {
                Ok(_) => RootChoice {
                    path: requested.to_path_buf(),
                    fallback: false,
                    note: None,
                },
                Err(err) => {
                    let path = Self::detect_root();
                    let note = format!(
                        "cannot read {}: {err}; showing {} instead",
                        requested.display(),
                        path.display()
                    );
                    RootChoice {
                        path,
                        fallback: true,
                        note: Some(note),
                    }
                }
            };
        }
        let path = Self::detect_root();
        if path == Path::new("/") && !can_list(Path::new("/storage/emulated/0")) {
            RootChoice {
                path,
                fallback: true,
                note: Some(
                    "shared storage is not readable by this user; showing the whole filesystem"
                        .to_string(),
                ),
            }
        } else {
            RootChoice {
                path,
                fallback: false,
                note: None,
            }
        }
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
        let meta = stat_follow(&path).map_err(to_response_error)?;
        if !meta.is_dir() {
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
            let info = match stat_follow(&entry.path()) {
                Ok(meta) => {
                    let mode = meta.mode();
                    // Sockets, FIFOs and device nodes (common in `/`) would hang
                    // or confuse Explorer; only folders and regular files are shown.
                    if !models::is_dir_mode(mode) && !models::is_regular_mode(mode) {
                        continue;
                    }
                    metadata_to_file_info(name, &meta)
                }
                // Cannot stat (typically permission denied on /data/*, Android/data):
                // still show it, so the listing is not mysteriously empty.
                Err(_) => match entry.file_type() {
                    Ok(kind) if kind.is_dir() => placeholder_info(name, true),
                    Ok(kind) if kind.is_file() => placeholder_info(name, false),
                    _ => continue, // broken symlink or special file
                },
            };
            files.push(info);
        }
        files.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(files)
    }

    pub fn stat_file(&self, raw: &str) -> responses::Result<FileInfo> {
        let path = self.resolve(raw)?;
        let meta = stat_follow(&path).map_err(to_response_error)?;
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
        let meta = stat_follow(&path).map_err(to_response_error)?;
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
        // The root of the drive may be `/` or `/storage` (a tiny read-only or
        // tmpfs mount). Reporting that would make Windows believe the drive is
        // full and refuse every copy, so measure the real storage instead.
        let tiny_root = matches!(self.root.to_str(), Some("/" | "/storage" | "/mnt"));
        let mut candidates: Vec<PathBuf> = Vec::new();
        if !tiny_root {
            candidates.push(self.root.clone());
        }
        candidates.extend(ROOT_CANDIDATES.iter().map(PathBuf::from));
        candidates.push(PathBuf::from("/data"));
        candidates.push(self.root.clone());
        candidates
            .iter()
            .find_map(|path| disk_space(path))
            .ok_or(Error::CouldNotFindDisk)
    }

    pub fn set_times(&self, req: &requests::SetTimes) -> responses::Result<()> {
        use std::os::unix::ffi::OsStrExt;
        let path = self.resolve(&req.path)?;
        if path == self.root {
            return Ok(());
        }
        let to_timespec = |time: Option<SystemTime>| match time {
            None => libc::timespec {
                tv_sec: 0,
                tv_nsec: libc::UTIME_OMIT as _,
            },
            Some(time) => {
                let since = time
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default();
                libc::timespec {
                    tv_sec: since.as_secs() as _,
                    tv_nsec: since.subsec_nanos() as _,
                }
            }
        };
        let times = [to_timespec(req.accessed), to_timespec(req.modified)];
        let mut bytes = path.as_os_str().as_bytes().to_vec();
        bytes.push(0);
        let result = unsafe {
            libc::utimensat(
                libc::AT_FDCWD,
                bytes.as_ptr() as *const libc::c_char,
                times.as_ptr(),
                0,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(to_response_error(std::io::Error::last_os_error()))
        }
    }
}

/// Metadata that follows symlinks (so a linked directory looks like a directory
/// in Explorer), falling back to the link itself when its target is missing.
fn stat_follow(path: &Path) -> std::io::Result<fs::Metadata> {
    fs::metadata(path).or_else(|_| fs::symlink_metadata(path))
}

fn placeholder_info(name: String, is_dir: bool) -> FileInfo {
    FileInfo {
        name,
        size: 0,
        last_accessed: SystemTime::UNIX_EPOCH,
        last_modified: SystemTime::UNIX_EPOCH,
        creation_time: SystemTime::UNIX_EPOCH,
        mode: if is_dir { 0o040_555 } else { 0o100_444 },
        ino: 0,
    }
}

fn metadata_to_file_info(name: String, metadata: &fs::Metadata) -> FileInfo {
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

    #[test]
    fn finds_removable_storage() {
        let dir = temp_root();
        for name in ["emulated", "self", "1A2B-3C4D", "sdcard0"] {
            fs::create_dir(dir.join(name)).unwrap();
        }
        assert_eq!(FsService::find_removable_in(&dir), Some(dir.join("1A2B-3C4D")));
        let empty = temp_root();
        assert_eq!(FsService::find_removable_in(&empty), None);
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&empty);
    }

    #[test]
    fn unreadable_root_falls_back_with_a_note() {
        let root = temp_root();
        let ok = FsService::choose_root(Some(&root));
        assert_eq!(ok.path, root);
        assert!(!ok.fallback);
        assert!(ok.note.is_none());

        let missing = root.join("does-not-exist");
        let choice = FsService::choose_root(Some(&missing));
        assert!(choice.fallback);
        assert_ne!(choice.path, missing);
        assert!(choice.note.unwrap().contains("does-not-exist"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn listing_skips_special_files_and_broken_links() {
        let root = temp_root();
        fs::create_dir(root.join("dir")).unwrap();
        fs::write(root.join("file.txt"), b"abc").unwrap();
        std::os::unix::fs::symlink(root.join("nowhere"), root.join("broken")).unwrap();
        std::os::unix::fs::symlink(root.join("dir"), root.join("link-to-dir")).unwrap();
        let svc = FsService::new(root.clone());
        let names: Vec<String> = svc
            .list_files("/")
            .unwrap()
            .into_iter()
            .map(|info| info.name)
            .collect();
        assert_eq!(names, vec!["dir", "file.txt", "link-to-dir"]);
        assert_eq!(count_entries(&root), Some(4));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn set_times_changes_modification_time() {
        let root = temp_root();
        fs::write(root.join("photo.jpg"), b"x").unwrap();
        let svc = FsService::new(root.clone());
        let when = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_400_000_000);
        svc.set_times(&requests::SetTimes {
            path: "/photo.jpg".into(),
            accessed: None,
            modified: Some(when),
        })
        .unwrap();
        let info = svc.stat_file("/photo.jpg").unwrap();
        assert_eq!(info.last_modified, when);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn free_space_ignores_tiny_root() {
        let svc = FsService::new(PathBuf::from("/"));
        assert!(svc.free_space().unwrap().total_bytes > 0);
    }
}
