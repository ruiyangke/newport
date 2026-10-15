//! Durable intent plus atomic directory exchange. An error after an exchange
//! does not imply it was undone: reopen/reconcile the SAME journal to recover.
use super::{PreparedBundle, bundle_version, validate_candidate};
use serde::{Deserialize, Serialize};
use std::{
    ffi::CString,
    fs::{self, File},
    io::{Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

const JOURNAL: &str = "transaction.json";
const JOURNAL_TEMP: &str = "transaction.next";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryState {
    /// The previous app is installed and the candidate remains staged.
    Previous,
    /// The candidate is installed and the previous app is retained as backup.
    Updated,
}

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum Intent {
    Install,
    Restore,
    AcceptUpdated,
    AcceptPrevious,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema: u32,
    destination: PathBuf,
    previous: String,
    updated: String,
    team: String,
    intent: Intent,
}

/// Holds an OS lock shared by all transactions for this installation. Backups
/// and the journal intentionally survive Drop and process exit.
pub struct Transaction {
    root: PathBuf,
    record: Record,
    _lock: InstallationLock,
}

/// Hold discovery and profile reservation under the same installation lock.
/// Even a clean installation keeps the lock until the caller has reserved its
/// profile, closing the gap between "no journal" and application startup.
pub struct StartupGuard {
    transaction: Option<Transaction>,
    _lock: Option<InstallationLock>,
}
impl StartupGuard {
    pub fn acquire(installed: &Path) -> Result<Self, String> {
        let destination = installation_path(installed)?;
        let lock = installation_lock(&destination)?;
        match pending_record(&destination)? {
            Some((root, record)) => Ok(Self {
                transaction: Some(Transaction {
                    root,
                    record,
                    _lock: lock,
                }),
                _lock: None,
            }),
            None => Ok(Self {
                transaction: None,
                _lock: Some(lock),
            }),
        }
    }

    /// Consume only after profile ownership has been reserved. A pending
    /// transaction retains the lock; a clean startup releases it here.
    pub fn into_transaction(self) -> Option<Transaction> {
        self.transaction
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryStatus {
    pub installed: semver::Version,
    pub intended: semver::Version,
    pub accepted: bool,
}

struct InstallationLock(File);
impl Drop for InstallationLock {
    fn drop(&mut self) {
        // Explicitly release the shared flock description: a concurrently
        // forked child may briefly retain the FD until its close-on-exec runs.
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

impl Transaction {
    pub(super) fn begin(prepared: PreparedBundle) -> Result<Self, String> {
        let lock = installation_lock(&prepared.destination)?;
        ensure_no_pending(&prepared.destination)?;
        prepared.revalidate()?;
        sync_tree(&prepared.candidate())?;
        let record = Record {
            schema: 1,
            destination: prepared.destination.clone(),
            previous: prepared.installed_version.to_string(),
            updated: prepared.expected_version.to_string(),
            team: prepared.team.clone(),
            intent: Intent::Install,
        };
        write_record(prepared.staging.path(), &record)?;
        crate::filesystem::sync_directory(prepared.destination.parent().unwrap())
            .map_err(|error| error.to_string())?;
        Ok(Self {
            root: prepared.staging.keep(),
            record,
            _lock: lock,
        })
    }

    /// Open a known recovery directory beside this installed app. The expected
    /// installation path comes from the host, never from the journal alone.
    pub fn open(installed: &Path, root: &Path) -> Result<Self, String> {
        if !root.is_absolute() {
            return Err("Update recovery requires absolute paths.".into());
        }
        let destination = installation_path(installed)?;
        let lock = installation_lock(&destination)?;
        private_directory(root)?;
        let root = root.canonicalize().map_err(|error| error.to_string())?;
        if root.parent() != destination.parent()
            || !root
                .file_name()
                .and_then(|part| part.to_str())
                .is_some_and(|name| name.starts_with(".newport-update-"))
        {
            return Err("The recovery directory does not belong beside this installation.".into());
        }
        let (_, record) = read_record(&root)?;
        validate_record(&record, &destination)?;
        Ok(Self {
            root,
            record,
            _lock: lock,
        })
    }

    /// Find an interrupted transaction without requiring a remembered staging
    /// path. Discovery only reads journals; it never exchanges bundles, resumes
    /// installation, or deletes backups. The host must decide when to recover,
    /// after draining services. Ambiguous or damaged journals require attention
    /// instead of silently starting another update.
    pub fn discover(installed: &Path) -> Result<Option<Self>, String> {
        Ok(StartupGuard::acquire(installed)?.into_transaction())
    }

    pub fn recovery_directory(&self) -> &Path {
        &self.root
    }
    pub fn destination(&self) -> &Path {
        &self.record.destination
    }

    /// Validate the actual installation and recorded intent without exchanging
    /// either bundle. Accepted cleanup tolerates a partially deleted backup.
    pub fn inspect(&mut self) -> Result<RecoveryStatus, String> {
        self.inspect_with(validate_candidate)
    }

    fn inspect_with(
        &mut self,
        verify: impl Fn(&Path, &semver::Version, &str) -> Result<(), String>,
    ) -> Result<RecoveryStatus, String> {
        self.reload_intent()?;
        let intended = super::super::version(if self.desired_state() == RecoveryState::Updated {
            &self.record.updated
        } else {
            &self.record.previous
        })?;
        let accepted = matches!(
            self.record.intent,
            Intent::AcceptUpdated | Intent::AcceptPrevious
        );
        let installed = if accepted {
            verify(self.destination(), &intended, &self.record.team)?;
            intended.clone()
        } else {
            let actual = self.state_with(verify)?;
            super::super::version(if actual == RecoveryState::Updated {
                &self.record.updated
            } else {
                &self.record.previous
            })?
        };
        Ok(RecoveryStatus {
            installed,
            intended,
            accepted,
        })
    }

    /// Idempotently realize the recorded intent. The same directory exchange is
    /// NEVER blindly replayed: the live/backup versions determine whether it has
    /// already happened. Unknown or invalid bundles stop recovery without writes.
    pub fn reconcile(&mut self) -> Result<RecoveryState, String> {
        self.reconcile_with(validate_candidate)
    }

    /// Persist rollback intent BEFORE exchanging directories, so interrupted
    /// rollback resumes toward the previous version instead of reinstalling.
    pub fn restore(&mut self) -> Result<RecoveryState, String> {
        self.reload_intent()?;
        if matches!(
            self.record.intent,
            Intent::AcceptUpdated | Intent::AcceptPrevious
        ) {
            return Err("This update has already been accepted.".into());
        }
        self.state_with(validate_candidate)?;
        self.set_intent(Intent::Restore)?;
        self.reconcile()
    }

    /// Call only after the host has verified the relaunched app/backend is
    /// healthy. Removes the other bundle; until then rollback stays available.
    pub fn accept(self, running_version: &semver::Version) -> Result<(), String> {
        self.accept_with(running_version, validate_candidate)
    }

    fn accept_with(
        mut self,
        running_version: &semver::Version,
        verify: impl Fn(&Path, &semver::Version, &str) -> Result<(), String>,
    ) -> Result<(), String> {
        self.reload_intent()?;
        let state = self.desired_state();
        let expected = super::super::version(if state == RecoveryState::Updated {
            &self.record.updated
        } else {
            &self.record.previous
        })?;
        if running_version != &expected {
            return Err("Only the running accepted version can discard its backup.".into());
        }
        if matches!(
            self.record.intent,
            Intent::AcceptUpdated | Intent::AcceptPrevious
        ) {
            verify(self.destination(), &expected, &self.record.team)?;
        } else if self.state_with(verify)? != state {
            return Err(
                "The requested replacement has not completed; the backup was preserved.".into(),
            );
        }
        let intent = match state {
            RecoveryState::Updated => Intent::AcceptUpdated,
            RecoveryState::Previous => Intent::AcceptPrevious,
        };
        self.set_intent(intent)?;
        self.cleanup()
    }

    fn other(&self) -> PathBuf {
        self.root.join("Newport.app")
    }

    fn state_with(
        &self,
        verify: impl Fn(&Path, &semver::Version, &str) -> Result<(), String>,
    ) -> Result<RecoveryState, String> {
        let live = bundle_version(self.destination())?;
        let other = bundle_version(&self.other())?;
        let previous = super::super::version(&self.record.previous)?;
        let updated = super::super::version(&self.record.updated)?;
        let state = if live == previous && other == updated {
            RecoveryState::Previous
        } else if live == updated && other == previous {
            RecoveryState::Updated
        } else {
            return Err(
                "The installed and recovery bundles no longer match this transaction.".into(),
            );
        };
        verify(self.destination(), &live, &self.record.team)?;
        verify(&self.other(), &other, &self.record.team)?;
        Ok(state)
    }

    fn reload_intent(&mut self) -> Result<(), String> {
        // A record rename can succeed even if the subsequent directory flush
        // reports an error. Reload it rather than acting on stale in-memory
        // intent, and flush that intent before changing either bundle.
        private_directory(&self.root)?;
        let (journal, record) = read_record(&self.root)?;
        if record.schema != self.record.schema
            || record.destination != self.record.destination
            || record.previous != self.record.previous
            || record.updated != self.record.updated
            || record.team != self.record.team
        {
            return Err("The update journal changed unexpectedly.".into());
        }
        self.record = record;
        journal.sync_all().map_err(|error| error.to_string())?;
        crate::filesystem::sync_directory(&self.root).map_err(|error| error.to_string())?;
        Ok(())
    }

    fn desired_state(&self) -> RecoveryState {
        match self.record.intent {
            Intent::Install | Intent::AcceptUpdated => RecoveryState::Updated,
            Intent::Restore | Intent::AcceptPrevious => RecoveryState::Previous,
        }
    }

    fn reconcile_with(
        &mut self,
        verify: impl Fn(&Path, &semver::Version, &str) -> Result<(), String>,
    ) -> Result<RecoveryState, String> {
        self.reload_intent()?;
        let desired = self.desired_state();
        if matches!(
            self.record.intent,
            Intent::AcceptUpdated | Intent::AcceptPrevious
        ) {
            let version = super::super::version(if desired == RecoveryState::Updated {
                &self.record.updated
            } else {
                &self.record.previous
            })?;
            verify(self.destination(), &version, &self.record.team)?;
            return Ok(desired);
        }
        let state = self.state_with(&verify)?;
        if state != desired {
            exchange(self.destination(), &self.other())?;
        }
        // Also flush on a retry after an exchange succeeded but syncing failed.
        crate::filesystem::sync_directory(&self.root).map_err(|error| error.to_string())?;
        crate::filesystem::sync_directory(self.destination().parent().unwrap())
            .map_err(|error| error.to_string())?;
        Ok(desired)
    }

    fn set_intent(&mut self, intent: Intent) -> Result<(), String> {
        // Update memory only after replacement of the durable record succeeds.
        let record = Record {
            intent,
            schema: self.record.schema,
            destination: self.record.destination.clone(),
            previous: self.record.previous.clone(),
            updated: self.record.updated.clone(),
            team: self.record.team.clone(),
        };
        write_record(&self.root, &record)?;
        self.record = record;
        Ok(())
    }

    fn cleanup(&self) -> Result<(), String> {
        // Preserve the accepted journal until the backup is fully removed. A
        // crash partway through removal can resume without validating a partial
        // backup, but always validates the accepted live bundle first.
        if self.other().exists() {
            fs::remove_dir_all(self.other()).map_err(|error| error.to_string())?;
        }
        for name in [JOURNAL_TEMP, JOURNAL] {
            match fs::remove_file(self.root.join(name)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        fs::remove_dir(&self.root).map_err(|error| error.to_string())?;
        crate::filesystem::sync_directory(self.destination().parent().unwrap())
            .map_err(|error| error.to_string())
    }
}

fn installation_path(installed: &Path) -> Result<PathBuf, String> {
    if !installed.is_absolute()
        || installed.extension().and_then(|part| part.to_str()) != Some("app")
        || !fs::symlink_metadata(installed)
            .map_err(|error| error.to_string())?
            .is_dir()
    {
        return Err("Update recovery requires an absolute, non-symlink app directory.".into());
    }
    installed.canonicalize().map_err(|error| error.to_string())
}

fn read_record(root: &Path) -> Result<(File, Record), String> {
    let mut file = private_file(&root.join(JOURNAL), false)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > 64 * 1024 {
        return Err("The update journal exceeds its size limit.".into());
    }
    let record = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    Ok((file, record))
}

fn validate_record(record: &Record, destination: &Path) -> Result<(), String> {
    if record.schema != 1
        || record.destination != destination
        || record.team.is_empty()
        || super::super::version(&record.updated)?
            .cmp_precedence(&super::super::version(&record.previous)?)
            != std::cmp::Ordering::Greater
    {
        return Err("The update journal does not match this installation.".into());
    }
    Ok(())
}

// Called only while holding the installation lock. Staging directories without
// a journal have never begun installation and must not be treated as recovery.
fn pending_record(destination: &Path) -> Result<Option<(PathBuf, Record)>, String> {
    let parent = destination.parent().ok_or("Missing application parent.")?;
    let mut pending = None;
    for entry in fs::read_dir(parent).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if !entry
            .file_name()
            .as_bytes()
            .starts_with(b".newport-update-")
        {
            continue;
        }
        let root = entry.path();
        let metadata = match fs::symlink_metadata(&root) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
        };
        // Other users can have their own installations in the same directory.
        // SAFETY: geteuid has no arguments or preconditions.
        if metadata.uid() != unsafe { libc::geteuid() } {
            continue;
        }
        private_directory(&root)?;
        match fs::symlink_metadata(root.join(JOURNAL)) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
        }
        let (_, record) = read_record(&root)?;
        if record.destination != destination {
            continue;
        }
        validate_record(&record, destination)?;
        if pending.is_some() {
            return Err(
                "Multiple update journals refer to this installation; backups were preserved."
                    .into(),
            );
        }
        pending = Some((root, record));
    }
    Ok(pending)
}

fn ensure_no_pending(destination: &Path) -> Result<(), String> {
    if pending_record(destination)?.is_some() {
        return Err(
            "An earlier update still needs recovery or acceptance before another can begin.".into(),
        );
    }
    Ok(())
}

fn private_file(path: &Path, create: bool) -> Result<File, String> {
    let file = crate::filesystem::private_file_options()
        .read(true)
        .write(true)
        .create(create)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| error.to_string())?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    // SAFETY: geteuid has no arguments or preconditions.
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
    {
        return Err("Update files must be private regular files owned by this user.".into());
    }
    Ok(file)
}
fn private_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    // SAFETY: geteuid has no arguments or preconditions.
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err("The update recovery directory is not private to this user.".into());
    }
    Ok(())
}
fn installation_lock(destination: &Path) -> Result<InstallationLock, String> {
    let name = destination
        .file_name()
        .and_then(|part| part.to_str())
        .ok_or("Invalid application path.")?;
    let path = destination
        .parent()
        .ok_or("Missing application parent.")?
        .join(format!(".{name}.newport-update.lock"));
    let file = private_file(&path, true)?;
    fs2::FileExt::try_lock_exclusive(&file)
        .map_err(|_| "Another update owns this installation.".to_owned())?;
    Ok(InstallationLock(file))
}
fn write_record(root: &Path, record: &Record) -> Result<(), String> {
    let temporary = root.join(JOURNAL_TEMP);
    let mut file = private_file(&temporary, true)?;
    file.set_len(0).map_err(|error| error.to_string())?;
    file.write_all(&serde_json::to_vec(record).map_err(|error| error.to_string())?)
        .and_then(|_| file.sync_all())
        .map_err(|error| error.to_string())?;
    fs::rename(temporary, root.join(JOURNAL)).map_err(|error| error.to_string())?;
    crate::filesystem::sync_directory(root).map_err(|error| error.to_string())
}
fn exchange(left: &Path, right: &Path) -> Result<(), String> {
    let left = CString::new(left.as_os_str().as_bytes()).map_err(|error| error.to_string())?;
    let right = CString::new(right.as_os_str().as_bytes()).map_err(|error| error.to_string())?;
    // SAFETY: both strings are valid NUL-terminated paths for this synchronous
    // call. RENAME_SWAP exchanges existing directories atomically on macOS.
    let result = unsafe {
        libc::renameatx_np(
            libc::AT_FDCWD,
            left.as_ptr(),
            libc::AT_FDCWD,
            right.as_ptr(),
            libc::RENAME_SWAP,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}
fn sync_tree(root: &Path) -> Result<(), String> {
    let mut directories = vec![root.to_owned()];
    let mut index = 0;
    while index < directories.len() {
        for entry in fs::read_dir(&directories[index]).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            if kind.is_dir() {
                directories.push(entry.path());
            } else if kind.is_file() {
                File::open(entry.path())
                    .and_then(|file| file.sync_all())
                    .map_err(|error| error.to_string())?;
            }
        }
        index += 1;
    }
    for directory in directories.into_iter().rev() {
        crate::filesystem::sync_directory(&directory).map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
