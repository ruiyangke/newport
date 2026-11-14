//! The Full agent (core and Remote apps): newport-remote's
//! `products/unified-agent`, built as static Linux binaries in Docker, and
//! their export for desktop packaging.
//!
//! The build reads a snapshot of this checkout with newport-remote at
//! `remote/`, laid out as in the checkout so the product's path dependencies
//! resolve. A production snapshot is `git archive` of Newport's HEAD and of
//! the commit it pins at `remote/`, and needs both checkouts clean with
//! `remote/` at that commit. A development snapshot copies both working trees
//! and is marked dirty. `source-provenance.json` records both commits.
//!
//! The builder image and the script it runs are newport-remote's
//! `tools/static-agent`. An export carries only the binaries, their product
//! identities, the allowlisted notices and the provenance, and its
//! `newport-build.json` binds them to this commit.
use super::publication;
use crate::{command, remote, root, util};
use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use newport_protocol::product::{Identity, Product, Variant};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::Stdio,
};

/// newport-remote's `resources/licenses`, which every export carries. A new
/// dependency's notice is added here on purpose, never by copying the folder.
const NOTICES: [&str; 8] = [
    "README.md",
    "libaom-LICENSE",
    "libaom-PATENTS",
    "smithay-LICENSE.txt",
    "yuv-LICENSE-APACHE.md",
    "yuv-LICENSE.md",
    "opus-COPYING",
    "pulseaudio-LICENSE.md",
];
/// Newport's license and newport-remote's, as exports name them.
const LICENSES: [(&str, &str); 2] = [
    ("newport-core.txt", "LICENSE"),
    ("newport-full.txt", "remote/LICENSE"),
];
const SCHEMA: u32 = 2;
const SOURCE_PROVENANCE: &str = "source-provenance.json";
const BUILD_PROVENANCE: &str = "build-provenance.json";
pub(super) const EXPORT_PROVENANCE: &str = "full-agent-provenance.json";
const PRODUCT: &str = "remote/products/unified-agent";
const BUILDER: &str = "remote/tools/static-agent";
/// Server architectures as Newport names them, as Docker does, and their ELF
/// machine.
const ARCHITECTURES: [(&str, &str, u16); 2] = [("x86_64", "amd64", 62), ("aarch64", "arm64", 183)];

/// The notices every Full export carries, by file name.
pub(crate) fn notice_names() -> impl Iterator<Item = &'static str> {
    NOTICES.into_iter().chain(LICENSES.map(|(name, _)| name))
}

/// The notices an export's manifest lists, by file name: its files directly
/// in `licenses/`, the only files that folder may hold.
pub(crate) fn exported_notices(export: &Path) -> Result<Vec<String>> {
    let mut notices = Vec::new();
    for path in crate::artifact::listed(export)? {
        if let Some(name) = path.strip_prefix("licenses/") {
            ensure!(
                !name.contains('/'),
                "Agent export notices must be files directly in licenses/: {path}"
            );
            notices.push(name.to_owned());
        }
    }
    ensure!(
        !notices.is_empty(),
        "The agent export lists no notices: {}",
        export.display()
    );
    Ok(notices)
}

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Build the Full agent in Docker from this checkout and remote/, one
    /// directory per architecture.
    Build {
        /// Only these architectures (x86_64, aarch64); default both.
        #[arg(long = "arch")]
        architectures: Vec<String>,
        /// Build the committed sources: both checkouts clean, remote/ at the
        /// commit this one pins.
        #[arg(long)]
        production: bool,
        /// Where the architecture directories go; default
        /// remote/target/unified-agent.
        #[arg(long)]
        output_dir: Option<PathBuf>,
    },
    /// Export both architectures' builds for desktop packaging.
    Export {
        /// Require production builds of this commit and the newport-remote
        /// commit it pins.
        #[arg(long)]
        production: bool,
        /// The build's output directory; default remote/target/unified-agent.
        #[arg(long)]
        build_dir: Option<PathBuf>,
        /// A new directory for the export.
        #[arg(long)]
        output: PathBuf,
    },
}

pub fn run(action: Action) -> Result<()> {
    match action {
        Action::Build {
            architectures,
            production,
            output_dir,
        } => {
            let output = match output_dir {
                Some(directory) => std::path::absolute(directory)?,
                None => default_builds(),
            };
            build(
                &root(),
                &remote::architectures(&architectures)?,
                production,
                &output,
            )
        }
        Action::Export {
            production,
            build_dir,
            output,
        } => {
            let builds = match build_dir {
                Some(directory) => std::path::absolute(directory)?,
                None => default_builds(),
            };
            let revisions = if production {
                Some((util::git_head()?, super::pinned_revision(&root())?))
            } else {
                None
            };
            export(
                &builds,
                &std::path::absolute(output)?,
                revisions
                    .as_ref()
                    .map(|(newport, remote)| (newport.as_str(), remote.as_str())),
            )
            .map(drop)
        }
    }
}

pub(super) fn default_builds() -> PathBuf {
    root().join("remote/target/unified-agent")
}

/// A commit and whether the built source differs from it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Source {
    pub commit: String,
    pub dirty: bool,
}

/// `source-provenance.json`, which every build and export carries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Provenance {
    pub schema: u32,
    pub variant: String,
    pub development: bool,
    pub newport: Source,
    pub remote: Source,
}

