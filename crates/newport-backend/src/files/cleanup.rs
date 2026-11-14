//! Exact staging paths journaled before remote creation and retired after
//! publication/removal. Never scans remote directories or deletes by prefix.
//! Recovery uses authorized SFTP connections, never background authentication.
use anyhow::{Context, Result, bail};
use newport_protocol::desktop::configuration::{AuthMethod, ConnectionSettings, Proxy};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashMap, path::PathBuf};
use uuid::Uuid;

/// The connection a staging path was created over: its settings without the
/// connection revision, so cleanup may retry after a reconnect but never on a
/// different endpoint, user, authentication profile or server record.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Scope(pub(crate) ConnectionSettings);
impl Scope {
    pub(super) fn server(&self) -> Uuid {
        self.0.server_id
    }
    /// The form the journal holds. Journals outlive the build that wrote
    /// them, so this list keeps its shape: the settings, a zero revision and
    /// an empty backend instance.
    fn journal(&self) -> Value {
        let ConnectionSettings {
            server_id,
            ssh_host,
            ssh_user,
            ssh_port,
            auth_method,
            identity_file,
            agent_source,
            agent_key_fingerprint,
            proxy,
        } = &self.0;
        json!([
            server_id.to_string(),
            ssh_host,
            ssh_user,
            ssh_port,
            match auth_method {
                AuthMethod::PublicKey => "publicKey",
                AuthMethod::Password => "password",
            },
            identity_file,
            agent_source,
            agent_key_fingerprint,
            match proxy {
                None => Value::Null,
                Some(Proxy::Command(command)) => json!({"command": command.as_str()}),
                Some(Proxy::Jump(id)) => json!({"jump": id.to_string()}),
            },
            0,
            "",
        ])
    }
}

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
    pub fn register(&mut self, scope: &Scope, path: String) -> Result<Uuid> {
        let id = staging_id(&path)?;
        if self.entries.contains_key(&id) {
            bail!("Upload recovery identity already exists");
        }
        self.entries.insert(
            id,
            Entry {
                scope: scope.journal(),
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

    pub fn take(&mut self, scope: &Scope) -> Vec<(Uuid, String)> {
        let scope = scope.journal();
        self.entries
            .iter_mut()
            .filter(|(_, entry)| !entry.active && !entry.running && entry.scope == scope)
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
pub(in crate::files) mod tests {
    use super::*;

    fn path() -> String {
        format!("/work/.newport-upload-{}", Uuid::new_v4())
    }

    pub(in crate::files) fn scope(server: Uuid) -> Scope {
        scope_at(server, "host")
    }

    fn scope_at(server: Uuid, host: &str) -> Scope {
        Scope(ConnectionSettings {
            server_id: server,
            ssh_host: host.into(),
            ssh_user: "user".into(),
            ssh_port: 22,
            auth_method: AuthMethod::PublicKey,
            identity_file: None,
            agent_source: None,
            agent_key_fingerprint: None,
            proxy: None,
        })
    }

    /// Journals written before this build must keep matching their scopes.
    #[test]
    fn the_journal_keeps_the_scope_list_it_has_always_held() {
        let server = Uuid::parse_str("7a9c1d4e-0000-4000-8000-000000000001").unwrap();
        let jump = Uuid::parse_str("7a9c1d4e-0000-4000-8000-000000000002").unwrap();
        let mut scope = scope(server);
        assert_eq!(
            scope.journal(),
            json!([
                server.to_string(),
                "host",
                "user",
                22,
                "publicKey",
                null,
                null,
                null,
                null,
                0,
                ""
            ])
        );
        scope.0.auth_method = AuthMethod::Password;
        scope.0.identity_file = Some("~/.ssh/id".into());
        scope.0.agent_source = Some("onePassword".into());
        scope.0.agent_key_fingerprint = Some("SHA256:key".into());
        scope.0.proxy = Some(Proxy::Jump(Box::new(jump)));
        assert_eq!(
            scope.journal(),
            json!([server.to_string(), "host", "user", 22, "password", "~/.ssh/id",
                "onePassword", "SHA256:key", {"jump": jump.to_string()}, 0, ""])
        );
        scope.0.proxy = Some(Proxy::Command(Box::new("nc %h %p".into())));
        assert_eq!(scope.journal()[8], json!({"command": "nc %h %p"}));
    }

    #[test]
    fn crash_recovery_restores_active_and_inflight_entries_but_never_finished_uploads() {
        let temp = tempfile::tempdir().unwrap();
        let journal = temp.path().join("upload-recovery.json");
        let scope = scope(Uuid::new_v4());
        let mut queue = UploadCleanup::open(journal.clone());
        let active_path = path();
        let active = queue.register(&scope, active_path.clone()).unwrap();
        assert!(
            queue.take(&scope).is_empty(),
            "new connections must not remove live uploads"
        );
        let failed_path = path();
        let failed = queue.register(&scope, failed_path.clone()).unwrap();
        queue.release(failed);
        assert_eq!(queue.take(&scope), vec![(failed, failed_path.clone())]);
        let completed = queue.register(&scope, path()).unwrap();
        queue.finish(completed, true);
        drop(queue);
        let mut recovered = UploadCleanup::open(journal.clone());
        assert!(
            recovered
                .take(&super::tests::scope(Uuid::new_v4()))
                .is_empty()
        );
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
            let scope = scope(Uuid::new_v4());
            assert!(queue.register(&scope, path()).is_err());
            assert!(queue.take(&scope).is_empty());
            assert_eq!(std::fs::read(&journal).unwrap(), contents);
        }
        let mut queue = UploadCleanup::open(temp.path().join("missing/recovery.json"));
        assert!(queue.register(&scope(Uuid::new_v4()), path()).is_err());
        assert!(queue.entries.is_empty());
    }

    #[test]
    fn recovery_rejects_destination_paths_and_mismatched_identities() {
        let temp = tempfile::tempdir().unwrap();
        let journal = temp.path().join("upload-recovery.json");
        let server = Uuid::new_v4();
        for bad in [
            "/work/payload.bin".to_owned(),
            format!("/work/../.newport-upload-{}", Uuid::new_v4()),
            path(),
        ] {
            let id = Uuid::new_v4();
            std::fs::write(
                &journal,
                serde_json::to_vec(
                    &json!({"version":1,"entries":{id.to_string():{"scope":scope(server).journal(),"path":bad}}}),
                )
                .unwrap(),
            )
            .unwrap();
            let mut queue = UploadCleanup::open(journal.clone());
            assert!(queue.take(&scope(server)).is_empty());
            assert!(queue.register(&scope(server), path()).is_err());
        }
    }

    #[test]
    fn failed_retirement_remains_retryable_and_journal_is_private() {
        let temp = tempfile::tempdir().unwrap();
        let journal = temp.path().join("upload-recovery.json");
        let mut queue = UploadCleanup::open(journal.clone());
        let scope = scope(Uuid::new_v4());
        let path = path();
        let id = queue.register(&scope, path.clone()).unwrap();
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
        let server = Uuid::new_v4();
        let original = scope_at(server, "host");
        let other = scope_at(server, "different-host");
        let path = path();
        let id = queue.register(&original, path.clone()).unwrap();
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
        let scope = scope(Uuid::new_v4());
        for _ in 0..30 {
            let id = queue.register(&scope, path()).unwrap();
            queue.release(id);
        }
        assert_eq!(queue.take(&scope).len(), 8);
    }
}
