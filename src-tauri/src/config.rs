use crate::migration::{data_directory_override, profile_directory};
use crate::model::Config;
#[cfg(test)]
use crate::model::Server;
use std::{fs, path::PathBuf};
use uuid::Uuid;

pub struct Store {
    pub directory: PathBuf,
    key: std::sync::OnceLock<Result<zeroize::Zeroizing<Vec<u8>>, String>>,
}
impl Store {
    pub fn open() -> Result<Self, String> {
        let directory = match data_directory_override() {
            Some(path) => path,
            None => profile_directory(
                &crate::platform::filesystem::application_data_directory()
                    .ok_or("Cannot locate application data directory")?,
            )?,
        };
        fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        crate::platform::filesystem::protect_directory(&directory).map_err(|e| e.to_string())?;
        Ok(Self {
            directory,
            key: Default::default(),
        })
    }
    #[cfg(test)]
    pub fn for_test(directory: PathBuf) -> Self {
        let key = std::sync::OnceLock::new();
        key.set(Ok(zeroize::Zeroizing::new(vec![7; 32]))).unwrap();
        Self { directory, key }
    }
    pub fn vault_key(&self) -> Result<&[u8], String> {
        self.key
            .get_or_init(|| crate::platform::secrets::vault_key(&self.directory))
            .as_ref()
            .map(|key| key.as_slice())
            .map_err(Clone::clone)
    }
    pub fn load(&self) -> Result<Config, String> {
        let vault = self.directory.join("profiles.stronghold");
        if vault.exists() {
            let config = crate::vault::read(&vault, self.vault_key()?)?;
            crate::migration::remove_plaintext(&self.directory)?;
            return Ok(config);
        }
        crate::migration::import_profiles(self)
    }
    pub fn save(&self, config: &Config) -> Result<(), String> {
        crate::vault::write(
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
        crate::vault::write_with_password(
            &self.directory.join("profiles.stronghold"),
            self.vault_key()?,
            config,
            password,
        )
    }
    pub fn password(&self, id: Uuid) -> Result<zeroize::Zeroizing<String>, String> {
        crate::vault::password(
            &self.directory.join("profiles.stronghold"),
            self.vault_key()?,
            id,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
