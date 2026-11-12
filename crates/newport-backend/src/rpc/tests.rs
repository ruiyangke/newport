use super::*;
use newport_protocol::{
    common::Present,
    desktop::{configuration as config, events, files, host, stream as streams},
    encoding::protobuf,
};
use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};

type WorkerRequests = Vec<(Uuid, Vec<String>)>;
#[derive(Clone, Default)]
struct TestHost {
    dispatched: Arc<AtomicUsize>,
    terminals: Arc<AtomicUsize>,
    pending: Arc<AtomicUsize>,
    hold_revocations: Arc<AtomicBool>,
    remote_apps: Arc<Mutex<WorkerRequests>>,
}
struct Terminal(Arc<AtomicUsize>);
impl Drop for Terminal {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
impl Host for TestHost {
    fn instance_id(&self) -> Uuid {
        Uuid::from_u128(1)
    }
    async fn dispatch(&self, operation: &Operation) -> Result<Reply, Failure> {
        self.dispatched.fetch_add(1, Ordering::SeqCst);
        match operation {
            Operation::Host(operation) => match operation.as_ref() {
                host::Operation::Activate(_) => Ok(Reply::Host(Box::new(host::Reply::Activate(
                    Box::new(Present {}),
                )))),
                host::Operation::CheckForUpdates(_) => {
                    self.pending.fetch_add(1, Ordering::SeqCst);
                    let _guard = Terminal(self.pending.clone());
                    std::future::pending().await
                }
                host::Operation::UpdateStatus(_) => Ok(Reply::Host(Box::new(
                    host::Reply::UpdateStatus(Box::new(host::UpdateStatus {
                        enabled: false,
                        current_version: "x".repeat(desktop::MAX_RESPONSE),
                        build: host::BuildStamp {
                            commit: None,
                            local: false,
                            built_at: None,
                        },
                        phase: host::UpdatePhase::Idle,
                        version: None,
                        downloaded: 0,
                        total: None,
                        error: None,
                    })),
                ))),
                _ => Err(failure::operation("Unknown fixture operation")),
            },
            Operation::Configuration(operation) => {
                if self.hold_revocations.load(Ordering::SeqCst) {
                    self.pending.fetch_add(1, Ordering::SeqCst);
                    let _guard = Terminal(self.pending.clone());
                    std::future::pending::<()>().await;
                }
                Ok(Reply::Configuration(Box::new(match operation.as_ref() {
                    config::Operation::SetClipboardEnabled(_) => {
                        config::Reply::SetClipboardEnabled(Box::new(Present {}))
                    }
                    config::Operation::SetIntegrationEnabled(_) => {
                        config::Reply::SetIntegrationEnabled(Box::new(Present {}))
                    }
                    _ => return Err(failure::operation("Unknown fixture configuration")),
                })))
            }
            Operation::Files(operation) => match operation.as_ref() {
                files::Operation::WriteText(args) => Ok(Reply::Files(Box::new(
                    files::Reply::WriteText(Box::new(files::Document {
                        path: args.path.clone(),
                        content: args.content.clone(),
                        revision: "saved".into(),
                        page: None,
                        cached: false,
                    })),
                ))),
                _ => Err(failure::operation("Unknown fixture file operation")),
            },
            _ => Err(failure::operation("Unknown fixture operation")),
        }
    }
    async fn open_terminal(&self, _: TerminalOpen) -> Result<TerminalChannel, Failure> {
        self.terminals.fetch_add(1, Ordering::SeqCst);
        let (input, inputs) = tokio::sync::mpsc::channel(1);
        let (outputs, output) = tokio::sync::mpsc::channel(1);
        Ok(TerminalChannel {
            io: crate::terminal::Io { input, output },
            guard: Box::new((Terminal(self.terminals.clone()), inputs, outputs)),
        })
    }
    async fn open_remote_apps(
        &self,
        server: Uuid,
        arguments: Vec<String>,
    ) -> Result<RemoteChannel, Failure> {
        self.remote_apps.lock().unwrap().push((server, arguments));
        Err(failure::operation("Fixture worker unavailable"))
    }
}
struct Fixture {
    directory: PathBuf,
    endpoint: PathBuf,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
    host: TestHost,
    events: EventLog,
}
impl Fixture {
    fn start() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "np-rpc-{}",
            &Uuid::new_v4().simple().to_string()[..12]
        ));
        newport_platform::filesystem::create_private_directory(&directory).unwrap();
        let endpoint = directory.join("native-client/desktop.sock");
        let server = Server::bind(&endpoint).unwrap();
        let events = server.events();
        let host = TestHost::default();
        let task = tokio::spawn(server.run(host.clone()));
        Self {
            directory,
            endpoint,
            task,
            host,
            events,
        }
    }
    async fn stop(self) {
        self.task.abort();
        assert!(self.task.await.unwrap_err().is_cancelled());
        std::fs::remove_dir_all(self.directory).unwrap();
    }
    async fn call(
        &self,
        operation: Operation,
    ) -> Result<Reply, newport_client::backend::exchange::Error> {
        let endpoint = self.endpoint.clone();
        tokio::task::spawn_blocking(move || {
            newport_client::backend::Backend::at_path(endpoint)
                .execute_with_timeout(operation, Duration::from_secs(5))
        })
        .await
        .unwrap()
    }
}
fn activate() -> Operation {
    Operation::Host(Box::new(host::Operation::Activate(Box::new(Present {}))))
}
fn pending() -> Operation {
    Operation::Host(Box::new(host::Operation::CheckForUpdates(Box::new(
        Present {},
    ))))
}
fn terminal() -> Operation {
    Operation::Stream(Box::new(streams::Operation::TerminalOpen(Box::new(
        TerminalOpen {
            server: Uuid::new_v4(),
            session: Uuid::new_v4(),
            columns: 80,
            rows: 24,
            persist: false,
            attach: false,
            colors: None,
        },
    ))))
}
pub(super) fn scope(server: Uuid) -> config::ConnectionScope {
    config::ConnectionScope {
        settings: config::ConnectionSettings {
            server_id: server,
            ssh_host: "localhost".into(),
            ssh_user: "fixture".into(),
            ssh_port: 22,
            auth_method: config::AuthMethod::PublicKey,
            identity_file: None,
            agent_source: None,
            agent_key_fingerprint: None,
            proxy: None,
        },
        connection_revision: 1,
        backend_instance: Uuid::from_u128(1),
    }
}
fn integration(clipboard: bool, enabled: bool) -> Operation {
    let server = Uuid::new_v4();
    let expected = scope(server);
    Operation::Configuration(Box::new(if clipboard {
        config::Operation::SetClipboardEnabled(Box::new(config::SetClipboardEnabledArgs {
            server,
            enabled,
            expected,
        }))
    } else {
        config::Operation::SetIntegrationEnabled(Box::new(config::SetIntegrationEnabledArgs {
            server,
            feature: config::Feature::Browser,
            enabled,
            expected,
        }))
    }))
}
fn connection(path: PathBuf) -> newport_platform::ipc::Stream {
    let mut attempts = 0;
    let stream = loop {
        match newport_platform::ipc::Stream::connect(&path) {
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused && attempts < 200 => {
                attempts += 1;
                std::thread::sleep(Duration::from_millis(10));
            }
            result => break result.unwrap(),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
}
fn request(
    stream: &mut newport_platform::ipc::Stream,
    operation: Operation,
) -> Result<Request, Failure> {
    stream
        .write_all(
            &wire::encode_client(&ClientMessage::Hello(Box::new(desktop::Hello {
                contract_id: Some(newport_protocol::desktop::CONTRACT_ID.into()),

                version: desktop::VERSION,
            })))
            .unwrap(),
        )
        .unwrap();
    match wire::read_admission(stream).unwrap() {
        ServerMessage::Ready(_) => {}
        ServerMessage::Refused(error) => {
            return Err(Failure {
                error: *error,
                detail: None,
            });
        }
        _ => panic!("not admission"),
    }
    let request = Request {
        id: desktop::RequestId::new(1).unwrap(),
        operation,
    };
    stream
        .write_all(
            &wire::encode_client(&ClientMessage::Request(Box::new(request.clone()))).unwrap(),
        )
        .unwrap();
    Ok(request)
}
fn response(stream: &mut newport_platform::ipc::Stream, request: &Request) -> Outcome {
    loop {
        let body = protobuf::read_frame(stream, desktop::MAX_RESPONSE)
            .unwrap()
            .unwrap();
        match wire::decode_server_for(body, request).unwrap() {
            ServerMessage::Response(reply) => return reply.outcome,
            ServerMessage::Progress(_) => {}
            _ => panic!("not a response"),
        }
    }
}
async fn wait_count(counter: &AtomicUsize, expected: usize) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while counter.load(Ordering::SeqCst) != expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn actual_client_routes_remote_apps_to_registered_workers() {
    let fixture = Fixture::start();
    let endpoint = fixture.endpoint.clone();
    let server = Uuid::new_v4();
    let arguments = vec![
        "--".to_owned(),
        "$(echo literal)".into(),
        "a b".into(),
        "".into(),
    ];
    let sent = arguments.clone();
    let error = tokio::task::spawn_blocking(move || {
        newport_client::backend::Backend::at_path(endpoint)
            .open(
                streams::Operation::RemoteAppsOpen(Box::new(streams::RemoteAppsOpenArgs {
                    server,
                    arguments: sent,
                })),
                Duration::from_secs(3),
            )
            .err()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(error.to_string().contains("Fixture worker unavailable"));
    assert_eq!(
        *fixture.host.remote_apps.lock().unwrap(),
        vec![(server, arguments)]
    );
    assert_eq!(fixture.host.dispatched.load(Ordering::SeqCst), 0);
    let endpoint = fixture.endpoint.clone();
    let invalid = streams::Operation::RemoteAppsOpen(Box::new(streams::RemoteAppsOpenArgs {
        server,
        arguments: vec!["bad\0argument".into()],
    }));
    assert!(
        tokio::task::spawn_blocking(move || newport_client::backend::Backend::at_path(endpoint)
            .open(invalid, Duration::from_secs(3))
            .is_err())
        .await
        .unwrap()
    );
    assert_eq!(fixture.host.remote_apps.lock().unwrap().len(), 1);
    fixture.stop().await;
}
#[tokio::test]
async fn actual_client_uses_host_and_bounded_event_history() {
    let fixture = Fixture::start();
    assert!(
        matches!(fixture.call(activate()).await.unwrap(), Reply::Host(value) if matches!(*value, host::Reply::Activate(_)))
    );
    assert!(
        fixture
            .call(Operation::Host(Box::new(host::Operation::StartupSettings(
                Box::new(Present {})
            ))))
            .await
            .unwrap_err()
            .to_string()
            .contains("Unknown fixture operation")
    );
    for index in 0..125 {
        fixture
            .events
            .record(events::Event::GitCommand(Box::new(events::GitCommand {
                server: Uuid::from_u128(1),
                repo_id: None,
                operation_id: None,
                entry: newport_protocol::git::CommandLog {
                    command: index.to_string(),
                    duration_ms: 0,
                    exit_code: Some(0),
                    output: String::new(),
                    interrupted: false,
                },
            })));
    }
    let poll = |after| {
        Operation::Events(Box::new(events::Operation::Poll(Box::new(
            events::PollArgs { after },
        ))))
    };
    let Reply::Events(value) = fixture.call(poll(0)).await.unwrap() else {
        panic!("events")
    };
    let events::Reply::Poll(history) = *value;
    assert_eq!(history.cursor, 125);
    assert_eq!(history.entries.len(), 100);
    let events::Event::GitCommand(first) = &history.entries[0];
    assert_eq!(first.entry.command, "25");
    let Reply::Events(value) = fixture.call(poll(124)).await.unwrap() else {
        panic!("events")
    };
    let events::Reply::Poll(recent) = *value;
    assert_eq!(recent.entries.len(), 1);
    let events::Event::GitCommand(last) = &recent.entries[0];
    assert_eq!(last.entry.command, "124");
    assert_eq!(fixture.host.dispatched.load(Ordering::SeqCst), 2);
    fixture.stop().await;
}
#[tokio::test]
async fn incompatible_clients_are_refused_without_dispatching_operations() {
    let fixture = Fixture::start();
    for contract_id in [None, Some("0".repeat(64))] {
        let endpoint = fixture.endpoint.clone();
        tokio::task::spawn_blocking(move || {
            let mut socket = connection(endpoint);
            socket
                .write_all(
                    &wire::encode_client(&ClientMessage::Hello(Box::new(desktop::Hello {
                        version: desktop::VERSION,
                        contract_id,
                    })))
                    .unwrap(),
                )
                .unwrap();
            let ServerMessage::Refused(error) = wire::read_admission(&mut socket).unwrap() else {
                panic!("Expected incompatible admission refusal");
            };
            assert_eq!(error.code, "incompatible");
            assert_eq!(error.retry, newport_protocol::common::Retry::No);
            let mut byte = [0];
            assert_eq!(socket.read(&mut byte).unwrap(), 0);
        })
        .await
        .unwrap();
    }
    assert_eq!(fixture.host.dispatched.load(Ordering::SeqCst), 0);
    assert!(fixture.call(activate()).await.is_ok());
    assert_eq!(fixture.host.dispatched.load(Ordering::SeqCst), 1);
    fixture.stop().await;
}

#[tokio::test]
async fn malformed_frames_never_dispatch_and_do_not_kill_the_listener() {
    let fixture = Fixture::start();
    for bytes in [vec![0], vec![0xff; 11], vec![1, 0xff], vec![0x80]] {
        let path = fixture.endpoint.clone();
        tokio::task::spawn_blocking(move || {
            let mut stream = connection(path);
            stream.write_all(&bytes).unwrap();
            drop(stream);
        })
        .await
        .unwrap();
    }
    assert_eq!(fixture.host.dispatched.load(Ordering::SeqCst), 0);
    assert!(fixture.call(activate()).await.is_ok());
    fixture.stop().await;
}
#[tokio::test]
async fn closing_a_real_terminal_lease_releases_only_its_guard() {
    let fixture = Fixture::start();
    let mut leases = Vec::new();
    for _ in 0..2 {
        let path = fixture.endpoint.clone();
        leases.push(
            tokio::task::spawn_blocking(move || {
                let mut stream = connection(path);
                let request = request(&mut stream, terminal()).unwrap();
                assert!(matches!(
                    response(&mut stream, &request),
                    Outcome::Opened(_)
                ));
                stream
            })
            .await
            .unwrap(),
        );
    }
    assert_eq!(fixture.host.terminals.load(Ordering::SeqCst), 2);
    for expected in [1, 0] {
        drop(leases.pop());
        wait_count(&fixture.host.terminals, expected).await;
    }
    fixture.stop().await;
}
#[tokio::test]
async fn invalid_terminal_open_does_not_acquire_ownership() {
    let fixture = Fixture::start();
    let mut operation = terminal();
    if let Operation::Stream(stream) = &mut operation
        && let streams::Operation::TerminalOpen(args) = stream.as_mut()
    {
        args.columns = 0;
    }
    let Operation::Stream(operation) = operation else {
        panic!("terminal")
    };
    let endpoint = fixture.endpoint.clone();
    assert!(
        tokio::task::spawn_blocking(move || newport_client::backend::Backend::at_path(endpoint)
            .open(*operation, Duration::from_secs(3))
            .is_err())
        .await
        .unwrap()
    );
    assert_eq!(fixture.host.terminals.load(Ordering::SeqCst), 0);
    fixture.stop().await;
}
#[tokio::test]
async fn shutdown_drains_open_leases_pending_commands_and_incomplete_frames() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint = directory.path().join("desktop.sock");
    let server = Server::bind(&endpoint).unwrap();
    let host = TestHost::default();
    let (shutdown, wait) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(server.run_until(host.clone(), async {
        let _ = wait.await;
    }));
    let path = endpoint.clone();
    let streams = tokio::task::spawn_blocking(move || {
        let mut lease = connection(path.clone());
        let call = request(&mut lease, terminal()).unwrap();
        assert!(matches!(response(&mut lease, &call), Outcome::Opened(_)));
        let mut queued = connection(path.clone());
        request(&mut queued, pending()).unwrap();
        let mut incomplete = connection(path);
        incomplete.write_all(&[0x80]).unwrap();
        [lease, queued, incomplete]
    })
    .await
    .unwrap();
    wait_count(&host.pending, 1).await;
    assert_eq!(host.terminals.load(Ordering::SeqCst), 1);
    shutdown.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(host.pending.load(Ordering::SeqCst), 0);
    assert_eq!(host.terminals.load(Ordering::SeqCst), 0);
    tokio::task::spawn_blocking(move || {
        for mut stream in streams {
            assert!(matches!(stream.read(&mut [0]), Ok(0)));
        }
        assert!(newport_platform::ipc::Stream::connect(endpoint).is_err());
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn aborting_listener_does_not_detach_terminal_lease_tasks() {
    let fixture = Fixture::start();
    let path = fixture.endpoint.clone();
    let stream = tokio::task::spawn_blocking(move || {
        let mut stream = connection(path);
        let request = request(&mut stream, terminal()).unwrap();
        assert!(matches!(
            response(&mut stream, &request),
            Outcome::Opened(_)
        ));
        stream
    })
    .await
    .unwrap();
    let active = fixture.host.terminals.clone();
    fixture.stop().await;
    wait_count(&active, 0).await;
    drop(stream);
}
#[tokio::test]
async fn requests_over_capacity_are_refused_with_busy_instead_of_closed() {
    let fixture = Fixture::start();
    let path = fixture.endpoint.clone();
    let held = tokio::task::spawn_blocking(move || {
        (0..desktop::OPERATION_SLOTS)
            .map(|_| {
                let mut stream = connection(path.clone());
                request(&mut stream, pending()).unwrap();
                stream
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap();
    wait_count(&fixture.host.pending, desktop::OPERATION_SLOTS).await;
    let path = fixture.endpoint.clone();
    let (refused, idle) = tokio::task::spawn_blocking(move || {
        let mut stream = connection(path.clone());
        let call = request(&mut stream, activate()).unwrap();
        let refused = response(&mut stream, &call);
        drop(stream);
        let mut lease = connection(path.clone());
        let call = request(&mut lease, terminal()).unwrap();
        assert!(matches!(response(&mut lease, &call), Outcome::Opened(_)));
        drop(lease);
        std::thread::sleep(Duration::from_millis(100));
        let idle = (0..desktop::CONNECTION_SLOTS - desktop::OPERATION_SLOTS)
            .map(|_| connection(path.clone()))
            .collect::<Vec<_>>();
        std::thread::sleep(Duration::from_millis(100));
        let mut over = connection(path);
        let over = request(&mut over, activate()).unwrap_err();
        ((refused, over), idle)
    })
    .await
    .unwrap();
    assert!(
        matches!(refused.0, Outcome::Failed(error) if error.error.code == "busy" && error.error.retry == Retry::Yes)
    );
    assert_eq!(refused.1.error.code, "busy");
    drop(idle);
    fixture.stop().await;
    drop(held);
}
#[tokio::test]
async fn integration_revocation_has_bounded_admission_separate_from_slow_commands() {
    for ordinary_saturated in [false, true] {
        let fixture = Fixture::start();
        let path = fixture.endpoint.clone();
        let held = tokio::task::spawn_blocking(move || {
            (0..if ordinary_saturated {
                desktop::OPERATION_SLOTS
            } else {
                0
            })
                .map(|_| {
                    let mut stream = connection(path.clone());
                    request(&mut stream, pending()).unwrap();
                    stream
                })
                .collect::<Vec<_>>()
        })
        .await
        .unwrap();
        if ordinary_saturated {
            wait_count(&fixture.host.pending, desktop::OPERATION_SLOTS).await;
            assert!(fixture.call(activate()).await.is_err());
            assert!(fixture.call(integration(false, true)).await.is_err());
        }
        for clipboard in [false, true] {
            assert!(fixture.call(integration(clipboard, false)).await.is_ok());
        }
        fixture.host.hold_revocations.store(true, Ordering::SeqCst);
        let path = fixture.endpoint.clone();
        let reserved = tokio::task::spawn_blocking(move || {
            (0..desktop::REVOCATION_SLOTS)
                .map(|_| {
                    let mut stream = connection(path.clone());
                    request(&mut stream, integration(false, false)).unwrap();
                    stream
                })
                .collect::<Vec<_>>()
        })
        .await
        .unwrap();
        wait_count(
            &fixture.host.pending,
            if ordinary_saturated {
                desktop::OPERATION_SLOTS + desktop::REVOCATION_SLOTS
            } else {
                desktop::REVOCATION_SLOTS
            },
        )
        .await;
        assert_eq!(
            fixture
                .call(integration(false, false))
                .await
                .unwrap_err()
                .code(),
            "busy"
        );
        if !ordinary_saturated {
            assert!(fixture.call(activate()).await.is_ok());
        }
        let active = fixture.host.pending.clone();
        fixture.stop().await;
        wait_count(&active, 0).await;
        drop(held);
        drop(reserved);
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_response_returns_an_error_without_closing_before_the_header() {
    let fixture = Fixture::start();
    let error = fixture
        .call(Operation::Host(Box::new(host::Operation::UpdateStatus(
            Box::new(Present {}),
        ))))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "invalid_result");
    assert_eq!(error.retry(), Retry::Unknown);
    assert!(fixture.call(activate()).await.is_ok());
    assert_eq!(fixture.host.dispatched.load(Ordering::SeqCst), 2);
    fixture.stop().await;
}
#[tokio::test]
async fn only_file_saves_accept_large_native_payloads() {
    let fixture = Fixture::start();
    let server = Uuid::new_v4();
    let content = "x".repeat(12 * 1024 * 1024);
    let result = fixture
        .call(Operation::Files(Box::new(files::Operation::WriteText(
            Box::new(files::WriteTextArgs {
                server,
                operation: Uuid::new_v4(),
                root: "/repo".into(),
                path: "/repo/file".into(),
                expected: scope(server),
                content,
                revision: "previous".into(),
            }),
        ))))
        .await
        .unwrap();
    let Reply::Files(result) = result else {
        panic!("files")
    };
    let files::Reply::WriteText(document) = *result else {
        panic!("write")
    };
    assert_eq!(document.content.len(), 12 * 1024 * 1024);
    let invalid = Operation::Host(Box::new(host::Operation::OpenRemoteApp(Box::new(
        host::OpenRemoteAppArgs {
            id: "x".repeat(desktop::MAX_REQUEST),
            action: None,
        },
    ))));
    let before = fixture.host.dispatched.load(Ordering::SeqCst);
    assert!(fixture.call(invalid).await.is_err());
    assert_eq!(fixture.host.dispatched.load(Ordering::SeqCst), before);
    fixture.stop().await;
}
