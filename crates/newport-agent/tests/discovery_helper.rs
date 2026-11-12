#![cfg(target_os = "linux")]
#[path = "support/discovery_edges.rs"]
mod edges;

use newport_protocol::monitor::{
    self,
    ports::discovery::helper::{Request, wire},
};
use std::{
    io::{Read, Write},
    net::TcpListener,
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

struct Helper(Child);
impl Drop for Helper {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

impl Helper {
    fn sudo(agent: &std::path::Path) -> Self {
        Self(
            Command::new("/usr/bin/sudo")
                .args(["-n", "--"])
                .arg(agent)
                .args(["listeners", "--serve"])
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        )
    }

    fn start(home: &std::path::Path) -> Self {
        let agent = std::env::var_os("NEWPORT_DISCOVERY_TEST_AGENT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_newport-agent").into());
        assert!(agent.is_absolute() && agent.is_file());
        Self(
            Command::new(agent)
                .args(["listeners", "--serve"])
                .env_clear()
                .env("HOME", home)
                .env("PATH", "/usr/bin:/bin")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        )
    }

    fn wait(&mut self, limit: Duration) -> ExitStatus {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "Helper failed to exit");
            thread::sleep(Duration::from_millis(10));
        }
    }
}

fn isolated_agent() -> std::path::PathBuf {
    let path = std::path::PathBuf::from(
        std::env::var_os("NEWPORT_DISCOVERY_TEST_AGENT")
            .expect("Set the isolated agent executable path"),
    );
    assert!(path.is_absolute() && path.is_file());
    assert_ne!(
        // SAFETY: geteuid has no preconditions and cannot fail.
        unsafe { libc::geteuid() },
        0,
        "Run qualification as the unprivileged SSH account"
    );
    path
}

fn root_helpers(agent: &std::path::Path) -> Vec<u32> {
    use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
            if entry.metadata().ok()?.uid() != 0 {
                return None;
            }
            let file = std::fs::File::open(entry.path().join("cmdline")).ok()?;
            let mut bytes = Vec::new();
            file.take(4096).read_to_end(&mut bytes).ok()?;
            (bytes.split(|byte| *byte == 0).next()? == agent.as_os_str().as_bytes()).then_some(pid)
        })
        .collect()
}

