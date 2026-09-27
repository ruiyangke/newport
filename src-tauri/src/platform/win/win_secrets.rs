//! The per-profile encryption key is persisted in the current user's Credential Manager.
use crate::migration::{
    credentials::{self, KeyStorage},
    legacy,
};
use std::{path::Path, ptr};
use windows_sys::Win32::{
    Foundation::{GetLastError, ERROR_NOT_FOUND},
    Security::{
        Credentials::*,
        Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG},
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

struct SystemStorage {
    target: Vec<u16>,
    legacy_target: Vec<u16>,
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
pub fn vault_key(directory: &Path) -> Result<Zeroizing<Vec<u8>>, String> {
    if let Some(key) = credentials::test_key()? {
        return Ok(key);
    }
    let directory = directory.canonicalize().map_err(|e| e.to_string())?;
    let suffix = credentials::account_for_path(&directory);
    let target = super::wide(std::ffi::OsStr::new(&format!("app.newport/vault/{suffix}")));
    let legacy_target = super::wide(std::ffi::OsStr::new(&format!(
        "{}/vault/{suffix}",
        legacy::APP_ID
    )));
    credentials::load(
        &directory,
        &SystemStorage {
            target,
            legacy_target,
        },
    )
}
