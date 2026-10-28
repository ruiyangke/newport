//! Downloads the files copied on the server: the agent publishes
//! `application/x-newport-files` with their server paths, and the tree is
//! fetched with SFTP into `~/Library/Caches/Newport/Clipboard/<uuid>/` when the
//! copy is made, so a Mac paste finds local files. The newest copy wins.
use crate::clipboard_files::{Kind, MAX_ENTRIES, RemoteFs, TOO_LARGE};
use crate::files::child;
use crate::files::transfer::LimitExceeded;
use anyhow::{Context, Result, anyhow, bail};
use newport_platform::clipboard::files::{mac_name_key, valid_name};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::io::AsyncWriteExt;
use tokio::sync::OnceCell;
use tokio::time::Instant;

/// Copies up to this size are downloaded without asking.
const AUTO_BYTES: u64 = 512 << 20;
/// Nothing larger is downloaded.
const MAX_BYTES: u64 = 2 << 30;
const MAX_PATHS: usize = 10_000;
/// The agent's reply limit for the path list.
const MAX_LIST: usize = 1 << 20;
/// A superseded download stays available to a paste already copying it.
const GRACE: Duration = Duration::from_secs(10 * 60);
/// Downloads older than this are removed on first use.
const STALE: Duration = Duration::from_secs(60 * 60);
const PARTIAL: &str = ".partial-";
const SUPERSEDED: &str = "A newer copy replaced the copied files";
const CHANGED: &str = "A copied file changed on the server during the transfer";
/// Top-level names with these endings are apps or installers.
const BUNDLES: [&str; 3] = [".app", ".pkg", ".dmg"];
/// Top-level names with these endings are scripts that run when opened.
const SCRIPTS: [&str; 2] = [".command", ".sh"];
/// The longest top-level name shown when asking, in characters.
const SHOWN_NAME: usize = 40;
/// How many top-level names are shown when asking.
const SHOWN_NAMES: usize = 3;
const OUTSIDE: &str = "links outside the copied folder and cannot be copied";

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Paths {
    version: u32,
    paths: Vec<String>,
}

/// The server paths of a read of `application/x-newport-files`.
pub(crate) fn parse_paths(bytes: &[u8]) -> Result<Vec<String>> {
    if bytes.len() > MAX_LIST {
        bail!("The copied path list is too long");
    }
    let parsed: Paths = serde_json::from_slice(bytes).context("Invalid copied path list")?;
    if parsed.version != 1 {
        bail!("Unsupported copied path list version");
    }
    if parsed.paths.is_empty() || parsed.paths.len() > MAX_PATHS {
        bail!("The copied path list must hold 1 to 10,000 items");
    }
    if parsed
        .paths
        .iter()
        .any(|path| !path.starts_with('/') || path.contains('\0'))
    {
        bail!("The copied path list holds an invalid path");
    }
    Ok(parsed.paths)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EntryKind {
    Directory,
    File {
        size: u64,
        mtime: Option<u32>,
    },
    /// A link whose relative target stays inside the copied tree, as stored.
    Symlink {
        target: String,
    },
}

/// One thing to create on the Mac.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    /// Relative to the download folder, starting with the top-level name.
    pub(crate) path: PathBuf,
    /// Where to read it on the server.
    pub(crate) source: String,
    pub(crate) kind: EntryKind,
    /// Unix permission bits on the server.
    pub(crate) mode: u32,
    /// Modification time on the server, in seconds since the Unix epoch.
    pub(crate) mtime: Option<u32>,
}

