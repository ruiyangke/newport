//! Serves reads of `application/x-newport-files`: the files copied on the Mac
//! are uploaded with SFTP into the server's `~/.cache/newport/clipboard/<uuid>/`
//! and the reply lists the top-level items there. One upload serves every read
//! of a copy; a newer copy, revoked permission or the end of the session
//! cancels it. An upload outlives a read that gave up waiting, so a later
//! paste of the same copy reuses it.
use crate::files::child;
use anyhow::{Context, Result, bail};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::sync::{OnceCell, Semaphore, watch};
use tokio::time::Instant;

/// Clipboard service version whose waits treat an empty not-done reply as
/// progress, so a read can last as long as its upload.
pub(crate) const LONG_READ_VERSION: u32 = 3;
/// Clipboard service version served within [`BUDGET`] until servers update.
const INTERIM_VERSION: u32 = 2;
pub(crate) const KEEP_ALIVE: Duration = Duration::from_secs(5);
/// Older servers fail a read after a fixed time; stay under it.
const BUDGET: Duration = Duration::from_secs(12);
const BUDGET_BYTES: u64 = 64 << 20;
/// The longest a server waits for a long read.
const LONG_READ_CEILING: Duration = Duration::from_secs(30 * 60);
/// A superseded copy stays available to a paste already reading it.
const GRACE: Duration = Duration::from_secs(10 * 60);
/// Uploads and staging directories older than this are removed when a session
/// starts. Another desktop of the same account may be uploading meanwhile, so
/// a staging directory is only old when nobody has refreshed it for this long.
const STALE: Duration = Duration::from_secs(60 * 60);
/// How often a running upload refreshes the modification time of its staging
/// directory; well below [`STALE`].
const REFRESH: Duration = Duration::from_secs(2 * 60);
const MAX_REPLY: usize = 1 << 20;
const MAX_UPLOADS: usize = 2;
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(60);
/// How long ending a session waits to remove the uploads it made.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
/// How long reusing an upload waits to confirm and refresh its folder.
const REUSE_TIMEOUT: Duration = Duration::from_secs(5);
const CACHE: [&str; 3] = [".cache", "newport", "clipboard"];
const PARTIAL: &str = ".partial-";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Keep-alives hold the read open while the upload runs.
    LongRead,
    /// The read must finish within [`BUDGET`]; larger copies are refused.
    Budget,
}

/// How files are offered to a server whose clipboard service supports the
/// versions `supports` accepts, or None when they are not offered at all.
pub(crate) fn mode(supports: impl Fn(u32) -> bool) -> Option<Mode> {
    if !cfg!(unix) {
        None
    } else if supports(LONG_READ_VERSION) {
        Some(Mode::LongRead)
    } else if supports(INTERIM_VERSION) {
        Some(Mode::Budget)
    } else {
        None
    }
}

impl Mode {
    pub(crate) fn keep_alive(self) -> bool {
        self == Mode::LongRead
    }
    fn deadline(self) -> Duration {
        match self {
            Mode::LongRead => LONG_READ_CEILING,
            Mode::Budget => BUDGET,
        }
    }
    fn admits(self, bytes: u64) -> bool {
        self == Mode::LongRead || bytes <= BUDGET_BYTES
    }
}

/// Reads whose upload is running, each with the time of its next keep-alive.
pub(crate) struct KeepAlives<T>(Vec<(u64, T, Instant)>);
impl<T> Default for KeepAlives<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}
impl<T: Clone> KeepAlives<T> {
    pub(crate) fn start(&mut self, id: u64, request: T) {
        self.0.push((id, request, Instant::now() + KEEP_ALIVE));
    }
    /// Called before the final reply is queued, so no keep-alive follows it.
    pub(crate) fn finish(&mut self, id: u64) {
        if let Some(index) = self.0.iter().position(|(pending, ..)| *pending == id) {
            self.0.remove(index);
        }
    }
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }
    /// The requests owed a keep-alive at `now`.
    pub(crate) fn due(&mut self, now: Instant) -> Vec<T> {
        let mut due = Vec::new();
        for (_, request, next) in &mut self.0 {
            if *next <= now {
                *next = now + KEEP_ALIVE;
                due.push(request.clone());
            }
        }
        due
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Directory,
    File,
    Link,
    Other,
}
pub(crate) struct Item {
    pub(crate) name: String,
    pub(crate) kind: Kind,
    /// Seconds since the Unix epoch.
    pub(crate) mtime: Option<u32>,
    #[cfg(unix)]
    pub(crate) size: u64,
    /// Unix permission bits.
    pub(crate) mode: u32,
}
/// What `lstat` reports about one path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stat {
    pub(crate) kind: Kind,
    #[cfg(unix)]
    pub(crate) size: u64,
    /// Seconds since the Unix epoch.
    pub(crate) mtime: Option<u32>,
    /// Unix permission bits.
    pub(crate) mode: u32,
}

