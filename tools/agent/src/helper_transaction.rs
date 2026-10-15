//! Durable ownership evidence for explicit helper installation. Never used by feature toggles.
use crate::deployment::{acquire_lock, private_directory, private_file, valid_hash};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Write},
    os::unix::fs::{symlink, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};
use uuid::Uuid;

const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_JOURNAL: u64 = 1024 * 1024;
#[path = "helper_history.rs"]
mod history;
#[path = "helper_removal.rs"]
mod removal;
pub(crate) use history::{history, recover_standalone, review_standalone};

/// Non-mutating permission preflight. Atomic replacement needs directory access
/// as well as file access; effective-ID checks include ACLs and mount policy.
/// Actual writes must still handle failures and revalidate ownership/content.
pub(crate) fn check_write_permission(path: &Path) -> io::Result<()> {
    fn denied(path: &Path, reason: &str) -> io::Error {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} {reason}", path.display()),
        )
    }
    fn access(path: &Path, mode: libc::c_int) -> io::Result<()> {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let name = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::other("Invalid helper path"))?;
        // SAFETY: name is NUL-terminated and lives through this call.
        if unsafe { libc::faccessat(libc::AT_FDCWD, name.as_ptr(), mode, libc::AT_EACCESS) } != 0 {
            return Err(denied(
                path,
                &format!("is not accessible: {}", io::Error::last_os_error()),
            ));
        }
        Ok(())
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.uid() != unsafe { libc::geteuid() } {
                return Err(denied(path, "belongs to another account"));
            }
            if metadata.is_file() {
                if metadata.mode() & 0o200 == 0 {
                    return Err(denied(path, "is read-only"));
                }
                access(path, libc::R_OK | libc::W_OK)?;
            } else if !metadata.is_symlink() {
                return Err(denied(path, "is not an ordinary file"));
            }
            // Managed aliases are replaced as links, never written through.
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Missing helper parent"))?;
    loop {
        match fs::metadata(parent) {
            Ok(metadata) => {
                if !metadata.is_dir() {
                    return Err(denied(parent, "is not a directory"));
                }
                // Respect explicitly read-only owned directories even as root.
                if metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o300 != 0o300
                {
                    return Err(denied(
                        parent,
                        "does not permit creating or replacing files",
                    ));
                }
                return access(parent, libc::W_OK | libc::X_OK);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                parent = parent.parent().ok_or(error)?;
            }
            Err(error) => return Err(error),
        }
    }
}

pub(crate) fn removal_plan(
    home: &Path,
) -> io::Result<crate::deployment_protocol::HelperRemovalPlan> {
    removal::inspect(home)
}

pub(crate) fn prepare_removal(home: &Path, id: Uuid) -> io::Result<Transaction> {
    removal::prepare(home, id)
}

/// A first binary-only deployment records that it owns no shell changes.
/// Never reconstruct missing history in an existing helper directory.
pub(crate) fn initialize_ownership(home: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let root = home.join(".local/share/newport/needle/helpers");
    let created = match fs::DirBuilder::new().mode(0o700).create(&root) {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
        Err(error) => return Err(error),
    };
    let tx = Transaction::open(home, Uuid::new_v4())?;
    // A concurrent standalone helper installation may have published history
    // before we acquired its lock. Preserve that history instead of replacing it.
    match Journal::load_existing(&tx.root) {
        Ok(_) => Ok(()),
        Err(error) if created && error.kind() == io::ErrorKind::NotFound => {
            tx.journal.save(&tx.root)
        }
        Err(error) => Err(error),
    }?;
    if created {
        fs::File::open(tx.root.parent().unwrap())?.sync_all()?;
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) enum Snapshot {
    Missing,
    Link(PathBuf),
    File { hash: String, mode: u32 },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    path: PathBuf,
    before: Snapshot,
    after: Snapshot,
    /// Only shell planning may grant block-level ownership. Old records retain
    /// whole-file comparison semantics rather than inferring ownership by name.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    shell: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kept: Option<Snapshot>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum State {
    Prepared,
    Applied,
    RollingBack,
    RolledBack,
    NeedsResolution,
}
impl State {
    fn unsettled(self) -> bool {
        !matches!(self, Self::Applied | Self::RolledBack)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    id: Uuid,
    state: State,
    changes: Vec<Change>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    removal: bool,
    /// Absence in older records is unknown provenance, never proof of standalone ownership.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    origin: Option<Origin>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Origin {
    Standalone,
    Deployment,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    records: Vec<Record>,
}
impl Journal {
    fn load(root: &Path) -> io::Result<Self> {
        match Self::load_existing(root) {
            Ok(journal) => Ok(journal),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self {
                version: 1,
                records: Vec::new(),
            }),
            Err(e) => Err(e),
        }
    }
    fn load_existing(root: &Path) -> io::Result<Self> {
        let file = private_file(&root.join("journal.json"))?;
        let mut bytes = Vec::new();
        file.take(MAX_JOURNAL + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_JOURNAL {
            return Err(io::Error::other("Needle helper journal exceeds limit"));
        }
        let journal: Self = serde_json::from_slice(&bytes)?;
        let mut ids = std::collections::HashSet::new();
        if journal.version != 1 || journal.records.len() > 1000 {
            return Err(io::Error::other("Unsupported Needle helper journal"));
        }
        for record in &journal.records {
            let mut paths = std::collections::HashSet::new();
            if record.id.is_nil()
                || !ids.insert(record.id)
                || record.changes.len() > 32
                || (record.removal && record.origin.is_some())
            {
                return Err(io::Error::other("Invalid Needle helper record"));
            }
            for change in &record.changes {
                if change.shell
                    && (!matches!(change.before, Snapshot::Missing | Snapshot::File { .. })
                        || !matches!(change.after, Snapshot::File { .. }))
                {
                    return Err(io::Error::other("Invalid shell ownership record"));
                }
                if !change.path.is_absolute()
                    || !paths.insert(&change.path)
                    || change
                        .path
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir))
                {
                    return Err(io::Error::other("Invalid Needle helper path"));
                }
                for state in [&change.before, &change.after]
                    .into_iter()
                    .chain(change.kept.as_ref())
                {
                    if let Snapshot::File { hash, mode } = state {
                        valid_hash(hash)?;
                        if *mode & !0o777 != 0 {
                            return Err(io::Error::other("Invalid Needle helper file mode"));
                        }
                    }
                }
            }
        }
        Ok(journal)
    }
    fn save(&self, root: &Path) -> io::Result<()> {
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() as u64 > MAX_JOURNAL {
            return Err(io::Error::other("Needle helper journal exceeds limit"));
        }
        let mut file = tempfile::NamedTempFile::new_in(root)?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        file.persist(root.join("journal.json"))
            .map_err(io::Error::other)?;
        fs::File::open(root)?.sync_all()
    }
}

