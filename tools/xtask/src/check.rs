//! Formatting, Clippy and tests, and the everyday desktop build commands.
use crate::{command, ghostty, grammars, pdfium, util, BuildArgs};
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
    let env = ghostty::prepare()?;
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
        util::run(env.apply(util::without_release_secrets(&mut cargo)))
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
    let agents = crate::root().join("resources/agents");
    if ["x86_64", "aarch64"]
        .iter()
        .any(|arch| !agents.join(format!("newport-agent-{arch}")).is_file())
    {
        eprintln!("Linux agent binaries are missing; building them in Docker");
        crate::remote::build_agents()?;
    }
    grammars::ensure(release, target)?;
    let target = util::build_target(target);
    if util::Platform::of(target.as_deref())?.os == util::Os::Windows {
        pdfium::stage(release, target.as_deref())?;
    }
    Ok(())
}

/// Stage resources, then `cargo run -p newport-app` in the Ghostty environment.
/// Without `NEWPORT_DATA_DIR`, the app uses a disposable profile in the target directory.
pub fn dev(args: &[OsString]) -> Result<()> {
    resources(false, None)?;
    let env = ghostty::prepare()?;
    let mut cargo = command("cargo");
    cargo.args(["run", "--locked", "-p", "newport-app"]);
    if !args.is_empty() {
        cargo.arg("--").args(args);
    }
    if std::env::var_os("NEWPORT_DATA_DIR").is_none() {
        let profile = util::target_directory()?.join("dev-profile");
        eprintln!(
            "Development profile: {} (set NEWPORT_DATA_DIR to use another)",
            profile.display()
        );
        cargo.env("NEWPORT_DATA_DIR", profile);
    }
    let status = env
        .apply(util::without_release_secrets(&mut cargo))
        .status()
        .context("Cannot start cargo")?;
    std::process::exit(status.code().unwrap_or(1))
}

/// Stage resources, then `cargo build -p newport-app` in the Ghostty environment.
pub fn build(args: &BuildArgs) -> Result<()> {
    resources(args.release, args.target.as_deref())?;
    let env = ghostty::prepare()?;
    let mut cargo = command("cargo");
    cargo.args(["build", "--locked", "-p", "newport-app"]);
    if args.release {
        cargo.arg("--release");
    }
    if let Some(target) = &args.target {
        cargo.args(["--target", target]);
    }
    util::run(env.apply(util::without_release_secrets(&mut cargo)))
}
