//! Host-key verification with OpenSSH-compatible known_hosts records.
use anyhow::{bail, Context, Result};
use fs2::FileExt;
use hmac::{Hmac, KeyInit, Mac};
use russh::keys::ssh_key::{
    known_hosts::{HostPatterns, KnownHosts, Marker},
    PublicKey,
};
use sha1::Sha1;
use std::{
    fs::OpenOptions,
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

pub fn user_path() -> Result<PathBuf> {
    #[cfg(test)]
    if let Ok(path) = std::env::var("PORTHOP_TEST_KNOWN_HOSTS") {
        return Ok(path.into());
    }
    Ok(dirs::home_dir()
        .context("Cannot locate home directory")?
        .join(".ssh/known_hosts"))
}
fn wildcard(pattern: &[u8], value: &[u8]) -> bool {
    let (mut p, mut v, mut star, mut resume) = (0, 0, None, 0);
    while v < value.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p].eq_ignore_ascii_case(&value[v])) {
            p += 1;
            v += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            p += 1;
            resume = v;
        } else if let Some(s) = star {
            resume += 1;
            v = resume;
            p = s + 1;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}
fn matches(patterns: &HostPatterns, host: &str) -> bool {
    match patterns {
        HostPatterns::HashedName { salt, hash } => {
            Hmac::<Sha1>::new_from_slice(salt).is_ok_and(|mut mac| {
                mac.update(host.as_bytes());
                mac.verify_slice(hash).is_ok()
            })
        }
        HostPatterns::Patterns(patterns) => {
            let mut matched = false;
            for pattern in patterns {
                if let Some(negative) = pattern.strip_prefix('!') {
                    if wildcard(negative.as_bytes(), host.as_bytes()) {
                        return false;
                    }
                } else if wildcard(pattern.as_bytes(), host.as_bytes()) {
                    matched = true;
                }
            }
            matched
        }
    }
}
fn inspect(text: &str, host: &str, key: &PublicKey) -> Result<(bool, bool)> {
    let (mut found, mut trusted) = (false, false);
    for entry in KnownHosts::new(text) {
        let entry = entry.context("Invalid known_hosts entry; file was preserved")?;
        if !matches(entry.host_patterns(), host) {
            continue;
        }
        let same = entry.public_key().key_data() == key.key_data();
        if entry.marker() == Some(&Marker::Revoked) {
            if same {
                bail!("The SSH host key for {host} is revoked.");
            }
            continue;
        }
        found = true;
        if entry.marker() == Some(&Marker::CertAuthority) {
            continue;
        }
        trusted |= same;
    }
    Ok((found, trusted))
}
pub fn verify(
    host: &str,
    port: u16,
    key: &PublicKey,
    path: &Path,
    system: Option<&Path>,
) -> Result<()> {
    let host = host.to_ascii_lowercase();
    let host = if port == 22 {
        host.to_owned()
    } else {
        format!("[{host}]:{port}")
    };
    let mut system_result = (false, false);
    if let Some(system) = system {
        match std::fs::read_to_string(system) {
            Ok(text) => system_result = inspect(&text, &host, key)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).context("Cannot read system known_hosts"),
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
    }
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .context("Cannot access known_hosts; refusing unverified SSH connection")?;
    file.lock_exclusive()?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    let local = inspect(&text, &host, key)?;
    if local.1 || system_result.1 {
        return Ok(());
    }
    if local.0 || system_result.0 {
        bail!("SSH host key changed or requires a certificate for {host}. Verify the server and update known_hosts before reconnecting.");
    }
    // Trust-on-first-use, matching the previous accept-new policy.
    file.seek(SeekFrom::End(0))?;
    if !text.is_empty() && !text.ends_with('\n') {
        file.write_all(b"\n")?;
    }
    writeln!(file, "{host} {}", key.to_openssh()?)?;
    file.sync_all()?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn key() -> PublicKey {
        russh::keys::load_secret_key(std::env::var("PORTHOP_TEST_IDENTITY").unwrap(), None)
            .unwrap()
            .public_key()
            .clone()
    }
    #[test]
    fn patterns_respect_ports_and_negation() {
        let p: HostPatterns = "*.example.com,!private.example.com".parse().unwrap();
        assert!(matches(&p, "dev.example.com"));
        assert!(!matches(&p, "private.example.com"));
        assert!(matches(
            &"[127.0.0.1]:2222".parse().unwrap(),
            "[127.0.0.1]:2222"
        ));
    }
    #[test]
    fn hashed_hostnames_match_only_the_original_host() {
        let salt = b"fixture-salt".to_vec();
        // Fixed independently generated HMAC-SHA1 vector: preserve OpenSSH compatibility
        // across crypto dependency upgrades instead of generating both sides here.
        let hash = [
            75, 2, 196, 62, 97, 207, 205, 67, 95, 204, 219, 221, 231, 157, 113, 82, 38, 228, 182,
            242,
        ];
        let patterns = HostPatterns::HashedName { salt, hash };
        assert!(matches(&patterns, "[host.example.com]:2222"));
        assert!(!matches(&patterns, "[other.example.com]:2222"));
    }
    #[test]
    #[ignore = "Uses the ephemeral key created by scripts/test-ssh.sh"]
    fn host_key_trust_and_revocation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        let key = key();
        verify("host", 22, &key, &path, None).unwrap();
        let before = std::fs::read(&path).unwrap();
        verify("host", 22, &key, &path, None).unwrap();
        assert_eq!(before, std::fs::read(&path).unwrap());
        let other = russh::keys::ssh_key::PublicKey::read_openssh_file(
            std::env::var("PORTHOP_TEST_OTHER_KEY").unwrap(),
        )
        .unwrap();
        assert!(verify("host", 22, &other, &path, None)
            .unwrap_err()
            .to_string()
            .contains("changed"));
        assert_eq!(before, std::fs::read(&path).unwrap());
        std::fs::write(
            &path,
            format!("@revoked host {}\n", key.to_openssh().unwrap()),
        )
        .unwrap();
        assert!(verify("host", 22, &key, &path, None)
            .unwrap_err()
            .to_string()
            .contains("revoked"));
    }
}
