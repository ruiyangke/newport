//! Update manifests: signed per-platform fragments, the merged `latest.json`
//! and the release `SHA256SUMS`.
use crate::util;
use anyhow::{anyhow, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use minisign_verify::{PublicKey, Signature};
use newport_release::SecretKey;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

/// Every update target a release ships.
pub const TARGETS: [&str; 3] = ["darwin-aarch64", "windows-x86_64", "windows-aarch64"];

/// An updater manifest: one release version and a signed payload per target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: String,
    pub notes: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pub_date: Option<String>,
    pub platforms: BTreeMap<String, Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// Base64-wrapped Minisign signature, identical to `<payload>.sig`.
    pub signature: String,
    pub url: String,
}

impl Manifest {
    pub fn new(
        version: &str,
        pub_date: Option<String>,
        platforms: BTreeMap<String, Entry>,
    ) -> Self {
        Self {
            version: version.into(),
            notes: format!("Newport {version}"),
            pub_date,
            platforms,
        }
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        fs::write(path, format!("{}\n", serde_json::to_string_pretty(self)?))
            .with_context(|| format!("Cannot write {}", path.display()))
    }
}

/// `Newport-<version>-macos-arm64`, the stem of the macOS update and download archives.
pub fn macos_name(version: &str) -> String {
    format!("Newport-{version}-macos-arm64")
}

/// A plain release asset file name: no separators, encoding or relative components.
fn checked_name(name: &str) -> Result<&str> {
    ensure!(
        !name.is_empty()
            && name != "."
            && name != ".."
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+')),
        "Invalid update artifact filename"
    );
    Ok(name)
}

/// The asset file name an update URL downloads.
fn filename(url: &str) -> Result<&str> {
    checked_name(url.rsplit('/').next().unwrap_or_default())
}

/// Sign `payload` (writing `<payload>.sig`) and write the one-target manifest
/// fragment `fragment` beside it; returns the fragment path.
pub fn write_fragment(
    payload: &Path,
    target: &str,
    fragment: &str,
    version: &str,
    public_key: &str,
    key: &SecretKey,
) -> Result<PathBuf> {
    ensure!(TARGETS.contains(&target), "Unknown update target: {target}");
    let name = checked_name(
        payload
            .file_name()
            .and_then(|n| n.to_str())
            .context("Invalid update artifact filename")?,
    )?;
    let url = format!("{}{name}", util::release_download_base(version)?);
    let signature = newport_release::sign(payload, version, public_key, key)?;
    let signature = fs::read_to_string(&signature)?.trim().to_owned();
    let pub_date = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let manifest = Manifest::new(
        version,
        Some(pub_date),
        BTreeMap::from([(target.to_owned(), Entry { signature, url })]),
    );
    let path = payload.with_file_name(fragment);
    manifest.write(&path)?;
    Ok(path)
}

/// Merge fragments for `version`. Every target appears exactly once, under this
/// release's download URL, with its payload and a `.sig` equal to the manifest signature in `assets`.
pub fn merge(paths: &[PathBuf], version: &str, assets: &Path) -> Result<Manifest> {
    let base = util::release_download_base(version)?;
    let mut pub_date = None;
    let mut platforms = BTreeMap::new();
    for path in paths {
        let fragment: Manifest = util::read_json(path)?;
        ensure!(
            fragment.version == version,
            "Cannot merge different release versions"
        );
        pub_date = pub_date.or(fragment.pub_date);
        for (target, entry) in fragment.platforms {
            ensure!(
                TARGETS.contains(&target.as_str()),
                "Unknown update target: {target}"
            );
            ensure!(
                !platforms.contains_key(&target),
                "Duplicate update target: {target}"
            );
            let name = entry
                .url
                .strip_prefix(&base)
                .context("Update URL does not match this release")?;
            let name = checked_name(name)?;
            ensure!(
                assets.join(name).is_file(),
                "Missing update artifact: {name}"
            );
            let signature = fs::read_to_string(assets.join(format!("{name}.sig"))).ok();
            ensure!(
                !entry.signature.is_empty()
                    && signature.as_deref().map(str::trim) == Some(entry.signature.as_str()),
                "Missing or inconsistent signature: {name}"
            );
            platforms.insert(target, entry);
        }
    }
    for target in TARGETS {
        ensure!(
            platforms.contains_key(target),
            "Missing release target: {target}"
        );
    }
    Ok(Manifest::new(version, pub_date, platforms))
}

