//! Third-party notices: cargo-about renders the licenses of every crate a
//! desktop package ships, from the dependency graphs it is built from, with
//! `about.toml`'s policy and `about.hbs`. A license the policy doesn't accept
//! fails the package.
use crate::remote_apps::Composed;
use crate::util::{self, Os, Platform};
use crate::{command, root};
use anyhow::{Context, Result, bail, ensure};
use clap::Args;
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// cargo-about's release; `cargo xtask` installs exactly this one.
const VERSION: &str = "0.9.3";
/// The generated file, in the package's `Licenses`.
pub const FILE: &str = "THIRD-PARTY-NOTICES.txt";
/// Server architectures the agent a desktop bundles runs on.
const AGENT_TARGETS: [&str; 2] = ["x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl"];

#[derive(Args, Debug)]
pub struct Options {
    /// Target triple; defaults to CARGO_BUILD_TARGET, then the host.
    #[arg(long)]
    target: Option<String>,
    /// With Remote apps compiled in, from the private checkout at remote/.
    #[arg(long)]
    remote_apps: bool,
    /// Where to write the notices; default target/THIRD-PARTY-NOTICES.txt.
    #[arg(long)]
    output: Option<PathBuf>,
}

/// Write the notices a desktop package of `target` would carry.
pub fn run(options: &Options) -> Result<()> {
    let target = util::build_target(options.target.as_deref());
    let text = if options.remote_apps {
        crate::check::clean_leftover()?;
        desktop(target.as_deref(), Some(&crate::remote_apps::compose()?))?
    } else {
        desktop(target.as_deref(), None)?
    };
    let output = options
        .output
        .clone()
        .unwrap_or_else(|| root().join("target").join(FILE));
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&output, text).with_context(|| format!("Cannot write {}", output.display()))?;
    println!("Third-party notices: {}", output.display());
    Ok(())
}

/// The installed cargo-about, after installing it into `target/tools` with
/// Cargo the first time, so every checkout and CI runner uses the same one.
pub fn cargo_about() -> Result<PathBuf> {
    let directory = root().join(format!("target/tools/cargo-about-{VERSION}"));
    let binary = directory
        .join("bin")
        .join(format!("cargo-about{}", std::env::consts::EXE_SUFFIX));
    let installed = || {
        util::capture(Command::new(&binary).arg("--version"))
            .is_ok_and(|version| version == format!("cargo-about {VERSION}"))
    };
    if !installed() {
        let mut install = command("cargo");
        install
            .args(["install", "--locked", "--features", "cli", "--root"])
            .arg(&directory)
            .arg(format!("cargo-about@{VERSION}"))
            // It runs here, whatever the package targets.
            .env_remove("CARGO_BUILD_TARGET");
        util::run(util::without_release_secrets(&mut install))
            .context("Cannot install cargo-about")?;
        ensure!(installed(), "cargo-about {VERSION} did not install");
    }
    Ok(binary)
}

/// One Cargo dependency graph a package ships.
struct Graph<'a> {
    title: String,
    manifest: PathBuf,
    targets: Vec<String>,
    /// Exactly these features instead of the defaults.
    features: Option<&'a str>,
    locked: bool,
    /// `--config` arguments the build passes to Cargo.
    config: &'a [String],
}

/// The desktop executable's Rust target triple.
fn desktop_triple(target: Option<&str>) -> Result<String> {
    if let Some(triple) = target {
        return Ok(triple.to_owned());
    }
    let host = Platform::host();
    Ok(match host.os {
        Os::Macos => format!("{}-apple-darwin", host.arch),
        Os::Windows => format!("{}-pc-windows-msvc", host.arch),
        Os::Linux => bail!("Newport has no Linux desktop package"),
    })
}

