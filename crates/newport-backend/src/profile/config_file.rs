//! `profiles.json`: servers, forwards and their sharing permissions as JSON
//! only this user can read, with each saved SSH password sealed with the
//! master key. One atomic write saves the configuration and its passwords.
use newport_domain::model::Config;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

pub(super) const FILE: &str = "profiles.json";
const SCHEMA_VERSION: u64 = 1;

/// Passwords by server, each sealed for that server.
pub(super) type Passwords = BTreeMap<Uuid, String>;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Saved<'a> {
    schema_version: u64,
    config: &'a Config,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    passwords: &'a Passwords,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Loaded {
    #[serde(rename = "schemaVersion")]
    _schema_version: u64,
    config: Config,
    #[serde(default)]
    passwords: Passwords,
}

#[derive(Default)]
pub(super) struct Record {
    pub config: Config,
    pub passwords: Passwords,
}

/// The saved profile, or `None` before the first save.
pub(super) fn read(path: &Path) -> Result<Option<Record>, String> {
    match fs::read(path) {
        Ok(data) => decode(&data).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Cannot read saved profiles: {error}")),
    }
}

fn decode(data: &[u8]) -> Result<Record, String> {
    #[derive(serde::Deserialize)]
    struct Version {
        #[serde(rename = "schemaVersion")]
        schema_version: u64,
    }
    let version: Version =
        serde_json::from_slice(data).map_err(|_| "Saved profiles are damaged.".to_string())?;
    if version.schema_version > SCHEMA_VERSION {
        return Err(
            "Saved profiles are from a newer Newport. Update Newport before editing them.".into(),
        );
    }
    if version.schema_version != SCHEMA_VERSION {
        return Err("Saved profiles are damaged.".into());
    }
    let loaded: Loaded =
        serde_json::from_slice(data).map_err(|_| "Saved profiles are damaged.".to_string())?;
    loaded.config.validate()?;
    Ok(Record {
        config: loaded.config,
        passwords: loaded.passwords,
    })
}

/// A verified replacement, not yet visible to readers. Dropping it discards the
/// attempt. The configuration owner must recheck its base revision and
/// serialize `publish` with publication of the new configuration.
pub(crate) struct Prepared {
    staging: tempfile::TempDir,
    destination: PathBuf,
}
impl Prepared {
    pub(crate) fn commit(self) -> Result<(), String> {
        self.publish()?.map_or(Ok(()), Err)
    }

    /// Once the rename succeeds the file is committed even if syncing the
    /// directory fails. Callers must publish that state before reporting the
    /// durability warning; treating it as an uncommitted write loses truth.
    pub(crate) fn publish(self) -> Result<Option<String>, String> {
        let name = self.destination.file_name().ok_or("Invalid profile file")?;
        newport_platform::filesystem::replace_file(
            &self.staging.path().join(name),
            &self.destination,
        )
        .map_err(|e| e.to_string())?;
        Ok(newport_platform::filesystem::sync_directory(
            self.destination
                .parent()
                .ok_or("Profile directory is missing")?,
        )
        .err()
        .map(|error| format!("Profile saved, but directory synchronization failed: {error}")))
    }
}

/// Stages `config` beside `path`, keeping the saved passwords of the servers
/// it still has and replacing the one `password` names.
pub(super) fn prepare(
    path: &Path,
    config: &Config,
    password: Option<(Uuid, String)>,
) -> Result<Prepared, String> {
    config.validate()?;
    let kept = |id: &Uuid| config.servers.iter().any(|server| server.id == *id);
    let mut passwords = read(path)?.unwrap_or_default().passwords;
    passwords.retain(|id, _| kept(id));
    if let Some((id, sealed)) = password {
        if !kept(&id) {
            return Err("Server no longer exists.".into());
        }
        passwords.insert(id, sealed);
    }
    stage(path, config, &passwords)
}

