//! Desktop distribution only: S3-compatible R2 uploads, never a website deployment.
use super::manifest::{self, Manifest};
use crate::util;
use anyhow::{Context, Result, bail, ensure};
use aws_credential_types::Credentials;
use aws_sigv4::{
    http_request::{
        PayloadChecksumKind, PercentEncodingMode, SignableBody, SignableRequest, SigningSettings,
        UriPathNormalizationMode, sign,
    },
    sign::v4,
};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

const LATEST: &str = "updates/latest.json";
const LIMIT: u64 = 1024 * 1024 * 1024;
/// The least time any R2 request gets, however small.
const MIN_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
/// The slowest transfer a request tolerates; larger objects get proportionally longer.
const MIN_BYTES_PER_SECOND: u64 = 256 * 1024;

fn request_timeout(size: u64) -> Duration {
    MIN_REQUEST_TIMEOUT.max(Duration::from_secs(size.div_ceil(MIN_BYTES_PER_SECOND)))
}

/// A GET's size is unknown until it arrives; only the feed is known to be small.
fn transfer_size(method: &str, key: &str, bytes: &[u8]) -> u64 {
    match method {
        "GET" if key != LATEST => LIMIT,
        _ => bytes.len() as u64,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Artifact {
    size: u64,
    sha256: String,
}

/// Public distribution contract, consumed independently by the website Worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Release {
    version: String,
    manifest: Manifest,
    artifacts: BTreeMap<String, Artifact>,
}

struct Plan {
    directory: PathBuf,
    release: Release,
    manifest: Vec<u8>,
}

fn stable_version(value: &str) -> Result<Version> {
    let version = Version::parse(value).context("Invalid release version")?;
    ensure!(
        version.pre.is_empty() && version.build.is_empty(),
        "Only stable releases can update the production feed"
    );
    ensure!(
        version.to_string() == value,
        "Expected a canonical release version"
    );
    Ok(version)
}

fn prepare(directory: &Path, public_key: &str) -> Result<Plan> {
    let path = directory.join("latest.json");
    let bytes = fs::read(&path).context("Cannot read release/latest.json")?;
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    let version = &manifest.version;
    stable_version(version)?;
    chrono::DateTime::parse_from_rfc3339(
        manifest
            .pub_date
            .as_deref()
            .context("Missing publication date")?,
    )?;
    // Existing merge validation enforces target coverage, URL origin/version,
    // local files and sidecar consistency. Verification binds version and filename.
    manifest::merge(&[path], version, directory)?;
    manifest::verify_packages(&manifest, directory, public_key)?;
    let mut names = BTreeSet::from([
        "latest.json".to_owned(),
        format!("{}.zip", manifest::macos_name(version)),
    ]);
    for (target, suffix) in [
        ("darwin-aarch64", "macos-arm64.app.tar.gz"),
        ("windows-x86_64", "windows-x86_64-setup.exe"),
        ("windows-aarch64", "windows-aarch64-setup.exe"),
    ] {
        let name = format!("Newport-{version}-{suffix}");
        ensure!(
            manifest.platforms[target].url
                == format!("{}{name}", util::release_download_base(version)?),
            "Unexpected payload for {target}"
        );
        names.insert(format!("{name}.sig"));
        names.insert(name);
    }
    let mut artifacts = BTreeMap::new();
    let mut sums = String::new();
    for name in &names {
        let path = directory.join(name);
        let metadata =
            fs::symlink_metadata(&path).with_context(|| format!("Missing release file: {name}"))?;
        ensure!(
            metadata.file_type().is_file() && metadata.len() > 0 && metadata.len() <= LIMIT,
            "Invalid release file: {name}"
        );
        let sha256 = util::sha256_file(&path)?;
        sums.push_str(&format!("{sha256}  {name}\n"));
        artifacts.insert(
            name.clone(),
            Artifact {
                size: metadata.len(),
                sha256,
            },
        );
    }
    ensure!(
        fs::read(directory.join("SHA256SUMS"))? == sums.as_bytes(),
        "SHA256SUMS does not match the complete release"
    );
    artifacts.insert(
        "SHA256SUMS".into(),
        Artifact {
            size: sums.len() as u64,
            sha256: util::sha256_hex(sums.as_bytes()),
        },
    );
    Ok(Plan {
        directory: directory.to_owned(),
        release: Release {
            version: version.clone(),
            manifest,
            artifacts,
        },
        manifest: bytes,
    })
}

struct Object {
    bytes: Vec<u8>,
    etag: String,
}

enum Condition<'a> {
    Missing,
    Matches(&'a str),
}
impl Condition<'_> {
    fn header(&self) -> (&str, &str) {
        match self {
            Self::Missing => ("if-none-match", "*"),
            Self::Matches(etag) => ("if-match", etag),
        }
    }
}

