use crate::{command, process, root};
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::{fs, path::PathBuf};
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
    process::run(
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
    )
    .context("Docker agent build failed; Docker must support both amd64 and arm64")
}
pub fn runtime() -> Result<()> {
    let temp = tempfile::tempdir()?;
    for arch in ["x86_64", "aarch64"] {
        docker_build("runtime-check", arch, temp.path())?;
    }
    Ok(())
}
pub fn build_agents() -> Result<()> {
    if std::env::var("NEWPORT_PREBUILT").as_deref() == Ok("1") {
        use sha2::{Digest, Sha256};
        let dir = root().join("src-tauri/agents");
        let expected: Value = serde_json::from_slice(&fs::read(dir.join("newport-build.json"))?)?;
        ensure!(
            expected["commit"] == process::text(command("git").args(["rev-parse", "HEAD"]))?,
            "Prebuilt agent commit mismatch"
        );
        let mut actual = serde_json::Map::new();
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            if entry.file_name() == "newport-build.json" {
                continue;
            }
            ensure!(
                entry.file_type()?.is_file(),
                "Unexpected agent artifact entry"
            );
            let hash = Sha256::digest(fs::read(entry.path())?)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            actual.insert(
                entry
                    .file_name()
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("Invalid filename"))?,
                Value::String(hash),
            );
        }
        ensure!(
            !actual.is_empty() && expected["files"] == Value::Object(actual),
            "Prebuilt agent contents mismatch"
        );
        return Ok(());
    }
    let output = root().join("src-tauri/agents");
    fs::create_dir_all(&output)?;
    // Build both successfully before replacing the bundled binaries.
    let staging = tempfile::tempdir_in(&output)?;
    for arch in ["x86_64", "aarch64"] {
        docker_build("artifact", arch, staging.path())?;
    }
    for arch in ["x86_64", "aarch64"] {
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
            "Usage: remote [--filter TEST]"
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
    build_agents()?;
    let toolchain = fs::read_to_string(root().join("rust-toolchain.toml"))?;
    let channel = toolchain
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("channel = \"")
                .and_then(|s| s.strip_suffix('"'))
        })
        .context("Missing Rust channel")?;
    process::run(command(docker).args([
        "build",
        "--build-arg",
        &format!("RUST_VERSION={channel}"),
        "-t",
        "newport-test-remote:local",
        "tests/remote",
    ]))?;
    let mut run = command("cargo");
    run.args([
        "run",
        "--locked",
        "--manifest-path",
        "src-tauri/Cargo.toml",
        "--example",
        "remote_tests",
    ]);
    if let Some(filter) = filter {
        run.env("NEWPORT_TEST_FILTER", filter);
    }
    process::run(&mut run)
}