/// The SFTP operations an upload or a download uses.
pub(crate) trait RemoteFs: Send + Sync + 'static {
    /// The absolute path of the home directory.
    fn home(&self) -> impl Future<Output = Result<String>> + Send;
    /// The canonical absolute form of `path`, following links; a relative
    /// path is resolved against the home directory.
    #[cfg(unix)]
    fn realpath(&self, path: &str) -> impl Future<Output = Result<String>> + Send;
    /// The kind of `path`, following links; None when it does not exist.
    fn kind(&self, path: &str) -> impl Future<Output = Result<Option<Kind>>> + Send;
    /// `path` itself, without following a final link; None when it does not
    /// exist.
    #[cfg(unix)]
    fn lstat(&self, path: &str) -> impl Future<Output = Result<Option<Stat>>> + Send;
    /// The target of a link, as stored.
    #[cfg(unix)]
    fn readlink(&self, path: &str) -> impl Future<Output = Result<String>> + Send;
    /// The entries of a directory, without following links; None when it
    /// does not exist.
    fn list(&self, path: &str) -> impl Future<Output = Result<Option<Vec<Item>>>> + Send;
    /// Opens an existing file for reading and returns its handle.
    #[cfg(unix)]
    fn open_read(&self, path: &str) -> impl Future<Output = Result<String>> + Send;
    /// Up to `len` bytes at `offset`; None at the end of the file.
    #[cfg(unix)]
    fn read(
        &self,
        handle: &str,
        offset: u64,
        len: u32,
    ) -> impl Future<Output = Result<Option<Vec<u8>>>> + Send;
    fn mkdir(&self, path: &str, mode: u32) -> impl Future<Output = Result<()>> + Send;
    /// Creates a new file for writing and returns its handle; fails if it exists.
    #[cfg(unix)]
    fn create(&self, path: &str, mode: u32) -> impl Future<Output = Result<String>> + Send;
    #[cfg(unix)]
    fn write(
        &self,
        handle: &str,
        offset: u64,
        data: Vec<u8>,
    ) -> impl Future<Output = Result<()>> + Send;
    #[cfg(unix)]
    fn close(&self, handle: String) -> impl Future<Output = Result<()>> + Send;
    #[cfg(unix)]
    fn symlink(&self, link: &str, target: &str) -> impl Future<Output = Result<()>> + Send;
    /// Fails if `to` exists.
    fn rename(&self, from: &str, to: &str) -> impl Future<Output = Result<()>> + Send;
    fn remove_file(&self, path: &str) -> impl Future<Output = Result<()>> + Send;
    fn remove_dir(&self, path: &str) -> impl Future<Output = Result<()>> + Send;
    /// Sets the access and modification times of `path` to now.
    fn touch(&self, path: &str) -> impl Future<Output = Result<()>> + Send;
    /// Sets the access and modification times of `path` to `mtime` (seconds
    /// since the Unix epoch) and its permission bits to `mode`, whichever are
    /// given. Follows links, so it is never called for one.
    fn set_attributes(
        &self,
        path: &str,
        mtime: Option<u32>,
        mode: Option<u32>,
    ) -> impl Future<Output = Result<()>> + Send;
}

pub(crate) trait Connect: Send + Sync + 'static {
    type Remote: RemoteFs;
    fn connect(&self) -> impl Future<Output = Result<Arc<Self::Remote>>> + Send;
}

/// The server's authenticated connection.
pub(crate) struct Shared(pub(crate) Arc<crate::server_session::ServerSession>);
impl Connect for Shared {
    type Remote = crate::ssh::sftp::Sftp;
    async fn connect(&self) -> Result<Arc<Self::Remote>> {
        Ok(Arc::new(crate::ssh::sftp::Sftp::shared(&self.0).await?))
    }
}

/// The most items one copy of files may hold.
pub(crate) const MAX_ENTRIES: u64 = 10_000;
pub(crate) const TOO_LARGE: &str =
    "The copied files are larger than 2 GiB or hold more than 10,000 items";

mod sftp {
    use super::{Item, Kind, RemoteFs, Stat};
    use anyhow::{Context, Result, bail};
    use russh_sftp::client::error::Error;
    #[cfg(unix)]
    use russh_sftp::protocol::OpenFlags;
    use russh_sftp::protocol::{FileAttributes, StatusCode};

    fn status(error: &Error, code: StatusCode) -> bool {
        matches!(error, Error::Status(status) if status.status_code == code)
    }
    fn kind(attrs: &FileAttributes) -> Kind {
        if attrs.is_symlink() {
            Kind::Link
        } else if attrs.is_dir() {
            Kind::Directory
        } else if attrs.is_regular() {
            Kind::File
        } else {
            Kind::Other
        }
    }
    fn stat(attrs: &FileAttributes) -> Stat {
        Stat {
            kind: kind(attrs),
            #[cfg(unix)]
            size: attrs.size.unwrap_or(0),
            mtime: attrs.mtime,
            mode: attrs.permissions.unwrap_or(0) & 0o7777,
        }
    }
    fn permissions(mode: u32) -> FileAttributes {
        FileAttributes {
            permissions: Some(mode),
            ..FileAttributes::empty()
        }
    }

