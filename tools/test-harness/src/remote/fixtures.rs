//! Own the disposable server while running the backend tests in an isolated process.
//! Keeps test-only SSH credentials out of the parent process and other Rust tests.
use anyhow::{ensure, Context, Result};
use std::{
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use testcontainers::{
    bollard::models::HostConfig,
    core::{CgroupnsMode, IntoContainerPort, Mount, WaitFor},
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
// Published ports stay on loopback; these servers accept a throwaway key.
fn loopback(config: &mut HostConfig) {
    config
        .port_bindings
        .as_mut()
        .unwrap()
        .values_mut()
        .flatten()
        .flatten()
        .for_each(|binding| binding.host_ip = Some("127.0.0.1".into()));
}
fn checked(command: &mut Command) -> Result<()> {
    ensure!(command.status()?.success(), "Command failed: {command:?}");
    Ok(())
}

pub(super) async fn run(filter: &str) -> Result<()> {
    let root = crate::root();
    let temp = tempfile::tempdir()?;
    let key = temp.path().join("client");
    checked(
        Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&key),
    )?;
    let container = GenericImage::new("newport-test-remote", "local")
        .with_exposed_port(22.tcp())
        .with_wait_for(WaitFor::healthcheck())
        .with_copy_to("/fixture-key/client.pub", key.with_extension("pub"))
        .with_host_config_modifier(|config| {
            // Bound ENOSPC tests to disposable RAM, never the host disk.
            config.tmpfs = Some(std::collections::HashMap::from([(
                "/fault-disk".into(),
                "size=4m,mode=1777".into(),
            )]));
            loopback(config);
        })
        .start()
        .await
        .context("Starting OpenSSH test container")?;
    // systemd as PID 1 for the Services tab: it manages cgroups, so it needs
    // privilege and the host's cgroup tree.
    let systemd = GenericImage::new("newport-test-systemd", "local")
        .with_exposed_port(22.tcp())
        .with_wait_for(WaitFor::healthcheck())
        .with_copy_to("/fixture-key/client.pub", key.with_extension("pub"))
        .with_privileged(true)
        .with_cgroupns_mode(CgroupnsMode::Host)
        .with_mount(Mount::bind_mount("/sys/fs/cgroup", "/sys/fs/cgroup"))
        .with_startup_timeout(Duration::from_secs(120))
        .with_host_config_modifier(loopback)
        .start()
        .await
        .context("Starting systemd test container")?;
    // A Docker engine of its own (Docker-in-Docker) for the Containers tab:
    // prune and remove act engine-wide, so they must never reach the host's.
    let docker = GenericImage::new("newport-test-docker", "local")
        .with_exposed_port(22.tcp())
        .with_wait_for(WaitFor::healthcheck())
        .with_copy_to("/fixture-key/client.pub", key.with_extension("pub"))
        .with_privileged(true)
        .with_startup_timeout(Duration::from_secs(120))
        .with_host_config_modifier(loopback)
        .start()
        .await
        .context("Starting Docker test container")?;
    let port = container.get_host_port_ipv4(22).await?;
    let systemd_port = systemd.get_host_port_ipv4(22).await?;
    let docker_port = docker.get_host_port_ipv4(22).await?;
    let mut known = String::new();
    for (server, port) in [
        (&container, port),
        (&systemd, systemd_port),
        (&docker, docker_port),
    ] {
        let mut host_key = Vec::new();
        server
            .copy_file_from("/etc/ssh/ssh_host_ed25519_key.pub", &mut host_key)
            .await?;
        known += &format!("[127.0.0.1]:{port} {}", String::from_utf8(host_key)?);
    }
    let known_hosts = temp.path().join("known_hosts");
    std::fs::write(&known_hosts, known)?;
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
    let mut backend = Command::new("cargo");
    backend.args(["test", "--locked", "-p", "newport-backend", "--lib"]);
    // Backend tests use the workspace's normal build profile/cache.
    backend
        .arg("--target-dir")
        .arg(root.join("target"))
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_PROFILE_DEV_DEBUG");
    backend
        .args([filter, "--", "--ignored", "--nocapture", "--test-threads=1"])
        .current_dir(&root)
        .env("SSH_AUTH_SOCK", &socket)
        .env("NEWPORT_TEST_SSH_PORT", port.to_string())
        .env("NEWPORT_TEST_SYSTEMD_PORT", systemd_port.to_string())
        .env("NEWPORT_TEST_DOCKER_PORT", docker_port.to_string())
        .env("NEWPORT_TEST_IDENTITY", &key)
        .env("NEWPORT_TEST_KNOWN_HOSTS", &known_hosts);
    let mut tests = Process(backend.spawn()?);
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
    // Stop and reap the child before removing its containers, including timeout.
    drop(tests);
    if !result {
        eprintln!(
            "OpenSSH fixture logs:\n{}",
            String::from_utf8_lossy(&container.stderr_to_vec().await?)
        );
    }
    container.rm().await?;
    systemd.rm().await?;
    docker.rm().await?;
    ensure!(result, "Remote tests failed or exceeded ten minutes");
    Ok(())
}
