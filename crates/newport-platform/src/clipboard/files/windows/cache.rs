//! Owner-only download storage. Native file promises carry validated handles;
//! their paths survive ownership loss through a bounded on-disk grace period.
use super::super::{Limits, windows_policy};
use super::{
    filesystem,
    security::{PrivateSecurity, verify_private},
};
use std::{
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    os::windows::io::FromRawHandle,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};
use windows::{
    Win32::{
        Foundation::{
            ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS, ERROR_LOCK_VIOLATION, GENERIC_READ,
            GENERIC_WRITE,
        },
        Storage::FileSystem::*,
        System::IO::OVERLAPPED,
    },
    core::PCWSTR,
};

const GRACE: Duration = Duration::from_secs(10 * 60);
const MAX_RECORDS: usize = 10_000;
const PARTIAL: &str = ".partial-";
const LEASE: &str = ".lease-";
const MAGIC: &[u8; 8] = b"NPTCLIP1";

fn error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

pub fn cache_base() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(base) = std::env::var_os("NEWPORT_CLIPBOARD_TEST_BASE") {
        return Some(base.into());
    }
    dirs::cache_dir().map(|base| base.join("Newport").join("Clipboard"))
}

fn profile_root(path: &Path) -> io::Result<()> {
    let base = cache_base().ok_or_else(|| error("Clipboard cache is unavailable"))?;
    let relative = path
        .strip_prefix(&base)
        .map_err(|_| error("Not a clipboard cache folder"))?;
    let mut parts = relative.components();
    match (parts.next(), parts.next()) {
        (Some(Component::Normal(name)), None)
            if name
                .to_str()
                .is_some_and(|s| s.len() == 16 && s.bytes().all(|b| b.is_ascii_hexdigit())) =>
        {
            Ok(())
        }
        _ => Err(error("Invalid clipboard cache instance")),
    }
}

fn wide(path: &Path) -> io::Result<Vec<u16>> {
    let (drive, parts) =
        windows_policy::drive_path(path.to_str().ok_or_else(|| error("Invalid Unicode path"))?)
            .map_err(error)?;
    let result: Vec<_> = format!("\\\\?\\{drive}:\\{}", parts.join("\\"))
        .encode_utf16()
        .chain(Some(0))
        .collect();
    if result.len() > 32_767 {
        return Err(error("Clipboard path exceeds Windows length limit"));
    }
    Ok(result)
}

fn create_directory(path: &Path) -> io::Result<()> {
    let text = wide(path)?;
    let security = PrivateSecurity::new()?;
    // SAFETY: `text` is NUL-terminated, and the attributes (with their descriptor)
    // outlive the call.
    unsafe { CreateDirectoryW(PCWSTR(text.as_ptr()), Some(&security.attributes())) }.map_err(|e| {
        if e.code() == windows::core::HRESULT::from_win32(ERROR_ALREADY_EXISTS.0) {
            io::ErrorKind::AlreadyExists.into()
        } else {
            error(e)
        }
    })
}

fn create_file(path: &Path) -> io::Result<File> {
    let text = wide(path)?;
    let security = PrivateSecurity::new()?;
    // SAFETY: `text` is NUL-terminated, and the attributes (with their descriptor)
    // outlive the call.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(text.as_ptr()),
            GENERIC_READ.0 | GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            Some(&security.attributes()),
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            None,
        )
    }
    .map_err(|e| {
        if [ERROR_ALREADY_EXISTS.0, ERROR_FILE_EXISTS.0]
            .into_iter()
            .any(|code| e.code() == windows::core::HRESULT::from_win32(code))
        {
            io::ErrorKind::AlreadyExists.into()
        } else {
            error(e)
        }
    })?;
    // SAFETY: CreateFileW succeeded, so the handle is new and owned.
    Ok(unsafe { File::from_raw_handle(handle.0) })
}

fn ensure_private(path: &Path) -> io::Result<()> {
    match create_directory(path) {
        Ok(()) => (),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e),
    }
    let file = filesystem::open_source(path).map_err(error)?;
    if !filesystem::identity(&file).map_err(error)?.directory {
        return Err(error("Cache root is not a directory"));
    }
    verify_private(&file, true)
}

