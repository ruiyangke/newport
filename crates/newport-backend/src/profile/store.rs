#[cfg(test)]
use super::profile_directory;
use newport_domain::model::Config;
#[cfg(test)]
use newport_domain::model::Server;
use std::{
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

type KeySource = dyn Fn(&Path) -> Result<zeroize::Zeroizing<Vec<u8>>, String> + Send + Sync;
#[cfg(test)]
type PreparationHook = dyn Fn(&Config) + Send + Sync;

pub struct Store {
    key_source: Box<KeySource>,
    pub directory: PathBuf,
    key: std::sync::OnceLock<Result<zeroize::Zeroizing<Vec<u8>>, String>>,
    #[cfg(test)]
    pub(crate) preparation_hook: std::sync::Mutex<Option<std::sync::Arc<PreparationHook>>>,
}
impl Store {
    /// The application supplies its OS credential adapter. No UI framework is needed.
    pub fn open(
        directory: PathBuf,
        key_source: impl Fn(&Path) -> Result<zeroize::Zeroizing<Vec<u8>>, String>
        + Send
        + Sync
        + 'static,
    ) -> Result<Self, String> {
        fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        newport_platform::filesystem::protect_directory(&directory).map_err(|e| e.to_string())?;
        Ok(Self {
            directory,
            key_source: Box::new(key_source),
            key: Default::default(),
            #[cfg(test)]
            preparation_hook: Default::default(),
        })
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test(directory: PathBuf) -> Self {
        let key = std::sync::OnceLock::new();
        key.set(Ok(zeroize::Zeroizing::new(vec![7; 32]))).unwrap();
        Self {
            directory,
            key,
            key_source: Box::new(|_| Err("Fixture key already initialized".into())),
            #[cfg(test)]
            preparation_hook: Default::default(),
        }
    }
    pub fn vault_key(&self) -> Result<&[u8], String> {
        self.key
            .get_or_init(|| (self.key_source)(&self.directory).and_then(super::credentials::valid))
            .as_ref()
            .map(|key| key.as_slice())
            .map_err(Clone::clone)
    }
    pub fn load(&self) -> Result<Config, String> {
        let vault = self.directory.join("profiles.stronghold");
        if vault.exists() {
            let config = super::vault::read(&vault, self.vault_key()?)?;
            super::remove_plaintext(&self.directory)?;
            return Ok(config);
        }
        super::import_profiles(self)
    }
    pub fn save(&self, config: &Config) -> Result<(), String> {
        super::vault::write(
            &self.directory.join("profiles.stronghold"),
            self.vault_key()?,
            config,
        )
    }
    pub fn save_with_password(
        &self,
        config: &Config,
        password: Option<(Uuid, &str)>,
    ) -> Result<(), String> {
        super::vault::write_with_password(
            &self.directory.join("profiles.stronghold"),
            self.vault_key()?,
            config,
            password,
        )
    }
    pub fn password(&self, id: Uuid) -> Result<zeroize::Zeroizing<String>, String> {
        super::vault::password(
            &self.directory.join("profiles.stronghold"),
            self.vault_key()?,
            id,
        )
    }
    pub(crate) fn prepare(
        &self,
        config: &Config,
        password: Option<(Uuid, &str)>,
    ) -> Result<super::vault::Prepared, String> {
        #[cfg(test)]
        {
            let hook = self.preparation_hook.lock().unwrap().clone();
            if let Some(hook) = hook {
                hook(config);
            }
        }
        super::vault::prepare(
            &self.directory.join("profiles.stronghold"),
            self.vault_key()?,
            config,
            password,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_actual_v030_release_snapshot_without_rewriting_it() {
        // Created through the signed release UI, not this crate's vault writer.
        // See the fixture README for provenance and the public test key.
        let snapshot = include_bytes!(
            "../../../../resources/test-fixtures/released-profile/v0.3.0.stronghold"
        );
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("profiles.stronghold");
        fs::write(&path, snapshot).unwrap();
        let store = Store::for_test(directory.path().into());
        let mut config = store.load().unwrap();
        assert_eq!(config.servers.len(), 1);
        assert!(config.tunnels.is_empty());
        let server = &config.servers[0];
        assert_eq!(server.name, "Release baseline fixture");
        assert_eq!(server.ssh_host, "127.0.0.1");
        assert_eq!(server.ssh_port, 1);
        assert_eq!(server.ssh_user, "fixture");
        assert_eq!(
            server.auth_method,
            newport_domain::model::AuthMethod::PublicKey
        );
        assert!(server.identity_file.is_none());
        assert!(server.agent_source.is_none());
        assert!(server.agent_key_fingerprint.is_none());
        assert_eq!(fs::read(&path).unwrap(), snapshot);

        config.servers[0].name = "Native roundtrip fixture".into();
        store.save(&config).unwrap();
        let reopened = Store::for_test(directory.path().into());
        assert_eq!(reopened.load().unwrap(), config);
    }

    #[test]
    fn explicit_key_source_is_cached_and_failures_cannot_replace_a_vault() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let directory = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let store = Store::open(directory.path().into(), move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(zeroize::Zeroizing::new(vec![7; 32]))
        })
        .unwrap();
        store.save(&Config::default()).unwrap();
        store.load().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let path = directory.path().join("profiles.stronghold");
        let original = fs::read(&path).unwrap();
        let denied = Store::open(directory.path().into(), |_| {
            Err("Credential access denied".into())
        })
        .unwrap();
        assert_eq!(denied.load().unwrap_err(), "Credential access denied");
        assert!(denied.save(&Config::default()).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        let invalid = Store::open(directory.path().into(), |_| {
            Ok(zeroize::Zeroizing::new(vec![7; 31]))
        })
        .unwrap();
        assert!(invalid.vault_key().is_err());
        assert!(invalid.save(&Config::default()).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
    }
    #[test]
    fn new_install_uses_newport_and_existing_profiles_keep_their_path() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            profile_directory(root.path()).unwrap(),
            root.path().join("Newport")
        );
        let legacy = root.path().join("Porthop");
        fs::create_dir(&legacy).unwrap();
        // An empty/new Newport directory must not hide a user's existing profile.
        fs::create_dir(root.path().join("Newport")).unwrap();
        assert_eq!(profile_directory(root.path()).unwrap(), legacy);
    }

    #[test]
    fn rebrand_preserves_encrypted_profiles_passwords_and_supporting_data() {
        let root = tempfile::tempdir().unwrap();
        let legacy = root.path().join("Porthop");
        fs::create_dir(&legacy).unwrap();
        let old = Store::for_test(legacy.clone());
        let server: Server = serde_json::from_value(serde_json::json!({
            "id": Uuid::new_v4(), "name": "My server", "sshHost": "host", "sshUser": "dev", "sshPort": 22
        }))
        .unwrap();
        let id = server.id;
        let config = Config {
            servers: vec![server],
            ..Default::default()
        };
        old.save_with_password(&config, Some((id, "test-password")))
            .unwrap();
        for name in [
            "metrics.sqlite3",
            "vault-key.storage",
            "clipboard-client-test",
        ] {
            fs::write(legacy.join(name), name.as_bytes()).unwrap();
        }
        let original_vault = fs::read(legacy.join("profiles.stronghold")).unwrap();
        for _ in 0..2 {
            let new = Store::for_test(profile_directory(root.path()).unwrap());
            assert_eq!(new.load().unwrap().servers[0].id, id);
            assert_eq!(new.password(id).unwrap().as_str(), "test-password");
            assert_eq!(
                fs::read(new.directory.join("profiles.stronghold")).unwrap(),
                original_vault
            );
            for name in [
                "metrics.sqlite3",
                "vault-key.storage",
                "clipboard-client-test",
            ] {
                assert_eq!(fs::read(new.directory.join(name)).unwrap(), name.as_bytes());
            }
        }
        assert!(!root.path().join("Newport").exists());
    }

    #[test]
    fn invalid_legacy_profile_does_not_silently_create_a_new_profile() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("Porthop"), "invalid").unwrap();
        assert!(profile_directory(root.path()).is_err());
        assert!(!root.path().join("Newport").exists());
    }

    #[test]
    fn corrupt_config_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::for_test(dir.path().into());
        fs::write(dir.path().join("config.json"), "bad").unwrap();
        assert!(s.load().is_err());
        assert_eq!(
            fs::read_to_string(dir.path().join("config.json")).unwrap(),
            "bad"
        );
    }
    #[test]
    fn migrates_json_and_never_falls_back_from_a_damaged_vault() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::for_test(dir.path().into());
        let source = dir.path().join("config.json");
        fs::write(&source, r#"{"servers":[],"tunnels":[]}"#).unwrap();
        store.load().unwrap();
        assert!(!source.exists());
        store.load().unwrap();
        let vault = dir.path().join("profiles.stronghold");
        fs::write(&vault, "damaged").unwrap();
        fs::write(&source, r#"{"servers":[],"tunnels":[]}"#).unwrap();
        assert!(store.load().is_err());
        assert!(source.exists());
        assert_eq!(fs::read(&vault).unwrap(), b"damaged");
    }
    #[test]
    fn imports_legacy_into_encrypted_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::for_test(dir.path().into());
        let data=serde_json::json!([{"id":Uuid::new_v4(),"name":"Web","sshUser":"dev","sshHost":"host","sshPort":22,"localPort":8000,"remoteHost":"127.0.0.1","remotePort":80}]).to_string();
        fs::write(dir.path().join("tunnels.json"), &data).unwrap();
        let c = s.load().unwrap();
        assert_eq!(c.servers.len(), 1);
        assert_eq!(c.tunnels[0].server_id, c.servers[0].id);
        assert!(!dir.path().join("tunnels.json").exists());
        assert!(dir.path().join("profiles.stronghold").exists());
        assert_eq!(s.load().unwrap().tunnels.len(), 1);
    }
}
