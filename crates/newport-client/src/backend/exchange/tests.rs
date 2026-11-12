use super::*;
use desktop::{account, stream};
use newport_platform::ipc::{Listener, Stream};
use std::{
    fs,
    io::{Read, Write},
    sync::mpsc,
    thread,
};

struct Host {
    directory: PathBuf,
    backend: Backend,
    task: Option<thread::JoinHandle<()>>,
}
impl Host {
    fn new(peer: impl FnOnce(Stream) + Send + 'static) -> Self {
        Self::with_listener(move |listener| {
            let (socket, _) = listener.accept().unwrap();
            peer(socket);
        })
    }
    fn with_listener(peer: impl FnOnce(Listener) + Send + 'static) -> Self {
        let directory =
            std::env::temp_dir().join(format!("newport-desktop-client-{}", uuid::Uuid::new_v4()));
        newport_platform::filesystem::create_private_directory(&directory).unwrap();
        let path = ipc::profile_endpoint(&directory);
        newport_platform::filesystem::create_private_directory(path.parent().unwrap()).unwrap();
        let listener = Listener::bind(&path).unwrap();
        let task = thread::spawn(move || peer(listener));
        Self {
            directory,
            backend: Backend::at_path(path),
            task: Some(task),
        }
    }
    fn finish(mut self) {
        self.task.take().unwrap().join().unwrap();
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let _ = fs::remove_file(self.backend.socket_path());
            let _ = fs::remove_dir(self.backend.socket_path().parent().unwrap());
        }
        let _ = fs::remove_dir_all(&self.directory);
    }
}
fn status() -> desktop::Operation {
    desktop::Operation::Account(Box::new(account::Operation::Status(Box::new(
        common::Present {},
    ))))
}
fn admitted(socket: &mut Stream) -> desktop::Request {
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    socket
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let hello = wire::read_hello(socket).unwrap();
    assert_eq!(hello.version, desktop::VERSION);
    socket
        .write_all(
            &wire::encode_server(&desktop::ServerMessage::Ready(Box::new(desktop::Ready {
                contract_id: Some(newport_protocol::desktop::CONTRACT_ID.into()),
                version: desktop::VERSION,
                backend_instance: uuid::Uuid::new_v4(),
            })))
            .unwrap(),
        )
        .unwrap();
    let desktop::ClientMessage::Request(request) = wire::read_client(socket, false).unwrap() else {
        panic!("expected request");
    };
    *request
}
fn completed(request: &desktop::Request) -> desktop::ServerMessage {
    desktop::ServerMessage::Response(Box::new(desktop::Response {
        id: request.id,
        outcome: desktop::Outcome::Completed(Box::new(desktop::Reply::Account(Box::new(
            account::Reply::Status(Box::new(account::Status {
                state: account::Phase::SignedOut,
                email: None,
                name: None,
                license: None,
                signing_out: false,
                notice: None,
                license_revision: 7,
            })),
        )))),
    }))
}
#[test]
fn incompatible_backend_receives_no_request_arguments() {
    for contract_id in [None, Some("0".repeat(64))] {
        let host = Host::new(move |mut socket| {
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let hello = wire::read_hello(&mut socket).unwrap();
            assert_eq!(hello.contract_id.as_deref(), Some(desktop::CONTRACT_ID));
            socket
                .write_all(
                    &wire::encode_server(&desktop::ServerMessage::Ready(Box::new(
                        desktop::Ready {
                            version: desktop::VERSION,
                            backend_instance: uuid::Uuid::new_v4(),
                            contract_id,
                        },
                    )))
                    .unwrap(),
                )
                .unwrap();
            let mut byte = [0];
            assert_eq!(socket.read(&mut byte).unwrap(), 0);
        });
        let error = host
            .backend
            .execute_with_timeout(status(), Duration::from_secs(2))
            .unwrap_err();
        assert_eq!(error.phase, Phase::Admission);
        assert!(!error.dispatched);
        assert_eq!(error.code(), "incompatible");
        assert_eq!(error.retry(), common::Retry::No);
        host.finish();
    }
}

