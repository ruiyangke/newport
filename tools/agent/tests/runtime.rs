use newport_agent::runtime_protocol as protocol;
use std::{
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    time::Duration,
};
const BIN: &str = env!("CARGO_BIN_EXE_newport-agent");
struct Worker(Child);
impl std::ops::Deref for Worker {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.0
    }
}
impl std::ops::DerefMut for Worker {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
// Keep IPC roots independent across concurrent tests and short enough for
// macOS Unix sockets. Product runtime paths are selected independently.
fn fixture_home() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("np-runtime-")
        .tempdir_in("/tmp")
        .unwrap()
}

struct Runtime {
    session: String,
    client: String,
    child: Child,
    input: Option<ChildStdin>,
    frames: mpsc::Receiver<serde_json::Value>,
    hello: protocol::Hello,
}
impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Runtime {
    fn start(home: &std::path::Path, path: &str) -> Self {
        let mut child = Command::new(BIN)
            .args(["runtime", "--stdio"])
            .env("HOME", home)
            .env("XDG_RUNTIME_DIR", home)
            .env("PATH", path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let mut output = child.stdout.take().unwrap();
        let (send, frames) = mpsc::sync_channel(8);
        std::thread::spawn(move || {
            while let Ok(frame) = protocol::read(&mut output) {
                if send.send(frame).is_err() {
                    break;
                }
            }
        });
        let hello: protocol::Hello =
            serde_json::from_value(frames.recv_timeout(Duration::from_secs(15)).expect("hello"))
                .unwrap();
        hello.validate().unwrap();
        Self {
            session: uuid::Uuid::new_v4().to_string(),
            client: uuid::Uuid::new_v4().to_string(),
            child,
            input,
            frames,
            hello,
        }
    }
    fn request(&mut self, runtime_id: String, operation: protocol::Operation) -> protocol::Reply {
        let request = protocol::Request {
            id: uuid::Uuid::new_v4().to_string(),
            runtime_id,
            operation,
        };
        protocol::write(self.input.as_mut().unwrap(), &request).unwrap();
        let reply: protocol::Reply =
            serde_json::from_value(self.frames.recv_timeout(Duration::from_secs(5)).unwrap())
                .unwrap();
        assert_eq!(reply.id, request.id);
        reply
    }
    fn initialize(&mut self) {
        let id = self.hello.instance_id.clone();
        assert!(self
            .request(
                id,
                protocol::Operation::Initialize {
                    version: protocol::VERSION,
                    session_id: self.session.clone(),
                    transport_generation: 1,
                }
            )
            .error
            .is_none());
    }
    fn register(&mut self, service: &str, version: u32) -> String {
        let worker = uuid::Uuid::new_v4().to_string();
        let id = self.hello.instance_id.clone();
        assert!(self
            .request(
                id,
                protocol::Operation::Register {
                    worker_id: worker.clone(),
                    service: service.into(),
                    version
                }
            )
            .error
            .is_none());
        worker
    }
    fn worker(&self, worker: &str, service: &str, home: &std::path::Path) -> Worker {
        let mut command = Command::new(BIN);
        command.args([
            "worker",
            &self.hello.build_sha256,
            &self.hello.instance_id,
            &self.hello.worker_socket,
            worker,
            service,
        ]);
        if service == "integration" {
            command.arg(&self.client);
        }
        Worker(
            command
                .env("HOME", home)
                .env("XDG_RUNTIME_DIR", home)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        )
    }
    fn integration_root(&self, home: &std::path::Path) -> std::path::PathBuf {
        for entry in std::fs::read_dir(home.join(".cache/newport/sessions")).unwrap() {
            let value: serde_json::Value =
                serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap();
            if value["runtime"] == self.hello.instance_id {
                return value["root"].as_str().unwrap().into();
            }
        }
        panic!("registered integration root missing");
    }
    fn stopped(&mut self) {
        for _ in 0..200 {
            if self.child.try_wait().unwrap().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("runtime did not stop");
    }
}

#[test]
fn control_eof_revokes_an_idle_integration_and_cleans_its_endpoints() {
    use newport_agent::{integration_control as control, wire};
    let home = fixture_home();
    let mut runtime = Runtime::start(home.path(), "");
    runtime.initialize();
    let registration = runtime.register("integration", control::VERSION);
    let mut worker = runtime.worker(&registration, "integration", home.path());
    let mut output = worker.stdout.take().unwrap();
    let (send, events) = mpsc::sync_channel(4);
    std::thread::spawn(move || {
        while let Ok(event) = wire::read(&mut output) {
            if send.send(event).is_err() {
                break;
            }
        }
    });
    let hello = events.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(hello.0, b'R', "{}", String::from_utf8_lossy(&hello.1));
    control::decode::<control::Hello>(&hello.1)
        .unwrap()
        .validate()
        .unwrap();
    let runtime_id = runtime.hello.instance_id.clone();
    let applied = runtime.request(
        runtime_id,
        protocol::Operation::Permissions {
            worker_id: registration,
            permissions: control::Permissions {
                revision: 1,
                clipboard: false,
                browser: true,
            },
        },
    );
    assert!(applied.error.is_none(), "{:?}", applied.error);
    assert!(applied.applied.unwrap().permissions.browser);
    let root = runtime.integration_root(home.path());
    assert!(root.join("agent.sock").exists());
    assert!(root.join("agent-client").exists());
    // Keep feature stdin open: only the control session disappearing should
    // revoke the worker, including its published helper endpoint and metadata.
    runtime.input.take();
    runtime.stopped();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while worker.try_wait().unwrap().is_none() {
        if std::time::Instant::now() >= deadline {
            let _ = worker.kill();
            let _ = worker.wait();
            panic!("integration survived control EOF");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    for file in ["agent.sock", "agent-client", "features", "snapshot.tar"] {
        assert!(!root.join(file).exists(), "leftover {file}");
    }
}
#[test]
fn negotiates_build_and_capabilities_without_installing_anything() {
    use sha2::{Digest, Sha256};
    let home = fixture_home();
    // Do expensive fixture work before the runtime's initialization deadline.
    let expected: String = Sha256::digest(std::fs::read(BIN).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let mut runtime = Runtime::start(home.path(), &std::env::var("PATH").unwrap());
    assert_eq!(runtime.hello.build_sha256, expected);
    runtime.initialize();
    let id = runtime.hello.instance_id.clone();
    assert!(runtime
        .request(id.clone(), protocol::Operation::Ping)
        .error
        .is_none());
    assert!(runtime
        .request(id, protocol::Operation::Shutdown)
        .error
        .is_none());
    runtime.stopped();
    assert!(!home.path().join(".local/bin").exists());
    assert!(!home.path().join(".local/share/newport/needle").exists());
}

#[test]
fn maintenance_excludes_every_cli_service_without_installing_anything() {
    let home = fixture_home();
    let maintenance =
        newport_agent::installation_use::MaintenanceLease::acquire(home.path()).unwrap();
    for args in [
        vec!["runtime", "--stdio"],
        vec!["worker", "invalid", "registration"],
        vec!["serve"],
        vec!["git-rpc", "--stdio"],
        vec!["monitor", "--stdio"],
        vec!["clipboard", "-o"],
        vec!["open", "https://example.test"],
        vec!["display", "--backend", "wayland"],
        vec!["env"],
        vec!["sessions"],
    ] {
        let result = Command::new(BIN)
            .args(&args)
            .env("HOME", home.path())
            .env("XDG_RUNTIME_DIR", home.path())
            .output()
            .unwrap();
        assert!(!result.status.success(), "{args:?}");
        assert!(
            result.stdout.is_empty(),
            "feature output before admission: {args:?}"
        );
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("maintenance is in progress"),
            "{args:?}: {:?}",
            result.stderr
        );
    }
    let aliases = fixture_home();
    for name in ["xclip", "wl-paste", "newport-browser", "xdg-open"] {
        let alias = aliases.path().join(name);
        std::os::unix::fs::symlink(BIN, &alias).unwrap();
        let result = Command::new(alias)
            .env("HOME", home.path())
            .env("XDG_RUNTIME_DIR", home.path())
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("maintenance is in progress"));
    }
    let probe = Command::new(BIN)
        .args(["deployment", "protocol"])
        .env("HOME", home.path())
        .env("XDG_RUNTIME_DIR", home.path())
        .output()
        .unwrap();
    assert!(probe.status.success());
    let probe: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    assert_eq!(probe["usageLease"], 2);
    assert!(!home.path().join(".local/bin").exists());
    assert!(!home.path().join(".local/share/newport/needle").exists());
    drop(maintenance);
    let mut runtime = Runtime::start(home.path(), "");
    runtime.initialize();
    runtime.request(
        runtime.hello.instance_id.clone(),
        protocol::Operation::Shutdown,
    );
    runtime.stopped();
}

#[test]
fn runtime_and_worker_usage_leases_survive_controller_loss_independently() {
    let home = fixture_home();
    let other_home = fixture_home();
    let path = std::env::var("PATH").unwrap();
    let mut first = Runtime::start(home.path(), &path);
    first.initialize();
    let mut second = Runtime::start(home.path(), &path);
    second.initialize();
    assert!(newport_agent::installation_use::MaintenanceLease::acquire(home.path()).is_err());
    assert!(newport_agent::installation_use::MaintenanceLease::acquire(other_home.path()).is_ok());
    let registration = first.register("git", 4);
    let mut worker = first.worker(&registration, "git", home.path());
    let mut output = worker.stdout.take().unwrap();
    let (send, receive) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = send.send(newport_agent::git::protocol::read(&mut output));
    });
    assert!(matches!(
        receive
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap(),
        newport_agent::git::protocol::Message::Hello { .. }
    ));
    // A stopped worker cannot process controller EOF yet, but still owns the
    // installation. Maintenance must not infer lifetime from control alone.
    assert_eq!(unsafe { libc::kill(worker.id() as i32, libc::SIGSTOP) }, 0);
    first.child.kill().unwrap();
    first.child.wait().unwrap();
    second.request(
        second.hello.instance_id.clone(),
        protocol::Operation::Shutdown,
    );
    second.stopped();
    assert!(newport_agent::installation_use::MaintenanceLease::acquire(home.path()).is_err());
    worker.kill().unwrap();
    worker.wait().unwrap();
    let maintenance =
        newport_agent::installation_use::MaintenanceLease::acquire(home.path()).unwrap();
    maintenance.validate().unwrap();
}

#[test]
fn permission_revocation_bypasses_an_incomplete_feature_upload() {
    use newport_agent::{integration_control as control, wire};
    use std::io::Write;
    let home = fixture_home();
    let mut runtime = Runtime::start(home.path(), &std::env::var("PATH").unwrap());
    runtime.initialize();
    let worker_id = runtime.register("integration", control::VERSION);
    let mut worker = runtime.worker(&worker_id, "integration", home.path());
    assert_eq!(wire::read(worker.stdout.as_mut().unwrap()).unwrap().0, b'R');
    let id = runtime.hello.instance_id.clone();
    let enable = runtime.request(
        id.clone(),
        protocol::Operation::Permissions {
            worker_id: worker_id.clone(),
            permissions: control::Permissions {
                revision: 1,
                clipboard: false,
                browser: true,
            },
        },
    );
    assert!(enable.applied.unwrap().permissions.browser);
    // Declare a feature frame and stop in its header. The feature reader will
    // wait indefinitely; permission traffic must use the separate connection.
    worker
        .stdin
        .as_mut()
        .unwrap()
        .write_all(&[b'M', 0, 0])
        .unwrap();
    let start = std::time::Instant::now();
    let disable = runtime.request(
        id.clone(),
        protocol::Operation::Permissions {
            worker_id,
            permissions: control::Permissions {
                revision: 2,
                clipboard: false,
                browser: false,
            },
        },
    );
    assert!(disable.error.is_none(), "{:?}", disable.error);
    assert!(!disable.applied.unwrap().permissions.browser);
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(
        std::fs::read_to_string(runtime.integration_root(home.path()).join("features"))
            .unwrap()
            .trim(),
        ""
    );
    assert!(worker.try_wait().unwrap().is_none());
    // Revocation is scoped to this worker; the runtime can still admit Git.
    let git_id = runtime.register("git", 4);
    let mut git = runtime.worker(&git_id, "git", home.path());
    assert!(matches!(
        newport_agent::git::protocol::read(git.stdout.as_mut().unwrap()).unwrap(),
        newport_agent::git::protocol::Message::Hello { .. }
    ));
    assert!(runtime
        .request(id, protocol::Operation::Ping)
        .error
        .is_none());
}

#[test]
fn stalled_feature_output_cannot_prevent_revocation_or_worker_cleanup() {
    use newport_agent::{integration_control as control, wire};
    use std::{
        io::Write,
        os::{fd::OwnedFd, unix::net::UnixStream},
    };
    let home = fixture_home();
    let mut runtime = Runtime::start(home.path(), "");
    runtime.initialize();
    let worker_id = runtime.register("integration", control::VERSION);
    let (mut output, mut reader) = UnixStream::pair().unwrap();
    // Starting a debug worker verifies the executable's hash before Ready.
    // Allow the same startup budget as Runtime::start on loaded CI runners;
    // the revocation and cleanup deadlines below remain independently bounded.
    reader
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let descriptor: OwnedFd = output.try_clone().unwrap().into();
    let mut worker = Worker(
        Command::new(BIN)
            .args([
                "worker",
                &runtime.hello.build_sha256,
                &runtime.hello.instance_id,
                &runtime.hello.worker_socket,
                &worker_id,
                "integration",
                &uuid::Uuid::new_v4().to_string(),
            ])
            .env("HOME", home.path())
            .env("XDG_RUNTIME_DIR", home.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::from(descriptor))
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    assert_eq!(wire::read(&mut reader).expect("worker ready").0, b'R');
    let id = runtime.hello.instance_id.clone();
    assert!(
        runtime
            .request(
                id.clone(),
                protocol::Operation::Permissions {
                    worker_id: worker_id.clone(),
                    permissions: control::Permissions {
                        revision: 1,
                        clipboard: false,
                        browser: true
                    },
                }
            )
            .applied
            .unwrap()
            .permissions
            .browser
    );
    let root = runtime.integration_root(home.path());
    output.set_nonblocking(true).unwrap();
    loop {
        match output.write(&[0; 65536]) {
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => panic!("{error}"),
        }
    }
    wire::write(worker.stdin.as_mut().unwrap(), b'H', &[]).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let start = std::time::Instant::now();
    let reply = runtime.request(
        id.clone(),
        protocol::Operation::Permissions {
            worker_id,
            permissions: control::Permissions {
                revision: 2,
                clipboard: false,
                browser: false,
            },
        },
    );
    assert!(start.elapsed() < Duration::from_secs(3));
    assert!(
        reply.error.is_some(),
        "a blocked worker must not claim to apply permissions"
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while worker.try_wait().unwrap().is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "blocked worker did not clean up"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!root.join("agent.sock").exists());
    assert!(!root.join("features").exists());
    assert!(runtime
        .request(id, protocol::Operation::Ping)
        .error
        .is_none());
}
#[test]
fn missing_git_does_not_disable_other_capabilities_and_eof_stops_control() {
    let home = fixture_home();
    let mut runtime = Runtime::start(home.path(), "");
    assert!(!runtime.hello.dependencies["git"]);
    assert!(runtime.hello.require("git", 4).is_err());
    runtime.hello.require("integration", 1).unwrap();
    runtime.initialize();
    runtime.input.take();
    runtime.stopped();
}
#[test]
fn obsolete_runtime_identity_is_rejected() {
    let home = fixture_home();
    let mut runtime = Runtime::start(home.path(), "");
    runtime.initialize();
    assert!(runtime
        .request(uuid::Uuid::new_v4().to_string(), protocol::Operation::Ping)
        .error
        .unwrap()
        .contains("obsolete"));
    runtime.stopped();
}

#[test]
fn worker_admission_waits_for_a_fragmented_handshake() {
    use std::{io::Write, os::unix::net::UnixStream};
    let home = fixture_home();
    let mut runtime = Runtime::start(home.path(), "");
    runtime.initialize();
    let worker_id = runtime.register("integration", newport_agent::integration_control::VERSION);
    let mut stream = UnixStream::connect(&runtime.hello.worker_socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut request = Vec::new();
    protocol::write(
        &mut request,
        &protocol::Attach {
            runtime_id: runtime.hello.instance_id.clone(),
            worker_id: worker_id.clone(),
            build_sha256: runtime.hello.build_sha256.clone(),
            service: "integration".into(),
        },
    )
    .unwrap();
    // The listener must wait for input even when accept runs before the writer.
    std::thread::sleep(Duration::from_millis(100));
    stream.write_all(&request[..2]).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    stream.write_all(&request[2..]).unwrap();
    let reply: protocol::Attached = protocol::read(&mut stream).unwrap();
    assert!(reply.error.is_none(), "{:?}", reply.error);
    assert_eq!(reply.worker_id, worker_id);
    assert_eq!(reply.session_id.as_deref(), Some(runtime.session.as_str()));
}

#[test]
fn two_desktops_route_helpers_explicitly_without_crossing_sessions() {
    use newport_agent::{integration_control as control, wire};
    let home = fixture_home();
    let mut first = Runtime::start(home.path(), "");
    first.initialize();
    let mut second = Runtime::start(home.path(), "");
    second.initialize();
    let first_id = first.register("integration", control::VERSION);
    let second_id = second.register("integration", control::VERSION);
    let mut a = first.worker(&first_id, "integration", home.path());
    let mut b = second.worker(&second_id, "integration", home.path());
    let events = |worker: &mut Worker| {
        let mut stdout = worker.stdout.take().unwrap();
        let (send, receive) = mpsc::sync_channel(8);
        std::thread::spawn(move || {
            while let Ok(frame) = wire::read(&mut stdout) {
                if send.send(frame).is_err() {
                    break;
                }
            }
        });
        receive
    };
    let ea = events(&mut a);
    let eb = events(&mut b);
    let event = |events: &mpsc::Receiver<(u8, Vec<u8>)>, kind| {
        let (actual, data) = events.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(actual, kind, "{}", String::from_utf8_lossy(&data));
        data
    };
    event(&ea, b'R');
    event(&eb, b'R');
    for (runtime, worker) in [(&mut first, &first_id), (&mut second, &second_id)] {
        let id = runtime.hello.instance_id.clone();
        let applied = runtime.request(
            id,
            protocol::Operation::Permissions {
                worker_id: worker.clone(),
                permissions: control::Permissions {
                    revision: 1,
                    clipboard: true,
                    browser: true,
                },
            },
        );
        let applied = applied.applied.unwrap();
        assert!(applied.error.is_none(), "{:?}", applied.error);
        assert!(applied.permissions.clipboard && applied.permissions.browser);
    }
    let root_a = first.integration_root(home.path());
    let root_b = second.integration_root(home.path());
    assert_ne!(root_a, root_b);
    assert_ne!(
        std::fs::read_to_string(root_a.join("display")).unwrap(),
        std::fs::read_to_string(root_b.join("display")).unwrap()
    );
    let helper = |session: Option<&str>, args: &[&str]| {
        let mut command = Command::new(BIN);
        command
            .args(args)
            .env("HOME", home.path())
            .env("XDG_RUNTIME_DIR", home.path())
            .env_remove("NEWPORT_SESSION");
        if let Some(session) = session {
            command.env("NEWPORT_SESSION", session);
        }
        command.output().unwrap()
    };
    let ambiguous = helper(None, &["env"]);
    assert!(!ambiguous.status.success());
    assert!(String::from_utf8_lossy(&ambiguous.stderr).contains("Multiple Newport sessions"));
    for (runtime, worker, events, format) in [
        (&first, &mut a, &ea, "application/x-first"),
        (&second, &mut b, &eb, "application/x-second"),
    ] {
        wire::write(
            worker.stdin.as_mut().unwrap(),
            b'M',
            &rmp_serde::to_vec(&(42i64, vec![format])).unwrap(),
        )
        .unwrap();
        event(events, b'A');
        let result = helper(
            Some(&runtime.session),
            &["clipboard", "-o", "-t", "TARGETS"],
        );
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(String::from_utf8(result.stdout).unwrap().trim(), format);
        let env = helper(Some(&runtime.session), &["env"]);
        assert!(env.status.success());
        assert!(String::from_utf8_lossy(&env.stdout)
            .contains(&format!("NEWPORT_SESSION='{}'", runtime.session)));
    }
    // Browser requests must arrive at the selected desktop only.
    let mut opener = Worker(
        Command::new(BIN)
            .args(["open", "https://example.test/first"])
            .env("HOME", home.path())
            .env("XDG_RUNTIME_DIR", home.path())
            .env("NEWPORT_SESSION", &first.session)
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let (request, url): (u64, String) = rmp_serde::from_slice(&event(&ea, b'O')).unwrap();
    assert_eq!(url, "https://example.test/first");
    assert!(eb.try_recv().is_err());
    wire::write(
        a.stdin.as_mut().unwrap(),
        b'B',
        &wire::browser_reply(request, true, None).unwrap(),
    )
    .unwrap();
    assert!(opener.wait().unwrap().success());
    let id = first.hello.instance_id.clone();
    first.request(
        id.clone(),
        protocol::Operation::Permissions {
            worker_id: first_id,
            permissions: control::Permissions {
                revision: 2,
                clipboard: false,
                browser: false,
            },
        },
    );
    assert!(!helper(Some(&first.session), &["env"]).status.success());
    let only_second = helper(None, &["env"]);
    assert!(only_second.status.success());
    assert!(String::from_utf8_lossy(&only_second.stdout).contains(&second.session));
    first.request(id, protocol::Operation::Shutdown);
    first.stopped();
    // A dead explicit selector cannot silently switch to the remaining desktop.
    let stale = helper(Some(&first.session), &["clipboard", "-o", "-t", "TARGETS"]);
    assert!(!stale.status.success());
    let error = String::from_utf8_lossy(&stale.stderr);
    assert!(error.contains("Selected Newport session"), "{error}");
    assert!(error.contains(&first.session), "{error}");
    assert!(!error.contains("Enable it in Newport"), "{error}");
    assert!(
        helper(Some(&second.session), &["clipboard", "-o", "-t", "TARGETS"])
            .status
            .success()
    );
    // Relaunching the same desktop preserves the terminal's routing selector,
    // but the new worker must receive fresh permissions before it can serve it.
    let mut restarted = Runtime::start(home.path(), "");
    restarted.session = first.session.clone();
    restarted.client = first.client.clone();
    restarted.initialize();
    let worker_id = restarted.register("integration", control::VERSION);
    let mut worker = restarted.worker(&worker_id, "integration", home.path());
    let restarted_events = events(&mut worker);
    event(&restarted_events, b'R');
    assert!(!helper(Some(&first.session), &["env"]).status.success());
    let reply = restarted.request(
        restarted.hello.instance_id.clone(),
        protocol::Operation::Permissions {
            worker_id,
            permissions: control::Permissions {
                revision: 1,
                clipboard: true,
                browser: true,
            },
        },
    );
    assert!(reply.applied.unwrap().error.is_none());
    let mut opener = Worker(
        Command::new(BIN)
            .args(["open", "https://example.test/restarted"])
            .env("HOME", home.path())
            .env("XDG_RUNTIME_DIR", home.path())
            .env("NEWPORT_SESSION", &first.session)
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let (request, url): (u64, String) =
        rmp_serde::from_slice(&event(&restarted_events, b'O')).unwrap();
    assert_eq!(url, "https://example.test/restarted");
    assert!(eb.try_recv().is_err());
    wire::write(
        worker.stdin.as_mut().unwrap(),
        b'B',
        &wire::browser_reply(request, true, None).unwrap(),
    )
    .unwrap();
    assert!(opener.wait().unwrap().success());
}
#[test]
fn workers_require_the_negotiated_build_before_serving_feature_protocols() {
    let home = fixture_home();
    let mut runtime = Runtime::start(home.path(), &std::env::var("PATH").unwrap());
    runtime.initialize();
    let worker_id = runtime.register("git", 4);
    let rejected = Command::new(BIN)
        .args(["worker", &"0".repeat(64), "git"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("build changed"));
    let mut worker = runtime.worker(&worker_id, "git", home.path());
    let mut output = worker.stdout.take().unwrap();
    let (send, receive) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = send.send(newport_agent::git::protocol::read(&mut output));
    });
    let result = receive.recv_timeout(Duration::from_secs(10));
    assert!(matches!(
        result.unwrap().unwrap(),
        newport_agent::git::protocol::Message::Hello { .. }
    ));
    // The same registration cannot be consumed twice, even while the first
    // worker is sitting idle with its feature stdin still open.
    let mut duplicate = runtime.worker(&worker_id, "git", home.path());
    assert!(!duplicate.wait().unwrap().success());
    let socket = runtime.hello.worker_socket.clone();
    let id = runtime.hello.instance_id.clone();
    assert!(runtime
        .request(id, protocol::Operation::Shutdown)
        .error
        .is_none());
    runtime.stopped();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if worker.try_wait().unwrap().is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            let _ = worker.kill();
            let _ = worker.wait();
            panic!("registered worker survived its control session with stdin still open");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!std::path::Path::new(&socket).exists());
    // No files are copied by the worker launcher.
    assert!(!home.path().join(".local/bin/newport-agent").exists());
}

#[test]
fn registration_limit_does_not_kill_control_or_lose_liveness() {
    let home = fixture_home();
    let mut runtime = Runtime::start(home.path(), "");
    runtime.initialize();
    for _ in 0..protocol::MAX_WORKERS {
        runtime.register("integration", 1);
    }
    let id = runtime.hello.instance_id.clone();
    assert!(runtime
        .request(
            id.clone(),
            protocol::Operation::Register {
                worker_id: uuid::Uuid::new_v4().to_string(),
                service: "integration".into(),
                version: 1,
            }
        )
        .error
        .unwrap()
        .contains("limit"));
    assert!(runtime
        .request(id.clone(), protocol::Operation::Ping)
        .error
        .is_none());
    assert!(runtime
        .request(id, protocol::Operation::Shutdown)
        .error
        .is_none());
    runtime.stopped();
}
#[test]
fn registration_cannot_be_used_in_another_runtime() {
    let home = fixture_home();
    let path = std::env::var("PATH").unwrap();
    let mut first = Runtime::start(home.path(), &path);
    first.initialize();
    let mut second = Runtime::start(home.path(), &path);
    second.initialize();
    let worker = first.register("git", 4);
    let mut wrong = second.worker(&worker, "git", home.path());
    assert!(!wrong.wait().unwrap().success());
    assert!(first
        .request(first.hello.instance_id.clone(), protocol::Operation::Ping)
        .error
        .is_none());
}

fn start_integration(runtime: &mut Runtime, home: &std::path::Path) -> Worker {
    let registration = runtime.register("integration", newport_agent::integration_control::VERSION);
    let mut worker = runtime.worker(&registration, "integration", home);
    assert_eq!(
        newport_agent::wire::read(worker.stdout.as_mut().unwrap())
            .unwrap()
            .0,
        b'R'
    );
    worker
}
fn runtime_path(runtime: &Runtime) -> std::path::PathBuf {
    std::path::Path::new(&runtime.hello.worker_socket)
        .parent()
        .unwrap()
        .into()
}
fn records(home: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_dir(home.join(".cache/newport/sessions"))
        .unwrap()
        .map(|entry| {
            serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap()
        })
        .collect()
}

#[test]
fn crashed_runtime_cleanup_preserves_live_workers_and_other_clients() {
    let home = fixture_home();
    let mut crashed = Runtime::start(home.path(), "");
    crashed.initialize();
    let mut stranded = start_integration(&mut crashed, home.path());
    let crashed_path = runtime_path(&crashed);
    let stranded_root = crashed.integration_root(home.path());
    let mut live = Runtime::start(home.path(), "");
    live.initialize();
    let mut live_worker = start_integration(&mut live, home.path());
    let live_root = live.integration_root(home.path());
    // A paused worker cannot respond to control loss or perform Rust cleanup.
    // Its kernel-held directory lease must still protect its files.
    assert_eq!(
        unsafe { libc::kill(stranded.id() as i32, libc::SIGSTOP) },
        0
    );
    crashed.child.kill().unwrap();
    crashed.child.wait().unwrap();
    let mut next = Runtime::start(home.path(), "");
    next.initialize();
    assert!(stranded_root.join("agent.sock").exists());
    assert!(live_root.join("agent.sock").exists());
    stranded.kill().unwrap();
    stranded.wait().unwrap();
    let mut replacement = Runtime::start(home.path(), "");
    replacement.initialize();
    assert!(!crashed_path.exists(), "dead runtime directory retained");
    let _replacement_worker = start_integration(&mut replacement, home.path());
    let remaining = records(home.path());
    assert_eq!(remaining.len(), 2, "dead registry record retained");
    assert!(remaining
        .iter()
        .any(|r| r["runtime"] == live.hello.instance_id));
    // The disabled-but-live bridge must retain its publication and endpoint.
    assert!(live_worker.try_wait().unwrap().is_none());
    assert!(live_root.join("agent.sock").exists());
    let id = live.hello.instance_id.clone();
    assert!(live.request(id, protocol::Operation::Ping).error.is_none());
}

#[test]
fn crashed_integration_directory_is_reclaimed_with_control_still_running() {
    let home = fixture_home();
    let mut runtime = Runtime::start(home.path(), "");
    runtime.initialize();
    let mut crashed = start_integration(&mut runtime, home.path());
    let abandoned = runtime.integration_root(home.path());
    std::fs::write(abandoned.join("snapshot.tar"), b"stale clipboard bytes").unwrap();
    crashed.kill().unwrap();
    crashed.wait().unwrap();
    assert!(abandoned.exists(), "SIGKILL unexpectedly ran cleanup");
    let _replacement = start_integration(&mut runtime, home.path());
    assert!(!abandoned.exists(), "dead integration content retained");
    assert_eq!(records(home.path()).len(), 1);
    assert!(runtime_path(&runtime).exists());
    let id = runtime.hello.instance_id.clone();
    assert!(runtime
        .request(id, protocol::Operation::Ping)
        .error
        .is_none());
}

#[test]
fn runtime_start_reclaims_crashed_x11_socket_and_authority_without_touching_live_display() {
    use std::{
        io::{BufRead, BufReader},
        path::PathBuf,
    };
    fn display(home: &std::path::Path) -> (Worker, PathBuf, PathBuf) {
        let mut child = Worker(
            Command::new(BIN)
                .args(["display", "--backend", "x11"])
                .env("HOME", home)
                .env("XDG_RUNTIME_DIR", home)
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        let output = child.stdout.take().unwrap();
        let (send, lines) = mpsc::sync_channel(8);
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines().map_while(Result::ok) {
                // The caller only needs DISPLAY and XAUTHORITY. Keep draining
                // later environment lines after it drops the receiver, so the
                // live display does not exit because stdout was closed early.
                let _ = send.send(line);
            }
        });
        let mut socket = None;
        let mut authority = None;
        while socket.is_none() || authority.is_none() {
            let line = lines.recv_timeout(Duration::from_secs(5)).unwrap();
            if let Some(number) = line
                .strip_prefix("export DISPLAY=':")
                .and_then(|s| s.strip_suffix('\''))
            {
                socket = Some(PathBuf::from(format!("/tmp/.X11-unix/X{number}")));
            }
            if let Some(path) = line
                .strip_prefix("export XAUTHORITY='")
                .and_then(|s| s.strip_suffix('\''))
            {
                authority = Some(PathBuf::from(path));
            }
        }
        (child, socket.unwrap(), authority.unwrap())
    }
    let home = fixture_home();
    let (mut crashed, socket, authority) = display(home.path());
    let (mut live, live_socket, live_authority) = display(home.path());
    assert_ne!(socket, live_socket);
    crashed.kill().unwrap();
    crashed.wait().unwrap();
    assert!(socket.exists());
    assert!(authority.exists());
    let _runtime = Runtime::start(home.path(), "");
    assert!(!socket.exists(), "crashed X11 public socket retained");
    assert!(
        !authority.parent().unwrap().exists(),
        "crashed Xauthority retained"
    );
    assert!(live.try_wait().unwrap().is_none());
    assert!(live_socket.exists());
    assert!(live_authority.exists());
    assert!(std::os::unix::net::UnixStream::connect(&live_socket).is_ok());
    live.kill().unwrap();
    live.wait().unwrap();
    let _cleanup = Runtime::start(home.path(), "");
    assert!(!live_socket.exists());
    assert!(!live_authority.parent().unwrap().exists());
}

#[test]
fn runtime_start_reclaims_crashed_standalone_wayland_directory() {
    use std::io::{BufRead, BufReader};
    let home = fixture_home();
    let mut display = Worker(
        Command::new(BIN)
            .args(["display", "--backend", "wayland"])
            .env("HOME", home.path())
            .env("XDG_RUNTIME_DIR", home.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let output = display.stdout.take().unwrap();
    let (send, line) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut lines = BufReader::new(output).lines();
        let first = lines.next().unwrap().unwrap();
        let _ = send.send(first);
        // Keep stdout open while the display prints the remaining environment.
        for _ in lines {}
    });
    let line = line.recv_timeout(Duration::from_secs(5)).unwrap();
    let socket = std::path::PathBuf::from(
        line.strip_prefix("export WAYLAND_DISPLAY='")
            .and_then(|v| v.strip_suffix('\''))
            .unwrap(),
    );
    let _before = Runtime::start(home.path(), "");
    assert!(socket.exists(), "runtime removed a live Wayland socket");
    display.kill().unwrap();
    display.wait().unwrap();
    assert!(socket.exists());
    let _after = Runtime::start(home.path(), "");
    assert!(!socket.parent().unwrap().exists());
}

#[test]
fn reconnect_reconciles_old_selectors_and_retires_only_the_replaced_client() {
    use newport_agent::integration_control as control;
    let home = fixture_home();
    let mut old = Runtime::start(home.path(), "");
    old.initialize();
    let mut old_worker = start_integration(&mut old, home.path());
    let mut other = Runtime::start(home.path(), "");
    other.initialize();
    let mut other_worker = start_integration(&mut other, home.path());
    let mut next = Runtime::start(home.path(), "");
    next.client = old.client.clone();
    next.initialize();
    let id = next.register("integration", control::VERSION);
    let mut worker = next.worker(&id, "integration", home.path());
    assert_eq!(
        newport_agent::wire::read(worker.stdout.as_mut().unwrap())
            .unwrap()
            .0,
        b'R'
    );
    let helper = |selector: &str| {
        Command::new(BIN)
            .args(["env", "--shell", "zsh"])
            .env("HOME", home.path())
            .env("XDG_RUNTIME_DIR", home.path())
            .env("NEWPORT_SESSION", selector)
            .output()
            .unwrap()
    };
    // Alias resolution must not grant permissions or use the old endpoint.
    assert!(!helper(&old.session).status.success());
    assert!(next
        .request(
            next.hello.instance_id.clone(),
            protocol::Operation::Permissions {
                worker_id: id.clone(),
                permissions: control::Permissions {
                    revision: 1,
                    browser: true,
                    clipboard: true
                }
            }
        )
        .applied
        .unwrap()
        .error
        .is_none());
    let result = helper(&old.session);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains(&next.session));
    // Retiring the old worker must not remove the replacement's publication.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while old_worker.try_wait().unwrap().is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "superseded worker was not retired"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(other_worker.try_wait().unwrap().is_none());
    assert!(helper(&old.session).status.success());
    let listed = Command::new(BIN)
        .arg("sessions")
        .env("HOME", home.path())
        .env("XDG_RUNTIME_DIR", home.path())
        .output()
        .unwrap();
    assert!(listed.status.success());
    let listed = String::from_utf8(listed.stdout).unwrap();
    assert!(!listed.contains(&old.session));
    assert!(listed.contains(&next.session) && listed.contains(&other.session));
    assert!(next
        .request(
            next.hello.instance_id.clone(),
            protocol::Operation::Permissions {
                worker_id: id,
                permissions: control::Permissions {
                    revision: 2,
                    browser: false,
                    clipboard: false
                }
            }
        )
        .applied
        .unwrap()
        .error
        .is_none());
    assert!(!helper(&old.session).status.success());
}

#[test]
fn unknown_selector_recovers_only_a_single_known_client_and_remembers_the_binding() {
    use newport_agent::integration_control as control;
    let home = fixture_home();
    let mut runtime = Runtime::start(home.path(), "");
    runtime.initialize();
    let worker_id = runtime.register("integration", control::VERSION);
    let mut worker = runtime.worker(&worker_id, "integration", home.path());
    assert_eq!(
        newport_agent::wire::read(worker.stdout.as_mut().unwrap())
            .unwrap()
            .0,
        b'R'
    );
    let helper = |id: &str, args: &[&str]| {
        Command::new(BIN)
            .args(args)
            .env("HOME", home.path())
            .env("XDG_RUNTIME_DIR", home.path())
            .env("NEWPORT_SESSION", id)
            .output()
            .unwrap()
    };
    let old = uuid::Uuid::new_v4().to_string();
    assert!(
        !helper(&old, &["env"]).status.success(),
        "must not recover disabled integrations"
    );
    assert!(!helper("invalid-id", &["env"]).status.success());
    assert!(runtime
        .request(
            runtime.hello.instance_id.clone(),
            protocol::Operation::Permissions {
                worker_id: worker_id.clone(),
                permissions: control::Permissions {
                    revision: 1,
                    clipboard: true,
                    browser: false
                }
            }
        )
        .applied
        .unwrap()
        .error
        .is_none());
    assert!(!helper(&old, &["open", "https://example.test/disabled"])
        .status
        .success());
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let helper = &helper;
            let old = &old;
            scope.spawn(move || {
                let result = helper(old, &["env"]);
                assert!(
                    result.status.success(),
                    "{}",
                    String::from_utf8_lossy(&result.stderr)
                );
            });
        }
    });
    let recovered = helper(&old, &["env", "--shell", "zsh"]);
    assert!(
        recovered.status.success(),
        "{}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    assert!(String::from_utf8_lossy(&recovered.stdout).contains(&runtime.session));
    // Once bound, the unchanged old shell can still resolve its own client when
    // another desktop joins. A fresh unknown selector must remain ambiguous.
    let mut other = Runtime::start(home.path(), "");
    other.initialize();
    let _other_worker = start_integration(&mut other, home.path());
    assert!(helper(&old, &["env"]).status.success());
    let unknown = uuid::Uuid::new_v4().to_string();
    assert!(!helper(&unknown, &["env"]).status.success());
    other.request(
        other.hello.instance_id.clone(),
        protocol::Operation::Shutdown,
    );
    other.stopped();
    assert!(
        !helper(&unknown, &["env"]).status.success(),
        "an offline other desktop still makes guessing unsafe"
    );
    assert!(runtime
        .request(
            runtime.hello.instance_id.clone(),
            protocol::Operation::Permissions {
                worker_id,
                permissions: control::Permissions {
                    revision: 2,
                    clipboard: false,
                    browser: false
                }
            }
        )
        .applied
        .unwrap()
        .error
        .is_none());
    assert!(!helper(&old, &["env"]).status.success());
}
