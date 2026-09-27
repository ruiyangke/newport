//! Shared vault-key migration policy; platform adapters only read/write/generate keys.
use std::path::Path;
use zeroize::Zeroizing;
const MARKER: &str = "vault-key.newport.storage";
pub trait KeyStorage {
    fn modern(&self) -> Result<Option<Zeroizing<Vec<u8>>>, String>;
    fn legacy(&self) -> Result<Option<Zeroizing<Vec<u8>>>, String>;
    fn create(&self, key: &[u8]) -> Result<(), String>;
    fn generate(&self) -> Result<Zeroizing<Vec<u8>>, String>;
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
    if let Some(key) = storage.modern()? {
        let key = valid(key)?;
        if !directory.join(MARKER).exists() {
            if directory.join("profiles.stronghold").exists() {
                crate::vault::read(&directory.join("profiles.stronghold"), &key)?;
            }
            mark(directory)?;
        }
        return Ok(key);
    }
    // Never silently fall back to an old key after a successful transfer.
    if directory.join(MARKER).exists() {
        return Err("The Newport vault key is missing. Restore the original key; profiles have not been changed.".into());
    }
    let key = match storage.legacy()? {
        Some(key) => valid(key)?,
        None if directory.join("profiles.stronghold").exists() => {
            return Err(
                "Profile vault key is missing. Restore the original key to unlock your profiles."
                    .into(),
            );
        }
        None => storage.generate()?,
    };
    if directory.join("profiles.stronghold").exists() {
        crate::vault::read(&directory.join("profiles.stronghold"), &key)?;
    }
    storage.create(&key)?;
    let verified = storage
        .modern()?
        .ok_or("The new vault key could not be verified")?;
    if verified.as_slice() != key.as_slice() {
        return Err(
            "Vault key verification failed. The original key and profiles were preserved.".into(),
        );
    }
    mark(directory)?;
    // Retain the legacy item as a recovery copy; ordinary launches never read it again.
    eprintln!("Newport: vault key migration verified");
    Ok(key)
}
pub fn account_for_path(directory: &Path) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(directory.as_os_str().as_encoded_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn test_key() -> Result<Option<Zeroizing<Vec<u8>>>, String> {
    if super::data_directory_override().is_some() {
        if let Some(path) = std::env::var_os("NEWPORT_TEST_VAULT_KEY_FILE")
            .or_else(|| std::env::var_os(super::legacy::TEST_KEY_ENV))
        {
            return valid(Zeroizing::new(
                std::fs::read(path).map_err(|e| e.to_string())?,
            ))
            .map(Some);
        }
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
            account_for_path(Path::new(
                "/Users/test/Library/Application Support/com.porthop.app"
            )),
            "9270e56f575a05fedd2a451768eb360d5571955d3726c3cab0995b79c7bdbd91"
        );
    }
    struct Fake {
        modern: RefCell<Option<Vec<u8>>>,
        legacy: Option<Vec<u8>>,
        legacy_reads: Cell<usize>,
        writes: Cell<usize>,
        fail_write: bool,
        fail_read: bool,
    }
    impl Fake {
        fn new(legacy: Option<Vec<u8>>) -> Self {
            Self {
                modern: RefCell::new(None),
                legacy,
                legacy_reads: Cell::new(0),
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
        fn modern(&self) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
            if self.fail_read {
                return Err("Access unavailable".into());
            }
            Ok(self.modern.borrow().clone().map(Zeroizing::new))
        }
        fn legacy(&self) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
            self.legacy_reads.set(self.legacy_reads.get() + 1);
            Ok(self.legacy.clone().map(Zeroizing::new))
        }
        fn create(&self, key: &[u8]) -> Result<(), String> {
            self.writes.set(self.writes.get() + 1);
            if self.fail_write {
                return Err("Write rejected".into());
            }
            *self.modern.borrow_mut() = Some(key.to_vec());
            Ok(())
        }
    }
    #[test]
    fn transfer_preserves_key_and_future_launches_skip_legacy() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Fake::new(Some(vec![4; 32]));
        assert_eq!(*load(dir.path(), &storage).unwrap(), vec![4; 32]);
        assert!(dir.path().join(MARKER).exists());
        assert_eq!(*load(dir.path(), &storage).unwrap(), vec![4; 32]);
        assert_eq!(storage.legacy_reads.get(), 1);
        assert_eq!(storage.writes.get(), 1);
        assert_eq!(storage.legacy, Some(vec![4; 32]));
    }
    #[test]
    fn failed_transfer_retains_original_and_does_not_mark_success() {
        let dir = tempfile::tempdir().unwrap();
        let mut storage = Fake::new(Some(vec![4; 32]));
        storage.fail_write = true;
        assert!(load(dir.path(), &storage).is_err());
        assert!(!dir.path().join(MARKER).exists());
        assert_eq!(storage.legacy, Some(vec![4; 32]));
    }
    #[test]
    fn unavailable_or_missing_modern_key_never_downgrades() {
        let dir = tempfile::tempdir().unwrap();
        let mut storage = Fake::new(Some(vec![4; 32]));
        storage.fail_read = true;
        assert!(load(dir.path(), &storage).is_err());
        assert_eq!(storage.legacy_reads.get(), 0);
        storage.fail_read = false;
        mark(dir.path()).unwrap();
        assert!(load(dir.path(), &storage).is_err());
        assert_eq!(storage.legacy_reads.get(), 0);
        assert_eq!(storage.writes.get(), 0);
    }
    #[test]
    fn existing_profiles_never_get_a_replacement_key() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("profiles.stronghold"), b"preserve").unwrap();
        let storage = Fake::new(None);
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
        let storage = Fake::new(None);
        assert_eq!(load(dir.path(), &storage).unwrap().len(), 32);
        assert_eq!(storage.writes.get(), 1);
        assert!(dir.path().join(MARKER).exists());
    }
}
