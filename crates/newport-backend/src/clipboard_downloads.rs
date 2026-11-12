//! Downloads the files copied on the server: the agent publishes
//! `application/x-newport-files` with their server paths, and the tree is
//! fetched with SFTP into the host's private clipboard cache when the copy is
//! made, so a local paste finds files. The newest copy wins.
use crate::clipboard_files::{Kind, MAX_ENTRIES, RemoteFs, TOO_LARGE};
use crate::files::child;
use crate::files::transfer::LimitExceeded;
use anyhow::{Context, Result, anyhow, bail};
use newport_platform::clipboard::files::{local_name_key, valid_local_name};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::io::AsyncWriteExt;
#[cfg(unix)]
use tokio::sync::OnceCell;
#[cfg(unix)]
use tokio::time::Instant;

/// Copies up to this size are downloaded without asking.
const AUTO_BYTES: u64 = 512 << 20;
/// Nothing larger is downloaded.
const MAX_BYTES: u64 = 2 << 30;
const MAX_PATHS: usize = 10_000;
/// The agent's reply limit for the path list.
const MAX_LIST: usize = 1 << 20;
/// A superseded download stays available to a paste already copying it.
#[cfg(unix)]
const GRACE: Duration = Duration::from_secs(10 * 60);
/// Downloads older than this are removed on first use.
#[cfg(unix)]
const STALE: Duration = Duration::from_secs(60 * 60);
#[cfg(unix)]
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
#[cfg(unix)]
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
    #[cfg(unix)]
    Symlink {
        target: String,
    },
}

/// One thing to create on the host.
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
    #[cfg(all(test, unix))]
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
            #[cfg(windows)]
            {
                let name = entry
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default();
                let (executable, installer) = windows::risk(name);
                self.executable |= executable;
                self.installer |= installer;
            }
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

/// Names already planned in one local folder, with host collision rules.
#[derive(Default)]
struct Folder(HashMap<String, PathBuf>);
impl Folder {
    /// Refuses `path` when its name matches one already planned in its
    /// folder except for case or Unicode normalization.
    fn add(&mut self, path: &Path, name: &str) -> Result<()> {
        if let Some(other) = self.0.insert(local_name_key(name), path.to_owned()) {
            #[cfg(unix)]
            let host = "Mac";
            #[cfg(windows)]
            let host = "computer";
            bail!(
                "“{}” and “{}” would have the same name on this {host}",
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
        if !valid_local_name(name) {
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
            if !valid_local_name(&item.name) {
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
                #[cfg(unix)]
                Kind::Link => EntryKind::Symlink {
                    target: link_target(fs, root, &source, depth, &path).await?,
                },
                #[cfg(windows)]
                Kind::Link => bail!(
                    "“{}” is a symbolic link that cannot be copied to Windows",
                    path.display()
                ),
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
#[cfg(unix)]
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

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub(crate) use unix::{Cache, Ticket};
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::{Cache, Ticket};
