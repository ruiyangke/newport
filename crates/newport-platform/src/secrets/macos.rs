//! Device-local items in the provisioned app's protected Keychain access group.
use security_framework::{
    access_control::{ProtectionMode, SecAccessControl},
    passwords::{self, PasswordOptions},
};
use zeroize::Zeroizing;

use super::Secrets;

const NOT_FOUND: i32 = -25300;
const MISSING_ENTITLEMENT: i32 = -34018;

fn options_for(service: &str, account: &str) -> PasswordOptions {
    let mut options = PasswordOptions::new_generic_password(service, account);
    options.use_protected_keychain();
    options.set_access_synchronized(Some(false));
    options
}

fn failure(action: &str, error: security_framework::base::Error) -> String {
    if error.code() == MISSING_ENTITLEMENT {
        "Newport needs a signed app bundle with a valid Mac provisioning profile to use the Keychain.".into()
    } else {
        format!("Cannot {action} the Keychain item: {error}")
    }
}

pub struct SystemStorage {
    service: String,
}
impl SystemStorage {
    pub fn new(service: &str) -> Self {
        Self {
            service: service.into(),
        }
    }
    fn add(&self, account: &str, value: &[u8]) -> security_framework::base::Result<()> {
        let mut options = options_for(&self.service, account);
        // Background reconnects remain possible after the user signs in. No biometric prompt.
        options.set_access_control(SecAccessControl::create_with_protection(
            Some(ProtectionMode::AccessibleAfterFirstUnlockThisDeviceOnly),
            0,
        )?);
        passwords::set_generic_password_options(value, options)
    }
    /// `SecItemUpdate` of the item's data, matched by class, service and
    /// account in the data protection keychain.
    fn update(&self, account: &str, value: &[u8]) -> i32 {
        use core_foundation::{
            base::{CFType, TCFType},
            boolean::CFBoolean,
            data::CFData,
            dictionary::CFDictionary,
            string::CFString,
        };
        use security_framework_sys::{item::*, keychain_item::SecItemUpdate};
        // SAFETY: the kSec* constants are static CFStrings; the get rule retains them.
        let key = |name| unsafe { CFString::wrap_under_get_rule(name) };
        let query: CFDictionary<CFString, CFType> = CFDictionary::from_CFType_pairs(&[
            (
                // SAFETY: reads a static CFString constant.
                key(unsafe { kSecClass }),
                // SAFETY: reads a static CFString constant.
                key(unsafe { kSecClassGenericPassword }).as_CFType(),
            ),
            (
                // SAFETY: reads a static CFString constant.
                key(unsafe { kSecAttrService }),
                CFString::new(&self.service).as_CFType(),
            ),
            (
                // SAFETY: reads a static CFString constant.
                key(unsafe { kSecAttrAccount }),
                CFString::new(account).as_CFType(),
            ),
            (
                // SAFETY: reads a static CFString constant.
                key(unsafe { kSecUseDataProtectionKeychain }),
                CFBoolean::true_value().as_CFType(),
            ),
        ]);
        let data = zeroize::Zeroizing::new(value.to_vec());
        let update: CFDictionary<CFString, CFType> = CFDictionary::from_CFType_pairs(&[(
            // SAFETY: reads a static CFString constant.
            key(unsafe { kSecValueData }),
            CFData::from_buffer(&data).as_CFType(),
        )]);
        // SAFETY: both dictionaries are live CFDictionaries for the call.
        unsafe { SecItemUpdate(query.as_concrete_TypeRef(), update.as_concrete_TypeRef()) }
    }
}
impl Secrets for SystemStorage {
    fn read(&self, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        match passwords::generic_password(options_for(&self.service, account)) {
            Ok(value) => Ok(Some(Zeroizing::new(value))),
            Err(error) if error.code() == NOT_FOUND => Ok(None),
            Err(error) => Err(failure("read", error)),
        }
    }
    fn write(&self, account: &str, value: &[u8]) -> Result<(), String> {
        // Replace in place, so no moment passes without the item. Add only when
        // there is none.
        match self.update(account, value) {
            0 => Ok(()),
            NOT_FOUND => self
                .add(account, value)
                .map_err(|error| failure("save", error)),
            status => Err(failure(
                "save",
                security_framework::base::Error::from_code(status),
            )),
        }
    }
    fn delete(&self, account: &str) -> Result<(), String> {
        match passwords::delete_generic_password_options(options_for(&self.service, account)) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == NOT_FOUND => Ok(()),
            Err(error) => Err(failure("remove", error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Owned<'a>(&'a SystemStorage, Vec<String>);
    impl Drop for Owned<'_> {
        fn drop(&mut self) {
            for account in &self.1 {
                let _ = self.0.delete(account);
            }
        }
    }

    #[test]
    #[ignore = "Requires a signed, provisioned app; writes and removes unique protected-Keychain fixtures"]
    fn protected_keychain_stores_replaces_and_removes_secrets() {
        let storage = SystemStorage::new("app.newport.tests");
        let account = uuid::Uuid::new_v4().to_string();
        let other = uuid::Uuid::new_v4().to_string();
        let _owned = Owned(&storage, vec![account.clone(), other.clone()]);

        assert!(storage.read(&account).unwrap().is_none());
        storage.write(&account, b"first").unwrap();
        // Boolean assertions intentionally avoid including secrets in failures.
        let reopened = SystemStorage::new("app.newport.tests");
        assert!(reopened.read(&account).unwrap().unwrap().as_slice() == b"first");
        assert!(reopened.read(&other).unwrap().is_none());
        storage.write(&account, b"replaced").unwrap();
        assert!(reopened.read(&account).unwrap().unwrap().as_slice() == b"replaced");
        storage.write(&other, b"unrelated").unwrap();
        assert!(reopened.read(&account).unwrap().unwrap().as_slice() == b"replaced");

        storage.delete(&account).unwrap();
        storage.delete(&account).unwrap();
        assert!(reopened.read(&account).unwrap().is_none());
        assert!(reopened.read(&other).unwrap().unwrap().as_slice() == b"unrelated");
    }
}
