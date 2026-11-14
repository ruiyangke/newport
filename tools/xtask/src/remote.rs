use crate::{command, process, root};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
#[cfg(test)]
mod agent_resources_tests;
mod fixtures;
#[cfg(all(test, unix))]
mod tests;
pub(crate) fn docker() -> PathBuf {
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
/// The product version from `resources/app.json`, passed to the agent's
/// Docker builds as an input so a cached build with another version is never
/// reused.
pub(crate) fn agent_version() -> Result<String> {
    Ok(crate::util::app()?.version)
}

/// The Rust version rust-toolchain.toml pins, for the Docker builders.
pub(crate) fn rust_version() -> Result<String> {
    let toolchain = fs::read_to_string(root().join("rust-toolchain.toml"))?;
    toolchain
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("channel = \"")
                .and_then(|v| v.strip_suffix('"'))
        })
        .map(str::to_owned)
        .context("Missing pinned Rust version")
}
/// The directory, inside Docker's shared Cargo cache, that an agent build
/// compiles in. Cargo decides what to rebuild by modification time, so the
/// shared one can hand one checkout's build to another; a clean build empties
/// its own before and after.
fn agent_build(clean: bool) -> &'static str {
    if clean { "clean" } else { "shared" }
}

fn docker_build(stage: &str, arch: &str, output: &std::path::Path, build: &str) -> Result<()> {
    let version = rust_version()?;
    let agent_version = agent_version()?;
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
                "crates/newport-agent/Dockerfile",
                "--build-arg",
                &format!("RUST_VERSION={version}"),
                "--build-arg",
                &format!("NEWPORT_AGENT_VERSION={agent_version}"),
                "--build-arg",
                &format!("NEWPORT_AGENT_BUILD={build}"),
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
pub(crate) fn architectures(names: &[String]) -> Result<Vec<&'static str>> {
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
        docker_build("runtime-check", arch, temp.path(), agent_build(false))?;
    }
    Ok(())
}

/// Builds the Linux agents, or only the named architectures, into
/// resources/agents; binaries of other architectures are left as they are.
pub fn build_agents(names: &[String], clean: bool) -> Result<()> {
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
        docker_build("artifact", arch, staging.path(), agent_build(clean))?;
    }
    publish_agents(staging.path(), &output, &selected)
}

fn checked_core_agent(directory: &Path, arch: &str) -> Result<newport_protocol::product::Product> {
    use newport_protocol::product::{Identity, Variant};

    let name = format!("newport-agent-{arch}");
    let path = directory.join(&name);
    let identity_path = directory.join(format!("{name}.product.json"));
    for file in [&path, &identity_path] {
        ensure!(
            fs::symlink_metadata(file)
                .with_context(|| format!("Missing staged agent resource: {}", file.display()))?
                .file_type()
                .is_file(),
            "Staged agent resources must be regular files: {}",
            file.display()
        );
    }
    let binary = fs::read(&path)?;
    let machine: u16 = match arch {
        "x86_64" => 62,
        "aarch64" => 183,
        _ => anyhow::bail!("Unknown agent architecture: {arch}"),
    };
    ensure!(
        binary.len() >= 20
            && &binary[..6] == b"\x7fELF\x02\x01"
            && u16::from_le_bytes([binary[18], binary[19]]) == machine,
        "Staged agent is not a Linux ELF for {arch}"
    );
    let identity: Identity = crate::util::read_json(&identity_path)?;
    identity
        .validate()
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("Invalid staged agent product identity for {arch}"))?;
    ensure!(
        identity.product.variant == Variant::Core,
        "Core agent build returned a different product for {arch}"
    );
    ensure!(
        identity.build_sha256 == crate::util::sha256_hex(&binary),
        "Staged agent identity does not match its exact payload for {arch}"
    );
    Ok(identity.product)
}

fn publish_agents(staging: &Path, output: &Path, selected: &[&str]) -> Result<()> {
    let mut product = None;
    // Validate the entire requested set before replacing any accepted resource.
    for arch in selected {
        let next = checked_core_agent(staging, arch)?;
        if let Some(previous) = &product {
            ensure!(
                previous == &next,
                "Staged agent architectures contain different compiled product protocols"
            );
        }
        product = Some(next);
    }
    // A locally rebuilt payload must not inherit a signed release identity.
    match fs::remove_file(output.join("needle-release.json")) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    for arch in selected {
        let name = format!("newport-agent-{arch}");
        for file in [name.clone(), format!("{name}.product.json")] {
            fs::rename(staging.join(&file), output.join(&file))?;
        }
    }
    Ok(())
}

