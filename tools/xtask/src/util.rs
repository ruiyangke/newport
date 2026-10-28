//! Repository paths, process helpers, product identity and the release secret boundary.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    env,
    ffi::OsStr,
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// The repository root, two levels above this crate. Not canonicalized, so
/// Windows paths keep their drive-letter form.
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repository root")
        .to_owned()
}

/// Variables `cargo run` sets for the program it runs. Passed on to a nested
/// Cargo they look like changed build inputs and rebuild crates whose build
/// scripts watch them.
fn cargo_run_variable(name: &str) -> bool {
    name.starts_with("CARGO_PKG_")
        || name.starts_with("CARGO_MANIFEST_")
        || matches!(
            name,
            "CARGO_CRATE_NAME" | "CARGO_BIN_NAME" | "CARGO_PRIMARY_PACKAGE" | "OUT_DIR"
        )
}

/// A command that runs from the repository root, without this tool's own `cargo run` variables.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut c = Command::new(program);
    c.current_dir(root());
    for (name, _) in env::vars_os() {
        if name.to_str().is_some_and(cargo_run_variable) {
            c.env_remove(name);
        }
    }
    c
}

fn describe(c: &Command) -> String {
    c.get_program().to_string_lossy().into_owned()
}

/// Run to completion with inherited stdio. The error names the program only,
/// never its arguments, which may carry credentials.
pub fn run(c: &mut Command) -> Result<()> {
    let status = c
        .status()
        .with_context(|| format!("Cannot start {}", describe(c)))?;
    ensure!(status.success(), "{} failed: {status}", describe(c));
    Ok(())
}

/// Run and return trimmed UTF-8 stdout; stderr is inherited.
pub fn capture(c: &mut Command) -> Result<String> {
    let output = c
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("Cannot start {}", describe(c)))?;
    ensure!(
        output.status.success(),
        "{} failed: {}",
        describe(c),
        output.status
    );
    Ok(String::from_utf8(output.stdout)
        .with_context(|| format!("{} printed invalid UTF-8", describe(c)))?
        .trim()
        .to_owned())
}

/// True when the program starts and exits successfully; output is discarded.
pub fn succeeds(c: &mut Command) -> bool {
    c.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).with_context(|| format!("Cannot read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("Invalid JSON: {}", path.display()))
}

/// Product identity from `resources/app.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct App {
    #[serde(rename = "productName")]
    pub product_name: String,
    pub version: String,
    pub identifier: String,
}

pub fn app() -> Result<App> {
    read_json(&root().join("resources/app.json"))
}

/// The updater public key from `resources/updater.json`, base64-wrapped as stored.
pub fn updater_public_key() -> Result<String> {
    #[derive(Deserialize)]
    struct Updater {
        pubkey: String,
    }
    Ok(read_json::<Updater>(&root().join("resources/updater.json"))?.pubkey)
}

