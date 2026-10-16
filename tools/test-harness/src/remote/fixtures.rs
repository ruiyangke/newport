//! Own the disposable server while running the backend tests in an isolated process.
//! Keeps test-only SSH credentials out of the parent process and other Rust tests.
use anyhow::{ensure, Context, Result};
use std::{
    path::{Path, PathBuf},
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

fn backend_command() -> Command {
    let mut command = crate::command("cargo");
    command
        .args(["test", "--locked", "-p", "newport-backend", "--lib"])
        .arg("--target-dir")
        .arg(crate::root().join("target"))
        .env_remove("CARGO_TARGET_DIR");
    command
}

pub(super) fn prepare(filters: &[String]) -> Result<(PathBuf, Vec<Vec<String>>)> {
    // Cold compilation must not consume the fixture runtime allowance.
    // Reuse Cargo's reported executable across isolated servers instead of
    // recompiling or guessing a hashed name in target/debug/deps.
    let output = tempfile::NamedTempFile::new()?;
    let build = crate::process::run(
        backend_command()
            .args(["--no-run", "--message-format=json"])
            .stdout(output.reopen()?),
    );
    let mut executable = None;
    for line in std::fs::read_to_string(output.path())?.lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(rendered) = message["message"]["rendered"].as_str() {
            eprint!("{rendered}");
        }
        if message["reason"] == "compiler-artifact"
            && message["target"]["name"] == "newport_backend"
            && message["profile"]["test"] == true
        {
            executable = message["executable"].as_str().map(PathBuf::from);
        }
    }
    build?;
    let executable = executable.context("Cargo did not report the backend test executable")?;
    let list = crate::process::text(
        Command::new(&executable)
            .args(["--ignored", "--list"])
            .args(filters),
    )?;
    let names: Vec<String> = list
        .lines()
        .filter_map(|line| line.strip_suffix(": test"))
        .map(str::to_owned)
        .collect();
    ensure!(
        !names.is_empty(),
        "No remote tests matched the requested filters"
    );
    Ok((executable, crate::fixture_suites::runs(names)))
}

pub(super) async fn run(executable: &Path, tests: &[String]) -> Result<()> {
    let root = crate::root();
    let (needs_systemd, needs_docker) = crate::fixture_suites::auxiliary_services(tests);
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
    let systemd = if needs_systemd {
        Some(
            GenericImage::new("newport-test-systemd", "local")
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
                .context("Starting systemd test container")?,
        )
    } else {
        None
    };
    // A Docker engine of its own (Docker-in-Docker) for the Containers tab:
    // prune and remove act engine-wide, so they must never reach the host's.
    let docker = if needs_docker {
        Some(
            GenericImage::new("newport-test-docker", "local")
                .with_exposed_port(22.tcp())
                .with_wait_for(WaitFor::healthcheck())
                .with_copy_to("/fixture-key/client.pub", key.with_extension("pub"))
                .with_privileged(true)
                .with_startup_timeout(Duration::from_secs(120))
                .with_host_config_modifier(loopback)
                .start()
                .await
                .context("Starting Docker test container")?,
        )
    } else {
        None
    };
    let port = container.get_host_port_ipv4(22).await?;
    let systemd_port = match &systemd {
        Some(server) => Some(server.get_host_port_ipv4(22).await?),
        None => None,
    };
    let docker_port = match &docker {
        Some(server) => Some(server.get_host_port_ipv4(22).await?),
        None => None,
    };
    let mut servers = vec![(&container, port)];
    if let Some(server) = &systemd {
        servers.push((server, systemd_port.unwrap()));
    }
    if let Some(server) = &docker {
        servers.push((server, docker_port.unwrap()));
    }
    let mut known = String::new();
    for (server, port) in servers {
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
    let mut backend = Command::new(executable);
    backend
        .args(["--ignored", "--exact", "--nocapture", "--test-threads=1"])
        .args(tests)
        .current_dir(&root)
        .env("SSH_AUTH_SOCK", &socket)
        .env("NEWPORT_TEST_SSH_PORT", port.to_string())
        .env_remove("NEWPORT_TEST_SYSTEMD_PORT")
        .env_remove("NEWPORT_TEST_DOCKER_PORT")
        .env("NEWPORT_TEST_IDENTITY", &key)
        .env("NEWPORT_TEST_KNOWN_HOSTS", &known_hosts);
    if let Some(port) = systemd_port {
        backend.env("NEWPORT_TEST_SYSTEMD_PORT", port.to_string());
    }
    if let Some(port) = docker_port {
        backend.env("NEWPORT_TEST_DOCKER_PORT", port.to_string());
    }
    // The shared feature suite includes real image builds, service operations,
    // and fault recovery. Keep a bounded suite budget separate from the tighter
    // allowance for each isolated lifecycle case.
    let budget = Duration::from_secs(if tests.len() > 1 { 1800 } else { 600 });
    eprintln!("Running {} remote tests (budget {budget:?})", tests.len());
    let mut tests = Process(backend.spawn()?);
    let deadline = Instant::now() + budget;
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
    if let Some(server) = systemd {
        server.rm().await?;
    }
    if let Some(server) = docker {
        server.rm().await?;
    }
    ensure!(result, "Remote tests failed or exceeded {budget:?}");
    Ok(())
}