trait Store {
    fn get(&mut self, key: &str) -> Result<Option<Object>>;
    fn put(&mut self, key: &str, bytes: &[u8], condition: Condition<'_>) -> Result<()>;
}

fn immutable(store: &mut impl Store, key: &str, bytes: &[u8]) -> Result<()> {
    if let Some(existing) = store.get(key)? {
        ensure!(
            existing.bytes == bytes,
            "Published object differs: {key}. Use a new version"
        );
        return Ok(());
    }
    store.put(key, bytes, Condition::Missing)?;
    let stored = store.get(key)?.context("Uploaded object is missing")?;
    ensure!(
        stored.bytes == bytes,
        "Uploaded object failed verification: {key}"
    );
    Ok(())
}

fn publish(plan: &Plan, store: &mut impl Store) -> Result<()> {
    let previous = store.get(LATEST)?;
    if let Some(previous) = &previous {
        let manifest: Manifest =
            serde_json::from_slice(&previous.bytes).context("Invalid existing update feed")?;
        let current = stable_version(&manifest.version)?;
        let candidate = stable_version(&plan.release.version)?;
        ensure!(
            candidate >= current,
            "Refusing to replace a newer published release"
        );
        if candidate == current {
            ensure!(
                manifest == plan.release.manifest,
                "This version already has a different update manifest"
            );
        }
    }
    let prefix = format!("releases/{}/", plan.release.version);
    // Check an existing release record before touching any of its objects.
    let record = format!("{}\n", serde_json::to_string_pretty(&plan.release)?).into_bytes();
    if let Some(existing) = store.get(&format!("{prefix}release.json"))? {
        ensure!(
            existing.bytes == record,
            "This version already has different release files"
        );
    }
    for (name, artifact) in &plan.release.artifacts {
        let bytes = fs::read(plan.directory.join(name))?;
        ensure!(
            bytes.len() as u64 == artifact.size && util::sha256_hex(&bytes) == artifact.sha256,
            "Local artifact changed after verification: {name}"
        );
        immutable(store, &format!("{prefix}{name}"), &bytes)?;
        println!("Verified R2 object: {name}");
    }
    immutable(store, &format!("{prefix}release.json"), &record)?;
    // One conditional write announces the release only after every immutable
    // object has been read back. Competing publishers cannot clobber the feed.
    if previous
        .as_ref()
        .is_none_or(|old| old.bytes != plan.manifest)
    {
        let condition = previous
            .as_ref()
            .map_or(Condition::Missing, |old| Condition::Matches(&old.etag));
        store.put(LATEST, &plan.manifest, condition)?;
    }
    ensure!(
        store
            .get(LATEST)?
            .context("Published feed is missing")?
            .bytes
            == plan.manifest,
        "Published feed changed; inspect the active release before retrying"
    );
    println!(
        "Published Newport {} to R2; the website source and deployment were not changed",
        plan.release.version
    );
    Ok(())
}

struct R2 {
    base: String,
    credentials: Credentials,
    agent: ureq::Agent,
}

impl R2 {
    fn from_env(bucket: &str) -> Result<Self> {
        let account = env::var("CLOUDFLARE_ACCOUNT_ID").context("Set CLOUDFLARE_ACCOUNT_ID")?;
        ensure!(
            account.len() == 32 && account.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid Cloudflare account ID"
        );
        ensure!(
            (3..=63).contains(&bucket.len())
                && bucket
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "Invalid R2 bucket name"
        );
        let access = env::var("R2_ACCESS_KEY_ID").context("Set R2_ACCESS_KEY_ID")?;
        let secret = env::var("R2_SECRET_ACCESS_KEY").context("Set R2_SECRET_ACCESS_KEY")?;
        ensure!(
            !access.is_empty() && !secret.is_empty(),
            "R2 credentials must not be empty"
        );
        Ok(Self {
            base: format!("https://{account}.r2.cloudflarestorage.com/{bucket}"),
            credentials: Credentials::new(access, secret, None, None, "newport-release"),
            agent: ureq::Agent::config_builder()
                .max_redirects(0)
                .http_status_as_error(false)
                .timeout_connect(Some(Duration::from_secs(30)))
                .timeout_global(Some(MIN_REQUEST_TIMEOUT))
                .build()
                .new_agent(),
        })
    }

    fn request(
        &self,
        method: &str,
        key: &str,
        bytes: &[u8],
        condition: Option<Condition<'_>>,
    ) -> Result<ureq::http::Response<ureq::Body>> {
        // R2 can weaken ETags on compressed responses. If-Match requires the
        // strong validator for the stored representation, not a transformed one.
        self.request_with_encoding(method, key, bytes, condition, "identity")
    }

