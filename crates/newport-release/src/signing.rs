use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use minisign::{PublicKeyBox, SecretKey, SecretKeyBox};
use std::{
    fs::File,
    io::{Seek, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

pub fn secret_key(encoded: &str, password: String) -> Result<SecretKey> {
    let bytes = Zeroizing::new(
        STANDARD
            .decode(encoded.trim())
            .map_err(|_| anyhow!("Invalid signing key encoding"))?,
    );
    let text = std::str::from_utf8(&bytes).map_err(|_| anyhow!("Invalid signing key encoding"))?;
    let key = SecretKeyBox::from_string(text).map_err(|_| anyhow!("Invalid signing key format"))?;
    // The library explicitly distinguishes passwordless keys from encrypted
    // keys with an empty password. Both are used by existing release tooling.
    if let Ok(secret) = key.clone().into_unencrypted_secret_key() {
        return Ok(secret);
    }
    key.into_secret_key(Some(password))
        .map_err(|_| anyhow!("Cannot unlock signing key; check its format and password"))
}

/// Emit the base64-wrapped Minisign format accepted by both Newport updaters.
/// The signed comment binds the bytes to a version, independently of the manifest.
pub fn sign(artifact: &Path, version: &str, public: &str, key: &SecretKey) -> Result<PathBuf> {
    let parsed = semver::Version::parse(version).context("Invalid release version")?;
    if parsed.to_string() != version {
        bail!("Release version must be canonical");
    }
    let filename = artifact
        .file_name()
        .and_then(|s| s.to_str())
        .context("Invalid artifact filename")?;
    if filename.chars().any(char::is_control) {
        bail!("Artifact filename must not contain control characters");
    }
    let public = STANDARD
        .decode(public.trim())
        .context("Invalid public key encoding")?;
    let public = PublicKeyBox::from_string(std::str::from_utf8(&public)?)?.into_public_key()?;
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let comment = format!("timestamp:{timestamp}\tfile:{filename}\tversion:{version}");
    let mut file = File::open(artifact).context("Cannot open update artifact")?;
    let signature = minisign::sign(
        Some(&public),
        key,
        &mut file,
        Some(&comment),
        Some("Newport update"),
    )
    .context("Cannot sign artifact with the configured updater key")?;
    file.rewind()?;
    minisign::verify(&public, &signature, &mut file, true, false, false)
        .context("Update signature did not verify against the configured key and artifact")?;
    let encoded = STANDARD.encode(signature.to_string());
    let output = artifact.with_added_extension("sig");
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut pending = tempfile::NamedTempFile::new_in(parent)?;
    pending.write_all(encoded.as_bytes())?;
    pending.as_file().sync_all()?;
    pending
        .persist(&output)
        .context("Cannot publish update signature")?;
    Ok(output)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

/// Sign both exact Linux payloads before either desktop is compiled. Unsigned
/// development binaries deliberately have no ordered release identity.
pub fn sign_needle(
    directory: &Path,
    version: &str,
    public: &str,
    key: &SecretKey,
) -> Result<PathBuf> {
    use crate::needle::{Builds, Envelope, FILENAME, MANIFEST_FILENAME, Manifest};
    use sha2::{Digest, Sha256};
    let hash = |arch: &str| -> Result<String> {
        let bytes = std::fs::read(directory.join(format!("newport-agent-{arch}")))?;
        if !bytes.starts_with(b"\x7fELF") {
            bail!("Needle release requires Linux ELF payloads");
        }
        Ok(Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect())
    };
    let manifest = Manifest {
        schema_version: 1,
        product: "newport-needle".into(),
        version: version.into(),
        builds: Builds {
            x86_64: hash("x86_64")?,
            aarch64: hash("aarch64")?,
        },
    };
    let bytes = serde_json::to_vec(&manifest)?;
    let temporary = tempfile::tempdir()?;
    let path = temporary.path().join(MANIFEST_FILENAME);
    std::fs::write(&path, &bytes)?;
    let signature = std::fs::read_to_string(sign(&path, version, public, key)?)?;
    let envelope = serde_json::to_vec(&Envelope {
        manifest: STANDARD.encode(bytes),
        signature,
    })?;
    crate::needle::verify(&envelope, public).map_err(|e| anyhow!(e))?;
    let output = directory.join(FILENAME);
    let mut pending = tempfile::NamedTempFile::new_in(directory)?;
    pending.write_all(&envelope)?;
    pending.as_file().sync_all()?;
    pending
        .persist(&output)
        .context("Cannot publish Needle release metadata")?;
    Ok(output)
}
