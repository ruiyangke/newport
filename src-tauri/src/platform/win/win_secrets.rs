//! The per-profile encryption key is persisted in the current user's Credential Manager.
use sha2::{Digest, Sha256};
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

pub fn vault_key(directory: &Path) -> Result<Zeroizing<Vec<u8>>, String> {
    if std::env::var_os("PORTHOP_DATA_DIR").is_some() {
        if let Some(path) = std::env::var_os("PORTHOP_TEST_VAULT_KEY_FILE") {
            let key = Zeroizing::new(std::fs::read(path).map_err(|e| e.to_string())?);
            return if key.len() == 32 {
                Ok(key)
            } else {
                Err("Invalid test vault key".into())
            };
        }
    }
    let directory = directory.canonicalize().map_err(|e| e.to_string())?;
    let hash = Sha256::digest(directory.as_os_str().as_encoded_bytes());
    let suffix: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
    let name = format!("ke.ry.porthop/vault/{suffix}");
    let target = super::wide(std::ffi::OsStr::new(&name));
    if let Some(key) = read(&target)? {
        return Ok(key);
    }
    if directory.join("profiles.stronghold").exists() {
        return Err("The saved vault key is missing from Windows Credential Manager. Restore the original key to unlock your profiles.".into());
    }
    let mut key = Zeroizing::new(vec![0; 32]);
    unsafe {
        if BCryptGenRandom(
            ptr::null_mut(),
            key.as_mut_ptr(),
            32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        ) < 0
        {
            return Err("Cannot generate a secure vault key".into());
        }
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_ptr().cast_mut(),
            CredentialBlobSize: 32,
            CredentialBlob: key.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            ..Default::default()
        };
        if CredWriteW(&credential, 0) == 0 {
            return Err(format!(
                "Cannot save the vault key in Windows Credential Manager: {}",
                GetLastError()
            ));
        }
    }
    let verified = read(&target)?.ok_or("The saved vault key could not be verified")?;
    if verified.as_slice() != key.as_slice() {
        return Err("Vault key verification failed".into());
    }
    Ok(verified)
}
