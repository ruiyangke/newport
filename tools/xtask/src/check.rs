//! Formatting, Clippy and tests, and the everyday desktop build commands.
use crate::{BuildArgs, command, grammars, pdfium, util};
use anyhow::{Context, Result};
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

pub fn run(scope: Option<Scope>) -> Result<()> {
    match scope {
        Some(Scope::Rust) => rust(),
        Some(Scope::Agent) => standalone("tools/agent/Cargo.toml"),
        Some(Scope::Xtask) => standalone("tools/xtask/Cargo.toml"),
        None => {
            rust()?;
            standalone("tools/agent/Cargo.toml")?;
            standalone("tools/xtask/Cargo.toml")
        }
    }
}

/// Desktop workspace: formatting, then Clippy and tests with the signing tools enabled.
fn rust() -> Result<()> {
    util::run(command("cargo").args(["fmt", "--all", "--check"]))?;
    for action in ["clippy", "test"] {
        let mut cargo = command("cargo");
        cargo.args([
            action,
            "--locked",
            "--workspace",
            "--features",
            "newport-release/signing",
        ]);
        if action == "clippy" {
            cargo.args(["--all-targets", "--", "-D", "warnings"]);
        }
        util::run(util::without_release_secrets(&mut cargo))
            .with_context(|| format!("Desktop {action} failed"))?;
    }
    Ok(())
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

/// Linux agents when missing, grammars when stale, and PDFium for Windows targets.
fn resources(release: bool, target: Option<&str>) -> Result<()> {
    agents_if_changed()?;
    grammars::ensure(release, target)?;
    let target = util::build_target(target);
    if util::Platform::of(target.as_deref())?.os == util::Os::Windows {
        pdfium::stage(release, target.as_deref())?;
    }
    Ok(())
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
                && newest_source(&crate::root().join("tools/agent"))? > Some(built) =>
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

/// The newest modification among the agent's sources: its manifest, lockfile
/// and everything under `src`.
fn newest_source(agent: &std::path::Path) -> Result<Option<std::time::SystemTime>> {
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
    let mut newest = ["Cargo.toml", "Cargo.lock"]
        .iter()
        .filter_map(|name| modified(&agent.join(name)))
        .max();
    walk(&agent.join("src"), &mut newest)?;
    Ok(newest)
}

/// Stage resources, then `cargo run -p newport-app`.
/// Without `NEWPORT_DATA_DIR`, the app uses a disposable profile in the target directory.
/// With `remote_apps`, Remote apps are compiled in and the private crates are
/// out of the manifests again before the app starts.
pub fn dev(args: &[OsString], remote_apps: bool) -> Result<()> {
    clean_leftover()?;
    resources(false, None)?;
    if std::env::var_os("NEWPORT_DATA_DIR").is_none() {
        let profile = util::target_directory()?.join("dev-profile");
        eprintln!(
            "Development profile: {} (set NEWPORT_DATA_DIR to use another)",
            profile.display()
        );
        // The app reads it through `cargo run` or the development runner.
        std::env::set_var("NEWPORT_DATA_DIR", profile);
    }
    if remote_apps {
        let prepared = crate::remote_apps::prepare()?;
        let mut cargo = command("cargo");
        cargo.args(["build", "-p", "newport-app"]);
        util::run(util::without_release_secrets(prepared.apply(&mut cargo)))?;
        drop(prepared);
        // Launch as `cargo run` would, through the development runner.
        let binary = util::target_directory()?.join("debug/newport");
        return crate::dev_run::run(&binary, args);
    }
    let mut cargo = command("cargo");
    cargo.args(["run", "--locked", "-p", "newport-app"]);
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
    resources(false, None)?;
    let prepared = crate::remote_apps::prepare()?;
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
    resources(args.release, args.target.as_deref())?;
    let prepared = remote_apps.then(crate::remote_apps::prepare).transpose()?;
    let mut cargo = command("cargo");
    // Remote apps change the lockfile for the build, so it can't be locked.
    cargo.args(["build", "-p", "newport-app"]);
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
    util::run(util::without_release_secrets(&mut cargo))
}