fn sha(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl Provenance {
    fn dirty(&self) -> bool {
        self.newport.dirty || self.remote.dirty
    }

    /// Well formed, and dirty sources only in a development build.
    pub(super) fn check(&self) -> Result<()> {
        ensure!(
            self.schema == SCHEMA
                && self.variant == "full"
                && sha(&self.newport.commit, 40)
                && sha(&self.remote.commit, 40),
            "Invalid Full agent source provenance"
        );
        ensure!(
            self.development || !self.dirty(),
            "Dirty Full agent sources must be marked as development"
        );
        Ok(())
    }

    /// A production build of exactly these commits.
    pub(super) fn check_production(&self, newport: &str, remote: &str) -> Result<()> {
        ensure!(
            !self.development
                && !self.dirty()
                && self.newport.commit == newport
                && self.remote.commit == remote,
            "A production Full agent must be built from clean sources of this commit and the newport-remote commit it pins"
        );
        Ok(())
    }
}

fn git(repository: &Path, args: &[&str]) -> Result<String> {
    util::capture(command("git").arg("-C").arg(repository).args(args))
}

fn source(repository: &Path) -> Result<Source> {
    Ok(Source {
        commit: git(repository, &["rev-parse", "HEAD"])?,
        dirty: !git(
            repository,
            &["status", "--porcelain", "--untracked-files=all"],
        )?
        .is_empty(),
    })
}

fn check_production_sources(newport: &Source, remote: &Source, pinned: &str) -> Result<()> {
    ensure!(
        !newport.dirty && !remote.dirty && remote.commit == pinned,
        "A production build needs clean Newport and newport-remote checkouts, with remote/ at {pinned}, the newport-remote commit this release builds"
    );
    Ok(())
}

/// The source a build reads, deleted when dropped.
struct Snapshot {
    directory: tempfile::TempDir,
    provenance: Provenance,
}
impl Snapshot {
    fn tree(&self) -> &Path {
        self.directory.path()
    }
}

fn stage(newport: &Path, development: bool) -> Result<Snapshot> {
    let remote = newport.join("remote");
    let mut sources = (source(newport)?, source(&remote)?);
    let directory = tempfile::Builder::new()
        .prefix("newport-full-agent-")
        .tempdir()?;
    let tree = directory.path();
    if development {
        copy_working_tree(newport, tree)?;
        copy_working_tree(&remote, &tree.join("remote"))?;
        sources.0.dirty = true;
        sources.1.dirty = true;
    } else {
        let pinned = super::pinned_revision(newport)?;
        check_production_sources(&sources.0, &sources.1, &pinned)?;
        archive(newport, &sources.0.commit, tree)?;
        archive(&remote, &pinned, &tree.join("remote"))?;
        ensure!(
            (source(newport)?, source(&remote)?) == sources,
            "The checkouts changed while staging; build again from clean checkouts"
        );
    }
    // Neither copy holds the patched dependencies, which Git ignores.
    crate::patched::ensure_in(&tree.join("remote"))?;
    check_lock(tree)?;
    let provenance = Provenance {
        schema: SCHEMA,
        variant: "full".into(),
        development,
        newport: sources.0,
        remote: sources.1,
    };
    fs::write(
        tree.join(SOURCE_PROVENANCE),
        serde_json::to_string_pretty(&provenance)? + "\n",
    )?;
    Ok(Snapshot {
        directory,
        provenance,
    })
}

/// Unpack `git archive` of `commit`. Files get the current time, so the
/// shared Cargo target volume never takes changed sources for built ones.
fn archive(repository: &Path, commit: &str, destination: &Path) -> Result<()> {
    let output = command("git")
        .arg("-C")
        .arg(repository)
        .args(["archive", "--format=tar", commit])
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .context("Cannot start git")?;
    ensure!(
        output.status.success(),
        "git archive failed in {}",
        repository.display()
    );
    unpack(&output.stdout[..], destination)
}

fn unpack(archive: impl Read, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    let mut archive = tar::Archive::new(archive);
    archive.set_preserve_mtime(false);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let kind = entry.header().entry_type();
        if kind == tar::EntryType::XGlobalHeader {
            continue;
        }
        let name = entry.path()?.display().to_string();
        ensure!(
            kind.is_file() || kind.is_dir(),
            "Source archives may hold only regular files and directories: {name}"
        );
        ensure!(
            entry.unpack_in(destination)?,
            "Source archive member leaves the snapshot: {name}"
        );
    }
    Ok(())
}

