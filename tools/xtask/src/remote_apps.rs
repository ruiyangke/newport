//! Newport with Remote apps (`--remote-apps` on `dev`, `build` and `test
//! remote-apps`), from a private newport-remote checkout.
//!
//! `NEWPORT_REMOTE_DIR` names the checkout (default: `../newport-remote`,
//! beside this repository). Without it, build the free edition.
//!
//! The streaming crates are added to the UI and application composition with
//! `cargo add` for the build only. `NEWPORT_REMOTE_APPS=1` enables Remote apps. A guard
//! takes them out of the manifests and lockfile again when the build ends,
//! fails or is interrupted, so the checkout never keeps a reference to the
//! private crates; a run that was killed outright is cleaned up by the next
//! `dev`, `build` or `test`.
//!
//! Remote Apps uses the shared full agent payloads. NEWPORT_AGENT_RESOURCE_DIR
//! supplies a verified binary export; local builds can create one inside the
//! private checkout. NEWPORT_PREBUILT never rebuilds it. Servers receive binaries.
//!
//! CI builds the newport-remote commit named in [`REVISION`]; a local build
//! uses the checkout as it is and says when it differs.
//!
//! Remote app windows run inside Newport's GPUI application, so the streaming
//! crates must use this GPUI, not a second copy: their crates.io `gpui-pre`
//! crates are patched to the revision this lockfile uses.
mod publication;

use crate::{command, root, util};
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

/// Private dependency names to clean, including the previous archive installer.
/// New builds omit newport-remote-app and use the shared backend lifecycle.
const CRATES: [&str; 6] = [
    "newport-remote-client",
    "newport-remote-config",
    "newport-remote-protocol",
    "newport-remote-provider-gpui",
    "newport-remote-transport",
    "newport-remote-app",
];
const GPUI_MANIFEST: &str = "crates/newport-gpui/Cargo.toml";
const APP_MANIFEST: &str = "crates/newport-app/Cargo.toml";
/// The files a Remote apps build edits while it runs.
pub const EDITED: [&str; 3] = [GPUI_MANIFEST, APP_MANIFEST, "Cargo.lock"];
const COMPOSITION: [(&str, &str); 2] = [
    ("newport-gpui", GPUI_MANIFEST),
    ("newport-app", APP_MANIFEST),
];
const APP_CRATES: [&str; 4] = [
    "newport-remote-client",
    "newport-remote-protocol",
    "newport-remote-provider-gpui",
    "newport-remote-transport",
];
/// The newport-remote commit CI and releases build with.
pub const REVISION: &str = ".github/newport-remote-revision";
/// Server architectures, as Newport names them and as Docker does.
const ARCHITECTURES: [(&str, &str); 2] = [("x86_64", "amd64"), ("aarch64", "arm64")];

