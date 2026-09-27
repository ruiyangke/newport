//! One device-local vault key, scoped to the provisioned app's Keychain access group.
use security_framework::{
    access_control::{ProtectionMode, SecAccessControl},
    passwords::{self, PasswordOptions},
};
use std::path::Path;
use zeroize::Zeroizing;

const SERVICE: &str = "app.newport.profile-vault";
use crate::migration::{
    credentials::{self, KeyStorage},
    legacy::MAC_VAULT_SERVICE as LEGACY_SERVICE,
};

fn options_for(service: &str, account: &str) -> PasswordOptions {
    let mut options = PasswordOptions::new_generic_password(service, account);
    options.use_protected_keychain();
    options.set_access_synchronized(Some(false));
    options
}

struct SystemStorage<'a> {
    account: &'a str,
    legacy_protected: bool,
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
impl KeyStorage for SystemStorage<'_> {
    fn generate(&self) -> Result<Zeroizing<Vec<u8>>, String> {
        let mut key = Zeroizing::new(vec![0; 32]);
        security_framework::random::SecRandom::default()
            .copy_bytes(&mut key)
            .map_err(|e| e.to_string())?;
        Ok(key)
    }
    fn modern(&self) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        result(passwords::generic_password(options_for(
            SERVICE,
            self.account,
        )))
    }
    fn legacy(&self) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        // Requires the old Keychain access group in Newport's provisioning profile.
        if let Some(key) = result(passwords::generic_password(options_for(
            LEGACY_SERVICE,
            self.account,
        )))? {
            return Ok(Some(key));
        }
        if self.legacy_protected {
            return Err("Cannot access the existing vault key. Newport needs access to the previous app’s Keychain group; profiles were preserved.".into());
        }
        result(passwords::get_generic_password(
            LEGACY_SERVICE,
            self.account,
        ))
    }
    fn create(&self, key: &[u8]) -> Result<(), String> {
        let mut options = options_for(SERVICE, self.account);
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
pub fn vault_key(directory: &Path) -> Result<Zeroizing<Vec<u8>>, String> {
    if let Some(key) = credentials::test_key()? {
        return Ok(key);
    }
    let directory = directory.canonicalize().map_err(|e| e.to_string())?;
    let account = credentials::account_for_path(&directory);
    credentials::load(
        &directory,
        &SystemStorage {
            account: &account,
            legacy_protected: directory
                .join(crate::migration::legacy::PROTECTED_KEY_MARKER)
                .exists(),
        },
    )
}
