//! Formatting, Clippy and tests, and the everyday desktop build commands.
use crate::{BuildArgs, command, grammars, pdfium, util};
use anyhow::{Context, Result, ensure};
use clap::ValueEnum;
use std::ffi::OsString;

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum Scope {
    /// Desktop crates.
    Rust,
    /// The Linux agent.
    Agent,
    /// This tool.
    Xtask,
}

pub fn fmt(args: &[OsString]) -> Result<()> {
    let mut cargo = command("cargo");
    cargo.arg("fmt").args(args);
    util::run(util::without_release_secrets(&mut cargo))
}

pub fn test_rust(args: &[OsString]) -> Result<()> {
    let mut cargo = command("cargo");
    cargo.arg("test").args(args);
    util::run(util::without_release_secrets(&mut cargo))
}

pub fn run(scope: Option<Scope>, packages: &[String]) -> Result<()> {
    ensure!(
        packages.is_empty() || matches!(scope, Some(Scope::Rust)),
        "--package requires the rust check scope"
    );
    match scope {
        Some(Scope::Rust) => rust(packages),
        Some(Scope::Agent) => standalone("crates/newport-agent/Cargo.toml"),
        Some(Scope::Xtask) => standalone("tools/xtask/Cargo.toml"),
        None => {
            rust(&[])?;
            standalone("crates/newport-agent/Cargo.toml")?;
            standalone("tools/xtask/Cargo.toml")
        }
    }
}

/// Desktop workspace: formatting, then Clippy and tests with the signing tools enabled.
fn rust(packages: &[String]) -> Result<()> {
    util::run(command("cargo").args(["fmt", "--all", "--check"]))?;
    protocol_contracts()?;
    for action in ["clippy", "test"] {
        let mut cargo = command("cargo");
        cargo.args([action, "--locked"]);
        if packages.is_empty() {
            cargo.arg("--workspace");
        } else {
            for package in packages {
                cargo.args(["--package", package]);
            }
        }
        if packages.is_empty() || packages.iter().any(|package| package == "newport-release") {
            cargo.args(["--features", "newport-release/signing"]);
        }
        if action == "clippy" {
            cargo.args(["--all-targets", "--", "-D", "warnings"]);
        }
        util::run(util::without_release_secrets(&mut cargo))
            .with_context(|| format!("Desktop {action} failed"))?;
    }
    Ok(())
}

/// An isolated consumer keeps workspace feature unification from hiding a codec dependency.
fn protocol_contracts() -> Result<()> {
    let root = crate::root();
    let scratch = tempfile::tempdir()?;
    let package = scratch.path();
    std::fs::create_dir(package.join("src"))?;
    let dependency = toml::Value::String(
        root.join("crates/newport-protocol")
            .to_string_lossy()
            .into_owned(),
    );
    std::fs::write(
        package.join("Cargo.toml"),
        format!(
            "[package]\nname = \"newport-contract-consumer\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n[dependencies]\nnewport-protocol = {{ path = {dependency}, default-features = false }}\n"
        ),
    )?;
    std::fs::write(
        package.join("src/lib.rs"),
        include_str!("../fixtures/protocol-contracts.rs"),
    )?;
    std::fs::copy(root.join("Cargo.lock"), package.join("Cargo.lock"))?;
    let manifest = package.join("Cargo.toml");
    let graph = crate::process::text(
        command("cargo")
            .args([
                "tree",
                "--offline",
                "--edges",
                "normal,build",
                "--prefix",
                "none",
                "--format",
                "{p}",
                "--manifest-path",
            ])
            .arg(&manifest),
    )?;
    for name in graph
        .lines()
        .filter_map(|line| line.split_whitespace().next())
    {
        anyhow::ensure!(
            !name.starts_with("prost")
                && !name.starts_with("protox")
                && name != "heck"
                && name != "newport-domain"
                && !name.starts_with("newport-remote"),
            "Protocol contracts unexpectedly depend on {name}",
        );
    }
    util::run(
        command("cargo")
            .args(["test", "--offline", "--locked", "--manifest-path"])
            .arg(manifest)
            .arg("--target-dir")
            .arg(root.join("target/protocol-contracts")),
    )
}

/// A crate outside the desktop workspace: formatting, Clippy and tests.
fn standalone(manifest: &str) -> Result<()> {
    util::run(command("cargo").args(["fmt", "--manifest-path", manifest, "--check"]))?;
    util::run(command("cargo").args([
        "clippy",
        "--manifest-path",
        manifest,
        "--locked",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ]))?;
    util::run(command("cargo").args(["test", "--manifest-path", manifest, "--locked"]))
}

