//! Generic credentials in the current user's Credential Manager, named
//! `<service>/<account>`.
use super::Secrets;
use std::ptr;
use windows_sys::Win32::{
    Foundation::{ERROR_NOT_FOUND, GetLastError},
    Security::Credentials::*,
};
use zeroize::Zeroizing;

/// The largest value a generic credential holds.
const MAX_VALUE: usize = CRED_MAX_CREDENTIAL_BLOB_SIZE as usize;

pub struct SystemStorage {
    service: String,
}
impl SystemStorage {
    pub fn new(service: &str) -> Self {
        Self {
            service: service.into(),
        }
    }
    fn target(&self, account: &str) -> Vec<u16> {
        format!("{}/{account}", self.service)
            .encode_utf16()
            .chain(Some(0))
            .collect()
    }
}
impl Secrets for SystemStorage {
    fn read(&self, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        let target = self.target(account);
        let mut credential = ptr::null_mut();
        // SAFETY: `target` is NUL-terminated. On success CredReadW returns a credential
        // that stays valid, and whose blob is `CredentialBlobSize` bytes, until CredFree.
        unsafe {
            if CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) == 0 {
                return match GetLastError() {
                    ERROR_NOT_FOUND => Ok(None),
                    code => Err(format!(
                        "Cannot read from Windows Credential Manager: {code}"
                    )),
                };
            }
            let entry = &*credential;
            let size = entry.CredentialBlobSize as usize;
            let value = if size == 0 || entry.CredentialBlob.is_null() {
                Zeroizing::new(Vec::new())
            } else {
                Zeroizing::new(std::slice::from_raw_parts(entry.CredentialBlob, size).to_vec())
            };
            // CredFree does not wipe the secret.
            if !entry.CredentialBlob.is_null() {
                for offset in 0..size {
                    ptr::write_volatile(entry.CredentialBlob.add(offset), 0);
                }
            }
            CredFree(credential.cast());
            Ok(Some(value))
        }
    }
    fn write(&self, account: &str, value: &[u8]) -> Result<(), String> {
        if value.len() > MAX_VALUE {
            return Err(format!(
                "Cannot save {} bytes in Windows Credential Manager; it holds at most {MAX_VALUE}.",
                value.len()
            ));
        }
        let target = self.target(account);
        let mut value = Zeroizing::new(value.to_vec());
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_ptr().cast_mut(),
            CredentialBlobSize: value.len() as u32,
            CredentialBlob: value.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            ..Default::default()
        };
        // CredWriteW replaces an existing credential with the same target.
        // SAFETY: `credential` and the buffers it points to outlive the call, which
        // copies them.
        if unsafe { CredWriteW(&credential, 0) } == 0 {
            return Err(format!(
                "Cannot save in Windows Credential Manager: {}",
                // SAFETY: reads the calling thread's last error, set by the call above.
                unsafe { GetLastError() }
            ));
        }
        Ok(())
    }
    fn delete(&self, account: &str) -> Result<(), String> {
        let target = self.target(account);
        // SAFETY: `target` is NUL-terminated.
        if unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
            // SAFETY: reads the calling thread's last error, set by the call above.
            let code = unsafe { GetLastError() };
            if code != ERROR_NOT_FOUND {
                return Err(format!(
                    "Cannot remove from Windows Credential Manager: {code}"
                ));
            }
        }
        Ok(())
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
    #[ignore = "Writes and removes uniquely named fixture entries in Windows Credential Manager"]
    fn credential_manager_stores_replaces_and_removes_secrets() {
        let storage = SystemStorage::new("app.newport.tests");
        let account = uuid::Uuid::new_v4().to_string();
        let other = uuid::Uuid::new_v4().to_string();
        let _owned = Owned(&storage, vec![account.clone(), other.clone()]);

        assert!(storage.read(&account).unwrap().is_none());
        storage.write(&account, b"first").unwrap();
        // A new adapter must read the persisted bytes.
        let reopened = SystemStorage::new("app.newport.tests");
        assert_eq!(
            reopened.read(&account).unwrap().unwrap().as_slice(),
            b"first"
        );
        assert!(reopened.read(&other).unwrap().is_none());
        storage.write(&account, b"replaced").unwrap();
        assert_eq!(
            reopened.read(&account).unwrap().unwrap().as_slice(),
            b"replaced"
        );
        storage.write(&other, b"unrelated").unwrap();
        assert!(storage.write(&other, &vec![1; MAX_VALUE + 1]).is_err());
        assert_eq!(
            reopened.read(&other).unwrap().unwrap().as_slice(),
            b"unrelated"
        );

        storage.delete(&account).unwrap();
        storage.delete(&account).unwrap();
        assert!(reopened.read(&account).unwrap().is_none());
        assert_eq!(
            reopened.read(&other).unwrap().unwrap().as_slice(),
            b"unrelated"
        );
    }
}
