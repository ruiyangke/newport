use super::*;
use std::collections::HashMap;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// A server's file system in a local temporary directory, used as its home.
pub(crate) struct Disk {
    pub(crate) home: tempfile::TempDir,
    handles: Mutex<HashMap<String, std::fs::File>>,
    next: AtomicUsize,
    /// Staging directories created.
    pub(crate) uploads: AtomicUsize,
    pub(crate) connects: AtomicUsize,
    pub(crate) fail_writes: AtomicBool,
    pub(crate) touches: AtomicUsize,
    /// `set_attributes` calls.
    pub(crate) attributes: AtomicUsize,
    /// Each write takes a permit; writes waiting for one are counted.
    pub(crate) writes: Semaphore,
    pub(crate) waiting: AtomicUsize,
    /// While set, each read waits for a permit of `reads`; reads waiting for
    /// one are counted.
    pub(crate) hold_reads: AtomicBool,
    pub(crate) reads: Semaphore,
    pub(crate) reading: AtomicUsize,
}
impl Disk {
    pub(crate) fn new(blocked: bool) -> Arc<Self> {
        Arc::new(Self {
            home: tempfile::tempdir().unwrap(),
            handles: Mutex::default(),
            next: AtomicUsize::new(0),
            uploads: AtomicUsize::new(0),
            connects: AtomicUsize::new(0),
            fail_writes: AtomicBool::new(false),
            touches: AtomicUsize::new(0),
            attributes: AtomicUsize::new(0),
            writes: Semaphore::new(if blocked { 0 } else { Semaphore::MAX_PERMITS }),
            waiting: AtomicUsize::new(0),
            hold_reads: AtomicBool::new(false),
            reads: Semaphore::new(0),
            reading: AtomicUsize::new(0),
        })
    }
    pub(crate) fn root(&self) -> PathBuf {
        self.home.path().join(".cache/newport/clipboard")
    }
    /// Names in the cache root.
    pub(crate) fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.root())
            .map(|entries| {
                entries
                    .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
    pub(crate) fn partials(&self) -> usize {
        self.names()
            .iter()
            .filter(|name| name.starts_with(PARTIAL))
            .count()
    }
    pub(crate) async fn until(&self, condition: impl Fn(&Self) -> bool) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !condition(self) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("condition not reached");
    }
}
fn io<T>(result: std::io::Result<T>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn stat_of(meta: &std::fs::Metadata) -> Result<Stat> {
    let mtime = meta.modified()?.duration_since(SystemTime::UNIX_EPOCH)?;
    Ok(Stat {
        kind: kind_of(meta.file_type()),
        size: meta.len(),
        mtime: Some(mtime.as_secs() as u32),
        mode: meta.mode() & 0o7777,
    })
}
fn kind_of(file_type: std::fs::FileType) -> Kind {
    if file_type.is_symlink() {
        Kind::Link
    } else if file_type.is_dir() {
        Kind::Directory
    } else if file_type.is_file() {
        Kind::File
    } else {
        Kind::Other
    }
}
impl RemoteFs for Disk {
    async fn home(&self) -> Result<String> {
        Ok(self.home.path().to_str().unwrap().to_owned())
    }
    async fn realpath(&self, path: &str) -> Result<String> {
        let path = self.home.path().join(path);
        Ok(std::fs::canonicalize(path)?.to_str().unwrap().to_owned())
    }
    async fn kind(&self, path: &str) -> Result<Option<Kind>> {
        Ok(io(std::fs::metadata(path))?.map(|meta| kind_of(meta.file_type())))
    }
    async fn lstat(&self, path: &str) -> Result<Option<Stat>> {
        io(std::fs::symlink_metadata(path))?
            .map(|meta| stat_of(&meta))
            .transpose()
    }
    async fn readlink(&self, path: &str) -> Result<String> {
        Ok(std::fs::read_link(path)?.to_str().unwrap().to_owned())
    }
    async fn list(&self, path: &str) -> Result<Option<Vec<Item>>> {
        let Some(entries) = io(std::fs::read_dir(path))? else {
            return Ok(None);
        };
        let mut items = Vec::new();
        for entry in entries {
            let entry = entry?;
            let stat = stat_of(&entry.path().symlink_metadata()?)?;
            items.push(Item {
                name: entry.file_name().into_string().unwrap(),
                kind: stat.kind,
                mtime: stat.mtime,
                size: stat.size,
                mode: stat.mode,
            });
        }
        Ok(Some(items))
    }
    async fn mkdir(&self, path: &str, mode: u32) -> Result<()> {
        std::fs::create_dir(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
        if Path::new(path)
            .file_name()
            .is_some_and(|name| name.to_str().unwrap().starts_with(PARTIAL))
        {
            self.uploads.fetch_add(1, Ordering::AcqRel);
        }
        Ok(())
    }
    async fn create(&self, path: &str, mode: u32) -> Result<String> {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(path)?;
        let handle = self.next.fetch_add(1, Ordering::AcqRel).to_string();
        self.handles.lock().unwrap().insert(handle.clone(), file);
        Ok(handle)
    }
    async fn open_read(&self, path: &str) -> Result<String> {
        let file = std::fs::File::open(path)?;
        let handle = self.next.fetch_add(1, Ordering::AcqRel).to_string();
        self.handles.lock().unwrap().insert(handle.clone(), file);
        Ok(handle)
    }
    async fn read(&self, handle: &str, offset: u64, len: u32) -> Result<Option<Vec<u8>>> {
        if self.hold_reads.load(Ordering::Acquire) {
            self.reading.fetch_add(1, Ordering::AcqRel);
            let permit = self.reads.acquire().await;
            self.reading.fetch_sub(1, Ordering::AcqRel);
            permit?.forget();
        }
        let mut data = vec![0; len as usize];
        let count = self.handles.lock().unwrap()[handle].read_at(&mut data, offset)?;
        data.truncate(count);
        Ok((count > 0).then_some(data))
    }
    async fn write(&self, handle: &str, offset: u64, data: Vec<u8>) -> Result<()> {
        self.waiting.fetch_add(1, Ordering::AcqRel);
        let permit = self.writes.acquire().await;
        self.waiting.fetch_sub(1, Ordering::AcqRel);
        permit?.forget();
        if self.fail_writes.load(Ordering::Acquire) {
            bail!("No space left on the server");
        }
        let handles = self.handles.lock().unwrap();
        handles[handle].write_all_at(&data, offset)?;
        Ok(())
    }
    async fn close(&self, handle: String) -> Result<()> {
        self.handles.lock().unwrap().remove(&handle);
        Ok(())
    }
    async fn symlink(&self, link: &str, target: &str) -> Result<()> {
        std::os::unix::fs::symlink(target, link)?;
        Ok(())
    }
    async fn rename(&self, from: &str, to: &str) -> Result<()> {
        if Path::new(to).symlink_metadata().is_ok() {
            bail!("Destination exists");
        }
        std::fs::rename(from, to)?;
        Ok(())
    }
    async fn remove_file(&self, path: &str) -> Result<()> {
        std::fs::remove_file(path)?;
        Ok(())
    }
    async fn remove_dir(&self, path: &str) -> Result<()> {
        std::fs::remove_dir(path)?;
        Ok(())
    }
    async fn touch(&self, path: &str) -> Result<()> {
        std::fs::File::open(path)?.set_modified(SystemTime::now())?;
        self.touches.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
    async fn set_attributes(
        &self,
        path: &str,
        mtime: Option<u32>,
        mode: Option<u32>,
    ) -> Result<()> {
        use std::fs::{FileTimes, Permissions};
        if let Some(mtime) = mtime {
            let current = std::fs::metadata(path)?.mode() & 0o7777;
            std::fs::set_permissions(path, Permissions::from_mode(current | 0o700))?;
            let time = SystemTime::UNIX_EPOCH + Duration::from_secs(mtime.into());
            let times = FileTimes::new().set_accessed(time).set_modified(time);
            std::fs::File::open(path)?.set_times(times)?;
            std::fs::set_permissions(path, Permissions::from_mode(current))?;
        }
        if let Some(mode) = mode {
            std::fs::set_permissions(path, Permissions::from_mode(mode))?;
        }
        self.attributes.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}
pub(crate) struct Local(pub(crate) Arc<Disk>);
impl Connect for Local {
    type Remote = Disk;
    async fn connect(&self) -> Result<Arc<Disk>> {
        self.0.connects.fetch_add(1, Ordering::AcqRel);
        Ok(self.0.clone())
    }
}

pub(crate) fn set_mtime(path: &Path, seconds: u64) {
    let time = SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
    std::fs::File::open(path)
        .unwrap()
        .set_modified(time)
        .unwrap();
}
pub(crate) fn mtime(path: &Path) -> u64 {
    std::fs::symlink_metadata(path)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// A copy of a folder (nested folder, in-tree link) and a Unicode-named file.
pub(crate) struct Copied {
    pub(crate) dir: tempfile::TempDir,
}
impl Copied {
    pub(crate) fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let docs = dir.path().join("docs");
        std::fs::create_dir_all(docs.join("nested")).unwrap();
        std::fs::write(docs.join("nested/deep.txt"), b"deep").unwrap();
        std::os::unix::fs::symlink("nested/deep.txt", docs.join("link")).unwrap();
        let named = dir.path().join("名前 1.txt");
        std::fs::write(&named, vec![7u8; 100_000]).unwrap();
        std::fs::set_permissions(&named, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Folders last, children first, as their times change with their contents.
        for (path, mode, mtime) in [
            (docs.join("nested/deep.txt"), 0o644, 1_600_000_000),
            (named.clone(), 0o755, 1_500_000_000),
            (docs.join("nested"), 0o755, 1_400_000_000),
            (docs.clone(), 0o750, 1_300_000_000),
        ] {
            set_mtime(&path, mtime);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        Self { dir }
    }
    pub(crate) fn paths(&self) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "paths": [self.dir.path().join("docs"), self.dir.path().join("名前 1.txt")],
        }))
        .unwrap()
    }
}

fn files(disk: &Arc<Disk>, mode: Mode) -> Files<Local> {
    Files::new(Local(disk.clone()), mode)
}
async fn serve(files: &Files<Local>, revision: i64, copied: &Copied) -> Option<Vec<u8>> {
    let paths = copied.paths();
    files.serve(revision, None, async { Some(paths) }).await
}

#[test]
fn files_are_offered_by_negotiated_clipboard_service_version() {
    assert_eq!(mode(|version| version <= 3), Some(Mode::LongRead));
    assert_eq!(mode(|version| version == 3), Some(Mode::LongRead));
    assert_eq!(mode(|version| version <= 2), Some(Mode::Budget));
    assert_eq!(mode(|version| version == 1), None);
    assert_eq!(mode(|_| false), None);
    assert!(Mode::LongRead.keep_alive() && !Mode::Budget.keep_alive());
    assert_eq!(Mode::Budget.deadline(), Duration::from_secs(12));
    assert_eq!(Mode::LongRead.deadline(), Duration::from_secs(30 * 60));
    assert!(Mode::Budget.admits(64 << 20) && !Mode::Budget.admits((64 << 20) + 1));
    assert!(Mode::LongRead.admits(2 << 30));
}

#[tokio::test]
async fn a_copied_tree_is_uploaded_and_published_with_an_exact_reply() {
    let disk = Disk::new(false);
    let copied = Copied::new();
    let files = files(&disk, Mode::LongRead);
    let reply = String::from_utf8(serve(&files, 42, &copied).await.unwrap()).unwrap();

    let names = disk.names();
    assert_eq!(names.len(), 1, "{names:?}");
    let id = uuid::Uuid::try_parse(&names[0]).unwrap();
    let root = disk.root();
    let folder = root.join(id.to_string());
    let base = folder.to_str().unwrap();
    assert_eq!(
        reply,
        format!(r#"{{"version":1,"paths":["{base}/docs","{base}/名前 1.txt"]}}"#)
    );
    assert_eq!(
        std::fs::read(folder.join("docs/nested/deep.txt")).unwrap(),
        b"deep"
    );
    assert_eq!(
        std::fs::read_link(folder.join("docs/link")).unwrap(),
        Path::new("nested/deep.txt")
    );
    assert_eq!(std::fs::read(folder.join("docs/link")).unwrap(), b"deep");
    let named = folder.join("名前 1.txt");
    assert_eq!(std::fs::read(&named).unwrap(), vec![7u8; 100_000]);
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&named), 0o755);
    // The cache folders are private; copied folders keep their own mode.
    for dir in [
        disk.home.path().join(".cache"),
        disk.home.path().join(".cache/newport"),
        root.clone(),
        folder.clone(),
    ] {
        assert_eq!(mode(&dir), 0o700, "{dir:?}");
    }
    assert_eq!(mode(&folder.join("docs")), 0o750);
    assert_eq!(mode(&folder.join("docs/nested")), 0o755);
    assert_eq!(mode(&folder.join("docs/nested/deep.txt")), 0o644);
    // Files and folders keep their modification times; the final folder
    // times are not disturbed by the entries created inside them.
    assert_eq!(mtime(&named), 1_500_000_000);
    assert_eq!(mtime(&folder.join("docs/nested/deep.txt")), 1_600_000_000);
    assert_eq!(mtime(&folder.join("docs/nested")), 1_400_000_000);
    assert_eq!(mtime(&folder.join("docs")), 1_300_000_000);
    assert_eq!(disk.uploads.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn read_only_folders_and_files_are_uploaded_with_their_modes_and_removed_again() {
    let disk = Disk::new(false);
    let source = tempfile::tempdir().unwrap();
    let locked = source.path().join("locked");
    std::fs::create_dir_all(locked.join("inner")).unwrap();
    std::fs::write(locked.join("inner/ro.txt"), b"ro").unwrap();
    for (path, mode, seconds) in [
        (locked.join("inner/ro.txt"), 0o444, 1_000_000_001),
        (locked.join("inner"), 0o555, 1_000_000_002),
        (locked.clone(), 0o500, 1_000_000_003),
    ] {
        set_mtime(&path, seconds);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    let paths = serde_json::to_vec(&serde_json::json!({"version": 1, "paths": [&locked]})).unwrap();
    let files = files(&disk, Mode::LongRead);
    let reply = files.serve(1, None, async { Some(paths) }).await;
    // Restore access so the temporary directories can be deleted.
    let restore = || {
        for path in [&locked, &locked.join("inner")] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    };
    restore();
    assert!(reply.is_some());
    let folder = disk.root().join(&disk.names()[0]);
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&folder.join("locked")), 0o500);
    assert_eq!(mode(&folder.join("locked/inner")), 0o555);
    assert_eq!(mode(&folder.join("locked/inner/ro.txt")), 0o444);
    assert_eq!(mtime(&folder.join("locked/inner/ro.txt")), 1_000_000_001);
    assert_eq!(mtime(&folder.join("locked/inner")), 1_000_000_002);
    assert_eq!(mtime(&folder.join("locked")), 1_000_000_003);
    // Removing the copy restores the owner's access to each folder first.
    remove_tree(&*disk, folder.to_str().unwrap()).await.unwrap();
    assert!(!folder.exists());
}

#[tokio::test]
async fn reads_of_one_copy_share_its_upload() {
    let disk = Disk::new(true);
    let copied = Copied::new();
    let files = files(&disk, Mode::LongRead);
    let (first, second, ()) = tokio::join!(
        serve(&files, 42, &copied),
        serve(&files, 42, &copied),
        async {
            disk.until(|disk| disk.waiting.load(Ordering::Acquire) > 0)
                .await;
            disk.writes.add_permits(Semaphore::MAX_PERMITS);
        }
    );
    assert!(first.is_some());
    assert_eq!(first, second);
    let polled = AtomicBool::new(false);
    let third = files
        .serve(42, None, async {
            polled.store(true, Ordering::Release);
            None
        })
        .await;
    assert_eq!(third, first);
    assert!(!polled.load(Ordering::Acquire));
    assert_eq!(disk.uploads.load(Ordering::Acquire), 1);
    assert_eq!(disk.names().len(), 1);
}

#[tokio::test]
async fn a_failed_upload_removes_its_partial_and_is_retried_by_the_next_read() {
    let disk = Disk::new(false);
    disk.fail_writes.store(true, Ordering::Release);
    let copied = Copied::new();
    let files = files(&disk, Mode::LongRead);
    assert!(serve(&files, 42, &copied).await.is_none());
    assert_eq!(disk.uploads.load(Ordering::Acquire), 1);
    assert!(disk.names().is_empty(), "{:?}", disk.names());

    disk.fail_writes.store(false, Ordering::Release);
    assert!(serve(&files, 42, &copied).await.is_some());
    assert_eq!(disk.uploads.load(Ordering::Acquire), 2);
    assert_eq!(disk.partials(), 0);
}

#[tokio::test]
async fn a_large_file_is_written_with_several_requests_outstanding_at_the_right_offsets() {
    let disk = Disk::new(true);
    let dir = tempfile::tempdir().unwrap();
    let large = dir.path().join("large.bin");
    let bytes: Vec<u8> = (0..20 * 65_536 + 17).map(|i| (i % 251) as u8).collect();
    std::fs::write(&large, &bytes).unwrap();
    let paths = serde_json::to_vec(&serde_json::json!({"version": 1, "paths": [large]})).unwrap();
    let files = files(&disk, Mode::LongRead);
    let (reply, ()) = tokio::join!(files.serve(42, None, async { Some(paths) }), async {
        // The whole window is sent before any acknowledgement.
        disk.until(|disk| disk.waiting.load(Ordering::Acquire) >= 16)
            .await;
        assert_eq!(disk.waiting.load(Ordering::Acquire), 16);
        disk.writes.add_permits(Semaphore::MAX_PERMITS);
    });
    assert!(reply.is_some());
    let folder = disk.root().join(&disk.names()[0]);
    assert!(std::fs::read(folder.join("large.bin")).unwrap() == bytes);
}

async fn failure(files: &Files<Local>, revision: i64, paths: Vec<u8>) -> Failure {
    files
        .serve_result(revision, None, async { Some(paths) })
        .await
        .unwrap_err()
}
fn listing(paths: &[&Path]) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({"version": 1, "paths": paths})).unwrap()
}
fn reason(failure: Failure) -> String {
    match failure {
        Failure::Failed(reason) => reason,
        Failure::Superseded => panic!("superseded"),
    }
}

