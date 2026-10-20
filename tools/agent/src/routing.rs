//! Explicit integration routing. A shell never guesses between desktop sessions.
mod history;
use serde::{Deserialize, Serialize};
use std::{
    env, fs, io,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

pub(crate) const SELECTOR: &str = "NEWPORT_SESSION";
#[derive(Clone)]
pub(crate) struct Scope {
    pub session: String,
    pub runtime: String,
    pub directory: PathBuf,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    session: String,
    runtime: String,
    client: String,
    root: PathBuf,
}
pub(crate) struct Selection {
    pub root: PathBuf,
    pub session: Option<String>,
}
// Holding the published file's lock protects it before features become active
// and while permissions are disabled. Kernel release also detects SIGKILL.
pub(crate) struct Publication {
    path: PathBuf,
    identity: (u64, u64),
    _lock: fs::File,
    record: Record,
    next_check: std::time::Instant,
}
impl Publication {
    pub(crate) fn check_current(&mut self) -> io::Result<()> {
        let now = std::time::Instant::now();
        if now < self.next_check {
            return Ok(());
        }
        self.next_check = now + std::time::Duration::from_secs(1);
        if !history::is_current(&self.record)? {
            return Err(io::Error::other(
                "Integration session superseded by a newer connection",
            ));
        }
        Ok(())
    }
}
impl Drop for Publication {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path).is_ok_and(|m| (m.dev(), m.ino()) == self.identity) {
            let _ = fs::remove_file(&self.path);
        }
    }
}
fn directory() -> io::Result<PathBuf> {
    Ok(
        PathBuf::from(env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is required"))?)
            .join(".cache/newport/sessions"),
    )
}
fn private(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|m| m.is_dir() && m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o077 == 0)
}
pub(crate) fn publish(scope: &Scope, client: &str, root: &Path) -> io::Result<Publication> {
    let directory = directory()?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&directory)?;
    if !private(&directory) {
        return Err(io::Error::other(
            "Integration registry is not private to this account",
        ));
    }
    // Best effort: a bad unrelated record must not prevent a new session.
    let previous = records().unwrap_or_default();
    let file = tempfile::Builder::new()
        .prefix("session-")
        .suffix(".json")
        .tempfile_in(directory)?;
    file.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    fs2::FileExt::try_lock_exclusive(file.as_file())?;
    let record = Record {
        session: scope.session.clone(),
        runtime: scope.runtime.clone(),
        client: client.into(),
        root: root.into(),
    };
    serde_json::to_writer(file.as_file(), &record)?;
    history::publish(&record, &previous)?;
    let m = file.as_file().metadata()?;
    let (file, path) = file.keep().map_err(io::Error::other)?;
    Ok(Publication {
        path,
        identity: (m.dev(), m.ino()),
        _lock: file,
        record,
        next_check: std::time::Instant::now(),
    })
}
fn active(root: &Path) -> bool {
    if !private(root) {
        return false;
    }
    let Ok(lock) = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(root.join("agent.sock.lock"))
    else {
        return false;
    };
    if !lock.metadata().is_ok_and(|m| {
        m.is_file() && m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o077 == 0
    }) {
        return false;
    }
    matches!(fs2::FileExt::try_lock_exclusive(&lock), Err(e) if e.kind() == io::ErrorKind::WouldBlock)
}
fn enabled(root: &Path, capability: Option<&str>) -> bool {
    fs::read_to_string(root.join("features")).is_ok_and(|features| match capability {
        Some(capability) => features.split_whitespace().any(|f| f == capability),
        None => !features.trim().is_empty(),
    })
}
pub(crate) fn select(capability: Option<&str>) -> io::Result<Selection> {
    let selector = env::var_os(SELECTOR)
        .map(|s| {
            s.into_string()
                .map_err(|_| io::Error::other("Invalid NEWPORT_SESSION"))
        })
        .transpose()?;
    if selector
        .as_ref()
        .is_some_and(|id| uuid::Uuid::parse_str(id).is_err())
    {
        return Err(io::Error::other(
            "Invalid NEWPORT_SESSION; use a session ID from newport-agent sessions",
        ));
    }
    let records = records()?;
    let mut candidates = history::reconcile(records.clone(), selector.as_deref())?
        .into_iter()
        .filter(|r| active(&r.root) && enabled(&r.root, capability))
        .map(|r| Selection {
            root: r.root,
            session: Some(r.session),
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        if let Some(selector) = selector.as_deref() {
            let home = PathBuf::from(
                env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is required"))?,
            );
            let standalone = crate::paths::standalone_root(&home);
            // A standalone bridge is another possible destination, even if disabled.
            if !active(&standalone) {
                if let Some(record) = history::recover_unknown(&records, selector, capability)? {
                    candidates.push(Selection {
                        root: record.root,
                        session: Some(record.session),
                    });
                } else {
                    // Another helper may have just recorded this alias. Resolve
                    // it once more before failing; no operation has been sent.
                    candidates = history::reconcile(self::records()?, Some(selector))?
                        .into_iter()
                        .filter(|r| active(&r.root) && enabled(&r.root, capability))
                        .map(|r| Selection {
                            root: r.root,
                            session: Some(r.session),
                        })
                        .collect();
                }
            }
        }
    }
    // Standalone bridges participate in ambiguity checks, but an explicit
    // session selector must never fall back to another computer or bridge.
    if selector.is_none() {
        let home =
            PathBuf::from(env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is required"))?);
        let standalone = crate::paths::standalone_root(&home);
        if active(&standalone) && enabled(&standalone, capability) {
            candidates.push(Selection {
                root: standalone,
                session: None,
            });
        }
    }
    match candidates.len() {
        1 => Ok(candidates.remove(0)),
        0 if selector.is_some() => Err(io::Error::other(format!(
            "Selected Newport session {} is unavailable or does not permit this integration. Run newport-agent sessions to select an active session; NEWPORT_SESSION may refer to an earlier Newport session.",
            selector.as_deref().unwrap()
        ))),
        0 => Err(io::Error::other("No permitted Newport integration is active for this session. Enable it in Newport.")),
        _ => Err(io::Error::other("Multiple Newport sessions are available. Set NEWPORT_SESSION using newport-agent sessions.")),
    }
}
fn records() -> io::Result<Vec<Record>> {
    let directory = directory()?;
    if !directory.exists() {
        return Ok(Vec::new());
    }
    if !private(&directory) {
        return Err(io::Error::other(
            "Integration registry is not private to this account",
        ));
    }
    let mut records = Vec::new();
    for (index, entry) in fs::read_dir(directory)?.enumerate() {
        if index >= 4096 {
            return Err(io::Error::other("Integration registry needs cleanup"));
        }
        let path = entry?.path();
        let Ok(file) = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
        else {
            continue;
        };
        let m = file.metadata()?;
        if !m.is_file()
            || m.uid() != unsafe { libc::geteuid() }
            || m.mode() & 0o077 != 0
            || m.len() > 4096
        {
            continue;
        }
        let Ok(record) = serde_json::from_reader::<_, Record>(&file) else {
            continue;
        };
        if uuid::Uuid::parse_str(&record.session).is_ok()
            && uuid::Uuid::parse_str(&record.runtime).is_ok()
            && record.root.is_absolute()
        {
            // A record whose endpoint no longer holds its ownership lock is stale.
            // Delete only our recognized, unchanged record.
            if !active(&record.root)
                && path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.starts_with("session-") && s.ends_with(".json"))
                && fs2::FileExt::try_lock_exclusive(&file).is_ok()
                && fs::symlink_metadata(&path)
                    .is_ok_and(|current| (current.dev(), current.ino()) == (m.dev(), m.ino()))
            {
                let _ = fs::remove_file(&path);
                continue;
            }
            records.push(record);
        }
    }
    Ok(records)
}
pub fn list() -> io::Result<()> {
    for record in history::reconcile(records()?, None)?
        .into_iter()
        .filter(|r| active(&r.root))
    {
        println!(
            "{}  client {}  {}",
            record.session,
            record.client,
            fs::read_to_string(record.root.join("features"))
                .unwrap_or_default()
                .trim()
        );
    }
    Ok(())
}
