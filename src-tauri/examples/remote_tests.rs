//! Own the disposable server while running the backend tests in an isolated process.
//! Keeps test-only SSH credentials out of the parent process and other Rust tests.
use anyhow::{ensure, Context, Result};
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use testcontainers::{
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
    GenericImage, ImageExt,
};

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn checked(command: &mut Command) -> Result<()> {
    ensure!(command.status()?.success(), "Command failed: {command:?}");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let temp = tempfile::tempdir()?;
    let key = temp.path().join("client");
    checked(
        Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&key),
    )?;
    let container = GenericImage::new("porthop-test-remote", "local")
        .with_exposed_port(22.tcp())
        .with_wait_for(WaitFor::healthcheck())
        .with_copy_to("/fixture-key/client.pub", key.with_extension("pub"))
        .with_host_config_modifier(|config| {
            // Bound ENOSPC tests to disposable RAM, never the host disk.
            config.tmpfs = Some(std::collections::HashMap::from([(
                "/fault-disk".into(),
                "size=4m,mode=1777".into(),
            )]));
            config
                .port_bindings
                .as_mut()
                .unwrap()
                .values_mut()
                .for_each(|bindings| {
                    if let Some(bindings) = bindings {
                        for binding in bindings {
                            binding.host_ip = Some("127.0.0.1".into());
                        }
                    }
                });
        })
        .start()
        .await
        .context("Starting OpenSSH test container")?;
    let port = container.get_host_port_ipv4(22).await?;
    let mut host_key = Vec::new();
    container
        .copy_file_from("/etc/ssh/ssh_host_ed25519_key.pub", &mut host_key)
        .await?;
    let known_hosts = temp.path().join("known_hosts");
    std::fs::write(
        &known_hosts,
        format!("[127.0.0.1]:{port} {}", String::from_utf8(host_key)?),
    )?;
    let socket = temp.path().join("agent.sock");
    let _agent = Process(
        Command::new("ssh-agent")
            .args(["-D", "-a"])
            .arg(&socket)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !socket.exists() {
        ensure!(Instant::now() < deadline, "SSH agent did not start");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    checked(
        Command::new("ssh-add")
            .arg(&key)
            .env("SSH_AUTH_SOCK", &socket),
    )?;
    let mut tests = Process(
        Command::new("cargo")
            .args([
                "test",
                "--locked",
                "--manifest-path",
                "src-tauri/Cargo.toml",
                "remote_tests::",
                "--",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .current_dir(root)
            .env("SSH_AUTH_SOCK", &socket)
            .env("PORTHOP_TEST_SSH_PORT", port.to_string())
            .env("PORTHOP_TEST_IDENTITY", &key)
            .env("PORTHOP_TEST_KNOWN_HOSTS", &known_hosts)
            .spawn()?,
    );
    let deadline = Instant::now() + Duration::from_secs(600);
    let result = loop {
        if let Some(status) = tests.0.try_wait()? {
            break status.success();
        }
        if Instant::now() >= deadline {
            break false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    if !result {
        eprintln!(
            "OpenSSH fixture logs:\n{}",
            String::from_utf8_lossy(&container.stderr_to_vec().await?)
        );
    }
    drop(tests);
    container.rm().await?;
    ensure!(result, "Remote tests failed or exceeded ten minutes");
    Ok(())
}