    impl RemoteFs for crate::ssh::sftp::Sftp {
        async fn home(&self) -> Result<String> {
            self.session
                .realpath(".")
                .await?
                .files
                .into_iter()
                .next()
                .map(|entry| entry.filename)
                .context("Server returned no home folder")
        }
        #[cfg(unix)]
        async fn realpath(&self, path: &str) -> Result<String> {
            self.session
                .realpath(path)
                .await?
                .files
                .into_iter()
                .next()
                .map(|entry| entry.filename)
                .context("Server returned no canonical path")
        }
        async fn kind(&self, path: &str) -> Result<Option<Kind>> {
            match self.session.stat(path).await {
                Ok(found) => Ok(Some(kind(&found.attrs))),
                Err(error) if status(&error, StatusCode::NoSuchFile) => Ok(None),
                Err(error) => Err(error.into()),
            }
        }
        #[cfg(unix)]
        async fn lstat(&self, path: &str) -> Result<Option<Stat>> {
            match self.session.lstat(path).await {
                Ok(found) => Ok(Some(stat(&found.attrs))),
                Err(error) if status(&error, StatusCode::NoSuchFile) => Ok(None),
                Err(error) => Err(error.into()),
            }
        }
        #[cfg(unix)]
        async fn readlink(&self, path: &str) -> Result<String> {
            self.session
                .readlink(path)
                .await?
                .files
                .into_iter()
                .next()
                .map(|entry| entry.filename)
                .context("Server returned no link target")
        }
        async fn list(&self, path: &str) -> Result<Option<Vec<Item>>> {
            let handle = match self.session.opendir(path).await {
                Ok(handle) => handle.handle,
                Err(error) if status(&error, StatusCode::NoSuchFile) => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            let mut items = Vec::new();
            let result = async {
                loop {
                    let batch = match self.session.readdir(&handle).await {
                        Ok(batch) => batch,
                        Err(error) if status(&error, StatusCode::Eof) => return Ok(()),
                        Err(error) => return Err(error.into()),
                    };
                    if batch.files.is_empty() {
                        bail!("Server returned an empty directory response without finishing");
                    }
                    for entry in batch.files {
                        if entry.filename != "." && entry.filename != ".." {
                            let found = stat(&entry.attrs);
                            items.push(Item {
                                name: entry.filename,
                                kind: found.kind,
                                mtime: found.mtime,
                                #[cfg(unix)]
                                size: found.size,
                                mode: found.mode,
                            });
                            // One folder never needs more than the copy limit
                            // plus one entry to be refused.
                            if items.len() as u64 > super::MAX_ENTRIES {
                                bail!(super::TOO_LARGE);
                            }
                        }
                    }
                }
            }
            .await;
            let _ = self.session.close(handle).await;
            result.map(|()| Some(items))
        }
        async fn mkdir(&self, path: &str, mode: u32) -> Result<()> {
            self.session.mkdir(path, permissions(mode)).await?;
            Ok(())
        }
        #[cfg(unix)]
        async fn create(&self, path: &str, mode: u32) -> Result<String> {
            let flags = OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE;
            Ok(self
                .session
                .open(path, flags, permissions(mode))
                .await?
                .handle)
        }
        #[cfg(unix)]
        async fn open_read(&self, path: &str) -> Result<String> {
            Ok(self
                .session
                .open(path, OpenFlags::READ, FileAttributes::empty())
                .await?
                .handle)
        }
        #[cfg(unix)]
        async fn read(&self, handle: &str, offset: u64, len: u32) -> Result<Option<Vec<u8>>> {
            match self.session.read(handle, offset, len).await {
                Ok(data) => Ok(Some(data.data)),
                Err(error) if status(&error, StatusCode::Eof) => Ok(None),
                Err(error) => Err(error.into()),
            }
        }
        #[cfg(unix)]
        async fn write(&self, handle: &str, offset: u64, data: Vec<u8>) -> Result<()> {
            self.session.write(handle, offset, data).await?;
            Ok(())
        }
        #[cfg(unix)]
        async fn close(&self, handle: String) -> Result<()> {
            self.session.close(handle).await?;
            Ok(())
        }
        #[cfg(unix)]
        async fn symlink(&self, link: &str, target: &str) -> Result<()> {
            // OpenSSH's sftp-server reads SSH_FXP_SYMLINK's two paths in the
            // reverse of the draft's order: the target comes first.
            self.session.symlink(target, link).await?;
            let made = self.session.readlink(link).await?.files.into_iter().next();
            if made.is_none_or(|made| made.filename != target) {
                bail!("The server created a different symbolic link");
            }
            Ok(())
        }
        async fn rename(&self, from: &str, to: &str) -> Result<()> {
            // Standard SFTP v3 rename fails when the destination exists.
            self.session.rename(from, to).await?;
            Ok(())
        }
        async fn remove_file(&self, path: &str) -> Result<()> {
            self.session.remove(path).await?;
            Ok(())
        }
        async fn remove_dir(&self, path: &str) -> Result<()> {
            self.session.rmdir(path).await?;
            Ok(())
        }
        async fn touch(&self, path: &str) -> Result<()> {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs();
            // SFTP v3 sets both times or neither.
            let now = u32::try_from(now)?;
            let times = FileAttributes {
                atime: Some(now),
                mtime: Some(now),
                ..FileAttributes::empty()
            };
            self.session.setstat(path, times).await?;
            Ok(())
        }
        async fn set_attributes(
            &self,
            path: &str,
            mtime: Option<u32>,
            mode: Option<u32>,
        ) -> Result<()> {
            // SFTP v3 sets both times or neither.
            let attrs = FileAttributes {
                permissions: mode,
                atime: mtime,
                mtime,
                ..FileAttributes::empty()
            };
            self.session.setstat(path, attrs).await?;
            Ok(())
        }
    }
}

/// Removes `path` and everything under it, without following links. A
/// missing path is not an error.
pub(crate) async fn remove_tree<R: RemoteFs>(remote: &R, path: &str) -> Result<()> {
    // Folders copied with their own permissions can lack the owner's access.
    let mut stack = vec![(path.to_owned(), Kind::Directory, false, false)];
    while let Some((path, kind, listed, restore)) = stack.pop() {
        match kind {
            Kind::Directory if listed => remote.remove_dir(&path).await?,
            Kind::Directory => {
                if restore {
                    remote.set_attributes(&path, None, Some(0o700)).await?;
                }
                let Some(items) = remote.list(&path).await? else {
                    continue;
                };
                stack.push((path.clone(), Kind::Directory, true, false));
                for item in items {
                    let restore = item.kind == Kind::Directory && item.mode & 0o700 != 0o700;
                    stack.push((child(&path, &item.name)?, item.kind, false, restore));
                }
            }
            _ => remote.remove_file(&path).await?,
        }
    }
    Ok(())
}

fn spawn_cleanup(what: &'static str, work: impl Future<Output = Result<()>> + Send + 'static) {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    runtime.spawn(async move {
        match tokio::time::timeout(CLEANUP_TIMEOUT, work).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => log::warn!("Clipboard files {what} failed: {error:#}"),
            Err(_) => log::warn!("Clipboard files {what} timed out"),
        }
    });
}

