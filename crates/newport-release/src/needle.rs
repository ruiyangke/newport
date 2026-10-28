//! Offline release identity. Remote versions are trusted only when a vendor
//! signature binds the observed executable hash and platform to that version.
use base64::{Engine, engine::general_purpose::STANDARD};
use minisign_verify::{PublicKey, Signature};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

pub const ENVELOPE_LIMIT: usize = 8192;
pub const FILENAME: &str = "needle-release.json";
pub const MANIFEST_FILENAME: &str = "needle-release-manifest.json";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Manifest {
    pub schema_version: u32,
    pub product: String,
    pub version: String,
    pub builds: Builds,
    /// Schema 2 binds compiled protocols and variant to the signed hashes.
    /// Omitted in schema 1, whose released products were core-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<crate::agent_product::Product>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Builds {
    pub x86_64: String,
    pub aarch64: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    /// Base64 preserves the exact signed bytes across serialization and upload.
    pub manifest: String,
    pub signature: String,
}

#[derive(Debug)]
pub struct VerifiedRelease {
    version: Version,
    x86_64: String,
    aarch64: String,
    agent: Option<crate::agent_product::Product>,
}

impl VerifiedRelease {
    pub fn variant_for(
        &self,
        architecture: &str,
        hash: &str,
    ) -> Option<crate::agent_product::Variant> {
        self.version_for(architecture, hash)?;
        Some(
            self.agent
                .as_ref()
                .map_or(crate::agent_product::Variant::Core, |p| p.variant),
        )
    }
    pub fn product_for(
        &self,
        architecture: &str,
        hash: &str,
    ) -> Option<&crate::agent_product::Product> {
        self.version_for(architecture, hash)?;
        self.agent.as_ref()
    }
    /// Only expose a version after matching the actual executable on this platform.
    pub fn version_for(&self, architecture: &str, hash: &str) -> Option<&Version> {
        let expected = match architecture {
            "x86_64" => &self.x86_64,
            "aarch64" | "arm64" => &self.aarch64,
            _ => return None,
        };
        (expected == hash).then_some(&self.version)
    }

    /// Ordering concerns semantic precedence, never build metadata or hash order.
    pub fn compare_builds(
        &self,
        installed_hash: &str,
        candidate: &Self,
        candidate_hash: &str,
        architecture: &str,
    ) -> Option<Ordering> {
        Some(
            self.version_for(architecture, installed_hash)?
                .cmp_precedence(candidate.version_for(architecture, candidate_hash)?),
        )
    }
}

fn decode(value: &str) -> Result<String, String> {
    let bytes = STANDARD
        .decode(value.trim())
        .map_err(|_| "Invalid release encoding")?;
    String::from_utf8(bytes).map_err(|_| "Invalid release text".into())
}

pub fn verify(envelope: &[u8], public_key: &str) -> Result<VerifiedRelease, String> {
    if envelope.len() > ENVELOPE_LIMIT {
        return Err("Needle release metadata is too large".into());
    }
    let envelope: Envelope =
        serde_json::from_slice(envelope).map_err(|_| "Invalid Needle release envelope")?;
    let manifest = decode(&envelope.manifest)?;
    let signature = Signature::decode(&decode(&envelope.signature)?)
        .map_err(|_| "Invalid Needle release signature")?;
    let key =
        PublicKey::decode(&decode(public_key)?).map_err(|_| "Invalid Needle release public key")?;
    key.verify(manifest.as_bytes(), &signature, true)
        .map_err(|_| "Needle release signature did not verify")?;
    let manifest: Manifest =
        serde_json::from_str(&manifest).map_err(|_| "Invalid Needle release manifest")?;
    if manifest.product != "newport-needle" {
        return Err("Unsupported Needle release manifest".into());
    }
    match (manifest.schema_version, &manifest.agent) {
        (1, None) => (),
        (2, Some(product)) => product.validate()?,
        _ => return Err("Unsupported agent release product identity".into()),
    }
    let version =
        Version::parse(&manifest.version).map_err(|_| "Invalid Needle release version")?;
    if version.to_string() != manifest.version {
        return Err("Needle release version is not canonical".into());
    }
    let comment = signature.trusted_comment();
    for (field, expected) in [
        ("file:", MANIFEST_FILENAME),
        ("version:", &manifest.version),
    ] {
        let mut values = comment
            .split('\t')
            .filter_map(|part| part.strip_prefix(field));
        if values.next() != Some(expected) || values.next().is_some() {
            return Err("Needle release signature metadata does not match".into());
        }
    }
    for hash in [&manifest.builds.x86_64, &manifest.builds.aarch64] {
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("Invalid Needle release build hash".into());
        }
    }
    Ok(VerifiedRelease {
        version,
        x86_64: manifest.builds.x86_64,
        aarch64: manifest.builds.aarch64,
        agent: manifest.agent,
    })
}

pub fn verify_production(envelope: &[u8]) -> Result<VerifiedRelease, String> {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../../../resources/updater.json"))
            .map_err(|_| "Invalid release trust configuration")?;
    verify(
        envelope,
        config["pubkey"]
            .as_str()
            .ok_or("Missing release public key")?,
    )
}
