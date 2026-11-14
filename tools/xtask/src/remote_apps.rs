//! Newport with Remote apps (`--remote-apps` on `dev`, `build` and `test
//! remote-apps`), from a private newport-remote checkout at `remote/`, which
//! Newport's Git ignores.
//!
//! A clone without it, where `remote/` is missing or empty, builds the free
//! edition.
//!
//! The streaming crates are added to the UI and application composition with
//! `cargo add` for the build only. `NEWPORT_REMOTE_APPS=1` enables Remote apps. A guard
//! takes them out of the manifests and lockfile again when the build ends,
//! fails or is interrupted, so the checkout never keeps a reference to the
//! private crates; a run that was killed outright is cleaned up by the next
//! `dev`, `build` or `test`.
//!
//! Remote Apps uses the shared full agent payloads. NEWPORT_AGENT_RESOURCE_DIR
//! supplies a verified binary export; local builds create one in
//! `remote/target` (see `full_agent`). NEWPORT_PREBUILT never rebuilds it.
//! Servers receive binaries.
//!
//! Remote app windows run inside Newport's GPUI application, so the streaming
//! crates must use this GPUI, not a second copy: Newport's `[patch.crates-io]`
//! gives their crates.io `gpui-pre` dependencies the patched GPUI in
//! `vendor/`.
pub mod full_agent;
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

/// The private crates a build adds, and cleaning takes out again.
const CRATES: [&str; 4] = [
    "newport-remote-client",
    "newport-remote-provider-gpui",
    "newport-remote-provider-stubs",
    "newport-remote-transport",
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
    "newport-remote-provider-gpui",
    "newport-remote-provider-stubs",
    "newport-remote-transport",
];
/// Only the application composition presents remote windows as Mac apps.
const APP_ONLY: [&str; 1] = ["newport-remote-provider-stubs"];
/// The per-app stub binary, built standalone with its own size profile, and
/// the variable that hands its path to bundling.
const STUB_CRATE: &str = "newport-remote-stub";
/// The stub's name inside `Contents/Helpers`, where Remote apps looks for it.
pub const STUB_NAME: &str = STUB_CRATE;
pub const STUB_VARIABLE: &str = "NEWPORT_REMOTE_STUB_BINARY";
/// Every generated app bundle carries a copy, so the stub stays small.
const MAX_STUB_BYTES: u64 = 1024 * 1024;

/// Where the stub build for `target` (the host's when none) leaves the binary.
fn stub_binary(newport: &Path, target: Option<&str>) -> PathBuf {
    let mut path = newport.join("target/remote-stub");
    if let Some(target) = target {
        path.push(target);
    }
    path.join("release").join(STUB_CRATE)
}

/// Build the stub for a package of `target`, or for this Mac's development
/// bundle when none, from the private checkout. A package builds it even when
/// the app itself isn't built, so it never carries a stub older than its
/// checkout or built for another architecture.
pub fn package_stub(target: Option<&str>) -> Result<PathBuf> {
    build_stub(&root(), &checkout()?, target)
}
/// The stub `build --remote-apps` left for this Mac's development bundle.
pub fn development_stub() -> Option<PathBuf> {
    let path = stub_binary(&root(), None);
    path.is_file().then_some(path)
}
/// Server architectures, as Newport names them and as Docker does.
const ARCHITECTURES: [(&str, &str); 2] = [("x86_64", "amd64"), ("aarch64", "arm64")];

/// The newport-remote checkout.
fn checkout() -> Result<PathBuf> {
    let remote = root().join("remote");
    ensure!(
        remote.join("crates/newport-remote-client").is_dir(),
        "No newport-remote checkout at {}. Clone newport-remote there, or leave out --remote-apps.",
        remote.display()
    );
    Ok(remote.canonicalize()?)
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
    pub composed: Composed,
    /// `NEWPORT_REMOTE_APPS` and the agent archives, for the build.
    pub env: Vec<(String, PathBuf)>,
    /// The checked Full-agent export selected for this composition.
    pub resources: PathBuf,
}

pub fn prepare() -> Result<Prepared> {
    let resources = full_agent_resources(&root(), &checkout()?)?;
    let composed = compose()?;
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
    env.push(("NEWPORT_AGENT_RESOURCE_DIR".into(), resources.clone()));
    Ok(Prepared {
        composed,
        env,
        resources,
    })
}

