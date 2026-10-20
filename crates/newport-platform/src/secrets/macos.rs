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
    service: String,
    account: String,
}
impl SystemStorage {
    pub fn new(service: &str, account: &str) -> Self {
        Self {
            service: service.into(),
            account: account.into(),
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
    fn read(&self) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        result(passwords::generic_password(options_for(
            &self.service,
            &self.account,
        )))
    }
    fn create(&self, key: &[u8]) -> Result<(), String> {
        let mut options = options_for(&self.service, &self.account);
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
    fn protected_keychain_stores_and_isolates_profile_keys() {
        const SERVICE: &str = "app.newport.tests.vault";
        let account = uuid::Uuid::new_v4().to_string();
        let other_account = uuid::Uuid::new_v4().to_string();
        let storage = SystemStorage::new(SERVICE, &account);
        let other = SystemStorage::new(SERVICE, &other_account);
        let mut owned = OwnedItems(vec![
            (SERVICE, account.clone()),
            (SERVICE, other_account.clone()),
        ]);

        assert!(storage.read().unwrap().is_none());
        assert!(other.read().unwrap().is_none());
        let original = storage.generate().unwrap();
        assert_eq!(original.len(), 32);
        storage.create(&original).unwrap();
        // Boolean assertions intentionally avoid including key bytes in failures.
        let reopened = SystemStorage::new(SERVICE, &account);
        assert!(reopened.read().unwrap().unwrap().as_slice() == original.as_slice());
        assert!(other.read().unwrap().is_none());

        let unrelated = other.generate().unwrap();
        other.create(&unrelated).unwrap();
        assert!(other.read().unwrap().unwrap().as_slice() == unrelated.as_slice());
        assert!(reopened.read().unwrap().unwrap().as_slice() == original.as_slice());

        owned.remove().expect("Remove only the owned fixture items");
        assert!(reopened.read().unwrap().is_none());
        assert!(other.read().unwrap().is_none());
    }
}