/// The graphs a desktop package of `target` ships: the app, the grammar
/// libraries and the agent it installs on servers. With Remote apps composed
/// (and GPUI patched for them), the app includes the private crates and the
/// agent is the Full agent.
fn desktop_graphs<'a>(root: &Path, target: &str, patch: Option<&'a [String]>) -> Vec<Graph<'a>> {
    let config = patch.unwrap_or_default();
    let locked = patch.is_none();
    let (agent, agent_manifest) = match patch {
        Some(_) => (
            "Newport agent with Remote apps",
            root.join("remote/products/unified-agent/Cargo.toml"),
        ),
        None => (
            "Newport agent",
            root.join("crates/newport-agent/Cargo.toml"),
        ),
    };
    vec![
        Graph {
            title: format!("Newport ({target})"),
            manifest: root.join("crates/newport-app/Cargo.toml"),
            targets: vec![target.to_owned()],
            features: None,
            locked,
            config,
        },
        Graph {
            title: format!("Syntax grammars ({target})"),
            manifest: root.join("crates/newport-grammars/Cargo.toml"),
            targets: vec![target.to_owned()],
            features: Some("catalog"),
            locked,
            config,
        },
        Graph {
            title: format!("{agent} ({})", AGENT_TARGETS.join(", ")),
            manifest: agent_manifest,
            targets: AGENT_TARGETS.map(str::to_owned).to_vec(),
            features: None,
            locked: true,
            config: &[],
        },
    ]
}

/// The notices of a desktop package: every graph it ships, then the
/// statements of the changes Newport made to patched crates.
pub fn desktop(target: Option<&str>, composed: Option<&Composed>) -> Result<String> {
    let root = root();
    let tool = cargo_about()?;
    let target = desktop_triple(target)?;
    let product = util::app()?;
    let mut text = format!(
        "Third-party notices for {} {}\n\n\
         {} is licensed under the GNU Affero General Public License, version 3\n\
         only, in LICENSE. These are the licenses of the third-party Rust crates\n\
         it is built from, generated from its dependency graphs by cargo-about,\n\
         and the NOTICE files those crates publish. Bundled fonts and icons keep\n\
         their own notices beside them.\n",
        product.product_name, product.version, product.product_name
    );
    let patch = composed.map(|composed| &composed.patch[..]);
    for graph in desktop_graphs(&root, &target, patch) {
        section(&mut text, &graph.title);
        text.push_str(&render(&tool, &root, &graph)?);
    }
    let mut patched = vec![root.join("patches")];
    if composed.is_some() {
        patched.push(root.join("remote/patches"));
    }
    let statements = change_statements(&patched)?;
    if !statements.is_empty() {
        section(&mut text, "Changes Newport made to these crates");
        text.push_str(&statements.join("\n"));
    }
    Ok(text)
}

fn section(text: &mut String, title: &str) {
    let rule = "=".repeat(72);
    text.push_str(&format!("\n\n{rule}\n{title}\n{rule}\n\n"));
}

/// The `<name>.NOTICE` next to each patched crate's patch, which states what
/// Newport changed as the crate's license asks.
fn change_statements(directories: &[PathBuf]) -> Result<Vec<String>> {
    let mut statements = Vec::new();
    for directory in directories {
        let mut paths: Vec<PathBuf> = match fs::read_dir(directory) {
            Ok(entries) => entries
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<Result<_, _>>()?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error.into()),
        };
        paths.retain(|path| path.extension().is_some_and(|ext| ext == "NOTICE"));
        paths.sort();
        for path in paths {
            statements.push(
                fs::read_to_string(&path)
                    .with_context(|| format!("Cannot read {}", path.display()))?,
            );
        }
    }
    Ok(statements)
}

/// A directory to run Cargo in with the build's `--config` arguments as its
/// configuration: cargo-about runs Cargo itself and can't pass them on.
fn workspace(root: &Path, config: &[String]) -> Result<tempfile::TempDir> {
    let parent = root.join("target/notices");
    fs::create_dir_all(&parent)?;
    let directory = tempfile::tempdir_in(&parent)?;
    if !config.is_empty() {
        let settings: Vec<&str> = config
            .iter()
            .map(|argument| {
                argument
                    .strip_prefix("--config=")
                    .context("Unsupported Cargo configuration argument")
            })
            .collect::<Result<_>>()?;
        fs::create_dir(directory.path().join(".cargo"))?;
        fs::write(
            directory.path().join(".cargo/config.toml"),
            settings.join("\n") + "\n",
        )?;
    }
    Ok(directory)
}

