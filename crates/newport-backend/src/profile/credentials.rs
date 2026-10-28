//! Vault-key policy; platform adapters only read, write and generate keys.
use std::path::Path;
use zeroize::Zeroizing;
/// Written once a vault key has been created or verified for this profile.
/// A missing key is then an error, never a reason to generate a new one.
const MARKER: &str = "vault-key.newport.storage";
pub use newport_platform::secrets::KeyStorage;

/// The OS namespace and per-profile account hash for the vault key.
/// Unsupported platforms must never substitute plaintext or a generated key.
pub fn system_key(directory: &Path) -> Result<Zeroizing<Vec<u8>>, String> {
    #[cfg(feature = "test-support")]
    if let Some(key) = test_key()? {
        return Ok(key);
    }
    #[cfg(any(target_os = "macos", windows))]
    {
        let directory = directory
            .canonicalize()
            .map_err(|error| error.to_string())?;
        load(&directory, &system_storage(&directory))
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = directory;
        Err("System vault storage is unavailable on this platform.".into())
    }
}
pub fn valid(key: Zeroizing<Vec<u8>>) -> Result<Zeroizing<Vec<u8>>, String> {
    if key.len() == 32 {
        Ok(key)
    } else {
        Err("Invalid profile vault key; saved profiles were not changed.".into())
    }
}
fn mark(directory: &Path) -> Result<(), String> {
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new_in(directory).map_err(|e| e.to_string())?;
    file.write_all(b"newport-v1\n").map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(directory.join(MARKER))
        .map_err(|e| e.to_string())?;
    Ok(())
}
pub fn load(directory: &Path, storage: &impl KeyStorage) -> Result<Zeroizing<Vec<u8>>, String> {
    let vault = directory.join("profiles.stronghold");
    if let Some(key) = storage.read()? {
        let key = valid(key)?;
        if !directory.join(MARKER).exists() {
            if vault.exists() {
                super::vault::read(&vault, &key)?;
            }
            mark(directory)?;
        }
        return Ok(key);
    }
    // Never replace a key that protected data in this profile.
    if directory.join(MARKER).exists() || vault.exists() {
        return Err("The Newport vault key is missing. Restore the original key; profiles have not been changed.".into());
    }
    let key = storage.generate()?;
    storage.create(&key)?;
    let verified = storage
        .read()?
        .ok_or("The new vault key could not be verified")?;
    if verified.as_slice() != key.as_slice() {
        return Err("Vault key verification failed; profiles have not been changed.".into());
    }
    mark(directory)?;
    Ok(key)
}
/// The OS credential entry for the vault key of a canonical profile directory.
#[cfg(any(target_os = "macos", windows))]
pub fn system_storage(directory: &Path) -> newport_platform::secrets::SystemStorage {
    let account = account_for_path(directory);
    #[cfg(target_os = "macos")]
    let storage =
        newport_platform::secrets::SystemStorage::new("app.newport.profile-vault", &account);
    #[cfg(windows)]
    let storage = newport_platform::secrets::SystemStorage::new("app.newport/vault", &account);
    storage
}
pub fn account_for_path(directory: &Path) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(directory.as_os_str().as_encoded_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Isolated test profiles may read their key from a file instead of the OS store.
#[cfg(feature = "test-support")]
pub fn test_key() -> Result<Option<Zeroizing<Vec<u8>>>, String> {
    if super::data_directory_override().is_some()
        && let Some(path) = std::env::var_os("NEWPORT_TEST_VAULT_KEY_FILE")
    {
        return valid(Zeroizing::new(
            std::fs::read(path).map_err(|e| e.to_string())?,
        ))
        .map(Some);
    }
    Ok(None)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    #[test]
    fn account_hash_is_stable_across_crypto_upgrades() {
        assert_eq!(
            account_for_path(Path::new("/Users/test/Library/Application Support/Newport")),
            "c70d210acb98917454d7b7e9f149173f8f167d2dc7d18cceb9d125adb22e93bf"
        );
    }
    struct Fake {
        saved: RefCell<Option<Vec<u8>>>,
        writes: Cell<usize>,
        fail_write: bool,
        fail_read: bool,
    }
    impl Fake {
        fn new() -> Self {
            Self {
                saved: RefCell::new(None),
                writes: Cell::new(0),
                fail_write: false,
                fail_read: false,
            }
        }
    }
    impl KeyStorage for Fake {
        fn generate(&self) -> Result<Zeroizing<Vec<u8>>, String> {
            Ok(Zeroizing::new(vec![7; 32]))
        }
        fn read(&self) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
            if self.fail_read {
                return Err("Access unavailable".into());
            }
            Ok(self.saved.borrow().clone().map(Zeroizing::new))
        }
        fn create(&self, key: &[u8]) -> Result<(), String> {
            self.writes.set(self.writes.get() + 1);
            if self.fail_write {
                return Err("Write rejected".into());
            }
            *self.saved.borrow_mut() = Some(key.to_vec());
            Ok(())
        }
    }
    #[test]
    fn failed_write_does_not_mark_a_key() {
        let dir = tempfile::tempdir().unwrap();
        let mut storage = Fake::new();
        storage.fail_write = true;
        assert!(load(dir.path(), &storage).is_err());
        assert!(!dir.path().join(MARKER).exists());
    }
    #[test]
    fn unavailable_or_missing_key_is_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let mut storage = Fake::new();
        storage.fail_read = true;
        assert!(load(dir.path(), &storage).is_err());
        storage.fail_read = false;
        mark(dir.path()).unwrap();
        assert!(load(dir.path(), &storage).is_err());
        assert_eq!(storage.writes.get(), 0);
    }
    #[test]
    fn existing_profiles_never_get_a_replacement_key() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("profiles.stronghold"), b"preserve").unwrap();
        let storage = Fake::new();
        assert!(load(dir.path(), &storage).is_err());
        assert_eq!(storage.writes.get(), 0);
        assert_eq!(
            std::fs::read(dir.path().join("profiles.stronghold")).unwrap(),
            b"preserve"
        );
    }
    #[test]
    fn fresh_install_creates_and_verifies_a_random_key() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Fake::new();
        assert_eq!(load(dir.path(), &storage).unwrap().len(), 32);
        assert_eq!(storage.writes.get(), 1);
        assert!(dir.path().join(MARKER).exists());
        assert_eq!(*load(dir.path(), &storage).unwrap(), vec![7; 32]);
        assert_eq!(storage.writes.get(), 1);
    }
}