fn root(path: &Path, create: bool) -> io::Result<Vec<File>> {
    profile_root(path)?;
    let base = cache_base().unwrap();
    if create {
        let parent = base.parent().unwrap();
        // The application cache container can already exist; only our namespace
        // and profile roots require the exact owner-only ACL.
        let _container = filesystem::pin_path(
            parent
                .parent()
                .ok_or_else(|| error("Invalid cache container"))?,
        )
        .map_err(error)?;
        match create_directory(parent) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e),
        }
        let _parent = filesystem::pin_path(parent).map_err(error)?;
        ensure_private(&base)?;
        let _base = filesystem::pin_path(&base).map_err(error)?;
        ensure_private(path)?;
    }
    let pinned = filesystem::pin_path(path).map_err(error)?;
    verify_private(pinned.last().unwrap(), true)?;
    Ok(pinned)
}

fn lock(file: &File, exclusive: bool) -> io::Result<bool> {
    let flags = LOCKFILE_FAIL_IMMEDIATELY
        | if exclusive {
            LOCKFILE_EXCLUSIVE_LOCK
        } else {
            LOCK_FILE_FLAGS(0)
        };
    let mut overlapped = OVERLAPPED::default();
    // SAFETY: the handle is open, and `overlapped` lives until the call returns.
    match unsafe { LockFileEx(filesystem::handle(file), flags, None, 1, 0, &mut overlapped) } {
        Ok(()) => Ok(true),
        Err(e) if e.code() == windows::core::HRESULT::from_win32(ERROR_LOCK_VIOLATION.0) => {
            Ok(false)
        }
        Err(e) => Err(error(e)),
    }
}

fn admission(path: &Path, parent: &File) -> io::Result<File> {
    match create_file(&path.join("admission")) {
        Ok(file) => drop(file),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e),
    }
    let file = filesystem::open_child(parent, "admission").map_err(error)?;
    verify_private(&file, true)?;
    if !lock(&file, true)? {
        return Err(error(
            "Another file copy is updating the cache; retry the paste",
        ));
    }
    Ok(file)
}

#[derive(Clone, Copy)]
struct Reservation {
    bytes: u64,
    entries: u64,
    object: [u8; 32],
}
impl Reservation {
    fn read(file: &mut File) -> io::Result<Self> {
        if file.metadata()?.len() != 56 {
            return Err(error("Invalid clipboard cache reservation"));
        }
        file.seek(SeekFrom::Start(0))?;
        let mut bytes = [0u8; 56];
        file.read_exact(&mut bytes)?;
        if &bytes[..8] != MAGIC {
            return Err(error("Unknown clipboard cache reservation"));
        }
        let result = Self {
            bytes: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            entries: u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            object: bytes[24..].try_into().unwrap(),
        };
        if result.bytes > Limits::CLIPBOARD.total_bytes
            || result.entries == 0
            || result.entries > Limits::CLIPBOARD.entries
        {
            return Err(error("Invalid clipboard cache quota"));
        }
        Ok(result)
    }
    fn write(self, file: &mut File) -> io::Result<()> {
        file.write_all(MAGIC)?;
        file.write_all(&self.bytes.to_le_bytes())?;
        file.write_all(&self.entries.to_le_bytes())?;
        file.write_all(&self.object)?;
        file.sync_all()
    }
}

fn lease(parent: &File, id: &str) -> io::Result<File> {
    let file = filesystem::open_child(parent, &format!("{LEASE}{id}")).map_err(error)?;
    let meta = filesystem::identity(&file).map_err(error)?;
    if meta.directory || meta.links != 1 {
        return Err(error("Invalid clipboard lease object"));
    }
    verify_private(&file, true)?;
    filesystem::writable(&file).map_err(error)
}

fn records(parent: &File) -> io::Result<Vec<String>> {
    Ok(filesystem::children(parent, MAX_RECORDS * 3 + 1)
        .map_err(error)?
        .into_iter()
        .filter_map(|name| {
            name.strip_prefix(LEASE)
                .and_then(|id| uuid::Uuid::try_parse(id).ok().map(|_| id.to_owned()))
        })
        .collect())
}

fn touch(file: &File) {
    let _ = file.set_times(fs::FileTimes::new().set_modified(SystemTime::now()));
}