#[test]
#[ignore = "requires an isolated container where sudo is missing or denied"]
fn unavailable_sudo_returns_a_typed_failure_without_starting_a_root_helper() {
    use newport_agent::ports::discovery::helper::{Elevated, Error};
    let agent = isolated_agent();
    let started = Instant::now();
    let result = Elevated::start(&agent)
        .and_then(|mut elevated| elevated.scan(201, started + Duration::from_secs(3), &|| false));
    match result {
        Err(Error::Io(error)) => assert!(matches!(
            error.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
        )),
        Err(Error::Refused { diagnostic }) => assert!(!diagnostic.is_empty()),
        result => panic!("Expected unavailable elevation: {result:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(root_helpers(&agent).is_empty());
}

#[test]
#[ignore = "requires an isolated agent and existing noninteractive sudo permission"]
fn one_root_helper_serves_every_scan_and_exits_when_dropped() {
    let agent = isolated_agent();
    let (mut listener, pid, port) = root_listener();
    let mut elevated = newport_agent::ports::discovery::helper::Elevated::start(&agent).unwrap();
    for generation in [200, 201] {
        let snapshot = elevated
            .scan(generation, Instant::now() + Duration::from_secs(8), &|| {
                false
            })
            .unwrap();
        assert_eq!(snapshot.generation, generation);
        let socket = snapshot
            .sockets
            .iter()
            .find(|socket| socket.endpoint.port == port && socket.endpoint.address.is_loopback())
            .expect("isolated root listener");
        assert!(
            snapshot
                .processes
                .iter()
                .any(|process| socket.holders.contains(&process.id)
                    && process.identity.pid == pid
                    && process.uid == Some(0)
                    && process.name.is_some())
        );
        assert!(
            snapshot
                .processes
                .iter()
                .all(|process| process.details.is_none())
        );
    }
    assert_eq!(
        root_helpers(&agent).len(),
        1,
        "one helper serves both scans"
    );
    drop(elevated);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !root_helpers(&agent).is_empty() {
        assert!(Instant::now() < deadline, "Root helper survived its client");
        thread::sleep(Duration::from_millis(20));
    }
    drop(listener.0.stdin.take());
    assert!(listener.wait(Duration::from_secs(3)).success());
}

fn root_listener() -> (Helper, u32, u16) {
    use std::io::{BufRead, BufReader};
    let mut child = Helper(
        Command::new("/usr/bin/sudo")
            .args(["-n", "--"])
            .arg(std::env::current_exe().unwrap())
            .args([
                "root_listener_fixture",
                "--exact",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let output = child.0.stdout.take().unwrap();
    let (sent, received) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut output = BufReader::new(output.take(1024));
        let mut line = String::new();
        while output.read_line(&mut line).unwrap() != 0 {
            if let Some((_, identity)) = line.split_once("NEWPORT_ROOT_LISTENER=") {
                let (pid, port) = identity.trim().split_once(':').unwrap();
                sent.send((
                    pid.parse().unwrap(),
                    port.parse().unwrap(),
                    output.into_inner().into_inner(),
                ))
                .unwrap();
                return;
            }
            line.clear();
        }
    });
    let (pid, port, output) = received
        .recv_timeout(Duration::from_secs(3))
        .expect("root listener readiness");
    reader.join().unwrap();
    child.0.stdout = Some(output);
    (child, pid, port)
}

#[test]
#[ignore = "finite child fixture invoked through sudo by the holder qualification"]
fn root_listener_fixture() {
    // SAFETY: geteuid has no preconditions and cannot fail.
    assert_eq!(unsafe { libc::geteuid() }, 0);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    println!(
        "NEWPORT_ROOT_LISTENER={}:{}",
        std::process::id(),
        listener.local_addr().unwrap().port()
    );
    std::io::stdout().flush().unwrap();
    let mut control = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN | libc::POLLHUP,
        revents: 0,
    };
    // The parent owns this listener only while its control pipe remains open.
    // SAFETY: one initialized pollfd for the call.
    assert_eq!(unsafe { libc::poll(&mut control, 1, 20_000) }, 1);
    assert_eq!(std::io::stdin().read(&mut [0]).unwrap(), 0);
    drop(listener);
}

#[test]
#[ignore = "requires an isolated agent and existing noninteractive sudo permission"]
fn root_helper_exits_on_control_eof_during_request_and_backpressured_response() {
    use std::os::fd::AsRawFd;
    let agent = isolated_agent();
    for responding in [false, true] {
        let listeners: Vec<_> = if responding {
            (0..256)
                .map(|_| TcpListener::bind("127.0.0.1:0").unwrap())
                .collect()
        } else {
            Vec::new()
        };
        let mut helper = Helper::sudo(&agent);
        if responding {
            let fd = helper.0.stdout.as_ref().unwrap().as_raw_fd();
            // SAFETY: fcntl takes scalar arguments on the helper's open pipe.
            assert_eq!(unsafe { libc::fcntl(fd, libc::F_SETPIPE_SZ, 4096) }, 4096);
            // SAFETY: as above.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            assert!(flags >= 0);
            assert_eq!(
                // SAFETY: as above.
                unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) },
                0
            );
            helper
                .0
                .stdin
                .as_mut()
                .unwrap()
                .write_all(&wire::encode_request(&request()).unwrap())
                .unwrap();
        } else {
            helper
                .0
                .stdin
                .as_mut()
                .unwrap()
                .write_all(&[100, 1])
                .unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while root_helpers(&agent).is_empty() {
            assert!(Instant::now() < deadline, "No privileged helper appeared");
            thread::sleep(Duration::from_millis(5));
        }
        if responding {
            loop {
                match helper.0.stdout.as_mut().unwrap().read(&mut [0]) {
                    Ok(1) => break,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => (),
                    result => panic!("Expected helper output: {result:?}"),
                }
                assert!(
                    Instant::now() < deadline,
                    "Helper did not begin its response"
                );
                thread::sleep(Duration::from_millis(5));
            }
        }
        assert!(helper.0.try_wait().unwrap().is_none());
        drop(helper.0.stdin.take());
        assert!(!helper.wait(Duration::from_secs(2)).success());
        assert!(
            root_helpers(&agent).is_empty(),
            "Privileged helper survived control EOF"
        );
        drop(listeners);
    }
}

fn request() -> Request {
    Request {
        projects: None,
        contract_id: Some(monitor::CONTRACT_ID.into()),
        generation: 34,
    }
}

fn identity_snapshot(
    home: &std::path::Path,
) -> newport_protocol::monitor::ports::discovery::Snapshot {
    let mut helper = Helper::start(home);
    let request = request();
    helper
        .0
        .stdin
        .as_mut()
        .unwrap()
        .write_all(&wire::encode_request(&request).unwrap())
        .unwrap();
    let mut stdout = helper.0.stdout.take().unwrap();
    let snapshot = wire::read_reply(&mut stdout, &request)
        .unwrap()
        .result
        .unwrap();
    // Closing the control pipe between requests ends the helper normally.
    drop(helper.0.stdin.take());
    assert!(helper.wait(Duration::from_secs(8)).success());
    let mut rest = Vec::new();
    stdout.read_to_end(&mut rest).unwrap();
    assert!(rest.is_empty(), "Helper sent more than one frame");
    *snapshot
}

#[test]
fn actual_helper_returns_verified_lightweight_holders_without_starting_services() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let home = tempfile::tempdir().unwrap();
    let snapshot = identity_snapshot(home.path());
    let endpoint = listener.local_addr().unwrap();
    let socket = snapshot
        .sockets
        .iter()
        .find(|socket| {
            socket.endpoint.port == endpoint.port() && socket.endpoint.address == endpoint.ip()
        })
        .unwrap();
    assert!(socket.holders.iter().any(|id| {
        snapshot
            .processes
            .iter()
            .any(|process| process.id == *id && process.identity.pid == std::process::id())
    }));
    assert!(
        snapshot
            .processes
            .iter()
            .all(|process| process.details.is_none() && process.account.is_none())
    );
    assert!(snapshot.services.is_empty() && snapshot.publications.is_empty());
    assert!(
        std::fs::read_dir(home.path()).unwrap().next().is_none(),
        "Identity helper initialized services or user state"
    );
}

