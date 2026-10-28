//! Packaged desktop check: the agent payloads and notices a built
//! `Newport.app` embeds must be exactly those of the signed full-agent export
//! it was built from. Packaging runs this before Developer ID signing.
use super::agent_export::verified_export;
use crate::{
    agent_product::Variant,
    desktop::{self, AgentReport, Payload, Payloads, REPORT_SCHEMA},
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{self, Read},
    path::Path,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

const EXECUTABLE: &str = "newport";
const ARCHITECTURES: [&str; 2] = ["x86_64", "aarch64"];
const PAYLOAD_LIMIT: u64 = 512 * 1024 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const OUTPUT_LIMIT: u64 = 1024 * 1024;

/// Run `<executable> --agent-identity` without the caller's environment.
pub fn run_probe(executable: &Path) -> Result<Vec<u8>> {
    let home = tempfile::tempdir()?;
    let mut command = Command::new(executable);
    command
        .arg(desktop::ARGUMENT)
        .env_clear()
        .env("HOME", home.path())
        .env("NEWPORT_DATA_DIR", home.path().join("profile"))
        .env("PATH", "/usr/bin:/bin");
    let output = bounded(&mut command, PROBE_TIMEOUT)?;
    ensure!(
        output.status.success(),
        "The packaged desktop could not report its agent identity"
    );
    Ok(output.stdout)
}

/// Compare a packaged desktop with the exact signed export it must embed:
/// its own report, the decompressed bundled payloads and the bundled notices.
/// Returns the verified report.
pub fn verify_desktop_agent(
    app: &Path,
    export: &Path,
    version: &str,
    public: &str,
    probe: &dyn Fn(&Path) -> Result<Vec<u8>>,
) -> Result<AgentReport> {
    let verified = verified_export(export, version, public)?;
    let mut payloads = Vec::with_capacity(2);
    for (architecture, sha256) in [
        ("x86_64", &verified.builds.x86_64),
        ("aarch64", &verified.builds.aarch64),
    ] {
        let path = export.join(format!("newport-agent-{architecture}"));
        let (hash, size) = hash_file(&path, PAYLOAD_LIMIT)?;
        ensure!(
            &hash == sha256,
            "Agent export changed during comparison: {architecture}"
        );
        payloads.push(Payload { sha256: hash, size });
    }
    let aarch64 = payloads.pop().unwrap();
    let expected = AgentReport {
        schema: REPORT_SCHEMA,
        variant: Variant::Full,
        payloads: Payloads {
            x86_64: payloads.pop().unwrap(),
            aarch64,
        },
        envelope: Some(desktop::Envelope {
            sha256: sha256(&verified.envelope),
            size: verified.envelope.len() as u64,
            version: version.into(),
            product: Some(verified.product.clone()),
        }),
    };
    let report: AgentReport = strict_json(&probe(&app.join("Contents/MacOS").join(EXECUTABLE))?)
        .context("The packaged desktop printed an invalid agent identity")?;
    ensure!(
        report == expected,
        "The packaged desktop's embedded agent identity does not match the signed export"
    );
    let mut licenses = BTreeMap::new();
    for entry in fs::read_dir(export.join("licenses")).context("Cannot read export notices")? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow!("Invalid export notice name"))?;
        licenses.insert(name, hash_file(&entry.path(), PAYLOAD_LIMIT)?.0);
    }
    bundled_agents(app, &expected.payloads, &licenses)?;
    Ok(report)
}

