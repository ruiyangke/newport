//! Stage, verify and archive the desktop app.
use crate::util::{self, Os, Platform};
use crate::{build_info, command, ghostty, grammars, root};
use anyhow::{bail, ensure, Context, Result};
use clap::Args;
use serde::Deserialize;
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

mod macos;
pub mod windows;

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

/// `cargo build --locked [--release] -p newport-app [--target <triple>]` in the Ghostty
/// environment, without release secrets.
fn cargo_build(release: bool, target: Option<&str>, env: &ghostty::BuildEnv) -> Command {
    let mut c = command("cargo");
    c.args(["build", "--locked"]);
    if release {
        c.arg("--release");
    }
    c.args(["-p", "newport-app"]);
    if let Some(target) = target {
        c.args(["--target", target]);
    }
    env.apply(util::without_release_secrets(&mut c));
    c
}

/// Prepare Ghostty, build the grammars, then build the app; nothing with `skip_build`.
fn build(
    skip_build: bool,
    release: bool,
    target: Option<&str>,
    prepare: impl FnOnce() -> Result<ghostty::BuildEnv>,
    grammars: impl FnOnce() -> Result<()>,
    run: impl FnOnce(&mut Command) -> Result<()>,
) -> Result<()> {
    if skip_build {
        return Ok(());
    }
    // Pinned compiler and source exist before Cargo invokes Ghostty's build script.
    let env = prepare()?;
    grammars()?;
    run(&mut cargo_build(release, target, &env))
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
/// from the repository `root` and the profile directory `output`.
fn stage_app(root: &Path, output: &Path, stage: &Path, os: Os) -> Result<Layout> {
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
    copy_file(
        &root.join("crates/newport-grammars/LICENSE-GPUI-KIT"),
        &resources.join("LICENSE-GPUI-KIT"),
    )?;
    copy_dir(
        &root.join("resources/licenses"),
        &resources.join("Licenses"),
    )?;
    Ok(Layout {
        app,
        resources,
        executable,
    })
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
    build(
        options.skip_build,
        release,
        target,
        ghostty::prepare,
        || grammars::build(release, target).map(drop),
        util::run,
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
    let layout = stage_app(&root, &output, &stage, platform.os)?;
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
        },
        product: &product,
        nsis: options.nsis,
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

    fn ghostty_env() -> ghostty::BuildEnv {
        ghostty::BuildEnv {
            vars: vec![
                ("GHOSTTY_SOURCE_DIR".into(), "prepared-ghostty".into()),
                ("LIBGHOSTTY_VT_SYS_OPTIMIZE".into(), "ReleaseSafe".into()),
            ],
        }
    }

    #[test]
    fn building_bootstraps_ghostty_then_grammars_then_the_app_without_secrets() {
        let steps = RefCell::new(Vec::new());
        build(
            false,
            false,
            None,
            || {
                steps.borrow_mut().push("bootstrap".to_owned());
                Ok(ghostty_env())
            },
            || {
                steps.borrow_mut().push("grammars".to_owned());
                Ok(())
            },
            |c| {
                steps.borrow_mut().push("cargo".to_owned());
                assert_eq!(c.get_program(), "cargo");
                let args: Vec<_> = c.get_args().collect();
                assert_eq!(args, ["build", "--locked", "-p", "newport-app"]);
                let envs: HashMap<_, _> = c.get_envs().collect();
                assert_eq!(
                    envs[OsStr::new("GHOSTTY_SOURCE_DIR")],
                    Some(OsStr::new("prepared-ghostty"))
                );
                assert_eq!(
                    envs[OsStr::new("LIBGHOSTTY_VT_SYS_OPTIMIZE")],
                    Some(OsStr::new("ReleaseSafe"))
                );
                for name in util::RELEASE_SECRETS {
                    assert_eq!(envs.get(OsStr::new(name)), Some(&None), "{name}");
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(*steps.borrow(), ["bootstrap", "grammars", "cargo"]);

        let c = cargo_build(true, Some("aarch64-apple-darwin"), &ghostty_env());
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
    fn prebuilt_packaging_skips_the_compiler_and_source_bootstrap() {
        build(
            true,
            true,
            None,
            || panic!("must not bootstrap"),
            || panic!("must not build grammars"),
            |_| panic!("must not build"),
        )
        .unwrap();
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
    fn staging_retains_agents_and_terminal_license_notices_byte_for_byte() {
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
        copy_dir(
            &root().join("resources/fonts"),
            &fixture.join("resources/fonts"),
        )
        .unwrap();
        copy_dir(
            &root().join("resources/licenses"),
            &fixture.join("resources/licenses"),
        )
        .unwrap();
        let original = root().join("resources/licenses/terminal");
        let notices = files(&original);
        for name in [
            "LICENSE-Ghostty",
            "LICENSE-libghostty-rs",
            "uucode/licenses/LICENSE_unicode",
        ] {
            assert!(notices.contains(&PathBuf::from(name)), "{name}");
        }
        for os in [Os::Macos, Os::Windows] {
            let stage = output.join("native-package");
            fs::create_dir_all(stage.join("Newport.app/stale")).unwrap();
            fs::create_dir_all(stage.join("Newport/stale")).unwrap();
            let layout = stage_app(fixture, &output, &stage, os).unwrap();
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
            let staged = layout.resources.join("Licenses/terminal");
            assert_eq!(files(&staged), notices);
            for file in &notices {
                assert_eq!(
                    fs::read(staged.join(file)).unwrap(),
                    fs::read(original.join(file)).unwrap(),
                    "{}",
                    file.display()
                );
            }
            assert_eq!(
                files(&layout.resources.join("Fonts")),
                files(&root().join("resources/fonts"))
            );
            assert!(layout.resources.join("LICENSE-GPUI-KIT").is_file());
            assert!(layout.resources.join("grammars/catalog.json").is_file());
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
