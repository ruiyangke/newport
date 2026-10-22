use crate::{command, process, root};
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
mod fixtures;
fn docker() -> PathBuf {
    if let Some(path) = std::env::var_os("NEWPORT_TEST_DOCKER") {
        return path.into();
    }
    if command("docker")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
    {
        return "docker".into();
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    [
        home.join(".orbstack/bin/docker"),
        PathBuf::from("/Applications/OrbStack.app/Contents/MacOS/xbin/docker"),
        PathBuf::from("/Applications/Docker.app/Contents/Resources/bin/docker"),
    ]
    .into_iter()
    .find(|p| p.exists())
    .unwrap_or_else(|| "docker".into())
}
fn docker_build(stage: &str, arch: &str, output: &std::path::Path) -> Result<()> {
    let toolchain = fs::read_to_string(root().join("rust-toolchain.toml"))?;
    let version = toolchain
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("channel = \"")
                .and_then(|v| v.strip_suffix('"'))
        })
        .context("Missing pinned Rust version")?;
    let platform = if arch == "x86_64" {
        "linux/amd64"
    } else {
        "linux/arm64"
    };
    // A cold optimized ARM build under QEMU can exceed the generic command
    // budget on hosted runners. Keep the longer limit scoped to image builds.
    process::Process::spawn(
        command(docker())
            .args([
                "buildx",
                "build",
                "--platform",
                platform,
                "--file",
                "tools/agent/Dockerfile",
                "--build-arg",
                &format!("RUST_VERSION={version}"),
                "--target",
                stage,
                "--output",
            ])
            .arg(format!("type=local,dest={}", output.display()))
            .arg("."),
    )?
    .success(Duration::from_secs(2400))
    .with_context(|| format!("Docker agent {stage} build failed for {platform}"))
}
/// The Linux agent architectures, or the named subset of them.
fn architectures(names: &[String]) -> Result<Vec<&'static str>> {
    const ALL: [&str; 2] = ["x86_64", "aarch64"];
    if names.is_empty() {
        return Ok(ALL.to_vec());
    }
    names
        .iter()
        .map(|name| {
            ALL.into_iter()
                .find(|arch| arch == name)
                .with_context(|| format!("Unknown architecture {name}; use x86_64 or aarch64"))
        })
        .collect()
}

pub fn runtime(names: &[String]) -> Result<()> {
    let temp = tempfile::tempdir()?;
    for arch in architectures(names)? {
        docker_build("runtime-check", arch, temp.path())?;
    }
    Ok(())
}

/// Builds the Linux agents, or only the named architectures, into
/// resources/agents; binaries of other architectures are left as they are.
pub fn build_agents(names: &[String]) -> Result<()> {
    let selected = architectures(names)?;
    if std::env::var("NEWPORT_PREBUILT").as_deref() == Ok("1") {
        return crate::artifact::verify(Path::new("resources/agents"));
    }
    let output = root().join("resources/agents");
    if cfg!(windows) {
        // Linux binaries are built in the Linux CI job, never replaced with stubs.
        for arch in ["x86_64", "aarch64"] {
            ensure!(
                output.join(format!("newport-agent-{arch}")).exists(),
                "Linux agent binaries are missing. Copy the linux-agents CI artifact into resources/agents before building on Windows."
            );
        }
        return Ok(());
    }
    fs::create_dir_all(&output)?;
    // Build every selected one successfully before replacing the bundled binaries.
    let staging = tempfile::tempdir_in(&output)?;
    for arch in &selected {
        docker_build("artifact", arch, staging.path())?;
    }
    // A locally rebuilt payload must not inherit a signed release identity.
    match fs::remove_file(output.join("needle-release.json")) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    for arch in &selected {
        let name = format!("newport-agent-{arch}");
        fs::rename(staging.path().join(&name), output.join(&name))?;
    }
    Ok(())
}

pub fn run(args: &[String]) -> Result<()> {
    let mut filter = None;
    let mut i = 0;
    while i < args.len() {
        ensure!(
            args[i] == "--filter" && i + 1 < args.len(),
            "Usage: cargo xtask test remote [--filter TEST]"
        );
        filter = Some(args[i + 1].clone());
        i += 2;
    }
    let docker = docker();
    let context = match std::env::var("NEWPORT_TEST_DOCKER_CONTEXT") {
        Ok(context) => context,
        Err(_) => process::text(command(&docker).args(["context", "show"]))?,
    };
    let config: Value = serde_json::from_str(&process::text(
        command(&docker).args(["context", "inspect", &context]),
    )?)?;
    let endpoint = config[0]["Endpoints"]["docker"]["Host"]
        .as_str()
        .context("Docker context has no endpoint")?;
    ensure!(
        endpoint.starts_with("unix://"),
        "Tests require a local Docker socket"
    );
    // Set before starting any harness worker threads. Child processes inherit one local Docker context.
    std::env::set_var("DOCKER_HOST", endpoint);
    std::env::set_var("DOCKER_CONTEXT", context);
    std::env::set_var("NEWPORT_TEST_DOCKER", &docker);
    build_agents(&[])?;
    let toolchain = fs::read_to_string(root().join("rust-toolchain.toml"))?;
    let channel = toolchain
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("channel = \"")
                .and_then(|s| s.strip_suffix('"'))
        })
        .context("Missing Rust channel")?;
    process::run(command(&docker).args([
        "build",
        "--build-arg",
        &format!("RUST_VERSION={channel}"),
        "-t",
        "newport-test-remote:local",
        "tests/remote",
    ]))?;
    process::run(command(&docker).args([
        "build",
        "-t",
        "newport-test-systemd:local",
        "tests/remote/systemd",
    ]))?;
    process::run(command(docker).args([
        "build",
        "-t",
        "newport-test-docker:local",
        "tests/remote/docker",
    ]))?;
    let filters = filter
        .or_else(|| std::env::var("NEWPORT_TEST_FILTER").ok())
        .map(|filter| vec![filter])
        .unwrap_or_else(|| {
            crate::fixture_suites::LINUX
                .iter()
                .map(|s| (*s).to_owned())
                .collect()
        });
    let (executable, runs) = fixtures::prepare(&filters)?;
    let runtime = tokio::runtime::Runtime::new()?;
    let mut failed = 0;
    for tests in runs {
        if let Err(error) = runtime.block_on(fixtures::run(&executable, &tests)) {
            eprintln!("Remote fixture failed: {error:#}");
            failed += 1;
        }
    }
    ensure!(failed == 0, "{failed} remote fixture runs failed");
    Ok(())
}
