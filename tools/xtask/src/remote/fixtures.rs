//! Own the disposable server while running the backend tests in an isolated process.
//! Keeps test-only SSH credentials out of the parent process and other Rust tests.
use anyhow::{Context, Result, ensure};
use std::{
    io::Read,
    net::{Ipv4Addr, SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use testcontainers::{
    ContainerAsync, GenericImage, ImageExt,
    bollard::models::{HostConfig, PortBinding},
    core::{CgroupnsMode, IntoContainerPort, Mount, WaitFor},
    runners::AsyncRunner,
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
    config.publish_all_ports = Some(false);
    config.port_bindings = Some(std::collections::HashMap::from([(
        "22/tcp".into(),
        Some(vec![PortBinding {
            host_ip: Some("127.0.0.1".into()),
            host_port: Some("0".into()),
        }]),
    )]));
}
fn checked(command: &mut Command) -> Result<()> {
    ensure!(command.status()?.success(), "Command failed: {command:?}");
    Ok(())
}

fn wait_for_ssh(port: u16, budget: Duration) -> Result<()> {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let deadline = Instant::now() + budget;
    loop {
        let result = (|| -> std::io::Result<()> {
            let mut socket = TcpStream::connect_timeout(&address, Duration::from_millis(250))?;
            socket.set_read_timeout(Some(Duration::from_millis(250)))?;
            let mut banner = [0; 8];
            socket.read_exact(&mut banner)?;
            if &banner != b"SSH-2.0-" {
                return Err(std::io::Error::other(
                    "Endpoint did not return an SSH banner",
                ));
            }
            Ok(())
        })();
        match result {
            Ok(()) => return Ok(()),
            Err(error) if Instant::now() >= deadline => {
                return Err(error).with_context(|| {
                    format!("SSH endpoint {address} was not ready within {budget:?}")
                });
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn diagnostics(label: &str, server: &ContainerAsync<GenericImage>) {
    eprintln!("{label} fixture state, published ports and recent logs:");
    for args in [
        vec![
            "inspect",
            "--format",
            "{{json .State}} {{json .NetworkSettings.Ports}}",
            server.id(),
        ],
        vec!["logs", "--tail", "80", server.id()],
    ] {
        let result = crate::process::Process::spawn(crate::command(super::docker()).args(args))
            .and_then(|mut process| process.success(Duration::from_secs(10)));
        if let Err(error) = result {
            eprintln!("Cannot collect {label} fixture diagnostics: {error:#}");
        }
    }
}

pub(super) fn prepare(filters: &[String]) -> Result<(PathBuf, Vec<Vec<String>>)> {
    let executable = crate::fixture_suites::backend_executable()?;
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
        let mut command = vec!["/bin/sh", "/usr/local/bin/start-fixture"];
        if tests
            .iter()
            .any(|name| name.starts_with("remote_tests::containers::discovery::"))
        {
            command.push("--userland-proxy=false");
        }
        Some(
            GenericImage::new("newport-test-docker", "local")
                .with_exposed_port(22.tcp())
                .with_wait_for(WaitFor::healthcheck())
                .with_cmd(command)
                .with_env_var(
                    "NEWPORT_TEST_SECOND_ENGINE",
                    if tests.iter().any(|name| {
                        name.starts_with("remote_tests::containers::discovery::replacement::")
                    }) {
                        "1"
                    } else {
                        "0"
                    },
                )
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
    let podman = if crate::fixture_suites::requires_podman(tests) {
        // Docker refuses more CPUs than its host has; a CI runner's may have 3.
        let cpus = host_cpus().clamp(1, 4);
        Some(
            GenericImage::new("newport-test-podman", "local")
                .with_exposed_port(22.tcp())
                .with_wait_for(WaitFor::healthcheck())
                .with_copy_to("/fixture-key/client.pub", key.with_extension("pub"))
                .with_privileged(true)
                .with_startup_timeout(Duration::from_secs(120))
                .with_host_config_modifier(move |config| {
                    loopback(config);
                    config.memory = Some(2 * 1024 * 1024 * 1024);
                    config.nano_cpus = Some(cpus * 1_000_000_000);
                    config.pids_limit = Some(512);
                })
                .start()
                .await
                .context("Starting isolated Podman test container")?,
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
    let podman_port = match &podman {
        Some(server) => Some(server.get_host_port_ipv4(22).await?),
        None => None,
    };
    let mut servers = vec![("OpenSSH", &container, port)];
    if let Some(server) = &systemd {
        servers.push(("systemd", server, systemd_port.unwrap()));
    }
    if let Some(server) = &docker {
        servers.push(("Docker", server, docker_port.unwrap()));
    }
    if let Some(server) = &podman {
        servers.push((
            "Podman",
            server,
            podman_port.context("Missing Podman SSH port")?,
        ));
    }
    let mut known = String::new();
    for (label, server, port) in &servers {
        // Container health does not prove that the VM's published port is
        // reachable from the host running the backend tests (notably Colima).
        if let Err(error) = wait_for_ssh(*port, Duration::from_secs(30)) {
            diagnostics(label, server);
            return Err(error).with_context(|| format!("Waiting for {label} fixture SSH"));
        }
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
        .env_remove("NEWPORT_TEST_PODMAN_PORT")
        .env("NEWPORT_TEST_IDENTITY", &key)
        .env("NEWPORT_TEST_KNOWN_HOSTS", &known_hosts);
    if let Some(port) = systemd_port {
        backend.env("NEWPORT_TEST_SYSTEMD_PORT", port.to_string());
    }
    if let Some(port) = docker_port {
        backend.env("NEWPORT_TEST_DOCKER_PORT", port.to_string());
    }
    if let Some(port) = podman_port {
        backend.env("NEWPORT_TEST_PODMAN_PORT", port.to_string());
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
        for (label, server, _) in &servers {
            diagnostics(label, server);
        }
    }
    drop(servers);
    container.rm().await?;
    if let Some(server) = systemd {
        server.rm().await?;
    }
    if let Some(server) = docker {
        server.rm().await?;
    }
    if let Some(server) = podman {
        server.rm().await?;
    }
    ensure!(result, "Remote tests failed or exceeded {budget:?}");
    Ok(())
}

/// The CPUs Docker's host has, or 1 when Docker doesn't say.
fn host_cpus() -> i64 {
    crate::command(super::docker())
        .args(["info", "--format", "{{.NCPU}}"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, net::TcpListener};

    #[test]
    fn loopback_maps_ssh_without_automatic_wildcard_publication() {
        let mut config = HostConfig {
            port_bindings: Some(Default::default()),
            publish_all_ports: Some(true),
            ..Default::default()
        };
        loopback(&mut config);
        assert_eq!(config.publish_all_ports, Some(false));
        let bindings = config.port_bindings.unwrap();
        assert_eq!(bindings.len(), 1);
        let ports = bindings["22/tcp"].as_ref().unwrap();
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].host_ip.as_deref(), Some("127.0.0.1"));
        assert_eq!(ports[0].host_port.as_deref(), Some("0"));
    }

    #[test]
    fn host_readiness_waits_for_a_fragmented_ssh_banner() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let writer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.write_all(b"SSH-").unwrap();
            std::thread::sleep(Duration::from_millis(50));
            stream.write_all(b"2.0-OpenSSH\r\n").unwrap();
        });
        wait_for_ssh(port, Duration::from_secs(2)).unwrap();
        writer.join().unwrap();
    }

    #[test]
    fn a_listening_non_ssh_endpoint_is_not_ready() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let writer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.write_all(b"HTTP/1.1 200 OK\r\n").unwrap();
        });
        assert!(wait_for_ssh(port, Duration::from_millis(100)).is_err());
        writer.join().unwrap();
    }
}
