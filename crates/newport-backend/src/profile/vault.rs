//! Encrypted profile snapshots. The unlock key is never sent to the UI.
use super::snapshot::Snapshot as Stronghold;
use newport_domain::model::Config;
use std::{collections::BTreeMap, fs, path::Path};
use uuid::Uuid;
use zeroize::Zeroize;
use zeroize::Zeroizing;

const CLIENT: &[u8] = b"newport-profiles-v1";
const RECORD: &[u8] = b"config";

// v3 prevents older apps from reinterpreting reverse forwards or erasing consent.
const SCHEMA_VERSION: u64 = 3;

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
        schema_version: u64,
    }
    let version: Version =
        serde_json::from_slice(data).map_err(|_| "Invalid encrypted profile format".to_string())?;
    if version.schema_version > SCHEMA_VERSION {
        return Err(
            "Unsupported profile version. Update Newport before editing these profiles.".into(),
        );
    }
    if version.schema_version < SCHEMA_VERSION {
        return Err("Unsupported profile version from an older app.".into());
    }
    let record: ProfileRecord =
        serde_json::from_slice(data).map_err(|_| "Invalid encrypted profiles".to_string())?;
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
    prepare(path, key, config, password)?.commit()
}

/// A verified encrypted replacement, not yet visible to readers. Dropping it
/// discards the attempt. The configuration owner must recheck its base revision
/// and serialize the final rename with publication before calling `commit`.
pub(crate) struct Prepared {
    staging: tempfile::TempDir,
    destination: std::path::PathBuf,
}
impl Prepared {
    pub(crate) fn commit(self) -> Result<(), String> {
        self.publish()?.map_or(Ok(()), Err)
    }

    /// Once the atomic rename succeeds the file is committed even if syncing
    /// the directory fails. Callers must publish that state before reporting
    /// the durability warning; treating it as an uncommitted write loses truth.
    pub(crate) fn publish(self) -> Result<Option<String>, String> {
        let staged_path = self.staging.path().join("profiles.stronghold");
        newport_platform::filesystem::replace_file(&staged_path, &self.destination)
            .map_err(|e| e.to_string())?;
        Ok(newport_platform::filesystem::sync_directory(
            self.destination
                .parent()
                .ok_or("Vault directory is missing")?,
        )
        .err()
        .map(|error| format!("Profile saved, but directory synchronization failed: {error}")))
    }
}

pub(crate) fn prepare(
    path: &Path,
    key: &[u8],
    config: &Config,
    password: Option<(Uuid, &str)>,
) -> Result<Prepared, String> {
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
    newport_platform::filesystem::protect_file(&staged_path).map_err(|e| e.to_string())?;
    // Windows FlushFileBuffers requires a handle opened with write access.
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&staged_path)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(Prepared {
        staging,
        destination: path.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Config {
        serde_json::from_value(serde_json::json!({"servers":[{"id":uuid::Uuid::new_v4(),"name":"Private production","sshHost":"private.internal.example","sshPort":22,"sshUser":"private-user","identityFile":"/private/key"}],"tunnels":[]})).unwrap()
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

    #[test]
    fn only_the_current_schema_is_accepted() {
        let config = config();
        let record = serde_json::json!({"schemaVersion": SCHEMA_VERSION, "config": config});
        assert_eq!(
            decode(&serde_json::to_vec(&record).unwrap()).unwrap(),
            config
        );
        // Bare configs and other schema versions are never reinterpreted.
        assert!(decode(&serde_json::to_vec(&config).unwrap()).is_err());
        for version in [
            serde_json::json!(1),
            serde_json::json!(2),
            serde_json::json!(4),
            serde_json::json!("3"),
            serde_json::Value::Null,
        ] {
            let mut other = record.clone();
            other["schemaVersion"] = version;
            assert!(decode(&serde_json::to_vec(&other).unwrap()).is_err());
        }
    }
    #[test]
    fn install_consent_and_reverse_forwards_round_trip_with_the_current_schema() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("profiles.stronghold");
        let key = [7; 32];
        let mut config = config();
        let server = config.servers[0].id;
        config.servers[0].agent_install_approved = true;
        config.servers[0].setup_complete = true;
        config.tunnels.push(
            serde_json::from_value(serde_json::json!({
                "id":Uuid::new_v4(), "serverId":server, "name":"Reverse forward",
                "localPort":3000, "remotePort":9000, "remoteHost":"127.0.0.1",
                "direction":"remote", "project":"/srv/project"
            }))
            .unwrap(),
        );
        write_with_password(&path, &key, &config, Some((server, "retained password"))).unwrap();
        assert_eq!(read(&path, &key).unwrap(), config);
        assert_eq!(
            password(&path, &key, server).unwrap().as_str(),
            "retained password"
        );
        let snapshot = Stronghold::new(&path, key.to_vec()).unwrap();
        let bytes = snapshot
            .load_client(CLIENT)
            .unwrap()
            .store()
            .get(RECORD)
            .unwrap()
            .unwrap();
        let envelope: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(envelope["schemaVersion"], SCHEMA_VERSION);
    }
    #[test]
    fn encrypted_roundtrip_and_wrong_key_preserve_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("profiles.stronghold");
        let config = config();
        write(&path, &[3; 32], &config).unwrap();
        let bytes = fs::read(&path).unwrap();
        for value in ["private.internal.example", "private-user", "/private/key"] {
            assert!(
                !bytes
                    .windows(value.len())
                    .any(|window| window == value.as_bytes())
            );
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