fn decode_text(value: &str) -> Result<String> {
    String::from_utf8(STANDARD.decode(value).context("Invalid base64")?).context("Invalid UTF-8")
}

/// The single value of a trusted-comment field such as `version:`.
fn field<'a>(comment: &'a str, name: &str) -> Option<&'a str> {
    let mut values = comment.split('\t').filter_map(|f| f.strip_prefix(name));
    values.next().filter(|_| values.next().is_none())
}

/// Verify every payload against `public_key` (base64-wrapped Minisign), and
/// require each signature to bind exactly this version and file name.
pub fn verify_packages(manifest: &Manifest, assets: &Path, public_key: &str) -> Result<()> {
    let key = PublicKey::decode(&decode_text(public_key).context("Invalid updater public key")?)
        .map_err(|e| anyhow!("Invalid updater public key: {e}"))?;
    for entry in manifest.platforms.values() {
        let name = filename(&entry.url)?;
        let signature = Signature::decode(
            &decode_text(&entry.signature).with_context(|| format!("Invalid signature: {name}"))?,
        )
        .map_err(|e| anyhow!("Invalid signature: {name}: {e}"))?;
        let path = assets.join(name);
        let bytes = fs::read(&path).with_context(|| format!("Cannot read {}", path.display()))?;
        key.verify(&bytes, &signature, true)
            .map_err(|e| anyhow!("Signature verification failed: {name}: {e}"))?;
        let comment = signature.trusted_comment();
        ensure!(
            field(comment, "version:") == Some(manifest.version.as_str()),
            "Artifact signature is not bound to this release version: {name}"
        );
        ensure!(
            field(comment, "file:") == Some(name),
            "Artifact signature names a different file: {name}"
        );
    }
    Ok(())
}