/// Versioned website payload directory, from the updater endpoint.
pub fn release_download_base(version: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Updater {
        endpoints: Vec<String>,
    }
    let updater: Updater = read_json(&root().join("resources/updater.json"))?;
    let endpoint = updater
        .endpoints
        .first()
        .context("resources/updater.json has no endpoint")?;
    let base = endpoint
        .strip_suffix("/latest.json")
        .context("Unexpected updater endpoint")?;
    ensure!(
        !version.is_empty()
            && version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'))
            && version != "."
            && version != "..",
        "Invalid release version"
    );
    Ok(format!("{base}/{version}/"))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn sha256_file(path: &Path) -> Result<String> {
    Ok(sha256_hex(&fs::read(path).with_context(|| {
        format!("Cannot read {}", path.display())
    })?))
}

pub fn git_head() -> Result<String> {
    capture(command("git").args(["rev-parse", "HEAD"]))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Macos,
    Windows,
    Linux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    pub fn as_str(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Os {
    pub fn as_str(self) -> &'static str {
        match self {
            Os::Macos => "macos",
            Os::Windows => "windows",
            Os::Linux => "linux",
        }
    }
}

/// The platform a build produces: the explicit target, else `CARGO_BUILD_TARGET`, else the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform {
    pub os: Os,
    pub arch: Arch,
}

impl Platform {
    pub fn host() -> Self {
        Self {
            os: if cfg!(target_os = "macos") {
                Os::Macos
            } else if cfg!(windows) {
                Os::Windows
            } else {
                Os::Linux
            },
            arch: if cfg!(target_arch = "aarch64") {
                Arch::Aarch64
            } else {
                Arch::X86_64
            },
        }
    }

    pub fn from_triple(triple: &str) -> Result<Self> {
        let arch = match triple.split('-').next() {
            Some("x86_64") => Arch::X86_64,
            Some("aarch64") => Arch::Aarch64,
            _ => bail!("Unsupported target architecture: {triple}"),
        };
        let os = if triple.contains("-apple-darwin") {
            Os::Macos
        } else if triple.contains("-pc-windows-") {
            Os::Windows
        } else if triple.contains("-linux-") {
            Os::Linux
        } else {
            bail!("Unsupported target: {triple}")
        };
        Ok(Self { os, arch })
    }

    pub fn of(target: Option<&str>) -> Result<Self> {
        match target {
            Some(triple) => Self::from_triple(triple),
            None => Ok(Self::host()),
        }
    }
}

/// The explicit `--target`, else `CARGO_BUILD_TARGET`.
pub fn build_target(explicit: Option<&str>) -> Option<String> {
    explicit.map(str::to_owned).or_else(|| {
        env::var("CARGO_BUILD_TARGET")
            .ok()
            .filter(|t| !t.is_empty())
    })
}

/// The root workspace's Cargo target directory, honoring `CARGO_TARGET_DIR` and config.
pub fn target_directory() -> Result<PathBuf> {
    #[derive(Deserialize)]
    struct Metadata {
        target_directory: PathBuf,
    }
    let json = capture(command("cargo").args([
        "metadata",
        "--format-version",
        "1",
        "--no-deps",
        "--locked",
    ]))?;
    Ok(serde_json::from_str::<Metadata>(&json)?.target_directory)
}

/// `<target-dir>[/<triple>]/<debug|release>` for the root workspace.
pub fn profile_directory(target: Option<&str>, release: bool) -> Result<PathBuf> {
    let mut path = target_directory()?;
    if let Some(triple) = target {
        path.push(triple);
    }
    path.push(if release { "release" } else { "debug" });
    Ok(path)
}

/// Secrets that must never reach a build, a build script or an app under test.
pub const RELEASE_SECRETS: &[&str] = &[
    "NEWPORT_SIGNING_PRIVATE_KEY",
    "NEWPORT_SIGNING_PRIVATE_KEY_PATH",
    "NEWPORT_SIGNING_PRIVATE_KEY_PASSWORD",
    "APPLE_PASSWORD",
    "APPLE_CERTIFICATE",
    "APPLE_CERTIFICATE_PASSWORD",
];

/// Remove [`RELEASE_SECRETS`] from a child's environment.
pub fn without_release_secrets(c: &mut Command) -> &mut Command {
    for name in RELEASE_SECRETS {
        c.env_remove(name);
    }
    c
}

/// Where the update signing key comes from.
#[derive(Debug, PartialEq, Eq)]
pub enum KeySource {
    Inline(String),
    File(PathBuf),
}

/// Resolve the signing key source from `NEWPORT_SIGNING_PRIVATE_KEY` or `_PATH`.
/// A single-line inline value that names an existing file is read as a path.
pub fn key_source(var: impl Fn(&str) -> Option<String>) -> Result<KeySource> {
    let key = var("NEWPORT_SIGNING_PRIVATE_KEY").filter(|v| !v.is_empty());
    let path = var("NEWPORT_SIGNING_PRIVATE_KEY_PATH").filter(|v| !v.is_empty());
    match (key, path) {
        (Some(_), Some(_)) => bail!("Supply a signing key or a key path, not both"),
        (None, None) => {
            bail!("NEWPORT_SIGNING_PRIVATE_KEY or NEWPORT_SIGNING_PRIVATE_KEY_PATH is required")
        }
        (None, Some(path)) => Ok(KeySource::File(path.into())),
        (Some(key), None) => {
            if !key.contains('\n') && key.len() < 1024 && Path::new(&key).is_file() {
                Ok(KeySource::File(key.into()))
            } else {
                Ok(KeySource::Inline(key))
            }
        }
    }
}

/// The update signing key from the process environment; the password defaults to empty.
pub fn signing_key() -> Result<newport_release::SecretKey> {
    let var = |name: &str| env::var(name).ok();
    let encoded = match key_source(var)? {
        KeySource::Inline(key) => key,
        KeySource::File(path) => {
            fs::read_to_string(&path).context("Cannot read signing key file")?
        }
    };
    let password = env::var("NEWPORT_SIGNING_PRIVATE_KEY_PASSWORD").unwrap_or_default();
    newport_release::secret_key(&encoded, password)
}

pub fn read_plist(path: &Path) -> Result<plist::Value> {
    plist::Value::from_file(path)
        .with_context(|| format!("Invalid property list: {}", path.display()))
}

/// Append `name=value` to `$GITHUB_OUTPUT`.
pub fn github_output(name: &str, value: &str) -> Result<()> {
    append_github_file("GITHUB_OUTPUT", &format!("{name}={value}\n"))
}

/// Append raw lines to the file named by a GitHub Actions variable such as `GITHUB_ENV`.
pub fn append_github_file(variable: &str, text: &str) -> Result<()> {
    let path = env::var_os(variable).with_context(|| format!("{variable} is not set"))?;
    fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
        .with_context(|| format!("Cannot open {variable}"))?
        .write_all(text.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn release_secrets_are_removed_from_children() {
        let mut c = Command::new("true");
        c.env("NEWPORT_SIGNING_PRIVATE_KEY", "secret")
            .env("APPLE_PASSWORD", "secret")
            .env("CARGO_TARGET_DIR", "target");
        without_release_secrets(&mut c);
        let envs: HashMap<_, _> = c.get_envs().collect();
        for name in RELEASE_SECRETS {
            assert_eq!(envs.get(OsStr::new(name)), Some(&None), "{name}");
        }
        assert_eq!(
            envs.get(OsStr::new("CARGO_TARGET_DIR")),
            Some(&Some(OsStr::new("target")))
        );
    }

    #[test]
    fn signing_key_source_accepts_exactly_one_form() {
        assert!(
            key_source(vars(&[]))
                .unwrap_err()
                .to_string()
                .contains("required")
        );
        assert!(
            key_source(vars(&[
                ("NEWPORT_SIGNING_PRIVATE_KEY", "a"),
                ("NEWPORT_SIGNING_PRIVATE_KEY_PATH", "b"),
            ]))
            .unwrap_err()
            .to_string()
            .contains("not both")
        );
        assert_eq!(
            key_source(vars(&[("NEWPORT_SIGNING_PRIVATE_KEY_PATH", "/k")])).unwrap(),
            KeySource::File("/k".into())
        );
        assert_eq!(
            key_source(vars(&[("NEWPORT_SIGNING_PRIVATE_KEY", "line1\nline2")])).unwrap(),
            KeySource::Inline("line1\nline2".into())
        );
        let file = tempfile::NamedTempFile::new().unwrap();
        let path = file.path().to_str().unwrap();
        assert_eq!(
            key_source(vars(&[("NEWPORT_SIGNING_PRIVATE_KEY", path)])).unwrap(),
            KeySource::File(path.into())
        );
    }

    #[test]
    fn targets_map_to_platforms() {
        assert_eq!(
            Platform::from_triple("aarch64-pc-windows-msvc").unwrap(),
            Platform {
                os: Os::Windows,
                arch: Arch::Aarch64
            }
        );
        assert_eq!(
            Platform::from_triple("x86_64-apple-darwin").unwrap(),
            Platform {
                os: Os::Macos,
                arch: Arch::X86_64
            }
        );
        assert!(Platform::from_triple("riscv64gc-unknown-linux-gnu").is_err());
    }

    #[test]
    fn release_downloads_come_from_the_update_website() {
        assert_eq!(
            release_download_base("1.2.3").unwrap(),
            "https://newport.app/updates/1.2.3/"
        );
        for version in ["", "..", "../1.2.3", "1.2.3?download", "1.2.3#fragment"] {
            assert!(release_download_base(version).is_err());
        }
    }
}