/// Everything one copy creates, parents first. Planning stops once the copy
/// is beyond the limits, which [`decide`] then refuses.
#[derive(Debug)]
pub(crate) struct Plan {
    entries: Vec<Entry>,
    bytes: u64,
    count: u64,
    /// Every top-level item resolves under the server user's home.
    inside_home: bool,
    /// Something is hidden (a `.`-prefixed folder or file under home, such as
    /// `.ssh`).
    hidden: bool,
    /// Something is executable or a script.
    executable: bool,
    /// Something is an application or installer.
    installer: bool,
}
/// Whether `path` has a `.`-prefixed component.
fn hidden(path: &Path) -> bool {
    path.components().any(|part| {
        matches!(part, std::path::Component::Normal(name)
            if name.as_encoded_bytes().first() == Some(&b'.'))
    })
}
impl Plan {
    #[cfg(test)]
    pub(crate) fn entries(&self) -> &[Entry] {
        &self.entries
    }
    pub(crate) fn bytes(&self) -> u64 {
        self.bytes
    }
    pub(crate) fn count(&self) -> u64 {
        self.count
    }
    pub(crate) fn inside_home(&self) -> bool {
        self.inside_home
    }
    pub(crate) fn hidden(&self) -> bool {
        self.hidden
    }
    pub(crate) fn executable(&self) -> bool {
        self.executable
    }
    pub(crate) fn installer(&self) -> bool {
        self.installer
    }
    /// Whether the copy holds anything that asks before it downloads.
    fn sensitive(&self) -> bool {
        self.hidden || self.executable || self.installer
    }
    /// Whether the copy is large enough to ask before it downloads.
    pub(crate) fn large(&self) -> bool {
        self.bytes > AUTO_BYTES
    }
    /// How many items were copied at the top level.
    pub(crate) fn top_level_count(&self) -> u64 {
        self.top_level().count() as u64
    }
    /// The first few top-level names, long ones cut short, for the question.
    pub(crate) fn shown_names(&self) -> Vec<String> {
        self.top_level()
            .take(SHOWN_NAMES)
            .map(|name| match name.char_indices().nth(SHOWN_NAME) {
                Some((end, _)) => format!("{}…", &name[..end]),
                None => name.to_owned(),
            })
            .collect()
    }
    /// The names of the copied items, in copy order.
    pub(crate) fn top_level(&self) -> impl Iterator<Item = &str> {
        self.entries
            .iter()
            .filter(|entry| entry.path.components().count() == 1)
            .filter_map(|entry| entry.path.to_str())
    }
    /// Counts `entry`; false once the copy is beyond the limits.
    fn push(&mut self, entry: Entry) -> bool {
        self.count += 1;
        if let EntryKind::File { size, .. } = entry.kind {
            self.bytes = self.bytes.saturating_add(size);
            self.executable |= entry.mode & 0o111 != 0;
        }
        self.hidden |= hidden(&entry.path);
        self.entries.push(entry);
        self.count <= MAX_ENTRIES && self.bytes <= MAX_BYTES
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    Auto,
    Ask,
    Refuse,
}

/// Whether a planned copy downloads at once, asks first or is refused. A copy
/// asks when it leaves home, is large, or holds hidden, executable or
/// installable items.
pub(crate) fn decide(plan: &Plan) -> Decision {
    if plan.bytes > MAX_BYTES || plan.count > MAX_ENTRIES {
        Decision::Refuse
    } else if plan.bytes <= AUTO_BYTES && plan.inside_home && !plan.sensitive() {
        Decision::Auto
    } else {
        Decision::Ask
    }
}

fn unsupported(path: &Path) -> anyhow::Error {
    anyhow!(
        "“{}” is a device, socket or pipe and cannot be copied",
        path.display()
    )
}
fn invalid_name(path: &Path) -> anyhow::Error {
    anyhow!("“{}” has a name that cannot be copied", path.display())
}

/// The names already planned in one Mac folder, by the key under which the
/// Mac file system compares them.
#[derive(Default)]
struct Folder(HashMap<String, PathBuf>);
impl Folder {
    /// Refuses `path` when its name matches one already planned in its
    /// folder except for case or Unicode normalization.
    fn add(&mut self, path: &Path, name: &str) -> Result<()> {
        if let Some(other) = self.0.insert(mac_name_key(name), path.to_owned()) {
            bail!(
                "“{}” and “{}” would have the same name on this Mac",
                other.display(),
                path.display()
            );
        }
        Ok(())
    }
}

/// Plans the copy of `paths` on the server, following links only at the top
/// level. Anything that cannot be copied exactly is an error naming it.
pub(crate) async fn plan<R: RemoteFs>(fs: &R, paths: &[String]) -> Result<Plan> {
    if paths.is_empty() || paths.len() > MAX_PATHS {
        bail!("The copy must hold 1 to 10,000 items");
    }
    let home = fs.realpath(".").await?;
    if !home.starts_with('/') {
        bail!("Server returned a relative home folder");
    }
    let home = format!("{}/", home.trim_end_matches('/'));
    let mut plan = Plan {
        entries: Vec::new(),
        bytes: 0,
        count: 0,
        inside_home: true,
        hidden: false,
        executable: false,
        installer: false,
    };
    let mut names = HashSet::new();
    let mut folder = Folder::default();
    for path in paths {
        let name = path.rsplit('/').next().unwrap_or_default();
        if !valid_name(name) {
            return Err(invalid_name(Path::new(path)));
        }
        if !names.insert(name) {
            bail!("Two copied items are named “{name}”");
        }
        folder.add(Path::new(name), name)?;
        let root = fs
            .realpath(path)
            .await
            .with_context(|| format!("“{name}” is not available on the server"))?;
        plan.inside_home &= root.starts_with(&home);
        plan.hidden |= root
            .strip_prefix(&home)
            .is_some_and(|rest| hidden(Path::new(rest)));
        let lower = name.to_lowercase();
        plan.installer |= BUNDLES.iter().any(|ending| lower.ends_with(ending));
        plan.executable |= SCRIPTS.iter().any(|ending| lower.ends_with(ending));
        let stat = fs
            .lstat(&root)
            .await?
            .with_context(|| format!("“{name}” is not available on the server"))?;
        let entry = |kind| Entry {
            path: PathBuf::from(name),
            source: root.clone(),
            kind,
            mode: stat.mode,
            mtime: stat.mtime,
        };
        let within = match stat.kind {
            Kind::File => plan.push(entry(EntryKind::File {
                size: stat.size,
                mtime: stat.mtime,
            })),
            Kind::Directory => {
                plan.push(entry(EntryKind::Directory))
                    && directory(fs, &mut plan, &root, name).await?
            }
            Kind::Link | Kind::Other => return Err(unsupported(Path::new(name))),
        };
        if !within {
            break;
        }
    }
    Ok(plan)
}

/// Adds everything under the copied folder `root`; false once beyond the limits.
async fn directory<R: RemoteFs>(fs: &R, plan: &mut Plan, root: &str, name: &str) -> Result<bool> {
    // Folders still to list: server path, local relative path, depth below root.
    let mut pending = vec![(root.to_owned(), PathBuf::from(name), 0)];
    while let Some((dir, relative, depth)) = pending.pop() {
        let mut items = fs
            .list(&dir)
            .await?
            .with_context(|| format!("“{}” is not available on the server", relative.display()))?;
        items.sort_by(|a, b| a.name.cmp(&b.name));
        let mut folders = Vec::new();
        let mut names = Folder::default();
        for item in items {
            let path = relative.join(&item.name);
            if !valid_name(&item.name) {
                return Err(invalid_name(&path));
            }
            names.add(&path, &item.name)?;
            let source = child(&dir, &item.name)?;
            let kind = match item.kind {
                Kind::File => EntryKind::File {
                    size: item.size,
                    mtime: item.mtime,
                },
                Kind::Directory => {
                    folders.push((source.clone(), path.clone(), depth + 1));
                    EntryKind::Directory
                }
                Kind::Link => EntryKind::Symlink {
                    target: link_target(fs, root, &source, depth, &path).await?,
                },
                Kind::Other => return Err(unsupported(&path)),
            };
            let entry = Entry {
                path,
                source,
                kind,
                mode: item.mode,
                mtime: item.mtime,
            };
            if !plan.push(entry) {
                return Ok(false);
            }
        }
        pending.extend(folders.into_iter().rev());
    }
    Ok(true)
}

/// The stored target of `link`, which sits `depth` folders below the copied
/// folder `root`, when it is relative and stays inside the copied tree.
async fn link_target<R: RemoteFs>(
    fs: &R,
    root: &str,
    link: &str,
    depth: usize,
    path: &Path,
) -> Result<String> {
    let escaping = || anyhow!("“{}” {OUTSIDE}", path.display());
    let target = fs.readlink(link).await?;
    if target.is_empty() || target.starts_with('/') || target.contains('\0') {
        return Err(escaping());
    }
    let mut level = depth;
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => level = level.checked_sub(1).ok_or_else(escaping)?,
            _ => level += 1,
        }
    }
    // The server resolves links met on the way, which the lexical walk above
    // cannot; a link to nowhere fails here too.
    let resolved = fs.realpath(link).await.map_err(|_| escaping())?;
    let root = root.trim_end_matches('/');
    if resolved != root && !resolved.starts_with(&format!("{root}/")) {
        return Err(escaping());
    }
    Ok(target)
}