/// The private crates in Newport's manifests and lockfile, resolved against
/// this GPUI, for as long as it lives.
pub struct Composed {
    /// Held for its drop: the crates leave the manifests with it.
    _guard: Added,
}

/// Add the private crates to the composition, without the agents a build
/// also needs.
pub fn compose() -> Result<Composed> {
    let newport = root();
    let remote = checkout()?;
    let guard = Added::new()?;
    for name in CRATES.into_iter().filter(|name| !APP_ONLY.contains(name)) {
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
    // Resolve the added crates once, and refuse a graph that would still
    // contain a second GPUI.
    let tree = crate::process::text(command("cargo").current_dir(&newport).args([
        "tree",
        "--quiet",
        "-p",
        "newport-app",
        "--duplicates",
        "--depth",
        "0",
        "-e",
        "normal",
    ]))?;
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
    Ok(Composed { _guard: guard })
}

/// Prepare full agent resources without changing desktop dependencies.
pub fn build_full_resources() -> Result<PathBuf> {
    let newport = root();
    let remote = checkout()?;
    let resources = full_agent_resources(&newport, &remote)?;
    println!("Full agent resources verified: {}", resources.display());
    Ok(resources)
}

/// Build the stub that generated remote app bundles copy. Its target
/// directory is Newport's, so the private checkout keeps no build products.
fn build_stub(newport: &Path, remote: &Path, target: Option<&str>) -> Result<PathBuf> {
    let mut cargo = command("cargo");
    cargo
        .current_dir(remote.join("crates").join(STUB_CRATE))
        .args(["build", "--quiet", "--release", "--locked", "--target-dir"])
        .arg(newport.join("target/remote-stub"));
    if let Some(target) = target {
        cargo.args(["--target", target]);
    }
    util::run(&mut cargo)?;
    let binary = stub_binary(newport, target);
    let size = fs::metadata(&binary)
        .with_context(|| format!("The stub build left no {}", binary.display()))?
        .len();
    ensure!(
        size <= MAX_STUB_BYTES,
        "The remote app stub is {size} bytes; it must stay within {MAX_STUB_BYTES}"
    );
    // The test hook drives input from a FIFO; only test builds may have it.
    let bytes = fs::read(&binary)?;
    ensure!(
        !bytes
            .windows(17)
            .any(|window| window == b"NEWPORT_STUB_HOOK"),
        "The remote app stub was built with its test hook"
    );
    // Remote apps finds the stub by this name in Contents/Helpers.
    let provider = remote.join("crates/newport-remote-provider-stubs/src/lib.rs");
    ensure!(
        fs::read_to_string(&provider)?.contains(&format!("STUB_NAME: &str = \"{STUB_NAME}\"")),
        "{} looks for the stub under another name than {STUB_NAME}",
        provider.display()
    );
    Ok(binary)
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
    let builds = remote.join("target/unified-agent");
    full_agent::build(newport, &["x86_64", "aarch64"], false, &builds)?;
    let temporary = tempfile::tempdir_in(&builds)?;
    let fresh = temporary.path().join("resources");
    full_agent::export(&builds, &fresh, None)?;
    checked_full_resources(newport, &fresh)?;
    publication::publish(&fresh, &directory, |published| {
        checked_full_resources(newport, published)
    })
}
pub(crate) fn checked_full_resources(newport: &Path, directory: &Path) -> Result<PathBuf> {
    crate::artifact::verify(directory).context(
        "Full agent export must match this public commit and its exact artifact manifest",
    )?;
    // Only a production export is held to the private commit a release
    // builds; a development package bundles whatever remote/ had.
    let production = std::env::var_os("CI").is_some()
        || std::env::var_os("NEWPORT_REQUIRE_NEEDLE_RELEASE").is_some();
    let remote = if production {
        Some(pinned_revision(newport)?)
    } else {
        None
    };
    verify_full_provenance(
        &fs::read(directory.join(full_agent::EXPORT_PROVENANCE))?,
        &util::git_head()?,
        remote.as_deref(),
    )?;
    for (arch, _) in ARCHITECTURES {
        let binary = directory.join(format!("newport-agent-{arch}"));
        let identity = directory.join(format!("newport-agent-{arch}.product.json"));
        ensure!(
            binary.is_file() && identity.is_file(),
            "Full agent resources are missing in {}. Run `cargo xtask agents --remote-apps` or set NEWPORT_AGENT_RESOURCE_DIR.",
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

fn verify_full_provenance(
    bytes: &[u8],
    newport_revision: &str,
    production_remote: Option<&str>,
) -> Result<()> {
    let provenance: full_agent::Provenance =
        serde_json::from_slice(bytes).context("Invalid full agent provenance")?;
    provenance.check()?;
    ensure!(
        provenance.newport.commit == newport_revision,
        "The full agent export was built from another Newport commit"
    );
    if let Some(remote_revision) = production_remote {
        provenance
            .check_production(newport_revision, remote_revision)
            .context("CI requires a clean production full agent export of this commit")?;
    }
    Ok(())
}

impl Prepared {
    pub fn apply<'a>(&self, cargo: &'a mut Command) -> &'a mut Command {
        for (name, value) in &self.env {
            cargo.env(name, value);
        }
        cargo
    }
}

fn prebuilt() -> bool {
    std::env::var("NEWPORT_PREBUILT").as_deref() == Ok("1")
}

/// Release CI's newport-remote commit, for jobs that never fetch `remote/`.
const REVISION_VARIABLE: &str = "NEWPORT_REMOTE_REVISION";

/// The newport-remote commit a release builds: the newport-remote tag named
/// like the Newport release tag at HEAD. Release CI checks that tag out and
/// passes its commit in `NEWPORT_REMOTE_REVISION`, since its checkouts have no
/// tags.
fn pinned_revision(newport: &Path) -> Result<String> {
    match std::env::var_os(REVISION_VARIABLE) {
        Some(revision) => ci_revision(&revision.to_string_lossy()),
        None => tagged_revision(newport),
    }
}

fn ci_revision(revision: &str) -> Result<String> {
    ensure!(
        revision.len() == 40
            && revision
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')),
        "{REVISION_VARIABLE} must be a full newport-remote commit"
    );
    Ok(revision.to_owned())
}

fn tagged_revision(newport: &Path) -> Result<String> {
    let tag = util::capture(Command::new("git").arg("-C").arg(newport).args([
        "describe",
        "--exact-match",
        "--tags",
        "--match",
        "v*",
        "HEAD",
    ]))
    .context("A production build is made from a Newport release tag")?;
    util::capture(
        Command::new("git")
            .arg("-C")
            .arg(newport.join("remote"))
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/tags/{tag}^{{commit}}"),
            ]),
    )
    .with_context(|| format!("newport-remote has no tag {tag}; tag the commit this release builds"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_ci_names_a_full_remote_commit() {
        let commit = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(ci_revision(commit).unwrap(), commit);
        for revision in ["", "v0.7.1", &commit[..39], &commit.to_uppercase()] {
            assert!(ci_revision(revision).is_err(), "{revision}");
        }
    }

    #[test]
    fn full_export_provenance_distinguishes_development_and_the_exact_production_pair() {
        let newport = "a".repeat(40);
        let remote = "b".repeat(40);
        let mut provenance = serde_json::json!({
            "schema": 2, "variant": "full", "development": false,
            "newport": {"commit": newport, "dirty": false},
            "remote": {"commit": remote, "dirty": false},
        });
        let check = |value: &serde_json::Value, production: bool| {
            verify_full_provenance(
                &serde_json::to_vec(value).unwrap(),
                &newport,
                production.then_some(remote.as_str()),
            )
        };
        check(&provenance, true).unwrap();
        provenance["remote"]["commit"] = "d".repeat(40).into();
        assert!(check(&provenance, true).is_err());
        provenance["development"] = true.into();
        provenance["remote"]["dirty"] = true.into();
        check(&provenance, false).unwrap();
        assert!(check(&provenance, true).is_err());
        provenance["development"] = false.into();
        assert!(check(&provenance, false).is_err());
        provenance["development"] = true.into();
        provenance["newport"]["commit"] = "e".repeat(40).into();
        assert!(check(&provenance, false).is_err());
        provenance["newport"]["commit"] = newport.clone().into();
        provenance["schema"] = 1.into();
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
}