/// Render one graph with `about.hbs`, after checking cargo-about leaves out
/// only Newport's own crates.
fn render(tool: &Path, root: &Path, graph: &Graph) -> Result<String> {
    let directory = workspace(root, graph.config)?;
    let selection = |c: &mut Command| {
        c.current_dir(directory.path())
            .arg("--manifest-path")
            .arg(&graph.manifest);
        if graph.locked {
            c.arg("--locked");
        }
        if let Some(features) = graph.features {
            c.args(["--no-default-features", "--features", features]);
        }
    };
    let mut metadata = command("cargo");
    metadata.args(["metadata", "--format-version", "1", "--quiet"]);
    selection(&mut metadata);
    let metadata = util::capture(&mut metadata)?;
    check_unpublished(root, &metadata)?;

    let output = directory.path().join(FILE);
    let mut about = command(tool);
    about
        .args(["generate", "--fail", "--config"])
        .arg(root.join("about.toml"))
        .arg("--output-file")
        .arg(&output);
    for target in &graph.targets {
        about.args(["--target", target]);
    }
    selection(&mut about);
    about.arg(root.join("about.hbs"));
    let result = util::without_release_secrets(&mut about)
        .output()
        .context("Cannot start cargo-about")?;
    let log = String::from_utf8_lossy(&result.stderr);
    if !result.status.success() || degraded(&log) {
        eprint!("{log}");
        bail!(
            "cargo-about could not license every crate in {}",
            graph.manifest.display()
        );
    }
    let mut text = fs::read_to_string(&output).context("cargo-about wrote no notices")?;
    text.push_str(&dependency_notices(&metadata)?);
    Ok(text)
}

/// The NOTICE files third-party crates publish, which Apache-2.0 asks every
/// redistribution to carry. cargo-about copies only license texts.
fn dependency_notices(metadata: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Metadata {
        packages: Vec<Package>,
    }
    #[derive(Deserialize)]
    struct Package {
        name: String,
        version: String,
        source: Option<String>,
        manifest_path: PathBuf,
    }
    let metadata: Metadata = serde_json::from_str(metadata).context("Invalid cargo metadata")?;
    let mut notices = std::collections::BTreeMap::new();
    for package in metadata.packages {
        // Newport's own crates and the private Remote apps crates.
        if package.source.is_none() {
            continue;
        }
        let Some(directory) = package.manifest_path.parent() else {
            continue;
        };
        let mut files: Vec<PathBuf> = fs::read_dir(directory)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<_, _>>()?;
        files.retain(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.to_ascii_uppercase().starts_with("NOTICE"))
        });
        files.sort();
        for file in files {
            let text = fs::read_to_string(&file)
                .with_context(|| format!("Cannot read {}", file.display()))?;
            notices.insert(
                (package.name.clone(), package.version.clone()),
                text.trim_end().to_owned(),
            );
        }
    }
    let mut text = String::new();
    for ((name, version), notice) in notices {
        text.push_str(&format!("\n\nNOTICE of {name} {version}:\n\n{notice}\n"));
    }
    Ok(text)
}

/// cargo-about only warns when a clarification's files can't be fetched or
/// no longer match, then falls back to a generic license text.
fn degraded(log: &str) -> bool {
    log.contains("failed to validate all files specified in clarification")
        || log.contains("[ERROR]")
}

