//! A secret sealed with the profile's master key: XChaCha20-Poly1305 with a
//! random nonce, bound to what it is (`purpose`), so a sealed value can't be
//! moved to another server or field. Text form: `v1.` and base64url of the
//! nonce followed by the ciphertext.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use zeroize::Zeroizing;

const VERSION: &str = "v1.";
const NONCE: usize = 24;

pub(super) fn password_purpose(id: uuid::Uuid) -> String {
    format!("newport/ssh-password/{id}")
}
pub(super) const ACCOUNT_PURPOSE: &str = "newport/account";

pub(super) fn seal(key: &[u8], purpose: &str, plain: &[u8]) -> Result<String, String> {
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|_| "Invalid Newport key")?;
    let mut nonce = [0; NONCE];
    getrandom::fill(&mut nonce).map_err(|error| format!("Cannot encrypt: {error}"))?;
    let sealed = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plain,
                aad: purpose.as_bytes(),
            },
        )
        .map_err(|_| "Cannot encrypt")?;
    let mut bytes = nonce.to_vec();
    bytes.extend(sealed);
    Ok(format!("{VERSION}{}", URL_SAFE_NO_PAD.encode(bytes)))
}

pub(super) fn open(key: &[u8], purpose: &str, sealed: &str) -> Result<Zeroizing<Vec<u8>>, String> {
    const DAMAGED: &str = "A saved secret is damaged or was saved with another Newport key.";
    let bytes = sealed
        .strip_prefix(VERSION)
        .and_then(|text| URL_SAFE_NO_PAD.decode(text).ok())
        .filter(|bytes| bytes.len() > NONCE)
        .ok_or(DAMAGED)?;
    let (nonce, ciphertext) = bytes.split_at(NONCE);
    XChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| "Invalid Newport key")?
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: purpose.as_bytes(),
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| DAMAGED.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sealed_secret_opens_only_with_its_key_and_purpose() {
        let sealed = seal(&[7; 32], "newport/account", b"secret").unwrap();
        assert!(sealed.starts_with("v1."));
        assert!(!sealed.contains("secret"));
        assert_eq!(
            open(&[7; 32], "newport/account", &sealed)
                .unwrap()
                .as_slice(),
            b"secret"
        );
        assert!(open(&[8; 32], "newport/account", &sealed).is_err());
        assert!(open(&[7; 32], "newport/ssh-password/x", &sealed).is_err());
        for damaged in ["", "v1.", "v2.AAAA", "v1.!!!!", &sealed[..sealed.len() - 2]] {
            assert!(
                open(&[7; 32], "newport/account", damaged).is_err(),
                "{damaged}"
            );
        }
        // A fresh nonce each time: the same secret never seals the same way.
        assert_ne!(
            sealed,
            seal(&[7; 32], "newport/account", b"secret").unwrap()
        );
    }
}
