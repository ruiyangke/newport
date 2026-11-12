use super::{distro::Architecture, fixture, supervisor::Supervisor};
use crate::{remote, root, util};
use anyhow::{Context, Result, ensure};
use clap::Args;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    path::Path,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Args)]
pub struct Options {
    #[arg(long, value_enum)]
    arch: Architecture,
    /// A fresh directory for canonical executables, build logs and provenance.
    #[arg(long)]
    output: PathBuf,
    #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u64).range(4..=1024))]
    reserve_gib: u64,
}

fn sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let size = file.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        hash.update(&buffer[..size]);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn source_hashes() -> Result<BTreeMap<String, String>> {
    let mut files = BTreeMap::new();
    for path in [
        "rust-toolchain.toml",
        "resources/app.json",
        "crates/newport-agent/Dockerfile",
        "crates/newport-agent/Dockerfile.dockerignore",
        "crates/newport-agent/Cargo.toml",
        "crates/newport-agent/Cargo.lock",
        "crates/newport-agent/build.rs",
        "crates/newport-terminal/Cargo.toml",
        "crates/newport-domain/Cargo.toml",
        "crates/newport-protocol/Cargo.toml",
    ] {
        files.insert(path.into(), sha256(&root().join(path))?);
    }
    for entry in fs::read_dir(root().join("crates/newport-protocol"))? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "rs") {
            files.insert(
                path.strip_prefix(root())?.to_string_lossy().into_owned(),
                sha256(&path)?,
            );
        }
    }
    for directory in [
        "crates/newport-agent/src",
        "crates/newport-agent/tests",
        "crates/newport-agent/benches",
        "tests/agent-runtime",
        "crates/newport-terminal/src",
        "crates/newport-domain/src",
        "crates/newport-protocol/src",
        "crates/newport-protocol/proto",
        "crates/newport-protocol/tests",
    ] {
        for entry in walkdir::WalkDir::new(root().join(directory)) {
            let entry = entry?;
            ensure!(
                !entry.file_type().is_symlink(),
                "Discovery build sources must not contain symlinks"
            );
            if entry.file_type().is_file() {
                files.insert(
                    entry
                        .path()
                        .strip_prefix(root())?
                        .to_string_lossy()
                        .into_owned(),
                    sha256(entry.path())?,
                );
            }
        }
    }
    Ok(files)
}

#[derive(Serialize)]
struct BuildReceipt {
    schema: u8,
    checkout: String,
    architecture: Architecture,
    source_sha256: BTreeMap<String, String>,
    artifact_sha256: BTreeMap<String, String>,
    status: &'static str,
    failure: Option<String>,
    minimum_free_bytes: u64,
    finished_utc: String,
    steps: Vec<super::supervisor::Step>,
}

pub fn run(options: Options) -> Result<()> {
    ensure!(
        cfg!(unix),
        "Discovery builds currently require a macOS or Linux host"
    );
    let output = std::path::absolute(&options.output)?;
    ensure!(
        !output.to_string_lossy().contains(','),
        "Docker export paths cannot contain commas"
    );
    let source_sha256 = source_hashes()?;
    let interrupted = Arc::new(AtomicBool::new(false));
    let signal = interrupted.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::SeqCst))?;
    let mut runner = Supervisor::new(
        output.clone(),
        options.reserve_gib * 1024 * 1024 * 1024,
        interrupted,
    )?;
    let mut artifact_sha256 = BTreeMap::new();
    let result = (|| {
        runner.run(
            fixture::docker().args([
                "buildx",
                "build",
                "--platform",
                options.arch.platform(),
                "--file",
                "crates/newport-agent/Dockerfile",
                "--build-arg",
                &format!("RUST_VERSION={}", remote::rust_version()?),
                "--build-arg",
                &format!("NEWPORT_AGENT_VERSION={}", remote::agent_version()?),
                "--target",
                "discovery-artifact",
                "--output",
                &format!("type=local,dest={}", output.join("artifacts").display()),
                ".",
            ]),
            "build",
            Duration::from_secs(2400),
        )?;
        ensure!(
            source_hashes()? == source_sha256,
            "Sources changed during the build; rebuild from a stable tree"
        );
        for name in [
            "agent",
            "unit-tests",
            "helper-tests",
            "monitor-tests",
            "systemd-tests",
        ] {
            let path = output.join("artifacts").join(name);
            ensure!(
                fs::symlink_metadata(&path)?.file_type().is_file(),
                "Missing regular {name} artifact"
            );
            let mut header = [0; 20];
            File::open(&path)?.read_exact(&mut header)?;
            super::validate_elf(&header, options.arch)?;
            artifact_sha256.insert(name.into(), sha256(&path)?);
        }
        Ok(())
    })();
    let receipt = BuildReceipt {
        schema: 1,
        checkout: util::git_head()?,
        architecture: options.arch,
        source_sha256,
        artifact_sha256,
        status: if result.is_ok() { "passed" } else { "failed" },
        failure: result
            .as_ref()
            .err()
            .map(|error: &anyhow::Error| format!("{error:#}")),
        minimum_free_bytes: runner.minimum_free_bytes,
        finished_utc: chrono::Utc::now().to_rfc3339(),
        steps: runner.steps,
    };
    serde_json::to_writer_pretty(
        File::create_new(output.join("build-receipt.json"))?,
        &receipt,
    )
    .context("Writing discovery build provenance")?;
    eprintln!(
        "Discovery build {}; evidence {}",
        receipt.status,
        output.display()
    );
    result
}
