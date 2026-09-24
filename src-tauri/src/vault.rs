//! Encrypted profile snapshots. The unlock key never crosses the webview boundary.
use crate::model::Config;
use std::{collections::BTreeMap, fs, path::Path};
use tauri_plugin_stronghold::stronghold::Stronghold;
use uuid::Uuid;
use zeroize::Zeroize;
use zeroize::Zeroizing;

const CLIENT: &[u8] = b"porthop-profiles-v1";
const RECORD: &[u8] = b"config";

const SCHEMA_VERSION: u64 = 2;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileRecord {
    schema_version: u64,
    config: Config,
    #[serde(default)]
    passwords: BTreeMap<Uuid, String>,
}
impl Drop for ProfileRecord {
    fn drop(&mut self) {
        for password in self.passwords.values_mut() {
            password.zeroize();
        }
    }
}

fn decode_record(data: &[u8]) -> Result<ProfileRecord, String> {
    #[derive(serde::Deserialize)]
    struct Version {
        #[serde(rename = "schemaVersion")]
        schema_version: Option<u64>,
    }
    let version: Version =
        serde_json::from_slice(data).map_err(|_| "Invalid encrypted profile format".to_string())?;
    let record = match version.schema_version {
        Some(1 | SCHEMA_VERSION) => {
            serde_json::from_slice(data).map_err(|_| "Invalid encrypted profiles".to_string())?
        }
        Some(_) => {
            return Err(
                "Unsupported profile version. Update Porthop before editing these profiles.".into(),
            )
        }
        None => ProfileRecord {
            schema_version: SCHEMA_VERSION,
            config: serde_json::from_slice(data)
                .map_err(|_| "Invalid encrypted profiles".to_string())?,
            passwords: BTreeMap::new(),
        },
    };
    record.config.validate()?;
    Ok(record)
}
#[cfg(test)]
fn decode(data: &[u8]) -> Result<Config, String> {
    Ok(decode_record(data)?.config.clone())
}

pub fn read(path: &Path, key: &[u8]) -> Result<Config, String> {
    Ok(read_record(path, key)?.config.clone())
}
fn read_record(path: &Path, key: &[u8]) -> Result<ProfileRecord, String> {
    let vault = Stronghold::new(path, key.to_vec()).map_err(|_| {
        "Cannot unlock saved profiles. Check the vault key in your system credential storage."
            .to_string()
    })?;
    let client = vault.load_client(CLIENT).map_err(|e| e.to_string())?;
    let data = Zeroizing::new(
        client
            .store()
            .get(RECORD)
            .map_err(|e| e.to_string())?
            .ok_or("Encrypted profile record is missing")?,
    );
    decode_record(&data)
}

pub fn password(path: &Path, key: &[u8], id: Uuid) -> Result<Zeroizing<String>, String> {
    read_record(path, key)?
        .passwords
        .get(&id)
        .cloned()
        .map(Zeroizing::new)
        .ok_or_else(|| "No saved SSH password. Save a password in Edit Server.".into())
}

pub fn write(path: &Path, key: &[u8], config: &Config) -> Result<(), String> {
    write_with_password(path, key, config, None)
}

