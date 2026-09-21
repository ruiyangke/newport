use crate::model::{Config, Server, Tunnel};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

pub struct Store {
    pub directory: PathBuf,
    key: std::sync::OnceLock<Result<zeroize::Zeroizing<Vec<u8>>, String>>,
}
impl Store {
    pub fn open() -> Result<Self, String> {
        let directory = if let Some(path) = std::env::var_os("PORTHOP_DATA_DIR") {
            PathBuf::from(path)
        } else {
            dirs::data_dir()
                .ok_or("Cannot locate Application Support")?
                .join("Porthop")
        };
        fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
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
            .get_or_init(|| crate::keychain::vault_key(&self.directory))
            .as_ref()
            .map(|key| key.as_slice())
            .map_err(Clone::clone)
    }
    pub fn load(&self) -> Result<Config, String> {
        let target = self.directory.join("config.json");
        let vault = self.directory.join("profiles.stronghold");
        if vault.exists() {
            let config = crate::vault::read(&vault, self.vault_key()?)?;
            self.remove_plaintext()?;
            return Ok(config);
        }
        if target.exists() {
            let config = read_config(&target)?;
            self.save(&config)?;
            self.remove_plaintext()?;
            return Ok(config);
        }
        let legacy = if self.directory.join("tunnels.json").exists()
            || self.directory.join("servers.json").exists()
        {
            self.directory.clone()
        } else if std::env::var_os("PORTHOP_DATA_DIR").is_none() {
            self.directory.with_file_name("SSHTunnelBar")
        } else {
            self.directory.clone()
        };
        let mut config = Config::default();
        if legacy.join("servers.json").exists() {
            config.servers = serde_json::from_slice(&read(&legacy.join("servers.json"))?)
                .map_err(|e| format!("Cannot import servers.json: {e}"))?;
            if legacy.join("tunnels.json").exists() {
                config.tunnels = serde_json::from_slice(&read(&legacy.join("tunnels.json"))?)
                    .map_err(|e| format!("Cannot import tunnels.json: {e}"))?;
            }
        } else if legacy.join("tunnels.json").exists() {
            let values: Vec<Value> = serde_json::from_slice(&read(&legacy.join("tunnels.json"))?)
                .map_err(|e| e.to_string())?;
            for mut value in values {
                let mut server: Server = serde_json::from_value(value.clone())
                    .map_err(|e| format!("Cannot import legacy tunnel: {e}"))?;
                let id = if let Some(found) = config.servers.iter().find(|s| {
                    s.ssh_host == server.ssh_host
                        && s.ssh_user == server.ssh_user
                        && s.ssh_port == server.ssh_port
                        && s.identity_file == server.identity_file
                }) {
                    found.id
                } else {
                    server.id = Uuid::new_v4();
                    server.name = server.ssh_host.clone();
                    let id = server.id;
                    config.servers.push(server);
                    id
                };
                value["serverId"] = serde_json::json!(id);
                config
                    .tunnels
                    .push(serde_json::from_value::<Tunnel>(value).map_err(|e| e.to_string())?);
            }
        }
        config.validate()?;
        self.save(&config)?;
        self.remove_plaintext()?;
        Ok(config)
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
    fn remove_plaintext(&self) -> Result<(), String> {
        for name in ["config.json", "servers.json", "tunnels.json"] {
            match fs::remove_file(self.directory.join(name)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(format!(
                        "Profiles encrypted, but cannot remove old {name}: {e}"
                    ))
                }
            }
        }
        Ok(())
    }
}
fn read(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))
}
fn read_config(path: &Path) -> Result<Config, String> {
    let c: Config = serde_json::from_slice(&read(path)?)
        .map_err(|e| format!("Cannot read saved profiles; original file was preserved: {e}"))?;
    c.validate()?;
    Ok(c)
}
#[cfg(test)]
mod tests {
    use super::*;
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