/// An upload's staging directory, removed unless it was published. A dropped
/// upload leaves the removal to a task on the same connection.
struct Partial<R: RemoteFs> {
    remote: Arc<R>,
    path: Option<String>,
    /// Lists the path as staged until it is published, discarded or dropped.
    state: Arc<Mutex<State>>,
}
impl<R: RemoteFs> Partial<R> {
    fn new(remote: Arc<R>, path: String, state: Arc<Mutex<State>>) -> Self {
        state.lock().unwrap().staged.push(path.clone());
        Self {
            remote,
            path: Some(path),
            state,
        }
    }
    /// The upload was published: nothing is left to remove.
    fn publish(&mut self) {
        self.take();
    }
    fn take(&mut self) -> Option<String> {
        let path = self.path.take()?;
        self.state
            .lock()
            .unwrap()
            .staged
            .retain(|staged| *staged != path);
        Some(path)
    }
    async fn discard(&mut self) {
        if let Some(path) = self.take() {
            match tokio::time::timeout(CLEANUP_TIMEOUT, remove_tree(&*self.remote, &path)).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => log::warn!("Clipboard files cleanup failed: {error:#}"),
                Err(_) => log::warn!("Clipboard files cleanup timed out"),
            }
        }
    }
}
impl<R: RemoteFs> Drop for Partial<R> {
    fn drop(&mut self) {
        if let Some(path) = self.take() {
            let remote = self.remote.clone();
            spawn_cleanup("cleanup", async move { remove_tree(&*remote, &path).await });
        }
    }
}

fn root_of(home: &str) -> String {
    let mut root = home.trim_end_matches('/').to_owned();
    for name in CACHE {
        root.push('/');
        root.push_str(name);
    }
    root
}

/// `~/.cache/newport/clipboard`, creating each missing level with mode 0700.
async fn ensure_root<R: RemoteFs>(remote: &R) -> Result<String> {
    let home = remote.home().await?;
    if !home.starts_with('/') {
        bail!("Server returned a relative home folder");
    }
    let mut path = home.trim_end_matches('/').to_owned();
    for name in CACHE {
        path = child(&path, name)?;
        match remote.kind(&path).await? {
            Some(Kind::Directory) => {}
            Some(_) => bail!("The clipboard cache path on the server is not a folder"),
            None => {
                if let Err(error) = remote.mkdir(&path, 0o700).await
                    && remote.kind(&path).await? != Some(Kind::Directory)
                {
                    return Err(error);
                }
            }
        }
    }
    Ok(path)
}

/// Removes staging directories and uploads not modified for [`STALE`]. Only
/// names this module creates are touched.
async fn sweep<R: RemoteFs>(remote: &R, now: SystemTime) -> Result<()> {
    let root = root_of(&remote.home().await?);
    let Some(items) = remote.list(&root).await? else {
        return Ok(());
    };
    for item in items {
        let id = item.name.strip_prefix(PARTIAL).unwrap_or(&item.name);
        if uuid::Uuid::try_parse(id).is_err() {
            continue;
        }
        let stale = item.mtime.is_some_and(|mtime| {
            now.duration_since(SystemTime::UNIX_EPOCH + Duration::from_secs(mtime.into()))
                .is_ok_and(|age| age >= STALE)
        });
        if stale {
            let path = child(&root, &item.name)?;
            let removed = match item.kind {
                Kind::Directory => remove_tree(remote, &path).await,
                _ => remote.remove_file(&path).await,
            };
            if let Err(error) = removed {
                log::warn!("Clipboard files sweep could not remove an upload: {error:#}");
            }
        }
    }
    Ok(())
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalPaths {
    version: u32,
    paths: Vec<PathBuf>,
}
fn parse_paths(bytes: &[u8]) -> Result<Vec<PathBuf>> {
    let local: LocalPaths = serde_json::from_slice(bytes).context("Invalid local paths")?;
    if local.version != 1 {
        bail!("Unsupported local paths version");
    }
    Ok(local.paths)
}

#[derive(serde::Serialize)]
struct Reply {
    version: u32,
    paths: Vec<String>,
}
fn reply<'a>(folder: &str, names: impl Iterator<Item = &'a str>) -> Result<Arc<[u8]>> {
    let paths = names
        .map(|name| child(folder, name))
        .collect::<Result<_>>()?;
    let bytes = serde_json::to_vec(&Reply { version: 1, paths })?;
    if bytes.len() > MAX_REPLY {
        bail!("Too many copied items");
    }
    Ok(bytes.into())
}

