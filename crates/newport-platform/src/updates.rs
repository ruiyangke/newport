//! Release discovery from the `latest.json` platform manifest and Minisign
//! verification of downloaded payloads.
//! Blocking I/O: call from a worker, never the GUI thread or async executor.
//! This module does not install packages or modify the application bundle.
use base64::{Engine, engine::general_purpose::STANDARD};
use minisign_verify::{PublicKey, Signature};
use semver::Version;
use std::{
    io::Read,
    time::{Duration, Instant},
};
use url::Url;

mod health;
pub use health::verify_startup;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(windows)]
pub mod windows;

const MANIFEST_LIMIT: usize = 1024 * 1024;
const PAYLOAD_LIMIT: usize = 1024 * 1024 * 1024;
const SIGNATURE_LIMIT: usize = 16 * 1024;
const REDIRECT_LIMIT: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    MacArm64,
    WindowsX64,
    WindowsArm64,
}
impl Target {
    pub fn key(self) -> &'static str {
        match self {
            Self::MacArm64 => "darwin-aarch64",
            Self::WindowsX64 => "windows-x86_64",
            Self::WindowsArm64 => "windows-aarch64",
        }
    }
}

/// Construction is private: only a successful cryptographic check can produce
/// a payload eligible for installation. Manifest metadata alone is untrusted.
pub struct VerifiedUpdate {
    version: Version,
    target: Target,
    bytes: Vec<u8>,
    artifact_name: Option<String>,
}
impl VerifiedUpdate {
    pub fn version(&self) -> &Version {
        &self.version
    }
    pub fn target(&self) -> Target {
        self.target
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn artifact_name(&self) -> Option<&str> {
        self.artifact_name.as_deref()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Progress {
    pub version: Version,
    pub downloaded: u64,
    pub total: Option<u64>,
}

enum Transport {
    Https,
    #[cfg(test)]
    Loopback,
}
impl Transport {
    fn validate(&self, url: &Url) -> Result<(), String> {
        if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
            return Err("Update URLs must not contain credentials or fragments.".into());
        }
        if url.scheme() == "https" && url.host_str().is_some() {
            return Ok(());
        }
        #[cfg(test)]
        if matches!(self, Self::Loopback)
            && url.scheme() == "http"
            && url.host_str() == Some("127.0.0.1")
        {
            return Ok(());
        }
        Err("Update requests and redirects require HTTPS.".into())
    }
}

pub struct Updater {
    endpoint: Url,
    public_key: PublicKey,
    current: Version,
    target: Target,
    transport: Transport,
    agent: ureq::Agent,
}

impl Updater {
    pub fn production(current: &str, target: Target) -> Result<Self, String> {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../../../resources/updater.json"))
                .map_err(|error| error.to_string())?;
        Self::new(
            config["endpoints"][0]
                .as_str()
                .ok_or("Missing update endpoint.")?,
            config["pubkey"]
                .as_str()
                .ok_or("Missing update public key.")?,
            current,
            target,
        )
    }

    pub fn new(
        endpoint: &str,
        public_key: &str,
        current: &str,
        target: Target,
    ) -> Result<Self, String> {
        let endpoint = Url::parse(endpoint).map_err(|error| error.to_string())?;
        Transport::Https.validate(&endpoint)?;
        Self::configured(endpoint, public_key, current, target, Transport::Https)
    }

    fn configured(
        endpoint: Url,
        public_key: &str,
        current: &str,
        target: Target,
        transport: Transport,
    ) -> Result<Self, String> {
        let public_key =
            PublicKey::decode(&decode_text(public_key)?).map_err(|error| error.to_string())?;
        // Redirects are handled explicitly so each destination is validated,
        // including HTTPS-to-HTTP downgrades. No cookies or credentials are sent.
        let config = ureq::Agent::config_builder()
            .max_redirects(0)
            .timeout_connect(Some(Duration::from_secs(15)));
        // Windows uses Schannel through native-tls; other targets use rustls.
        // Use Windows' trust store as well: Schannel can build a valid chain to
        // a system root absent from ureq's default Mozilla bundle.
        // Both retain certificate and hostname verification.
        #[cfg(windows)]
        let config = config.tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        );
        let agent = config.build().new_agent();
        Ok(Self {
            endpoint,
            public_key,
            current: version(current)?,
            target,
            transport,
            agent,
        })
    }

    /// Check and download once. Cancellation is observed before requests and
    /// between chunks; individual stalled reads remain bounded by the deadline.
    pub fn check(
        &self,
        cancelled: impl Fn() -> bool,
        mut progress: impl FnMut(Progress),
    ) -> Result<Option<VerifiedUpdate>, String> {
        let mut manifest = self.get(self.endpoint.clone(), Duration::from_secs(30), &cancelled)?;
        if manifest.status() == 204 {
            return Ok(None);
        }
        require_ok(&manifest)?;
        let manifest = read_body(&mut manifest, MANIFEST_LIMIT, &cancelled, |_, _| {})?;
        let Some(release) = self.select(&manifest)? else {
            return Ok(None);
        };
        let signature = Signature::decode(&decode_text(&release.signature)?)
            .map_err(|error| error.to_string())?;
        let mut payload = self.get(release.url, Duration::from_secs(600), &cancelled)?;
        require_ok(&payload)?;
        let bytes = read_body(
            &mut payload,
            PAYLOAD_LIMIT,
            &cancelled,
            |downloaded, total| {
                progress(Progress {
                    version: release.version.clone(),
                    downloaded,
                    total,
                });
            },
        )?;
        if cancelled() {
            return Err("Update check cancelled.".into());
        }
        self.verify(bytes, signature, release.version).map(Some)
    }

    fn select(&self, bytes: &[u8]) -> Result<Option<Release>, String> {
        let manifest: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        let announced = version(
            manifest["version"]
                .as_str()
                .ok_or("Update manifest has no version.")?,
        )?;
        // Build metadata does not make a release newer. Never downgrade or
        // reinstall the current version based on an unsigned manifest.
        if announced.cmp_precedence(&self.current) != std::cmp::Ordering::Greater {
            return Ok(None);
        }
        let entry = &manifest["platforms"][self.target.key()];
        let url = Url::parse(
            entry["url"]
                .as_str()
                .ok_or("Update manifest has no package for this target.")?,
        )
        .map_err(|error| error.to_string())?;
        self.transport.validate(&url)?;
        let signature = entry["signature"]
            .as_str()
            .ok_or("Update manifest has no package signature.")?;
        if signature.len() > SIGNATURE_LIMIT {
            return Err("Update signature exceeds its size limit.".into());
        }
        Ok(Some(Release {
            version: announced,
            url,
            signature: signature.into(),
        }))
    }

    fn verify(
        &self,
        bytes: Vec<u8>,
        signature: Signature,
        announced: Version,
    ) -> Result<VerifiedUpdate, String> {
        // The library verifies the payload AND the signature over the trusted
        // comment. Do not inspect that comment as trusted data before this call.
        self.public_key
            .verify(&bytes, &signature, true)
            .map_err(|error| error.to_string())?;
        let mut versions = signature
            .trusted_comment()
            .split('\t')
            .filter_map(|field| field.strip_prefix("version:"));
        let signed = versions
            .next()
            .ok_or("Update signature does not bind a release version.")?;
        if versions.next().is_some() || version(signed)? != announced {
            return Err("Update signature does not match the announced version.".into());
        }
        let mut names = signature
            .trusted_comment()
            .split('\t')
            .filter_map(|field| field.strip_prefix("file:"));
        let artifact_name = names.next().map(str::to_owned);
        if names.next().is_some() {
            return Err("Update signature has ambiguous artifact names.".into());
        }
        Ok(VerifiedUpdate {
            version: announced,
            target: self.target,
            bytes,
            artifact_name,
        })
    }

    fn get(
        &self,
        mut url: Url,
        timeout: Duration,
        cancelled: &impl Fn() -> bool,
    ) -> Result<ureq::http::Response<ureq::Body>, String> {
        let deadline = Instant::now() + timeout;
        for redirects in 0..=REDIRECT_LIMIT {
            if cancelled() {
                return Err("Update check cancelled.".into());
            }
            self.transport.validate(&url)?;
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or("Update request timed out.")?;
            let response = self
                .agent
                .get(url.as_str())
                .header("User-Agent", "Newport-Updater")
                .config()
                .timeout_global(Some(remaining))
                .build()
                .call()
                .map_err(|error| error.to_string())?;
            if !matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
                return Ok(response);
            }
            if redirects == REDIRECT_LIMIT {
                return Err("Too many update redirects.".into());
            }
            let location = response
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok())
                .ok_or("Update redirect has no valid location.")?;
            url = url.join(location).map_err(|error| error.to_string())?;
        }
        unreachable!()
    }
}