/// Write `SHA256SUMS` over exactly the public release files: the manifest, each
/// payload and signature, and the macOS download zip when present.
pub fn write_checksums(manifest: &Manifest, assets: &Path, manifest_name: &str) -> Result<()> {
    let mut names = BTreeSet::from([manifest_name.to_owned()]);
    for entry in manifest.platforms.values() {
        let name = filename(&entry.url)?;
        names.insert(name.to_owned());
        names.insert(format!("{name}.sig"));
    }
    let zip = format!("{}.zip", macos_name(&manifest.version));
    if assets.join(&zip).exists() {
        names.insert(zip);
    }
    let mut sums = String::new();
    for name in names {
        sums += &format!("{}  {name}\n", util::sha256_file(&assets.join(&name))?);
    }
    fs::write(assets.join("SHA256SUMS"), sums).context("Cannot write SHA256SUMS")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(version: &str) -> String {
        util::release_download_base(version).unwrap()
    }

    fn fixture(path: &str) -> String {
        fs::read_to_string(util::root().join(path)).unwrap()
    }

    /// Three fragments with placeholder payloads and matching `.sig` files.
    fn fragments(dir: &Path) -> Vec<PathBuf> {
        TARGETS
            .iter()
            .map(|target| {
                let name = format!("{target}.package");
                fs::write(dir.join(&name), "package").unwrap();
                fs::write(dir.join(format!("{name}.sig")), "test signature\n").unwrap();
                let manifest = Manifest::new(
                    "1.2.3",
                    Some("2026-01-01T00:00:00Z".into()),
                    BTreeMap::from([(
                        target.to_string(),
                        Entry {
                            signature: "test signature".into(),
                            url: format!("{}{name}", base("1.2.3")),
                        },
                    )]),
                );
                let path = dir.join(format!("{target}.json"));
                manifest.write(&path).unwrap();
                path
            })
            .collect()
    }

    fn edit(path: &Path, change: impl FnOnce(&mut Manifest)) {
        let mut manifest: Manifest = util::read_json(path).unwrap();
        change(&mut manifest);
        manifest.write(path).unwrap();
    }

    fn entry(manifest: &mut Manifest) -> &mut Entry {
        manifest.platforms.values_mut().next().unwrap()
    }

    #[test]
    fn merge_combines_every_target() {
        let dir = tempfile::tempdir().unwrap();
        let paths = fragments(dir.path());
        let merged = merge(&paths, "1.2.3", dir.path()).unwrap();
        assert_eq!(merged.platforms.len(), 3);
        assert_eq!(merged.notes, "Newport 1.2.3");
        assert_eq!(merged.pub_date.as_deref(), Some("2026-01-01T00:00:00Z"));
    }

    #[test]
    fn merge_rejects_incomplete_or_inconsistent_fragments() {
        type Scenario = fn(&Path, &mut Vec<PathBuf>) -> &'static str;
        let scenarios: &[(&str, Scenario, &str)] = &[
            (
                "missing platform",
                |_, p| {
                    p.pop();
                    "1.2.3"
                },
                "Missing release target",
            ),
            (
                "mixed versions",
                |_, _| "1.2.4",
                "different release versions",
            ),
            (
                "duplicate platform",
                |_, p| {
                    p.push(p[0].clone());
                    "1.2.3"
                },
                "Duplicate update target",
            ),
            (
                "missing payload",
                |d, _| {
                    fs::remove_file(d.join("windows-aarch64.package")).unwrap();
                    "1.2.3"
                },
                "Missing update artifact",
            ),
            (
                "missing signature file",
                |d, _| {
                    fs::remove_file(d.join("windows-aarch64.package.sig")).unwrap();
                    "1.2.3"
                },
                "inconsistent signature",
            ),
            (
                "mismatched signature",
                |d, _| {
                    fs::write(d.join("windows-x86_64.package.sig"), "wrong").unwrap();
                    "1.2.3"
                },
                "inconsistent signature",
            ),
            (
                "empty signature",
                |_, p| {
                    edit(&p[0], |m| entry(m).signature.clear());
                    "1.2.3"
                },
                "inconsistent signature",
            ),
            (
                "unknown target",
                |_, p| {
                    edit(&p[0], |m| {
                        let e = m.platforms.remove("darwin-aarch64").unwrap();
                        m.platforms.insert("linux-x86_64".into(), e);
                    });
                    "1.2.3"
                },
                "Unknown update target",
            ),
            (
                "other release",
                |_, p| {
                    edit(&p[0], |m| {
                        entry(m).url = format!("{}darwin-aarch64.package", base("1.2.2"))
                    });
                    "1.2.3"
                },
                "does not match this release",
            ),
            (
                "other host",
                |_, p| {
                    edit(&p[0], |m| {
                        entry(m).url = "https://example.com/ruiyangke/newport/releases/download/v1.2.3/darwin-aarch64.package".into()
                    });
                    "1.2.3"
                },
                "does not match this release",
            ),
            (
                "credentials",
                |_, p| {
                    edit(&p[0], |m| {
                        entry(m).url = "https://user:pass@github.com/ruiyangke/newport/releases/download/v1.2.3/darwin-aarch64.package".into()
                    });
                    "1.2.3"
                },
                "does not match this release",
            ),
            (
                "nested path",
                |_, p| {
                    edit(&p[0], |m| {
                        entry(m).url = format!("{}../v1.2.2/darwin-aarch64.package", base("1.2.3"))
                    });
                    "1.2.3"
                },
                "Invalid update artifact filename",
            ),
            (
                "encoded name",
                |_, p| {
                    edit(&p[0], |m| entry(m).url = format!("{}a%2Fb", base("1.2.3")));
                    "1.2.3"
                },
                "Invalid update artifact filename",
            ),
        ];
        for (name, change, pattern) in scenarios {
            let dir = tempfile::tempdir().unwrap();
            let mut paths = fragments(dir.path());
            let version = change(dir.path(), &mut paths);
            let error = format!("{:#}", merge(&paths, version, dir.path()).unwrap_err());
            assert!(error.contains(pattern), "{name}: {error}");
        }
    }

    /// The fixture payload, signed for 2.0.0 by the fixture updater key.
    fn signed_fixture(dir: &Path) -> (Manifest, String) {
        let source = util::root().join("resources/test-fixtures/updater");
        fs::copy(source.join("payload.txt"), dir.join("payload.txt")).unwrap();
        let manifest = Manifest::new(
            "2.0.0",
            None,
            BTreeMap::from([(
                "windows-aarch64".into(),
                Entry {
                    url: format!("{}payload.txt", base("2.0.0")),
                    signature: fixture("resources/test-fixtures/updater/payload.txt.sig")
                        .trim()
                        .into(),
                },
            )]),
        );
        let key = fixture("resources/test-fixtures/updater/public.key");
        (manifest, key.trim().to_owned())
    }

    #[test]
    fn packages_verify_in_process() {
        let dir = tempfile::tempdir().unwrap();
        let (manifest, key) = signed_fixture(dir.path());
        verify_packages(&manifest, dir.path(), &key).unwrap();

        let reject = |manifest: &Manifest, key: &str, pattern: &str| {
            let error = format!(
                "{:#}",
                verify_packages(manifest, dir.path(), key).unwrap_err()
            );
            assert!(error.contains(pattern), "{error}");
        };
        let mut wrong_version = manifest.clone();
        wrong_version.version = "3.0.0".into();
        reject(&wrong_version, &key, "not bound to this release version");
        reject(
            &manifest,
            &util::updater_public_key().unwrap(),
            "verification failed",
        );
        reject(&manifest, "not base64!", "Invalid updater public key");

        let mut renamed = manifest.clone();
        fs::copy(
            dir.path().join("payload.txt"),
            dir.path().join("renamed.txt"),
        )
        .unwrap();
        entry(&mut renamed).url = format!("{}renamed.txt", base("2.0.0"));
        reject(&renamed, &key, "names a different file");

        fs::write(dir.path().join("payload.txt"), "tampered").unwrap();
        reject(&manifest, &key, "verification failed");
    }

    #[test]
    fn signatures_must_bind_exactly_one_version() {
        let dir = tempfile::tempdir().unwrap();
        let (mut manifest, _) = signed_fixture(dir.path());
        let key = fixture("resources/test-fixtures/updater/version-fields.public.key");
        let key = key.trim();
        for signature in ["versionless.sig", "duplicate-version.sig"] {
            entry(&mut manifest).signature =
                fixture(&format!("resources/test-fixtures/updater/{signature}"))
                    .trim()
                    .into();
            let error = verify_packages(&manifest, dir.path(), key).unwrap_err();
            assert!(
                error.to_string().contains("not bound"),
                "{signature}: {error}"
            );
        }
    }

    #[test]
    fn checksums_cover_every_release_file_and_exclude_staging_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut contents = BTreeMap::from([
            ("latest.json".to_owned(), "combined manifest".to_owned()),
            (
                "Newport-1.2.3-macos-arm64.zip".to_owned(),
                "download archive".to_owned(),
            ),
        ]);
        let mut platforms = BTreeMap::new();
        for target in TARGETS {
            let name = format!("{target}.package");
            platforms.insert(
                target.to_owned(),
                Entry {
                    signature: String::new(),
                    url: format!("{}{name}", base("1.2.3")),
                },
            );
            contents.insert(format!("{name}.sig"), format!("{target} signature"));
            contents.insert(name, format!("{target} payload"));
        }
        let manifest = Manifest::new("1.2.3", None, platforms);
        for (name, value) in &contents {
            fs::write(dir.path().join(name), value).unwrap();
        }
        for name in ["private.key", "windows-x86_64.json", "SHA256SUMS"] {
            fs::write(dir.path().join(name), "staging only").unwrap();
        }
        write_checksums(&manifest, dir.path(), "latest.json").unwrap();
        let expected: String = contents
            .iter()
            .map(|(name, value)| format!("{}  {name}\n", util::sha256_hex(value.as_bytes())))
            .collect();
        assert_eq!(
            fs::read_to_string(dir.path().join("SHA256SUMS")).unwrap(),
            expected
        );
        fs::remove_file(dir.path().join("windows-x86_64.package")).unwrap();
        assert!(write_checksums(&manifest, dir.path(), "latest.json").is_err());
    }

    /// Fixture signing key and its base64-wrapped public key.
    fn signing_fixture() -> (SecretKey, String) {
        let fixtures = "crates/newport-release/tests/fixtures";
        let key = newport_release::secret_key(
            &fixture(&format!("{fixtures}/empty-password.key")),
            String::new(),
        )
        .unwrap();
        (
            key,
            fixture(&format!("{fixtures}/empty-password.key.pub"))
                .trim()
                .to_owned(),
        )
    }

    #[test]
    fn signed_fragments_merge_into_a_verified_release() {
        let dir = tempfile::tempdir().unwrap();
        let (key, public) = signing_fixture();
        let mut paths = Vec::new();
        for (target, file) in [
            (
                "darwin-aarch64",
                format!("{}.app.tar.gz", macos_name("1.2.3")),
            ),
            (
                "windows-x86_64",
                "Newport-1.2.3-windows-x86_64-setup.exe".into(),
            ),
            (
                "windows-aarch64",
                "Newport-1.2.3-windows-aarch64-setup.exe".into(),
            ),
        ] {
            let payload = dir.path().join(&file);
            fs::write(&payload, format!("{target} payload")).unwrap();
            let fragment = if target == "darwin-aarch64" {
                "latest.json".to_owned()
            } else {
                format!("{target}.json")
            };
            let path = write_fragment(&payload, target, &fragment, "1.2.3", &public, &key).unwrap();
            assert_eq!(path, dir.path().join(&fragment));
            let written: Manifest = util::read_json(&path).unwrap();
            let entry = &written.platforms[target];
            assert_eq!(entry.url, format!("{}{file}", base("1.2.3")));
            assert_eq!(
                entry.signature,
                fs::read_to_string(dir.path().join(format!("{file}.sig")))
                    .unwrap()
                    .trim()
            );
            assert!(written.pub_date.unwrap().ends_with('Z'));
            paths.push(path);
        }
        fs::write(
            dir.path().join(format!("{}.zip", macos_name("1.2.3"))),
            "zip",
        )
        .unwrap();
        let merged = merge(&paths, "1.2.3", dir.path()).unwrap();
        verify_packages(&merged, dir.path(), &public).unwrap();
        let output = dir.path().join("latest.json");
        merged.write(&output).unwrap();
        write_checksums(&merged, dir.path(), "latest.json").unwrap();
        let sums = fs::read_to_string(dir.path().join("SHA256SUMS")).unwrap();
        assert_eq!(sums.lines().count(), 8);
        assert!(!sums.contains("windows-x86_64.json"));
        assert!(write_fragment(
            &dir.path().join("latest.json"),
            "linux-x86_64",
            "x.json",
            "1.2.3",
            &public,
            &key
        )
        .is_err());
    }
}