#[tokio::test]
async fn an_sftp_error_is_a_failure_to_tell_the_person() {
    let disk = Disk::new(false);
    disk.fail_writes.store(true, Ordering::Release);
    let copied = Copied::new();
    let files = files(&disk, Mode::LongRead);
    let failed = failure(&files, 42, copied.paths()).await;
    assert_eq!(reason(failed), "No space left on the server");
}

#[tokio::test]
async fn limits_and_unsupported_entries_are_failures_to_tell_the_person() {
    let disk = Disk::new(false);
    let dir = tempfile::tempdir().unwrap();
    let files = files(&disk, Mode::LongRead);
    let large = dir.path().join("large.bin");
    std::fs::File::create(&large)
        .unwrap()
        .set_len((2 << 30) + 1)
        .unwrap();
    assert_eq!(
        reason(failure(&files, 1, listing(&[&large])).await),
        "The copied files are larger than 2 GB"
    );
    let pipe = dir.path().join("pipe");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&pipe)
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        reason(failure(&files, 2, listing(&[&pipe])).await),
        "“pipe” is not a file, folder or link"
    );
    let missing = dir.path().join("missing");
    assert!(reason(failure(&files, 3, listing(&[&missing])).await).contains("“missing”"));
    // The budget of a server without long reads is refused up front.
    let budget = self::files(&disk, Mode::Budget);
    let big = dir.path().join("big.bin");
    std::fs::File::create(&big)
        .unwrap()
        .set_len((64 << 20) + 1)
        .unwrap();
    assert!(
        reason(failure(&budget, 4, listing(&[&big])).await)
            .contains("too large for this server's Newport agent")
    );
    assert_eq!(disk.uploads.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn an_expired_budget_is_a_failure_to_tell_the_person() {
    let disk = Disk::new(true);
    let copied = Copied::new();
    let files = files(&disk, Mode::Budget);
    let paths = copied.paths();
    let (failed, ()) = tokio::join!(failure(&files, 42, paths), async {
        disk.until(|disk| disk.waiting.load(Ordering::Acquire) > 0)
            .await;
        tokio::time::pause();
        tokio::time::advance(BUDGET).await;
    });
    assert!(reason(failed).starts_with("Copying took too long"));
    files.shutdown().await;
    disk.until(|disk| disk.names().is_empty()).await;
}

#[tokio::test]
async fn a_superseded_or_unavailable_copy_is_not_a_failure_to_tell() {
    let disk = Disk::new(true);
    let copied = Copied::new();
    let files = files(&disk, Mode::LongRead);
    // A newer copy ends the upload.
    let (failed, ()) = tokio::join!(failure(&files, 42, copied.paths()), async {
        disk.until(|disk| disk.waiting.load(Ordering::Acquire) > 0)
            .await;
        files.observe(43);
    });
    assert_eq!(failed, Failure::Superseded);
    // A late read of the older copy.
    assert_eq!(
        failure(&files, 42, copied.paths()).await,
        Failure::Superseded
    );
    // Revoked permission ends the upload.
    let (failed, ()) = tokio::join!(failure(&files, 44, copied.paths()), async {
        disk.until(|disk| disk.waiting.load(Ordering::Acquire) > 0)
            .await;
        files.revoke();
    });
    assert_eq!(failed, Failure::Superseded);
    // A newer copy replaced this one before its paths could be read.
    let fresh = self::files(&Disk::new(false), Mode::LongRead);
    fresh.observe(2);
    assert_eq!(
        fresh
            .serve_result(1, None, async { None })
            .await
            .unwrap_err(),
        Failure::Superseded
    );
    // A copy replaced while its upload finished.
    let disk = Disk::new(true);
    let files = self::files(&disk, Mode::LongRead);
    let (failed, ()) = tokio::join!(failure(&files, 5, copied.paths()), async {
        disk.until(|disk| disk.waiting.load(Ordering::Acquire) > 0)
            .await;
        files.observe(6);
        disk.writes.add_permits(Semaphore::MAX_PERMITS);
    });
    assert_eq!(failed, Failure::Superseded);
}

#[tokio::test]
async fn a_newer_copy_cancels_the_upload_and_removes_its_partial() {
    let disk = Disk::new(true);
    let copied = Copied::new();
    let files = files(&disk, Mode::LongRead);
    let (reply, ()) = tokio::join!(serve(&files, 42, &copied), async {
        disk.until(|disk| disk.waiting.load(Ordering::Acquire) > 0)
            .await;
        assert_eq!(disk.partials(), 1);
        files.observe(43);
    });
    assert!(reply.is_none());
    disk.until(|disk| disk.names().is_empty()).await;
    // A late read of the older copy does not start another upload.
    assert!(serve(&files, 42, &copied).await.is_none());
    assert_eq!(disk.uploads.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn a_dropped_read_leaves_the_upload_running_until_the_session_ends() {
    let disk = Disk::new(true);
    let copied = Copied::new();
    let files = files(&disk, Mode::LongRead);
    tokio::select! {
        _ = serve(&files, 42, &copied) => panic!("blocked upload completed"),
        () = disk.until(|disk| disk.waiting.load(Ordering::Acquire) > 0) => {}
    }
    assert_eq!(disk.partials(), 1);
    files.shutdown().await;
    disk.until(|disk| disk.names().is_empty()).await;
}

#[tokio::test]
async fn ending_the_session_leaves_no_partial_of_a_running_upload() {
    let disk = Disk::new(true);
    let copied = Copied::new();
    let files = files(&disk, Mode::LongRead);
    tokio::select! {
        _ = serve(&files, 42, &copied) => panic!("blocked upload completed"),
        () = disk.until(|disk| disk.waiting.load(Ordering::Acquire) > 0) => {}
    }
    assert_eq!(disk.partials(), 1);
    files.shutdown().await;
    assert!(disk.names().is_empty(), "{:?}", disk.names());
}

#[tokio::test]
async fn the_interim_budget_refuses_large_copies_up_front() {
    let disk = Disk::new(false);
    let dir = tempfile::tempdir().unwrap();
    let large = dir.path().join("large.bin");
    std::fs::File::create(&large)
        .unwrap()
        .set_len((64 << 20) + 1)
        .unwrap();
    let paths = serde_json::to_vec(&serde_json::json!({"version": 1, "paths": [large]})).unwrap();
    let files = files(&disk, Mode::Budget);
    files.swept().await;
    let connects = disk.connects.load(Ordering::Acquire);
    assert!(files.serve(42, None, async { Some(paths) }).await.is_none());
    assert_eq!(disk.connects.load(Ordering::Acquire), connects);
    assert_eq!(disk.uploads.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn the_interim_budget_expires_and_the_upload_goes_on_until_cancelled() {
    let disk = Disk::new(true);
    let copied = Copied::new();
    let files = files(&disk, Mode::Budget);
    let (reply, ()) = tokio::join!(serve(&files, 42, &copied), async {
        disk.until(|disk| disk.waiting.load(Ordering::Acquire) > 0)
            .await;
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(11)).await;
        assert_eq!(disk.partials(), 1);
        tokio::time::advance(Duration::from_secs(1)).await;
    });
    assert!(reply.is_none());
    assert_eq!(disk.partials(), 1);
    files.observe(43);
    disk.until(|disk| disk.names().is_empty()).await;
}

/// A paste that hit the budget fails; the upload finishes in the background
/// and the next paste of the same copy gets it without uploading again.
#[tokio::test]
async fn a_budget_upload_finishes_in_the_background_and_the_next_paste_reuses_it() {
    let disk = Disk::new(true);
    let copied = Copied::new();
    let files = files(&disk, Mode::Budget);
    let (failed, ()) = tokio::join!(failure(&files, 42, copied.paths()), async {
        disk.until(|disk| disk.waiting.load(Ordering::Acquire) > 0)
            .await;
        tokio::time::pause();
        tokio::time::advance(BUDGET).await;
    });
    assert!(reason(failed).starts_with("Copying took too long"));
    assert_eq!(disk.partials(), 1);

    disk.writes.add_permits(Semaphore::MAX_PERMITS);
    disk.until(|disk| disk.names().len() == 1 && disk.partials() == 0)
        .await;
    let polled = AtomicBool::new(false);
    let reply = files
        .serve(42, None, async {
            polled.store(true, Ordering::Release);
            None
        })
        .await;
    assert!(reply.is_some());
    assert!(!polled.load(Ordering::Acquire));
    assert_eq!(disk.uploads.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn a_second_paste_during_a_budget_upload_waits_for_the_same_upload() {
    let disk = Disk::new(true);
    let copied = Copied::new();
    let files = files(&disk, Mode::Budget);
    let (failed, ()) = tokio::join!(failure(&files, 42, copied.paths()), async {
        disk.until(|disk| disk.waiting.load(Ordering::Acquire) > 0)
            .await;
        tokio::time::pause();
        tokio::time::advance(BUDGET).await;
    });
    assert!(reason(failed).starts_with("Copying took too long"));
    let (reply, ()) = tokio::join!(serve(&files, 42, &copied), async {
        disk.writes.add_permits(Semaphore::MAX_PERMITS);
    });
    assert!(reply.is_some());
    assert_eq!(disk.uploads.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn keep_alives_are_due_every_five_seconds_while_an_upload_runs() {
    let disk = Disk::new(true);
    let copied = Copied::new();
    let files = files(&disk, Mode::LongRead);
    let mut keep_alives = KeepAlives::default();
    let mut sent = Vec::new();
    let read = serve(&files, 42, &copied);
    tokio::pin!(read);
    tokio::select! {
        _ = &mut read => panic!("blocked upload completed"),
        () = disk.until(|disk| disk.waiting.load(Ordering::Acquire) > 0) => {}
    }
    tokio::time::pause();
    let start = Instant::now();
    keep_alives.start(7, "request 7");
    // Same period as the integration timer.
    let mut timer = tokio::time::interval(Duration::from_millis(200));
    let reply = loop {
        tokio::select! {
            reply = &mut read => break reply,
            _ = timer.tick() => {
                // Paused timers fire at whole milliseconds.
                for request in keep_alives.due(Instant::now()) {
                    sent.push((request, (Instant::now() - start).as_secs()));
                }
                if Instant::now() - start >= Duration::from_secs(16) {
                    disk.writes.add_permits(Semaphore::MAX_PERMITS);
                }
            }
        }
    };
    assert!(reply.is_some());
    keep_alives.finish(7);
    // Finishing the upload may take a few more (paused) timer ticks.
    assert_eq!(
        sent[..3],
        [("request 7", 5), ("request 7", 10), ("request 7", 15)]
    );
    assert!(sent.windows(2).all(|pair| pair[1].1 - pair[0].1 == 5));
    tokio::time::advance(Duration::from_secs(10)).await;
    assert!(keep_alives.due(Instant::now()).is_empty());
}

#[tokio::test]
async fn superseded_uploads_are_removed_after_the_grace_period() {
    let disk = Disk::new(false);
    let copied = Copied::new();
    let files = files(&disk, Mode::LongRead);
    serve(&files, 42, &copied).await.unwrap();
    let first = disk.names();
    // Reading a newer copy retires the older upload. Uploads run before the
    // clock is paused: their blocking file reads would let it auto-advance.
    serve(&files, 43, &copied).await.unwrap();
    assert_eq!(disk.names().len(), 2);
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(9 * 60)).await;
    files.tick();
    tokio::task::yield_now().await;
    assert_eq!(disk.names().len(), 2);
    tokio::time::advance(Duration::from_secs(60)).await;
    files.tick();
    disk.until(|disk| disk.names().len() == 1).await;
    assert!(!disk.names().contains(&first[0]));
    // Revoking permission removes the uploads at once.
    files.revoke();
    disk.until(|disk| disk.names().is_empty()).await;
}

#[tokio::test]
async fn ending_the_session_removes_completed_and_retired_uploads() {
    let disk = Disk::new(false);
    let copied = Copied::new();
    let files = files(&disk, Mode::LongRead);
    serve(&files, 42, &copied).await.unwrap();
    serve(&files, 43, &copied).await.unwrap();
    assert_eq!(disk.names().len(), 2);
    files.shutdown().await;
    assert!(disk.names().is_empty(), "{:?}", disk.names());
    // Nothing is left to remove or to reuse.
    files.shutdown().await;
    assert!(serve(&files, 43, &copied).await.is_some());
}

#[tokio::test]
async fn ending_the_session_cancels_a_running_upload() {
    let disk = Disk::new(true);
    let copied = Copied::new();
    let files = files(&disk, Mode::Budget);
    let paths = copied.paths();
    let (failed, ()) = tokio::join!(failure(&files, 42, paths), async {
        disk.until(|disk| disk.waiting.load(Ordering::Acquire) > 0)
            .await;
        files.shutdown().await;
    });
    assert_eq!(failed, Failure::Superseded);
    disk.until(|disk| disk.names().is_empty()).await;
}

struct Unreachable;
impl Connect for Unreachable {
    type Remote = Disk;
    async fn connect(&self) -> Result<Arc<Disk>> {
        std::future::pending().await
    }
}

#[tokio::test(start_paused = true)]
async fn ending_the_session_never_waits_longer_than_its_bound() {
    let files = Files::new(Unreachable, Mode::LongRead);
    files
        .state
        .lock()
        .unwrap()
        .retired
        .push(("folder".into(), Instant::now()));
    tokio::time::timeout(SHUTDOWN_TIMEOUT + Duration::from_secs(1), files.shutdown())
        .await
        .expect("shutdown is bounded");
}

#[tokio::test]
async fn reusing_a_cached_reply_refreshes_the_upload_for_other_desktops() {
    let disk = Disk::new(false);
    let copied = Copied::new();
    let files = files(&disk, Mode::LongRead);
    let reply = serve(&files, 42, &copied).await.unwrap();
    let folder = disk.root().join(&disk.names()[0]);
    age(&folder, Duration::from_secs(2 * 60 * 60));
    let touches = disk.touches.load(Ordering::Acquire);
    assert_eq!(serve(&files, 42, &copied).await.unwrap(), reply);
    assert_eq!(disk.touches.load(Ordering::Acquire), touches + 1);
    let modified = std::fs::metadata(&folder).unwrap().modified().unwrap();
    assert!(modified.elapsed().unwrap() < Duration::from_secs(60));
    // Another desktop starting now keeps it.
    sweep(&*disk, SystemTime::now()).await.unwrap();
    assert_eq!(disk.names().len(), 1);
    assert_eq!(disk.uploads.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn a_cached_reply_whose_upload_vanished_is_uploaded_again() {
    let disk = Disk::new(false);
    let copied = Copied::new();
    let files = files(&disk, Mode::LongRead);
    let first = String::from_utf8(serve(&files, 42, &copied).await.unwrap()).unwrap();
    let folder = disk.root().join(&disk.names()[0]);
    std::fs::remove_dir_all(&folder).unwrap();
    let second = String::from_utf8(serve(&files, 42, &copied).await.unwrap()).unwrap();
    assert_ne!(first, second);
    assert_eq!(disk.uploads.load(Ordering::Acquire), 2);
    let names = disk.names();
    assert_eq!(names.len(), 1, "{names:?}");
    assert_ne!(disk.root().join(&names[0]), folder);
    assert!(second.contains(&names[0]));
    // The replacement is the cached one now.
    let third = String::from_utf8(serve(&files, 42, &copied).await.unwrap()).unwrap();
    assert_eq!(second, third);
    assert_eq!(disk.uploads.load(Ordering::Acquire), 2);
}

fn age(path: &Path, by: Duration) {
    std::fs::File::open(path)
        .unwrap()
        .set_modified(SystemTime::now() - by)
        .unwrap();
}

#[tokio::test]
async fn a_session_start_sweeps_stale_partials_and_uploads_only() {
    let disk = Disk::new(false);
    let root = disk.root();
    std::fs::create_dir_all(&root).unwrap();
    let name = |prefix: &str| format!("{prefix}{}", uuid::Uuid::new_v4());
    let (stale_partial, stale, fresh, foreign) = (name(PARTIAL), name(""), name(""), name(PARTIAL));
    for dir in [&stale_partial, &stale, &fresh, &foreign] {
        std::fs::create_dir_all(root.join(dir).join("inner")).unwrap();
        std::fs::write(root.join(dir).join("inner/file"), b"x").unwrap();
    }
    let old = Duration::from_secs(2 * 60 * 60);
    age(&root.join(&stale_partial), old);
    age(&root.join(&stale), old);
    // Another desktop's upload in progress was modified moments ago.
    age(&root.join(&foreign), Duration::from_secs(5 * 60));
    std::fs::write(root.join("unrelated"), b"kept").unwrap();
    let files = files(&disk, Mode::LongRead);
    files.swept().await;
    let mut expected = vec![fresh, foreign.clone(), "unrelated".to_owned()];
    expected.sort();
    assert_eq!(disk.names(), expected);
    assert_eq!(
        std::fs::read(root.join(&foreign).join("inner/file")).unwrap(),
        b"x"
    );
}

#[tokio::test]
async fn a_running_upload_refreshes_its_partial_before_other_sweeps_find_it_stale() {
    let disk = Disk::new(true);
    let copied = Copied::new();
    let files = files(&disk, Mode::LongRead);
    let (reply, ()) = tokio::join!(serve(&files, 42, &copied), async {
        disk.until(|disk| disk.waiting.load(Ordering::Acquire) > 0)
            .await;
        let partial = disk.root().join(&disk.names()[0]);
        assert!(disk.names()[0].starts_with(PARTIAL));
        age(&partial, Duration::from_secs(50 * 60));
        tokio::time::pause();
        tokio::time::advance(REFRESH).await;
        disk.until(|disk| disk.touches.load(Ordering::Acquire) > 0)
            .await;
        let modified = std::fs::metadata(&partial).unwrap().modified().unwrap();
        assert!(modified.elapsed().unwrap() < Duration::from_secs(60));
        // Another desktop starting now keeps the refreshed upload.
        sweep(&*disk, SystemTime::now()).await.unwrap();
        assert_eq!(disk.partials(), 1);
        files.observe(43);
    });
    assert!(reply.is_none());
    disk.until(|disk| disk.names().is_empty()).await;
}

/// The real [`crate::ssh::sftp::Sftp`] against the fixture of
/// `cargo xtask test ssh`, whose SFTP server works in a real directory.
pub(crate) mod fixture {
    use super::*;
    use crate::ssh::sftp::Sftp;
    use russh_sftp::client::error::Error;
    use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};

    struct Fixture;
    impl Connect for Fixture {
        type Remote = Sftp;
        async fn connect(&self) -> Result<Arc<Sftp>> {
            Ok(Arc::new(Sftp::connect(&server()).await?))
        }
    }
    pub(crate) fn server() -> crate::model::Server {
        serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::new_v4(), "name": "Clipboard fixture",
            "sshUser": "fixture", "sshHost": "127.0.0.1",
            "sshPort": std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
            "identityFile": std::env::var("NEWPORT_TEST_IDENTITY").unwrap(),
        }))
        .unwrap()
    }

    /// A connection with an empty clipboard cache.
    async fn clean() -> (Arc<Sftp>, String) {
        let sftp = Arc::new(Sftp::connect(&server()).await.unwrap());
        let root = root_of(&sftp.home().await.unwrap());
        remove_tree(&*sftp, &root).await.unwrap();
        (sftp, root)
    }
    async fn names(sftp: &Sftp, root: &str) -> Vec<String> {
        let mut names: Vec<String> = sftp
            .list(root)
            .await
            .unwrap()
            .unwrap_or_default()
            .into_iter()
            .map(|item| item.name)
            .collect();
        names.sort();
        names
    }
    async fn read(sftp: &Sftp, path: &str) -> Vec<u8> {
        let handle = sftp
            .session
            .open(path, OpenFlags::READ, FileAttributes::empty())
            .await
            .unwrap()
            .handle;
        let mut bytes = Vec::new();
        loop {
            match sftp.session.read(&handle, bytes.len() as u64, 65536).await {
                Ok(data) => bytes.extend(data.data),
                Err(Error::Status(status)) if status.status_code == StatusCode::Eof => break,
                Err(error) => panic!("{error}"),
            }
        }
        sftp.session.close(handle).await.unwrap();
        bytes
    }
    async fn mode(sftp: &Sftp, path: &str) -> u32 {
        sftp.session
            .stat(path)
            .await
            .unwrap()
            .attrs
            .permissions
            .unwrap()
            & 0o777
    }
    async fn upload(copied: &Copied) -> Option<Vec<u8>> {
        let files = Files::new(Fixture, Mode::LongRead);
        let paths = copied.paths();
        files.serve(42, None, async { Some(paths) }).await
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- clipboard_files::tests::fixture"]
    async fn a_multi_megabyte_file_round_trips_over_pipelined_sftp() {
        let (sftp, root) = clean().await;
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.bin");
        let bytes: Vec<u8> = (0..3 * 1024 * 1024 + 12_345)
            .map(|i| (i % 251) as u8)
            .collect();
        std::fs::write(&big, &bytes).unwrap();
        let paths = serde_json::to_vec(&serde_json::json!({"version": 1, "paths": [big]})).unwrap();
        let reply = Files::new(Fixture, Mode::LongRead)
            .serve(42, None, async { Some(paths) })
            .await
            .unwrap();
        let reply: serde_json::Value = serde_json::from_slice(&reply).unwrap();
        let remote = reply["paths"][0].as_str().unwrap().to_owned();
        assert!(read(&sftp, &remote).await == bytes, "upload landed wrongly");
        assert!(
            !names(&sftp, &root)
                .await
                .iter()
                .any(|name| name.starts_with(PARTIAL))
        );

        // Read back with several requests outstanding, with and without the size.
        for size in [Some(bytes.len() as u64), None] {
            let handle = sftp.open_read(&remote).await.unwrap();
            let mut copy = Vec::new();
            let copied = crate::files::transfer::read_file_capped(
                &mut copy,
                |offset, len| sftp.read(&handle, offset, len),
                size,
                None,
                |_| {},
            )
            .await
            .unwrap();
            sftp.close(handle).await.unwrap();
            assert_eq!(copied, bytes.len() as u64);
            assert!(copy == bytes, "download differs ({size:?})");
        }
        remove_tree(&*sftp, &root).await.unwrap();
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- clipboard_files::tests::fixture"]
    async fn a_tree_is_uploaded_over_sftp_and_renamed_into_place() {
        let (sftp, root) = clean().await;
        let copied = Copied::new();
        let reply = upload(&copied).await.unwrap();
        let names = names(&sftp, &root).await;
        assert_eq!(names.len(), 1, "{names:?}");
        uuid::Uuid::try_parse(&names[0]).unwrap();
        let folder = format!("{root}/{}", names[0]);
        assert_eq!(
            String::from_utf8(reply).unwrap(),
            format!(r#"{{"version":1,"paths":["{folder}/docs","{folder}/名前 1.txt"]}}"#)
        );
        assert_eq!(
            read(&sftp, &format!("{folder}/docs/nested/deep.txt")).await,
            b"deep"
        );
        // The link was stored as written, then reads through to its target.
        let link = format!("{folder}/docs/link");
        let target = sftp.session.readlink(&link).await.unwrap().files;
        assert_eq!(target[0].filename, "nested/deep.txt");
        assert_eq!(sftp.kind(&link).await.unwrap(), Some(Kind::File));
        assert!(sftp.session.lstat(&link).await.unwrap().attrs.is_symlink());
        assert_eq!(read(&sftp, &link).await, b"deep");
        let named = format!("{folder}/名前 1.txt");
        assert_eq!(read(&sftp, &named).await, vec![7u8; 100_000]);
        assert_eq!(mode(&sftp, &named).await, 0o755);
        assert_eq!(mode(&sftp, &folder).await, 0o700);
        assert_eq!(mode(&sftp, &format!("{folder}/docs")).await, 0o750);
        assert_eq!(mode(&sftp, &format!("{folder}/docs/nested")).await, 0o755);
        for (path, seconds) in [
            (named.clone(), 1_500_000_000),
            (format!("{folder}/docs/nested/deep.txt"), 1_600_000_000),
            (format!("{folder}/docs/nested"), 1_400_000_000),
            (format!("{folder}/docs"), 1_300_000_000),
        ] {
            let stat = sftp.lstat(&path).await.unwrap().unwrap();
            assert_eq!(stat.mtime, Some(seconds), "{path}");
        }
        assert_eq!(mode(&sftp, &root).await, 0o700);
        assert!(names.iter().all(|name| !name.starts_with(PARTIAL)));
        remove_tree(&*sftp, &root).await.unwrap();
        assert!(names_after_removal(&sftp, &root).await);
    }
    async fn names_after_removal(sftp: &Sftp, root: &str) -> bool {
        sftp.list(root).await.unwrap().is_none()
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- clipboard_files::tests::fixture"]
    async fn a_failed_upload_over_sftp_removes_its_partial_tree() {
        let (sftp, root) = clean().await;
        let copied = Copied::new();
        // Planning only inspects files, so the second one fails while being
        // opened, after the first and the directories were already created.
        let tree = copied.dir.path().join("tree");
        std::fs::create_dir_all(tree.join("sub")).unwrap();
        std::fs::write(tree.join("a.txt"), b"written first").unwrap();
        std::os::unix::fs::symlink("../a.txt", tree.join("sub/link")).unwrap();
        std::fs::write(tree.join("sub/b.txt"), b"unreadable").unwrap();
        std::fs::set_permissions(
            tree.join("sub/b.txt"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        let paths =
            serde_json::to_vec(&serde_json::json!({"version": 1, "paths": [tree]})).unwrap();
        let files = Files::new(Fixture, Mode::LongRead);
        assert!(
            files
                .serve(7, None, async { Some(paths.clone()) })
                .await
                .is_none()
        );
        assert_eq!(names(&sftp, &root).await, Vec::<String>::new());

        std::fs::set_permissions(
            tree.join("sub/b.txt"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert!(files.serve(7, None, async { Some(paths) }).await.is_some());
        let names = names(&sftp, &root).await;
        assert_eq!(names.len(), 1, "{names:?}");
        let folder = format!("{root}/{}", names[0]);
        assert_eq!(
            read(&sftp, &format!("{folder}/tree/sub/b.txt")).await,
            b"unreadable"
        );
        remove_tree(&*sftp, &root).await.unwrap();
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- clipboard_files::tests::fixture"]
    async fn the_sweep_over_sftp_keeps_fresh_partials_and_touch_refreshes_one() {
        let (sftp, root) = clean().await;
        ensure_root(&*sftp).await.unwrap();
        let partial = format!("{root}/{PARTIAL}{}", uuid::Uuid::new_v4());
        let finished = format!("{root}/{}", uuid::Uuid::new_v4());
        for dir in [&partial, &finished] {
            sftp.mkdir(dir, 0o700).await.unwrap();
            sftp.mkdir(&format!("{dir}/inner"), 0o700).await.unwrap();
            sftp.symlink(&format!("{dir}/inner/link"), "missing")
                .await
                .unwrap();
            let handle = sftp
                .create(&format!("{dir}/inner/file"), 0o600)
                .await
                .unwrap();
            sftp.write(&handle, 0, b"x".to_vec()).await.unwrap();
            sftp.close(handle).await.unwrap();
        }
        let now = SystemTime::now();
        sweep(&*sftp, now).await.unwrap();
        assert_eq!(names(&sftp, &root).await.len(), 2);
        sweep(&*sftp, now + Duration::from_secs(2 * 60 * 60))
            .await
            .unwrap();
        assert!(names(&sftp, &root).await.is_empty());

        sftp.mkdir(&partial, 0o700).await.unwrap();
        let old = FileAttributes {
            atime: Some(1_000_000_000),
            mtime: Some(1_000_000_000),
            ..FileAttributes::empty()
        };
        sftp.session.setstat(&partial, old).await.unwrap();
        let mtime = |attrs: FileAttributes| attrs.mtime.unwrap();
        assert_eq!(
            mtime(sftp.session.stat(&partial).await.unwrap().attrs),
            1_000_000_000
        );
        sftp.touch(&partial).await.unwrap();
        let modified = mtime(sftp.session.stat(&partial).await.unwrap().attrs);
        let now = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(
            u64::from(modified).abs_diff(now) < 60,
            "{modified} vs {now}"
        );
        sweep(&*sftp, SystemTime::now()).await.unwrap();
        assert_eq!(names(&sftp, &root).await.len(), 1);
        remove_tree(&*sftp, &root).await.unwrap();
    }
}

#[tokio::test]
async fn unreadable_local_paths_fail_with_a_reason_and_the_next_paste_retries() {
    let copied = Copied::new();
    let disk = Disk::new(false);
    let files = self::files(&disk, Mode::LongRead);
    files.observe(3);
    let first = files
        .serve_result(3, None, async { None })
        .await
        .unwrap_err();
    assert!(reason(first).contains("could not be read"));
    // The copy is still the current one: the next paste reads it again.
    let paths = copied.paths();
    let again = files.serve_result(3, None, async { Some(paths) }).await;
    assert!(again.is_ok());
}

#[tokio::test]
async fn too_many_copied_items_name_the_limit() {
    let files = self::files(&Disk::new(false), Mode::LongRead);
    let items: Vec<_> = (0..10_001).map(|index| format!("/tmp/{index}")).collect();
    let paths = serde_json::to_vec(&serde_json::json!({"version": 1, "paths": items})).unwrap();
    let failed = files
        .serve_result(9, None, async { Some(paths) })
        .await
        .unwrap_err();
    assert!(reason(failed).contains("Too many files are copied (up to 10,000)"));
}
