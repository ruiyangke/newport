//! Stage, verify and archive the desktop app.
use crate::util::{self, Os, Platform};
use crate::{build_info, command, grammars, root};
use anyhow::{Context, Result, bail, ensure};
use clap::Args;
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

mod macos;
pub mod windows;

/// The notices of the icons the UI embeds, in `crates/newport-gpui/assets`.
const ICON_NOTICES: [&str; 7] = [
    "file-icons/LICENSE-lucide",
    "file-icons/LICENSE-symbols",
    "process-icons/DISCLAIMER.md",
    "process-icons/LICENSE.md",
    "process-icons/LICENSE-opencode",
    "process-icons/SOURCES.json",
    "process-icons/SOURCE-opencode.json",
];

/// Every file a package's `Licenses` holds, relative to its resources.
fn license_files() -> Vec<String> {
    std::iter::once(format!("Licenses/{}", crate::notices::FILE))
        .chain(ICON_NOTICES.map(|path| format!("Licenses/icons/{path}")))
        .collect()
}

#[derive(Args, Debug, Clone, Default)]
pub struct Options {
    /// Package the debug profile.
    #[arg(long)]
    pub debug: bool,
    /// Package an existing build instead of building first.
    #[arg(long)]
    pub skip_build: bool,
    /// Target triple; defaults to CARGO_BUILD_TARGET, then the host.
    #[arg(long)]
    pub target: Option<String>,
    /// Windows: also build the NSIS installer.
    #[arg(long)]
    pub nsis: bool,
    /// Compile Remote apps in, from the private newport-remote
    /// checkout at remote/.
    #[arg(long)]
    pub remote_apps: bool,
}

/// What a package run produced.
#[derive(Debug, Clone)]
pub struct Package {
    /// `native-package` directory in the profile directory.
    pub stage: PathBuf,
    /// `Newport.app` on macOS, the `Newport` directory on Windows.
    pub app: PathBuf,
    /// Zip archive of the app.
    pub archive: PathBuf,
    /// Windows installer, with `--nsis`.
    pub installer: Option<PathBuf>,
}

/// The platform to package: a macOS or Windows MSVC triple for this host, else the host.
fn platform(target: Option<&str>, host: Platform) -> Result<Platform> {
    if host.os == Os::Linux || target.is_some_and(|t| t.contains("-linux-")) {
        bail!("Newport has no Linux desktop package; package on macOS or Windows");
    }
    let Some(triple) = target else {
        return Ok(host);
    };
    let supported = matches!(
        triple,
        "aarch64-apple-darwin"
            | "x86_64-apple-darwin"
            | "aarch64-pc-windows-msvc"
            | "x86_64-pc-windows-msvc"
    );
    let platform = Platform::from_triple(triple).ok().filter(|_| supported);
    match platform {
        Some(platform) if platform.os == host.os => Ok(platform),
        _ => bail!("Package target must be a supported triple for this host platform"),
    }
}

/// Build the app without release secrets. With Remote apps the private crates
/// change the lockfile for the build, so it isn't locked.
fn cargo_build(
    release: bool,
    target: Option<&str>,
    remote_apps: Option<&crate::remote_apps::Prepared>,
) -> Command {
    let mut c = command("cargo");
    c.arg("build");
    match remote_apps {
        Some(prepared) => {
            prepared.apply(&mut c);
        }
        None => {
            c.arg("--locked");
        }
    }
    if release {
        c.arg("--release");
    }
    c.args(["-p", "newport-app"]);
    util::with_build_stamp(&mut c);
    if let Some(target) = target {
        c.args(["--target", target]);
    }
    util::without_release_secrets(&mut c);
    c
}

/// Build the grammars, add Remote apps when asked, write the notices from
/// that dependency graph, then build the app. With `skip_build`, only the
/// notices, which then compose Remote apps themselves. The private crates
/// leave the manifests when this returns. Returns the Full agent export a
/// Remote apps build selected, and the notices.
fn build(
    skip_build: bool,
    release: bool,
    target: Option<&str>,
    grammars: impl FnOnce() -> Result<()>,
    remote_apps: impl FnOnce() -> Result<Option<crate::remote_apps::Prepared>>,
    notices: impl FnOnce(Option<&crate::remote_apps::Composed>) -> Result<String>,
    run: impl FnOnce(&mut Command) -> Result<()>,
) -> Result<(Option<PathBuf>, String)> {
    if skip_build {
        return Ok((None, notices(None)?));
    }
    grammars()?;
    let remote_apps = remote_apps()?;
    // Before the build, so a refused license fails fast.
    let notices = notices(remote_apps.as_ref().map(|prepared| &prepared.composed))?;
    run(&mut cargo_build(release, target, remote_apps.as_ref()))?;
    Ok((
        remote_apps
            .as_ref()
            .map(|prepared| prepared.resources.clone()),
        notices,
    ))
}

