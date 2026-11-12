#[path = "support/runtime_reply.rs"]
mod runtime_reply;
use newport_protocol::runtime::{self as protocol, wire as codec};
use newport_protocol::{
    clipboard::imports,
    integration::{
        self, BrowserReply, BrowserRequest, Call as IntegrationCall, ClientMessage,
        Request as IntegrationRequest, ServerMessage,
    },
};
fn configure_git(input: &mut impl std::io::Write) {
    use newport_protocol::git::{ClientMessage, Configure, wire};
    wire::write(
        input,
        &ClientMessage::Configure(Configure {
            id: uuid::Uuid::new_v4().to_string(),
            client_id: uuid::Uuid::new_v4().to_string(),
            client_version: "test".into(),
            command_logs: false,
        }),
    )
    .unwrap();
}
fn integration_call(request: IntegrationRequest) -> IntegrationCall {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    IntegrationCall {
        id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        request,
    }
}
fn integration_ready(input: &mut impl std::io::Write, output: &mut impl std::io::Read) {
    let call = integration_call(IntegrationRequest::Heartbeat);
    integration::wire::write(input, &ClientMessage::Call(call.clone())).unwrap();
    let ServerMessage::Response(response) = integration::wire::read(output).unwrap() else {
        panic!("expected heartbeat response")
    };
    response.validate_for(&call).unwrap();
}
impl Worker {
    fn ready(&mut self) {
        integration_ready(
            self.0.stdin.as_mut().unwrap(),
            self.0.stdout.as_mut().unwrap(),
        );
    }
}
use runtime_reply::Expected;
#[path = "support/worker_stream.rs"]
mod worker_stream;
#[path = "support/x11.rs"]
mod x11;
use std::{
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

/// Newport waits this long for Needle's startup and replies; a slow runner
/// can exceed a tighter limit without anything being wrong.
const REPLY: Duration = Duration::from_secs(25);
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
/// `env` without a connection it may use: it still succeeds, with only the
/// stable part (Needle's commands on PATH), so a shell that starts now still
/// reaches them later.
fn stable_only(output: &std::process::Output) -> bool {
    let stdout = String::from_utf8_lossy(&output.stdout);
    output.status.success()
        && stdout.contains("/.local/bin")
        && !stdout.contains("DISPLAY")
        && !output.stderr.is_empty()
}
/// A plain terminal on the Mac whose integration session is `session`, as
/// its route names it.
fn routed_terminal(home: &std::path::Path, session: &str, name: &str) -> String {
    let terminal = uuid::Uuid::new_v4().to_string();
    let status = Command::new(BIN)
        .args(["terminal", "route", &terminal, session])
        .arg(name.bytes().map(|b| format!("{b:02x}")).collect::<String>())
        .env("HOME", home)
        .env("XDG_RUNTIME_DIR", home)
        .status()
        .unwrap();
    assert!(status.success());
    terminal
}
/// The Git worker's protocol.
const GIT: u32 = newport_protocol::git::VERSION;
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
    frames: mpsc::Receiver<protocol::Reply>,
    sequence: u64,
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
        let (announced, announcement) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let hello: protocol::Hello = codec::read(&mut output).unwrap();
            announced.send(hello).unwrap();
            while let Ok(frame) = codec::read::<protocol::Reply>(&mut output) {
                if send.send(frame).is_err() {
                    break;
                }
            }
        });
        let hello = announcement
            .recv_timeout(Duration::from_secs(15))
            .expect("hello");
        hello.validate().unwrap();
        assert_eq!(hello.agent_version.as_deref(), Some(newport_agent::VERSION));
        Self {
            session: uuid::Uuid::new_v4().to_string(),
            client: uuid::Uuid::new_v4().to_string(),
            child,
            input,
            frames,
            sequence: 0,
            hello,
        }
    }
    fn request(&mut self, runtime_id: String, operation: protocol::Operation) -> protocol::Reply {
        self.sequence += 1;
        let request = protocol::Request {
            id: self.sequence,
            runtime_id,
            operation,
        };
        codec::write(self.input.as_mut().unwrap(), &request).unwrap();
        let reply = self.frames.recv_timeout(REPLY).unwrap();
        assert_eq!(reply.id, request.id);
        reply
    }
    fn initialize(&mut self) {
        let id = self.hello.instance_id.clone();
        assert!(
            self.request(
                id,
                protocol::Operation::Initialize {
                    version: protocol::VERSION,
                    session_id: self.session.clone(),
                    transport_generation: 1,
                }
            )
            .result
            .is_ok()
        );
    }
    fn register(&mut self, service: &str, version: u32) -> String {
        let worker = uuid::Uuid::new_v4().to_string();
        let id = self.hello.instance_id.clone();
        assert!(
            self.request(
                id,
                protocol::Operation::PrepareWorkerStream {
                    worker_id: worker.clone(),
                    service: service.into(),
                    contract_id: protocol::service_contract_id(service).map(str::to_owned),
                    version
                }
            )
            .result
            .is_ok()
        );
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
fn monitor_contract_mismatch_is_refused_before_worker_registration() {
    let home = fixture_home();
    let mut runtime = Runtime::start(home.path(), "");
    runtime.initialize();
    assert_eq!(
        runtime.hello.services["monitor"].contract_id.as_deref(),
        Some(newport_protocol::monitor::CONTRACT_ID)
    );
    let worker = uuid::Uuid::new_v4().to_string();
    for contract_id in [None, Some("0".repeat(64))] {
        let reply = runtime.request(
            runtime.hello.instance_id.clone(),
            protocol::Operation::PrepareWorkerStream {
                worker_id: worker.clone(),
                service: "monitor".into(),
                version: 1,
                contract_id,
            },
        );
        let error = reply.result.unwrap_err();
        assert_eq!(error.code, "incompatible");
        assert_eq!(error.retry, newport_protocol::common::Retry::No);
        assert!(
            !std::path::Path::new(&runtime.hello.worker_socket)
                .parent()
                .unwrap()
                .join("streams.sock")
                .exists()
        );
    }
    let reply = runtime.request(
        runtime.hello.instance_id.clone(),
        protocol::Operation::PrepareWorkerStream {
            worker_id: worker,
            service: "monitor".into(),
            version: 1,
            contract_id: Some(newport_protocol::monitor::CONTRACT_ID.into()),
        },
    );
    if cfg!(target_os = "linux") {
        assert!(reply.result.is_ok(), "{:?}", reply.result);
    } else {
        let error = reply.result.unwrap_err();
        assert_eq!(error.code, "unavailable");
        assert_eq!(error.retry, newport_protocol::common::Retry::No);
        assert_eq!(error.message, "Monitoring requires Linux.");
        assert!(
            !std::path::Path::new(&runtime.hello.worker_socket)
                .parent()
                .unwrap()
                .join("streams.sock")
                .exists()
        );
    }
}

#[test]
fn control_eof_revokes_an_idle_integration_and_cleans_its_endpoints() {
    use newport_protocol::integration::{self as control, wire};
    let home = fixture_home();
    let mut runtime = Runtime::start(home.path(), "");
    runtime.initialize();
    let registration = runtime.register("integration", control::VERSION);
    let mut worker = runtime.worker(&registration, "integration", home.path());
    worker.ready();
    let mut output = worker.stdout.take().unwrap();
    let (send, _events) = mpsc::sync_channel(4);
    std::thread::spawn(move || {
        while let Ok(event) = wire::read::<ServerMessage>(&mut output) {
            if send.send(event).is_err() {
                break;
            }
        }
    });
    let runtime_id = runtime.hello.instance_id.clone();
    let applied = runtime.request(
        runtime_id,
        protocol::Operation::Permissions {
            worker_id: registration,
            permissions: newport_protocol::runtime::Permissions {
                revision: 1,
                clipboard: false,
                browser: true,
            },
        },
    );
    assert!(
        applied.result.is_ok(),
        "{:?}",
        applied.result.as_ref().err()
    );
    assert!(applied.result.unwrap().permissions().permissions.browser);
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

#[cfg(target_os = "linux")]
#[test]
fn terminal_workers_exchange_typed_lists_and_mutation_results() {
    use newport_protocol::terminal::{self, Reply, Request, wire};
    use std::io::Write;
    let home = fixture_home();
    let mut runtime = Runtime::start(home.path(), "/usr/bin:/bin");
    runtime.initialize();
    let missing = uuid::Uuid::new_v4().simple().to_string();
    for (request, expected) in [
        (Request::List, Reply::Listed(Vec::new())),
        (Request::Watch, Reply::Listed(Vec::new())),
        (
            Request::Rename {
                id: missing.clone(),
                name: "Build".into(),
            },
            Reply::Renamed { renamed: false },
        ),
        (Request::End { id: missing }, Reply::Ended { ended: false }),
    ] {
        let id = runtime.register("terminal", terminal::VERSION);
        let mut worker = runtime.worker(&id, "terminal", home.path());
        let mut input = worker.stdin.take().unwrap();
        input
            .write_all(&wire::encode_request(&request).unwrap())
            .unwrap();
        drop(input);
        let mut output = worker.stdout.take().unwrap();
        let (send, receive) = mpsc::sync_channel(1);
        std::thread::spawn(move || send.send(wire::read_reply(&mut output)).unwrap());
        assert_eq!(receive.recv_timeout(REPLY).unwrap().unwrap(), expected);
        let deadline = std::time::Instant::now() + REPLY;
        loop {
            if let Some(status) = worker.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "terminal worker did not exit"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
#[cfg(target_os = "linux")]
#[test]
fn monitor_worker_correlates_replies_and_survives_a_failed_action() {
    use newport_protocol::{
        common::Retry,
        monitor::{self, Call, Reply, Request, compose, stable_display, wire},
    };
    use std::io::Write;

    let home = fixture_home();
    let mut runtime = Runtime::start(home.path(), "/usr/bin:/bin");
    runtime.initialize();
    let id = runtime.register("monitor", monitor::VERSION);
    let mut worker = runtime.worker(&id, "monitor", home.path());
    let calls: Vec<_> = [
        Request::Compose(compose::Request::Jobs),
        Request::Compose(compose::Request::Cancel {
            job: uuid::Uuid::new_v4().simple().to_string(),
        }),
        Request::StableDisplay,
        Request::Metrics,
        Request::Compose(compose::Request::Jobs),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, request)| Call {
        id: 31 + index as u64,
        request,
    })
    .collect();
    let mut input = worker.stdin.take().unwrap();
    let mut output = worker.stdout.take().unwrap();
    let expected = calls.clone();
    let (send, receive) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        for call in expected {
            if send.send(wire::read_reply(&mut output, &call)).is_err() {
                break;
            }
        }
    });
    for call in calls {
        input
            .write_all(&wire::encode_request(&call).unwrap())
            .unwrap();
        input.flush().unwrap();
        let response = receive.recv_timeout(REPLY).unwrap().unwrap();
        assert_eq!(response.id, call.id);
        match (call.request, response.reply) {
            (Request::Compose(compose::Request::Jobs), Reply::Compose(compose::Reply::Jobs(v))) => {
                assert!(v.jobs.is_empty());
            }
            (Request::Compose(compose::Request::Cancel { .. }), Reply::Failed(error)) => {
                assert_eq!(error.code, "compose_failed");
                assert_eq!(error.retry, Retry::No);
            }
            (Request::StableDisplay, Reply::StableDisplay(v)) => {
                assert_eq!(v, stable_display::State::default());
            }
            (Request::Metrics, Reply::Metrics(_)) => {}
            unexpected => panic!("unexpected monitor result: {unexpected:?}"),
        }
    }
    drop(input);
    let deadline = std::time::Instant::now() + REPLY;
    loop {
        if let Some(status) = worker.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "monitor worker did not exit"
        );
        std::thread::sleep(Duration::from_millis(10));
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
    assert!(
        runtime
            .request(id.clone(), protocol::Operation::Ping)
            .result
            .is_ok()
    );
    assert!(
        runtime
            .request(id, protocol::Operation::Shutdown)
            .result
            .is_ok()
    );
    runtime.stopped();
    assert!(!home.path().join(".local/bin").exists());
    assert!(!home.path().join(".local/share/newport/needle").exists());
}

#[test]
fn permission_revocation_bypasses_an_incomplete_feature_upload() {
    use newport_protocol::integration as control;
    use std::io::Write;
    let home = fixture_home();
    let mut runtime = Runtime::start(home.path(), &std::env::var("PATH").unwrap());
    runtime.initialize();
    let worker_id = runtime.register("integration", control::VERSION);
    let mut worker = runtime.worker(&worker_id, "integration", home.path());
    worker.ready();
    let id = runtime.hello.instance_id.clone();
    let enable = runtime.request(
        id.clone(),
        protocol::Operation::Permissions {
            worker_id: worker_id.clone(),
            permissions: newport_protocol::runtime::Permissions {
                revision: 1,
                clipboard: false,
                browser: true,
            },
        },
    );
    assert!(enable.result.unwrap().permissions().permissions.browser);
    // Declare a feature frame and stop in its header. The feature reader will
    // wait indefinitely; permission traffic must use the separate connection.
    worker.stdin.as_mut().unwrap().write_all(&[128]).unwrap();
    let start = std::time::Instant::now();
    let disable = runtime.request(
        id.clone(),
        protocol::Operation::Permissions {
            worker_id,
            permissions: newport_protocol::runtime::Permissions {
                revision: 2,
                clipboard: false,
                browser: false,
            },
        },
    );
    assert!(
        disable.result.is_ok(),
        "{:?}",
        disable.result.as_ref().err()
    );
    assert!(!disable.result.unwrap().permissions().permissions.browser);
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(
        std::fs::read_to_string(runtime.integration_root(home.path()).join("features"))
            .unwrap()
            .trim(),
        "clipboard-local"
    );
    // The runtime broker remains reachable locally with host sharing off.
    // Its empty selection must not expose or fetch the host's clipboard.
    for target in ["TARGETS", "text/plain"] {
        let result = Command::new(BIN)
            .args(["clipboard", "-o", "-t", target])
            .env("HOME", home.path())
            .env("XDG_RUNTIME_DIR", home.path())
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("WAYLAND_SOCKET")
            .output()
            .unwrap();
        assert_eq!(result.status.success(), target == "TARGETS", "{result:?}");
        assert!(result.stdout.is_empty());
    }
    assert!(worker.try_wait().unwrap().is_none());
    // Revocation is scoped to this worker; the runtime can still admit Git.
    let git_id = runtime.register("git", GIT);
    let mut git = runtime.worker(&git_id, "git", home.path());
    configure_git(git.stdin.as_mut().unwrap());
    assert!(matches!(
        newport_protocol::git::wire::read::<newport_protocol::git::ServerMessage>(
            git.stdout.as_mut().unwrap()
        )
        .unwrap(),
        newport_protocol::git::ServerMessage::Ready(..)
    ));
    assert!(
        runtime
            .request(id, protocol::Operation::Ping)
            .result
            .is_ok()
    );
}

#[test]
fn stalled_feature_output_cannot_prevent_revocation_or_worker_cleanup() {
    use newport_protocol::integration as control;
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
    integration_ready(worker.stdin.as_mut().unwrap(), &mut reader);
    let id = runtime.hello.instance_id.clone();
    assert!(
        runtime
            .request(
                id.clone(),
                protocol::Operation::Permissions {
                    worker_id: worker_id.clone(),
                    permissions: newport_protocol::runtime::Permissions {
                        revision: 1,
                        clipboard: false,
                        browser: true
                    },
                }
            )
            .result
            .unwrap()
            .permissions()
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
    integration::wire::write(
        worker.stdin.as_mut().unwrap(),
        &ClientMessage::Call(integration_call(IntegrationRequest::Heartbeat)),
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let start = std::time::Instant::now();
    let reply = runtime.request(
        id.clone(),
        protocol::Operation::Permissions {
            worker_id,
            permissions: newport_protocol::runtime::Permissions {
                revision: 2,
                clipboard: false,
                browser: false,
            },
        },
    );
    assert!(start.elapsed() < Duration::from_secs(3));
    assert!(
        reply.result.is_err(),
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
    assert!(
        runtime
            .request(id, protocol::Operation::Ping)
            .result
            .is_ok()
    );
}
#[test]
fn missing_git_does_not_disable_other_capabilities_and_eof_stops_control() {
    let home = fixture_home();
    let mut runtime = Runtime::start(home.path(), "");
    assert!(!runtime.hello.dependencies["git"]);
    assert!(runtime.hello.require("git", GIT).is_err());
    runtime.hello.require("integration", 1).unwrap();
    runtime
        .hello
        .require(
            newport_protocol::browser::CAPABILITY,
            newport_protocol::browser::VERSION,
        )
        .unwrap();
    runtime.initialize();
    runtime.input.take();
    runtime.stopped();
}
#[test]
fn obsolete_runtime_identity_is_rejected() {
    let home = fixture_home();
    let mut runtime = Runtime::start(home.path(), "");
    runtime.initialize();
    assert!(
        runtime
            .request(uuid::Uuid::new_v4().to_string(), protocol::Operation::Ping)
            .result
            .as_ref()
            .err()
            .unwrap()
            .message
            .contains("obsolete")
    );
    runtime.stopped();
}

#[test]
fn worker_admission_waits_for_a_fragmented_handshake() {
    use std::{io::Write, os::unix::net::UnixStream};
    let home = fixture_home();
    let mut runtime = Runtime::start(home.path(), "");
    runtime.initialize();
    let worker_id = runtime.register("integration", integration::VERSION);
    let mut stream = UnixStream::connect(&runtime.hello.worker_socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut request = Vec::new();
    newport_protocol::runtime::wire::write(
        &mut request,
        &newport_protocol::runtime::Attach {
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
    let reply: newport_protocol::runtime::Attached =
        newport_protocol::runtime::wire::read(&mut stream).unwrap();
    assert!(reply.result.is_ok(), "{:?}", reply.result);
    assert_eq!(reply.worker_id, worker_id);
    assert_eq!(reply.result.unwrap().session_id, runtime.session);
}

#[test]
fn malformed_lease_frames_leave_the_registered_worker_available() {
    use newport_protocol::runtime::{self as lease, wire};
    use std::{
        io::{Read, Write},
        os::unix::net::UnixStream,
    };
    let home = fixture_home();
    let mut runtime = Runtime::start(home.path(), "");
    runtime.initialize();
    let worker_id = runtime.register("integration", integration::VERSION);
    let request = lease::Attach {
        runtime_id: runtime.hello.instance_id.clone(),
        worker_id: worker_id.clone(),
        build_sha256: runtime.hello.build_sha256.clone(),
        service: "integration".into(),
    };
    let mut wrong_direction = Vec::new();
    wire::write(
        &mut wrong_direction,
        &lease::WorkerPermissions {
            runtime_id: request.runtime_id.clone(),
            worker_id: request.worker_id.clone(),
            permissions: lease::Permissions {
                revision: 1,
                clipboard: false,
                browser: false,
            },
        },
    )
    .unwrap();
    // A length beyond the limit must be rejected without waiting for its body.
    for bytes in [wrong_direction, vec![0xff, 0xff, 0x7f]] {
        let mut stream = UnixStream::connect(&runtime.hello.worker_socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream.write_all(&bytes).unwrap();
        let mut byte = [0];
        match stream.read(&mut byte) {
            Ok(0) => {}
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
            result => panic!("invalid lease must close immediately: {result:?}"),
        }
    }
    let mut stream = UnixStream::connect(&runtime.hello.worker_socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    wire::write(&mut stream, &request).unwrap();
    let reply: lease::Attached = wire::read(&mut stream).unwrap();
    reply
        .validate_for(&request, &runtime.hello.worker_socket)
        .unwrap();
    assert_eq!(reply.result.unwrap().session_id, runtime.session);
}

#[test]
fn two_desktops_route_each_terminals_helpers_to_its_own_mac() {
    use newport_protocol::integration::{self as control, wire};
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
        worker.ready();
        let mut stdout = worker.stdout.take().unwrap();
        let (send, receive) = mpsc::sync_channel(8);
        std::thread::spawn(move || {
            while let Ok(frame) = wire::read::<ServerMessage>(&mut stdout) {
                if send.send(frame).is_err() {
                    break;
                }
            }
        });
        receive
    };
    let ea = events(&mut a);
    let eb = events(&mut b);
    let event = |events: &mpsc::Receiver<ServerMessage>| events.recv_timeout(REPLY).unwrap();
    for (runtime, worker) in [(&mut first, &first_id), (&mut second, &second_id)] {
        let id = runtime.hello.instance_id.clone();
        let applied = runtime.request(
            id,
            protocol::Operation::Permissions {
                worker_id: worker.clone(),
                permissions: newport_protocol::runtime::Permissions {
                    revision: 1,
                    clipboard: true,
                    browser: true,
                },
            },
        );
        let applied = applied.result.unwrap().permissions();
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
    let helper = |terminal: Option<&str>, args: &[&str]| {
        let mut command = Command::new(BIN);
        command
            .args(args)
            .env("HOME", home.path())
            .env("XDG_RUNTIME_DIR", home.path())
            .env_remove("NEWPORT_TERMINAL");
        if let Some(terminal) = terminal {
            command.env("NEWPORT_TERMINAL", terminal);
        }
        command.output().unwrap()
    };
    let ambiguous = helper(None, &["env"]);
    assert!(stable_only(&ambiguous));
    assert!(
        String::from_utf8_lossy(&ambiguous.stderr).contains(
            "Several computers are connected to this server. Run this in a Newport terminal on the computer you want."
        )
    );
    let in_first = routed_terminal(home.path(), &first.session, "First");
    let in_second = routed_terminal(home.path(), &second.session, "Second");
    for (terminal, worker, events, format) in [
        (&in_first, &mut a, &ea, "application/x-first"),
        (&in_second, &mut b, &eb, "application/x-second"),
    ] {
        integration::wire::write(
            worker.stdin.as_mut().unwrap(),
            &ClientMessage::Call(integration_call(IntegrationRequest::ClipboardOffer(
                imports::Offer {
                    revision: 42i64,
                    formats: vec![format].into_iter().map(String::from).collect(),
                },
            ))),
        )
        .unwrap();
        assert!(matches!(event(events), ServerMessage::Response(_)));
        let result = helper(Some(terminal), &["clipboard", "-o", "-t", "TARGETS"]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(String::from_utf8(result.stdout).unwrap().trim(), format);
        assert!(helper(Some(terminal), &["env"]).status.success());
    }
    // Browser requests must arrive at the terminal's desktop only.
    let mut opener = Worker(
        Command::new(BIN)
            .args(["open", "https://example.test/first"])
            .env("HOME", home.path())
            .env("XDG_RUNTIME_DIR", home.path())
            .env("NEWPORT_TERMINAL", &in_first)
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let ServerMessage::Browser(BrowserRequest { id: request, url }) = event(&ea) else {
        panic!("expected browser request")
    };
    assert_eq!(url, "https://example.test/first");
    assert!(eb.try_recv().is_err());
    integration::wire::write(
        a.stdin.as_mut().unwrap(),
        &ClientMessage::Browser(BrowserReply {
            id: request,
            success: true,
            message: None,
        }),
    )
    .unwrap();
    assert!(opener.wait().unwrap().success());
    let id = first.hello.instance_id.clone();
    first.request(
        id.clone(),
        protocol::Operation::Permissions {
            worker_id: first_id,
            permissions: newport_protocol::runtime::Permissions {
                revision: 2,
                clipboard: false,
                browser: false,
            },
        },
    );
    let local_only = helper(Some(&in_first), &["clipboard", "-o", "-t", "TARGETS"]);
    assert!(local_only.status.success(), "{local_only:?}");
    assert!(
        local_only.stdout.is_empty(),
        "revoked host metadata remained visible"
    );
    let revoked = helper(
        Some(&in_first),
        &["clipboard", "-o", "-t", "application/x-first"],
    );
    assert!(!revoked.status.success());
    assert!(revoked.stdout.is_empty());
    assert!(
        ea.try_recv().is_err(),
        "local-only helper requested host data"
    );
    let only_second = helper(None, &["env"]);
    assert!(only_second.status.success());
    assert!(String::from_utf8_lossy(&only_second.stdout).contains("BROWSER"));
    first.request(id, protocol::Operation::Shutdown);
    first.stopped();
    // A terminal whose Mac disconnected never switches to the remaining one.
    let stale = helper(Some(&in_first), &["clipboard", "-o", "-t", "TARGETS"]);
    assert!(!stale.status.success());
    let error = String::from_utf8_lossy(&stale.stderr);
    assert!(error.contains("First"), "{error}");
    assert!(!error.contains("Enable it in Newport"), "{error}");
    assert!(
        helper(Some(&in_second), &["clipboard", "-o", "-t", "TARGETS"])
            .status
            .success()
    );
    // A terminal Needle has no route for never goes to the only Mac left.
    let unknown = uuid::Uuid::new_v4().to_string();
    let lost = helper(Some(&unknown), &["clipboard", "-o", "-t", "TARGETS"]);
    assert!(!lost.status.success());
    let error = String::from_utf8_lossy(&lost.stderr);
    assert!(
        error.contains("Open this terminal in Newport to continue."),
        "{error}"
    );
    // A route outlives the runtime directory it was recorded with, which
    // logind removes at the account's last logout.
    let runtime = tempfile::tempdir_in(home.path()).unwrap();
    let kept = uuid::Uuid::new_v4().to_string();
    let recorded = Command::new(BIN)
        .args(["terminal", "route", &kept, &second.session])
        .arg("5365636f6e64")
        .env("HOME", home.path())
        .env("XDG_RUNTIME_DIR", runtime.path())
        .status()
        .unwrap();
    assert!(recorded.success());
    drop(runtime);
    let routed = helper(Some(&kept), &["clipboard", "-o", "-t", "TARGETS"]);
    assert!(
        routed.status.success(),
        "{}",
        String::from_utf8_lossy(&routed.stderr)
    );
    // Relaunching the same desktop keeps the terminal's route, but the new
    // worker must receive fresh permissions before it can serve it.
    let mut restarted = Runtime::start(home.path(), "");
    restarted.session = first.session.clone();
    restarted.client = first.client.clone();
    restarted.initialize();
    let worker_id = restarted.register("integration", control::VERSION);
    let mut worker = restarted.worker(&worker_id, "integration", home.path());
    let restarted_events = events(&mut worker);
    let reply = restarted.request(
        restarted.hello.instance_id.clone(),
        protocol::Operation::Permissions {
            worker_id,
            permissions: newport_protocol::runtime::Permissions {
                revision: 1,
                clipboard: true,
                browser: true,
            },
        },
    );
    assert!(reply.result.unwrap().permissions().error.is_none());
    let mut opener = Worker(
        Command::new(BIN)
            .args(["open", "https://example.test/restarted"])
            .env("HOME", home.path())
            .env("XDG_RUNTIME_DIR", home.path())
            .env("NEWPORT_TERMINAL", &in_first)
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let ServerMessage::Browser(BrowserRequest { id: request, url }) = event(&restarted_events)
    else {
        panic!("expected browser request")
    };
    assert_eq!(url, "https://example.test/restarted");
    assert!(eb.try_recv().is_err());
    integration::wire::write(
        worker.stdin.as_mut().unwrap(),
        &ClientMessage::Browser(BrowserReply {
            id: request,
            success: true,
            message: None,
        }),
    )
    .unwrap();
    assert!(opener.wait().unwrap().success());
}
/// A plain terminal's clipboard display reads the clipboard of the Mac the
/// terminal runs on, though another is connected, and what a program copies
/// on it reaches the terminal as a copy, which the terminal makes on its Mac.
#[test]
fn a_plain_terminals_display_reads_its_macs_clipboard_and_copies_through_the_terminal() {
    use base64::Engine;
    use newport_protocol::integration::{self as control, wire};
    use std::{io::Read, os::fd::FromRawFd};
    use x11rb::{
        CURRENT_TIME,
        connection::Connection,
        protocol::{Event, xproto::*},
        rust_connection::{DefaultStream, RustConnection},
        wrapper::ConnectionExt as _,
    };
    let home = fixture_home();
    let mut desktops = Vec::new();
    for format in ["application/x-first", "application/x-second"] {
        let mut runtime = Runtime::start(home.path(), "");
        runtime.initialize();
        let worker_id = runtime.register("integration", control::VERSION);
        let mut worker = runtime.worker(&worker_id, "integration", home.path());
        worker.ready();
        let mut stdout = worker.stdout.take().unwrap();
        let (send, events) = mpsc::sync_channel(8);
        std::thread::spawn(move || {
            while let Ok(frame) = wire::read::<ServerMessage>(&mut stdout) {
                if send.send(frame).is_err() {
                    break;
                }
            }
        });
        let id = runtime.hello.instance_id.clone();
        let applied = runtime.request(
            id,
            protocol::Operation::Permissions {
                worker_id,
                permissions: newport_protocol::runtime::Permissions {
                    revision: 1,
                    clipboard: true,
                    browser: false,
                },
            },
        );
        assert!(applied.result.unwrap().permissions().error.is_none());
        integration::wire::write(
            worker.stdin.as_mut().unwrap(),
            &ClientMessage::Call(integration_call(IntegrationRequest::ClipboardOffer(
                imports::Offer {
                    revision: 42i64,
                    formats: vec![format].into_iter().map(String::from).collect(),
                },
            ))),
        )
        .unwrap();
        assert!(matches!(
            events.recv_timeout(REPLY).unwrap(),
            ServerMessage::Response(_)
        ));
        desktops.push((runtime, worker, events));
    }
    let terminal = routed_terminal(home.path(), &desktops[0].0.session, "First");

    // The shell, on a terminal of its own.
    let (mut master, mut slave) = (-1, -1);
    // SAFETY: both descriptor slots are valid and the other pointers are null.
    let opened = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(opened, 0);
    // SAFETY: both descriptors came from openpty and are used only here.
    let (mut master, slave) = unsafe {
        (
            std::fs::File::from_raw_fd(master),
            std::fs::File::from_raw_fd(slave),
        )
    };
    let _shell = Worker(
        Command::new(BIN)
            .args(["display", "--terminal", &terminal, "--", "sh", "-c"])
            .arg("printf '<%s>\\n' \"$DISPLAY\"; exec cat >/dev/null")
            .env("HOME", home.path())
            .env("XDG_RUNTIME_DIR", home.path())
            .stdin(slave.try_clone().unwrap())
            .stdout(slave.try_clone().unwrap())
            .stderr(slave)
            .spawn()
            .unwrap(),
    );
    let (send, output) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = [0; 4096];
        while let Ok(n) = master.read(&mut buffer) {
            if n == 0 || send.send(buffer[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut shown = Vec::new();
    let mut until = |wanted: &dyn Fn(&str) -> bool| {
        while !wanted(&String::from_utf8_lossy(&shown)) {
            shown.extend(output.recv_timeout(REPLY).expect("terminal output"));
        }
        String::from_utf8_lossy(&shown).into_owned()
    };
    let text = until(&|text| text.contains('>'));
    let display = &text[text.find('<').unwrap() + 1..text.find('>').unwrap()];
    assert!(display.starts_with(':'), "{text}");

    // A program on the display reads the First Mac's clipboard.
    let socket =
        std::os::unix::net::UnixStream::connect(format!("/tmp/.X11-unix/X{}", &display[1..]))
            .unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let stream = DefaultStream::from_unix_stream(socket).unwrap().0;
    let c = RustConnection::connect_to_stream(stream, 0).unwrap();
    let atom = |name: &str| {
        c.intern_atom(false, name.as_bytes())
            .unwrap()
            .reply()
            .unwrap()
            .atom
    };
    let w = c.generate_id().unwrap();
    c.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        w,
        c.setup().roots[0].root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::COPY_FROM_PARENT,
        x11rb::COPY_FROM_PARENT,
        &CreateWindowAux::new(),
    )
    .unwrap()
    .check()
    .unwrap();
    let (clipboard, targets) = (atom("CLIPBOARD"), atom("TARGETS"));
    c.convert_selection(w, clipboard, targets, targets, CURRENT_TIME)
        .unwrap();
    c.flush().unwrap();
    loop {
        if let Event::SelectionNotify(event) = c.wait_for_event().unwrap() {
            assert_ne!(event.property, x11rb::NONE, "the First Mac's clipboard");
            break;
        }
    }
    let listed = c
        .get_property(true, w, targets, AtomEnum::ATOM, 0, 1024)
        .unwrap()
        .reply()
        .unwrap();
    let names: Vec<String> = listed
        .value32()
        .unwrap()
        .map(|id| String::from_utf8(c.get_atom_name(id).unwrap().reply().unwrap().name).unwrap())
        .collect();
    assert!(
        names.iter().any(|name| name == "application/x-first"),
        "{names:?}"
    );
    assert!(
        !names.iter().any(|name| name == "application/x-second"),
        "{names:?}"
    );

    // A program copies on the display: the terminal gets the copy.
    let utf8 = atom("UTF8_STRING");
    c.set_selection_owner(w, clipboard, CURRENT_TIME)
        .unwrap()
        .check()
        .unwrap();
    loop {
        match c.wait_for_event().unwrap() {
            Event::SelectionRequest(request) => {
                let property = if request.target == targets {
                    c.change_property32(
                        PropMode::REPLACE,
                        request.requestor,
                        request.property,
                        AtomEnum::ATOM,
                        &[targets, utf8],
                    )
                    .unwrap();
                    request.property
                } else if request.target == utf8 {
                    c.change_property8(
                        PropMode::REPLACE,
                        request.requestor,
                        request.property,
                        utf8,
                        b"copied in a plain terminal",
                    )
                    .unwrap();
                    request.property
                } else {
                    x11rb::NONE
                };
                let event = SelectionNotifyEvent {
                    response_type: SELECTION_NOTIFY_EVENT,
                    sequence: 0,
                    time: request.time,
                    requestor: request.requestor,
                    selection: request.selection,
                    target: request.target,
                    property,
                };
                c.send_event(false, request.requestor, EventMask::NO_EVENT, event)
                    .unwrap()
                    .check()
                    .unwrap();
            }
            Event::SelectionClear(_) => break,
            _ => (),
        }
    }
    let copy = format!(
        "\x1b]52;c;{}\x07",
        base64::engine::general_purpose::STANDARD.encode("copied in a plain terminal")
    );
    until(&|text| text.contains(&copy));
    // Neither Mac was asked for anything: the copy goes through the terminal.
    for (_, _, events) in &desktops {
        assert!(events.try_recv().is_err());
    }
}

#[test]
fn workers_require_the_negotiated_build_before_serving_feature_protocols() {
    let home = fixture_home();
    let mut runtime = Runtime::start(home.path(), &std::env::var("PATH").unwrap());
    runtime.initialize();
    let worker_id = runtime.register("git", GIT);
    let rejected = Command::new(BIN)
        .args(["worker", &"0".repeat(64), "git"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("build changed"));
    let mut worker = runtime.worker(&worker_id, "git", home.path());
    configure_git(worker.stdin.as_mut().unwrap());
    let mut output = worker.stdout.take().unwrap();
    let (send, receive) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = send.send(newport_protocol::git::wire::read::<
            newport_protocol::git::ServerMessage,
        >(&mut output));
    });
    let result = receive.recv_timeout(Duration::from_secs(10));
    assert!(matches!(
        result.unwrap().unwrap(),
        newport_protocol::git::ServerMessage::Ready(..)
    ));
    // The same registration cannot be consumed twice, even while the first
    // worker is sitting idle with its feature stdin still open.
    let mut duplicate = runtime.worker(&worker_id, "git", home.path());
    assert!(!duplicate.wait().unwrap().success());
    let socket = runtime.hello.worker_socket.clone();
    let id = runtime.hello.instance_id.clone();
    assert!(
        runtime
            .request(id, protocol::Operation::Shutdown)
            .result
            .is_ok()
    );
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
    assert!(
        runtime
            .request(
                id.clone(),
                protocol::Operation::PrepareWorkerStream {
                    worker_id: uuid::Uuid::new_v4().to_string(),
                    service: "integration".into(),
                    version: 1,
                    contract_id: None,
                }
            )
            .result
            .as_ref()
            .err()
            .unwrap()
            .message
            .contains("limit")
    );
    assert!(
        runtime
            .request(id.clone(), protocol::Operation::Ping)
            .result
            .is_ok()
    );
    assert!(
        runtime
            .request(id, protocol::Operation::Shutdown)
            .result
            .is_ok()
    );
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
    let worker = first.register("git", GIT);
    let mut wrong = second.worker(&worker, "git", home.path());
    assert!(!wrong.wait().unwrap().success());
    assert!(
        first
            .request(first.hello.instance_id.clone(), protocol::Operation::Ping)
            .result
            .is_ok()
    );
}

fn start_integration(runtime: &mut Runtime, home: &std::path::Path) -> Worker {
    let registration = runtime.register("integration", integration::VERSION);
    let mut worker = runtime.worker(&registration, "integration", home);
    worker.ready();
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
        // SAFETY: kill takes scalar arguments on a child this test owns.
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
    assert!(
        remaining
            .iter()
            .any(|r| r["runtime"] == live.hello.instance_id)
    );
    // The disabled-but-live bridge must retain its publication and endpoint.
    assert!(live_worker.try_wait().unwrap().is_none());
    assert!(live_root.join("agent.sock").exists());
    let id = live.hello.instance_id.clone();
    assert!(live.request(id, protocol::Operation::Ping).result.is_ok());
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
    assert!(
        runtime
            .request(id, protocol::Operation::Ping)
            .result
            .is_ok()
    );
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
            let line = lines.recv_timeout(REPLY).unwrap();
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
    let witness = x11::socket_witness();
    let crashed_original = witness.path().join("crashed");
    let live_original = witness.path().join("live");
    std::fs::hard_link(&socket, &crashed_original).unwrap();
    std::fs::hard_link(&live_socket, &live_original).unwrap();
    crashed.kill().unwrap();
    crashed.wait().unwrap();
    let _runtime = Runtime::start(home.path(), "");
    // Display numbers are shared across concurrent processes. Another display
    // may reuse the public path immediately after recovery removes our inode.
    x11::assert_socket_released(&socket, &crashed_original);
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
    x11::assert_socket_released(&live_socket, &live_original);
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
    let line = line.recv_timeout(REPLY).unwrap();
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
fn a_terminal_follows_its_mac_to_its_reconnection_and_only_the_replaced_client_retires() {
    use newport_protocol::integration as control;
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
    worker.ready();
    // A terminal opened on the Mac before it reconnected.
    let terminal = routed_terminal(home.path(), &old.session, "Studio");
    let open = || {
        Command::new(BIN)
            .args(["open", "https://example.test/next"])
            .env("HOME", home.path())
            .env("XDG_RUNTIME_DIR", home.path())
            .env("NEWPORT_TERMINAL", &terminal)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    // The link reaches the reconnection, which answers it.
    let opened = |worker: &mut Worker| {
        let mut opener = Worker(open());
        let request = loop {
            if let ServerMessage::Browser(BrowserRequest { id, url }) =
                integration::wire::read(worker.stdout.as_mut().unwrap()).unwrap()
            {
                assert_eq!(url, "https://example.test/next");
                break id;
            }
        };
        integration::wire::write(
            worker.stdin.as_mut().unwrap(),
            &ClientMessage::Browser(BrowserReply {
                id: request,
                success: true,
                message: None,
            }),
        )
        .unwrap();
        assert!(opener.wait().unwrap().success());
    };
    let permit = |next: &mut Runtime, revision, browser| {
        let applied = next.request(
            next.hello.instance_id.clone(),
            protocol::Operation::Permissions {
                worker_id: id.clone(),
                permissions: newport_protocol::runtime::Permissions {
                    revision,
                    browser,
                    clipboard: true,
                },
            },
        );
        assert!(applied.result.unwrap().permissions().error.is_none());
    };
    // Following the reconnection must not grant permissions or use the old
    // endpoint.
    permit(&mut next, 1, false);
    let refused = open().wait_with_output().unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("Studio doesn’t open links"));
    permit(&mut next, 2, true);
    opened(&mut worker);
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
    // What the reconnection's Mac copied reaches the terminal.
    integration::wire::write(
        worker.stdin.as_mut().unwrap(),
        &ClientMessage::Call(integration_call(IntegrationRequest::ClipboardOffer(
            imports::Offer {
                revision: 42i64,
                formats: vec!["application/x-next"]
                    .into_iter()
                    .map(String::from)
                    .collect(),
            },
        ))),
    )
    .unwrap();
    while !matches!(
        integration::wire::read(worker.stdout.as_mut().unwrap()).unwrap(),
        ServerMessage::Response(_)
    ) {}
    let pasted = Command::new(BIN)
        .args(["clipboard", "-o", "-t", "TARGETS"])
        .env("HOME", home.path())
        .env("XDG_RUNTIME_DIR", home.path())
        .env("NEWPORT_TERMINAL", &terminal)
        .output()
        .unwrap();
    assert!(pasted.status.success(), "{pasted:?}");
    assert_eq!(
        String::from_utf8(pasted.stdout).unwrap().trim(),
        "application/x-next"
    );
    permit(&mut next, 3, false);
    assert!(!open().wait().unwrap().success());
}

#[test]
fn clipboard_endpoints_and_helpers_survive_restart_without_shell_display_exports() {
    use newport_protocol::integration as control;
    let home = fixture_home();
    let enable = |runtime: &mut Runtime, worker_id: &str| {
        assert!(
            runtime
                .request(
                    runtime.hello.instance_id.clone(),
                    protocol::Operation::Permissions {
                        worker_id: worker_id.to_owned(),
                        permissions: newport_protocol::runtime::Permissions {
                            revision: 1,
                            browser: false,
                            clipboard: true
                        }
                    }
                )
                .result
                .unwrap()
                .permissions()
                .error
                .is_none()
        );
    };
    // The same desktop again, as Newport starts it: the same client and,
    // derived from its profile and the server, the same session ID.
    let start = |previous: Option<(&str, &str)>| {
        let mut runtime = Runtime::start(home.path(), "");
        if let Some((client, session)) = previous {
            runtime.client = client.to_owned();
            runtime.session = session.to_owned();
        }
        runtime.initialize();
        let id = runtime.register("integration", control::VERSION);
        let mut worker = runtime.worker(&id, "integration", home.path());
        worker.ready();
        enable(&mut runtime, &id);
        (runtime, worker)
    };
    // Ordinary shell refresh does not publish the clipboard protocol socket
    // as a rendering display, even while the explicit endpoint remains usable.
    let shell = |wayland: Option<&str>| {
        let mut command = Command::new("sh");
        command
            .args([
                "-eu",
                "-c",
                "eval \"$(\"$AGENT\" env --shell sh)\"; printf '%s' \"${WAYLAND_DISPLAY-unset}\"",
            ])
            .env_clear()
            .env("AGENT", BIN)
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", home.path())
            .env("XDG_RUNTIME_DIR", home.path());
        if let Some(wayland) = wayland {
            command.env("WAYLAND_DISPLAY", wayland);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let (first, first_worker) = start(None);
    assert_eq!(shell(None), "unset");
    // Protocol clients opt into the published clipboard endpoint explicitly.
    // Its stable alias remains useful to those clients, not to GUI applications.
    use sha2::{Digest, Sha256};
    let client_hash = Sha256::digest(first.client.as_bytes());
    let name: String = client_hash[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let alias = first
        .integration_root(home.path())
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(format!("c-{name}"));
    let display = alias.join("wayland.sock").display().to_string();
    let old_directory = std::fs::canonicalize(&alias).unwrap();
    assert!(std::os::unix::net::UnixStream::connect(&display).is_ok());
    // The runtime restarts, as on an update, reconnect or reinstall; explicit
    // clipboard protocol clients keep the stable path they selected.
    let (client, session) = (first.client.clone(), first.session.clone());
    drop(first);
    drop(first_worker);
    let (_next, mut next_worker) = start(Some((&client, &session)));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::fs::canonicalize(&alias).ok().as_ref() == Some(&old_directory)
        || std::os::unix::net::UnixStream::connect(&display).is_err()
        || old_directory.exists()
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the kept path doesn't reach the new session"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        shell(Some(&display)),
        "unset",
        "owned clipboard endpoints leave the shell environment"
    );
    // Gone records cannot prove ownership. Retained helpers still select the
    // active session, but refresh conservatively leaves an unprovable old value.
    let stranded = old_directory.join("wayland.sock");
    assert_eq!(
        shell(Some(&stranded.to_string_lossy())),
        stranded.to_string_lossy()
    );
    // A program that started months ago, like Claude Code in tmux: its display
    // variables name a dead session, and Needle's clipboard command finds the
    // live one when it runs.
    integration::wire::write(
        next_worker.stdin.as_mut().unwrap(),
        &ClientMessage::Call(integration_call(IntegrationRequest::ClipboardOffer(
            imports::Offer {
                revision: 7i64,
                formats: vec!["image/png"].into_iter().map(String::from).collect(),
            },
        ))),
    )
    .unwrap();
    let mut stdout = next_worker.stdout.take().unwrap();
    let (frames, published) = mpsc::sync_channel(8);
    std::thread::spawn(move || {
        while let Ok(message) = integration::wire::read::<ServerMessage>(&mut stdout) {
            if frames.send(message).is_err() {
                break;
            }
        }
    });
    while !matches!(
        published.recv_timeout(REPLY).unwrap(),
        ServerMessage::Response(_)
    ) {}
    let paste = Command::new(BIN)
        .args(["clipboard", "-t", "TARGETS", "-o"])
        .env("HOME", home.path())
        .env("XDG_RUNTIME_DIR", home.path())
        .env("PATH", "/usr/bin:/bin")
        .env("DISPLAY", ":100")
        .env("XAUTHORITY", old_directory.join("Xauthority"))
        .env("WAYLAND_DISPLAY", &stranded)
        .output()
        .unwrap();
    assert!(
        paste.status.success(),
        "{}",
        String::from_utf8_lossy(&paste.stderr)
    );
    assert!(String::from_utf8_lossy(&paste.stdout).contains("image/png"));
}
