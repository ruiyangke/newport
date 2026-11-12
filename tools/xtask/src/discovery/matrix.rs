//! Serial, disposable Linux userspace qualification. Native kernels and desktop
//! interactions need their own acceptance fixtures.
use crate::{command, process, util};
use anyhow::{Context, Result, ensure};
use clap::{Args, ValueEnum};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

pub mod build;
mod distro;
mod fixture;
mod supervisor;
use distro::{Architecture, Distro};
use fixture::Fixture;
use supervisor::Supervisor;

#[derive(Args)]
pub struct Options {
    /// Distros to test, serially. Omit to test every supported preset.
    #[arg(long = "distro", value_enum)]
    distros: Vec<Distro>,
    /// Executable architecture; Docker may emulate it on a different host.
    #[arg(long, value_enum)]
    arch: Architecture,
    /// Override the image for exactly one distro, e.g. its recorded repository digest.
    #[arg(long)]
    image: Option<String>,
    /// Fresh output directory for receipts and complete logs, including failures.
    #[arg(long)]
    output: PathBuf,
    /// Minimum free GiB on the host output filesystem. Never less than four.
    #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u64).range(4..=1024))]
    reserve_gib: u64,
    #[arg(long)]
    agent: PathBuf,
    #[arg(long)]
    unit_tests: PathBuf,
    #[arg(long)]
    helper_tests: PathBuf,
    #[arg(long)]
    monitor_tests: PathBuf,
    /// Optional optimized discovery_load executable; runs in each permission mode.
    #[arg(long)]
    load_tests: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct Artifact {
    path: PathBuf,
    bytes: u64,
    sha256: String,
}

impl Options {
    fn binaries(&self) -> impl Iterator<Item = (&str, &PathBuf)> {
        [
            ("agent", &self.agent),
            ("unit-tests", &self.unit_tests),
            ("helper-tests", &self.helper_tests),
            ("monitor-tests", &self.monitor_tests),
        ]
        .into_iter()
        .chain(self.load_tests.as_ref().map(|path| ("load-tests", path)))
    }

    fn artifacts(&self) -> Result<BTreeMap<String, Artifact>> {
        self.binaries()
            .map(|(name, path)| {
                ensure!(
                    path.is_absolute() && fs::symlink_metadata(path)?.file_type().is_file(),
                    "Use absolute regular executable paths: {}",
                    path.display()
                );
                let mut file = File::open(path)?;
                let mut header = [0; 20];
                file.read_exact(&mut header)?;
                validate_elf(&header, self.arch)?;
                let mut hash = Sha256::new();
                hash.update(header);
                let mut buffer = [0; 64 * 1024];
                loop {
                    let size = file.read(&mut buffer)?;
                    if size == 0 {
                        break;
                    }
                    hash.update(&buffer[..size]);
                }
                Ok((
                    name.into(),
                    Artifact {
                        path: path.clone(),
                        bytes: file.metadata()?.len(),
                        sha256: hash.finalize().iter().map(|b| format!("{b:02x}")).collect(),
                    },
                ))
            })
            .collect()
    }
}

fn validate_elf(header: &[u8; 20], arch: Architecture) -> Result<()> {
    ensure!(
        &header[..4] == b"\x7fELF"
            && header[4] == 2
            && header[5] == 1
            && u16::from_le_bytes([header[18], header[19]]) == arch.machine(),
        "Expected a little-endian Linux ELF executable for {}",
        arch.platform()
    );
    Ok(())
}

#[derive(Serialize)]
struct Receipt<'a> {
    schema: u8,
    started_utc: String,
    finished_utc: Option<String>,
    scope: &'static str,
    artifact_provenance: &'static str,
    checkout: String,
    checkout_status: String,
    architecture: Architecture,
    distro: Distro,
    setup_note: Option<&'static str>,
    artifacts: &'a BTreeMap<String, Artifact>,
    fixture: Fixture,
    status: &'static str,
    failure: Option<String>,
    cleanup_failure: Option<String>,
    minimum_free_bytes: u64,
    minimum_guest_free_bytes: Option<u64>,
}

fn save(runner: &Supervisor, receipt: &Receipt<'_>) -> Result<()> {
    #[derive(Serialize)]
    struct Report<'a, 'b> {
        receipt: &'a Receipt<'b>,
        steps: &'a [supervisor::Step],
    }
    let mut file = tempfile::NamedTempFile::new_in(&runner.directory)?;
    serde_json::to_writer_pretty(
        file.as_file_mut(),
        &Report {
            receipt,
            steps: &runner.steps,
        },
    )?;
    file.persist(runner.directory.join("receipt.json"))?;
    Ok(())
}

pub fn run(options: Options) -> Result<()> {
    ensure!(
        cfg!(unix),
        "Discovery matrix currently requires a macOS or Linux host"
    );
    if let Some(image) = &options.image {
        ensure!(
            options.distros.len() == 1
                && !image.is_empty()
                && !image.starts_with('-')
                && !image.chars().any(char::is_whitespace),
            "--image requires exactly one --distro and a Docker image reference"
        );
    }
    let artifacts = options.artifacts()?;
    let interrupted = Arc::new(AtomicBool::new(false));
    let signal = interrupted.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::SeqCst))?;
    let distros = if options.distros.is_empty() {
        Distro::value_variants().to_vec()
    } else {
        options.distros.clone()
    };
    let mut unique = distros.clone();
    unique.sort_by_key(|distro| distro.name());
    unique.dedup();
    ensure!(unique.len() == distros.len(), "Do not repeat --distro");
    ensure!(
        options.arch == Architecture::X86_64 || !distros.contains(&Distro::Arch),
        "The Arch preset requires --arch x86_64; for aarch64 explicitly select the other distros"
    );
    fs::create_dir(&options.output)
        .context("Use a fresh output directory with an existing parent")?;
    for distro in distros {
        let mut runner = Supervisor::new(
            options.output.join(distro.name()),
            options.reserve_gib * 1024 * 1024 * 1024,
            interrupted.clone(),
        )?;
        qualify(&options, distro, &artifacts, &mut runner)?;
    }
    eprintln!(
        "Discovery matrix passed. Evidence: {}",
        options.output.display()
    );
    Ok(())
}