#[test]
fn actual_project_helper_preserves_socket_facts_and_revalidates_the_directory_owner() {
    use newport_protocol::monitor::ports::discovery::{Details, ObservedPath, Outcome, Report};
    use std::{fs, os::fd::OwnedFd};
    let directory = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("Cargo.toml"), "").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let holder = Helper(
        Command::new("/bin/sleep")
            .arg("30")
            .current_dir(directory.path())
            .stdin(Stdio::from(fs::File::from(OwnedFd::from(
                listener.try_clone().unwrap(),
            ))))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut input = identity_snapshot(home.path());
    let process = input
        .processes
        .iter_mut()
        .find(|process| process.identity.pid == holder.0.id())
        .unwrap();
    process.details = Some(Details {
        directory: Some(ObservedPath {
            value: directory.path().to_str().unwrap().into(),
            deleted: false,
            lossy: false,
        }),
        ..Default::default()
    });
    input.coverage.details = Report::complete();
    let agent = std::env::var_os("NEWPORT_DISCOVERY_TEST_AGENT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_newport-agent").into());
    let mut client = newport_agent::ports::discovery::helper::Client::default();
    let output = client
        .projects(
            &agent,
            &input,
            Instant::now() + Duration::from_secs(3),
            &|| false,
        )
        .unwrap();
    assert_eq!(output.sockets, input.sockets);
    let process = output
        .processes
        .iter()
        .find(|process| process.identity.pid == holder.0.id())
        .unwrap();
    assert_eq!(
        process
            .details
            .as_ref()
            .unwrap()
            .project
            .as_ref()
            .unwrap()
            .root,
        directory.path().to_str().unwrap()
    );
    input
        .processes
        .iter_mut()
        .find(|process| process.identity.pid == holder.0.id())
        .unwrap()
        .identity
        .start_ticks += 1;
    let refused = client
        .projects(
            &agent,
            &input,
            Instant::now() + Duration::from_secs(3),
            &|| false,
        )
        .unwrap();
    let process = refused
        .processes
        .iter()
        .find(|process| process.identity.pid == holder.0.id())
        .unwrap();
    assert!(process.details.as_ref().unwrap().project.is_none());
    assert_eq!(process.metadata.outcome, Outcome::Partial);
    assert_eq!(refused.sockets, input.sockets);
    assert!(fs::read_dir(home.path()).unwrap().next().is_none());
}