/// Prepare the selected agent composition, grammars and platform resources.
fn resources(
    release: bool,
    target: Option<&str>,
    remote_apps: bool,
) -> Result<Option<crate::remote_apps::Prepared>> {
    prepare_resources(
        remote_apps,
        agents_if_changed,
        || {
            grammars::ensure(release, target)?;
            let target = util::build_target(target);
            if util::Platform::of(target.as_deref())?.os == util::Os::Windows {
                pdfium::stage(release, target.as_deref())?;
            }
            Ok(())
        },
        crate::remote_apps::prepare,
    )
}

fn prepare_resources<T>(
    remote_apps: bool,
    core: impl FnOnce() -> Result<()>,
    platform: impl FnOnce() -> Result<()>,
    full: impl FnOnce() -> Result<T>,
) -> Result<Option<T>> {
    if !remote_apps {
        core()?;
    }
    // Grammar builds use the public locked graph, before full composition
    // temporarily adds private dependencies to the workspace.
    platform()?;
    if remote_apps {
        full().map(Some)
    } else {
        Ok(None)
    }
}

fn development_binary(
    mut directory: std::path::PathBuf,
    target: Option<&str>,
) -> Result<std::path::PathBuf> {
    let platform = util::Platform::of(target)?;
    if let Some(target) = target {
        directory.push(target);
    }
    Ok(directory
        .join("debug")
        .join(if platform.os == util::Os::Windows {
            "newport.exe"
        } else {
            "newport"
        }))
}

/// Builds the Linux agents when they are missing or older than their sources.
pub fn agents_if_changed() -> Result<()> {
    let agents = crate::root().join("resources/agents");
    let built = ["x86_64", "aarch64"]
        .iter()
        .map(|arch| modified(&agents.join(format!("newport-agent-{arch}"))))
        .min()
        .flatten();
    match built {
        None => {
            eprintln!("Linux agent binaries are missing; building them in Docker");
            crate::remote::build_agents(&[])?;
        }
        // Prebuilt agents (CI artifacts) are verified, not rebuilt.
        Some(built)
            if std::env::var("NEWPORT_PREBUILT").as_deref() != Ok("1")
                && newest_source(&crate::root())? > Some(built) =>
        {
            eprintln!("Linux agent sources changed; rebuilding the agents in Docker");
            crate::remote::build_agents(&[])?;
        }
        Some(_) => {}
    }
    Ok(())
}

fn modified(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The newest modification among the agent's sources: its lockfile, and the
/// manifest and everything under `src` of the agent and each crate it uses by
/// path, as its Dockerfile copies them.
fn newest_source(root: &std::path::Path) -> Result<Option<std::time::SystemTime>> {
    fn walk(path: &std::path::Path, newest: &mut Option<std::time::SystemTime>) -> Result<()> {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                walk(&entry.path(), newest)?;
            } else {
                *newest = (*newest).max(modified(&entry.path()));
            }
        }
        Ok(())
    }
    let mut newest = modified(&root.join("crates/newport-agent/Cargo.lock"));
    for package in [
        "crates/newport-agent",
        "crates/newport-terminal",
        "crates/newport-domain",
        "crates/newport-protocol",
    ] {
        let package = root.join(package);
        newest = newest.max(modified(&package.join("Cargo.toml")));
        walk(&package.join("src"), &mut newest)?;
        newest = newest.max(modified(&package.join("build.rs")));
        newest = newest.max(modified(&package.join("codegen.rs")));
        if package.join("proto").is_dir() {
            walk(&package.join("proto"), &mut newest)?;
        }
    }
    Ok(newest)
}

/// Stage resources, then `cargo run -p newport-app`.
/// Without `NEWPORT_DATA_DIR`, the app uses a disposable profile in the target directory.
/// With `remote_apps`, Remote apps are compiled in and the private crates are
/// out of the manifests again before the app starts.
pub fn dev(args: &[OsString], remote_apps: bool) -> Result<()> {
    clean_leftover()?;
    let prepared = resources(false, None, remote_apps)?;
    if std::env::var_os("NEWPORT_DATA_DIR").is_none() {
        let profile = util::target_directory()?.join("dev-profile");
        eprintln!(
            "Development profile: {} (set NEWPORT_DATA_DIR to use another)",
            profile.display()
        );
        // The app reads it through `cargo run` or the development runner.
        // SAFETY: the only other thread, the interrupt handler, never reads the
        // environment.
        unsafe { std::env::set_var("NEWPORT_DATA_DIR", profile) };
    }
    if let Some(prepared) = prepared {
        let mut cargo = command("cargo");
        cargo.args(["build", "-p", "newport-app"]);
        util::with_build_stamp(&mut cargo);
        util::run(util::without_release_secrets(prepared.apply(&mut cargo)))?;
        drop(prepared);
        // Bundling runs in this process, not under the cargo command above.
        if cfg!(target_os = "macos") {
            let stub = crate::remote_apps::package_stub(None)?;
            // SAFETY: the only other thread, the interrupt handler, never reads
            // the environment.
            unsafe { std::env::set_var(crate::remote_apps::STUB_VARIABLE, stub) };
        }
        // Launch as `cargo run` would, through the development runner.
        let binary = development_binary(
            util::target_directory()?,
            util::build_target(None).as_deref(),
        )?;
        return crate::dev_run::run(&binary, args);
    }
    let mut cargo = command("cargo");
    cargo.args(["run", "--locked", "-p", "newport-app"]);
    util::with_build_stamp(&mut cargo);
    if !args.is_empty() {
        cargo.arg("--").args(args);
    }
    let status = util::without_release_secrets(&mut cargo)
        .status()
        .context("Cannot start cargo")?;
    std::process::exit(status.code().unwrap_or(1))
}