fn new_receipt<'a>(
    options: &Options,
    distro: Distro,
    artifacts: &'a BTreeMap<String, Artifact>,
    runner: &Supervisor,
) -> Result<Receipt<'a>> {
    let mut random = [0; 12];
    getrandom::fill(&mut random)
        .map_err(|error| anyhow::anyhow!("Creating fixture identity: {error}"))?;
    let owner: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let image = options
        .image
        .clone()
        .unwrap_or_else(|| distro.image().into());
    Ok(Receipt {
        schema: 1,
        started_utc: chrono::Utc::now().to_rfc3339(),
        finished_utc: None,
        scope: "Disposable Linux userspace tests on Docker's shared kernel; emulation may apply. Native UI, systemd and kernel/SELinux acceptance are separate.",
        artifact_provenance: "Supplied prebuilt executables; hashes identify tested bytes. A separate build receipt must establish correspondence to source.",
        checkout: util::git_head()?,
        checkout_status: process::text(command("git").args(["status", "--short"]))?,
        architecture: options.arch,
        distro,
        setup_note: distro.setup_note(),
        artifacts,
        fixture: Fixture::new(
            format!("newport-discovery-{}-{owner}", distro.name()),
            image,
            owner,
        )?,
        status: "running",
        failure: None,
        cleanup_failure: None,
        minimum_free_bytes: runner.minimum_free_bytes,
        minimum_guest_free_bytes: None,
    })
}

fn qualify(
    options: &Options,
    distro: Distro,
    artifacts: &BTreeMap<String, Artifact>,
    runner: &mut Supervisor,
) -> Result<()> {
    let mut receipt = new_receipt(options, distro, artifacts, runner)?;
    save(runner, &receipt)?;
    let result = run_checks(options, distro, &mut receipt, runner);
    let cleanup = receipt.fixture.cleanup();
    receipt.status = if result.is_ok() && cleanup.is_ok() {
        "passed"
    } else {
        "failed"
    };
    receipt.failure = result
        .as_ref()
        .err()
        .map(|error: &anyhow::Error| format!("{error:#}"));
    receipt.cleanup_failure = cleanup.as_ref().err().map(|error| format!("{error:#}"));
    receipt.minimum_free_bytes = runner.minimum_free_bytes;
    receipt.minimum_guest_free_bytes = runner.minimum_guest_free_bytes;
    receipt.finished_utc = Some(chrono::Utc::now().to_rfc3339());
    save(runner, &receipt)?;
    eprintln!(
        "{}: {}; evidence {}",
        distro.name(),
        receipt.status,
        runner.directory.display()
    );
    match (result, cleanup) {
        (Err(test), Err(cleanup)) => Err(test.context(format!("Cleanup also failed: {cleanup:#}"))),
        (Err(error), _) | (_, Err(error)) => Err(error),
        _ => Ok(()),
    }
}

fn run_checks(
    options: &Options,
    distro: Distro,
    receipt: &mut Receipt<'_>,
    runner: &mut Supervisor,
) -> Result<()> {
    receipt.fixture.provision(distro, options.arch, runner)?;
    save(runner, receipt)?;
    for denied in [false, true] {
        if denied {
            runner.run(
                receipt
                    .fixture
                    .exec()
                    .args(["rm", "/etc/sudoers.d/newport-discovery"]),
                "disable-elevation",
                Duration::from_secs(30),
            )?;
        }
        let mut cmd = command(std::env::current_exe()?);
        cmd.args(["test", "discovery", "--container", &receipt.fixture.name]);
        for (name, path) in options.binaries() {
            cmd.arg(format!("--{name}")).arg(path);
        }
        if denied {
            cmd.arg("--elevation-unavailable");
        }
        if let Some(context) = std::env::var_os("NEWPORT_TEST_DOCKER_CONTEXT") {
            cmd.env("DOCKER_CONTEXT", context);
        }
        runner.run(
            &mut cmd,
            if denied { "denied" } else { "allowed" },
            Duration::from_secs(900),
        )?;
        receipt.fixture.check_empty()?;
        save(runner, receipt)?;
    }
    ensure!(
        options.artifacts()? == *receipt.artifacts,
        "Executables changed during qualification; rerun from stable artifacts"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_wrong_architecture_and_non_elf_artifacts() {
        let mut header = [0; 20];
        assert!(validate_elf(&header, Architecture::Aarch64).is_err());
        header[..6].copy_from_slice(b"\x7fELF\x02\x01");
        header[18..20].copy_from_slice(&183u16.to_le_bytes());
        assert!(validate_elf(&header, Architecture::Aarch64).is_ok());
        assert!(validate_elf(&header, Architecture::X86_64).is_err());
        header[5] = 2;
        assert!(validate_elf(&header, Architecture::Aarch64).is_err());
    }
}
