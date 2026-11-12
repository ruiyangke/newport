use crate::process;
use anyhow::{Context, Result, ensure};
use clap::Args;
use std::{ffi::OsStr, path::PathBuf, process::Command};

pub mod matrix;
mod target;
use target::Target;

#[derive(Args)]
pub struct Options {
    /// An existing disposable container with a newport user and noninteractive sudo.
    #[arg(long, required_unless_present = "ssh", conflicts_with = "ssh")]
    container: Option<String>,
    /// An isolated Linux test VM, as user@host. Uses existing SSH host verification.
    #[arg(long, requires = "ssh_identity", required_unless_present = "container")]
    ssh: Option<String>,
    /// The explicit SSH identity for the isolated test VM.
    #[arg(long, requires = "ssh")]
    ssh_identity: Option<PathBuf>,
    /// A Core or Full agent executable built from the same tree as the tests.
    #[arg(long)]
    agent: PathBuf,
    /// The discovery_helper integration test executable.
    #[arg(long)]
    helper_tests: PathBuf,
    /// The agent library test executable, restricted to ports::discovery.
    #[arg(long)]
    unit_tests: PathBuf,
    /// Also qualify the active monitor transport using discovery_monitor's executable.
    #[arg(long)]
    monitor_tests: Option<PathBuf>,
    /// Run the opt-in discovery_load benchmark with real listener processes.
    #[arg(long)]
    load_tests: Option<PathBuf>,
    /// Qualify socket activation on an isolated native systemd machine.
    #[arg(long, conflicts_with = "elevation_unavailable")]
    systemd_tests: Option<PathBuf>,
    /// Expect sudo to be missing or denied; ordinary helper checks still run.
    #[arg(long)]
    elevation_unavailable: bool,
    /// An isolated FUSE directory whose lookup handler deliberately stalls.
    #[arg(long)]
    stalled_project_directory: Option<PathBuf>,
}

struct Fixture {
    target: Target,
    directory: String,
}

impl Fixture {
    fn exec(&self, args: impl IntoIterator<Item = impl AsRef<OsStr>>) -> Result<Command> {
        self.target.exec(args)
    }

    fn remove(&self) -> Result<()> {
        process::run(&mut self.exec([
            "rm",
            "-f",
            "--",
            &format!("{}/agent", self.directory),
            &format!("{}/helper-tests", self.directory),
            &format!("{}/unit-tests", self.directory),
            &format!("{}/monitor-tests", self.directory),
            &format!("{}/systemd-tests", self.directory),
            &format!("{}/load-tests", self.directory),
        ])?)?;
        process::run(&mut self.exec(["rmdir", "--", &self.directory])?)
    }

    fn check(&self, options: &Options) -> Result<()> {
        process::run(&mut self.exec(["sh", "-c", "cat /etc/os-release; uname -m; id"])?)?;
        if !options.elevation_unavailable {
            process::run(&mut self.exec(["/usr/bin/sudo", "-n", "true"])?)?;
        }
        for (source, name) in [
            (&options.agent, "agent"),
            (&options.helper_tests, "helper-tests"),
            (&options.unit_tests, "unit-tests"),
        ]
        .into_iter()
        .chain(
            options
                .monitor_tests
                .as_ref()
                .map(|path| (path, "monitor-tests")),
        ) {
            self.target
                .copy(source, &format!("{}/{name}", self.directory))?;
        }
        if let Some(path) = &options.systemd_tests {
            self.target
                .copy(path, &format!("{}/systemd-tests", self.directory))?;
        }
        if let Some(path) = &options.load_tests {
            self.target
                .copy(path, &format!("{}/load-tests", self.directory))?;
        }
        process::run(&mut self.exec([
            &format!("{}/unit-tests", self.directory),
            "ports::discovery",
            "--test-threads=1",
            "--nocapture",
        ])?)?;
        let mut helper = vec![
            "env".to_owned(),
            format!("NEWPORT_DISCOVERY_TEST_AGENT={}/agent", self.directory),
        ];
        if let Some(directory) = &options.stalled_project_directory {
            helper.push(format!(
                "NEWPORT_DISCOVERY_STALLED_DIRECTORY={}",
                directory.display()
            ));
        }
        helper.extend(
            [
                &format!("{}/helper-tests", self.directory),
                "--include-ignored",
                "--test-threads=1",
                "--nocapture",
                "--skip",
                "root_listener_fixture",
            ]
            .map(str::to_owned),
        );
        if options.stalled_project_directory.is_none() {
            helper.extend([
                "--skip",
                "stalled_project_filesystem_does_not_hold_the_monitor_or_start_replacement_helpers",
            ].map(str::to_owned));
        }
        let skipped: &[&str] = if options.elevation_unavailable {
            &[
                "one_root_helper_serves_every_scan_and_exits_when_dropped",
                "root_helper_exits_on_control_eof_during_request_and_backpressured_response",
            ]
        } else {
            &["unavailable_sudo_returns_a_typed_failure_without_starting_a_root_helper"]
        };
        for name in skipped {
            helper.extend(["--skip".to_owned(), (*name).to_owned()]);
        }
        process::run(&mut self.exec(helper)?)?;
        if options.monitor_tests.is_some() {
            process::run(&mut self.exec([
                "env",
                &format!("NEWPORT_DISCOVERY_TEST_AGENT={}/agent", self.directory),
                &format!("{}/monitor-tests", self.directory),
                "--test-threads=1",
                "--nocapture",
            ])?)?;
        }
        if options.systemd_tests.is_some() {
            process::run(&mut self.exec([
                "env",
                &format!("NEWPORT_DISCOVERY_TEST_AGENT={}/agent", self.directory),
                "NEWPORT_DISCOVERY_SYSTEMD=1",
                &format!("{}/systemd-tests", self.directory),
                "--include-ignored",
                "--test-threads=1",
                "--nocapture",
            ])?)?;
        }
        if options.load_tests.is_some() {
            let executable = format!("{}/load-tests", self.directory);
            let mut args = vec![executable.as_str(), "--qualify-discovery-load"];
            if !options.elevation_unavailable {
                args.push("--elevation");
            }
            process::run(&mut self.exec(args)?)?;
        }
        Ok(())
    }
}

pub fn run(options: Options) -> Result<()> {
    if let Some(directory) = &options.stalled_project_directory {
        ensure!(
            directory.is_absolute() && directory.parent().is_some(),
            "Use an absolute isolated stalled project directory"
        );
    }
    let target = Target::new(&options)?;
    for path in [&options.agent, &options.helper_tests, &options.unit_tests]
        .into_iter()
        .chain(options.monitor_tests.as_ref())
        .chain(options.systemd_tests.as_ref())
        .chain(options.load_tests.as_ref())
    {
        ensure!(
            path.is_absolute() && std::fs::symlink_metadata(path)?.file_type().is_file(),
            "Use absolute paths to regular prebuilt executables"
        );
    }
    let directory =
        process::text(&mut target.exec(["mktemp", "-d", "/tmp/newport-discovery.XXXXXXXXXX"])?)?;
    ensure!(
        directory
            .strip_prefix("/tmp/newport-discovery.")
            .is_some_and(|suffix| {
                suffix.len() == 10 && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
            }),
        "Unexpected disposable test directory"
    );
    let fixture = Fixture { target, directory };
    let result = fixture.check(&options);
    let cleanup = fixture
        .remove()
        .context("Removing disposable discovery executables");
    if let Err(error) = cleanup {
        if let Err(test) = result {
            return Err(test.context(format!("Cleanup also failed: {error:#}")));
        }
        return Err(error);
    }
    result
}