fn expired(file: &File, now: SystemTime) -> bool {
    file.metadata()
        .and_then(|m| m.modified())
        .is_ok_and(|modified| now.duration_since(modified).is_ok_and(|age| age >= GRACE))
}

fn reap(root: &Path, parent: &File, now: SystemTime) {
    let Ok(ids) = records(parent) else {
        return;
    };
    for id in ids {
        let Ok(mut file) = lease(parent, &id) else {
            continue;
        };
        if !expired(&file, now) || !lock(&file, true).unwrap_or(false) {
            continue;
        }
        let Ok(reservation) = Reservation::read(&mut file) else {
            continue;
        };
        // The admission lock serializes reclamation with creation; the exclusive
        // lease excludes cache and native clipboard holders across processes.
        for name in [id.clone(), format!("{PARTIAL}{id}")] {
            let Ok(directory) = filesystem::open_child(parent, &name) else {
                continue;
            };
            let Ok(directory) = filesystem::retirement_handle(&directory) else {
                continue;
            };
            if !filesystem::identity(&directory)
                .is_ok_and(|id| id.directory && id.object_key() == reservation.object)
                || verify_private(&directory, true).is_err()
            {
                continue;
            }
            let _ = filesystem::remove_held(directory, Limits::CLIPBOARD.entries as usize);
        }
        if !root.join(&id).exists() && !root.join(format!("{PARTIAL}{id}")).exists() {
            let _ = filesystem::remove_file_held(&file);
        }
    }
}

/// Removes only owned, unlocked copies whose publication grace has elapsed.
/// Pending and retained reservations both count toward the next admission.
pub fn sweep_cache(path: &Path, now: SystemTime) -> io::Result<()> {
    let pinned = root(path, false)?;
    let parent = pinned.last().unwrap();
    let _admission = admission(path, parent)?;
    reap(path, parent, now);
    Ok(())
}