fn signed_agent_export(
    remote_apps: bool,
    required: bool,
    built: Option<&Path>,
    explicit: Option<&Path>,
) -> Result<Option<PathBuf>> {
    if !remote_apps || !required {
        return Ok(None);
    }
    built.or(explicit).map(|path| Some(path.to_owned()))
        .context("Signed Full packaging requires its checked agent export; for --skip-build set NEWPORT_AGENT_RESOURCE_DIR")
}

/// The root workspace's target directory and the `newport-app` version.
fn metadata() -> Result<(PathBuf, String)> {
    #[derive(Deserialize)]
    struct Metadata {
        target_directory: PathBuf,
        packages: Vec<Member>,
    }
    #[derive(Deserialize)]
    struct Member {
        name: String,
        version: String,
    }
    let json = util::capture(command("cargo").args([
        "metadata",
        "--no-deps",
        "--format-version",
        "1",
        "--locked",
    ]))?;
    let metadata: Metadata = serde_json::from_str(&json).context("Invalid cargo metadata")?;
    let version = metadata
        .packages
        .into_iter()
        .find(|p| p.name == "newport-app")
        .map(|p| p.version)
        .context("The workspace has no newport-app package")?;
    Ok((metadata.target_directory, version))
}

/// Where staged files go inside the app.
struct Layout {
    pub app: PathBuf,
    pub resources: PathBuf,
    pub executable: PathBuf,
}

fn copy_file(from: &Path, to: &Path) -> Result<()> {
    fs::copy(from, to)
        .map(drop)
        .with_context(|| format!("Cannot copy {} to {}", from.display(), to.display()))
}

/// Copy a directory tree of regular files; symlinks are refused.
fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    for entry in walkdir::WalkDir::new(from).sort_by_file_name() {
        let entry = entry.with_context(|| format!("Cannot read {}", from.display()))?;
        let destination = to.join(entry.path().strip_prefix(from)?);
        let kind = entry.file_type();
        if kind.is_dir() {
            fs::create_dir_all(&destination)
                .with_context(|| format!("Cannot create {}", destination.display()))?;
        } else if kind.is_file() {
            copy_file(entry.path(), &destination)?;
        } else {
            bail!(
                "Unsupported package source entry: {}",
                entry.path().display()
            );
        }
    }
    Ok(())
}

/// Remove a previous staged app: a directory tree, or a file or link in its place.
fn remove(path: &Path) -> Result<()> {
    let result = match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => Err(error),
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
    };
    result.with_context(|| format!("Cannot remove {}", path.display()))
}

/// Recreate the app in `stage` with the binary, agents, grammars, fonts and licenses
/// from the repository `root` and the profile directory `output`, and the
/// generated third-party `notices`.
fn stage_app(
    root: &Path,
    output: &Path,
    stage: &Path,
    os: Os,
    stub: Option<&Path>,
    notices: &str,
) -> Result<Layout> {
    let mac = os == Os::Macos;
    let binary = if os == Os::Windows {
        "newport.exe"
    } else {
        "newport"
    };
    let app = stage.join(if mac { "Newport.app" } else { "Newport" });
    // Generated packaging output, never an installed application.
    remove(&app)?;
    let bin = if mac {
        app.join("Contents/MacOS")
    } else {
        app.clone()
    };
    let resources = if mac {
        app.join("Contents/Resources")
    } else {
        app.clone()
    };
    fs::create_dir_all(&bin)?;
    fs::create_dir_all(&resources)?;
    let executable = bin.join(binary);
    copy_file(&output.join(binary), &executable)?;
    copy_dir(&output.join("agents"), &resources.join("agents"))?;
    copy_dir(&output.join("grammars"), &resources.join("grammars"))?;
    copy_dir(&root.join("resources/fonts"), &resources.join("Fonts"))?;
    copy_file(&root.join("LICENSE"), &resources.join("LICENSE"))?;
    copy_file(
        &root.join("crates/newport-grammars/LICENSE-GPUI-KIT"),
        &resources.join("LICENSE-GPUI-KIT"),
    )?;
    stage_icon_notices(root, &resources)?;
    fs::write(
        resources.join("Licenses").join(crate::notices::FILE),
        notices,
    )?;
    // Remote apps opened as separate Mac apps copy this binary.
    if let Some(stub) = stub.filter(|_| mac) {
        let helpers = app.join("Contents/Helpers");
        fs::create_dir_all(&helpers)?;
        copy_file(stub, &helpers.join(crate::remote_apps::STUB_NAME))?;
    }
    Ok(Layout {
        app,
        resources,
        executable,
    })
}