#[cfg(unix)]
mod tree {
    use super::{RemoteFs, child};
    use anyhow::{Context, Result, anyhow, bail};
    use newport_platform::clipboard::files::{self, Entry, EntryKind, Limits};
    use std::path::{Component, Path, PathBuf};

    const CHANGED: &str = "A copied file changed during the transfer";

    /// Everything one copy creates on the server, parents first.
    pub(super) struct Tree {
        entries: Vec<Entry>,
        bytes: u64,
    }

    fn remote_path(base: &str, relative: &Path) -> Result<String> {
        let mut path = base.to_owned();
        for component in relative.components() {
            let Component::Normal(name) = component else {
                bail!("Invalid copied file name");
            };
            path = child(&path, name.to_str().context("Invalid copied file name")?)?;
        }
        Ok(path)
    }

    /// Applies a copied file's or folder's time and permissions. Best effort: a
    /// server that refuses leaves the copy with default times and modes.
    async fn finish<R: RemoteFs>(remote: &R, path: &str, mtime: Option<u32>, mode: u32) {
        if let Err(error) = remote.set_attributes(path, mtime, Some(mode)).await {
            log::debug!("Clipboard files could not set a copied file's attributes: {error:#}");
        }
    }

    /// Why the copied files cannot be transferred, for the person.
    pub(super) fn describe(error: &files::ManifestError) -> String {
        use files::ManifestError::*;
        let name = |path: &std::path::Path| {
            format!(
                "“{}”",
                path.file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
            )
        };
        match error {
            NoItems => "Nothing is copied".into(),
            TooManyItems | TooManyEntries => "Too many files are copied (up to 10,000)".into(),
            TooManyBytes => "The copied files are larger than 2 GB".into(),
            InvalidName(path) => format!("{} has a name that cannot be copied", name(path)),
            DuplicateName(path) => format!("Two copied items are named {}", name(path)),
            Unsupported(path) => format!("{} is not a file, folder or link", name(path)),
            EscapingLink(path) => format!("{} is a link that points outside the copy", name(path)),
            Io(path, kind) => format!("{} could not be read ({kind})", name(path)),
        }
    }

    impl Tree {
        pub(super) async fn plan(paths: Vec<PathBuf>) -> Result<Self> {
            let entries =
                tokio::task::spawn_blocking(move || files::plan(&paths, &Limits::CLIPBOARD))
                    .await?
                    .map_err(|error| anyhow!(describe(&error)))?;
            let bytes = entries.iter().map(|entry| entry.size).sum();
            Ok(Self { entries, bytes })
        }
        pub(super) fn bytes(&self) -> u64 {
            self.bytes
        }
        pub(super) fn top_level(&self) -> impl Iterator<Item = &str> {
            self.entries
                .iter()
                .filter(|entry| entry.path.components().count() == 1)
                .filter_map(|entry| entry.path.to_str())
        }
        /// Creates the tree under `folder`. Files and folders keep their
        /// modification times and permissions; `folder` itself stays as it is.
        pub(super) async fn write<R: RemoteFs>(&self, remote: &R, folder: &str) -> Result<()> {
            // Folders are finished last, children first: creating entries
            // changes a folder's time, and a copied folder may be read-only.
            let mut folders = Vec::new();
            for entry in &self.entries {
                let path = remote_path(folder, &entry.path)?;
                match &entry.kind {
                    EntryKind::Directory => {
                        remote.mkdir(&path, 0o700).await?;
                        folders.push((path, entry.mtime, entry.mode & 0o777));
                    }
                    EntryKind::Symlink { target } => {
                        let target = target.to_str().context("Invalid link target")?;
                        remote.symlink(&path, target).await?;
                    }
                    EntryKind::File => {
                        let mut source = tokio::fs::File::open(&entry.source).await?;
                        let metadata = source.metadata().await?;
                        if !metadata.is_file() || metadata.len() != entry.size {
                            bail!(CHANGED);
                        }
                        let handle = remote.create(&path, entry.mode & 0o777).await?;
                        let written = crate::files::transfer::write_file(
                            &mut source,
                            |offset, data| remote.write(&handle, offset, data),
                            |_| {},
                        )
                        .await;
                        let closed = remote.close(handle).await;
                        if written? != entry.size {
                            bail!(CHANGED);
                        }
                        closed?;
                        finish(remote, &path, entry.mtime, entry.mode & 0o777).await;
                    }
                }
            }
            for (path, mtime, mode) in folders.into_iter().rev() {
                finish(remote, &path, mtime, mode).await;
            }
            Ok(())
        }
    }
}
#[cfg(not(unix))]
mod tree {
    use super::RemoteFs;
    use anyhow::{Result, bail};
    use std::path::PathBuf;