/// An unpublished download. Only validated relative paths can be created.
/// Dropping a failed download removes its staging tree and reservation.
pub struct DownloadDirectory {
    root: PathBuf,
    partial: PathBuf,
    id: String,
    reservation: Reservation,
    lease: Option<File>,
    _parents: Vec<File>,
    finished: bool,
    renamed: bool,
}
impl DownloadDirectory {
    pub fn create(path: &Path, bytes: u64, entries: u64) -> io::Result<Self> {
        let pinned = root(path, true)?;
        let parent = pinned.last().unwrap();
        let _admission = admission(path, parent)?;
        reap(path, parent, SystemTime::now());
        let mut total = Reservation {
            bytes,
            entries,
            object: [0; 32],
        };
        for id in records(parent)? {
            let reservation = Reservation::read(&mut lease(parent, &id)?)?;
            total.bytes = total
                .bytes
                .checked_add(reservation.bytes)
                .ok_or_else(|| error("Clipboard cache is full"))?;
            total.entries = total
                .entries
                .checked_add(reservation.entries)
                .ok_or_else(|| error("Clipboard cache is full"))?;
        }
        if entries == 0
            || total.bytes > Limits::CLIPBOARD.total_bytes
            || total.entries > Limits::CLIPBOARD.entries
        {
            return Err(error(
                "Clipboard file cache is full; previous pastes are still retained",
            ));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let reservation = Reservation {
            bytes,
            entries,
            object: [0; 32],
        };
        let file = create_file(&path.join(format!("{LEASE}{id}")))?;
        let partial = path.join(format!("{PARTIAL}{id}"));
        let mut result = Self {
            root: path.to_owned(),
            partial,
            id,
            reservation,
            lease: Some(file),
            _parents: pinned,
            finished: false,
            renamed: false,
        };
        create_directory(&result.partial)?;
        let directory = filesystem::open_source(&result.partial).map_err(error)?;
        result.reservation.object = filesystem::identity(&directory)
            .map_err(error)?
            .object_key();
        result.reservation.write(result.lease.as_mut().unwrap())?;
        if !lock(result.lease.as_ref().unwrap(), false)? {
            return Err(error("Could not own the new clipboard lease"));
        }
        Ok(result)
    }
    pub fn path(&self) -> &Path {
        &self.partial
    }
    fn destination(&self, relative: &Path) -> io::Result<PathBuf> {
        if relative.as_os_str().is_empty() {
            return Err(error("Empty clipboard relative path"));
        }
        let text = relative
            .to_str()
            .ok_or_else(|| error("Invalid Unicode name"))?;
        for part in text.split(['/', '\\']) {
            windows_policy::component(part).map_err(error)?;
        }
        Ok(self.partial.join(relative))
    }
    pub fn create_dir(&self, relative: &Path) -> io::Result<()> {
        let path = self.destination(relative)?;
        let _parents = filesystem::pin_path(path.parent().unwrap()).map_err(error)?;
        create_directory(&path)
    }
    pub fn create_file(&self, relative: &Path) -> io::Result<File> {
        let path = self.destination(relative)?;
        let _parents = filesystem::pin_path(path.parent().unwrap()).map_err(error)?;
        create_file(&path)
    }
    /// Apply time metadata and Mark of the Web before exposing the copy.
    /// ACLs stay private; Unix executable bits never become Windows permissions.
    pub fn finish_file(&self, relative: &Path, mtime: Option<u32>) -> io::Result<()> {
        let path = self.destination(relative)?;
        let _parents = filesystem::pin_path(path.parent().unwrap()).map_err(error)?;
        let file = filesystem::open_source(&path).map_err(error)?;
        if filesystem::identity(&file).map_err(error)?.directory {
            return Ok(());
        }
        let _file = filesystem::pin(&file, false).map_err(error)?;
        // Only this fixed stream is created, on a verified private regular file.
        // Failure to retain download provenance refuses publication.
        let mut stream = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(format!(
                "{}:Zone.Identifier",
                path.to_str().ok_or_else(|| error("Invalid Unicode path"))?
            ))?;
        stream.write_all(b"[ZoneTransfer]\r\nZoneId=3\r\n")?;
        stream.sync_all()?;
        if let Some(seconds) = mtime {
            let writer = fs::OpenOptions::new().write(true).open(&path)?;
            writer.set_times(
                fs::FileTimes::new()
                    .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds.into())),
            )?;
        }
        Ok(())
    }
    pub fn finish(mut self) -> io::Result<RetainedFiles> {
        let folder = self.root.join(&self.id);
        // Validate all contents before the atomic publication rename. Reparse
        // entries, extra files, size changes and namespace mutations are errors.
        let validated = validate_tree(&self.partial, self.reservation)?;
        drop(validated);
        let source = filesystem::open_source(&self.partial).map_err(error)?;
        let source = filesystem::retirement_handle(&source).map_err(error)?;
        if filesystem::identity(&source).map_err(error)?.object_key() != self.reservation.object {
            return Err(error("Clipboard staging directory was replaced"));
        }
        filesystem::rename_held(&source, self._parents.last().unwrap(), &self.id).map_err(error)?;
        self.renamed = true;
        drop(source);
        let retained = RetainedFiles::open(&folder)?;
        self.finished = true;
        Ok(retained)
    }
}
impl Drop for DownloadDirectory {
    fn drop(&mut self) {
        if !self.finished {
            let path = if self.renamed {
                self.root.join(&self.id)
            } else {
                self.partial.clone()
            };
            if let Ok(file) =
                filesystem::open_source(&path).and_then(|file| filesystem::retirement_handle(&file))
                && filesystem::identity(&file)
                    .is_ok_and(|id| id.object_key() == self.reservation.object)
            {
                let _ = filesystem::remove_held(file, Limits::CLIPBOARD.entries as usize);
            }
            // Keep the reservation when cleanup cannot finish; admission counts
            // it and a later sweep can retry without losing ownership evidence.
            if !path.exists()
                && let Some(lease) = &self.lease
            {
                let _ = filesystem::remove_file_held(lease);
            }
        }
        drop(self.lease.take());
    }
}

struct Retained {
    folder: PathBuf,
    paths: Vec<PathBuf>,
    reservation: Reservation,
    lease: File,
    _handles: Vec<File>,
}
impl Drop for Retained {
    fn drop(&mut self) {
        touch(&self.lease);
    }
}

