use crate::{command, process};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Serialize)]
pub(super) struct Step {
    name: String,
    exit_code: Option<i32>,
    error: Option<String>,
    elapsed_ms: u128,
}

pub(super) struct Supervisor {
    pub directory: PathBuf,
    pub minimum_free_bytes: u64,
    pub minimum_guest_free_bytes: Option<u64>,
    pub steps: Vec<Step>,
    pub container: Option<String>,
    reserve: u64,
    interrupted: Arc<AtomicBool>,
    last_guest_check: Option<Instant>,
}

impl Supervisor {
    pub fn new(directory: PathBuf, reserve: u64, interrupted: Arc<AtomicBool>) -> Result<Self> {
        if let Some(parent) = directory.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Cannot create {}", parent.display()))?;
        }
        fs::create_dir(&directory).context("Use a fresh evidence directory")?;
        Ok(Self {
            minimum_free_bytes: free_bytes(&directory)?,
            minimum_guest_free_bytes: None,
            directory,
            steps: Vec::new(),
            container: None,
            reserve,
            interrupted,
            last_guest_check: None,
        })
    }

    fn check(&mut self, deadline: Instant) -> Result<()> {
        ensure!(
            !self.interrupted.load(Ordering::SeqCst),
            "Discovery matrix interrupted"
        );
        ensure!(Instant::now() < deadline, "Discovery matrix step timed out");
        let free = free_bytes(&self.directory)?;
        self.minimum_free_bytes = self.minimum_free_bytes.min(free);
        ensure!(
            free >= self.reserve,
            "Disk reserve reached: {free} bytes available, {} required",
            self.reserve
        );
        if let Some(name) = &self.container
            && self
                .last_guest_check
                .is_none_or(|last| last.elapsed() >= Duration::from_secs(5))
        {
            let free = parse_free(&process::text(
                super::fixture::docker().args(["exec", "--user", "root", name, "df", "-Pk", "/"]),
            )?)?;
            self.minimum_guest_free_bytes = Some(
                self.minimum_guest_free_bytes
                    .map_or(free, |minimum| minimum.min(free)),
            );
            self.last_guest_check = Some(Instant::now());
            ensure!(
                free >= self.reserve,
                "Docker filesystem reserve reached: {free} bytes available, {} required",
                self.reserve
            );
        }
        Ok(())
    }

    pub fn run(&mut self, cmd: &mut Command, name: &str, timeout: Duration) -> Result<()> {
        let started = Instant::now();
        let mut exit_code = None;
        let result = (|| {
            self.check(started + timeout)?;
            let log = File::create_new(self.directory.join(format!("{name}.log")))?;
            cmd.stdin(Stdio::null())
                .stdout(log.try_clone()?)
                .stderr(log);
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                cmd.process_group(0);
            }
            eprintln!("Discovery matrix: {name}");
            let mut child = process::Process::spawn(cmd)?;
            loop {
                if let Some(status) = child.0.try_wait()? {
                    exit_code = status.code();
                    ensure!(
                        status.success(),
                        "{name} failed: {status}; see {}",
                        self.directory.join(format!("{name}.log")).display()
                    );
                    return Ok(());
                }
                if let Err(error) = self.check(started + timeout) {
                    terminate(&mut child);
                    return Err(error);
                }
                if fs::metadata(self.directory.join(format!("{name}.log")))?.len()
                    > 128 * 1024 * 1024
                {
                    terminate(&mut child);
                    anyhow::bail!("{name} exceeded the fixture log limit");
                }
                thread::sleep(Duration::from_millis(500));
            }
        })();
        self.steps.push(Step {
            name: name.into(),
            exit_code,
            error: result
                .as_ref()
                .err()
                .map(|error: &anyhow::Error| format!("{error:#}")),
            elapsed_ms: started.elapsed().as_millis(),
        });
        result
    }
}

fn terminate(child: &mut process::Process) {
    // A nested xtask owns Docker CLI children. Stop that whole host group;
    // the matrix owner then removes its container to stop guest processes.
    #[cfg(unix)]
    {
        let _ = command("kill")
            .args(["-TERM", "--", &format!("-{}", child.0.id())])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        thread::sleep(Duration::from_millis(200));
        let _ = command("kill")
            .args(["-KILL", "--", &format!("-{}", child.0.id())])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.0.kill();
    let _ = child.0.wait();
}

fn free_bytes(directory: &Path) -> Result<u64> {
    parse_free(&process::text(
        command("df")
            .args(["-Pk"])
            .arg(directory)
            .env("LC_ALL", "C"),
    )?)
}

fn parse_free(output: &str) -> Result<u64> {
    let line = output.lines().last().context("Missing disk availability")?;
    let blocks = line
        .split_whitespace()
        .nth(3)
        .context("Missing available disk blocks")?
        .parse::<u64>()
        .context("Invalid available disk blocks")?;
    blocks
        .checked_mul(1024)
        .context("Disk availability overflow")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_available_blocks_and_rejects_invalid_disk_output() {
        assert_eq!(parse_free("Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk 100 60 40 60% /a path").unwrap(), 40 * 1024);
        for text in [
            "",
            "unavailable",
            "fs 1 2 -1",
            "fs 1 2 18446744073709551615",
        ] {
            assert!(parse_free(text).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn refuses_low_disk_before_start_and_retains_failed_command_output() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("started");
        let mut runner =
            Supervisor::new(tmp.path().join("evidence"), u64::MAX, Arc::default()).unwrap();
        assert!(
            runner
                .run(
                    Command::new("touch").arg(&marker),
                    "reserve",
                    Duration::from_secs(10)
                )
                .is_err()
        );
        assert!(!marker.exists());
        runner.reserve = 0;
        assert!(
            runner
                .run(
                    Command::new("sh").args(["-c", "printf failure-evidence; exit 7"]),
                    "failure",
                    Duration::from_secs(10)
                )
                .is_err()
        );
        assert_eq!(runner.steps[1].exit_code, Some(7));
        assert_eq!(
            fs::read_to_string(runner.directory.join("failure.log")).unwrap(),
            "failure-evidence"
        );
        assert!(Supervisor::new(runner.directory.clone(), 0, Arc::default()).is_err());
    }

    #[test]
    fn a_fresh_checkout_gets_its_evidence_directory_and_a_reused_one_is_refused() {
        // CI starts without target/.
        let tmp = tempfile::tempdir().unwrap();
        let directory = tmp.path().join("target/discovery-build");
        Supervisor::new(directory.clone(), 0, Arc::default()).unwrap();
        assert!(directory.is_dir());
        assert!(Supervisor::new(directory, 0, Arc::default()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn timeout_stops_descendants_and_cancellation_prevents_start() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("escaped");
        let interrupted = Arc::new(AtomicBool::new(false));
        let mut runner =
            Supervisor::new(tmp.path().join("evidence"), 0, interrupted.clone()).unwrap();
        assert!(
            runner
                .run(
                    Command::new("sh")
                        .args(["-c", "(sleep 2; touch \"$1\") & wait", "sh"])
                        .arg(&marker),
                    "timeout",
                    Duration::from_millis(100)
                )
                .is_err()
        );
        thread::sleep(Duration::from_secs(2));
        assert!(!marker.exists(), "descendant survived timeout");
        interrupted.store(true, Ordering::SeqCst);
        assert!(
            runner
                .run(
                    Command::new("touch").arg(&marker),
                    "cancelled",
                    Duration::from_secs(10)
                )
                .is_err()
        );
        assert!(!marker.exists());
    }
}