/// Stages exactly `config` and `passwords` beside `path`.
pub(super) fn stage(
    path: &Path,
    config: &Config,
    passwords: &Passwords,
) -> Result<Prepared, String> {
    config.validate()?;
    let data = serde_json::to_vec_pretty(&Saved {
        schema_version: SCHEMA_VERSION,
        config,
        passwords,
    })
    .map_err(|_| "Cannot encode profiles".to_string())?;
    let prepared = stage_file(path, &data)?;
    let staged = prepared.staging.path().join(FILE);
    let verified = decode(&fs::read(&staged).map_err(|e| e.to_string())?)?;
    if verified.config != *config || verified.passwords != *passwords {
        return Err("Profile verification failed; the saved profiles were not changed.".into());
    }
    Ok(prepared)
}

/// Writes `data` as a private file in a staging folder beside `path`, synced
/// and ready for an atomic replace.
pub(super) fn stage_file(path: &Path, data: &[u8]) -> Result<Prepared, String> {
    let directory = path.parent().ok_or("Profile directory is missing")?;
    let name = path.file_name().ok_or("Invalid profile file")?;
    let staging = tempfile::tempdir_in(directory).map_err(|e| e.to_string())?;
    let staged = staging.path().join(name);
    let mut file = newport_platform::filesystem::private_file_options()
        .write(true)
        .create_new(true)
        .open(&staged)
        .map_err(|e| e.to_string())?;
    std::io::Write::write_all(&mut file, data).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);
    newport_platform::filesystem::protect_file(&staged).map_err(|e| e.to_string())?;
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
    fn profiles_round_trip_and_keep_the_passwords_of_their_servers() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(FILE);
        let mut config = config();
        let id = config.servers[0].id;
        assert!(read(&path).unwrap().is_none());
        prepare(&path, &config, Some((id, "v1.sealed".into())))
            .unwrap()
            .commit()
            .unwrap();
        let saved = read(&path).unwrap().unwrap();
        assert_eq!(saved.config, config);
        assert_eq!(saved.passwords[&id], "v1.sealed");
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("private.internal.example"));
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["schemaVersion"], SCHEMA_VERSION);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        // An edit keeps the password; removing the server removes it.
        config.servers[0].name = "Renamed".into();
        prepare(&path, &config, None).unwrap().commit().unwrap();
        assert_eq!(read(&path).unwrap().unwrap().passwords[&id], "v1.sealed");
        prepare(&path, &Config::default(), None)
            .unwrap()
            .commit()
            .unwrap();
        assert!(read(&path).unwrap().unwrap().passwords.is_empty());
        assert!(!fs::read_to_string(&path).unwrap().contains("passwords"));
        assert!(prepare(&path, &Config::default(), Some((id, "v1.late".into()))).is_err());
    }

    #[test]
    fn invalid_damaged_or_newer_profiles_are_never_replaced_silently() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(FILE);
        let config = config();
        prepare(&path, &config, None).unwrap().commit().unwrap();
        let saved = fs::read(&path).unwrap();
        let mut invalid = config.clone();
        invalid.servers[0].ssh_port = 0;
        assert!(prepare(&path, &invalid, None).is_err());
        assert_eq!(fs::read(&path).unwrap(), saved);

        let current = serde_json::json!({"schemaVersion": SCHEMA_VERSION, "config": config});
        assert_eq!(
            decode(&serde_json::to_vec(&current).unwrap())
                .unwrap()
                .config,
            config
        );
        // A bare config, other versions and unknown fields are never reinterpreted.
        assert!(decode(&serde_json::to_vec(&config).unwrap()).is_err());
        for version in [
            serde_json::json!(0),
            serde_json::json!(2),
            serde_json::json!("1"),
        ] {
            let mut other = current.clone();
            other["schemaVersion"] = version;
            assert!(decode(&serde_json::to_vec(&other).unwrap()).is_err());
        }
        let mut extra = current.clone();
        extra["future"] = serde_json::json!({});
        assert!(decode(&serde_json::to_vec(&extra).unwrap()).is_err());

        fs::write(&path, "damaged").unwrap();
        assert!(read(&path).is_err());
        assert!(prepare(&path, &config, None).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"damaged");
    }
}
