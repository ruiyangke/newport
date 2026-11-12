//! Profiles saved by earlier versions: Stronghold snapshots encrypted with a
//! 32-byte key in OS credential storage. `profiles.stronghold` holds servers,
//! forwards and SSH passwords; `account.stronghold` holds the sign-in. They
//! are read once, when the profile has no `profiles.json` or no `account`
//! secret, and never written.
use super::account;
use iota_stronghold::{KeyProvider, SnapshotPath, Stronghold};
use newport_domain::model::Config;
use std::{collections::BTreeMap, path::Path};
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

pub(super) const PROFILES: &str = "profiles.stronghold";
/// Written by earlier versions once the profile had its key.
pub(super) const KEY_MARKER: &str = "vault-key.newport.storage";
pub(super) const ACCOUNT: &str = "account.stronghold";
const PROFILES_CLIENT: &[u8] = b"newport-profiles-v1";
const PROFILES_RECORD: &[u8] = b"config";
const PROFILES_SCHEMA: u64 = 3;
const ACCOUNT_CLIENT: &[u8] = b"newport-account-v1";
const ACCOUNT_RECORD: &[u8] = b"account";

#[derive(serde::Deserialize)]
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

/// Saved profiles and the passwords of their servers.
pub(super) struct Profiles {
    pub config: Config,
    pub passwords: Vec<(Uuid, Zeroizing<String>)>,
}

const LOCKED: &str = "Cannot unlock saved profiles. Check the Newport key in your system credential storage; profiles have not been changed.";

pub(super) fn profiles(path: &Path, key: Option<&[u8]>) -> Result<Profiles, String> {
    let key = key.ok_or(LOCKED)?;
    let data = record(path, key, PROFILES_CLIENT, PROFILES_RECORD)
        .map_err(|_| LOCKED.to_string())?
        .ok_or("Saved profiles are damaged; they have not been changed.")?;
    let mut record: ProfileRecord = serde_json::from_slice(&data)
        .map_err(|_| "Saved profiles are damaged; they have not been changed.".to_string())?;
    if record.schema_version != PROFILES_SCHEMA {
        return Err("Unsupported saved profiles. Update Newport to use them.".into());
    }
    record.config.validate()?;
    let passwords = std::mem::take(&mut record.passwords)
        .into_iter()
        .filter(|(id, _)| record.config.servers.iter().any(|server| server.id == *id))
        .map(|(id, password)| (id, Zeroizing::new(password)))
        .collect();
    Ok(Profiles {
        config: std::mem::take(&mut record.config),
        passwords,
    })
}

pub(super) fn account(path: &Path, key: Option<&[u8]>) -> Result<Option<account::Record>, String> {
    const LOCKED: &str = "Cannot unlock the saved Newport sign-in.";
    let key = key.ok_or(LOCKED)?;
    let Some(data) =
        record(path, key, ACCOUNT_CLIENT, ACCOUNT_RECORD).map_err(|_| LOCKED.to_string())?
    else {
        return Ok(None);
    };
    account::decode(&data).map(Some)
}

fn record(
    path: &Path,
    key: &[u8],
    client: &[u8],
    name: &[u8],
) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
    let key =
        KeyProvider::try_from(Zeroizing::new(key.to_vec())).map_err(|error| error.to_string())?;
    let snapshot = Stronghold::default();
    snapshot
        .load_snapshot(&key, &SnapshotPath::from_path(path))
        .map_err(|error| error.to_string())?;
    let client = snapshot
        .load_client(client)
        .map_err(|error| error.to_string())?;
    Ok(client
        .store()
        .get(name)
        .map_err(|error| error.to_string())?
        .map(Zeroizing::new))
}

/// Writes a snapshot as earlier versions did, for import tests.
#[cfg(test)]
pub fn write_fixture(path: &Path, key: &[u8], client: &[u8], name: &[u8], data: &[u8]) {
    let provider = KeyProvider::try_from(Zeroizing::new(key.to_vec())).unwrap();
    let snapshot = Stronghold::default();
    snapshot
        .create_client(client)
        .unwrap()
        .store()
        .insert(name.to_vec(), data.to_vec(), None)
        .unwrap();
    snapshot
        .commit_with_keyprovider(&SnapshotPath::from_path(path), &provider)
        .unwrap();
}
#[cfg(test)]
pub fn write_profiles_fixture(path: &Path, key: &[u8], record: &serde_json::Value) {
    write_fixture(
        path,
        key,
        PROFILES_CLIENT,
        PROFILES_RECORD,
        &serde_json::to_vec(record).unwrap(),
    );
}
#[cfg(test)]
pub fn write_account_fixture(path: &Path, key: &[u8], record: &account::Record) {
    write_fixture(
        path,
        key,
        ACCOUNT_CLIENT,
        ACCOUNT_RECORD,
        &account::encode(record).unwrap(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> serde_json::Value {
        serde_json::json!({"servers":[{"id":"8d43df10-73f7-4358-b818-c41dddc2006e","name":"Imported","sshHost":"imported.example","sshPort":22,"sshUser":"me","identityFile":null,"authMethod":"password"}],"tunnels":[]})
    }

    #[test]
    fn profiles_import_with_the_passwords_of_their_servers() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(PROFILES);
        let gone = Uuid::new_v4();
        write_profiles_fixture(
            &path,
            &[7; 32],
            &serde_json::json!({"schemaVersion":3,"config":config(),"passwords":{
                "8d43df10-73f7-4358-b818-c41dddc2006e":"kept", gone.to_string():"stale"}}),
        );
        let imported = profiles(&path, Some(&[7; 32])).unwrap();
        assert_eq!(imported.config, serde_json::from_value(config()).unwrap());
        assert_eq!(imported.passwords.len(), 1);
        assert_eq!(imported.passwords[0].1.as_str(), "kept");
        for key in [None, Some(&[8; 32][..])] {
            assert!(
                profiles(&path, key)
                    .err()
                    .unwrap()
                    .contains("Cannot unlock")
            );
        }
    }

    #[test]
    fn only_the_schema_earlier_versions_wrote_is_imported() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(PROFILES);
        for version in [2, 4] {
            write_profiles_fixture(
                &path,
                &[7; 32],
                &serde_json::json!({"schemaVersion":version,"config":config()}),
            );
            assert!(profiles(&path, Some(&[7; 32])).is_err());
        }
    }

    #[test]
    fn a_sign_in_imports_whole() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(ACCOUNT);
        let mut record = account::Record::new(
            "credential".into(),
            "device".into(),
            serde_json::json!({"state":"pro","token":"v4.public.x"}),
        );
        record.trusted_time = Some(1_800_000_000);
        write_account_fixture(&path, &[7; 32], &record);
        assert_eq!(account(&path, Some(&[7; 32])).unwrap(), Some(record));
        assert!(account(&path, Some(&[8; 32])).is_err());
        assert!(account(&path, None).is_err());
    }
}