/// Copy the icon notices into `Licenses/icons`, after checking every source
/// is a regular file, so nothing outside the assets is copied.
fn stage_icon_notices(root: &Path, resources: &Path) -> Result<()> {
    let assets = root.join("crates/newport-gpui/assets");
    for path in [
        assets.clone(),
        assets.join("file-icons"),
        assets.join("process-icons"),
    ] {
        ensure!(
            fs::symlink_metadata(&path)?.is_dir(),
            "Expected an icon notice directory: {}",
            path.display()
        );
    }
    for name in ICON_NOTICES {
        let source = assets.join(name);
        ensure!(
            fs::symlink_metadata(&source)?.is_file(),
            "Expected a regular icon notice: {}",
            source.display()
        );
    }
    for name in ICON_NOTICES {
        let destination = resources.join("Licenses/icons").join(name);
        fs::create_dir_all(destination.parent().unwrap())?;
        copy_file(&assets.join(name), &destination)?;
    }
    Ok(())
}

/// The staged agent notices are exactly the ones the export lists, byte for
/// byte, on every OS.
fn check_agent_notices(resources: &Path, export: &Path) -> Result<()> {
    let listed: BTreeSet<String> = crate::remote_apps::full_agent::exported_notices(export)?
        .into_iter()
        .collect();
    let staged = resources.join("agents/licenses");
    let mut found = BTreeSet::new();
    for entry in
        fs::read_dir(&staged).with_context(|| format!("Cannot read {}", staged.display()))?
    {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|name| anyhow::anyhow!("Invalid agent notice name: {name:?}"))?;
        ensure!(
            entry.file_type()?.is_file() && listed.contains(&name),
            "The agent export does not list the staged notice {name}"
        );
        ensure!(
            fs::read(entry.path())? == fs::read(export.join("licenses").join(&name))?,
            "The staged agent notice {name} differs from the export"
        );
        found.insert(name);
    }
    ensure!(
        found == listed,
        "The staged agent notices lack some the export lists"
    );
    Ok(())
}

/// Run a staged binary's self-check without release secrets.
fn probe(executable: &Path, check: &str) -> Result<()> {
    util::run(util::without_release_secrets(
        command(executable).arg(check),
    ))
    .with_context(|| format!("Packaged app failed {check}"))
}

/// Everything a platform step needs.
struct Staged<'a> {
    pub root: &'a Path,
    pub stage: &'a Path,
    pub layout: &'a Layout,
    pub platform: Platform,
    pub release: bool,
    pub target: Option<&'a str>,
    pub expect: build_info::Expect<'a>,
    pub product: &'a util::App,
    pub nsis: bool,
    /// The checked Full agent export a Remote apps package embeds, when known.
    pub agent_export: Option<&'a Path>,
    pub signed_agent_export: Option<&'a Path>,
}