/// Gives the owner access to every folder under (and including) `path`, which
/// copied folders may deny. Links are not followed.
fn open_up(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if !meta.is_dir() {
        return;
    }
    let mode = meta.permissions().mode();
    if mode & 0o700 != 0o700 {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | 0o700));
    }
    if let Ok(children) = std::fs::read_dir(path) {
        children.flatten().for_each(|child| open_up(&child.path()));
    }
}

fn remove(path: &Path) {
    let removed = match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path).or_else(|_| {
            open_up(path);
            std::fs::remove_dir_all(path)
        }),
        Ok(_) => std::fs::remove_file(path),
        Err(error) => Err(error),
    };
    if let Err(error) = removed
        && error.kind() != std::io::ErrorKind::NotFound
    {
        log::warn!("Clipboard downloads could not remove a folder: {error}");
    }
}
/// Removes `paths` on a blocking thread, or at once outside a runtime.
fn remove_later(paths: Vec<PathBuf>) {
    let work = move || paths.iter().for_each(|path| remove(path));
    match tokio::runtime::Handle::try_current() {
        Ok(runtime) => drop(runtime.spawn_blocking(work)),
        Err(_) => work(),
    }
}

/// A download's staging folder, removed unless it was renamed into place.
struct Partial(Option<PathBuf>);
impl Partial {
    async fn discard(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = tokio::task::spawn_blocking(move || remove(&path)).await;
        }
    }
}
impl Drop for Partial {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            remove_later(vec![path]);
        }
    }
}

