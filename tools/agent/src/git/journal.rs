//! Durable operation records and cross-process repository coordination.
//! A running record without its repository lock is uncertain, never retryable.
use super::protocol::Error;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};
use uuid::Uuid;
const MAX_RECORD: usize = 64 * 1024;
const MAX_RECORDS: usize = 10_000;
const MAX_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Record {
    pub operation_id: String,
    pub repository: String,
    pub payload_hash: String,
    pub state: String,
    pub seq: u32,
    pub result: Option<Value>,
    pub error: Option<Error>,
}
#[derive(Clone)]
pub struct Journal {
    root: PathBuf,
    client_id: String,
}
pub struct RepositoryLock {
    _file: File,
}
fn failure(_: impl std::fmt::Display) -> Error {
    Error::new(
        "JOURNAL_UNAVAILABLE",
        "Operation records could not be safely read or saved.",
    )
}
pub fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn uuid(value: &str) -> Result<(), Error> {
    Uuid::parse_str(value)
        .map(|_| ())
        .map_err(|_| Error::invalid("Operation and client IDs must be UUIDs."))
}
fn private_directory(path: &Path) -> Result<(), Error> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(failure)?;
    let meta = fs::symlink_metadata(path).map_err(failure)?;
    if !meta.is_dir() || meta.permissions().mode() & 0o077 != 0 {
        return Err(failure("unsafe journal directory"));
    }
    Ok(())
}
impl Journal {
    pub fn open(root: PathBuf, client_id: String) -> Result<Self, Error> {
        uuid(&client_id)?;
        private_directory(&root)?;
        private_directory(&root.join("records"))?;
        private_directory(&root.join("locks"))?;
        Ok(Self { root, client_id })
    }
    fn record_path(&self, id: &str) -> Result<PathBuf, Error> {
        uuid(id)?;
        Ok(self
            .root
            .join("records")
            .join(format!("{}-{id}.json", self.client_id)))
    }
    fn lock(&self, name: &str) -> Result<RepositoryLock, Error> {
        let path = self.root.join("locks").join(name);
        // Journal directories are private. Never replace a pre-existing lock inode.
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => {
                file.set_permissions(fs::Permissions::from_mode(0o600))
                    .map_err(failure)?;
                file
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let meta = fs::symlink_metadata(&path).map_err(failure)?;
                if !meta.is_file() || meta.permissions().mode() & 0o077 != 0 {
                    return Err(failure("unsafe journal lock"));
                }
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(path)
                    .map_err(failure)?
            }
            Err(e) => return Err(failure(e)),
        };
        file.try_lock_exclusive()
            .map_err(|_| Error::new("REPOSITORY_BUSY", "Another Git operation is running."))?;
        Ok(RepositoryLock { _file: file })
    }
    pub fn lock_repository(&self, repository: &str) -> Result<RepositoryLock, Error> {
        self.lock(&format!("{}.lock", hash(repository.as_bytes())))
    }
    fn load(&self, path: &Path) -> Result<Option<Record>, Error> {
        let meta = match fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(failure(e)),
        };
        if !meta.is_file()
            || meta.len() > MAX_RECORD as u64
            || meta.permissions().mode() & 0o077 != 0
        {
            return Err(failure("invalid record"));
        }
        let mut data = Vec::new();
        File::open(path)
            .map_err(failure)?
            .take(MAX_RECORD as u64 + 1)
            .read_to_end(&mut data)
            .map_err(failure)?;
        if data.len() > MAX_RECORD {
            return Err(failure("oversized record"));
        }
        serde_json::from_slice(&data).map(Some).map_err(failure)
    }
    pub fn existing(&self, id: &str, payload_hash: &str) -> Result<Option<Record>, Error> {
        let result = self.load(&self.record_path(id)?)?;
        if result
            .as_ref()
            .is_some_and(|r| r.payload_hash != payload_hash)
        {
            return Err(Error::new(
                "OPERATION_ID_REUSED",
                "This operation ID already belongs to a different request.",
            ));
        }
        Ok(result)
    }
    pub fn get(&self, id: &str) -> Result<Record, Error> {
        let mut record = self.load(&self.record_path(id)?)?.ok_or_else(|| {
            Error::new(
                "OPERATION_NOT_FOUND",
                "No operation record exists. Do not automatically replay a write.",
            )
        })?;
        if record.state == "running" {
            match self.lock_repository(&record.repository) {
                Ok(_guard) => {
                    // Re-read after acquiring the lock: its owner may just have finished.
                    record = self
                        .load(&self.record_path(id)?)?
                        .ok_or_else(|| failure("missing record"))?;
                    if record.state == "running" {
                        record.state = "outcome_unknown".into();
                        record.seq += 1;
                        record.error=Some(Error::new("OUTCOME_UNKNOWN","The operation was interrupted. Inspect repository state before continuing."));
                        self.save(&record)?;
                    }
                }
                Err(e) if e.code == "REPOSITORY_BUSY" => {}
                Err(e) => return Err(e),
            }
        }
        Ok(record)
    }
    /// Caller holds the common-repository lock. Unknown outcomes block new writes.
    pub fn begin(
        &self,
        id: &str,
        payload_hash: String,
        repository: String,
    ) -> Result<Record, Error> {
        let _catalog = self.lock("catalog.lock")?;
        if let Some(record) = self.existing(id, &payload_hash)? {
            return Ok(record);
        }
        for path in self.record_paths()? {
            if let Some(record) = self.load(&path)? {
                if record.repository == repository
                    && matches!(record.state.as_str(), "running" | "outcome_unknown")
                {
                    return Err(Error::new(
                        "RECOVERY_REQUIRED",
                        format!(
                            "Inspect interrupted operation {} before another write.",
                            record.operation_id
                        ),
                    ));
                }
            }
        }
        let record = Record {
            operation_id: id.into(),
            repository,
            payload_hash,
            state: "running".into(),
            seq: 1,
            result: None,
            error: None,
        };
        self.persist(&record)?;
        Ok(record)
    }
    fn record_paths(&self) -> Result<Vec<PathBuf>, Error> {
        let mut paths = Vec::new();
        for entry in fs::read_dir(self.root.join("records")).map_err(failure)? {
            let path = entry.map_err(failure)?.path();
            if path.extension().is_some_and(|e| e == "json") {
                paths.push(path);
                if paths.len() > MAX_RECORDS {
                    return Err(Error::new("JOURNAL_FULL", "Operation journal is full."));
                }
            }
        }
        Ok(paths)
    }
    pub fn save(&self, record: &Record) -> Result<(), Error> {
        let _catalog = self.lock("catalog.lock")?;
        self.persist(record)
    }
    fn persist(&self, record: &Record) -> Result<(), Error> {
        let path = self.record_path(&record.operation_id)?;
        let data = serde_json::to_vec(record).map_err(failure)?;
        if data.len() > MAX_RECORD {
            return Err(failure("record too large"));
        }
        let paths = self.record_paths()?;
        let size = paths
            .iter()
            .filter(|p| **p != path)
            .try_fold(0u64, |sum, p| {
                fs::symlink_metadata(p).map(|m| sum + m.len())
            })
            .map_err(failure)?;
        if (paths.len() >= MAX_RECORDS && !path.exists()) || size + data.len() as u64 > MAX_BYTES {
            return Err(Error::new(
                "JOURNAL_FULL",
                "Operation journal is full. No new writes can be accepted.",
            ));
        }
        let mut temporary =
            tempfile::NamedTempFile::new_in(self.root.join("records")).map_err(failure)?;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(failure)?;
        temporary.write_all(&data).map_err(failure)?;
        temporary.as_file().sync_all().map_err(failure)?;
        temporary.persist(path).map_err(failure)?;
        File::open(self.root.join("records"))
            .map_err(failure)?
            .sync_all()
            .map_err(failure)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn records_survive_reconnect_and_interruption_is_not_replayed() {
        let temp = tempfile::tempdir().unwrap();
        let client = Uuid::new_v4().to_string();
        let id = Uuid::new_v4().to_string();
        let journal = Journal::open(temp.path().join("journal"), client.clone()).unwrap();
        let guard = journal.lock_repository("repo").unwrap();
        journal.begin(&id, "hash".into(), "repo".into()).unwrap();
        assert_eq!(journal.get(&id).unwrap().state, "running");
        drop(guard);
        drop(journal);
        let journal = Journal::open(temp.path().join("journal"), client).unwrap();
        assert_eq!(journal.get(&id).unwrap().state, "outcome_unknown");
        assert_eq!(
            journal.existing(&id, "other").unwrap_err().code,
            "OPERATION_ID_REUSED"
        );
        let _guard = journal.lock_repository("repo").unwrap();
        assert_eq!(
            journal
                .begin(&Uuid::new_v4().to_string(), "new".into(), "repo".into())
                .unwrap_err()
                .code,
            "RECOVERY_REQUIRED"
        );
    }
    #[test]
    fn repository_lock_is_shared_across_clients() {
        let temp = tempfile::tempdir().unwrap();
        let a = Journal::open(temp.path().join("journal"), Uuid::new_v4().to_string()).unwrap();
        let b = Journal::open(temp.path().join("journal"), Uuid::new_v4().to_string()).unwrap();
        let guard = a.lock_repository("common-repo").unwrap();
        assert!(b.lock_repository("common-repo").is_err());
        assert!(b.lock_repository("other-repo").is_ok());
        drop(guard);
        assert!(b.lock_repository("common-repo").is_ok());
    }
}
