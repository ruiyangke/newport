//! Exact staging paths journaled before remote creation and retired after
//! publication/removal. Never scans remote directories or deletes by prefix.
//! Recovery uses authorized SFTP connections, never background authentication.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, path::PathBuf};
use uuid::Uuid;

#[derive(Default)]
pub(super) struct UploadCleanup {
    entries: HashMap<Uuid, Entry>,
    journal: Option<PathBuf>,
    load_error: Option<String>,
}
#[derive(Serialize, Deserialize)]
struct Entry {
    scope: Value,
    path: String,
    #[serde(skip)]
    active: bool,
    #[serde(skip)]
    running: bool,
}
#[derive(Serialize, Deserialize)]
struct Journal {
    version: u32,
    entries: HashMap<Uuid, Entry>,
}
impl UploadCleanup {
    fn decode(bytes: &[u8], server: Option<Uuid>) -> Result<Journal> {
        let journal: Journal = serde_json::from_slice(bytes)?;
        if journal.version != 1 {
            bail!("Unsupported upload recovery version");
        }
        for (id, entry) in &journal.entries {
            if let Some(server) = server
                && super::recovery_storage::scope_server(&entry.scope)? != server
            {
                bail!("Upload recovery record belongs to another server");
            }
            if staging_id(&entry.path)? != *id {
                bail!("Invalid upload recovery identity");
            }
        }
        Ok(journal)
    }
    pub(super) fn validate(bytes: &[u8], server: Option<Uuid>) -> Result<()> {
        Self::decode(bytes, server).map(|_| ())
    }
    pub fn open(path: PathBuf) -> Self {
        let mut queue = Self {
            journal: Some(path.clone()),
            ..Self::default()
        };
        let loaded = (|| -> Result<_> {
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(HashMap::new());
                }
                Err(error) => return Err(error.into()),
            };
            let journal = Self::decode(&bytes, None)?;
            Ok(journal.entries)
        })();
        match loaded {
            Ok(entries) => queue.entries = entries,
            // Preserve unreadable records. Browsing still works, but a new
            // upload must not overwrite recovery state it cannot understand.
            Err(error) => {
                queue.load_error = Some(format!("Cannot load upload recovery state: {error}"))
            }
        }
        queue
    }

    fn persist(&self) -> Result<()> {
        if let Some(error) = &self.load_error {
            bail!("{error}");
        }
        let Some(path) = &self.journal else {
            return Ok(());
        };
        let parent = path
            .parent()
            .context("Upload recovery directory is missing")?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer(
            file.as_file_mut(),
            &serde_json::json!({"version":1,"entries":self.entries}),
        )?;
        file.as_file().sync_all()?;
        file.persist(path).map_err(|error| error.error)?;
        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    }

    /// Journal before issuing remote OPEN. Active entries cannot be cleaned by
    /// another file connection. Restart restores them as abandoned entries.
    pub fn register(&mut self, scope: Value, path: String) -> Result<Uuid> {
        let id = staging_id(&path)?;
        if self.entries.contains_key(&id) {
            bail!("Upload recovery identity already exists");
        }
        self.entries.insert(
            id,
            Entry {
                scope,
                path,
                active: true,
                running: false,
            },
        );
        if let Err(error) = self.persist() {
            self.entries.remove(&id);
            return Err(error.context("Cannot record upload recovery state"));
        }
        Ok(id)
    }

    pub fn release(&mut self, id: Uuid) {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.active = false;
        }
    }

    pub fn take(&mut self, scope: &Value) -> Vec<(Uuid, String)> {
        self.entries
            .iter_mut()
            .filter(|(_, entry)| !entry.active && !entry.running && entry.scope == *scope)
            .take(8)
            .map(|(id, entry)| {
                entry.running = true;
                (*id, entry.path.clone())
            })
            .collect()
    }

    pub fn finish(&mut self, id: Uuid, removed: bool) {
        if removed {
            if let Some(mut entry) = self.entries.remove(&id)
                && let Err(error) = self.persist()
            {
                entry.active = false;
                entry.running = false;
                self.entries.insert(id, entry);
                log::warn!("Could not retire upload recovery record: {error}");
            }
        } else if let Some(entry) = self.entries.get_mut(&id) {
            entry.running = false;
        }
    }
}

