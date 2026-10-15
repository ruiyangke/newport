//! The per-profile encryption key is persisted in the current user's Credential Manager.
use super::KeyStorage;
use std::ptr;
use windows_sys::Win32::{
    Foundation::{ERROR_NOT_FOUND, GetLastError},
    Security::{
        Credentials::*,
        Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom},
    },
};
use zeroize::Zeroizing;

fn read(target: &[u16]) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
    let mut credential = ptr::null_mut();
    unsafe {
        if CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) == 0 {
            return match GetLastError() {
                ERROR_NOT_FOUND => Ok(None),
                code => Err(format!(
                    "Cannot read the vault key from Windows Credential Manager: {code}"
                )),
            };
        }
        let entry = &*credential;
        let result = if entry.CredentialBlobSize != 32 || entry.CredentialBlob.is_null() {
            Err("Invalid vault key; saved profiles were not changed.".into())
        } else {
            Ok(Some(Zeroizing::new(
                std::slice::from_raw_parts(entry.CredentialBlob, 32).to_vec(),
            )))
        };
        CredFree(credential.cast());
        result
    }
}

pub struct SystemStorage {
    target: Vec<u16>,
    legacy_target: Vec<u16>,
}
impl SystemStorage {
    pub fn new(modern: &str, legacy: &str, account: &str, _legacy_protected: bool) -> Self {
        let wide = |namespace: &str| {
            format!("{namespace}/{account}")
                .encode_utf16()
                .chain(Some(0))
                .collect()
        };
        Self {
            target: wide(modern),
            legacy_target: wide(legacy),
        }
    }
}
impl KeyStorage for SystemStorage {
    fn modern(&self) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        read(&self.target)
    }
    fn legacy(&self) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        read(&self.legacy_target)
    }
    fn generate(&self) -> Result<Zeroizing<Vec<u8>>, String> {
        let mut key = Zeroizing::new(vec![0; 32]);
        if unsafe {
            BCryptGenRandom(
                ptr::null_mut(),
                key.as_mut_ptr(),
                32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        } < 0
        {
            return Err("Cannot generate a secure vault key".into());
        }
        Ok(key)
    }
    fn create(&self, key: &[u8]) -> Result<(), String> {
        let mut key = Zeroizing::new(key.to_vec());
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: self.target.as_ptr().cast_mut(),
            CredentialBlobSize: key.len() as u32,
            CredentialBlob: key.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            ..Default::default()
        };
        if unsafe { CredWriteW(&credential, 0) } == 0 {
            return Err(format!(
                "Cannot save the vault key in Windows Credential Manager: {}",
                unsafe { GetLastError() }
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct OwnedTargets(Vec<Vec<u16>>);
    impl OwnedTargets {
        fn remove(&mut self) -> Result<(), u32> {
            for target in &self.0 {
                if unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
                    let code = unsafe { GetLastError() };
                    if code != ERROR_NOT_FOUND {
                        return Err(code);
                    }
                }
            }
            self.0.clear();
            Ok(())
        }
    }
    impl Drop for OwnedTargets {
        fn drop(&mut self) {
            let _ = self.remove();
        }
    }

    #[test]
    #[ignore = "Writes and removes uniquely named fixture entries in Windows Credential Manager"]
    fn credential_manager_preserves_legacy_key_and_isolates_profiles() {
        let account = uuid::Uuid::new_v4().to_string();
        let other_account = uuid::Uuid::new_v4().to_string();
        let modern = "app.newport.tests/vault-modern";
        let legacy = "app.newport.tests/vault-legacy";
        let storage = SystemStorage::new(modern, legacy, &account, false);
        let other = SystemStorage::new(modern, legacy, &other_account, false);
        let legacy_writer = SystemStorage::new(legacy, legacy, &account, false);
        let mut owned = OwnedTargets(vec![
            storage.target.clone(),
            storage.legacy_target.clone(),
            other.target.clone(),
        ]);
        assert!(storage.modern().unwrap().is_none());
        assert!(storage.legacy().unwrap().is_none());
        assert!(other.modern().unwrap().is_none());
        let original = storage.generate().unwrap();
        assert_eq!(original.len(), 32);
        legacy_writer.create(&original).unwrap();
        assert_eq!(
            storage.legacy().unwrap().unwrap().as_slice(),
            original.as_slice()
        );
        assert!(storage.modern().unwrap().is_none());

        // Migration copies the key into the new namespace and retains the old
        // item for rollback. A new adapter must read the persisted bytes.
        storage.create(&original).unwrap();
        let reopened = SystemStorage::new(modern, legacy, &account, false);
        assert_eq!(
            reopened.modern().unwrap().unwrap().as_slice(),
            original.as_slice()
        );
        assert_eq!(
            reopened.legacy().unwrap().unwrap().as_slice(),
            original.as_slice()
        );
        let unrelated = other.generate().unwrap();
        other.create(&unrelated).unwrap();
        assert_eq!(
            other.modern().unwrap().unwrap().as_slice(),
            unrelated.as_slice()
        );
        assert_eq!(
            reopened.modern().unwrap().unwrap().as_slice(),
            original.as_slice()
        );
        assert!(other.legacy().unwrap().is_none());

        owned
            .remove()
            .expect("Remove only the fixture credential targets");
        assert!(reopened.modern().unwrap().is_none());
        assert!(reopened.legacy().unwrap().is_none());
        assert!(other.modern().unwrap().is_none());
    }
}
