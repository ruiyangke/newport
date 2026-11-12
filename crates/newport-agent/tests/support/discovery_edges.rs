use super::{Helper, identity_snapshot};
use newport_protocol::monitor::ports::discovery::{Outcome, Process, Snapshot};
use std::{
    fs,
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, TcpListener},
    os::{fd::OwnedFd, unix::process::CommandExt},
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct ListenerProcess {
    child: Helper,
    directory: tempfile::TempDir,
    port: u16,
}

impl ListenerProcess {
    fn start(test: &str, restricted: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("holder-before");
        fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
        let ready = directory.path().join("ready");
        let mut command = Command::new(&executable);
        command.args([test, "--exact", "--nocapture", "--test-threads=1"]);
        if restricted {
            command.arg("--ignored");
        }
        let child = Helper(
            command
                .env("NEWPORT_DISCOVERY_EDGE_READY", &ready)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let mut fixture = Self {
            child,
            directory,
            port: 0,
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.exists() {
            assert!(fixture.child.0.try_wait().unwrap().is_none());
            assert!(Instant::now() < deadline, "Listener readiness deadline");
            thread::sleep(Duration::from_millis(5));
        }
        fixture.port = fs::read_to_string(ready).unwrap().parse().unwrap();
        fixture
    }

    fn pid(&self) -> u32 {
        self.child.0.id()
    }
}

fn child_fixture(restricted: bool) -> bool {
    let Some(ready) = std::env::var_os("NEWPORT_DISCOVERY_EDGE_READY") else {
        return false;
    };
    let ready = PathBuf::from(ready);
    assert!(ready.is_absolute() && !ready.exists());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    if restricted {
        // Only the disposable child loses ptrace visibility; the parent is unchanged.
        // SAFETY: prctl takes scalar arguments.
        assert_eq!(unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) }, 0);
    }
    let pending = ready.with_extension("pending");
    fs::write(&pending, listener.local_addr().unwrap().port().to_string()).unwrap();
    fs::rename(pending, ready).unwrap();
    let mut control = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN | libc::POLLHUP,
        revents: 0,
    };
    // A finite wait and parent-owned stdin also bound an orphaned fixture.
    // SAFETY: one initialized pollfd for the call.
    let readable = unsafe { libc::poll(&mut control, 1, 90_000) };
    if readable == 1 {
        let mut action = [0];
        if std::io::stdin().read(&mut action).unwrap() == 1 {
            assert_eq!(action, [b'e']);
            assert!(!restricted);
            let error = Command::new("/bin/sleep")
                .arg("90")
                .stdin(Stdio::from(fs::File::from(OwnedFd::from(listener))))
                .exec();
            panic!("Executing the replacement holder failed: {error}");
        }
    }
    true
}

fn holder(snapshot: &Snapshot, pid: u32, port: u16) -> &Process {
    let socket = snapshot
        .sockets
        .iter()
        .find(|socket| socket.endpoint.port == port)
        .expect("Fixture socket retained");
    snapshot
        .processes
        .iter()
        .find(|process| process.identity.pid == pid && socket.holders.contains(&process.id))
        .expect("Fixture holder verified")
}