fn staging_id(path: &str) -> Result<Uuid> {
    if !path.starts_with('/')
        || path.contains('\0')
        || path.split('/').any(|part| part == ".." || part == ".")
    {
        bail!("Invalid upload staging path");
    }
    let name = path.rsplit('/').next().unwrap_or_default();
    let id = name
        .strip_prefix(".newport-upload-")
        .context("Not an upload staging path")?;
    let parsed = Uuid::parse_str(id)?;
    if parsed.to_string() != id {
        bail!("Invalid upload staging identity");
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn path() -> String {
        format!("/work/.newport-upload-{}", Uuid::new_v4())
    }

    #[test]
    fn crash_recovery_restores_active_and_inflight_entries_but_never_finished_uploads() {
        let temp = tempfile::tempdir().unwrap();
        let journal = temp.path().join("upload-recovery.json");
        let scope = json!(["server", "host", "user", 22]);
        let mut queue = UploadCleanup::open(journal.clone());
        let active_path = path();
        let active = queue.register(scope.clone(), active_path.clone()).unwrap();
        assert!(
            queue.take(&scope).is_empty(),
            "new connections must not remove live uploads"
        );
        let failed_path = path();
        let failed = queue.register(scope.clone(), failed_path.clone()).unwrap();
        queue.release(failed);
        assert_eq!(queue.take(&scope), vec![(failed, failed_path.clone())]);
        let completed = queue.register(scope.clone(), path()).unwrap();
        queue.finish(completed, true);
        drop(queue);
        let mut recovered = UploadCleanup::open(journal.clone());
        assert!(recovered.take(&json!(["other-server"])).is_empty());
        let mut pending = recovered.take(&scope);
        pending.sort();
        let mut expected = vec![(active, active_path), (failed, failed_path)];
        expected.sort();
        assert_eq!(pending, expected);
        for (id, _) in pending {
            recovered.finish(id, true);
        }
        assert!(UploadCleanup::open(journal).take(&scope).is_empty());
    }

    #[test]
    fn invalid_or_unwritable_journal_blocks_registration_without_overwriting_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let journal = temp.path().join("upload-recovery.json");
        for contents in [b"invalid".as_slice(), br#"{"version":2,"entries":{}}"#] {
            std::fs::write(&journal, contents).unwrap();
            let mut queue = UploadCleanup::open(journal.clone());
            assert!(queue.register(json!(["scope"]), path()).is_err());
            assert!(queue.take(&json!(["scope"])).is_empty());
            assert_eq!(std::fs::read(&journal).unwrap(), contents);
        }
        let mut queue = UploadCleanup::open(temp.path().join("missing/recovery.json"));
        assert!(queue.register(json!(["scope"]), path()).is_err());
        assert!(queue.entries.is_empty());
    }

    #[test]
    fn recovery_rejects_destination_paths_and_mismatched_identities() {
        let temp = tempfile::tempdir().unwrap();
        let journal = temp.path().join("upload-recovery.json");
        for bad in [
            "/work/payload.bin".to_owned(),
            format!("/work/../.newport-upload-{}", Uuid::new_v4()),
            path(),
        ] {
            let id = Uuid::new_v4();
            std::fs::write(
                &journal,
                serde_json::to_vec(
                    &json!({"version":1,"entries":{id.to_string():{"scope":["s"],"path":bad}}}),
                )
                .unwrap(),
            )
            .unwrap();
            let mut queue = UploadCleanup::open(journal.clone());
            assert!(queue.take(&json!(["s"])).is_empty());
            assert!(queue.register(json!(["s"]), path()).is_err());
        }
    }

    #[test]
    fn failed_retirement_remains_retryable_and_journal_is_private() {
        let temp = tempfile::tempdir().unwrap();
        let journal = temp.path().join("upload-recovery.json");
        let mut queue = UploadCleanup::open(journal.clone());
        let scope = json!(["scope"]);
        let path = path();
        let id = queue.register(scope.clone(), path.clone()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&journal).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let saved = temp.path().join("saved");
        std::fs::rename(&journal, &saved).unwrap();
        std::fs::create_dir(&journal).unwrap();
        queue.finish(id, true);
        assert_eq!(queue.take(&scope), vec![(id, path)]);
        std::fs::remove_dir(&journal).unwrap();
        std::fs::rename(saved, &journal).unwrap();
        queue.finish(id, true);
        assert!(UploadCleanup::open(journal).take(&scope).is_empty());
    }

    #[test]
    fn cleanup_is_scoped_exact_and_retries_only_failures() {
        let mut queue = UploadCleanup::default();
        let original = json!(["original", "host", "user", 22, "key"]);
        let other = json!(["original", "different-host", "user", 22, "key"]);
        let path = path();
        let id = queue.register(original.clone(), path.clone()).unwrap();
        queue.release(id);
        assert!(queue.take(&other).is_empty());
        let attempt = queue.take(&original);
        assert_eq!(attempt.len(), 1);
        assert_eq!(attempt[0].1, path);
        assert!(
            queue.take(&original).is_empty(),
            "concurrent retry must not duplicate work"
        );
        queue.finish(attempt[0].0, false);
        assert_eq!(queue.take(&original), attempt);
        queue.finish(attempt[0].0, true);
        assert!(queue.take(&original).is_empty());
        queue.finish(attempt[0].0, false);
        assert!(
            queue.take(&original).is_empty(),
            "late failure cannot resurrect removed work"
        );
    }

    #[test]
    fn retries_have_bounded_batches() {
        let mut queue = UploadCleanup::default();
        for _ in 0..30 {
            let id = queue.register(json!(["scope"]), path()).unwrap();
            queue.release(id);
        }
        assert_eq!(queue.take(&json!(["scope"])).len(), 8);
    }
}
