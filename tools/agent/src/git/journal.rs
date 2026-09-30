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
pub struct Record<T = Value> {
    pub operation_id: String,
    pub repository: String,
    pub payload_hash: String,
    pub state: String,
    pub seq: u32,
    pub result: Option<T>,
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
impl Drop for RepositoryLock {
    fn drop(&mut self) {
        // Explicitly release the shared open-file-description lock. Closing our
        // descriptor alone can leave it held by a concurrently forked child
        // until that child execs and closes its inherited descriptors.
        let _ = FileExt::unlock(&self._file);
    }
}
/// Catalog usage excluding the record being written. Only valid while the
/// caller holds catalog.lock; never cached across requests or processes.
#[derive(Default)]
struct Usage {
    count: usize,
    bytes: u64,
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
        self.load_sized(path)
            .map(|record| record.map(|(record, _)| record))
    }
    fn load_sized<T: serde::de::DeserializeOwned>(
        &self,
        path: &Path,
    ) -> Result<Option<(Record<T>, u64)>, Error> {
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
        serde_json::from_slice(&data)
            .map(|record| Some((record, data.len() as u64)))
            .map_err(failure)
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
    /// Acknowledge uncertainty without claiming success or replaying any Git action.
    pub fn review(&self, id: &str) -> Result<Record, Error> {
        let path = self.record_path(id)?;
        let missing = || {
            Error::new(
                "OPERATION_NOT_FOUND",
                "No operation record exists for this client.",
            )
        };
        let before = self.load(&path)?.ok_or_else(missing)?;
        let _guard = self.lock_repository(&before.repository)?;
        let mut record = self.load(&path)?.ok_or_else(missing)?;
        if record.operation_id != id || record.repository != before.repository {
            return Err(failure("operation identity changed"));
        }
        match record.state.as_str() {
            "reviewed_unknown" => return Ok(record),
            "outcome_unknown" => {}
            _ => {
                return Err(Error::new(
                    "OPERATION_NOT_REVIEWABLE",
                    "Only an unknown outcome can be reviewed.",
                ))
            }
        }
        record.state = "reviewed_unknown".into();
        record.seq = record
            .seq
            .checked_add(1)
            .ok_or_else(|| failure("sequence exhausted"))?;
        self.save(&record)?;
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
        let mut usage = Usage::default();
        for path in self.record_paths()? {
            // Recovery needs the envelope, not a materialized result tree for
            // every completed operation. Keep the same typed envelope and
            // parse past its result without allocating its nested contents.
            if let Some((record, bytes)) = self.load_sized::<serde::de::IgnoredAny>(&path)? {
                usage.count += 1;
                usage.bytes += bytes;
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
        let record: Record = Record {
            operation_id: id.into(),
            repository,
            payload_hash,
            state: "running".into(),
            seq: 1,
            result: None,
            error: None,
        };
        // The recovery scan already read every record under the catalog lock.
        // Reuse its usage totals instead of walking and statting them again.
        self.persist(&record, Some(usage))?;
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
        self.persist(record, None)
    }
    fn persist(&self, record: &Record, usage: Option<Usage>) -> Result<(), Error> {
        let path = self.record_path(&record.operation_id)?;
        let data = serde_json::to_vec(record).map_err(failure)?;
        if data.len() > MAX_RECORD {
            return Err(failure("record too large"));
        }
        let usage = match usage {
            Some(usage) => usage,
            None => {
                let mut usage = Usage::default();
                for other in self.record_paths()? {
                    if other != path {
                        usage.count += 1;
                        usage.bytes += fs::symlink_metadata(other).map_err(failure)?.len();
                    }
                }
                usage
            }
        };
        if usage.count >= MAX_RECORDS || usage.bytes + data.len() as u64 > MAX_BYTES {
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
    fn recovery_projection_keeps_envelope_validation_and_full_replay() {
        let temp = tempfile::tempdir().unwrap();
        let journal =
            Journal::open(temp.path().join("journal"), Uuid::new_v4().to_string()).unwrap();
        let id = Uuid::new_v4().to_string();
        let record: Record = Record {
            operation_id: id.clone(),
            repository: "repo".into(),
            payload_hash: "fixture".into(),
            state: "outcome_unknown".into(),
            seq: 2,
            result: Some(
                serde_json::json!({"entries": (0..128).map(|n| serde_json::json!({"n":n,"message":"saved result"})).collect::<Vec<_>>()}),
            ),
            error: Some(Error::new("OUTCOME_UNKNOWN", "partial result")),
        };
        journal.save(&record).unwrap();
        let _guard = journal.lock_repository("repo").unwrap();
        assert_eq!(
            journal
                .begin(&Uuid::new_v4().to_string(), "next".into(), "repo".into())
                .unwrap_err()
                .code,
            "RECOVERY_REQUIRED"
        );
        assert_eq!(
            journal.existing(&id, "fixture").unwrap().unwrap().result,
            record.result
        );
        let valid = serde_json::to_value(&record).unwrap();
        for (field, value) in [
            ("state", serde_json::json!(1)),
            ("seq", serde_json::json!("invalid")),
            ("repository", serde_json::json!(false)),
            ("unexpected", serde_json::json!(true)),
            ("error", serde_json::json!({"code":"missing fields"})),
        ] {
            let mut invalid = valid.clone();
            invalid[field] = value;
            let data = serde_json::to_vec(&invalid).unwrap();
            assert!(serde_json::from_slice::<Record>(&data).is_err());
            assert!(serde_json::from_slice::<Record<serde::de::IgnoredAny>>(&data).is_err());
        }
        let malformed = br#"{"operationId":"id","repository":"repo","payloadHash":"hash","state":"succeeded","seq":2,"result":[1,},"error":null}"#;
        assert!(serde_json::from_slice::<Record<serde::de::IgnoredAny>>(malformed).is_err());
    }

    #[test]
    fn recovery_scan_counts_record_padding_toward_byte_limit() {
        let temp = tempfile::tempdir().unwrap();
        let journal =
            Journal::open(temp.path().join("journal"), Uuid::new_v4().to_string()).unwrap();
        for n in 1..=MAX_BYTES / MAX_RECORD as u64 {
            let record: Record = Record {
                operation_id: Uuid::from_u128(n as u128).to_string(),
                repository: "other-repository".into(),
                payload_hash: "fixture".into(),
                state: "succeeded".into(),
                seq: 2,
                result: None,
                error: None,
            };
            let mut data = serde_json::to_vec(&record).unwrap();
            data.resize(MAX_RECORD, b' ');
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(journal.record_path(&record.operation_id).unwrap())
                .unwrap()
                .write_all(&data)
                .unwrap();
        }
        let _guard = journal.lock_repository("repo").unwrap();
        let id = Uuid::new_v4().to_string();
        assert_eq!(
            journal
                .begin(&id, "overflow".into(), "repo".into())
                .unwrap_err()
                .code,
            "JOURNAL_FULL"
        );
        assert!(journal.existing(&id, "overflow").unwrap().is_none());
    }

    #[test]
    fn recovery_scan_usage_preserves_catalog_capacity_and_completion() {
        let temp = tempfile::tempdir().unwrap();
        let journal =
            Journal::open(temp.path().join("journal"), Uuid::new_v4().to_string()).unwrap();
        // Seed completed receipts without quadratic setup through save().
        for n in 1..MAX_RECORDS {
            let record: Record = Record {
                operation_id: Uuid::from_u128(n as u128).to_string(),
                repository: "other-repository".into(),
                payload_hash: "fixture".into(),
                state: "succeeded".into(),
                seq: 2,
                result: None,
                error: None,
            };
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(journal.record_path(&record.operation_id).unwrap())
                .unwrap();
            // Pretty formatting must count toward usage too, not just the
            // compact JSON size of a deserialized receipt.
            let data = serde_json::to_vec_pretty(&record).unwrap();
            file.write_all(&data).unwrap();
            assert_eq!(
                journal
                    .load_sized::<Value>(&journal.record_path(&record.operation_id).unwrap())
                    .unwrap()
                    .unwrap()
                    .1,
                data.len() as u64
            );
        }
        let _guard = journal.lock_repository("repo").unwrap();
        let id = Uuid::new_v4().to_string();
        let mut last = journal.begin(&id, "last".into(), "repo".into()).unwrap();
        last.state = "succeeded".into();
        last.seq += 1;
        journal.save(&last).unwrap(); // Completing the last slot is allowed.
        assert_eq!(journal.get(&id).unwrap().state, "succeeded");
        assert_eq!(
            journal
                .begin(
                    &Uuid::new_v4().to_string(),
                    "overflow".into(),
                    "repo".into()
                )
                .unwrap_err()
                .code,
            "JOURNAL_FULL"
        );
        assert_eq!(journal.record_paths().unwrap().len(), MAX_RECORDS);
    }

    #[test]
    fn explicit_review_unlocks_only_unknown_and_preserves_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("journal");
        let client = Uuid::new_v4().to_string();
        let journal = Journal::open(root.clone(), client.clone()).unwrap();
        let other = Journal::open(root.clone(), Uuid::new_v4().to_string()).unwrap();
        let id = Uuid::new_v4().to_string();
        let next = Uuid::new_v4().to_string();
        let guard = journal.lock_repository("repo").unwrap();
        journal.begin(&id, "hash".into(), "repo".into()).unwrap();
        assert_eq!(journal.review(&id).unwrap_err().code, "REPOSITORY_BUSY");
        assert_eq!(other.review(&id).unwrap_err().code, "OPERATION_NOT_FOUND");
        drop(guard);
        assert_eq!(
            journal.review(&id).unwrap_err().code,
            "OPERATION_NOT_REVIEWABLE"
        );
        let mut record = journal.get(&id).unwrap();
        record.result = Some(serde_json::json!({"partial":true}));
        journal.save(&record).unwrap();
        {
            let _guard = journal.lock_repository("repo").unwrap();
            assert_eq!(
                journal
                    .begin(&next, "next".into(), "repo".into())
                    .unwrap_err()
                    .code,
                "RECOVERY_REQUIRED"
            );
        }
        {
            let _guard = journal.lock_repository("unrelated-repo").unwrap();
            let unrelated = journal
                .begin(
                    &Uuid::new_v4().to_string(),
                    "unrelated".into(),
                    "unrelated-repo".into(),
                )
                .unwrap();
            assert_eq!(unrelated.state, "running");
            assert_eq!(journal.get(&id).unwrap().state, "outcome_unknown");
        }
        let reviewed = journal.review(&id).unwrap();
        assert_eq!(reviewed.state, "reviewed_unknown");
        assert_eq!(reviewed.seq, record.seq + 1);
        assert_eq!(reviewed.result, record.result);
        assert_eq!(
            serde_json::to_value(&reviewed.error).unwrap(),
            serde_json::to_value(&record.error).unwrap()
        );
        assert_eq!(journal.review(&id).unwrap().seq, reviewed.seq);
        let reopened = Journal::open(root, client).unwrap();
        assert_eq!(reopened.get(&id).unwrap().state, "reviewed_unknown");
        let _guard = reopened.lock_repository("repo").unwrap();
        assert_eq!(
            reopened
                .begin(&id, "hash".into(), "repo".into())
                .unwrap()
                .state,
            "reviewed_unknown"
        );
        assert_eq!(
            reopened
                .begin(&next, "next".into(), "repo".into())
                .unwrap()
                .state,
            "running"
        );
        assert_eq!(
            reopened
                .review(&Uuid::new_v4().to_string())
                .unwrap_err()
                .code,
            "OPERATION_NOT_FOUND"
        );
    }
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