    /// Files are not offered on this platform (see `mode`).
    pub(super) enum Tree {}
    impl Tree {
        pub(super) async fn plan(_: Vec<PathBuf>) -> Result<Self> {
            bail!("Copied files are not supported on this platform")
        }
        pub(super) fn bytes(&self) -> u64 {
            match *self {}
        }
        pub(super) fn top_level(&self) -> std::iter::Empty<&str> {
            match *self {}
        }
        pub(super) async fn write<R: RemoteFs>(&self, _: &R, _: &str) -> Result<()> {
            match *self {}
        }
    }
}

/// Why serving a read of the copied files failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    /// A newer copy, revoked permission or a dropped read ended it; nothing
    /// to tell the person.
    Superseded,
    /// The person's paste got no files; the reason is for them to read.
    Failed(String),
}

/// An upload that ended because its copy is no longer the current one.
#[derive(Debug)]
struct Superseded;
impl std::fmt::Display for Superseded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("A newer copy replaced the copied files")
    }
}
impl std::error::Error for Superseded {}

/// The reply of one copy's upload, or why it failed.
type Outcome = Result<Arc<[u8]>, Failure>;
/// The running upload of one copy: empty until it ends.
type Running = watch::Receiver<Option<Outcome>>;
/// Set once the paths of the copy are read and the upload runs.
type Delivery = Arc<OnceCell<Running>>;

fn failed(delivery: &Delivery) -> bool {
    delivery
        .get()
        .is_some_and(|running| matches!(&*running.borrow(), Some(Err(Failure::Failed(_)))))
}

/// Removes published uploads, continuing past one that cannot be removed.
async fn remove_folders<C: Connect>(connect: &C, folders: Vec<String>) -> Result<()> {
    let remote = connect.connect().await?;
    for folder in folders {
        if let Err(error) = remove_tree(&*remote, &folder).await {
            log::warn!("Clipboard files could not remove an upload: {error:#}");
        }
    }
    Ok(())
}

/// The copy uploads are currently for.
struct Slot {
    id: u64,
    revision: i64,
    ticket: Option<u64>,
    delivery: Delivery,
    cancel: watch::Sender<bool>,
    /// The published upload, once complete.
    completed: Option<String>,
}

#[derive(Default)]
struct State {
    next: u64,
    /// The newest local revision observed.
    latest: Option<i64>,
    current: Option<Slot>,
    /// Staging directories of uploads still running or being cleaned up.
    staged: Vec<String>,
    /// Published uploads of superseded copies, removed after [`GRACE`].
    retired: Vec<(String, Instant)>,
}
impl State {
    fn supersede(&mut self) {
        if let Some(slot) = self.current.take() {
            let _ = slot.cancel.send(true);
            if let Some(folder) = slot.completed {
                self.retired.push((folder, Instant::now()));
            }
        }
    }
}

/// File reads of one integration session.
pub(crate) struct Files<C: Connect> {
    connect: Arc<C>,
    mode: Mode,
    server: String,
    state: Arc<Mutex<State>>,
    uploads: Arc<Semaphore>,
    /// Held by the session-start sweep; uploads begin after it.
    swept: Arc<tokio::sync::Mutex<()>>,
}
impl<C: Connect> Clone for Files<C> {
    fn clone(&self) -> Self {
        Self {
            connect: self.connect.clone(),
            mode: self.mode,
            server: self.server.clone(),
            state: self.state.clone(),
            uploads: self.uploads.clone(),
            swept: self.swept.clone(),
        }
    }
}

impl<C: Connect> Files<C> {
    /// Starts by sweeping what earlier sessions left on the server.
    pub(crate) fn new(connect: C, mode: Mode) -> Self {
        let files = Self {
            connect: Arc::new(connect),
            mode,
            server: String::new(),
            state: Arc::default(),
            uploads: Arc::new(Semaphore::new(MAX_UPLOADS)),
            swept: Arc::default(),
        };
        let guard = files.swept.clone().try_lock_owned().ok();
        let connect = files.connect.clone();
        spawn_cleanup("sweep", async move {
            let _guard = guard;
            let remote = connect.connect().await?;
            sweep(&*remote, SystemTime::now()).await
        });
        files
    }

    pub(crate) fn mode(&self) -> Mode {
        self.mode
    }

    #[cfg(all(test, unix))]
    pub(crate) async fn serve(
        &self,
        revision: i64,
        ticket: Option<u64>,
        local_paths: impl Future<Output = Option<Vec<u8>>> + Send,
    ) -> Option<Vec<u8>> {
        self.serve_result(revision, ticket, local_paths).await.ok()
    }

    /// The reply to a read of the files copied at `revision`: their paths on
    /// the server once uploaded, or why the read got no files. `local_paths`
    /// reads the local paths format of that revision; it is not polled when an
    /// upload of the copy exists.
    pub(crate) async fn serve_result(
        &self,
        revision: i64,
        ticket: Option<u64>,
        local_paths: impl Future<Output = Option<Vec<u8>>> + Send,
    ) -> Result<Vec<u8>, Failure> {
        let (slot, delivery, cancelled) = self.live_slot(revision, ticket).await?;
        let read = async {
            let mut running = delivery
                .get_or_init(|| self.start(slot, local_paths, cancelled.clone()))
                .await
                .clone();
            match running.wait_for(Option::is_some).await {
                Ok(outcome) => outcome.clone().unwrap_or(Err(Failure::Superseded)),
                Err(_) => Err(Failure::Superseded),
            }
        };
        match tokio::time::timeout(self.mode.deadline(), read).await {
            Ok(reply) => reply.map(|reply| reply.to_vec()),
            Err(_) if *cancelled.borrow() => Err(Failure::Superseded),
            Err(_) => Err(Failure::Failed(match self.mode {
                Mode::LongRead => "Copying took too long".into(),
                Mode::Budget => {
                    "Copying took too long for this server's Newport agent. Update Newport on the server to copy more".into()
                }
            })),
        }
    }