/// Downloads `plan` into `cache_root/<uuid>/` and returns that folder. Stops
/// when `is_current` turns false, checked before every entry and read.
pub(crate) async fn fetch<R: RemoteFs>(
    fs: &R,
    plan: &Plan,
    cache_root: &Path,
    is_current: impl Fn() -> bool,
) -> Result<PathBuf> {
    if decide(plan) == Decision::Refuse {
        bail!(TOO_LARGE);
    }
    tokio::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(cache_root)
        .await?;
    let id = uuid::Uuid::new_v4();
    let partial = cache_root.join(format!("{PARTIAL}{id}"));
    let folder = cache_root.join(id.to_string());
    let mut staging = Partial(Some(partial.clone()));
    let written = async {
        tokio::fs::DirBuilder::new()
            .mode(0o700)
            .create(&partial)
            .await?;
        let mut budget = MAX_BYTES;
        for entry in &plan.entries {
            if !is_current() {
                bail!(SUPERSEDED);
            }
            let copied = write(fs, entry, &partial.join(&entry.path), budget, &is_current).await?;
            budget -= copied;
        }
        if !is_current() {
            bail!(SUPERSEDED);
        }
        // The server's word about where links lead is not trusted: they are
        // resolved here, on the finished tree, before anything is published.
        let (staged, entries) = (partial.clone(), plan.entries.clone());
        tokio::task::spawn_blocking(move || finish(&staged, &entries)).await??;
        tokio::fs::rename(&partial, &folder).await?;
        Ok(())
    }
    .await;
    match written {
        Ok(()) => {
            staging.0 = None;
            Ok(folder)
        }
        Err(error) => {
            staging.discard().await;
            Err(error)
        }
    }
}