#[test]
fn helper_refreshes_executable_names_after_rename_and_exec() {
    if child_fixture(false) {
        return;
    }
    let mut fixture = ListenerProcess::start(
        "edges::helper_refreshes_executable_names_after_rename_and_exec",
        false,
    );
    let home = tempfile::tempdir().unwrap();
    let before = identity_snapshot(home.path());
    let process = holder(&before, fixture.pid(), fixture.port);
    assert_eq!(process.executable_name.as_deref(), Some("holder-before"));
    let identity = process.identity;
    fs::rename(
        fixture.directory.path().join("holder-before"),
        fixture.directory.path().join("holder-renamed"),
    )
    .unwrap();
    let renamed = identity_snapshot(home.path());
    let process = holder(&renamed, fixture.pid(), fixture.port);
    assert_eq!(process.identity, identity);
    assert_eq!(process.executable_name.as_deref(), Some("holder-renamed"));
    fixture
        .child
        .0
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"e")
        .unwrap();
    let expected = fs::canonicalize("/bin/sleep").unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while fs::read_link(format!("/proc/{}/exe", fixture.pid())).unwrap() != expected {
        assert!(Instant::now() < deadline, "Executable replacement deadline");
        thread::sleep(Duration::from_millis(5));
    }
    let replaced = identity_snapshot(home.path());
    let process = holder(&replaced, fixture.pid(), fixture.port);
    assert_eq!(process.identity, identity);
    assert_eq!(
        process.executable_name.as_deref(),
        expected.file_name().unwrap().to_str()
    );
}

#[test]
fn helper_keeps_reuseport_sockets_and_their_distinct_holders() {
    use socket2::{Domain, Protocol, Socket, Type};
    let bind = |port| {
        let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP)).unwrap();
        socket.set_reuse_port(true).unwrap();
        socket
            .bind(&std::net::SocketAddr::from((Ipv4Addr::LOCALHOST, port)).into())
            .unwrap();
        socket.listen(8).unwrap();
        socket
    };
    let first = bind(0);
    let port = first.local_addr().unwrap().as_socket().unwrap().port();
    let second = bind(port);
    let spawn = |socket: Socket| {
        Helper(
            Command::new("/bin/sleep")
                .arg("90")
                .stdin(Stdio::from(fs::File::from(OwnedFd::from(socket))))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    };
    let first = spawn(first);
    let second = spawn(second);
    let home = tempfile::tempdir().unwrap();
    let snapshot = identity_snapshot(home.path());
    let sockets: Vec<_> = snapshot
        .sockets
        .iter()
        .filter(|socket| {
            socket.endpoint.port == port
                && socket.endpoint.address == IpAddr::V4(Ipv4Addr::LOCALHOST)
        })
        .collect();
    assert_eq!(sockets.len(), 2);
    assert_ne!(sockets[0].id, sockets[1].id);
    assert_ne!(sockets[0].inode, sockets[1].inode);
    let mut pids = Vec::new();
    for socket in sockets {
        assert_eq!(socket.holders.len(), 1);
        let process = snapshot
            .processes
            .iter()
            .find(|process| process.id == socket.holders[0])
            .unwrap();
        pids.push(process.identity.pid);
    }
    pids.sort_unstable();
    let mut expected = vec![first.0.id(), second.0.id()];
    expected.sort_unstable();
    assert_eq!(pids, expected);
}

#[test]
#[ignore = "requires an unprivileged account without ptrace capability"]
fn helper_retains_a_nondumpable_listener_without_inventing_a_holder() {
    if child_fixture(true) {
        return;
    }
    // SAFETY: geteuid has no preconditions and cannot fail.
    let uid = unsafe { libc::geteuid() };
    assert_ne!(
        uid, 0,
        "Use the unprivileged discovery qualification account"
    );
    let fixture = ListenerProcess::start(
        "edges::helper_retains_a_nondumpable_listener_without_inventing_a_holder",
        true,
    );
    let error = fs::read_dir(format!("/proc/{}/fd", fixture.pid())).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    let home = tempfile::tempdir().unwrap();
    let snapshot = identity_snapshot(home.path());
    let socket = snapshot
        .sockets
        .iter()
        .find(|socket| socket.endpoint.port == fixture.port)
        .unwrap();
    assert_eq!(socket.uid, uid);
    assert!(socket.holders.is_empty());
    assert!(
        snapshot
            .processes
            .iter()
            .all(|process| process.identity.pid != fixture.pid())
    );
    assert_eq!(snapshot.coverage.ipv4.outcome, Outcome::Complete);
    assert_eq!(snapshot.coverage.processes.outcome, Outcome::Partial);
}