/// A Remote apps build that was killed outright leaves the private crates in
/// the manifests; every build takes them out first.
pub(crate) fn clean_leftover() -> Result<()> {
    if crate::remote_apps::leftover()? {
        eprintln!("Taking the private crates a killed Remote apps build left out of the manifests");
        crate::remote_apps::clean()?;
    }
    Ok(())
}

/// UI and application composition tests with Remote apps compiled in.
pub fn test_remote_apps(args: &[String]) -> Result<()> {
    clean_leftover()?;
    let prepared = resources(false, None, true)?.context("Missing Remote apps composition")?;
    let mut cargo = command("cargo");
    cargo.args(["test", "-p", "newport-gpui", "-p", "newport-app"]);
    // Cargo configuration must precede a caller's test-harness `--` separator.
    prepared.apply(&mut cargo);
    cargo.args(args);
    util::run(util::without_release_secrets(&mut cargo))
}

/// Stage resources, then `cargo build -p newport-app`.
pub fn build(args: &BuildArgs, remote_apps: bool) -> Result<()> {
    clean_leftover()?;
    let prepared = resources(args.release, args.target.as_deref(), remote_apps)?;
    let mut cargo = command("cargo");
    // Remote apps change the lockfile for the build, so it can't be locked.
    cargo.args(["build", "-p", "newport-app"]);
    util::with_build_stamp(&mut cargo);
    match &prepared {
        Some(prepared) => {
            prepared.apply(&mut cargo);
        }
        None => {
            cargo.arg("--locked");
        }
    }
    if args.release {
        cargo.arg("--release");
    }
    if let Some(target) = &args.target {
        cargo.args(["--target", target]);
    }
    util::run(util::without_release_secrets(&mut cargo))?;
    // A Mac build's bundle copies the stub, so it is built from this checkout
    // too, never left from an older one.
    let mac = args
        .target
        .as_deref()
        .map_or(cfg!(target_os = "macos"), |target| {
            target.contains("apple-darwin")
        });
    if prepared.is_some() && mac {
        crate::remote_apps::package_stub(args.target.as_deref())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_resources_do_not_require_or_fall_back_to_core_agents() {
        let prepared = prepare_resources(
            true,
            || panic!("full builds must not invoke the core Docker builder"),
            || Ok(()),
            || Ok("verified full export"),
        )
        .unwrap();
        assert_eq!(prepared, Some("verified full export"));
        let failed = prepare_resources::<()>(
            true,
            || panic!("a rejected full export must not fall back to core"),
            || Ok(()),
            || anyhow::bail!("full export mismatch"),
        );
        assert_eq!(failed.unwrap_err().to_string(), "full export mismatch");
        assert!(
            prepare_resources::<()>(
                false,
                || Ok(()),
                || Ok(()),
                || panic!("core builds must remain independent of private sources"),
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn locked_platform_builds_precede_temporary_private_dependencies() {
        let order = std::cell::RefCell::new(Vec::new());
        prepare_resources(
            true,
            || panic!("unexpected core build"),
            || {
                order.borrow_mut().push("locked grammars");
                Ok(())
            },
            || {
                order.borrow_mut().push("private dependencies");
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(*order.borrow(), ["locked grammars", "private dependencies"]);
        assert!(
            prepare_resources::<()>(
                true,
                || panic!("unexpected core build"),
                || anyhow::bail!("grammar build failed"),
                || panic!("failed resource build must not mutate manifests"),
            )
            .is_err()
        );
    }

    #[test]
    fn development_binary_uses_the_target_directory_and_native_suffix() {
        for (target, name) in [
            ("aarch64-pc-windows-msvc", "newport.exe"),
            ("x86_64-pc-windows-msvc", "newport.exe"),
            ("aarch64-apple-darwin", "newport"),
        ] {
            let directory = std::path::PathBuf::from("output");
            assert_eq!(
                development_binary(directory.clone(), Some(target)).unwrap(),
                directory.join(target).join("debug").join(name),
            );
        }
        assert_eq!(
            development_binary("output".into(), None).unwrap(),
            std::path::Path::new("output")
                .join("debug")
                .join(if cfg!(windows) {
                    "newport.exe"
                } else {
                    "newport"
                }),
        );
    }
}