pub fn write_with_password(
    path: &Path,
    key: &[u8],
    config: &Config,
    password: Option<(Uuid, &str)>,
) -> Result<(), String> {
    config.validate()?;
    let mut record = if path.exists() {
        read_record(path, key)?
    } else {
        ProfileRecord {
            schema_version: SCHEMA_VERSION,
            config: config.clone(),
            passwords: BTreeMap::new(),
        }
    };
    record.schema_version = SCHEMA_VERSION;
    record.config = config.clone();
    record.passwords.retain(|id, secret| {
        let keep = config.servers.iter().any(|server| server.id == *id);
        if !keep {
            secret.zeroize();
        }
        keep
    });
    if let Some((id, secret)) = password {
        if !config.servers.iter().any(|server| server.id == id) {
            return Err("Server no longer exists.".into());
        }
        if let Some(mut previous) = record.passwords.insert(id, secret.to_owned()) {
            previous.zeroize();
        }
    }
    let directory = path.parent().ok_or("Vault directory is missing")?;
    // Build and verify a complete encrypted replacement before touching the last snapshot.
    let staging = tempfile::tempdir_in(directory).map_err(|e| e.to_string())?;
    let staged_path = staging.path().join("profiles.stronghold");
    let vault = Stronghold::new(&staged_path, key.to_vec()).map_err(|e| e.to_string())?;
    let client = vault.create_client(CLIENT).map_err(|e| e.to_string())?;
    client
        .store()
        .insert(
            RECORD.to_vec(),
            Zeroizing::new(
                serde_json::to_vec(&record)
                    .map_err(|_| "Cannot encode encrypted profiles".to_string())?,
            )
            .to_vec(),
            None,
        )
        .map_err(|e| e.to_string())?;
    vault.save().map_err(|e| e.to_string())?;
    let verified = read_record(&staged_path, key)?;
    if verified.config != record.config || verified.passwords != record.passwords {
        return Err("Encrypted profile verification failed; previous snapshot preserved".into());
    }
    crate::platform::filesystem::protect_file(&staged_path).map_err(|e| e.to_string())?;
    // Windows FlushFileBuffers requires a handle opened with write access.
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&staged_path)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    crate::platform::filesystem::replace_file(&staged_path, path).map_err(|e| e.to_string())?;
    crate::platform::filesystem::sync_directory(directory).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Config {
        serde_json::from_value(serde_json::json!({"servers":[{"id":uuid::Uuid::new_v4(),"name":"Private production","sshHost":"private.internal.example","sshPort":22,"sshUser":"private-user","identityFile":"/private/key"}],"tunnels":[]})).unwrap()
    }
    #[tokio::test]
    async fn passwords_are_atomic_private_and_removed_with_the_server() {
        let directory = tempfile::tempdir().unwrap();
        let store = crate::config::Store::for_test(directory.path().into());
        let path = directory.path().join("profiles.stronghold");
        let mut manager = crate::manager::Manager::new(store);
        let mut config = config();
        let id = config.servers[0].id;
        let secret = "vault-only-test-password-482";
        manager
            .save_with_password(config.clone(), Some((id, Zeroizing::new(secret.into()))))
            .await
            .unwrap();
        assert_eq!(manager.store.password(id).unwrap().as_str(), secret);
        let snapshot = serde_json::to_string(&manager.snapshot()).unwrap();
        assert!(!snapshot.contains(secret));
        assert!(!snapshot.contains("passwords"));
        let bytes = fs::read(&path).unwrap();
        assert!(!bytes
            .windows(secret.len())
            .any(|window| window == secret.as_bytes()));
        config.servers[0].name = "Updated name".into();
        manager.save(config.clone()).await.unwrap();
        let reopened = crate::config::Store::for_test(directory.path().into());
        assert_eq!(reopened.password(id).unwrap().as_str(), secret);
        let original = fs::read(&path).unwrap();
        let mut invalid = config.clone();
        invalid.servers[0].ssh_port = 0;
        assert!(manager
            .save_with_password(invalid, Some((id, Zeroizing::new("replacement".into()))))
            .await
            .is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(manager.store.password(id).unwrap().as_str(), secret);
        manager.save(Config::default()).await.unwrap();
        assert!(manager.store.password(id).is_err());
        manager.save(config).await.unwrap();
        assert!(manager.store.password(id).is_err());
    }

    #[test]
    fn wrong_key_cannot_replace_passwords() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("profiles.stronghold");
        let config = config();
        let id = config.servers[0].id;
        write_with_password(&path, &[7; 32], &config, Some((id, "original"))).unwrap();
        let original = fs::read(&path).unwrap();
        assert!(write_with_password(&path, &[8; 32], &config, Some((id, "replacement"))).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(password(&path, &[7; 32], id).unwrap().as_str(), "original");
        write_with_password(&path, &[7; 32], &config, Some((id, "replacement"))).unwrap();
        assert_eq!(
            password(&path, &[7; 32], id).unwrap().as_str(),
            "replacement"
        );
    }

    #[tokio::test]
    async fn future_snapshot_blocks_manager_writes_without_changing_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("profiles.stronghold");
        let vault = Stronghold::new(&path, vec![7; 32]).unwrap();
        let client = vault.create_client(CLIENT).unwrap();
        client
            .store()
            .insert(
                RECORD.to_vec(),
                serde_json::to_vec(&serde_json::json!({"schemaVersion": 999, "config": config()}))
                    .unwrap(),
                None,
            )
            .unwrap();
        vault.save().unwrap();
        let original = fs::read(&path).unwrap();
        let mut manager =
            crate::manager::Manager::new(crate::config::Store::for_test(directory.path().into()));
        assert!(manager
            .load_error
            .as_ref()
            .unwrap()
            .contains("Unsupported profile version"));
        assert!(manager.save(Config::default()).await.is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
    }
    #[test]
    fn versions_preserve_legacy_profiles_and_reject_future_records() {
        let config = config();
        assert_eq!(
            decode(&serde_json::to_vec(&config).unwrap()).unwrap(),
            config
        );
        let record = serde_json::json!({"schemaVersion": 1, "config": config});
        assert_eq!(
            decode(&serde_json::to_vec(&record).unwrap()).unwrap(),
            config
        );
        for version in [
            serde_json::json!(3),
            serde_json::json!("1"),
            serde_json::Value::Null,
        ] {
            let mut future = record.clone();
            future["schemaVersion"] = version;
            assert!(decode(&serde_json::to_vec(&future).unwrap()).is_err());
        }
    }
    #[test]
    fn encrypted_roundtrip_and_wrong_key_preserve_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("profiles.stronghold");
        let config = config();
        write(&path, &[3; 32], &config).unwrap();
        let bytes = fs::read(&path).unwrap();
        for value in ["private.internal.example", "private-user", "/private/key"] {
            assert!(!bytes
                .windows(value.len())
                .any(|window| window == value.as_bytes()));
        }
        assert_eq!(read(&path, &[3; 32]).unwrap().servers, config.servers);
        assert!(read(&path, &[4; 32]).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        let mut invalid = config;
        invalid.servers[0].ssh_port = 0;
        assert!(write(&path, &[3; 32], &invalid).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}