struct Release {
    version: Version,
    url: Url,
    signature: String,
}

fn version(value: &str) -> Result<Version, String> {
    Version::parse(value.strip_prefix('v').unwrap_or(value)).map_err(|error| error.to_string())
}
fn decode_text(value: &str) -> Result<String, String> {
    if value.len() > SIGNATURE_LIMIT {
        return Err("Update signing data exceeds its size limit.".into());
    }
    String::from_utf8(
        STANDARD
            .decode(value.trim())
            .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}
fn require_ok(response: &ureq::http::Response<ureq::Body>) -> Result<(), String> {
    if response.status() == 200 {
        Ok(())
    } else {
        Err(format!("Unexpected update response: {}", response.status()))
    }
}
fn read_body(
    response: &mut ureq::http::Response<ureq::Body>,
    limit: usize,
    cancelled: &impl Fn() -> bool,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<Vec<u8>, String> {
    let total = response
        .headers()
        .get("content-length")
        .map(|value| {
            value
                .to_str()
                .map_err(|error| error.to_string())?
                .parse::<u64>()
                .map_err(|error| error.to_string())
        })
        .transpose()?;
    if total.is_some_and(|size| size > limit as u64) {
        return Err("Update response exceeds its size limit.".into());
    }
    let mut bytes = Vec::new();
    let mut reader = response.body_mut().as_reader();
    let mut chunk = [0u8; 64 * 1024];
    progress(0, total);
    loop {
        if cancelled() {
            return Err("Update check cancelled.".into());
        }
        let count = reader.read(&mut chunk).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        if count > limit - bytes.len() {
            return Err("Update response exceeds its size limit.".into());
        }
        bytes.extend_from_slice(&chunk[..count]);
        progress(bytes.len() as u64, total);
    }
    if total.is_some_and(|total| total != bytes.len() as u64) {
        return Err("Update download was incomplete.".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