/// Holds the helper lock through planning, journal publication, and mutation.
pub(crate) struct Transaction {
    root: PathBuf,
    journal: Journal,
    id: Uuid,
    changes: Vec<Change>,
    _lock: fs::File,
}
impl Transaction {
    pub(crate) fn open(home: &Path, id: Uuid) -> io::Result<Self> {
        if id.is_nil() {
            return Err(io::Error::other("Invalid helper operation ID"));
        }
        let root = home.join(".local/share/newport/needle/helpers");
        private_directory(&root)?;
        private_directory(&root.join("objects"))?;
        let lock_path = root.join("lock");
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&lock_path)
        {
            Ok(_) => (),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e),
        }
        let lock = private_file(&lock_path)?;
        acquire_lock(&lock)?;
        let journal = Journal::load(&root)?;
        if journal.records.iter().any(|r| r.state.unsettled()) {
            return Err(io::Error::other(
                "Needle helper changes need resolution before another installation",
            ));
        }
        if journal.records.iter().any(|r| r.id == id) {
            return Err(io::Error::other("Needle helper operation already recorded; observe its outcome instead of replaying it"));
        }
        if journal.records.len() == 1000 {
            return Err(io::Error::other(
                "Needle helper journal is full; retain ownership evidence before clearing history",
            ));
        }
        Ok(Self {
            root,
            journal,
            id,
            changes: Vec::new(),
            _lock: lock,
        })
    }
    pub(crate) fn cancel_plan(mut self) -> io::Result<()> {
        self.journal.records.push(Record {
            id: self.id,
            state: State::RolledBack,
            changes: self.changes,
            removal: false,
            origin: None,
        });
        self.journal.save(&self.root)
    }
    /// Explicit recovery only; never called by ordinary installation or discovery.
    pub(crate) fn recover(home: &Path, id: Uuid) -> io::Result<()> {
        let root = home.join(".local/share/newport/needle/helpers");
        let lock = private_file(&root.join("lock"))?;
        acquire_lock(&lock)?;
        let journal = Journal::load(&root)?;
        let index = journal
            .records
            .iter()
            .position(|r| r.id == id)
            .ok_or_else(|| {
                io::Error::other("No helper ownership record; manual review is required")
            })?;
        let mut transaction = Self {
            root,
            journal,
            id,
            changes: Vec::new(),
            _lock: lock,
        };
        transaction.restore_record(index)
    }
    fn restore_record(&mut self, index: usize) -> io::Result<()> {
        if self.journal.records[index].state == State::RolledBack {
            return Ok(());
        }
        let paths: std::collections::HashSet<_> = self.journal.records[index]
            .changes
            .iter()
            .map(|c| &c.path)
            .collect();
        if self.journal.records[index + 1..].iter().any(|r| {
            r.state != State::RolledBack && r.changes.iter().any(|c| paths.contains(&c.path))
        }) {
            return Err(io::Error::other(
                "A later helper operation changed these paths; refusing to undo it",
            ));
        }
        self.rollback(index)
    }
    fn hash(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
    pub(crate) fn file(&self, bytes: &[u8], mode: u32) -> io::Result<Snapshot> {
        if bytes.len() as u64 > MAX_FILE || mode & !0o777 != 0 {
            return Err(io::Error::other(
                "Unsupported helper file size or permissions",
            ));
        }
        let hash = Self::hash(bytes);
        let objects = self.root.join("objects");
        let path = objects.join(&hash);
        match private_file(&path) {
            Ok(_) => {
                // Never trust an existing blob solely because its name matches.
                self.bytes(&Snapshot::File {
                    hash: hash.clone(),
                    mode,
                })?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let mut file = tempfile::NamedTempFile::new_in(&objects)?;
                file.write_all(bytes)?;
                file.as_file().sync_all()?;
                file.persist_noclobber(&path).map_err(io::Error::other)?;
                fs::File::open(&objects)?.sync_all()?;
            }
            Err(e) => return Err(e),
        }
        Ok(Snapshot::File { hash, mode })
    }
    pub(crate) fn bytes(&self, snapshot: &Snapshot) -> io::Result<Vec<u8>> {
        let Snapshot::File { hash, .. } = snapshot else {
            return Err(io::Error::other("Helper snapshot is not a regular file"));
        };
        valid_hash(hash)?;
        let file = private_file(&self.root.join("objects").join(hash))?;
        let mut bytes = Vec::new();
        file.take(MAX_FILE + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_FILE || Self::hash(&bytes) != *hash {
            return Err(io::Error::other("Needle helper backup failed verification"));
        }
        Ok(bytes)
    }
    pub(crate) fn read(&self, path: &Path) -> io::Result<Snapshot> {
        let (snapshot, bytes) = Self::read_current(path)?;
        if let Snapshot::File { mode, .. } = snapshot {
            self.file(&bytes, mode)
        } else {
            Ok(snapshot)
        }
    }
    fn read_current(path: &Path) -> io::Result<(Snapshot, Vec<u8>)> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok((Snapshot::Missing, Vec::new()))
            }
            Err(e) => return Err(e),
        };
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err(io::Error::other("Helper path belongs to another account"));
        }
        if metadata.is_symlink() {
            return Ok((Snapshot::Link(fs::read_link(path)?), Vec::new()));
        }
        if !metadata.is_file() || metadata.nlink() != 1 || metadata.mode() & 0o7000 != 0 {
            return Err(io::Error::other(
                "Helper path is not an ordinary owned file",
            ));
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        let opened = file.metadata()?;
        if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
            return Err(io::Error::other("Helper path changed during inspection"));
        }
        let mut bytes = Vec::new();
        file.take(MAX_FILE + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_FILE {
            return Err(io::Error::other("Helper file exceeds size limit"));
        }
        Ok((
            Snapshot::File {
                hash: Self::hash(&bytes),
                mode: metadata.mode() & 0o777,
            },
            bytes,
        ))
    }
    pub(crate) fn plan(
        &mut self,
        path: &Path,
        before: Snapshot,
        after: Snapshot,
    ) -> io::Result<()> {
        self.plan_kind(path, before, after, false)
    }
    pub(crate) fn plan_shell(
        &mut self,
        path: &Path,
        before: Snapshot,
        after: Snapshot,
    ) -> io::Result<()> {
        self.plan_kind(path, before, after, true)
    }
    fn plan_kind(
        &mut self,
        path: &Path,
        before: Snapshot,
        after: Snapshot,
        shell: bool,
    ) -> io::Result<()> {
        if before == after {
            return Ok(());
        }
        if !path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            || self.changes.len() == 32
            || self.changes.iter().any(|c| c.path == path)
        {
            return Err(io::Error::other("Invalid or duplicate helper change"));
        }
        check_write_permission(path)?;
        if let Snapshot::File { mode, .. } = &before {
            if mode & 0o200 == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Helper file is read-only; leaving it unchanged",
                ));
            }
            fs::OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(path)?;
        }
        self.changes.push(Change {
            path: path.to_owned(),
            before,
            after,
            shell,
            kept: None,
        });
        Ok(())
    }
    fn replace(&self, path: &Path, expected: &Snapshot, next: &Snapshot) -> io::Result<()> {
        if self.read(path)? != *expected {
            return Err(io::Error::other(format!(
                "{} changed since inspection; preserving it",
                path.display()
            )));
        }
        check_write_permission(path)?;
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("Missing helper parent"))?;
        fs::create_dir_all(parent)?;
        match next {
            Snapshot::Missing => fs::remove_file(path)?,
            Snapshot::Link(target) if *expected == Snapshot::Missing => {
                // Creation must still fail if another process fills the path
                // after the inspection above.
                symlink(target, path)?;
            }
            Snapshot::Link(target) => {
                let staging = tempfile::tempdir_in(parent)?;
                let link = staging.path().join("link");
                symlink(target, &link)?;
                if self.read(path)? != *expected {
                    return Err(io::Error::other("Helper changed before replacement"));
                }
                fs::rename(link, path)?;
            }
            Snapshot::File { mode, .. } => {
                let bytes = self.bytes(next)?;
                let mut file = tempfile::NamedTempFile::new_in(parent)?;
                file.as_file()
                    .set_permissions(fs::Permissions::from_mode(*mode))?;
                file.write_all(&bytes)?;
                file.as_file().sync_all()?;
                if self.read(path)? != *expected {
                    return Err(io::Error::other("Helper changed before replacement"));
                }
                if *expected == Snapshot::Missing {
                    file.persist_noclobber(path).map_err(io::Error::other)?;
                } else {
                    file.persist(path).map_err(io::Error::other)?;
                }
            }
        }
        fs::File::open(parent)?.sync_all()
    }
    #[cfg(test)]
    pub(crate) fn apply(mut self) -> io::Result<()> {
        self.apply_with(|| Ok(()))
    }
    pub(crate) fn apply_install(mut self, origin: Origin) -> io::Result<()> {
        self.apply_with_kind(|| Ok(()), false, Some(origin))
    }
    pub(crate) fn apply_removal(
        mut self,
        lease: &crate::installation_use::MaintenanceLease,
    ) -> io::Result<()> {
        lease.validate()?;
        self.apply_with_kind(|| lease.validate(), true, None)
    }
    #[cfg(test)]
    fn apply_with(&mut self, mut after_write: impl FnMut() -> io::Result<()>) -> io::Result<()> {
        self.apply_with_kind(&mut after_write, false, None)
    }
    fn apply_with_kind(
        &mut self,
        mut after_write: impl FnMut() -> io::Result<()>,
        removal: bool,
        origin: Option<Origin>,
    ) -> io::Result<()> {
        // Check every target before changing any of them. If access changed,
        // persist proof that no writes started rather than requiring recovery.
        let preflight = self.changes.iter().try_for_each(|change| {
            check_write_permission(&change.path)?;
            if self.read(&change.path)? != change.before {
                return Err(io::Error::other(
                    "Helper changed during planning; no changes applied",
                ));
            }
            Ok(())
        });
        if let Err(error) = preflight {
            self.journal.records.push(Record {
                id: self.id,
                state: State::RolledBack,
                changes: self.changes.clone(),
                removal,
                origin,
            });
            self.journal.save(&self.root)?;
            return Err(error);
        }
        let index = self.journal.records.len();
        self.journal.records.push(Record {
            id: self.id,
            state: State::Prepared,
            changes: self.changes.clone(),
            removal,
            origin,
        });
        self.journal.save(&self.root)?;
        for change in &self.changes {
            if removal {
                after_write()?;
            }
            if let Err(error) = self
                .replace(&change.path, &change.before, &change.after)
                .and_then(|()| after_write())
            {
                let recovery = self.rollback(index);
                return Err(io::Error::other(format!(
                    "Helper installation failed: {error}; rollback: {}",
                    match recovery {
                        Ok(()) => "confirmed".into(),
                        Err(e) => e.to_string(),
                    }
                )));
            }
        }
        self.journal.records[index].state = State::Applied;
        self.journal.save(&self.root)
    }
    fn rollback(&mut self, index: usize) -> io::Result<()> {
        self.journal.records[index].state = State::RollingBack;
        self.journal.save(&self.root)?;
        let mut errors = Vec::new();
        for change in self.journal.records[index].changes.iter().rev() {
            if change.kept.is_some() {
                continue;
            }
            let result = (|| {
                let current = self.read(&change.path)?;
                if current == change.before {
                    return Ok(());
                }
                if current != change.after {
                    if change.shell {
                        return self.restore_shell(change, &current);
                    }
                    return Err(io::Error::other(format!(
                        "{} has a later edit",
                        change.path.display()
                    )));
                }
                self.replace(&change.path, &change.after, &change.before)
            })();
            if let Err(e) = result {
                errors.push(e.to_string());
            }
        }
        self.journal.records[index].state = if errors.is_empty() {
            State::RolledBack
        } else {
            State::NeedsResolution
        };
        self.journal.save(&self.root)?;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(io::Error::other(errors.join("; ")))
        }
    }
    fn restore_shell(&self, change: &Change, current: &Snapshot) -> io::Result<()> {
        let Snapshot::File { mode, .. } = current else {
            return Err(io::Error::other(format!(
                "{} is no longer the recorded shell file",
                change.path.display()
            )));
        };
        let now = self.bytes(current)?;
        let restored = self.shell_restoration(change, current, &now)?;
        if restored.as_bytes() == now {
            return Ok(());
        }
        let restored = self.file(restored.as_bytes(), *mode)?;
        self.replace(&change.path, current, &restored)
    }
    fn shell_restoration(
        &self,
        change: &Change,
        current: &Snapshot,
        now: &[u8],
    ) -> io::Result<String> {
        let Snapshot::File { mode, .. } = current else {
            return Err(io::Error::other("Shell file was replaced"));
        };
        let before = if change.before == Snapshot::Missing {
            Vec::new()
        } else {
            self.bytes(&change.before)?
        };
        let after = self.bytes(&change.after)?;
        let text = |bytes| std::str::from_utf8(bytes).map_err(io::Error::other);
        let restored = crate::shell_setup::restore_block(text(&before)?, text(&after)?, text(now)?)
            .map_err(|e| io::Error::other(format!("{}: {e}", change.path.display())))?;
        if restored.as_bytes() == now {
            return Ok(restored);
        }
        if mode & 0o200 == 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} is read-only; preserving it", change.path.display()),
            ));
        }
        Ok(restored)
    }
    pub(crate) fn review(
        home: &Path,
        id: Uuid,
        keep: Option<(usize, &str)>,
    ) -> io::Result<crate::deployment_protocol::HelperReview> {
        use crate::deployment_protocol::{HelperFile, HelperReview};
        let root = home.join(".local/share/newport/needle/helpers");
        let lock = private_file(&root.join("lock"))?;
        acquire_lock(&lock)?;
        let journal = Journal::load(&root)?;
        let index = journal
            .records
            .iter()
            .position(|r| r.id == id)
            .ok_or_else(|| io::Error::other("No helper ownership record"))?;
        if !matches!(
            journal.records[index].state,
            State::RollingBack | State::NeedsResolution
        ) {
            return Err(io::Error::other(
                "Start restoration before reviewing conflicts",
            ));
        }
        if journal.records[index + 1..].iter().any(|r| {
            r.state != State::RolledBack
                && r.changes.iter().any(|c| {
                    journal.records[index]
                        .changes
                        .iter()
                        .any(|old| old.path == c.path)
                })
        }) {
            return Err(io::Error::other(
                "A later helper operation owns these paths",
            ));
        }
        let mut tx = Self {
            root,
            journal,
            id,
            changes: Vec::new(),
            _lock: lock,
        };
        let mut files = Vec::new();
        for (file_index, change) in tx.journal.records[index].changes.iter().enumerate() {
            let mut file = HelperFile {
                index: file_index,
                path: change.path.display().to_string(),
                status: "conflict".into(),
                message: String::new(),
                fingerprint: None,
            };
            match Self::read_current(&change.path) {
                _ if change.kept.is_some() => {
                    file.status = "kept".into();
                    file.message = "Kept by your choice; excluded from this restore.".into();
                }
                Err(error) => file.message = error.to_string(),
                Ok((current, bytes)) => {
                    if current == change.before {
                        file.status = "restored".into();
                        file.message = "Already restored.".into();
                    } else if current == change.after
                        || (change.shell && tx.shell_restoration(change, &current, &bytes).is_ok())
                    {
                        file.status = "restorable".into();
                        file.message =
                            "Ready to restore; surrounding shell edits will be preserved.".into();
                    } else {
                        file.message = "This path differs from the recorded installation. Keep it, or resolve the edit and retry restore.".into();
                        file.fingerprint = Some(Self::hash(&serde_json::to_vec(&(
                            id, file_index, &current,
                        ))?));
                    }
                }
            }
            files.push(file);
        }
        if let Some((file_index, fingerprint)) = keep {
            let file = files
                .get(file_index)
                .ok_or_else(|| io::Error::other("Unknown helper path"))?;
            if let Some(kept) = &tx.journal.records[index].changes[file_index].kept {
                if Self::hash(&serde_json::to_vec(&(id, file_index, kept))?) == fingerprint {
                    return Ok(HelperReview {
                        operation_id: id,
                        files,
                    });
                }
            }
            if file.status != "conflict" || file.fingerprint.as_deref() != Some(fingerprint) {
                return Err(io::Error::other(
                    "The file changed since review. Refresh before choosing to keep it.",
                ));
            }
            let change = &tx.journal.records[index].changes[file_index];
            let (current, _) = Self::read_current(&change.path)?;
            if Self::hash(&serde_json::to_vec(&(id, file_index, &current))?) != fingerprint {
                return Err(io::Error::other(
                    "The file changed since review. Refresh before choosing to keep it.",
                ));
            }
            tx.journal.records[index].changes[file_index].kept = Some(current);
            tx.journal.save(&tx.root)?;
            files[file_index].status = "kept".into();
            files[file_index].message = "Kept by your choice; excluded from this restore.".into();
            files[file_index].fingerprint = None;
        }
        Ok(HelperReview {
            operation_id: id,
            files,
        })
    }
}