    /// The server's name in failure notices.
    pub(crate) fn named(mut self, server: &str) -> Self {
        self.server = server.into();
        self
    }

    pub(crate) fn server(&self) -> &str {
        &self.server
    }

    /// A newer local copy cancels or retires the uploads of older ones.
    pub(crate) fn observe(&self, revision: i64) {
        let mut state = self.state.lock().unwrap();
        state.latest = state.latest.max(Some(revision));
        if state
            .current
            .as_ref()
            .is_some_and(|slot| slot.revision < revision)
        {
            state.supersede();
        }
    }

    /// Whether `slot` is still the upload for the newest copy observed.
    fn is_current(&self, slot: u64) -> bool {
        let state = self.state.lock().unwrap();
        state.current.as_ref().is_some_and(|current| {
            current.id == slot && state.latest.is_none_or(|latest| latest <= current.revision)
        })
    }

    /// Cancels the current upload and takes every published one.
    fn release(&self) -> Vec<String> {
        let mut state = self.state.lock().unwrap();
        state.supersede();
        std::mem::take(&mut state.retired)
            .into_iter()
            .map(|(folder, _)| folder)
            .collect()
    }

    /// Clipboard permission was revoked: the uploads are removed now.
    pub(crate) fn revoke(&self) {
        let folders = self.release();
        if folders.is_empty() {
            return;
        }
        let connect = self.connect.clone();
        spawn_cleanup("revoke cleanup", async move {
            remove_folders(&*connect, folders).await
        });
    }

