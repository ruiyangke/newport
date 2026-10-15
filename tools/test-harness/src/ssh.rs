use crate::{
    command,
    process::{self, Process},
    root,
};
use anyhow::{ensure, Result};
use std::{fs, time::Duration};
pub fn run(args: &[String]) -> Result<()> {
    ensure!(cfg!(unix), "SSH PTY fixture requires Unix");
    // Compatibility with earlier extraction runs; all fixture tests now live in the backend.
    let args: Vec<_> = args.iter().filter(|arg| *arg != "--backend-only").collect();
    process::run(command("cargo").args([
        "build",
        "--manifest-path",
        "tools/agent/Cargo.toml",
        "--target-dir",
        "tools/agent/target",
        "--locked",
    ]))?;
    // Compilation has its own budget, separate from fixture readiness/tests.
    process::run(command("cargo").args([
        "test",
        "--locked",
        "-p",
        "newport-backend",
        "--lib",
        "--no-run",
        "--target-dir",
        "target",
    ]))?;
    process::run(command("cargo").args([
        "build",
        "--manifest-path",
        "crates/newport-backend/Cargo.toml",
        "--target-dir",
        "target",
        "--example",
        "ssh_fixture",
        "--locked",
    ]))?;
    // Keep Unix socket paths below macOS's 104-byte limit.
    let temp = tempfile::Builder::new()
        .prefix("newport-ssh-")
        .tempdir_in("/tmp")?;
    let dir = temp.path();
    for name in ["client", "other", "host"] {
        process::run(
            command("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-C", "fixture", "-f"])
                .arg(dir.join(name)),
        )?;
    }
    fs::copy(dir.join("client"), dir.join("client.encrypted"))?;
    process::run(
        command("ssh-keygen")
            .args(["-q", "-p", "-P", "", "-N", "fixture-passphrase", "-f"])
            .arg(dir.join("client.encrypted")),
    )?;
    let socket = dir.join("agent.sock");
    let mut agent = Process::spawn(
        command("ssh-agent")
            .args(["-D", "-a"])
            .arg(&socket)
            .stdout(std::process::Stdio::null()),
    )?;
    process::ready(&socket, &mut agent)?;
    process::run(
        command("ssh-add")
            .arg(dir.join("client"))
            .env("SSH_AUTH_SOCK", &socket),
    )?;
    let mut server =
        Process::spawn(command(root().join("target/debug/examples/ssh_fixture")).arg(dir))?;
    process::ready(&dir.join("port"), &mut server)?;
    let mut test_command = command("cargo");
    test_command.args(["test", "--target-dir", "target"]);
    let mut tests = Process::spawn(
        test_command
            .args([
                "--manifest-path",
                "crates/newport-backend/Cargo.toml",
                "--locked",
            ])
            .args(&args)
            .args([
                "--",
                "--ignored",
                "--skip",
                "updates::",
                "--nocapture",
                "--test-threads=1",
            ])
            .args(
                crate::fixture_suites::LINUX
                    .iter()
                    .flat_map(|filter| ["--skip", *filter]),
            )
            .env("SSH_AUTH_SOCK", &socket)
            .env("NEWPORT_TEST_PASSWORD", "fixture-password")
            .env("NEWPORT_TEST_OTHER_KEY", dir.join("other.pub"))
            .env("NEWPORT_TEST_IDENTITY", dir.join("client"))
            .env("NEWPORT_TEST_KNOWN_HOSTS", dir.join("known_hosts"))
            .env(
                "NEWPORT_TEST_SSH_PORT",
                fs::read_to_string(dir.join("port"))?.trim(),
            ),
    )?;
    tests.success(Duration::from_secs(300))?;
    Ok(())
}