/// Requires every link of `entries` in `staged` to resolve, locally, inside
/// `staged` (a link to nowhere is refused), then marks everything downloaded
/// from the internet.
fn finish(staged: &Path, entries: &[Entry]) -> Result<()> {
    let root = staged.canonicalize()?;
    for entry in entries {
        if matches!(entry.kind, EntryKind::Symlink { .. }) {
            let inside = staged
                .join(&entry.path)
                .canonicalize()
                .is_ok_and(|resolved| resolved.starts_with(&root));
            if !inside {
                bail!("“{}” {OUTSIDE}", entry.path.display());
            }
        }
    }
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |age| age.as_secs());
    let value = format!("0081;{stamp:x};Newport;");
    for entry in entries {
        quarantine(&staged.join(&entry.path), &value);
    }
    keep_attributes(staged, entries);
    Ok(())
}

/// Gives files and folders the server's modification time and permissions
/// (as `cp -p` does; links keep neither). Folders go last, children first:
/// creating entries changed their times, and a copied folder may be read-only.
/// Best effort. The staging folder itself keeps its private mode.
fn keep_attributes(staged: &Path, entries: &[Entry]) {
    use std::os::unix::fs::PermissionsExt;
    let apply = |entry: &Entry| {
        let path = staged.join(&entry.path);
        if let Some(mtime) = entry.mtime {
            let time = SystemTime::UNIX_EPOCH + Duration::from_secs(mtime.into());
            let times = std::fs::FileTimes::new()
                .set_accessed(time)
                .set_modified(time);
            if let Err(error) = std::fs::File::open(&path).and_then(|file| file.set_times(times)) {
                log::debug!("Clipboard downloads could not set a modification time: {error}");
            }
        }
        let mode = std::fs::Permissions::from_mode(entry.mode & 0o777);
        if let Err(error) = std::fs::set_permissions(&path, mode) {
            log::debug!("Clipboard downloads could not set permissions: {error}");
        }
    };
    for entry in entries {
        if matches!(entry.kind, EntryKind::File { .. }) {
            apply(entry);
        }
    }
    for entry in entries.iter().rev() {
        if matches!(entry.kind, EntryKind::Directory) {
            apply(entry);
        }
    }
}

/// Sets `com.apple.quarantine` on `path` itself (not what a link leads to),
/// so Gatekeeper treats it as a download. Best effort.
#[cfg(target_os = "macos")]
fn quarantine(path: &Path, value: &str) {
    // The backend does not depend on `libc` outside its tests.
    unsafe extern "C" {
        fn setxattr(
            path: *const std::ffi::c_char,
            name: *const std::ffi::c_char,
            value: *const std::ffi::c_void,
            size: usize,
            position: u32,
            options: std::ffi::c_int,
        ) -> std::ffi::c_int;
    }
    const XATTR_NOFOLLOW: std::ffi::c_int = 0x0001;
    const EACCES: i32 = 13;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;
    let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return;
    };
    let set = || unsafe {
        setxattr(
            c_path.as_ptr(),
            c"com.apple.quarantine".as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
            XATTR_NOFOLLOW,
        )
    };
    if set() == 0 {
        return;
    }
    let mut error = std::io::Error::last_os_error();
    // A read-only file needs write permission for its attributes.
    if error.raw_os_error() == Some(EACCES)
        && let Ok(meta) = std::fs::symlink_metadata(path)
        && !meta.is_symlink()
    {
        let original = meta.permissions();
        let writable = std::fs::Permissions::from_mode(original.mode() | 0o200);
        if std::fs::set_permissions(path, writable).is_ok() {
            let retried = set();
            error = std::io::Error::last_os_error();
            let _ = std::fs::set_permissions(path, original);
            if retried == 0 {
                return;
            }
        }
    }
    log::warn!("Clipboard downloads could not mark a file as downloaded: {error}");
}
#[cfg(not(target_os = "macos"))]
fn quarantine(_: &Path, _: &str) {}