    /// The session ends: uploads are cancelled and removed, for at most
    /// [`SHUTDOWN_TIMEOUT`]. Called while the connection is still up.
    pub(crate) async fn shutdown(&self) {
        let mut folders = self.release();
        let staged = self.state.lock().unwrap().staged.clone();
        if folders.is_empty() && staged.is_empty() {
            return;
        }
        let cleanup = async {
            // A cancelled upload lets go of its staging directory as it stops;
            // only then is nothing left writing into it.
            while !self.state.lock().unwrap().staged.is_empty() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            folders.extend(staged);
            remove_folders(&*self.connect, folders).await
        };
        match tokio::time::timeout(SHUTDOWN_TIMEOUT, cleanup).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => log::warn!("Clipboard files shutdown cleanup failed: {error:#}"),
            Err(_) => log::warn!("Clipboard files shutdown cleanup timed out"),
        }
    }

    /// Removes superseded uploads whose grace period is over.
    pub(crate) fn tick(&self) {
        let now = Instant::now();
        let due: Vec<String> = {
            let mut state = self.state.lock().unwrap();
            let (due, kept) = std::mem::take(&mut state.retired)
                .into_iter()
                .partition(|(_, at)| now.saturating_duration_since(*at) >= GRACE);
            state.retired = kept;
            due.into_iter().map(|(folder, _)| folder).collect()
        };
        if due.is_empty() {
            return;
        }
        let connect = self.connect.clone();
        spawn_cleanup("expiry", async move {
            let remote = connect.connect().await?;
            for folder in due {
                remove_tree(&*remote, &folder).await?;
            }
            Ok(())
        });
    }

    fn slot(
        &self,
        revision: i64,
        ticket: Option<u64>,
    ) -> Option<(u64, Delivery, watch::Receiver<bool>)> {
        let mut state = self.state.lock().unwrap();
        // A late read of an older copy never uploads or cancels a newer one.
        if state.latest.is_some_and(|latest| revision < latest) {
            return None;
        }
        if let Some(slot) = &state.current
            && slot.ticket == ticket
        {
            if slot.revision > revision {
                return None;
            }
            // A failed upload is retried by the next read, not by the reads
            // already waiting for it.
            if slot.revision == revision && !failed(&slot.delivery) {
                return Some((slot.id, slot.delivery.clone(), slot.cancel.subscribe()));
            }
        }
        state.supersede();
        state.next += 1;
        let (cancel, cancelled) = watch::channel(false);
        let slot = Slot {
            id: state.next,
            revision,
            ticket,
            delivery: Delivery::default(),
            cancel,
            completed: None,
        };
        let found = (slot.id, slot.delivery.clone(), cancelled);
        state.current = Some(slot);
        Some(found)
    }

    /// The completed upload of `slot`, if any.
    fn completed(&self, slot: u64) -> Option<String> {
        let state = self.state.lock().unwrap();
        state
            .current
            .as_ref()
            .filter(|current| current.id == slot)
            .and_then(|current| current.completed.clone())
    }

    /// Ends `slot` without retiring its upload, which is gone.
    fn forget(&self, slot: u64) {
        let mut state = self.state.lock().unwrap();
        if state
            .current
            .as_ref()
            .is_some_and(|current| current.id == slot)
            && let Some(slot) = state.current.take()
        {
            let _ = slot.cancel.send(true);
        }
    }

    /// Whether the published upload is still there, refreshing its time so
    /// other desktops' sweeps keep it. An upload that cannot be checked is
    /// assumed to be there.
    async fn keep(&self, folder: &str) -> bool {
        let check = async {
            let remote = self.connect.connect().await?;
            if remote.kind(folder).await?.is_none() {
                return Ok(false);
            }
            if let Err(error) = remote.touch(folder).await {
                log::debug!("Clipboard files could not refresh an upload: {error:#}");
            }
            anyhow::Ok(true)
        };
        match tokio::time::timeout(REUSE_TIMEOUT, check).await {
            Ok(Ok(kept)) => kept,
            Ok(Err(error)) => {
                log::debug!("Clipboard files could not check an upload: {error:#}");
                true
            }
            Err(_) => true,
        }
    }

    /// Like [`Self::slot`], uploading the copy again when its published
    /// upload was removed from the server.
    async fn live_slot(
        &self,
        revision: i64,
        ticket: Option<u64>,
    ) -> Result<(u64, Delivery, watch::Receiver<bool>), Failure> {
        loop {
            let found = self.slot(revision, ticket).ok_or(Failure::Superseded)?;
            let Some(folder) = self.completed(found.0) else {
                return Ok(found);
            };
            if self.keep(&folder).await {
                return Ok(found);
            }
            self.forget(found.0);
        }
    }

    /// Reads the local paths of the copy, then lets its upload run in the
    /// background, so it survives a read that stops waiting.
    async fn start(
        &self,
        slot: u64,
        local_paths: impl Future<Output = Option<Vec<u8>>> + Send,
        mut cancelled: watch::Receiver<bool>,
    ) -> Running {
        let (send, running) = watch::channel(None);
        let bytes = tokio::select! {
            biased;
            _ = cancelled.wait_for(|cancelled| *cancelled) => {
                send.send_replace(Some(Err(Failure::Superseded)));
                return running;
            }
            bytes = local_paths => bytes,
        };
        let files = self.clone();
        tokio::spawn(async move {
            let outcome = files.deliver(slot, bytes, cancelled).await;
            send.send_replace(Some(outcome));
        });
        running
    }

    async fn deliver(
        &self,
        slot: u64,
        local_paths: Option<Vec<u8>>,
        mut cancelled: watch::Receiver<bool>,
    ) -> Result<Arc<[u8]>, Failure> {
        let upload = async {
            let _permit = self
                .uploads
                .acquire()
                .await
                .map_err(|_| Failure::Superseded)?;
            self.upload(slot, local_paths).await.map_err(|error| {
                if error.is::<Superseded>() {
                    return Failure::Superseded;
                }
                log::warn!("Clipboard files transfer failed: {error:#}");
                Failure::Failed(format!("{error:#}"))
            })
        };
        tokio::select! {
            biased;
            _ = cancelled.wait_for(|cancelled| *cancelled) => Err(Failure::Superseded),
            reply = upload => reply,
        }
    }

    async fn upload(&self, slot: u64, local_paths: Option<Vec<u8>>) -> Result<Arc<[u8]>> {
        let Some(bytes) = local_paths else {
            // A read that got nothing is the end of this copy only when a newer
            // one replaced it; otherwise its paths could not be read, which the
            // next paste retries.
            if self.is_current(slot) {
                bail!("The copied files could not be read. Copy them again");
            }
            return Err(Superseded.into());
        };
        let tree = tree::Tree::plan(parse_paths(&bytes)?).await?;
        if !self.mode.admits(tree.bytes()) {
            bail!("The copied files are too large for this server's Newport agent");
        }
        let remote = self.connect.connect().await?;
        drop(self.swept.lock().await);
        let root = ensure_root(&*remote).await?;
        let id = uuid::Uuid::new_v4();
        let folder = child(&root, &id.to_string())?;
        let reply = reply(&folder, tree.top_level())?;
        let partial = child(&root, &format!("{PARTIAL}{id}"))?;
        let mut staging = Partial::new(remote.clone(), partial.clone(), self.state.clone());
        let written = {
            let write = async {
                remote.mkdir(&partial, 0o700).await?;
                tree.write(&*remote, &partial).await?;
                remote.rename(&partial, &folder).await
            };
            tokio::pin!(write);
            // Creating files deep in the tree or writing a large file does not
            // change the staging directory's mtime, so it is set explicitly with
            // SFTP setstat; other desktops' sweeps then leave a live upload alone.
            let mut refresh = tokio::time::interval_at(Instant::now() + REFRESH, REFRESH);
            loop {
                tokio::select! {
                    written = &mut write => break written,
                    _ = refresh.tick() => {
                        if let Err(error) = remote.touch(&partial).await {
                            log::debug!("Clipboard files could not refresh its staging folder: {error:#}");
                        }
                    }
                }
            }
        };
        if let Err(error) = written {
            staging.discard().await;
            return Err(error);
        }
        staging.publish();
        let mut state = self.state.lock().unwrap();
        match state.current.as_mut().filter(|current| current.id == slot) {
            Some(current) => current.completed = Some(folder),
            None => {
                state.retired.push((folder, Instant::now()));
                return Err(Superseded.into());
            }
        }
        Ok(reply)
    }

    #[cfg(all(test, unix))]
    pub(crate) async fn swept(&self) {
        drop(self.swept.lock().await);
    }
}

#[cfg(all(test, unix))]
#[path = "clipboard_files_tests.rs"]
pub(crate) mod tests;
