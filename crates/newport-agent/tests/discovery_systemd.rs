#![cfg(target_os = "linux")]

use newport_protocol::monitor::{Call, Reply, Request, Response, ports::discovery as d, wire};
use std::{
    io::{BufReader, Write},
    net::{TcpListener, TcpStream},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

mod support {
    pub mod discovery_units;
}
use support::discovery_units::Fixture;

struct Monitor {
    child: Child,
    calls: mpsc::Sender<Call>,
    replies: mpsc::Receiver<Response>,
    generation: u64,
}

impl Monitor {
    fn start() -> Self {
        let agent =
            std::env::var_os("NEWPORT_DISCOVERY_TEST_AGENT").expect("explicit isolated test agent");
        let mut child = Command::new(agent)
            .args(["monitor", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let (calls, receive) = mpsc::channel::<Call>();
        let (send, replies) = mpsc::channel();
        std::thread::spawn(move || {
            for call in receive {
                let Ok(reply) = wire::read_reply(&mut output, &call) else {
                    break;
                };
                if send.send(reply).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            calls,
            replies,
            generation: 0,
        }
    }

    fn refresh(&mut self) -> d::Snapshot {
        let call = Call {
            id: self.generation + 1,
            request: Request::Ports(d::Request { refresh: true }),
        };
        self.child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(&wire::encode_request(&call).unwrap())
            .unwrap();
        self.calls.send(call).unwrap();
        let reply = self.replies.recv_timeout(Duration::from_secs(30)).unwrap();
        let Reply::Ports(snapshot) = reply.reply else {
            panic!("unexpected monitor reply: {reply:?}");
        };
        assert!(snapshot.generation > self.generation);
        self.generation = snapshot.generation;
        snapshot.validate().unwrap();
        assert_eq!(snapshot.coverage.ipv4.outcome, d::Outcome::Complete);
        *snapshot
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.child.stdin.take();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn qualify(manager: d::Manager) {
    assert_eq!(
        std::env::var("NEWPORT_DISCOVERY_SYSTEMD").as_deref(),
        Ok("1")
    );
    let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = reservation.local_addr().unwrap().port();
    let fixture = Fixture::new(manager, port);
    drop(reservation);
    fixture.start();
    assert_eq!(fixture.main_pid(), 0, "service must not be activated yet");
    let mut monitor = Monitor::start();
    let before = monitor.refresh();
    let socket = listener(&before, port);
    let socket_unit_identified = socket.services.iter().any(|association| {
        association.relation == d::ServiceRelation::SocketCgroup
            && before.services.iter().any(|service| {
                service.id == association.service
                    && service.manager == manager
                    && service.unit == fixture.socket()
                    && service.triggers.contains(&fixture.service())
                    && service.main_pid.is_none()
            })
    });
    if socket
        .cgroup
        .as_ref()
        .and_then(|group| group.path.as_deref())
        == Some(fixture.socket_cgroup().as_str())
    {
        assert!(
            socket_unit_identified,
            "socket activation provenance missing: {before:?}"
        );
    } else {
        assert!(matches!(manager, d::Manager::User { .. }));
        assert!(
            !socket_unit_identified,
            "manager-owned socket cannot establish a specific socket unit"
        );
        assert!(
            socket.services.iter().any(|association| {
                before.services.iter().any(|service| {
                    service.id == association.service
                        && service.manager == manager
                        && service.unit == "init.scope"
                })
            }),
            "user manager provenance missing: {before:?}"
        );
    }

    let connection = TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        Duration::from_secs(2),
    )
    .unwrap();
    let pid = fixture.wait_active();
    let mut active = monitor.refresh();
    if manager == d::Manager::System
        && active
            .processes
            .iter()
            .all(|process| process.identity.pid != pid)
    {
        assert_eq!(
            active.coverage.elevation.outcome,
            d::Outcome::NotAttempted,
            "a throttled scan cannot reuse successful coverage without that scan's elevated observations"
        );
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    while !active
        .processes
        .iter()
        .any(|process| process.identity.pid == pid)
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(500));
        active = monitor.refresh();
    }
    let socket = listener(&active, port);
    let holder = active
        .processes
        .iter()
        .find(|process| process.identity.pid == pid && socket.holders.contains(&process.id))
        .unwrap_or_else(|| panic!("activated process is not a verified holder: {active:?}"));
    assert!(holder.identity.start_ticks > 0);
    assert!(
        socket.services.iter().any(|association| {
            association.relation == d::ServiceRelation::Holder { process: holder.id }
                && active.services.iter().any(|service| {
                    service.id == association.service
                        && service.manager == manager
                        && service.unit == fixture.service()
                        && service.main_pid == Some(pid)
                })
        }),
        "activated service provenance missing: {active:?}"
    );
    drop(connection);
    fixture.stop();
    let stopped = monitor.refresh();
    assert_eq!(before.scope, stopped.scope);
    assert!(
        !stopped
            .sockets
            .iter()
            .any(|socket| socket.endpoint.port == port)
    );
    eprintln!(
        "{manager:?}: socket unit identified before activation={socket_unit_identified}, verified holder after activation, complete absence after stop; generations={}/{}/{}",
        before.generation, active.generation, stopped.generation
    );
    fixture.finish();
}

fn listener(snapshot: &d::Snapshot, port: u16) -> &d::Socket {
    snapshot
        .sockets
        .iter()
        .find(|socket| socket.endpoint.port == port)
        .unwrap_or_else(|| panic!("fixture listener missing: {snapshot:?}"))
}

#[test]
#[ignore = "requires an explicitly selected isolated systemd VM and noninteractive sudo"]
fn system_socket_activation_preserves_socket_and_holder_provenance() {
    qualify(d::Manager::System);
}

#[test]
#[ignore = "requires an explicitly selected isolated systemd VM and current-user manager"]
fn user_socket_activation_preserves_socket_and_holder_provenance() {
    qualify(d::Manager::User {
        // SAFETY: geteuid has no preconditions and cannot fail.
        uid: unsafe { libc::geteuid() },
    });
}