/// Writes `entry` to `local` and returns the bytes of file data copied, which
/// may not exceed `budget` or the entry's planned size.
async fn write<R: RemoteFs>(
    fs: &R,
    entry: &Entry,
    local: &Path,
    budget: u64,
    is_current: &impl Fn() -> bool,
) -> Result<u64> {
    match &entry.kind {
        EntryKind::Directory => {
            tokio::fs::DirBuilder::new()
                .mode(0o700)
                .create(local)
                .await?;
            Ok(0)
        }
        EntryKind::Symlink { target } => {
            tokio::fs::symlink(target, local).await?;
            Ok(0)
        }
        EntryKind::File { size, mtime } => {
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(entry.mode & 0o777 | 0o600)
                .open(local)
                .await?;
            let handle = fs.open_read(&entry.source).await?;
            let open = &handle;
            let copied = crate::files::transfer::read_file_capped(
                &mut file,
                |offset, len| async move {
                    if !is_current() {
                        bail!(SUPERSEDED);
                    }
                    fs.read(open, offset, len).await
                },
                Some(*size),
                Some((*size).min(budget)),
                |_| {},
            )
            .await;
            let closed = fs.close(handle).await;
            let copied = match copied {
                Err(error) if error.is::<LimitExceeded>() => {
                    // Beyond what was planned: the file grew, or the copy as a
                    // whole outgrew the limit.
                    bail!(if budget < *size { TOO_LARGE } else { CHANGED })
                }
                other => other?,
            };
            closed?;
            file.flush().await?;
            let after = fs.lstat(&entry.source).await?;
            if copied != *size
                || after.is_none_or(|after| {
                    after.kind != Kind::File || after.size != *size || after.mtime != *mtime
                })
            {
                bail!(CHANGED);
            }
            Ok(copied)
        }
    }
}

/// Whether `name` is a download or staging folder this module creates; Some
/// with true for a staging folder.
fn owned(name: &str) -> Option<bool> {
    let (partial, id) = match name.strip_prefix(PARTIAL) {
        Some(id) => (true, id),
        None => (false, name),
    };
    uuid::Uuid::try_parse(id).ok().map(|_| partial)
}

/// Removes, in `root`, staging folders and downloads older than [`STALE`], or
/// every download and staging folder when `all`.
fn sweep(root: &Path, now: SystemTime, all: bool) {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            log::warn!("Clipboard downloads could not read its cache: {error}");
            return;
        }
    };
    for entry in entries.flatten() {
        let Some(partial) = entry.file_name().to_str().and_then(owned) else {
            continue;
        };
        let stale = || {
            entry
                .metadata()
                .and_then(|meta| meta.modified())
                .is_ok_and(|modified| now.duration_since(modified).is_ok_and(|age| age >= STALE))
        };
        if all || partial || stale() {
            remove(&entry.path());
        }
    }
}

/// `~/Library/Caches/Newport/Clipboard` on macOS: the clipboard file cache the
/// helper attaches folders from.
fn base_root() -> Option<PathBuf> {
    dirs::cache_dir().map(|dir| dir.join("Newport").join("Clipboard"))
}