/// Tracked and untracked files that aren't ignored, as they are on disk.
fn copy_working_tree(repository: &Path, destination: &Path) -> Result<()> {
    let files = git(
        repository,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )?;
    for name in files.split('\0').filter(|name| !name.is_empty()) {
        let from = repository.join(name);
        let metadata = match fs::symlink_metadata(&from) {
            Ok(metadata) => metadata,
            // Deleted in the working tree.
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        // A nested repository's directory.
        if metadata.is_dir() {
            continue;
        }
        ensure!(
            metadata.is_file(),
            "Source must be regular files, not links: {}",
            from.display()
        );
        let to = destination.join(name);
        fs::create_dir_all(to.parent().context("Source file has no directory")?)?;
        fs::write(&to, fs::read(&from)?)?;
        fs::set_permissions(&to, metadata.permissions())?;
    }
    Ok(())
}

type Package = (String, String, Option<String>);

fn packages(lock: &Path) -> Result<BTreeSet<Package>> {
    #[derive(Deserialize)]
    struct Lock {
        #[serde(default)]
        package: Vec<Entry>,
    }
    #[derive(Deserialize)]
    struct Entry {
        name: String,
        version: String,
        source: Option<String>,
    }
    let lock: Lock = toml::from_str(
        &fs::read_to_string(lock).with_context(|| format!("Cannot read {}", lock.display()))?,
    )?;
    Ok(lock
        .package
        .into_iter()
        .map(|entry| (entry.name, entry.version, entry.source))
        .collect())
}

/// The product's own lock resolves only what newport-remote's workspace and
/// the core agent already lock.
fn check_lock(tree: &Path) -> Result<()> {
    let mut allowed = packages(&tree.join("remote/Cargo.lock"))?;
    allowed.extend(packages(&tree.join("crates/newport-agent/Cargo.lock"))?);
    allowed.insert(("newport-unified-agent".into(), "0.1.0".into(), None));
    let product = packages(&tree.join(PRODUCT).join("Cargo.lock"))?;
    let drifted: Vec<String> = product
        .difference(&allowed)
        .map(|(name, version, _)| format!("{name} {version}"))
        .collect();
    ensure!(
        drifted.is_empty(),
        "The Full agent's lock has packages neither newport-remote's nor the core agent's lock has: {}",
        drifted.join(", ")
    );
    Ok(())
}

/// Build each architecture into `output/<arch>`. A failed build keeps the
/// last one.
pub(super) fn build(
    newport: &Path,
    architectures: &[&str],
    production: bool,
    output: &Path,
) -> Result<()> {
    let snapshot = stage(newport, !production)?;
    let rust = remote::rust_version()?;
    fs::create_dir_all(output)?;
    for arch in architectures {
        let (_, platform, _) = ARCHITECTURES
            .into_iter()
            .find(|(name, _, _)| name == arch)
            .with_context(|| format!("Unknown architecture {arch}"))?;
        let image = format!("newport-unified-builder:{platform}");
        util::run(
            command(remote::docker())
                .args(["build", "--platform", &format!("linux/{platform}")])
                .args(["--build-arg", &format!("RUST_VERSION={rust}"), "-t", &image])
                .arg(snapshot.tree().join(BUILDER)),
        )?;
        let temporary = tempfile::Builder::new()
            .prefix(".build-")
            .tempdir_in(output)?;
        let artifacts = temporary.path().join("artifacts");
        fs::create_dir(&artifacts)?;
        let target = Target::new(platform, snapshot.tree(), !production);
        util::run(
            command(remote::docker())
                .args(["run", "--rm", "--platform", &format!("linux/{platform}")])
                .args([
                    "-e",
                    &format!("NEWPORT_AGENT_VERSION={}", remote::agent_version()?),
                ])
                .args([
                    "-v",
                    &format!("newport-static-agent-registry-{platform}:/opt/cargo/registry"),
                    "-v",
                    &format!("{}:/build", target.volume),
                    "-v",
                ])
                .arg(format!("{}:/src:ro", snapshot.tree().display()))
                .arg("-v")
                .arg(format!("{}:/out", artifacts.display()))
                .args([
                    &image,
                    "sh",
                    "-eu",
                    "/src/remote/tools/static-agent/build.sh",
                ]),
        )
        .with_context(|| format!("The Full agent build failed for {arch}"))?;
        drop(target);
        checked_build(&artifacts)?;
        fs::write(
            artifacts.join(BUILD_PROVENANCE),
            fs::read(snapshot.tree().join(SOURCE_PROVENANCE))?,
        )?;
        let notices = artifacts.join("licenses");
        fs::create_dir(&notices)?;
        for name in NOTICES {
            fs::copy(
                snapshot.tree().join("remote/resources/licenses").join(name),
                notices.join(name),
            )?;
        }
        for (name, path) in LICENSES {
            fs::copy(snapshot.tree().join(path), notices.join(name))?;
        }
        let published =
            publication::publish(&artifacts, &output.join(arch), |path| Ok(path.to_owned()))?;
        println!(
            "Full agent for {arch} ({}): {}",
            if snapshot.provenance.development {
                "development"
            } else {
                "production"
            },
            published.join("newport-agent").display()
        );
    }
    Ok(())
}

/// The Docker volume a build's Cargo output goes to. Development builds
/// share one per architecture, so rebuilding is quick. Cargo decides what to
/// rebuild by modification time, so a shared volume can hand one checkout's
/// build to another; a production build gets an empty volume of its own,
/// removed when it is dropped.
struct Target {
    volume: String,
    private: bool,
}
impl Target {
    /// `snapshot` is the build's source directory, unique to the run.
    fn new(platform: &str, snapshot: &Path, development: bool) -> Self {
        if development {
            return Self {
                volume: format!("newport-unified-agent-target-{platform}"),
                private: false,
            };
        }
        let run = snapshot
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .trim_start_matches("newport-full-agent-")
            .to_ascii_lowercase();
        Self {
            volume: format!("newport-unified-agent-production-{platform}-{run}"),
            private: true,
        }
    }
}
impl Drop for Target {
    fn drop(&mut self) {
        if self.private {
            let _ = command(remote::docker())
                .args(["volume", "rm", "--force", &self.volume])
                .stdout(Stdio::null())
                .status();
        }
    }
}

fn checked_build(artifacts: &Path) -> Result<()> {
    let identity: Identity = util::read_json(&artifacts.join("product.json"))?;
    identity.validate().map_err(anyhow::Error::msg)?;
    ensure!(
        identity.schema_version == 1
            && identity.build_sha256 == util::sha256_file(&artifacts.join("newport-agent"))?
            && identity.product.variant == Variant::Full,
        "The built agent does not identify the Full product"
    );
    Ok(())
}

/// A regular file below `directory`, through no links.
fn regular(directory: &Path, relative: &str) -> Result<Vec<u8>> {
    let mut path = directory.to_owned();
    for component in Path::new(relative).components() {
        path.push(component);
        ensure!(
            !fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_symlink()),
            "Build symlinks cannot be exported: {relative}"
        );
    }
    ensure!(
        fs::symlink_metadata(directory).is_ok_and(|metadata| metadata.is_dir()) && path.is_file(),
        "Build input must be a regular file: {}",
        path.display()
    );
    Ok(fs::read(&path)?)
}