/// The bundle's agent resources: exactly both compressed payloads, whose
/// decompressed bytes match `payloads`, and exactly the export notices.
fn bundled_agents(
    app: &Path,
    payloads: &Payloads,
    licenses: &BTreeMap<String, String>,
) -> Result<()> {
    let directory = app.join("Contents/Resources/agents");
    let names = entries(&directory)?;
    let expected: BTreeSet<String> = ARCHITECTURES
        .iter()
        .map(|arch| format!("newport-agent-{arch}.gz"))
        .chain(["licenses".into()])
        .collect();
    ensure!(
        names == expected,
        "Bundled agent resources differ from the exact payload allowlist"
    );
    for (architecture, payload) in [("x86_64", &payloads.x86_64), ("aarch64", &payloads.aarch64)] {
        let path = directory.join(format!("newport-agent-{architecture}.gz"));
        let (file, _) = regular(&path, PAYLOAD_LIMIT)?;
        let mut decoder = flate2::read::MultiGzDecoder::new(file).take(payload.size + 1);
        let (hash, size) = hash_reader(&mut decoder)?;
        ensure!(
            hash == payload.sha256 && size == payload.size,
            "Bundled {architecture} agent does not match the signed export"
        );
    }
    let notices = directory.join("licenses");
    ensure!(
        fs::symlink_metadata(&notices)?.is_dir(),
        "Bundled agent notices must be a directory"
    );
    let mut bundled = BTreeMap::new();
    for name in entries(&notices)? {
        let hash = hash_file(&notices.join(&name), PAYLOAD_LIMIT)?.0;
        bundled.insert(name, hash);
    }
    ensure!(
        &bundled == licenses,
        "Bundled agent notices differ from the signed export"
    );
    Ok(())
}

/// Child process with one deadline and bounded output.
fn bounded(command: &mut Command, timeout: Duration) -> Result<Output> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Cannot start the check")?;
    let reader = |pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || -> io::Result<Vec<u8>> {
            let mut pipe = pipe;
            let mut data = Vec::new();
            (&mut pipe).take(OUTPUT_LIMIT + 1).read_to_end(&mut data)?;
            io::copy(&mut pipe, &mut io::sink())?;
            Ok(data)
        })
    };
    let stdout = reader(Box::new(child.stdout.take().unwrap()));
    let stderr = reader(Box::new(child.stderr.take().unwrap()));
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("The check exceeded its deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let join = |thread: std::thread::JoinHandle<io::Result<Vec<u8>>>| -> Result<Vec<u8>> {
        let data = thread
            .join()
            .map_err(|_| anyhow!("Output reader failed"))??;
        ensure!(
            data.len() as u64 <= OUTPUT_LIMIT,
            "The check exceeded its output limit"
        );
        Ok(data)
    };
    Ok(Output {
        status,
        stdout: join(stdout)?,
        stderr: join(stderr)?,
    })
}

/// Names in a real directory; every entry must be a regular file or directory.
fn entries(directory: &Path) -> Result<BTreeSet<String>> {
    ensure!(
        fs::symlink_metadata(directory)
            .with_context(|| format!("Cannot inspect {}", directory.display()))?
            .is_dir(),
        "Expected a real directory: {}",
        directory.display()
    );
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        ensure!(
            kind.is_file() || kind.is_dir(),
            "Unexpected link or special file in {}",
            directory.display()
        );
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow!("Invalid file name in {}", directory.display()))?;
        names.insert(name);
    }
    Ok(names)
}

fn regular(path: &Path, limit: u64) -> Result<(File, u64)> {
    let check = |metadata: fs::Metadata| -> Result<u64> {
        ensure!(
            metadata.file_type().is_file(),
            "Expected a regular file: {}",
            path.display()
        );
        ensure!(
            metadata.len() > 0 && metadata.len() <= limit,
            "Invalid file size: {}",
            path.display()
        );
        Ok(metadata.len())
    };
    check(
        fs::symlink_metadata(path).with_context(|| format!("Cannot inspect {}", path.display()))?,
    )?;
    let file = File::open(path).with_context(|| format!("Cannot read {}", path.display()))?;
    let size = check(file.metadata()?)?;
    Ok((file, size))
}

fn hash_reader(reader: &mut impl Read) -> Result<(String, u64)> {
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 64 * 1024];
    let mut size = 0u64;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        size += count as u64;
        hash.update(&buffer[..count]);
    }
    Ok((hex(&hash.finalize()), size))
}

fn hash_file(path: &Path, limit: u64) -> Result<(String, u64)> {
    let (file, size) = regular(path, limit)?;
    let (hash, read) = hash_reader(&mut file.take(limit + 1))?;
    ensure!(
        read == size,
        "File size changed while reading: {}",
        path.display()
    );
    Ok((hash, size))
}

fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn strict_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    Ok(serde_json::from_slice(bytes)?)
}

#[cfg(all(test, unix))]
#[path = "desktop_agent_tests.rs"]
mod tests;