/// `about.toml` ignores unpublished crates, which must then be Newport's or
/// the private Remote apps source's, never a dependency's.
fn check_unpublished(root: &Path, metadata: &str) -> Result<()> {
    #[derive(Deserialize)]
    struct Metadata {
        packages: Vec<Package>,
    }
    #[derive(Deserialize)]
    struct Package {
        name: String,
        source: Option<String>,
        publish: Option<Vec<String>>,
        manifest_path: PathBuf,
    }
    let metadata: Metadata = serde_json::from_str(metadata).context("Invalid cargo metadata")?;
    let real = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_owned());
    let root = real(root);
    for package in metadata.packages {
        if package
            .publish
            .is_some_and(|registries| registries.is_empty())
        {
            ensure!(
                package.source.is_none() && real(&package.manifest_path).starts_with(&root),
                "{} is unpublished, so its license would be left out of the notices",
                package.name
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A workspace whose app depends on one path crate under `license`, in
    /// this checkout, where Newport's own unpublished crates live.
    fn fixture(license: &str) -> tempfile::TempDir {
        let parent = root().join("target/notices");
        fs::create_dir_all(&parent).unwrap();
        let directory = tempfile::tempdir_in(parent).unwrap();
        let path = directory.path();
        fs::create_dir_all(path.join("app/src")).unwrap();
        fs::create_dir_all(path.join("dependency/src")).unwrap();
        fs::write(
            path.join("app/Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
             license = \"AGPL-3.0-only\"\npublish = false\n\n[workspace]\n\n\
             [dependencies]\ndependency = { path = \"../dependency\" }\n",
        )
        .unwrap();
        fs::write(
            path.join("dependency/Cargo.toml"),
            format!(
                "[package]\nname = \"dependency\"\nversion = \"1.0.0\"\nedition = \"2021\"\n\
                 license = \"{license}\"\nauthors = [\"Fixture Author\"]\n"
            ),
        )
        .unwrap();
        fs::write(path.join("app/src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(path.join("dependency/src/lib.rs"), "").unwrap();
        directory
    }

    fn graph(manifest: PathBuf) -> Graph<'static> {
        Graph {
            title: "Fixture".into(),
            manifest,
            targets: Vec::new(),
            features: None,
            locked: false,
            config: &[],
        }
    }

    #[test]
    fn notices_name_each_permissive_dependency_and_refuse_any_other() {
        let tool = cargo_about().unwrap();
        let permissive = fixture("MIT");
        let text = render(
            &tool,
            &root(),
            &graph(permissive.path().join("app/Cargo.toml")),
        )
        .unwrap();
        assert!(text.contains("MIT License (MIT)"), "{text}");
        assert!(text.contains("dependency 1.0.0"), "{text}");
        assert!(text.contains("Authors: Fixture Author"), "{text}");
        assert!(
            !text.contains("app 0.1.0"),
            "Newport's own crate is not third-party"
        );

        for license in [
            "GPL-3.0-only",
            "LGPL-2.1-or-later",
            "AGPL-3.0-only",
            "SSPL-1.0",
        ] {
            let refused = fixture(license);
            let error = render(
                &tool,
                &root(),
                &graph(refused.path().join("app/Cargo.toml")),
            )
            .unwrap_err();
            assert!(
                error.to_string().contains("could not license"),
                "{license}: {error}"
            );
        }
    }

    #[test]
    fn only_newports_own_crates_may_be_unpublished() {
        let root = Path::new("/checkout");
        let metadata = |source: &str, path: &str| {
            format!(
                r#"{{"packages":[{{"name":"crate","source":{source},"publish":[],"manifest_path":"{path}"}}]}}"#
            )
        };
        check_unpublished(root, &metadata("null", "/checkout/crates/app/Cargo.toml")).unwrap();
        check_unpublished(
            root,
            &metadata("null", "/checkout/remote/crates/a/Cargo.toml"),
        )
        .unwrap();
        for (source, path) in [
            (r#""git+https://example.com/a""#, "/cargo/git/a/Cargo.toml"),
            ("null", "/elsewhere/a/Cargo.toml"),
        ] {
            let error = check_unpublished(root, &metadata(source, path)).unwrap_err();
            assert!(error.to_string().contains("left out"), "{error}");
        }
        let published = r#"{"packages":[{"name":"crate","source":"registry+https://github.com/rust-lang/crates.io-index","publish":null,"manifest_path":"/cargo/registry/a/Cargo.toml"}]}"#;
        check_unpublished(root, published).unwrap();
    }

    #[test]
    fn a_failed_clarification_fails_instead_of_falling_back() {
        assert!(degraded(
            "[WARN] failed to validate all files specified in clarification for crate a 1.0.0"
        ));
        assert!(degraded(
            "[ERROR] unable to scan for license files for crate 'a - 1.0.0'"
        ));
        assert!(!degraded(
            "warning: config found for a crate not present in the graph"
        ));
    }

    #[test]
    fn build_configuration_reaches_cargo_from_the_working_directory() {
        let root = tempfile::tempdir().unwrap();
        let config = [
            "--config=patch.crates-io.gpui-pre.git='https://example.com/gpui'".to_owned(),
            "--config=patch.crates-io.gpui-pre.rev='abc'".to_owned(),
        ];
        let directory = workspace(root.path(), &config).unwrap();
        let written: toml::Value = toml::from_str(
            &fs::read_to_string(directory.path().join(".cargo/config.toml")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            written["patch"]["crates-io"]["gpui-pre"]["rev"].as_str(),
            Some("abc")
        );
        assert!(
            !workspace(root.path(), &[])
                .unwrap()
                .path()
                .join(".cargo")
                .exists()
        );
        assert!(workspace(root.path(), &["--locked".to_owned()]).is_err());
    }

    #[test]
    fn remote_apps_ship_the_full_agent_and_the_changes_to_its_crates() {
        let root = Path::new("/checkout");
        let core = desktop_graphs(root, "aarch64-apple-darwin", None);
        assert_eq!(
            core[2].manifest,
            root.join("crates/newport-agent/Cargo.toml")
        );
        assert_eq!(core[2].targets, AGENT_TARGETS);
        assert!(
            core.iter()
                .all(|graph| graph.locked && graph.config.is_empty())
        );
        assert_eq!(core[1].features, Some("catalog"));
        assert_eq!(core[0].targets, ["aarch64-apple-darwin"]);

        let patch = ["--config=patch.crates-io.gpui-pre.rev='abc'".to_owned()];
        let full = desktop_graphs(root, "x86_64-pc-windows-msvc", Some(&patch));
        assert_eq!(
            full[2].manifest,
            root.join("remote/products/unified-agent/Cargo.toml")
        );
        // The private crates change the lockfile for the build, which passes
        // the patch to Cargo; the Full agent keeps its own lockfile.
        assert!(
            full.iter()
                .take(2)
                .all(|graph| !graph.locked && graph.config == patch)
        );
        assert!(full[2].locked && full[2].config.is_empty());

        let directory = tempfile::tempdir().unwrap();
        let patches = directory.path().join("patches");
        fs::create_dir(&patches).unwrap();
        fs::write(patches.join("cpal.NOTICE"), "Newport modifies cpal.\n").unwrap();
        fs::write(patches.join("cpal.patch"), "patch").unwrap();
        assert_eq!(
            change_statements(&[patches, directory.path().join("absent")]).unwrap(),
            ["Newport modifies cpal.\n"]
        );
    }

    #[test]
    fn dependency_notice_files_are_carried_and_newports_own_are_not() {
        let directory = tempfile::tempdir().unwrap();
        let crate_dir = directory.path().join("dependency");
        fs::create_dir_all(&crate_dir).unwrap();
        fs::write(crate_dir.join("NOTICE"), "Copyright the dependency.\n").unwrap();
        let own = directory.path().join("own");
        fs::create_dir_all(&own).unwrap();
        fs::write(own.join("NOTICE"), "Newport's own.\n").unwrap();
        let metadata = serde_json::json!({"packages": [
            {"name": "dependency", "version": "1.0.0",
             "source": "registry+https://github.com/rust-lang/crates.io-index",
             "manifest_path": crate_dir.join("Cargo.toml")},
            {"name": "own", "version": "0.1.0", "source": null,
             "manifest_path": own.join("Cargo.toml")},
        ]})
        .to_string();
        let text = dependency_notices(&metadata).unwrap();
        assert!(text.contains("NOTICE of dependency 1.0.0:\n\nCopyright the dependency."));
        assert!(!text.contains("Newport's own"));
    }
}
