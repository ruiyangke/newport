//! Newport with Remote apps (`--remote-apps` on `dev`, `build` and `test
//! remote-apps`), from a private newport-remote checkout.
//!
//! `NEWPORT_REMOTE_DIR` names the checkout (default: `../newport-remote`,
//! beside this repository). Without it, build the free edition.
//!
//! The streaming crates are added to newport-gpui with `cargo add` for the
//! build only, and `NEWPORT_REMOTE_APPS=1` compiles Remote apps in. A guard
//! takes them out of the manifests and lockfile again when the build ends,
//! fails or is interrupted, so the checkout never keeps a reference to the
//! private crates; a run that was killed outright is cleaned up by the next
//! `dev`, `build` or `test`.
//!
//! Install on the Remote apps page sends a compiled agent to the server; the
//! build embeds one archive per server architecture, made from newport-remote's
//! static agents, which are rebuilt in Docker when older than their sources.
//! Servers never receive source. With `NEWPORT_PREBUILT=1` the archives in
//! `target/remote-agent` are used as they are: CI packs each one once
//! (`cargo xtask remote-agents`), so every build of a commit embeds the same
//! bytes and so the same release.
//!
//! CI builds the newport-remote commit named in [`REVISION`]; a local build
//! uses the checkout as it is and says when it differs.
//!
//! Remote app windows run inside Newport's GPUI application, so the streaming
//! crates must use this GPUI, not a second copy: their crates.io `gpui-pre`
//! crates are patched to the revision this lockfile uses.
use crate::{command, root, util};
use anyhow::{bail, ensure, Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};

/// The private crates newport-gpui uses. `newport-remote-app` comes with only
/// its `install` feature: the install command and release identity.
const CRATES: [&str; 6] = [
    "newport-remote-client",
    "newport-remote-config",
    "newport-remote-protocol",
    "newport-remote-provider-gpui",
    "newport-remote-transport",
    "newport-remote-app",
];
const GPUI_MANIFEST: &str = "crates/newport-gpui/Cargo.toml";
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
    let newport = root();
    let manifest = fs::read_to_string(newport.join(GPUI_MANIFEST))?;
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
                .current_dir(&newport)
                .args(["remove", "--quiet", "-p", "newport-gpui"])
                .args(&present),
        );
        fs::write(newport.join("Cargo.toml"), root_manifest)?;
        removed?;
    }
    // Resolving offline drops the private packages from the lockfile and
    // leaves every other entry as it was.
    util::run(
        command("cargo")
            .current_dir(&newport)
            .args(["metadata", "--format-version", "1", "--offline"])
            .stdout(std::process::Stdio::null()),
    )?;
    for file in ["Cargo.lock", GPUI_MANIFEST] {
        ensure!(
            !fs::read_to_string(newport.join(file))?.contains("newport-remote"),
            "{file} still names newport-remote."
        );
    }
    Ok(())
}

/// Whether a killed Remote apps build left the private crates in.
pub fn leftover() -> Result<bool> {
    Ok(fs::read_to_string(root().join(GPUI_MANIFEST))?.contains("newport-remote"))
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
    saved: Mutex<Option<(Vec<u8>, Vec<u8>)>>,
}

impl Added {
    fn new() -> Result<Self> {
        let newport = root();
        let saved = Mutex::new(Some((
            fs::read(newport.join("Cargo.lock"))?,
            fs::read(newport.join(GPUI_MANIFEST))?,
        )));
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
        let Some((lock, manifest)) = saved.take() else {
            return;
        };
        if let Err(error) = clean() {
            eprintln!("{error:#}\nRestoring Cargo.lock and {GPUI_MANIFEST} from before the build.");
            let newport = root();
            let _ = fs::write(newport.join("Cargo.lock"), lock);
            let _ = fs::write(newport.join(GPUI_MANIFEST), manifest);
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
    check_install_marker(&newport, &remote)?;
    check_revision(&newport, &remote)?;
    let (url, rev) = gpui_source(&fs::read_to_string(newport.join("Cargo.lock"))?)?;
    let guard = Added::new()?;
    for name in CRATES {
        check_interrupted()?;
        let mut add = command("cargo");
        add.current_dir(&newport)
            .args(["add", "--quiet", "-p", "newport-gpui", "--path"]);
        add.arg(remote.join("crates").join(name));
        if name == "newport-remote-app" {
            add.args(["--no-default-features", "--features", "install"]);
        }
        util::run(&mut add)?;
    }
    check_interrupted()?;
    let archives = if prebuilt() {
        prebuilt_archives(&newport)?
    } else {
        agent_archives(&newport, &remote, &ARCHITECTURES)?
    };
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
    let mut env = vec![("NEWPORT_REMOTE_APPS".to_owned(), PathBuf::from("1"))];
    env.extend(archives);
    Ok(Prepared {
        _guard: guard,
        patch,
        env,
    })
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

/// The archives CI packed, used as they are.
fn prebuilt_archives(newport: &Path) -> Result<Vec<(String, PathBuf)>> {
    ARCHITECTURES
        .iter()
        .map(|(arch, _)| {
            let archive = archive_path(newport, arch);
            ensure!(
                fs::metadata(&archive).is_ok_and(|m| m.is_file() && m.len() > 0),
                "NEWPORT_PREBUILT is set, but there is no {}; make it with `cargo xtask remote-agents`",
                archive.display()
            );
            Ok((archive_variable(arch), archive))
        })
        .collect()
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