/// The export allows exactly control version 1 and Remote apps version 1,
/// narrower than `Product::validate`.
fn checked_product(product: &Product) -> Result<()> {
    ensure!(
        product.variant == Variant::Full,
        "Export requires the Full agent product"
    );
    product
        .validate_form()
        .map_err(anyhow::Error::msg)
        .context("Invalid Full agent service protocols")?;
    ensure!(
        product.control_versions == [1]
            && product
                .services
                .get(newport_protocol::runtime::service::REMOTE_APPS)
                .map(Vec::as_slice)
                == Some(&[1][..]),
        "Export requires control version 1 and remote-apps version 1"
    );
    Ok(())
}

struct Build {
    binary: Vec<u8>,
    identity: Vec<u8>,
    product: Product,
    version: Option<String>,
    provenance: Provenance,
    notices: Vec<(String, Vec<u8>)>,
}

fn read_build(directory: &Path, machine: u16, production: Option<(&str, &str)>) -> Result<Build> {
    let binary = regular(directory, "newport-agent")?;
    let identity = regular(directory, "product.json")?;
    let provenance: Provenance = serde_json::from_slice(&regular(directory, BUILD_PROVENANCE)?)
        .context("Invalid Full agent source provenance")?;
    provenance.check()?;
    if let Some((newport, remote)) = production {
        provenance.check_production(newport, remote)?;
    }
    let mut notices = Vec::new();
    for name in notice_names() {
        notices.push((
            name.to_owned(),
            regular(directory, &format!("licenses/{name}"))?,
        ));
    }
    ensure!(
        binary.len() >= 20
            && binary[..6] == *b"\x7fELF\x02\x01"
            && u16::from_le_bytes([binary[18], binary[19]]) == machine,
        "Wrong ELF architecture in {}",
        directory.display()
    );
    let parsed: Identity = serde_json::from_slice(&identity)
        .with_context(|| format!("Invalid product identity in {}", directory.display()))?;
    parsed.validate_build().map_err(anyhow::Error::msg)?;
    ensure!(
        parsed.schema_version == 1 && parsed.build_sha256 == util::sha256_hex(&binary),
        "Product identity does not match {}",
        directory.display()
    );
    checked_product(&parsed.product)?;
    Ok(Build {
        binary,
        identity,
        product: parsed.product,
        version: parsed.version,
        provenance,
        notices,
    })
}

