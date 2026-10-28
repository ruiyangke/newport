//! Offline full-product signing. Source-pair provenance belongs to the export
//! that newport-remote's scripts produce; no private source is read here.
use crate::{
    agent_product::{Identity, Product, Variant},
    needle::{self, Builds, Manifest},
};
use anyhow::{Context, Result, anyhow, ensure};
use minisign::SecretKey;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
};

const IDENTITY_LIMIT: u64 = 16 * 1024;
const PAYLOAD_LIMIT: u64 = 512 * 1024 * 1024;
const ARCHITECTURES: [(&str, u16); 2] = [("x86_64", 62), ("aarch64", 183)];

struct Checked {
    product: Product,
    builds: Builds,
}

/// Reject links and special files before opening them, then check the opened
/// file as well. The producer must exclusively own the input snapshot; this is
/// bounded file validation, not a lock against external filesystem mutation.
fn regular(path: &Path, limit: u64) -> Result<(File, u64)> {
    let check = |metadata: fs::Metadata| -> Result<u64> {
        ensure!(
            metadata.file_type().is_file(),
            "Expected a regular file: {}",
            path.display()
        );
        ensure!(
            metadata.len() > 0 && metadata.len() <= limit,
            "Invalid file size: {}",
            path.display()
        );
        Ok(metadata.len())
    };
    check(
        fs::symlink_metadata(path).with_context(|| format!("Cannot inspect {}", path.display()))?,
    )?;
    let file = File::open(path).with_context(|| format!("Cannot read {}", path.display()))?;
    let size = check(file.metadata()?)?;
    Ok((file, size))
}

fn small_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let (file, size) = regular(path, limit)?;
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 == size,
        "File size changed while reading: {}",
        path.display()
    );
    Ok(bytes)
}

fn payload_hash(path: &Path, machine: u16) -> Result<String> {
    let (mut file, size) = regular(path, PAYLOAD_LIMIT)?;
    let mut header = [0; 64];
    file.read_exact(&mut header)
        .context("Agent payload lacks a complete ELF64 header")?;
    ensure!(
        &header[..7] == b"\x7fELF\x02\x01\x01"
            && matches!(u16::from_le_bytes([header[16], header[17]]), 2 | 3)
            && u16::from_le_bytes([header[18], header[19]]) == machine
            && u32::from_le_bytes(header[20..24].try_into().unwrap()) == 1
            && u16::from_le_bytes([header[52], header[53]]) == 64,
        "Agent payload has the wrong ELF architecture or header: {}",
        path.display()
    );
    let mut hash = Sha256::new();
    hash.update(header);
    let mut count = header.len() as u64;
    let mut input = file.take(PAYLOAD_LIMIT - count + 1);
    let mut buffer = [0; 64 * 1024];
    loop {
        let length = input.read(&mut buffer)?;
        if length == 0 {
            break;
        }
        count += length as u64;
        ensure!(
            count <= PAYLOAD_LIMIT,
            "Agent payload exceeds its size limit"
        );
        hash.update(&buffer[..length]);
    }
    ensure!(count == size, "Agent payload size changed while reading");
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

impl Checked {
    fn read(directory: &Path) -> Result<Self> {
        ensure!(
            fs::symlink_metadata(directory)?.file_type().is_dir(),
            "Agent export must be a real directory"
        );
        let mut product = None;
        let mut hashes = Vec::with_capacity(2);
        for (architecture, machine) in ARCHITECTURES {
            let stem = format!("newport-agent-{architecture}");
            let identity: Identity = serde_json::from_slice(&small_file(
                &directory.join(format!("{stem}.product.json")),
                IDENTITY_LIMIT,
            )?)
            .context("Invalid full-agent product identity")?;
            identity.validate().map_err(|e| anyhow!(e))?;
            ensure!(
                identity.product.variant == Variant::Full,
                "Agent signing requires the full product"
            );
            let hash = payload_hash(&directory.join(stem), machine)?;
            ensure!(
                identity.build_sha256 == hash,
                "Agent product identity does not match {architecture} payload"
            );
            ensure!(
                product
                    .as_ref()
                    .is_none_or(|prior| prior == &identity.product),
                "Architecture product protocols differ"
            );
            product = Some(identity.product);
            hashes.push(hash);
        }
        let mut hashes = hashes.into_iter();
        Ok(Self {
            product: product.expect("two checked architectures"),
            builds: Builds {
                x86_64: hashes.next().unwrap(),
                aarch64: hashes.next().unwrap(),
            },
        })
    }

    fn verify(&self, envelope: &[u8], version: &str, public: &str) -> Result<()> {
        let version = canonical_version(version)?;
        let release = needle::verify(envelope, public).map_err(|e| anyhow!(e))?;
        for (architecture, hash) in [
            ("x86_64", &self.builds.x86_64),
            ("aarch64", &self.builds.aarch64),
        ] {
            ensure!(
                release.version_for(architecture, hash) == Some(&version),
                "Signed agent hash or release version does not match {architecture}"
            );
            // A matching full Product also requires schema 2; legacy schema 1
            // has no Product and can never pass this entry point.
            ensure!(
                release.product_for(architecture, hash) == Some(&self.product),
                "Signed full-agent product does not match {architecture}"
            );
        }
        Ok(())
    }
}

fn canonical_version(version: &str) -> Result<semver::Version> {
    let parsed = semver::Version::parse(version).context("Invalid release version")?;
    ensure!(
        parsed.to_string() == version,
        "Release version must be canonical"
    );
    Ok(parsed)
}

/// Sign exactly the checked payload digests and the Product derived from both
/// identities. No second payload read can silently substitute different bytes.
/// This does not update newport-build.json; the caller must record and
/// verify its new immutable export after this envelope has been added.
pub fn sign_full_agent_export(
    directory: &Path,
    version: &str,
    public: &str,
    key: &SecretKey,
) -> Result<PathBuf> {
    canonical_version(version)?;
    let checked = Checked::read(directory)?;
    let output = directory.join(needle::FILENAME);
    match fs::symlink_metadata(&output) {
        Ok(metadata) => ensure!(
            metadata.file_type().is_file(),
            "Existing agent envelope must be a regular file"
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    let manifest = Manifest {
        schema_version: 2,
        product: "newport-needle".into(),
        version: version.into(),
        builds: Builds {
            x86_64: checked.builds.x86_64.clone(),
            aarch64: checked.builds.aarch64.clone(),
        },
        agent: Some(checked.product.clone()),
    };
    let envelope = super::agent_envelope(&manifest, version, public, key)?;
    checked.verify(&envelope, version, public)?;
    super::publish_agent_envelope(directory, &envelope)
}

/// Key-free verification of both current payloads, typed identities and the
/// exact schema-2 signed version/Product. No output is written.
pub fn verify_full_agent_export(directory: &Path, version: &str, public: &str) -> Result<()> {
    verified_export(directory, version, public).map(drop)
}

/// A verified signed export: its signed Product, payload hashes and the exact
/// envelope bytes that were verified.
pub(crate) struct VerifiedExport {
    pub product: Product,
    pub builds: Builds,
    pub envelope: Vec<u8>,
}

pub(crate) fn verified_export(
    directory: &Path,
    version: &str,
    public: &str,
) -> Result<VerifiedExport> {
    canonical_version(version)?;
    let checked = Checked::read(directory)?;
    let envelope = small_file(
        &directory.join(needle::FILENAME),
        needle::ENVELOPE_LIMIT as u64,
    )?;
    checked.verify(&envelope, version, public)?;
    Ok(VerifiedExport {
        product: checked.product,
        builds: checked.builds,
        envelope,
    })
}