    fn request_with_encoding(
        &self,
        method: &str,
        key: &str,
        bytes: &[u8],
        condition: Option<Condition<'_>>,
        encoding: &str,
    ) -> Result<ureq::http::Response<ureq::Body>> {
        let url = format!("{}/{key}", self.base);
        let mut request = ureq::http::Request::builder()
            .method(method)
            .uri(&url)
            .header("accept-encoding", encoding);
        if let Some(condition) = condition {
            let (name, value) = condition.header();
            request = request.header(name, value);
        }
        if method == "PUT" {
            request = request.header(
                "content-type",
                if key.ends_with(".json") {
                    "application/json"
                } else {
                    "application/octet-stream"
                },
            );
            request = request.header(
                "cache-control",
                if key == LATEST {
                    "no-store"
                } else {
                    "public, max-age=31536000, immutable"
                },
            );
        }
        let mut request = request.body(bytes)?;
        let identity = self.credentials.clone().into();
        let mut settings = SigningSettings::default();
        settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;
        settings.percent_encoding_mode = PercentEncodingMode::Single;
        settings.uri_path_normalization_mode = UriPathNormalizationMode::Disabled;
        let params = v4::SigningParams::builder()
            .identity(&identity)
            .region("auto")
            .name("s3")
            .time(SystemTime::now())
            .settings(settings)
            .build()?
            .into();
        let signable = SignableRequest::new(
            method,
            &url,
            request
                .headers()
                .iter()
                .map(|(name, value)| (name.as_str(), value.to_str().expect("validated header"))),
            SignableBody::Bytes(bytes),
        )?;
        let (instructions, _) = sign(signable, &params)?.into_parts();
        instructions.apply_to_request_http1x(&mut request);
        let request = self
            .agent
            .configure_request(request)
            .timeout_global(Some(request_timeout(transfer_size(method, key, bytes))))
            .build();
        self.agent
            .run(request)
            .with_context(|| format!("R2 {method} request failed for {key}"))
    }
}

impl Store for R2 {
    fn get(&mut self, key: &str) -> Result<Option<Object>> {
        let mut response = self.request("GET", key, &[], None)?;
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        ensure!(
            response.status().as_u16() == 200,
            "R2 GET {key}: HTTP {}",
            response.status()
        );
        let etag = response
            .headers()
            .get("etag")
            .context("R2 response has no ETag")?
            .to_str()?
            .to_owned();
        let bytes = response
            .body_mut()
            .with_config()
            .limit(LIMIT)
            .read_to_vec()?;
        Ok(Some(Object { bytes, etag }))
    }
    fn put(&mut self, key: &str, bytes: &[u8], condition: Condition<'_>) -> Result<()> {
        let response = self.request("PUT", key, bytes, Some(condition))?;
        if matches!(response.status().as_u16(), 409 | 412) {
            bail!(
                "R2 conditional write rejected for {key}: HTTP {}; inspect the active feed before retrying",
                response.status()
            );
        }
        ensure!(
            response.status().is_success(),
            "R2 PUT {key}: HTTP {}",
            response.status()
        );
        Ok(())
    }
}

pub fn run(directory: &Path, bucket: &str, dry_run: bool) -> Result<()> {
    deploy(directory, &util::updater_public_key()?, dry_run, || {
        R2::from_env(bucket)
    })
}

/// A dry run verifies everything a publication would, but never opens the store.
fn deploy<S: Store>(
    directory: &Path,
    public_key: &str,
    dry_run: bool,
    store: impl FnOnce() -> Result<S>,
) -> Result<()> {
    let plan = prepare(directory, public_key)?;
    if dry_run {
        println!(
            "Verified Newport {}: {} release files plus release.json; updates/latest.json is promoted last",
            plan.release.version,
            plan.release.artifacts.len()
        );
        return Ok(());
    }
    publish(&plan, &mut store()?)
}

/// Only report public object metadata, never credentials or signed request headers.
pub fn inspect_storage(bucket: &str) -> Result<()> {
    let store = R2::from_env(bucket)?;
    for encoding in ["gzip", "identity"] {
        let mut response = store.request_with_encoding("GET", LATEST, &[], None, encoding)?;
        println!(
            "R2 GET {LATEST}: requested_encoding={encoding}, status={}, etag={:?}, content_encoding={:?}",
            response.status(),
            response.headers().get("etag"),
            response.headers().get("content-encoding"),
        );
        ensure!(
            response.status().is_success(),
            "Cannot inspect the active feed"
        );
        let bytes = response
            .body_mut()
            .with_config()
            .limit(LIMIT)
            .read_to_vec()?;
        let manifest: Manifest = serde_json::from_slice(&bytes)?;
        println!(
            "version={}, sha256={}",
            manifest.version,
            util::sha256_hex(&bytes)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