/// Read-only proof used to reconcile an activation interrupted after helper setup.
pub(crate) fn outcome(home: &Path, id: Uuid) -> io::Result<Option<State>> {
    let root = home.join(".local/share/newport/needle/helpers");
    Ok(Journal::load(&root)?
        .records
        .into_iter()
        .find(|r| r.id == id)
        .map(|r| r.state))
}

/// Removal cannot infer an empty history from lost ownership evidence.
pub(crate) fn removal_outcome(home: &Path, id: Uuid) -> io::Result<Option<State>> {
    let root = home.join(".local/share/newport/needle/helpers");
    let journal = Journal::load_existing(&root)?;
    let record = journal.records.into_iter().find(|r| r.id == id);
    if record.as_ref().is_some_and(|r| !r.removal) {
        return Err(io::Error::other(
            "Removal ID belongs to another helper operation",
        ));
    }
    Ok(record.map(|r| r.state))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn permission_preflight_checks_existing_files_and_missing_parent_directories() {
        let home = tempfile::tempdir().unwrap();
        let parent = home.path().join("config");
        fs::create_dir(&parent).unwrap();
        let file = parent.join("rc");
        fs::write(&file, "user content").unwrap();
        check_write_permission(&file).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o400)).unwrap();
        assert_eq!(
            check_write_permission(&file).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        for mode in [0o500, 0o600] {
            fs::set_permissions(&parent, fs::Permissions::from_mode(mode)).unwrap();
            for path in [
                &file,
                &parent.join("missing"),
                &parent.join("nested/deeper/rc"),
            ] {
                assert_eq!(
                    check_write_permission(path).unwrap_err().kind(),
                    io::ErrorKind::PermissionDenied
                );
            }
            fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
        }
        assert_eq!(fs::read_to_string(file).unwrap(), "user content");
        assert!(!parent.join("nested").exists());
    }

    #[test]
    fn permissions_are_rechecked_before_any_planned_target_changes() {
        let home = tempfile::tempdir().unwrap();
        let parent = home.path().join("config");
        fs::create_dir(&parent).unwrap();
        let file = parent.join("rc");
        fs::write(&file, "original").unwrap();
        let alias = home.path().join("alias");
        let mut tx = Transaction::open(home.path(), Uuid::new_v4()).unwrap();
        tx.plan(&alias, Snapshot::Missing, Snapshot::Link("agent".into()))
            .unwrap();
        let before = tx.read(&file).unwrap();
        let after = tx.file(b"updated", 0o600).unwrap();
        tx.plan_shell(&file, before, after).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o500)).unwrap();
        let result = tx.apply();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert!(fs::symlink_metadata(alias).is_err());
        assert_eq!(fs::read_to_string(file).unwrap(), "original");
    }

    fn shell_plan(home: &Path, id: Uuid, original: Option<&str>) -> Transaction {
        let path = home.join(".bashrc");
        if let Some(original) = original {
            fs::write(&path, original).unwrap();
        }
        let mut tx = Transaction::open(home, id).unwrap();
        let before = tx.read(&path).unwrap();
        let contents = format!(
            "{}{}",
            original.unwrap_or(""),
            crate::shell_setup::block(false)
        );
        let after = tx.file(contents.as_bytes(), 0o600).unwrap();
        tx.plan_shell(&path, before, after).unwrap();
        tx
    }
    #[test]
    fn shell_recovery_keeps_later_text_and_permissions_in_new_and_existing_files() {
        for original in [None, Some("# original\n")] {
            let home = tempfile::tempdir().unwrap();
            let id = Uuid::new_v4();
            shell_plan(home.path(), id, original).apply().unwrap();
            let path = home.path().join(".bashrc");
            let current = format!(
                "# later prefix\n{}# later suffix\n",
                fs::read_to_string(&path).unwrap()
            );
            fs::write(&path, current).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
            Transaction::recover(home.path(), id).unwrap();
            let expected = format!("# later prefix\n{}# later suffix\n", original.unwrap_or(""));
            assert_eq!(fs::read_to_string(&path).unwrap(), expected);
            assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o640);
            Transaction::recover(home.path(), id).unwrap();
            assert_eq!(fs::read_to_string(&path).unwrap(), expected);
        }
    }
    #[test]
    fn partial_shell_restore_can_resume_after_another_path_conflicts() {
        let home = tempfile::tempdir().unwrap();
        let id = Uuid::new_v4();
        let mut tx = shell_plan(home.path(), id, Some("# original\n"));
        let alias = home.path().join("alias");
        tx.plan(
            &alias,
            Snapshot::Missing,
            Snapshot::Link("newport-agent".into()),
        )
        .unwrap();
        tx.apply().unwrap();
        fs::remove_file(&alias).unwrap();
        fs::write(&alias, "unrelated command").unwrap();
        let shell = home.path().join(".bashrc");
        let contents = format!("{}# later edit\n", fs::read_to_string(&shell).unwrap());
        fs::write(&shell, contents).unwrap();
        assert!(Transaction::recover(home.path(), id).is_err());
        assert_eq!(
            outcome(home.path(), id).unwrap(),
            Some(State::NeedsResolution)
        );
        assert_eq!(
            fs::read_to_string(&shell).unwrap(),
            "# original\n# later edit\n"
        );
        fs::write(
            &shell,
            "# original\n# later edit\n# after partial restore\n",
        )
        .unwrap();
        fs::remove_file(&alias).unwrap();
        symlink("newport-agent", &alias).unwrap();
        Transaction::recover(home.path(), id).unwrap();
        assert_eq!(outcome(home.path(), id).unwrap(), Some(State::RolledBack));
        assert_eq!(
            fs::read_to_string(shell).unwrap(),
            "# original\n# later edit\n# after partial restore\n"
        );
    }
    #[test]
    fn changed_shell_block_and_read_only_file_remain_conflicts() {
        for read_only in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let id = Uuid::new_v4();
            shell_plan(home.path(), id, None).apply().unwrap();
            let shell = home.path().join(".bashrc");
            let original = fs::read_to_string(&shell).unwrap();
            let current = if read_only {
                format!("{original}# later\n")
            } else {
                original.replace(" env ", " edited ")
            };
            fs::write(&shell, &current).unwrap();
            if read_only {
                fs::set_permissions(&shell, fs::Permissions::from_mode(0o400)).unwrap();
            }
            assert!(Transaction::recover(home.path(), id).is_err());
            assert_eq!(
                outcome(home.path(), id).unwrap(),
                Some(State::NeedsResolution)
            );
            assert_eq!(fs::read_to_string(shell).unwrap(), current);
        }
    }
    #[test]
    fn legacy_receipt_never_infers_block_ownership_from_shell_markers() {
        let home = tempfile::tempdir().unwrap();
        let id = Uuid::new_v4();
        shell_plan(home.path(), id, None).apply().unwrap();
        let root = home.path().join(".local/share/newport/needle/helpers");
        let mut journal = Journal::load(&root).unwrap();
        journal.records[0].changes[0].shell = false;
        journal.save(&root).unwrap();
        assert!(!fs::read_to_string(root.join("journal.json"))
            .unwrap()
            .contains("\"shell\""));
        let shell = home.path().join(".bashrc");
        let current = format!("{}# user edit\n", fs::read_to_string(&shell).unwrap());
        fs::write(&shell, &current).unwrap();
        assert!(Transaction::recover(home.path(), id).is_err());
        assert_eq!(fs::read_to_string(shell).unwrap(), current);
    }
    #[test]
    fn conflict_review_is_read_only_and_keep_requires_the_reviewed_state() {
        let home = tempfile::tempdir().unwrap();
        let id = Uuid::new_v4();
        shell_plan(home.path(), id, None).apply().unwrap();
        let path = home.path().join(".bashrc");
        let edited = fs::read_to_string(&path)
            .unwrap()
            .replace(" env ", " edited ");
        fs::write(&path, &edited).unwrap();
        assert!(Transaction::recover(home.path(), id).is_err());
        let root = home.path().join(".local/share/newport/needle/helpers");
        let saved = fs::read(root.join("journal.json")).unwrap();
        let objects = fs::read_dir(root.join("objects")).unwrap().count();
        let review = Transaction::review(home.path(), id, None).unwrap();
        let token = review.files[0].fingerprint.clone().unwrap();
        assert_eq!(review.files[0].status, "conflict");
        assert_eq!(fs::read(root.join("journal.json")).unwrap(), saved);
        assert_eq!(fs::read_dir(root.join("objects")).unwrap().count(), objects);
        fs::write(&path, format!("{edited}# newer edit\n")).unwrap();
        assert!(Transaction::review(home.path(), id, Some((0, &token))).is_err());
        assert_eq!(fs::read(root.join("journal.json")).unwrap(), saved);
        let fresh = Transaction::review(home.path(), id, None).unwrap();
        let token = fresh.files[0].fingerprint.as_deref().unwrap();
        let decision = Transaction::review(home.path(), id, Some((0, token))).unwrap();
        assert_eq!(decision.files[0].status, "kept");
        let kept_journal = fs::read(root.join("journal.json")).unwrap();
        fs::write(&path, "# another edit after keeping\n").unwrap();
        Transaction::review(home.path(), id, Some((0, token))).unwrap();
        assert_eq!(fs::read(root.join("journal.json")).unwrap(), kept_journal);
        Transaction::recover(home.path(), id).unwrap();
        assert_eq!(outcome(home.path(), id).unwrap(), Some(State::RolledBack));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "# another edit after keeping\n"
        );
        assert!(Transaction::review(home.path(), id, Some((0, token))).is_err());
    }
    fn planned(home: &Path, id: Uuid) -> Transaction {
        let mut tx = Transaction::open(home, id).unwrap();
        let shell = home.join(".bashrc");
        let before = tx.read(&shell).unwrap();
        let after = tx.file(b"# user\n# managed\n", 0o640).unwrap();
        tx.plan(&shell, before, after).unwrap();
        tx.plan(
            &home.join("alias"),
            Snapshot::Missing,
            Snapshot::Link("newport-agent".into()),
        )
        .unwrap();
        tx
    }
    #[test]
    fn planning_failure_changes_no_targets() {
        let home = tempfile::tempdir().unwrap();
        fs::write(home.path().join(".bashrc"), "# user\n").unwrap();
        let tx = planned(home.path(), Uuid::new_v4());
        fs::write(home.path().join("alias"), "another command").unwrap();
        assert!(tx.apply().is_err());
        assert_eq!(fs::read(home.path().join(".bashrc")).unwrap(), b"# user\n");
        assert_eq!(
            fs::read(home.path().join("alias")).unwrap(),
            b"another command"
        );
    }
    #[test]
    fn failed_write_rolls_back_and_retains_verified_original() {
        let home = tempfile::tempdir().unwrap();
        let shell = home.path().join(".bashrc");
        fs::write(&shell, "# user\n").unwrap();
        fs::set_permissions(&shell, fs::Permissions::from_mode(0o640)).unwrap();
        let id = Uuid::new_v4();
        let mut tx = planned(home.path(), id);
        assert!(tx
            .apply_with(|| Err(io::Error::other("injected failure")))
            .is_err());
        assert_eq!(fs::read(&shell).unwrap(), b"# user\n");
        assert_eq!(fs::metadata(&shell).unwrap().mode() & 0o777, 0o640);
        assert!(!home.path().join("alias").exists());
        assert_eq!(outcome(home.path(), id).unwrap(), Some(State::RolledBack));
        let Snapshot::File { hash, .. } = &tx.journal.records[0].changes[0].before else {
            panic!()
        };
        assert_eq!(
            fs::read(tx.root.join("objects").join(hash)).unwrap(),
            b"# user\n"
        );
        drop(tx);
        assert!(Transaction::open(home.path(), Uuid::new_v4()).is_ok());
    }
    #[test]
    fn rollback_preserves_later_edits_and_blocks_further_changes() {
        let home = tempfile::tempdir().unwrap();
        fs::write(home.path().join(".bashrc"), "# user\n").unwrap();
        let id = Uuid::new_v4();
        let mut tx = planned(home.path(), id);
        assert!(tx
            .apply_with(|| {
                fs::write(home.path().join(".bashrc"), "# later edit\n")?;
                Err(io::Error::other("injected failure after a concurrent edit"))
            })
            .is_err());
        assert_eq!(
            fs::read(home.path().join(".bashrc")).unwrap(),
            b"# later edit\n"
        );
        assert_eq!(
            outcome(home.path(), id).unwrap(),
            Some(State::NeedsResolution)
        );
        drop(tx);
        assert!(Transaction::open(home.path(), Uuid::new_v4()).is_err());
    }
    #[test]
    fn corrupt_backup_and_foreign_journal_are_preserved() {
        let home = tempfile::tempdir().unwrap();
        let tx = Transaction::open(home.path(), Uuid::new_v4()).unwrap();
        let snapshot = tx.file(b"original", 0o600).unwrap();
        let Snapshot::File { hash, .. } = &snapshot else {
            panic!()
        };
        fs::write(tx.root.join("objects").join(hash), b"corrupt").unwrap();
        assert!(tx.bytes(&snapshot).is_err());
        assert!(tx.file(b"original", 0o600).is_err());
        let root = tx.root.clone();
        drop(tx);
        let foreign = home.path().join("foreign");
        fs::write(&foreign, b"user file").unwrap();
        symlink(&foreign, root.join("journal.json")).unwrap();
        assert!(Transaction::open(home.path(), Uuid::new_v4()).is_err());
        assert_eq!(fs::read(&foreign).unwrap(), b"user file");
    }
    #[test]
    fn recovery_refuses_to_undo_a_later_owned_change() {
        let home = tempfile::tempdir().unwrap();
        let id = Uuid::new_v4();
        planned(home.path(), id).apply().unwrap();
        let path = home.path().join(".bashrc");
        let mut later = Transaction::open(home.path(), Uuid::new_v4()).unwrap();
        let before = later.read(&path).unwrap();
        let after = later.file(b"# newer setup\n", 0o600).unwrap();
        later.plan(&path, before, after).unwrap();
        later.apply().unwrap();
        assert!(Transaction::recover(home.path(), id).is_err());
        assert_eq!(fs::read(path).unwrap(), b"# newer setup\n");
    }
    #[test]
    fn completed_id_is_not_replayed() {
        let home = tempfile::tempdir().unwrap();
        let id = Uuid::new_v4();
        planned(home.path(), id).apply().unwrap();
        fs::write(home.path().join(".bashrc"), b"later edit").unwrap();
        assert!(Transaction::open(home.path(), id).is_err());
        assert_eq!(
            fs::read(home.path().join(".bashrc")).unwrap(),
            b"later edit"
        );
        assert_eq!(outcome(home.path(), id).unwrap(), Some(State::Applied));
    }
    #[test]
    #[ignore = "subprocess helper; run by killed_installer_leaves_durable_intent"]
    fn crash_child() {
        let home = PathBuf::from(std::env::var_os("NEEDLE_HELPER_TEST_HOME").unwrap());
        let id = Uuid::parse_str(&std::env::var("NEEDLE_HELPER_TEST_ID").unwrap()).unwrap();
        planned(&home, id)
            .apply_with(|| {
                fs::write(home.join("ready"), b"ready")?;
                std::thread::sleep(std::time::Duration::from_secs(30));
                Ok(())
            })
            .unwrap();
    }
    #[test]
    fn killed_installer_leaves_durable_intent() {
        let home = tempfile::tempdir().unwrap();
        fs::write(home.path().join(".bashrc"), b"# user\n").unwrap();
        let id = Uuid::new_v4();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "helper_transaction::tests::crash_child",
                "--ignored",
            ])
            .env("NEEDLE_HELPER_TEST_HOME", home.path())
            .env("NEEDLE_HELPER_TEST_ID", id.to_string())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !home.path().join("ready").exists() && std::time::Instant::now() < deadline {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let ready = home.path().join("ready").exists();
        let _ = child.kill();
        child.wait().unwrap();
        assert!(ready, "child did not reach the committed write");
        assert_eq!(outcome(home.path(), id).unwrap(), Some(State::Prepared));
        assert_eq!(
            fs::read(home.path().join(".bashrc")).unwrap(),
            b"# user\n# managed\n"
        );
        assert!(fs::symlink_metadata(home.path().join("alias")).is_err());
        assert!(Transaction::open(home.path(), Uuid::new_v4()).is_err());
        let journal =
            Journal::load(&home.path().join(".local/share/newport/needle/helpers")).unwrap();
        assert_eq!(journal.records[0].changes.len(), 2);
        Transaction::recover(home.path(), id).unwrap();
        assert_eq!(outcome(home.path(), id).unwrap(), Some(State::RolledBack));
        assert_eq!(fs::read(home.path().join(".bashrc")).unwrap(), b"# user\n");
        Transaction::recover(home.path(), id).unwrap();
        assert!(Transaction::open(home.path(), Uuid::new_v4()).is_ok());
    }
}
