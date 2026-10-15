//! Recovery of this profile's local download staging files. Records contain
//! exact paths and file identities; recovery never scans folders or publishes a
//! download. The selected destination is deliberately absent from the journal.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Default)]
pub(super) struct DownloadCleanup {
    entries: HashMap<Uuid, Entry>,
    journal: Option<PathBuf>,
    load_error: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    // Unlike PathBuf's serde representation, OsString preserves non-UTF8 names.
    path: OsString,
    identity: Identity,
}

#[derive(Serialize, Deserialize, PartialEq)]
struct Identity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    created: u64,
}
impl Identity {
    fn of(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            Self {
                created: metadata.creation_time(),
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Journal {
    version: u32,
    entries: HashMap<Uuid, Entry>,
}

fn valid_path(id: Uuid, path: &Path) -> bool {
    path.is_absolute()
        && !path.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
        && path.file_name().and_then(|name| name.to_str())
            == Some(format!(".newport-download-{id}").as_str())
}

impl DownloadCleanup {
    fn decode(bytes: &[u8]) -> Result<Journal> {
        let journal: Journal = serde_json::from_slice(bytes)?;
        if journal.version != 1
            || journal
                .entries
                .iter()
                .any(|(id, entry)| !valid_path(*id, Path::new(&entry.path)))
        {
            bail!("Invalid download recovery state");
        }
        Ok(journal)
    }
    pub(super) fn validate(bytes: &[u8]) -> Result<()> {
        Self::decode(bytes).map(|_| ())
    }
    pub fn open(journal: PathBuf) -> Self {
        let mut queue = Self {
            journal: Some(journal.clone()),
            ..Self::default()
        };
        let loaded = (|| -> Result<_> {
            let bytes = match fs::read(journal) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(HashMap::new());
                }
                Err(error) => return Err(error.into()),
            };
            let journal = Self::decode(&bytes)?;
            Ok(journal.entries)
        })();
        match loaded {
            Ok(entries) => {
                queue.entries = entries;
                // No live transfers exist when the profile is first opened.
                for id in queue.entries.keys().copied().collect::<Vec<_>>() {
                    if let Err(error) = queue.remove_staging(id) {
                        log::warn!("Could not recover download staging: {error}");
                    }
                }
            }
            Err(error) => {
                queue.load_error = Some(format!("Cannot load download recovery state: {error}"))
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
            .context("Download recovery directory is missing")?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer(
            file.as_file_mut(),
            &serde_json::json!({"version": 1, "entries": self.entries}),
        )?;
        file.as_file().sync_all()?;
        file.persist(path).map_err(|error| error.error)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    }

    /// The empty file is allocated first so recovery can verify its identity.
    /// No payload is written until this durable record succeeds.
    pub fn create(&mut self, parent: &Path) -> Result<(Uuid, tempfile::NamedTempFile)> {
        if let Some(error) = &self.load_error {
            bail!("{error}");
        }
        let parent = fs::canonicalize(parent)?;
        let id = Uuid::new_v4();
        let file = tempfile::Builder::new()
            .prefix(&format!(".newport-download-{id}"))
            .rand_bytes(0)
            .tempfile_in(parent)?;
        let entry = Entry {
            path: file.path().as_os_str().to_owned(),
            identity: Identity::of(&file.as_file().metadata()?),
        };
        self.entries.insert(id, entry);
        if let Err(error) = self.persist() {
            self.entries.remove(&id);
            return Err(error.context("Cannot record download recovery state"));
        }
        Ok((id, file))
    }

    pub fn finish(&mut self, id: Uuid) {
        if let Some(entry) = self.entries.remove(&id)
            && let Err(error) = self.persist()
        {
            self.entries.insert(id, entry);
            log::warn!("Could not retire download recovery record: {error}");
        }
    }

    fn remove_staging(&mut self, id: Uuid) -> Result<()> {
        let Some(entry) = self.entries.get(&id) else {
            return Ok(());
        };
        let path = Path::new(&entry.path);
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                // Refuse replacement files, links, and changed parent traversal.
                if !metadata.is_file()
                    || Identity::of(&metadata) != entry.identity
                    || fs::canonicalize(path)? != path
                {
                    bail!("Download staging path no longer identifies the original file");
                }
                fs::remove_file(path)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        self.finish(id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn restart_removes_only_recorded_staging_and_never_publishes_it() {
        let profile = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let journal = profile.path().join("download-recovery.json");
        let destination = folder.path().join("selected.txt");
        let unrelated = folder
            .path()
            .join(format!(".newport-download-{}", Uuid::new_v4()));
        fs::write(&destination, b"original").unwrap();
        fs::write(&unrelated, b"unrelated").unwrap();
        let mut queue = DownloadCleanup::open(journal.clone());
        let (_, mut file) = queue.create(folder.path()).unwrap();
        file.write_all(b"partial payload").unwrap();
        let (handle, staging) = file.keep().unwrap();
        drop(handle);
        drop(queue);
        assert!(staging.exists());
        let recovered = DownloadCleanup::open(journal);
        assert!(recovered.entries.is_empty());
        assert!(!staging.exists());
        assert_eq!(fs::read(destination).unwrap(), b"original");
        assert_eq!(fs::read(unrelated).unwrap(), b"unrelated");
    }

    #[test]
    fn completed_rename_with_unretired_record_never_deletes_the_destination() {
        let profile = tempfile::tempdir().unwrap();
        let journal = profile.path().join("download-recovery.json");
        let mut queue = DownloadCleanup::open(journal.clone());
        let (_, mut file) = queue.create(profile.path()).unwrap();
        file.write_all(b"complete").unwrap();
        let destination = profile.path().join("saved");
        file.persist(&destination).unwrap();
        drop(queue);
        assert!(DownloadCleanup::open(journal).entries.is_empty());
        assert_eq!(fs::read(destination).unwrap(), b"complete");
    }

    #[test]
    fn invalid_record_cannot_target_the_selected_destination() {
        let profile = tempfile::tempdir().unwrap();
        let journal = profile.path().join("download-recovery.json");
        let destination = profile.path().join("saved.txt");
        fs::write(&destination, b"keep").unwrap();
        let id = Uuid::new_v4();
        let entry = Entry {
            path: destination.as_os_str().to_owned(),
            identity: Identity::of(&fs::metadata(&destination).unwrap()),
        };
        let bytes = serde_json::to_vec(&Journal {
            version: 1,
            entries: HashMap::from([(id, entry)]),
        })
        .unwrap();
        fs::write(&journal, &bytes).unwrap();
        let mut queue = DownloadCleanup::open(journal.clone());
        assert!(queue.create(profile.path()).is_err());
        assert_eq!(fs::read(destination).unwrap(), b"keep");
        assert_eq!(fs::read(journal).unwrap(), bytes);
    }

    #[test]
    fn failed_retirement_preserves_a_retryable_record() {
        let profile = tempfile::tempdir().unwrap();
        let journal = profile.path().join("download-recovery.json");
        let mut queue = DownloadCleanup::open(journal.clone());
        let (id, file) = queue.create(profile.path()).unwrap();
        file.close().unwrap();
        let original = fs::read(&journal).unwrap();
        fs::remove_file(&journal).unwrap();
        fs::create_dir(&journal).unwrap();
        queue.finish(id);
        assert!(queue.entries.contains_key(&id));
        fs::remove_dir(&journal).unwrap();
        fs::write(&journal, original).unwrap();
        assert!(DownloadCleanup::open(journal).entries.is_empty());
    }

    #[test]
    fn invalid_or_unwritable_journal_prevents_payload_and_preserves_evidence() {
        let profile = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let journal = profile.path().join("download-recovery.json");
        fs::write(&journal, b"invalid").unwrap();
        assert!(
            DownloadCleanup::open(journal.clone())
                .create(folder.path())
                .is_err()
        );
        assert_eq!(fs::read(journal).unwrap(), b"invalid");
        assert!(
            DownloadCleanup::open(profile.path().join("missing/journal"))
                .create(folder.path())
                .is_err()
        );
        assert_eq!(fs::read_dir(folder.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn recovery_preserves_replacement_files_and_links() {
        use std::os::unix::fs::symlink;
        for link in [false, true] {
            let profile = tempfile::tempdir().unwrap();
            let journal = profile.path().join("download-recovery.json");
            let mut queue = DownloadCleanup::open(journal.clone());
            let (_, file) = queue.create(profile.path()).unwrap();
            let (original, staging) = file.keep().unwrap();
            fs::remove_file(&staging).unwrap();
            let target = profile.path().join("keep");
            fs::write(&target, b"unrelated").unwrap();
            if link {
                symlink(&target, &staging).unwrap();
            } else {
                fs::write(&staging, b"replacement").unwrap();
            }
            // Keep the old inode allocated, eliminating inode reuse in this test.
            let recovered = DownloadCleanup::open(journal);
            assert_eq!(recovered.entries.len(), 1);
            assert_eq!(fs::read(&target).unwrap(), b"unrelated");
            assert!(staging.exists());
            drop(original);
        }
    }

    #[cfg(unix)]
    #[test]
    fn unicode_parent_roundtrips_and_journal_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let profile = tempfile::tempdir().unwrap();
        let folder = profile.path().join("Downloads 世界");
        fs::create_dir(&folder).unwrap();
        let journal = profile.path().join("download-recovery.json");
        let mut queue = DownloadCleanup::open(journal.clone());
        let (_, file) = queue.create(&folder).unwrap();
        assert_eq!(
            fs::metadata(&journal).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let (handle, path) = file.keep().unwrap();
        drop(handle);
        assert!(DownloadCleanup::open(journal).entries.is_empty());
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn journal_encoding_preserves_non_unicode_paths() {
        use std::os::unix::ffi::OsStringExt;
        // macOS's filesystem rejects invalid UTF8 names; test the lossless
        // journal representation independently of filesystem support.
        let id = Uuid::new_v4();
        let mut bytes = b"/downloads/".to_vec();
        bytes.push(0xff);
        bytes.extend_from_slice(format!("/.newport-download-{id}").as_bytes());
        let entry = Entry {
            path: OsString::from_vec(bytes),
            identity: Identity {
                device: 1,
                inode: 2,
            },
        };
        let encoded = serde_json::to_vec(&entry).unwrap();
        let decoded: Entry = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.path, entry.path);
        assert!(valid_path(id, Path::new(&decoded.path)));
    }
}
