//! The Newport account this computer is signed in to. It has its own snapshot,
//! encrypted with the profile's vault key, so account changes never contend
//! with configuration edits and older builds never rewrite it.
use super::snapshot::Snapshot as Stronghold;
use std::{fs, path::Path};
use zeroize::{Zeroize, Zeroizing};

pub(super) const FILE: &str = "account.stronghold";
const CLIENT: &[u8] = b"newport-account-v1";
const RECORD: &[u8] = b"account";
const SCHEMA_VERSION: u64 = 1;

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    #[serde(default = "schema_version")]
    schema_version: u64,
    /// Bearer credential for the desktop API. Never sent to the UI.
    pub credential: String,
    pub device_id: String,
    /// The API's license object, kept verbatim so newer fields survive.
    pub license: serde_json::Value,
}
fn schema_version() -> u64 {
    SCHEMA_VERSION
}
impl Record {
    pub fn new(credential: String, device_id: String, license: serde_json::Value) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            credential,
            device_id,
            license,
        }
    }
}
impl Drop for Record {
    fn drop(&mut self) {
        self.credential.zeroize();
        if let Some(serde_json::Value::String(token)) = self.license.get_mut("token") {
            token.zeroize();
        }
    }
}
impl std::fmt::Debug for Record {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Record")
            .field("device_id", &self.device_id)
            .finish_non_exhaustive()
    }
}

pub(super) fn read(path: &Path, key: &[u8]) -> Result<Option<Record>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let vault = Stronghold::new(path, key.to_vec())
        .map_err(|_| "Cannot unlock the saved Newport sign-in.".to_string())?;
    let client = vault.load_client(CLIENT)?;
    let Some(data) = client.store().get(RECORD).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let data = Zeroizing::new(data);
    let record: Record =
        serde_json::from_slice(&data).map_err(|_| "Invalid saved Newport sign-in.".to_string())?;
    if record.schema_version != SCHEMA_VERSION {
        return Err("Unsupported saved sign-in. Update Newport to use it.".into());
    }
    Ok(Some(record))
}

/// Builds and verifies the complete replacement before an atomic rename.
pub(super) fn write(path: &Path, key: &[u8], record: &Record) -> Result<(), String> {
    let directory = path.parent().ok_or("Profile directory is missing")?;
    let staging = tempfile::tempdir_in(directory).map_err(|e| e.to_string())?;
    let staged = staging.path().join(FILE);
    let vault = Stronghold::new(&staged, key.to_vec())?;
    let client = vault.create_client(CLIENT)?;
    let data = Zeroizing::new(
        serde_json::to_vec(record).map_err(|_| "Cannot encode the Newport sign-in.".to_string())?,
    );
    client
        .store()
        .insert(RECORD.to_vec(), data.to_vec(), None)
        .map_err(|e| e.to_string())?;
    vault.save()?;
    if read(&staged, key)?.as_ref() != Some(record) {
        return Err("Saved sign-in verification failed.".into());
    }
    newport_platform::filesystem::protect_file(&staged).map_err(|e| e.to_string())?;
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&staged)
        .and_then(|file| file.sync_all())
        .map_err(|e| e.to_string())?;
    newport_platform::filesystem::replace_file(&staged, path).map_err(|e| e.to_string())?;
    let _ = newport_platform::filesystem::sync_directory(directory);
    Ok(())
}

pub(super) fn remove(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => {
            if let Some(directory) = path.parent() {
                let _ = newport_platform::filesystem::sync_directory(directory);
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> Record {
        Record::new(
            "credential".into(),
            "device".into(),
            serde_json::json!({"state":"trial","email":"a@example.com","token":"v4.public.x","future":1}),
        )
    }

    #[test]
    fn round_trips_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        assert!(read(&path, &[7; 32]).unwrap().is_none());
        write(&path, &[7; 32], &record()).unwrap();
        assert_eq!(read(&path, &[7; 32]).unwrap(), Some(record()));
        let mut next = record();
        next.credential = "rotated".into();
        write(&path, &[7; 32], &next).unwrap();
        assert_eq!(
            read(&path, &[7; 32]).unwrap().unwrap().credential,
            "rotated"
        );
        remove(&path).unwrap();
        remove(&path).unwrap();
        assert!(read(&path, &[7; 32]).unwrap().is_none());
    }

    #[test]
    fn a_wrong_key_cannot_read_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        write(&path, &[7; 32], &record()).unwrap();
        assert!(read(&path, &[8; 32]).is_err());
    }
}