/// A short, stable name for the instance using `profile`.
fn instance(profile: &Path) -> String {
    use sha2::Digest;
    let profile = profile
        .canonicalize()
        .or_else(|_| std::path::absolute(profile))
        .unwrap_or_else(|_| profile.to_owned());
    sha2::Sha256::digest(profile.as_os_str().as_encoded_bytes())[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A download folder as the helper takes it: relative to the clipboard file
/// cache, `<instance>/<uuid>`.
#[cfg(target_os = "macos")]
pub(crate) fn attachable(folder: &Path) -> Option<String> {
    folder
        .strip_prefix(base_root()?)
        .ok()?
        .to_str()
        .map(str::to_owned)
}

/// Lets a download check that its copy is still the newest and still
/// available.
#[derive(Clone)]
pub(crate) struct Ticket {
    id: u64,
    cancel: Arc<AtomicBool>,
    fence: Arc<dyn Fn() -> bool + Send + Sync>,
}
impl Ticket {
    pub(crate) fn is_current(&self) -> bool {
        !self.cancel.load(Ordering::Acquire) && (self.fence)()
    }
}

struct Job {
    id: u64,
    cancel: Arc<AtomicBool>,
    /// The finished download.
    folder: Option<PathBuf>,
}

#[derive(Default)]
struct State {
    next: u64,
    current: Option<Job>,
    /// Downloads of superseded copies, removed after [`GRACE`].
    retired: Vec<(PathBuf, Instant)>,
}
impl State {
    fn supersede(&mut self) {
        if let Some(job) = self.current.take() {
            job.cancel.store(true, Ordering::Release);
            if let Some(folder) = job.folder {
                self.retired.push((folder, Instant::now()));
            }
        }
    }
}

/// The local download cache: one current copy, superseded ones kept for
/// [`GRACE`]. Nothing touches the disk until the first download.
pub(crate) struct Cache {
    root: PathBuf,
    state: Mutex<State>,
    swept: OnceCell<()>,
}
impl Cache {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root,
            state: Mutex::default(),
            swept: OnceCell::new(),
        }
    }
    /// `~/Library/Caches/Newport/Clipboard/<instance>` on macOS, where the
    /// instance is named after the profile directory: each Newport instance
    /// sweeps and clears only its own downloads.
    pub(crate) fn default_root(profile: &Path) -> Option<PathBuf> {
        Some(base_root()?.join(instance(profile)))
    }
    #[cfg(test)]
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Starts the download of a new copy: the previous one is cancelled and
    /// its finished folder retired. The ticket stays current while `fence`
    /// holds and no newer copy began.
    pub(crate) fn begin(&self, fence: impl Fn() -> bool + Send + Sync + 'static) -> Ticket {
        let mut state = self.state.lock().unwrap();
        state.supersede();
        state.next += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        state.current = Some(Job {
            id: state.next,
            cancel: cancel.clone(),
            folder: None,
        });
        Ticket {
            id: state.next,
            cancel,
            fence: Arc::new(fence),
        }
    }

    /// `ticket`'s copy left the clipboard: its download is cancelled and its
    /// folder retired like a superseded one. A newer copy's is kept.
    pub(crate) fn retire(&self, ticket: &Ticket) {
        ticket.cancel.store(true, Ordering::Release);
        let mut state = self.state.lock().unwrap();
        if state
            .current
            .as_ref()
            .is_some_and(|job| job.id == ticket.id)
        {
            state.supersede();
        }
    }

    /// Removes staging folders and hour-old downloads that earlier runs left;
    /// only the first call does anything.
    pub(crate) async fn sweep(&self) {
        let root = self.root.clone();
        self.swept
            .get_or_init(|| async move {
                let now = SystemTime::now();
                let _ = tokio::task::spawn_blocking(move || sweep(&root, now, false)).await;
            })
            .await;
    }

    /// Downloads `plan` for `ticket`'s copy and returns its folder.
    pub(crate) async fn fetch<R: RemoteFs>(
        &self,
        fs: &R,
        plan: &Plan,
        ticket: &Ticket,
    ) -> Result<PathBuf> {
        self.sweep().await;
        let folder = fetch(fs, plan, &self.root, || ticket.is_current()).await?;
        let mut state = self.state.lock().unwrap();
        match state.current.as_mut().filter(|job| job.id == ticket.id) {
            Some(job) => job.folder = Some(folder.clone()),
            None => {
                state.retired.push((folder, Instant::now()));
                bail!(SUPERSEDED);
            }
        }
        Ok(folder)
    }

    /// Removes superseded downloads whose grace period is over.
    pub(crate) fn tick(&self) {
        let now = Instant::now();
        let due: Vec<PathBuf> = {
            let mut state = self.state.lock().unwrap();
            let (due, kept) = std::mem::take(&mut state.retired)
                .into_iter()
                .partition(|(_, at)| now.saturating_duration_since(*at) >= GRACE);
            state.retired = kept;
            due.into_iter().map(|(folder, _)| folder).collect()
        };
        if !due.is_empty() {
            remove_later(due);
        }
    }

    /// Cancels the current download and removes every download, blocking on
    /// the removal; for quit.
    pub(crate) fn clear(&self) {
        {
            let mut state = self.state.lock().unwrap();
            state.supersede();
            state.retired.clear();
        }
        sweep(&self.root, SystemTime::now(), true);
    }
}

#[cfg(test)]
#[path = "clipboard_downloads_tests.rs"]
mod tests;