/// Validated CF_HDROP paths and a cross-process lease. Acquire off the UI thread,
/// pass this typed value to the native owner, and keep it while files are exposed.
/// Last release refreshes the on-disk grace; it never removes an exposed path.
#[derive(Clone)]
pub struct RetainedFiles(Arc<Retained>);
impl RetainedFiles {
    pub fn open(folder: &Path) -> io::Result<Self> {
        let parent = folder
            .parent()
            .ok_or_else(|| error("Invalid clipboard cache path"))?;
        let id = folder
            .file_name()
            .and_then(|id| id.to_str())
            .ok_or_else(|| error("Invalid clipboard cache ID"))?;
        if uuid::Uuid::try_parse(id).is_err() {
            return Err(error("Invalid clipboard cache ID"));
        }
        let mut handles = root(parent, false)?;
        let mut lease = lease(handles.last().unwrap(), id)?;
        if !lock(&lease, false)? {
            return Err(error("Clipboard cache is being reclaimed"));
        }
        let reservation = Reservation::read(&mut lease)?;
        let (paths, tree) = validate_tree(folder, reservation)?;
        handles.extend(tree);
        touch(&lease);
        Ok(Self(Arc::new(Retained {
            folder: folder.to_owned(),
            paths,
            reservation,
            lease,
            _handles: handles,
        })))
    }
    pub fn folder(&self) -> &Path {
        &self.0.folder
    }
    pub fn paths(&self) -> &[PathBuf] {
        &self.0.paths
    }
    pub fn bytes(&self) -> u64 {
        self.0.reservation.bytes
    }
    pub fn entries(&self) -> u64 {
        self.0.reservation.entries
    }
}
impl newport_clipboard::provider::windows::RetainedFiles for RetainedFiles {
    fn folder(&self) -> &Path {
        &self.0.folder
    }
    fn paths(&self) -> &[PathBuf] {
        &self.0.paths
    }
}
/// The clipboard helper reopens a finished copy by folder with its own lease.
pub fn open_retained(folder: &Path) -> io::Result<newport_clipboard::provider::windows::Files> {
    Ok(Arc::new(RetainedFiles::open(folder)?))
}