#[test]
fn unary_completion_requires_correlated_reply_and_transport_closure() {
    let host = Host::new(|mut socket| {
        let request = admitted(&mut socket);
        socket
            .write_all(&wire::encode_server_for(&completed(&request), &request).unwrap())
            .unwrap();
    });
    let reply = host
        .backend
        .execute_with_timeout(status(), Duration::from_secs(2))
        .unwrap();
    assert!(
        matches!(reply, desktop::Reply::Account(value) if matches!(value.as_ref(), account::Reply::Status(value) if value.license_revision == 7))
    );
    host.finish();
}
#[test]
fn truncated_admission_never_sends_the_request() {
    let host = Host::new(|mut socket| {
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        wire::read_hello(&mut socket).unwrap();
        socket.write_all(&[10, 8]).unwrap();
        // Force an incomplete greeting without keeping a blocked reader alive.
        socket.shutdown(Shutdown::Both).unwrap();
    });
    let error = host
        .backend
        .execute_with_timeout(status(), Duration::from_secs(2))
        .unwrap_err();
    assert_eq!(error.phase, Phase::Admission);
    assert!(!error.dispatched);
    assert_eq!(error.retry(), common::Retry::No);
    host.finish();
}
#[test]
fn coalesced_second_result_is_not_accepted_as_completed() {
    let host = Host::new(|mut socket| {
        let request = admitted(&mut socket);
        let frame = wire::encode_server_for(&completed(&request), &request).unwrap();
        let mut bytes = frame.clone();
        bytes.extend(frame);
        socket.write_all(&bytes).unwrap();
    });
    let error = host
        .backend
        .execute_with_timeout(status(), Duration::from_secs(2))
        .unwrap_err();
    assert_eq!(error.phase, Phase::Closure);
    assert!(error.dispatched);
    assert_eq!(error.retry(), common::Retry::Unknown);
    host.finish();
}
#[test]
fn result_without_eof_has_unknown_outcome_and_is_never_replayed() {
    let (release, wait) = mpsc::sync_channel(1);
    let host = Host::new(move |mut socket| {
        let request = admitted(&mut socket);
        socket
            .write_all(&wire::encode_server_for(&completed(&request), &request).unwrap())
            .unwrap();
        wait.recv_timeout(Duration::from_secs(2)).unwrap();
        let mut next = [0];
        assert_eq!(socket.read(&mut next).unwrap(), 0);
    });
    let error = host
        .backend
        .execute_with_timeout(status(), Duration::from_millis(100))
        .unwrap_err();
    assert_eq!(error.phase, Phase::Closure);
    assert_eq!(error.retry(), common::Retry::Unknown);
    release.send(()).unwrap();
    host.finish();
}
#[test]
fn opened_preserves_coalesced_first_stream_frame() {
    let server = uuid::Uuid::new_v4();
    let host = Host::new(move |mut socket| {
        let request = admitted(&mut socket);
        let opened = desktop::ServerMessage::Response(Box::new(desktop::Response {
            id: request.id,
            outcome: desktop::Outcome::Opened(Box::new(stream::Opened::Command(Box::new(
                stream::CommandOpened {
                    version: desktop::VERSION,
                    server,
                },
            )))),
        }));
        let mut bytes = wire::encode_server_for(&opened, &request).unwrap();
        bytes.extend(
            wire::encode_opaque_server(&stream::OpaqueServer::Data(Box::new(
                Vec::from(b"first frame").into(),
            )))
            .unwrap(),
        );
        socket.write_all(&bytes).unwrap();
    });
    let mut opened = host
        .backend
        .open(
            stream::Operation::CommandOpen(Box::new(stream::CommandOpenArgs {
                server,
                command: "printf data".into(),
            })),
            Duration::from_secs(2),
        )
        .unwrap();
    let frame = opened
        .reader
        .next_until(
            desktop::OPAQUE_LIMITS.message,
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap()
        .unwrap();
    assert!(
        matches!(wire::decode_opaque_server(frame.body()).unwrap(), stream::OpaqueServer::Data(bytes) if bytes.as_ref().as_ref() == b"first frame")
    );
    drop(opened);
    host.finish();
}
#[test]
fn consent_pauses_only_the_remaining_operation_budget() {
    let start = Instant::now();
    let mut clock = ResponseClock::new(start + Duration::from_secs(10));
    clock
        .approval(true, start + Duration::from_secs(4))
        .unwrap();
    clock
        .approval(true, start + Duration::from_secs(20))
        .unwrap();
    clock
        .approval(false, start + Duration::from_secs(50))
        .unwrap();
    assert_eq!(
        clock.deadline(start + Duration::from_secs(51)).unwrap(),
        start + Duration::from_secs(56)
    );
    assert!(clock.deadline(start + Duration::from_secs(56)).is_err());
}
#[test]
fn typed_io_source_survives_the_client_boundary() {
    #[derive(Debug)]
    struct Reset;
    impl fmt::Display for Reset {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("reset cause")
        }
    }
    impl error::Error for Reset {}
    let backend = Backend::at_path(PathBuf::from("fixture"));
    let error = Error::new(
        &backend,
        Phase::Response,
        true,
        io::Error::new(io::ErrorKind::ConnectionReset, Reset),
    );
    let Cause::Io(cause) = &error.cause else {
        panic!("typed IO cause");
    };
    assert_eq!(cause.kind(), io::ErrorKind::ConnectionReset);
    assert!(cause.get_ref().unwrap().is::<Reset>());
    assert_eq!(error.retry(), common::Retry::Unknown);
}

#[test]
fn refused_git_mutation_is_known_unsent_after_successful_preflight() {
    use newport_protocol::git;
    let host = Host::with_listener(|listener| {
        let (mut preflight, _) = listener.accept().unwrap();
        let request = admitted(&mut preflight);
        assert!(
            matches!(&request.operation, desktop::Operation::Git(operation)
            if matches!(operation.as_ref(), desktop::git::Operation::PendingOperations(_)))
        );
        let reply = desktop::ServerMessage::Response(Box::new(desktop::Response {
            id: request.id,
            outcome: desktop::Outcome::Completed(Box::new(desktop::Reply::Git(Box::new(
                desktop::git::Reply::PendingOperations(Box::new(desktop::git::Receipts {
                    receipts: vec![],
                })),
            )))),
        }));
        preflight
            .write_all(&wire::encode_server_for(&reply, &request).unwrap())
            .unwrap();
        drop(preflight);
        let (mut mutation, _) = listener.accept().unwrap();
        mutation
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        mutation
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        wire::read_hello(&mut mutation).unwrap();
        let refused = desktop::ServerMessage::Refused(Box::new(common::Error {
            code: "UPGRADE_REQUIRED".into(),
            message: "Backend admission refused".into(),
            retry: common::Retry::No,
        }));
        mutation
            .write_all(&wire::encode_server(&refused).unwrap())
            .unwrap();
    });
    let error = host
        .backend
        .bootstrap(
            &uuid::Uuid::new_v4().to_string(),
            super::super::RepositoryCreation::Init {
                path: git::Path::new(b"/repo"),
                initial_branch: "main".into(),
            },
            uuid::Uuid::new_v4(),
        )
        .unwrap_err();
    assert!(
        !error.dispatched,
        "admission sent no mutation: {}",
        error.message
    );
    assert!(error.message.contains("Backend admission refused"));
    assert!(!error.message.contains("check the outcome"));
    host.finish();
}
