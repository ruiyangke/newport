//! One device-local vault key, scoped to the provisioned app's Keychain access group.
use security_framework::{
    access_control::{ProtectionMode, SecAccessControl},
    passwords::{self, PasswordOptions},
};
use std::path::Path;
use zeroize::Zeroizing;

const SERVICE: &str = "com.porthop.profile-vault";
const MARKER: &str = "vault-key.storage";

fn options(account: &str) -> PasswordOptions {
    let mut options = PasswordOptions::new_generic_password(SERVICE, account);
    options.use_protected_keychain();
    options.set_access_synchronized(Some(false));
    options
}

trait KeyStorage {
    fn modern(&self) -> Result<Option<Zeroizing<Vec<u8>>>, String>;
    fn legacy(&self) -> Result<Option<Zeroizing<Vec<u8>>>, String>;
    fn create(&self, key: &[u8]) -> Result<(), String>;
}
struct SystemStorage<'a>(&'a str);
fn result(
    value: security_framework::base::Result<Vec<u8>>,
) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
    match value {
        Ok(key) => Ok(Some(Zeroizing::new(key))),
        Err(error) if error.code() == -25300 => Ok(None),
        Err(error) if error.code() == -34018 => Err("Porthop needs a signed app bundle with a valid Mac provisioning profile to access its vault key.".into()),
        Err(error) => Err(format!("Cannot access the vault key: {error}")),
    }
}
impl KeyStorage for SystemStorage<'_> {
    fn modern(&self) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        result(passwords::generic_password(options(self.0)))
    }
    fn legacy(&self) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        result(passwords::get_generic_password(SERVICE, self.0))
    }
    fn create(&self, key: &[u8]) -> Result<(), String> {
        let mut options = options(self.0);
        // Background reconnects remain possible after the user signs in. No biometric prompt.
        let access = SecAccessControl::create_with_protection(
            Some(ProtectionMode::AccessibleAfterFirstUnlockThisDeviceOnly),
            0,
        )
        .map_err(|e| e.to_string())?;
        options.set_access_control(access);
        passwords::set_generic_password_options(key, options)
            .map_err(|e| format!("Cannot protect the vault key: {e}"))
    }
}
fn valid(key: Zeroizing<Vec<u8>>) -> Result<Zeroizing<Vec<u8>>, String> {
    if key.len() == 32 {
        Ok(key)
    } else {
        Err("Invalid profile vault key; saved profiles were not changed.".into())
    }
}
fn mark(directory: &Path) -> Result<(), String> {
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new_in(directory).map_err(|e| e.to_string())?;
    file.write_all(b"data-protection-v1\n")
        .map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(directory.join(MARKER))
        .map_err(|e| e.to_string())?;
    Ok(())
}
fn load(directory: &Path, storage: &impl KeyStorage) -> Result<Zeroizing<Vec<u8>>, String> {
    if let Some(key) = storage.modern()? {
        let key = valid(key)?;
        if !directory.join(MARKER).exists() {
            mark(directory)?;
        }
        return Ok(key);
    }
    // Never silently fall back to an old key after a successful transfer.
    if directory.join(MARKER).exists() {
        return Err("The Data Protection Keychain vault key is missing. Restore the original key; profiles have not been changed.".into());
    }
    let key = match storage.legacy()? {
        Some(key) => valid(key)?,
        None if directory.join("profiles.stronghold").exists() => {
            return Err(
                "Profile vault key is missing. Restore the original key to unlock your profiles."
                    .into(),
            );
        }
        None => {
            let mut key = Zeroizing::new(vec![0; 32]);
            security_framework::random::SecRandom::default()
                .copy_bytes(&mut key)
                .map_err(|e| e.to_string())?;
            key
        }
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
    eprintln!("Porthop: vault key verified in Data Protection Keychain");
    Ok(key)
}
fn account_for_path(directory: &Path) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(directory.as_os_str().as_encoded_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn vault_key(directory: &Path) -> Result<Zeroizing<Vec<u8>>, String> {
    if std::env::var_os("PORTHOP_DATA_DIR").is_some() {
        if let Some(path) = std::env::var_os("PORTHOP_TEST_VAULT_KEY_FILE") {
            return valid(Zeroizing::new(
                std::fs::read(path).map_err(|e| e.to_string())?,
            ));
        }
    }
    let directory = directory.canonicalize().map_err(|e| e.to_string())?;
    let account = account_for_path(&directory);
    load(&directory, &SystemStorage(&account))
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