fn validate_tree(folder: &Path, reservation: Reservation) -> io::Result<(Vec<PathBuf>, Vec<File>)> {
    let mut held = filesystem::pin_path(folder).map_err(error)?;
    let directory = held.last().unwrap();
    verify_private(directory, true)?;
    if filesystem::identity(directory).map_err(error)?.object_key() != reservation.object {
        return Err(error("Clipboard cache directory was replaced"));
    }
    let mut paths = Vec::new();
    let mut pending = vec![(folder.to_owned(), directory.try_clone()?, true, None)];
    let mut bytes = 0u64;
    let mut count = 0u64;
    let mut scheduled = 0usize;
    while let Some((path, file, top, visited)) = pending.pop() {
        let identity = filesystem::identity(&file).map_err(error)?;
        if let Some(before) = visited {
            if identity != before {
                return Err(error("Clipboard cache changed during validation"));
            }
            continue;
        }
        let names =
            filesystem::children(&file, reservation.entries as usize - scheduled).map_err(error)?;
        scheduled += names.len();
        pending.push((path.clone(), file.try_clone()?, top, Some(identity)));
        for name in names {
            let child = filesystem::open_child(&file, &name).map_err(error)?;
            let identity = filesystem::identity(&child).map_err(error)?;
            if identity.links != 1 {
                return Err(error("Clipboard cache contains a hard link"));
            }
            verify_private(&child, true)?;
            let pinned = filesystem::pin(&child, !identity.directory).map_err(error)?;
            let child_path = path.join(name);
            count += 1;
            if top {
                paths.push(child_path.clone());
            }
            if identity.directory {
                pending.push((child_path, pinned.try_clone()?, false, None));
            } else {
                bytes = bytes
                    .checked_add(identity.bytes)
                    .ok_or_else(|| error("Clipboard cache size overflow"))?;
            }
            held.push(pinned);
        }
    }
    if bytes != reservation.bytes || count != reservation.entries || paths.is_empty() {
        return Err(error("Clipboard cache contents changed after planning"));
    }
    Ok((paths, held))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let base = std::env::var_os("NEWPORT_CLIPBOARD_TEST_BASE")
                .expect("the isolated Windows harness must set NEWPORT_CLIPBOARD_TEST_BASE");
            let base = PathBuf::from(base);
            assert!(base.is_absolute());
            fs::create_dir_all(base.parent().unwrap()).unwrap();
            Self(base.join(&uuid::Uuid::new_v4().simple().to_string()[..16]))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    #[ignore = "requires an isolated Windows cache root"]
    fn retained_files_pin_exact_bytes_and_release_into_paste_grace() {
        fn send_sync<T: Send + Sync>() {}
        send_sync::<RetainedFiles>();
        let fixture = Fixture::new();
        let download = DownloadDirectory::create(&fixture.0, 3, 2).unwrap();
        download.create_dir(Path::new("empty")).unwrap();
        let mut file = download.create_file(Path::new("世界.txt")).unwrap();
        file.write_all(b"abc").unwrap();
        drop(file);
        download
            .finish_file(Path::new("世界.txt"), Some(1_600_000_000))
            .unwrap();
        let files = download.finish().unwrap();
        let path = files.folder().to_owned();
        assert_eq!(
            (files.bytes(), files.entries(), files.paths().len()),
            (3, 2, 2)
        );
        assert_eq!(fs::read(path.join("世界.txt")).unwrap(), b"abc");
        assert!(
            fs::read_to_string(format!(
                "{}:Zone.Identifier",
                path.join("世界.txt").display()
            ))
            .unwrap()
            .contains("ZoneId=3")
        );
        assert!(
            fs::OpenOptions::new()
                .write(true)
                .open(path.join("世界.txt"))
                .is_err()
        );
        let native = RetainedFiles::open(&path).unwrap();
        drop(files);
        sweep_cache(
            &fixture.0,
            SystemTime::now() + GRACE + Duration::from_secs(1),
        )
        .unwrap();
        assert!(path.exists());
        drop(native);
        sweep_cache(&fixture.0, SystemTime::now()).unwrap();
        assert!(path.exists());
        sweep_cache(
            &fixture.0,
            SystemTime::now() + GRACE + Duration::from_secs(1),
        )
        .unwrap();
        assert!(!path.exists());
    }

    #[test]
    #[ignore = "requires an isolated Windows cache root"]
    fn reservations_bound_pending_and_retained_copies_before_creation() {
        let fixture = Fixture::new();
        let pending =
            DownloadDirectory::create(&fixture.0, Limits::CLIPBOARD.total_bytes, 1).unwrap();
        assert!(DownloadDirectory::create(&fixture.0, 1, 1).is_err());
        drop(pending);
        let next = DownloadDirectory::create(&fixture.0, 0, 1).unwrap();
        next.create_dir(Path::new("empty")).unwrap();
        let retained = next.finish().unwrap();
        let files = DownloadDirectory::create(&fixture.0, 0, Limits::CLIPBOARD.entries);
        assert!(files.is_err());
        assert!(retained.folder().exists());
        drop(retained);
    }

    #[test]
    #[ignore = "requires an isolated Windows cache root"]
    fn replaced_cache_directory_is_neither_published_nor_reclaimed() {
        let fixture = Fixture::new();
        let download = DownloadDirectory::create(&fixture.0, 0, 1).unwrap();
        download.create_dir(Path::new("empty")).unwrap();
        let retained = download.finish().unwrap();
        let path = retained.folder().to_owned();
        drop(retained);
        let original = fixture.0.join("original");
        fs::rename(&path, &original).unwrap();
        create_directory(&path).unwrap();
        let sentinel = path.join("sentinel");
        create_file(&sentinel).unwrap().write_all(b"keep").unwrap();
        assert!(RetainedFiles::open(&path).is_err());
        sweep_cache(
            &fixture.0,
            SystemTime::now() + GRACE + Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(fs::read(sentinel).unwrap(), b"keep");
        assert!(original.join("empty").is_dir());
    }

    #[test]
    #[ignore = "requires an isolated Windows cache root and developer-mode symlinks"]
    fn paths_outside_the_cache_and_reparse_contents_never_publish() {
        let fixture = Fixture::new();
        assert!(RetainedFiles::open(&std::env::temp_dir()).is_err());
        let download = DownloadDirectory::create(&fixture.0, 1, 1).unwrap();
        let outside = fixture.0.join("outside");
        fs::write(&outside, b"x").unwrap();
        std::os::windows::fs::symlink_file(&outside, download.path().join("link")).unwrap();
        assert!(download.finish().is_err());
        assert_eq!(fs::read(outside).unwrap(), b"x");
    }
}