/// Export both architectures from `builds` into the new directory `output`.
/// `production` names the Newport and newport-remote commits the builds must
/// come from.
pub(super) fn export(
    builds: &Path,
    output: &Path,
    production: Option<(&str, &str)>,
) -> Result<PathBuf> {
    ensure!(
        fs::symlink_metadata(output).is_err(),
        "Output must be a new directory: {}",
        output.display()
    );
    let mut checked: Vec<(&str, Build)> = Vec::new();
    for (arch, _, machine) in ARCHITECTURES {
        let build = read_build(&builds.join(arch), machine, production)?;
        if let Some((_, first)) = checked.first() {
            ensure!(
                first.version == build.version,
                "Architecture product versions differ"
            );
            ensure!(
                first.product == build.product,
                "Architecture product protocols differ"
            );
            ensure!(
                first.provenance == build.provenance,
                "Architectures must be built from the same source"
            );
            ensure!(
                first.notices == build.notices,
                "Architecture license notices differ"
            );
        }
        checked.push((arch, build));
    }
    let parent = output.parent().context("Output has no parent")?;
    fs::create_dir_all(parent)?;
    let temporary = tempfile::Builder::new()
        .prefix(".agent-export-")
        .tempdir_in(parent)?;
    let stage = temporary.path().join("agents");
    fs::create_dir_all(stage.join("licenses"))?;
    for (arch, build) in &checked {
        let binary = stage.join(format!("newport-agent-{arch}"));
        fs::write(&binary, &build.binary)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))?;
        }
        fs::write(
            stage.join(format!("newport-agent-{arch}.product.json")),
            &build.identity,
        )?;
    }
    let (_, first) = &checked[0];
    for (name, bytes) in &first.notices {
        fs::write(stage.join("licenses").join(name), bytes)?;
    }
    fs::write(
        stage.join(EXPORT_PROVENANCE),
        serde_json::to_string_pretty(&first.provenance)? + "\n",
    )?;
    crate::artifact::record(&stage)?;
    fs::rename(&stage, output)?;
    println!("Full agent export: {}", output.display());
    Ok(output.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    const NEWPORT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const REMOTE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn identity(commit: &str, dirty: bool) -> Source {
        Source {
            commit: commit.into(),
            dirty,
        }
    }

    #[test]
    fn only_development_builds_share_a_cargo_volume() {
        let snapshot = || {
            tempfile::Builder::new()
                .prefix("newport-full-agent-")
                .tempdir()
                .unwrap()
        };
        let (first, second) = (snapshot(), snapshot());
        let shared = Target::new("arm64", first.path(), true);
        assert_eq!(
            shared.volume,
            Target::new("arm64", second.path(), true).volume
        );
        let one = Target::new("arm64", first.path(), false);
        let other = Target::new("arm64", second.path(), false);
        assert_ne!(one.volume, other.volume);
        assert_ne!(one.volume, shared.volume);
        assert!(one.private && !shared.private);
        // No Docker volume was made for these, so none is removed.
        for target in [shared, one, other] {
            std::mem::forget(target);
        }
    }

    #[test]
    fn production_needs_clean_sources_at_the_pinned_commit() {
        check_production_sources(&identity(NEWPORT, false), &identity(REMOTE, false), REMOTE)
            .unwrap();
        for (newport, remote) in [
            (identity(NEWPORT, true), identity(REMOTE, false)),
            (identity(NEWPORT, false), identity(REMOTE, true)),
            (identity(NEWPORT, false), identity(&"d".repeat(40), false)),
        ] {
            let error = check_production_sources(&newport, &remote, REMOTE).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("clean Newport and newport-remote")
            );
        }
    }

    fn provenance(development: bool, dirty: bool) -> Value {
        json!({
            "schema": 2, "variant": "full", "development": development,
            "newport": {"commit": NEWPORT, "dirty": dirty},
            "remote": {"commit": REMOTE, "dirty": dirty},
        })
    }

    fn parse(value: &Value) -> Result<Provenance> {
        let provenance: Provenance = serde_json::from_value(value.clone())?;
        provenance.check()?;
        Ok(provenance)
    }

    #[test]
    fn provenance_is_exactly_schema_two() {
        parse(&provenance(false, false)).unwrap();
        parse(&provenance(true, true)).unwrap();
        assert!(parse(&provenance(false, true)).is_err());
        for schema in [
            json!(true),
            json!(1.0),
            json!("2"),
            json!(null),
            json!(1),
            json!(3),
        ] {
            let mut value = provenance(false, false);
            value["schema"] = schema;
            assert!(parse(&value).is_err());
        }
        for (field, value) in [
            ("variant", json!("core")),
            ("privateBuildPath", json!("/private")),
            ("unexpected", json!(true)),
        ] {
            let mut changed = provenance(false, false);
            changed[field] = value;
            assert!(parse(&changed).is_err(), "{field}");
        }
        for side in ["newport", "remote"] {
            for commit in [json!("A".repeat(40)), json!("a".repeat(39)), json!("main")] {
                let mut value = provenance(false, false);
                value[side]["commit"] = commit;
                assert!(parse(&value).is_err());
            }
            let mut value = provenance(false, false);
            value[side].as_object_mut().unwrap().remove("dirty");
            assert!(parse(&value).is_err());
        }
        let clean = parse(&provenance(false, false)).unwrap();
        clean.check_production(NEWPORT, REMOTE).unwrap();
        assert!(clean.check_production(NEWPORT, &"d".repeat(40)).is_err());
        assert!(clean.check_production(&"d".repeat(40), REMOTE).is_err());
        let development = parse(&provenance(true, false)).unwrap();
        assert!(development.check_production(NEWPORT, REMOTE).is_err());
    }

    fn git_in(repository: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(repository)
            .args([
                "-c",
                "user.name=Newport",
                "-c",
                "user.email=newport@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "tag.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    const TAG: &str = "v1.2.3";

    /// A Newport checkout tagged for release, with newport-remote at
    /// `remote/`, at its tag of the same name.
    fn checkouts() -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let newport = directory.path().join("newport");
        let remote = newport.join("remote");
        let lock = "version = 4\n\n[[package]]\nname = \"serde\"\nversion = \"1.0.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n";
        write(&remote.join("Cargo.lock"), lock);
        write(&newport.join(PRODUCT).join("Cargo.lock"), lock);
        write(&remote.join("LICENSE"), "newport-remote license");
        write(&remote.join(".gitignore"), "/target/\n");
        for name in NOTICES {
            write(&remote.join("resources/licenses").join(name), name);
        }
        git_in(&remote, &["init", "-q"]);
        git_in(&remote, &["add", "."]);
        git_in(&remote, &["commit", "-q", "--no-verify", "-m", "remote"]);
        git_in(&remote, &["tag", TAG]);
        write(
            &newport.join("crates/newport-agent/Cargo.lock"),
            "version = 4\n",
        );
        write(&newport.join("LICENSE"), "Newport license");
        write(&newport.join(".gitignore"), "/target/\n/remote/\n");
        git_in(&newport, &["init", "-q"]);
        git_in(&newport, &["add", "LICENSE", ".gitignore", "crates"]);
        git_in(&newport, &["commit", "-q", "--no-verify", "-m", "newport"]);
        git_in(&newport, &["tag", TAG]);
        (directory, newport)
    }

    #[test]
    fn production_snapshot_is_both_commits_without_working_tree_changes() {
        let (_directory, newport) = checkouts();
        let snapshot = stage(&newport, false).unwrap();
        let tree = snapshot.tree();
        assert!(!snapshot.provenance.development && !snapshot.provenance.dirty());
        assert_eq!(
            snapshot.provenance.remote.commit,
            git_in(&newport.join("remote"), &["rev-parse", TAG])
        );
        assert_eq!(
            fs::read_to_string(tree.join("remote/LICENSE")).unwrap(),
            "newport-remote license"
        );
        assert_eq!(
            fs::read_to_string(tree.join("LICENSE")).unwrap(),
            "Newport license"
        );
        let written: Provenance = util::read_json(&tree.join(SOURCE_PROVENANCE)).unwrap();
        assert_eq!(written, snapshot.provenance);
        // Later edits don't reach a snapshot, and dirty checkouts can't build
        // for production.
        write(&newport.join("remote/LICENSE"), "edited");
        assert_eq!(
            fs::read_to_string(tree.join("remote/LICENSE")).unwrap(),
            "newport-remote license"
        );
        assert!(stage(&newport, false).is_err());
        write(&newport.join("remote/LICENSE"), "newport-remote license");
        write(&newport.join("untracked"), "untracked");
        assert!(stage(&newport, false).is_err());
    }

    #[test]
    fn production_refuses_remote_at_another_commit() {
        let (_directory, newport) = checkouts();
        let remote = newport.join("remote");
        write(&remote.join("next"), "next");
        git_in(&remote, &["add", "next"]);
        git_in(&remote, &["commit", "-q", "--no-verify", "-m", "next"]);
        let error = stage(&newport, false).err().unwrap();
        assert!(
            error
                .to_string()
                .contains("the newport-remote commit this release builds")
        );
    }

    #[test]
    fn production_needs_both_release_tags() {
        let (_directory, newport) = checkouts();
        git_in(&newport.join("remote"), &["tag", "-d", TAG]);
        let error = stage(&newport, false).err().unwrap();
        assert!(
            error
                .to_string()
                .contains("newport-remote has no tag v1.2.3")
        );
        git_in(&newport.join("remote"), &["tag", TAG]);
        git_in(&newport, &["tag", "-d", TAG]);
        let error = stage(&newport, false).err().unwrap();
        assert!(error.to_string().contains("Newport release tag"));
    }

    #[test]
    fn development_snapshot_copies_working_trees_without_ignored_files() {
        let (_directory, newport) = checkouts();
        write(&newport.join("remote/LICENSE"), "edited");
        write(&newport.join("remote/new-notice"), "untracked");
        write(&newport.join("remote/target/secret"), "ignored");
        write(&newport.join("target/secret"), "ignored");
        fs::remove_file(newport.join("LICENSE")).unwrap();
        let snapshot = stage(&newport, true).unwrap();
        let tree = snapshot.tree();
        assert!(snapshot.provenance.development && snapshot.provenance.dirty());
        assert_eq!(
            fs::read_to_string(tree.join("remote/LICENSE")).unwrap(),
            "edited"
        );
        assert!(tree.join("remote/new-notice").is_file());
        assert!(!tree.join("remote/target").exists() && !tree.join("target").exists());
        assert!(!tree.join("LICENSE").exists());
    }

    #[cfg(unix)]
    #[test]
    fn source_symlinks_never_pull_outside_files_into_a_snapshot() {
        let (directory, newport) = checkouts();
        write(&directory.path().join("outside"), "outside the checkout");
        std::os::unix::fs::symlink(directory.path().join("outside"), newport.join("escape"))
            .unwrap();
        let error = stage(&newport, true).err().unwrap();
        assert!(error.to_string().contains("not links"));
    }

    #[test]
    fn product_lock_may_not_drift_from_the_source_locks() {
        let (_directory, newport) = checkouts();
        check_lock(&newport).unwrap();
        let product = newport.join(PRODUCT).join("Cargo.lock");
        let mut lock = fs::read_to_string(&product).unwrap();
        lock.push_str("\n[[package]]\nname = \"drifted\"\nversion = \"0.1.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n");
        fs::write(&product, lock).unwrap();
        let error = check_lock(&newport).unwrap_err();
        assert!(error.to_string().contains("drifted 0.1.0"));
    }

    fn product() -> Value {
        json!({"variant": "full", "controlVersions": [1], "services": {
            "git": [1], "monitor": [1], "integration": [1], "gui-notices": [1],
            "lsp": [1], "remote-apps": [1]}})
    }

    /// Both architectures' builds, as `build` leaves them.
    fn fixture(root: &Path, development: bool) -> PathBuf {
        let builds = root.join("builds");
        for (arch, _, machine) in ARCHITECTURES {
            let directory = builds.join(arch);
            fs::create_dir_all(directory.join("licenses")).unwrap();
            let mut binary = b"\x7fELF\x02\x01".to_vec();
            binary.extend([0; 12]);
            binary.extend(machine.to_le_bytes());
            fs::write(directory.join("newport-agent"), &binary).unwrap();
            fs::write(
                directory.join("product.json"),
                serde_json::to_vec(&json!({"schemaVersion": 1,
                    "buildSha256": util::sha256_hex(&binary), "product": product()}))
                .unwrap(),
            )
            .unwrap();
            for name in NOTICES.into_iter().chain(LICENSES.map(|(name, _)| name)) {
                fs::write(
                    directory.join("licenses").join(name),
                    format!("source snapshot notice: {name}"),
                )
                .unwrap();
            }
            fs::write(
                directory.join(BUILD_PROVENANCE),
                serde_json::to_vec(&provenance(development, development)).unwrap(),
            )
            .unwrap();
            fs::write(directory.join("private-source.tar"), "must stay private").unwrap();
            fs::write(
                directory.join("licenses/private-source.rs"),
                "must stay private",
            )
            .unwrap();
        }
        builds
    }

    fn edit_json(path: &Path, change: impl FnOnce(&mut Value)) {
        let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        change(&mut value);
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }

    /// Both architectures equally changed, so no later mismatch check can
    /// hide a missing validator.
    fn rejects_identity(change: impl Fn(&mut Value)) {
        let directory = tempfile::tempdir().unwrap();
        let builds = fixture(directory.path(), false);
        for (arch, _, _) in ARCHITECTURES {
            edit_json(&builds.join(arch).join("product.json"), &change);
        }
        let output = directory.path().join("unpublished/output");
        assert!(export(&builds, &output, None).is_err());
        assert!(!directory.path().join("unpublished").exists());
    }

    #[test]
    fn rejects_inexact_identity_and_product_fields_and_schema_types() {
        for schema in [
            json!(true),
            json!(false),
            json!(1.0),
            json!("1"),
            json!(null),
            json!(0),
            json!(2),
        ] {
            rejects_identity(|identity| identity["schemaVersion"] = schema.clone());
        }
        for field in ["schemaVersion", "buildSha256", "product"] {
            rejects_identity(|identity| {
                identity.as_object_mut().unwrap().remove(field);
            });
        }
        rejects_identity(|identity| identity["unexpected"] = "private field".into());
        for field in ["variant", "controlVersions", "services"] {
            rejects_identity(|identity| {
                identity["product"].as_object_mut().unwrap().remove(field);
            });
        }
        rejects_identity(|identity| identity["product"]["unexpected"] = true.into());
        for product in [json!(null), json!([]), json!("full")] {
            rejects_identity(|identity| identity["product"] = product.clone());
        }
        rejects_identity(|identity| identity["product"]["variant"] = "core".into());
    }

    #[test]
    fn rejects_incomplete_service_sets_and_invalid_names() {
        for missing in [
            "git",
            "monitor",
            "integration",
            "gui-notices",
            "lsp",
            "remote-apps",
        ] {
            rejects_identity(|identity| {
                identity["product"]["services"]
                    .as_object_mut()
                    .unwrap()
                    .remove(missing);
            });
        }
        for services in [
            json!(null),
            json!([]),
            json!("remote-apps"),
            json!({"remote-apps": [1]}),
        ] {
            rejects_identity(|identity| identity["product"]["services"] = services.clone());
        }
        let long = "a".repeat(65);
        for name in [
            "",
            "A",
            "é",
            "service_name",
            "service.name",
            "service/name",
            "service\n",
            long.as_str(),
            "product-",
            "product-full",
        ] {
            rejects_identity(|identity| identity["product"]["services"][name] = json!([1]));
        }
        // Six required services and ten more make sixteen.
        rejects_identity(|identity| {
            for index in 0..10 {
                identity["product"]["services"][format!("extra-{index}")] = json!([1]);
            }
        });
    }

    #[test]
    fn rejects_invalid_version_vectors() {
        let invalid = [
            json!(null),
            json!({}),
            json!("1"),
            json!(1),
            json!([]),
            json!([0]),
            json!([-1]),
            json!([0x1_0000_0000u64]),
            json!([true]),
            json!([false]),
            json!([1.0]),
            json!(["1"]),
            json!([null]),
            json!([{}]),
            json!([1, 1]),
            json!((1..10).collect::<Vec<_>>()),
        ];
        for value in invalid {
            rejects_identity(|identity| identity["product"]["controlVersions"] = value.clone());
            for service in ["remote-apps", "monitor"] {
                rejects_identity(|identity| {
                    identity["product"]["services"][service] = value.clone();
                });
            }
        }
    }

    #[test]
    fn keeps_exact_control_and_remote_apps_version_export_policy() {
        for value in [json!([2]), json!([1, 2]), json!([0xffff_ffffu32])] {
            rejects_identity(|identity| identity["product"]["controlVersions"] = value.clone());
            rejects_identity(|identity| {
                identity["product"]["services"]["remote-apps"] = value.clone();
            });
        }
    }

    #[test]
    fn accepts_valid_service_boundaries_and_keeps_exact_identity_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let builds = fixture(directory.path(), false);
        let mut expected = Vec::new();
        for (arch, _, _) in ARCHITECTURES {
            let path = builds.join(arch).join("product.json");
            edit_json(&path, |identity| {
                let services = &mut identity["product"]["services"];
                services["monitor"] = json!([1, 2, 3, 4, 5, 6, 7, 0xffff_ffffu32]);
                for index in 0..8 {
                    services[format!("extra-{index}")] = json!([2]);
                }
                // Exactly fifteen valid services.
                services["a".repeat(64)] = json!([0xffff_ffffu32]);
            });
            expected.push((arch, fs::read(&path).unwrap()));
        }
        let output = directory.path().join("output");
        export(&builds, &output, None).unwrap();
        for (arch, bytes) in expected {
            assert_eq!(
                fs::read(output.join(format!("newport-agent-{arch}.product.json"))).unwrap(),
                bytes
            );
        }
    }

    #[test]
    fn rejects_mismatched_individually_valid_architecture_products() {
        let directory = tempfile::tempdir().unwrap();
        let builds = fixture(directory.path(), false);
        edit_json(&builds.join("aarch64/product.json"), |identity| {
            identity["product"]["services"]["monitor"] = json!([1, 2]);
        });
        let output = directory.path().join("unpublished/output");
        let error = export(&builds, &output, None).unwrap_err();
        assert!(error.to_string().contains("product protocols differ"));
        assert!(!directory.path().join("unpublished").exists());
    }

    #[test]
    fn rejects_inconsistent_or_invalid_product_versions_before_publication() {
        for version in ["0.6.5", "0.6", "812297d"] {
            let directory = tempfile::tempdir().unwrap();
            let builds = fixture(directory.path(), false);
            for architecture in ["x86_64", "aarch64"] {
                edit_json(
                    &builds.join(architecture).join("product.json"),
                    |identity| {
                        identity["version"] = "0.6.4".into();
                    },
                );
            }
            edit_json(&builds.join("aarch64/product.json"), |identity| {
                identity["version"] = version.into();
            });
            assert!(export(&builds, &directory.path().join("unpublished/output"), None).is_err());
            assert!(!directory.path().join("unpublished").exists());
        }
    }

    #[test]
    fn rejects_noninteger_provenance_schema_before_publication() {
        for schema in [
            json!(true),
            json!(false),
            json!(1.0),
            json!("2"),
            json!(null),
            json!(0),
            json!(1),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let builds = fixture(directory.path(), false);
            for (arch, _, _) in ARCHITECTURES {
                edit_json(&builds.join(arch).join(BUILD_PROVENANCE), |provenance| {
                    provenance["schema"] = schema.clone();
                });
            }
            let output = directory.path().join("unpublished/output");
            let error = export(&builds, &output, None).unwrap_err();
            assert!(format!("{error:#}").contains("Invalid Full agent source provenance"));
            assert!(!directory.path().join("unpublished").exists());
        }
    }

    #[test]
    fn exports_only_checked_binaries_provenance_and_notices() {
        let directory = tempfile::tempdir().unwrap();
        let builds = fixture(directory.path(), false);
        let output = directory.path().join("output");
        export(&builds, &output, Some((NEWPORT, REMOTE))).unwrap();
        let mut expected: BTreeSet<String> = NOTICES
            .into_iter()
            .chain(LICENSES.map(|(name, _)| name))
            .map(|name| format!("licenses/{name}"))
            .collect();
        for arch in ["x86_64", "aarch64"] {
            expected.insert(format!("newport-agent-{arch}"));
            expected.insert(format!("newport-agent-{arch}.product.json"));
        }
        expected.insert(EXPORT_PROVENANCE.into());
        let actual: BTreeSet<String> = walkdir::WalkDir::new(&output)
            .into_iter()
            .map(Result::unwrap)
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| {
                entry
                    .path()
                    .strip_prefix(&output)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .replace('\\', "/")
            })
            .filter(|name| name != "newport-build.json")
            .collect();
        assert_eq!(actual, expected);
        crate::artifact::verify(&output).unwrap();
        let manifest: Value = util::read_json(&output.join("newport-build.json")).unwrap();
        assert_eq!(
            manifest["files"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>(),
            expected
        );
        let exported: Value = util::read_json(&output.join(EXPORT_PROVENANCE)).unwrap();
        assert_eq!(exported, provenance(false, false));
        assert_eq!(
            fs::read_to_string(output.join("licenses/libaom-LICENSE")).unwrap(),
            "source snapshot notice: libaom-LICENSE"
        );
    }

    #[test]
    fn development_exports_are_marked_and_never_production() {
        let directory = tempfile::tempdir().unwrap();
        let builds = fixture(directory.path(), true);
        export(&builds, &directory.path().join("development"), None).unwrap();
        let exported: Provenance =
            util::read_json(&directory.path().join("development").join(EXPORT_PROVENANCE)).unwrap();
        assert!(exported.development && exported.newport.dirty);
        let production = directory.path().join("production");
        let error = export(&builds, &production, Some((NEWPORT, REMOTE))).unwrap_err();
        assert!(error.to_string().contains("clean sources"));
        assert!(!production.exists());
    }

    #[test]
    fn production_requires_both_exact_commits() {
        let directory = tempfile::tempdir().unwrap();
        let builds = fixture(directory.path(), false);
        let other = "d".repeat(40);
        for revisions in [(other.as_str(), REMOTE), (NEWPORT, other.as_str())] {
            let output = directory.path().join("output");
            assert!(export(&builds, &output, Some(revisions)).is_err());
            assert!(!output.exists());
        }
    }

    #[test]
    fn rejects_mixed_builds_wrong_hashes_architecture_and_changed_notices() {
        let replacements: [(&str, &[u8]); 4] = [
            (BUILD_PROVENANCE, b"{}"),
            ("newport-agent", b"wrong"),
            (
                "product.json",
                br#"{"schemaVersion":1,"buildSha256":"wrong"}"#,
            ),
            ("licenses/libaom-LICENSE", b"notice replaced after build"),
        ];
        for (file, replacement) in replacements {
            let directory = tempfile::tempdir().unwrap();
            let builds = fixture(directory.path(), false);
            fs::write(builds.join("aarch64").join(file), replacement).unwrap();
            let output = directory.path().join("output");
            assert!(export(&builds, &output, None).is_err(), "{file}");
            assert!(!output.exists());
        }
        let directory = tempfile::tempdir().unwrap();
        let builds = fixture(directory.path(), false);
        let x86 = fs::read(builds.join("x86_64/newport-agent")).unwrap();
        fs::write(builds.join("aarch64/newport-agent"), &x86).unwrap();
        fs::copy(
            builds.join("x86_64/product.json"),
            builds.join("aarch64/product.json"),
        )
        .unwrap();
        let error = export(&builds, &directory.path().join("output"), None).unwrap_err();
        assert!(error.to_string().contains("Wrong ELF architecture"));
        let directory = tempfile::tempdir().unwrap();
        let builds = fixture(directory.path(), false);
        edit_json(
            &builds.join("aarch64").join(BUILD_PROVENANCE),
            |provenance| {
                provenance["remote"]["commit"] = "d".repeat(40).into();
            },
        );
        let error = export(&builds, &directory.path().join("output"), None).unwrap_err();
        assert!(error.to_string().contains("same source"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_build_symlinks_and_keeps_an_existing_export() {
        let directory = tempfile::tempdir().unwrap();
        let builds = fixture(directory.path(), false);
        let output = directory.path().join("output");
        export(&builds, &output, None).unwrap();
        let manifest = fs::read(output.join("newport-build.json")).unwrap();
        let error = export(&builds, &output, None).unwrap_err();
        assert!(error.to_string().contains("new directory"));
        assert_eq!(
            fs::read(output.join("newport-build.json")).unwrap(),
            manifest
        );
        let notice = builds.join("aarch64/licenses/libaom-LICENSE");
        fs::remove_file(&notice).unwrap();
        std::os::unix::fs::symlink(builds.join("x86_64/licenses/libaom-LICENSE"), &notice).unwrap();
        let rejected = directory.path().join("rejected");
        let error = export(&builds, &rejected, None).unwrap_err();
        assert!(error.to_string().contains("symlinks"));
        assert!(!rejected.exists());
    }

    #[test]
    fn a_build_must_identify_the_full_product_of_its_binary() {
        let directory = tempfile::tempdir().unwrap();
        let artifacts = directory.path();
        fs::write(artifacts.join("newport-agent"), "binary").unwrap();
        let write_identity = |variant: &str, sha: String| {
            let mut product = product();
            product["variant"] = variant.into();
            fs::write(
                artifacts.join("product.json"),
                serde_json::to_vec(
                    &json!({"schemaVersion": 1, "buildSha256": sha, "product": product}),
                )
                .unwrap(),
            )
            .unwrap();
        };
        write_identity("full", util::sha256_hex(b"binary"));
        checked_build(artifacts).unwrap();
        write_identity("full", util::sha256_hex(b"other"));
        assert!(checked_build(artifacts).is_err());
        write_identity("core", util::sha256_hex(b"binary"));
        assert!(checked_build(artifacts).is_err());
    }
}