/// Build unless `skip_build`, stage the app for this host, verify it and archive it.
pub fn run(options: &Options) -> Result<Package> {
    let release = !options.debug;
    let target = util::build_target(options.target.as_deref());
    let target = target.as_deref();
    let platform = platform(target, Platform::host())?;
    ensure!(
        !options.nsis || platform.os == Os::Windows,
        "Build the Windows installer on Windows"
    );
    let product = util::app()?;
    let (built_export, notices) = build(
        options.skip_build,
        release,
        target,
        || grammars::build(release, target).map(drop),
        || {
            if !options.remote_apps {
                return Ok(None);
            }
            crate::check::clean_leftover()?;
            crate::remote_apps::prepare().map(Some)
        },
        |composed| match composed {
            Some(composed) => crate::notices::desktop(target, Some(composed)),
            None if options.remote_apps => {
                crate::check::clean_leftover()?;
                crate::notices::desktop(target, Some(&crate::remote_apps::compose()?))
            }
            None => crate::notices::desktop(target, None),
        },
        util::run,
    )?;
    let explicit_export = std::env::var_os("NEWPORT_AGENT_RESOURCE_DIR").map(PathBuf::from);
    let agent_export = built_export
        .or(explicit_export)
        .filter(|_| options.remote_apps)
        .map(|export| crate::remote_apps::checked_full_resources(&root(), &export))
        .transpose()?;
    let signed_export = signed_agent_export(
        options.remote_apps,
        std::env::var_os("NEWPORT_REQUIRE_NEEDLE_RELEASE").is_some(),
        agent_export.as_deref(),
        None,
    )?;
    let (target_directory, version) = metadata()?;
    ensure!(
        version == product.version,
        "Package and product versions differ"
    );
    let mut output = target_directory;
    if let Some(triple) = target {
        output.push(triple);
    }
    output.push(if release { "release" } else { "debug" });
    let stage = output.join("native-package");
    fs::create_dir_all(&stage)?;
    let root = root();
    let stub = if options.remote_apps && platform.os == Os::Macos {
        Some(crate::remote_apps::package_stub(target)?)
    } else {
        None
    };
    let layout = stage_app(
        &root,
        &output,
        &stage,
        platform.os,
        stub.as_deref(),
        &notices,
    )?;
    if let Some(export) = &agent_export {
        check_agent_notices(&layout.resources, export)?;
    }
    let staged = Staged {
        root: &root,
        stage: &stage,
        layout: &layout,
        platform,
        release,
        target,
        expect: build_info::Expect {
            os: Some(platform.os.as_str()),
            arch: Some(platform.arch.as_str()),
            debug: Some(options.debug),
            remote_apps: Some(options.remote_apps),
        },
        product: &product,
        nsis: options.nsis,
        agent_export: agent_export.as_deref(),
        signed_agent_export: signed_export.as_deref(),
    };
    let (archive, installer) = match platform.os {
        Os::Macos => (macos::finish(&staged)?, None),
        Os::Windows => windows::finish(&staged)?,
        Os::Linux => unreachable!("rejected by platform()"),
    };
    let package = Package {
        stage,
        app: layout.app,
        archive,
        installer,
    };
    println!("Development package: {}", package.stage.display());
    println!("Archive: {}", package.archive.display());
    if let Some(installer) = &package.installer {
        println!("Unsigned installer: {}", installer.display());
    }
    Ok(package)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::Arch;
    use std::{cell::RefCell, collections::HashMap, ffi::OsStr};

    const MAC: Platform = Platform {
        os: Os::Macos,
        arch: Arch::Aarch64,
    };
    const WINDOWS: Platform = Platform {
        os: Os::Windows,
        arch: Arch::X86_64,
    };

    #[test]
    fn targets_must_be_desktop_triples_for_this_host() {
        assert_eq!(platform(None, MAC).unwrap(), MAC);
        assert_eq!(
            platform(Some("x86_64-apple-darwin"), MAC).unwrap(),
            Platform {
                os: Os::Macos,
                arch: Arch::X86_64
            }
        );
        assert_eq!(
            platform(Some("aarch64-pc-windows-msvc"), WINDOWS).unwrap(),
            Platform {
                os: Os::Windows,
                arch: Arch::Aarch64
            }
        );
        for (target, host) in [
            ("x86_64-pc-windows-msvc", MAC),
            ("aarch64-apple-darwin", WINDOWS),
            ("x86_64-pc-windows-gnu", WINDOWS),
            ("aarch64-apple-darwin-extra", MAC),
            ("riscv64gc-apple-darwin", MAC),
        ] {
            let error = platform(Some(target), host).unwrap_err().to_string();
            assert!(error.contains("supported triple"), "{target}: {error}");
        }
        let linux = Platform {
            os: Os::Linux,
            arch: Arch::X86_64,
        };
        for (target, host) in [
            (None, linux),
            (Some("x86_64-unknown-linux-gnu"), linux),
            (Some("aarch64-unknown-linux-musl"), MAC),
        ] {
            let error = platform(target, host).unwrap_err().to_string();
            assert!(error.contains("no Linux desktop package"), "{error}");
        }
    }

    #[test]
    fn building_writes_the_notices_between_the_grammars_and_the_app_without_secrets() {
        let steps = RefCell::new(Vec::new());
        let (export, notices) = build(
            false,
            false,
            None,
            || {
                steps.borrow_mut().push("grammars".to_owned());
                Ok(())
            },
            || Ok(None),
            |composed| {
                assert!(composed.is_none());
                steps.borrow_mut().push("notices".to_owned());
                Ok("Notices".to_owned())
            },
            |c| {
                steps.borrow_mut().push("cargo".to_owned());
                assert_eq!(c.get_program(), "cargo");
                let args: Vec<_> = c.get_args().collect();
                assert_eq!(args, ["build", "--locked", "-p", "newport-app"]);
                let envs: HashMap<_, _> = c.get_envs().collect();
                for name in util::RELEASE_SECRETS {
                    assert_eq!(envs.get(OsStr::new(name)), Some(&None), "{name}");
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(export, None);
        assert_eq!(notices, "Notices");
        assert_eq!(*steps.borrow(), ["grammars", "notices", "cargo"]);

        let c = cargo_build(true, Some("aarch64-apple-darwin"), None);
        let args: Vec<_> = c.get_args().collect();
        assert_eq!(
            args,
            [
                "build",
                "--locked",
                "--release",
                "-p",
                "newport-app",
                "--target",
                "aarch64-apple-darwin"
            ]
        );
    }

    #[test]
    fn prebuilt_packaging_skips_build_steps_but_still_writes_the_notices() {
        let (export, notices) = build(
            true,
            true,
            None,
            || panic!("must not build grammars"),
            || panic!("must not add Remote apps"),
            |composed| {
                assert!(composed.is_none());
                Ok("Notices".to_owned())
            },
            |_| panic!("must not build"),
        )
        .unwrap();
        assert_eq!(export, None);
        assert_eq!(notices, "Notices");
    }

    #[test]
    fn signed_full_packaging_keeps_the_builds_resolved_export_without_parent_environment() {
        let resolved = Path::new("checked-default-full-export");
        let other = Path::new("different-parent-export");
        assert_eq!(
            signed_agent_export(true, true, Some(resolved), None).unwrap(),
            Some(resolved.to_owned())
        );
        assert_eq!(
            signed_agent_export(true, true, Some(resolved), Some(other)).unwrap(),
            Some(resolved.to_owned())
        );
        assert_eq!(
            signed_agent_export(true, true, None, Some(other)).unwrap(),
            Some(other.to_owned())
        );
        assert!(
            signed_agent_export(true, true, None, None)
                .unwrap_err()
                .to_string()
                .contains("--skip-build")
        );
        for (remote_apps, required) in [(false, true), (false, false), (true, false)] {
            assert_eq!(
                signed_agent_export(remote_apps, required, None, None).unwrap(),
                None
            );
        }
    }

    fn files(directory: &Path) -> Vec<PathBuf> {
        walkdir::WalkDir::new(directory)
            .sort_by_file_name()
            .into_iter()
            .map(Result::unwrap)
            .filter(|e| e.file_type().is_file())
            .map(|e| e.path().strip_prefix(directory).unwrap().to_owned())
            .collect()
    }

    #[test]
    fn staging_retains_agents_and_ships_the_notices_and_licenses() {
        let directory = tempfile::tempdir().unwrap();
        let fixture = directory.path();
        let output = fixture.join("target/debug");
        for path in [
            output.join("grammars"),
            output.join("agents"),
            fixture.join("resources/icons"),
            fixture.join("crates/newport-grammars"),
        ] {
            fs::create_dir_all(path).unwrap();
        }
        fs::write(output.join("newport"), "fixture, never executed").unwrap();
        fs::write(output.join("newport.exe"), "fixture, never executed").unwrap();
        fs::write(output.join("grammars/catalog.json"), "{}").unwrap();
        for arch in ["x86_64", "aarch64"] {
            fs::write(
                output.join(format!("agents/newport-agent-{arch}.gz")),
                format!("compressed {arch}"),
            )
            .unwrap();
        }
        fs::write(
            fixture.join("crates/newport-grammars/LICENSE-GPUI-KIT"),
            "fixture",
        )
        .unwrap();
        copy_file(&root().join("LICENSE"), &fixture.join("LICENSE")).unwrap();
        for name in ICON_NOTICES {
            let path = Path::new("crates/newport-gpui/assets").join(name);
            fs::create_dir_all(fixture.join(&path).parent().unwrap()).unwrap();
            copy_file(&root().join(&path), &fixture.join(path)).unwrap();
        }
        copy_dir(
            &root().join("resources/fonts"),
            &fixture.join("resources/fonts"),
        )
        .unwrap();
        let notices = "Third-party notices for Newport\n";
        for os in [Os::Macos, Os::Windows] {
            let stage = output.join("native-package");
            fs::create_dir_all(stage.join("Newport.app/stale")).unwrap();
            fs::create_dir_all(stage.join("Newport/stale")).unwrap();
            let layout = stage_app(fixture, &output, &stage, os, None, notices).unwrap();
            assert!(!layout.app.join("stale").exists());
            assert_eq!(
                fs::read(&layout.executable).unwrap(),
                b"fixture, never executed"
            );
            if os == Os::Macos {
                assert_eq!(
                    layout.resources,
                    stage.join("Newport.app/Contents/Resources")
                );
                assert_eq!(
                    layout.executable,
                    stage.join("Newport.app/Contents/MacOS/newport")
                );
            } else {
                assert_eq!(layout.resources, stage.join("Newport"));
                assert_eq!(layout.executable, stage.join("Newport/newport.exe"));
            }
            for arch in ["x86_64", "aarch64"] {
                assert_eq!(
                    fs::read_to_string(
                        layout
                            .resources
                            .join(format!("agents/newport-agent-{arch}.gz"))
                    )
                    .unwrap(),
                    format!("compressed {arch}")
                );
            }
            assert_eq!(
                files(&layout.resources.join("Licenses"))
                    .into_iter()
                    .collect::<BTreeSet<_>>(),
                license_files()
                    .iter()
                    .map(|file| PathBuf::from(file.strip_prefix("Licenses/").unwrap()))
                    .collect()
            );
            assert_eq!(
                fs::read_to_string(layout.resources.join("Licenses").join(crate::notices::FILE))
                    .unwrap(),
                notices
            );
            for name in ICON_NOTICES {
                assert_eq!(
                    fs::read(layout.resources.join("Licenses/icons").join(name)).unwrap(),
                    fs::read(root().join("crates/newport-gpui/assets").join(name)).unwrap(),
                    "Icon notice changed: {name}"
                );
            }
            let payload = windows::payload_files().unwrap();
            for file in license_files().iter().chain(&["LICENSE".to_owned()]) {
                assert!(
                    payload.contains(file),
                    "Notice absent from Windows payload: {file}"
                );
            }
            assert_eq!(
                fs::read(layout.resources.join("LICENSE")).unwrap(),
                fs::read(root().join("LICENSE")).unwrap()
            );
            assert_eq!(
                files(&layout.resources.join("Fonts")),
                files(&root().join("resources/fonts"))
            );
            assert!(layout.resources.join("LICENSE-GPUI-KIT").is_file());
            assert!(layout.resources.join("grammars/catalog.json").is_file());
        }
    }

    #[test]
    fn every_icon_asset_notice_is_packaged() {
        let assets = root().join("crates/newport-gpui/assets");
        let actual: BTreeSet<_> = ["file-icons", "process-icons"]
            .into_iter()
            .flat_map(|dir| fs::read_dir(assets.join(dir)).unwrap())
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_none_or(|ext| ext != "svg"))
            .map(|path| {
                path.strip_prefix(&assets)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(
            actual,
            ICON_NOTICES.map(str::to_owned).into_iter().collect()
        );
    }

    #[cfg(unix)]
    #[test]
    fn icon_notices_reject_source_file_and_directory_symlinks_before_staging() {
        use std::os::unix::fs::symlink;

        for linked in ["file-icons/LICENSE-symbols", "file-icons", "process-icons"] {
            let directory = tempfile::tempdir().unwrap();
            let fixture = directory.path().join("source");
            let assets = fixture.join("crates/newport-gpui/assets");
            for name in ICON_NOTICES {
                let destination = assets.join(name);
                fs::create_dir_all(destination.parent().unwrap()).unwrap();
                copy_file(
                    &root().join("crates/newport-gpui/assets").join(name),
                    &destination,
                )
                .unwrap();
            }
            stage_icon_notices(&fixture, &directory.path().join("control")).unwrap();

            let source = assets.join(linked);
            let target = directory.path().join("outside-assets");
            fs::rename(&source, &target).unwrap();
            symlink(&target, &source).unwrap();
            let staged = directory.path().join("stage");
            assert!(stage_icon_notices(&fixture, &staged).is_err(), "{linked}");
            assert!(!staged.exists(), "Copied notices before rejecting {linked}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn staging_refuses_symlinked_sources() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        fs::create_dir(&source).unwrap();
        std::os::unix::fs::symlink("/etc/hosts", source.join("link")).unwrap();
        let error = copy_dir(&source, &directory.path().join("copy")).unwrap_err();
        assert!(error.to_string().contains("Unsupported"), "{error}");
    }
}