/// The label recording what a fixture image was built from.
const INPUTS_LABEL: &str = "newport.fixture.inputs";

/// Builds a fixture image, unless Docker already has one built from the same
/// inputs, such as an image CI restored from its cache.
fn fixture_image(docker: &Path, tag: &str, context: &str, args: &[String]) -> Result<()> {
    let inputs = fixture_inputs(&root().join(context), args)?;
    let built = process::text(command(docker).args([
        "image",
        "inspect",
        "--format",
        &format!("{{{{index .Config.Labels \"{INPUTS_LABEL}\"}}}}"),
        tag,
    ]));
    if built.is_ok_and(|built| built == inputs) {
        eprintln!("Using {tag}, already built from these inputs");
        return Ok(());
    }
    let mut build = command(docker);
    build.arg("build");
    for arg in args {
        build.args(["--build-arg", arg]);
    }
    build
        .args(["--label", &format!("{INPUTS_LABEL}={inputs}")])
        .args(["-t", tag, context]);
    process::run(&mut build)
}

/// A digest of the build arguments and every file the build context sends,
/// which excludes what tests/remote/.dockerignore lists.
fn fixture_inputs(context: &Path, args: &[String]) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    for arg in args {
        digest.update(arg.as_bytes());
        digest.update([0]);
    }
    let ignored = |entry: &walkdir::DirEntry| {
        entry.file_type().is_dir()
            && matches!(
                entry.file_name().to_str(),
                Some("target" | "__pycache__" | "node_modules")
            )
    };
    for entry in walkdir::WalkDir::new(context)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| !ignored(entry))
    {
        let entry = entry?;
        let relative = entry.path().strip_prefix(context)?;
        if entry.file_type().is_file() {
            digest.update(relative.to_string_lossy().as_bytes());
            digest.update([0]);
            digest.update(fs::read(entry.path())?);
            digest.update([0]);
        } else if entry.file_type().is_symlink() {
            digest.update(relative.to_string_lossy().as_bytes());
            digest.update([1]);
            digest.update(fs::read_link(entry.path())?.to_string_lossy().as_bytes());
            digest.update([0]);
        }
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
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
    let filter = filter.or_else(|| std::env::var("NEWPORT_TEST_FILTER").ok());
    if filter
        .as_deref()
        .is_some_and(crate::fixture_suites::requires_full_product)
    {
        crate::fixture_suites::full_product_resources()?;
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
    // Child processes inherit one local Docker context.
    // SAFETY: set before any harness worker thread starts, while no other thread
    // runs.
    unsafe {
        std::env::set_var("DOCKER_HOST", endpoint);
        std::env::set_var("DOCKER_CONTEXT", context);
        std::env::set_var("NEWPORT_TEST_DOCKER", &docker);
    }
    build_agents(&[], false)?;
    let toolchain = fs::read_to_string(root().join("rust-toolchain.toml"))?;
    let channel = toolchain
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("channel = \"")
                .and_then(|s| s.strip_suffix('"'))
        })
        .context("Missing Rust channel")?;
    fixture_image(
        &docker,
        "newport-test-remote:local",
        "tests/remote",
        &[format!("RUST_VERSION={channel}")],
    )?;
    fixture_image(
        &docker,
        "newport-test-systemd:local",
        "tests/remote/systemd",
        &[],
    )?;
    fixture_image(
        &docker,
        "newport-test-docker:local",
        "tests/remote/docker",
        &[],
    )?;
    let filters = filter.map(|filter| vec![filter]).unwrap_or_else(|| {
        crate::fixture_suites::LINUX
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    });
    let (executable, runs) = fixtures::prepare(&filters)?;
    if runs
        .iter()
        .any(|tests| crate::fixture_suites::requires_podman(tests))
    {
        fixture_image(
            &docker,
            "newport-test-podman:local",
            "tests/remote/podman",
            &[],
        )?;
    }
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