/// The newport-remote checkout.
fn checkout() -> Result<PathBuf> {
    let newport = root();
    let remote = std::env::var_os("NEWPORT_REMOTE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| newport.join("../newport-remote"));
    ensure!(
        remote.join("crates/newport-remote-client").is_dir(),
        "No newport-remote checkout at {}. Set NEWPORT_REMOTE_DIR, or leave out --remote-apps.",
        remote.display()
    );
    let remote = remote.canonicalize()?;
    ensure!(
        !remote.starts_with(newport.canonicalize()?),
        "Keep the newport-remote checkout outside {}.",
        newport.display()
    );
    Ok(remote)
}

/// Takes the private crates out of Newport's manifests and lockfile, keeping
/// every other edit. Also what every build does first, after a killed run.
pub fn clean() -> Result<()> {
    clean_at(&root())
}
fn clean_at(newport: &Path) -> Result<()> {
    for (package, path) in COMPOSITION {
        let manifest = fs::read_to_string(newport.join(path))?;
        let present: Vec<&str> = CRATES
            .iter()
            .copied()
            .filter(|name| {
                manifest
                    .lines()
                    .any(|line| line.starts_with(&format!("{name} = ")))
            })
            .collect();
        if !present.is_empty() {
            // `cargo remove` also drops profile overrides for packages that
            // aren't direct dependencies (the root manifest's scrypt and salsa20
            // speed-ups). Nothing here edits the root manifest: put it back.
            let root_manifest = fs::read(newport.join("Cargo.toml"))?;
            let removed = util::run(
                command("cargo")
                    .current_dir(newport)
                    .args(["remove", "--quiet", "-p", package])
                    .args(&present),
            );
            fs::write(newport.join("Cargo.toml"), root_manifest)?;
            removed?;
        }
    }
    // Resolving offline drops the private packages from the lockfile and
    // leaves every other entry as it was.
    util::run(
        command("cargo")
            .current_dir(newport)
            .args(["metadata", "--format-version", "1", "--offline"])
            .stdout(std::process::Stdio::null()),
    )?;
    for file in ["Cargo.lock", GPUI_MANIFEST, APP_MANIFEST] {
        ensure!(
            !fs::read_to_string(newport.join(file))?.contains("newport-remote"),
            "{file} still names newport-remote."
        );
    }
    Ok(())
}

/// Whether a killed Remote apps build left the private crates in.
pub fn leftover() -> Result<bool> {
    leftover_at(&root())
}
fn leftover_at(newport: &Path) -> Result<bool> {
    for (_, manifest) in COMPOSITION {
        if fs::read_to_string(newport.join(manifest))?.contains("newport-remote") {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Set by Ctrl-C or termination while the private crates are in.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Stops between steps after Ctrl-C; the steps' own commands stop on it.
fn check_interrupted() -> Result<()> {
    ensure!(!INTERRUPTED.load(Ordering::SeqCst), "Interrupted");
    Ok(())
}

/// The private crates are in the manifests while this lives. Dropping it
/// takes them out again; if that fails, the files from before are put back.
/// Ctrl-C and termination don't end this process while it lives: the build's
/// commands get the same signal and stop, the failure returns, and the drop
/// cleans up. One path, so cleanup is never cut short.
pub struct Added {
    saved: Mutex<Option<Saved>>,
}
struct Saved {
    lock: Vec<u8>,
    manifests: Vec<(&'static str, Vec<u8>)>,
}

impl Added {
    fn new() -> Result<Self> {
        let newport = root();
        let saved = Mutex::new(Some(Saved {
            lock: fs::read(newport.join("Cargo.lock"))?,
            manifests: COMPOSITION
                .iter()
                .map(|(_, path)| Ok((*path, fs::read(newport.join(path))?)))
                .collect::<std::io::Result<_>>()?,
        }));
        // Once per process: there is only ever one guard.
        ctrlc::set_handler(|| INTERRUPTED.store(true, Ordering::SeqCst))
            .context("Cannot watch for Ctrl-C during the Remote apps build")?;
        Ok(Self { saved })
    }
}

impl Drop for Added {
    fn drop(&mut self) {
        // Held throughout, so cleanup never runs twice at once.
        let mut saved = self.saved.lock().unwrap_or_else(|e| e.into_inner());
        let Some(saved) = saved.take() else {
            return;
        };
        if let Err(error) = clean() {
            eprintln!(
                "{error:#}\nRestoring Cargo.lock and composition manifests from before the build."
            );
            let newport = root();
            let _ = fs::write(newport.join("Cargo.lock"), saved.lock);
            for (path, bytes) in saved.manifests {
                let _ = fs::write(newport.join(path), bytes);
            }
        }
    }
}

/// Everything a Remote apps build needs: the crates added, the agents and
/// archives ready, and the cargo arguments that patch GPUI. Keep the guard
/// alive until the build ends.
pub struct Prepared {
    /// Held for its drop: the crates leave the manifests with it.
    _guard: Added,
    /// `--config` arguments for this GPUI.
    pub patch: Vec<String>,
    /// `NEWPORT_REMOTE_APPS` and the agent archives, for the build.
    pub env: Vec<(String, PathBuf)>,
}

pub fn prepare() -> Result<Prepared> {
    let newport = root();
    let remote = checkout()?;
    check_revision(&newport, &remote)?;
    let resources = full_agent_resources(&newport, &remote)?;
    let (url, rev) = gpui_source(&fs::read_to_string(newport.join("Cargo.lock"))?)?;
    let guard = Added::new()?;
    for name in CRATES
        .into_iter()
        .filter(|name| *name != "newport-remote-app")
    {
        check_interrupted()?;
        let mut add = command("cargo");
        add.current_dir(&newport)
            .args(["add", "--quiet", "-p", "newport-gpui", "--path"]);
        add.arg(remote.join("crates").join(name));
        util::run(&mut add)?;
    }
    for name in APP_CRATES {
        check_interrupted()?;
        util::run(
            command("cargo")
                .current_dir(&newport)
                .args(["add", "--quiet", "-p", "newport-app", "--path"])
                .arg(remote.join("crates").join(name)),
        )?;
    }
    check_interrupted()?;
    let patch: Vec<String> = ["gpui-pre", "gpui-pre-platform"]
        .iter()
        .flat_map(|name| {
            [
                format!("--config=patch.crates-io.{name}.git='{url}'"),
                format!("--config=patch.crates-io.{name}.rev='{rev}'"),
            ]
        })
        .collect();
    // Resolve the added crates once, and refuse a graph that would still
    // contain a second GPUI.
    let tree = crate::process::text(
        command("cargo")
            .current_dir(&newport)
            .args([
                "tree",
                "--quiet",
                "-p",
                "newport-app",
                "--duplicates",
                "--depth",
                "0",
                "-e",
                "normal",
            ])
            .args(&patch),
    )?;
    let duplicates: Vec<&str> = tree
        .lines()
        .filter(|line| line.starts_with("gpui-pre ") || line.starts_with("gpui-pre-platform "))
        .collect();
    if !duplicates.is_empty() {
        bail!(
            "Two copies of GPUI would be built:\n{}",
            duplicates.join("\n")
        );
    }
    check_interrupted()?;
    let mut env = vec![
        ("NEWPORT_REMOTE_APPS".to_owned(), PathBuf::from("1")),
        // The client's Opus (audiopus_sys) builds its bundled copy with CMake
        // where no system Opus is found, as on CI; its CMakeLists predates
        // CMake 3.5, which CMake 4 refuses without this.
        (
            "CMAKE_POLICY_VERSION_MINIMUM".to_owned(),
            PathBuf::from("3.5"),
        ),
    ];
    env.push(("NEWPORT_AGENT_RESOURCE_DIR".into(), resources));
    Ok(Prepared {
        _guard: guard,
        patch,
        env,
    })
}

/// Full binaries are composed in the private checkout. Only the exported
/// binary resources cross into desktop packaging; public core stays independent.
fn full_agent_resources(newport: &Path, remote: &Path) -> Result<PathBuf> {
    if let Some(directory) = std::env::var_os("NEWPORT_AGENT_RESOURCE_DIR") {
        return checked_full_resources(newport, &PathBuf::from(directory));
    }
    let directory = remote.join("target/unified-agent/desktop-resources");
    if prebuilt() {
        return checked_full_resources(newport, &directory);
    }
    ensure!(
        std::env::var_os("CI").is_none(),
        "CI must consume verified full agent resources through NEWPORT_AGENT_RESOURCE_DIR"
    );
    util::run(
        Command::new("python3")
            .current_dir(remote)
            .arg(remote.join("scripts/build-unified-agent.py"))
            .arg("--core-dir")
            .arg(newport)
            .arg("--development"),
    )?;
    let temporary = tempfile::tempdir_in(remote.join("target/unified-agent"))?;
    let fresh = temporary.path().join("resources");
    util::run(
        Command::new("python3")
            .current_dir(remote)
            .arg(remote.join("scripts/export-unified-agent.py"))
            .arg("--output")
            .arg(&fresh),
    )?;
    checked_full_resources(newport, &fresh)?;
    publication::publish(&fresh, &directory, |published| {
        checked_full_resources(newport, published)
    })
}
pub(crate) fn checked_full_resources(newport: &Path, directory: &Path) -> Result<PathBuf> {
    crate::artifact::verify(directory).context(
        "Full agent export must match this public commit and its exact artifact manifest",
    )?;
    let private_revision = fs::read_to_string(newport.join(REVISION))?;
    verify_full_provenance(
        &fs::read(directory.join("full-agent-provenance.json"))?,
        &util::git_head()?,
        private_revision.trim(),
        std::env::var_os("CI").is_some()
            || std::env::var_os("NEWPORT_REQUIRE_NEEDLE_RELEASE").is_some(),
    )?;
    for (arch, _) in ARCHITECTURES {
        let binary = directory.join(format!("newport-agent-{arch}"));
        let identity = directory.join(format!("newport-agent-{arch}.product.json"));
        ensure!(
            binary.is_file() && identity.is_file(),
            "Full agent resources are missing in {}. Build/export them in newport-remote or set NEWPORT_AGENT_RESOURCE_DIR.",
            directory.display()
        );
        let product: serde_json::Value = serde_json::from_slice(&fs::read(identity)?)?;
        ensure!(
            product["product"]["variant"] == "full",
            "Remote Apps requires the full agent product for {arch}"
        );
    }
    // The backend build additionally verifies the exact hashes, protocols and
    // signatures (for distribution builds) before staging compressed payloads.
    Ok(directory.canonicalize()?)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceIdentity {
    commit: String,
    dirty: bool,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SourcePair {
    core: SourceIdentity,
    remote: SourceIdentity,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FullAgentProvenance {
    schema: u32,
    variant: String,
    development: bool,
    sources: SourcePair,
    #[serde(rename = "sourceSha256")]
    source_sha256: String,
}

fn verify_full_provenance(
    bytes: &[u8],
    public_revision: &str,
    private_revision: &str,
    production: bool,
) -> Result<()> {
    let provenance: FullAgentProvenance = serde_json::from_slice(bytes)?;
    let sha = |value: &str, len| {
        value.len() == len
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    ensure!(
        provenance.schema == 1
            && provenance.variant == "full"
            && sha(&provenance.source_sha256, 64)
            && sha(&provenance.sources.core.commit, 40)
            && sha(&provenance.sources.remote.commit, 40)
            && provenance.sources.core.commit == public_revision,
        "Invalid full agent provenance or mismatched public source revision"
    );
    let dirty = provenance.sources.core.dirty || provenance.sources.remote.dirty;
    ensure!(
        provenance.development || !dirty,
        "Dirty full-agent sources must be marked as development"
    );
    ensure!(
        !production
            || (!provenance.development
                && !dirty
                && provenance.sources.remote.commit == private_revision),
        "CI requires a clean production full-agent export matching the public/private source pair"
    );
    Ok(())
}

impl Prepared {
    pub fn apply<'a>(&self, cargo: &'a mut Command) -> &'a mut Command {
        cargo.args(&self.patch);
        for (name, value) in &self.env {
            cargo.env(name, value);
        }
        cargo
    }
}

/// `cargo xtask remote-agents`: build the agent archives, or only those for
/// `architectures` (x86_64, aarch64), as Remote apps builds embed them.
pub fn build_archives(architectures: &[String]) -> Result<()> {
    let newport = root();
    let remote = checkout()?;
    check_install_marker(&newport, &remote)?;
    check_revision(&newport, &remote)?;
    let selected: Vec<_> = if architectures.is_empty() {
        ARCHITECTURES.to_vec()
    } else {
        architectures
            .iter()
            .map(|name| {
                ARCHITECTURES
                    .into_iter()
                    .find(|(arch, _)| arch == name)
                    .with_context(|| format!("Unknown architecture {name}; use x86_64 or aarch64"))
            })
            .collect::<Result<_>>()?
    };
    for (_, archive) in agent_archives(&newport, &remote, &selected)? {
        println!("{}", archive.display());
    }
    Ok(())
}

fn prebuilt() -> bool {
    std::env::var("NEWPORT_PREBUILT").as_deref() == Ok("1")
}

fn archive_path(newport: &Path, arch: &str) -> PathBuf {
    newport.join(format!("target/remote-agent/{arch}.tar.gz"))
}

fn archive_variable(arch: &str) -> String {
    format!("NEWPORT_REMOTE_AGENT_{}", arch.to_uppercase())
}

/// The compiled agent archives Install sends, at stable paths so an
/// unchanged agent doesn't rebuild Newport. Static agents are rebuilt first
/// when anything they are built from is newer.
fn agent_archives(
    newport: &Path,
    remote: &Path,
    architectures: &[(&str, &str)],
) -> Result<Vec<(String, PathBuf)>> {
    let directory = newport.join("target/remote-agent");
    fs::create_dir_all(&directory)?;
    util::run(
        Command::new("sh")
            .current_dir(remote)
            .arg(remote.join("scripts/ensure-static-agent.sh"))
            .args(architectures.iter().map(|(_, docker)| docker)),
    )?;
    let mut env = Vec::new();
    for (arch, _) in architectures {
        let archive = archive_path(newport, arch);
        let fresh = directory.join(format!("{arch}.tar.gz.new"));
        util::run(
            command("cargo")
                .current_dir(remote)
                .args(["xtask", "deploy-agent", "--archive"])
                .arg(&fresh)
                .args(["--arch", arch]),
        )?;
        if fs::read(&fresh)? == fs::read(&archive).unwrap_or_default() {
            fs::remove_file(&fresh)?;
        } else {
            fs::rename(&fresh, &archive)?;
        }
        env.push((archive_variable(arch), archive));
    }
    Ok(env)
}

/// Newport's backend finds the installed agent by the marker newport-remote's
/// installer writes into each validated generation. The two live in separate
/// repositories, so a build checks they still agree.
fn check_install_marker(newport: &Path, remote: &Path) -> Result<()> {
    let installer = fs::read_to_string(remote.join("crates/newport-remote-app/src/deploy.sh"))?;
    let backend = fs::read_to_string(newport.join("crates/newport-backend/src/remote_apps.rs"))?;
    let installed = markers(&installer);
    let found = markers(&backend);
    ensure!(
        installed.len() == 1 && found == installed,
        "newport-remote's installer marks agents {installed:?}, but Newport's backend looks for {found:?}. Update crates/newport-backend/src/remote_apps.rs."
    );
    Ok(())
}

/// CI must build the pinned newport-remote commit; locally a different one is
/// fine but worth knowing, since CI won't build it until the pin moves.
fn check_revision(newport: &Path, remote: &Path) -> Result<()> {
    let pinned = fs::read_to_string(newport.join(REVISION))
        .with_context(|| format!("Cannot read {REVISION}"))?;
    let pinned = pinned.trim();
    ensure!(
        pinned.len() == 40 && pinned.bytes().all(|b| b.is_ascii_hexdigit()),
        "{REVISION} must hold one full commit hash"
    );
    let head = util::capture(
        Command::new("git")
            .arg("-C")
            .arg(remote)
            .args(["rev-parse", "HEAD"]),
    );
    match head {
        Ok(head) if head.trim() == pinned => Ok(()),
        _ if std::env::var_os("CI").is_some() => {
            bail!("CI must build newport-remote {pinned} ({REVISION})")
        }
        Ok(head) => {
            eprintln!(
                "newport-remote is at {}; CI builds {pinned} ({REVISION})",
                head.trim()
            );
            Ok(())
        }
        Err(_) => {
            eprintln!("Cannot tell newport-remote's commit; CI builds {pinned} ({REVISION})");
            Ok(())
        }
    }
}

/// The distinct `.validated-vN` markers a script names.
fn markers(script: &str) -> std::collections::BTreeSet<&str> {
    script
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-'))
        .filter(|word| word.starts_with(".validated-v"))
        .collect()
}

/// This GPUI's git URL and revision: `source = "git+URL?rev=REV#REV"` for
/// gpui-pre in the lockfile. The lockfile may also list a crates.io gpui-pre
/// that Newport doesn't build with, so take the git one.
fn gpui_source(lock: &str) -> Result<(String, String)> {
    let lock: toml::Value = toml::from_str(lock).context("Cannot read Cargo.lock")?;
    let packages = lock["package"]
        .as_array()
        .context("Cargo.lock lists no packages")?;
    let source = packages
        .iter()
        .filter(|package| package["name"].as_str() == Some("gpui-pre"))
        .filter_map(|package| package.get("source")?.as_str())
        .find_map(|source| source.strip_prefix("git+"))
        .context("Cargo.lock has no git revision for gpui-pre")?;
    let (url, rest) = source
        .split_once("?rev=")
        .context("gpui-pre's git source has no revision")?;
    let rev = rest.rsplit('#').next().unwrap_or(rest);
    ensure!(
        !rev.is_empty() && rev.bytes().all(|b| b.is_ascii_hexdigit()),
        "gpui-pre's git revision is not a commit: {rev}"
    );
    Ok((url.to_owned(), rev.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_export_provenance_distinguishes_development_and_the_exact_production_pair() {
        let public = "a".repeat(40);
        let private = "b".repeat(40);
        let mut provenance = serde_json::json!({
            "schema": 1, "variant": "full", "development": false,
            "sources": {"core": {"commit": public, "dirty": false},
                        "remote": {"commit": private, "dirty": false}},
            "sourceSha256": "c".repeat(64),
        });
        let check = |value: &serde_json::Value, production| {
            verify_full_provenance(
                &serde_json::to_vec(value).unwrap(),
                &public,
                &private,
                production,
            )
        };
        check(&provenance, true).unwrap();
        provenance["sources"]["remote"]["commit"] = "d".repeat(40).into();
        assert!(check(&provenance, true).is_err());
        provenance["development"] = true.into();
        provenance["sources"]["remote"]["dirty"] = true.into();
        check(&provenance, false).unwrap();
        assert!(check(&provenance, true).is_err());
        provenance["development"] = false.into();
        assert!(check(&provenance, false).is_err());
        provenance["development"] = true.into();
        provenance["sources"]["core"]["commit"] = "e".repeat(40).into();
        assert!(check(&provenance, false).is_err());
        provenance["sources"]["core"]["commit"] = public.clone().into();
        provenance["sourceSha256"] = "unverified".into();
        assert!(check(&provenance, false).is_err());
    }

    #[test]
    fn cleanup_recovers_app_only_private_dependencies_and_preserves_user_edits() {
        let directory = tempfile::tempdir().unwrap();
        let public = directory.path().join("public");
        let private = directory.path().join("private");
        fs::create_dir_all(private.join("src")).unwrap();
        fs::write(private.join("Cargo.toml"), "[package]\nname = 'newport-remote-client'\nversion = '0.1.0'\nedition = '2021'\n[workspace]\n").unwrap();
        fs::write(private.join("src/lib.rs"), "").unwrap();
        for (name, path) in COMPOSITION {
            let manifest = public.join(path);
            let root = manifest.parent().unwrap();
            fs::create_dir_all(root.join("src")).unwrap();
            fs::write(root.join("src/lib.rs"), "").unwrap();
            let mut content = format!(
                "# User's uncommitted preference\n[package]\nname = '{name}'\nversion = '0.1.0'\nedition = '2021'\n[dependencies]\n"
            );
            if name == "newport-app" {
                content.push_str("newport-remote-client = { path = '../../../private' }\n");
            }
            fs::write(manifest, content).unwrap();
        }
        let manifest = "# Keep root settings\n[workspace]\nresolver = '2'\nmembers = ['crates/newport-app', 'crates/newport-gpui']\n[profile.dev]\ndebug = 0\n";
        fs::write(public.join("Cargo.toml"), manifest).unwrap();
        let ui = fs::read(public.join(GPUI_MANIFEST)).unwrap();
        assert!(leftover_at(&public).unwrap());
        clean_at(&public).unwrap();
        assert!(!leftover_at(&public).unwrap());
        assert_eq!(
            fs::read_to_string(public.join("Cargo.toml")).unwrap(),
            manifest
        );
        assert_eq!(fs::read(public.join(GPUI_MANIFEST)).unwrap(), ui);
        let app = fs::read_to_string(public.join(APP_MANIFEST)).unwrap();
        assert!(app.starts_with("# User's uncommitted preference\n"));
        assert!(!app.contains("newport-remote"));
        assert!(
            !fs::read_to_string(public.join("Cargo.lock"))
                .unwrap()
                .contains("newport-remote")
        );
        clean_at(&public).unwrap(); // Idempotent after an interrupted cleanup.
    }

    #[test]
    fn install_markers_are_read_from_either_script() {
        let installer = "touch .validated-v1\n[ -f \"$active/.validated-v1\" ]";
        assert_eq!(
            markers(installer).into_iter().collect::<Vec<_>>(),
            [".validated-v1"]
        );
        let backend = r#"[ -f "$release/current/.validated-v6" ]"#;
        assert_eq!(
            markers(backend).into_iter().collect::<Vec<_>>(),
            [".validated-v6"]
        );
        assert!(markers("no markers here").is_empty());
    }

    #[test]
    fn gpui_source_takes_the_git_revision() {
        let lock = r#"
version = 4
[[package]]
name = "gpui-pre"
version = "0.2.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
[[package]]
name = "gpui-pre"
version = "0.2.0"
source = "git+https://github.com/example/gpui?rev=abc123#abc123"
"#;
        assert_eq!(
            gpui_source(lock).unwrap(),
            ("https://github.com/example/gpui".into(), "abc123".into())
        );
        assert!(gpui_source("version = 4\n[[package]]\nname = \"gpui-pre\"\n").is_err());
    }
}