#[test]
#[ignore = "requires an isolated FUSE mount that stalls lookups and records a request"]
fn stalled_project_filesystem_does_not_hold_the_monitor_or_start_replacement_helpers() {
    use newport_protocol::monitor::ports::discovery::{
        Compose, ContainerState, Endpoint, Publication, Report,
    };
    let directory = std::env::var("NEWPORT_DISCOVERY_STALLED_DIRECTORY").unwrap();
    let home = tempfile::tempdir().unwrap();
    let mut input = identity_snapshot(home.path());
    input.publications.push(Publication {
        engine: Some("fixture".into()),
        container: "fixture".into(),
        name: "fixture".into(),
        image: None,
        state: ContainerState::Running,
        host: Endpoint {
            address: "127.0.0.1".parse().unwrap(),
            port: 18080,
            interface: None,
        },
        target_port: 80,
        target_address: None,
        compose: Some(Compose {
            directory: Some(directory.clone()),
            ..Default::default()
        }),
    });
    input.container_engine = Some("fixture".into());
    input.coverage.containers = Report::complete();
    input.coverage.details = Report::complete();
    let before = input.clone();
    let agent = std::path::PathBuf::from(std::env::var_os("NEWPORT_DISCOVERY_TEST_AGENT").unwrap());
    let mut client = newport_agent::ports::discovery::helper::Client::default();
    for cancelled in [false, true] {
        let started = Instant::now();
        let error = client
            .projects(
                &agent,
                &input,
                started + Duration::from_millis(500),
                &|| cancelled && started.elapsed() >= Duration::from_millis(150),
            )
            .unwrap_err();
        assert!(
            matches!(
                error,
                newport_agent::ports::discovery::helper::Error::TimedOut
                    | newport_agent::ports::discovery::helper::Error::Cancelled
                    | newport_agent::ports::discovery::helper::Error::CleanupPending { .. }
            ),
            "{error}"
        );
        assert!(started.elapsed() < Duration::from_secs(2), "{error}");
        assert!(
            std::path::Path::new(&format!("{directory}.requested")).exists(),
            "the kernel must have requested a blocking filesystem lookup"
        );
        assert_eq!(input, before);
        let children = std::fs::read_to_string("/proc/thread-self/children").unwrap();
        assert!(
            children.split_whitespace().count() <= 1,
            "unreaped helpers must not accumulate: {children}"
        );
    }
    std::fs::write(format!("{directory}.release"), "").unwrap();
    input.publications.clear();
    // A subsequent ordinary exchange proves the bounded client can be reused.
    let recovery = Instant::now() + Duration::from_secs(3);
    let recovered = loop {
        client.poll_cleanup();
        match client.projects(&agent, &input, recovery, &|| false) {
            Ok(value) => break value,
            Err(newport_agent::ports::discovery::helper::Error::CleanupPending { .. })
                if Instant::now() < recovery =>
            {
                thread::sleep(Duration::from_millis(10))
            }
            Err(error) => {
                panic!("Project helper did not recover after releasing the filesystem: {error}")
            }
        }
    };
    assert_eq!(recovered.sockets, input.sockets);
}

#[test]
fn actual_helper_exits_on_control_eof_and_times_out_an_incomplete_request() {
    let home = tempfile::tempdir().unwrap();
    let mut helper = Helper::start(home.path());
    drop(helper.0.stdin.take());
    assert!(helper.wait(Duration::from_secs(2)).success());
    let started = Instant::now();
    let mut helper = Helper::start(home.path());
    helper
        .0
        .stdin
        .as_mut()
        .unwrap()
        .write_all(&[100, 1])
        .unwrap();
    assert!(!helper.wait(Duration::from_secs(8)).success());
    assert!(started.elapsed() >= newport_agent::ports::discovery::helper::LIFETIME);
}
