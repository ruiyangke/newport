//! One device-local vault key, scoped to the provisioned app's Keychain access group.
use security_framework::{
    access_control::{ProtectionMode, SecAccessControl},
    passwords::{self, PasswordOptions},
};
use zeroize::Zeroizing;

use super::KeyStorage;

fn options_for(service: &str, account: &str) -> PasswordOptions {
    let mut options = PasswordOptions::new_generic_password(service, account);
    options.use_protected_keychain();
    options.set_access_synchronized(Some(false));
    options
}

pub struct SystemStorage {
    modern: String,
    legacy: String,
    account: String,
    legacy_protected: bool,
}
impl SystemStorage {
    pub fn new(modern: &str, legacy: &str, account: &str, legacy_protected: bool) -> Self {
        Self {
            modern: modern.into(),
            legacy: legacy.into(),
            account: account.into(),
            legacy_protected,
        }
    }
}
fn result(
    value: security_framework::base::Result<Vec<u8>>,
) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
    match value {
        Ok(key) => Ok(Some(Zeroizing::new(key))),
        Err(error) if error.code() == -25300 => Ok(None),
        Err(error) if error.code() == -34018 => Err("Newport needs a signed app bundle with a valid Mac provisioning profile to access its vault key.".into()),
        Err(error) => Err(format!("Cannot access the vault key: {error}")),
    }
}
impl KeyStorage for SystemStorage {
    fn generate(&self) -> Result<Zeroizing<Vec<u8>>, String> {
        let mut key = Zeroizing::new(vec![0; 32]);
        security_framework::random::SecRandom::default()
            .copy_bytes(&mut key)
            .map_err(|e| e.to_string())?;
        Ok(key)
    }
    fn modern(&self) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        result(passwords::generic_password(options_for(
            &self.modern,
            &self.account,
        )))
    }
    fn legacy(&self) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        // Requires the old Keychain access group in Newport's provisioning profile.
        if let Some(key) = result(passwords::generic_password(options_for(
            &self.legacy,
            &self.account,
        )))? {
            return Ok(Some(key));
        }
        if self.legacy_protected {
            return Err("Cannot access the existing vault key. Newport needs access to the previous app’s Keychain group; profiles were preserved.".into());
        }
        result(passwords::get_generic_password(&self.legacy, &self.account))
    }
    fn create(&self, key: &[u8]) -> Result<(), String> {
        let mut options = options_for(&self.modern, &self.account);
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

#[cfg(test)]
mod tests {
    use super::*;

    struct OwnedItems(Vec<(&'static str, String)>);

    impl OwnedItems {
        fn remove(&mut self) -> security_framework::base::Result<()> {
            while let Some((service, account)) = self.0.last() {
                match passwords::delete_generic_password_options(options_for(service, account)) {
                    Ok(()) => {}
                    Err(error) if error.code() == -25300 => {}
                    Err(error) => return Err(error),
                }
                self.0.pop();
            }
            Ok(())
        }
    }

    impl Drop for OwnedItems {
        fn drop(&mut self) {
            let _ = self.remove();
        }
    }

    #[test]
    #[ignore = "Requires a signed, provisioned app; writes and removes unique protected-Keychain fixtures"]
    fn protected_keychain_preserves_legacy_key_and_isolates_profiles() {
        const MODERN: &str = "app.newport.tests.vault-modern";
        const LEGACY: &str = "app.newport.tests.vault-legacy";
        let account = uuid::Uuid::new_v4().to_string();
        let other_account = uuid::Uuid::new_v4().to_string();
        let storage = SystemStorage::new(MODERN, LEGACY, &account, true);
        let other = SystemStorage::new(MODERN, LEGACY, &other_account, true);
        let legacy_writer = SystemStorage::new(LEGACY, LEGACY, &account, true);
        let mut owned = OwnedItems(vec![
            (MODERN, account.clone()),
            (LEGACY, account.clone()),
            (MODERN, other_account.clone()),
        ]);

        assert!(storage.modern().unwrap().is_none());
        assert!(other.modern().unwrap().is_none());
        // A protected legacy vault must never silently get a new encryption key
        // when the old item is absent or inaccessible.
        assert!(
            matches!(storage.legacy(), Err(error) if error.contains("profiles were preserved"))
        );
        let original = storage.generate().unwrap();
        assert_eq!(original.len(), 32);
        legacy_writer.create(&original).unwrap();
        // Boolean assertions intentionally avoid including key bytes in failures.
        assert!(storage.legacy().unwrap().unwrap().as_slice() == original.as_slice());
        assert!(storage.modern().unwrap().is_none());

        storage.create(&original).unwrap();
        let reopened = SystemStorage::new(MODERN, LEGACY, &account, true);
        assert!(reopened.modern().unwrap().unwrap().as_slice() == original.as_slice());
        assert!(reopened.legacy().unwrap().unwrap().as_slice() == original.as_slice());

        let unrelated = other.generate().unwrap();
        other.create(&unrelated).unwrap();
        assert!(other.modern().unwrap().unwrap().as_slice() == unrelated.as_slice());
        assert!(reopened.modern().unwrap().unwrap().as_slice() == original.as_slice());
        assert!(matches!(other.legacy(), Err(error) if error.contains("profiles were preserved")));

        owned.remove().expect("Remove only the owned fixture items");
        assert!(reopened.modern().unwrap().is_none());
        assert!(other.modern().unwrap().is_none());
        assert!(
            result(passwords::generic_password(options_for(LEGACY, &account)))
                .unwrap()
                .is_none()
        );
        assert!(
            matches!(reopened.legacy(), Err(error) if error.contains("profiles were preserved"))
        );
    }
}
