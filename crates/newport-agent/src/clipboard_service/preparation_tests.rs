use super::*;
use newport_protocol::{
    clipboard::service::{
        Hello, Request, Response, VERSION,
        wire::{self, Message},
    },
    common,
};
use std::{
    io::{Read as _, Write as _},
    os::unix::net::{UnixListener, UnixStream},
};

fn peer() -> (tempfile::TempDir, Access, UnixListener) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let access = Access {
        path: directory.path().join("clipboard.sock"),
        runtime: "runtime".into(),
        worker: "worker".into(),
        token: uuid::Uuid::new_v4().to_string(),
    };
    let listener = UnixListener::bind(&access.path).unwrap();
    (directory, access, listener)
}
fn greeting(stream: &mut UnixStream) {
    wire::write(
        stream,
        &Response::Hello(Hello {
            version: VERSION,
            runtime: "runtime".into(),
        }),
    )
    .unwrap();
}
fn authenticate(listener: &UnixListener, access: &Access) -> UnixStream {
    let (mut stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    greeting(&mut stream);
    assert_eq!(
        wire::read::<Request>(&mut stream).unwrap(),
        Request::Authenticate(access.clone())
    );
    stream
}
fn ended(stream: &mut UnixStream) {
    let mut bytes = Vec::new();
    if let Err(error) = stream.read_to_end(&mut bytes) {
        assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    }
    assert!(
        bytes.is_empty(),
        "unexpected requests before preparation committed"
    );
}
#[test]
fn unsupported_silent_or_wrong_runtime_greetings_receive_no_credentials() {
    for kind in 0..4 {
        let (_directory, access, listener) = peer();
        let task = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            match kind {
                0 => wire::write(
                    &mut stream,
                    &Response::Hello(Hello {
                        version: VERSION,
                        runtime: "another-runtime".into(),
                    }),
                )
                .unwrap(),
                1 => {
                    let mut bytes = Response::Hello(Hello {
                        version: VERSION,
                        runtime: "runtime".into(),
                    })
                    .encode()
                    .unwrap();
                    assert_eq!(&bytes[3..5], &[8, 1]);
                    bytes[4] = 2;
                    stream.write_all(&bytes).unwrap();
                }
                2 => stream
                    .write_all(&[0, 0, 0, 8, 0x81, 0xa5, b'R', b'e', b'a', b'd', b'y', 0xc0])
                    .unwrap(),
                _ => {}
            }
            ended(&mut stream);
        });
        assert!(
            NativePublisher::new_until(access, Instant::now() + Duration::from_millis(120))
                .is_err()
        );
        task.join().unwrap();
    }
}
#[test]
fn preparation_authenticates_both_handles_without_reading_or_publishing() {
    let (_directory, access, listener) = peer();
    let peer_access = access.clone();
    let (committed, proceed) = mpsc::sync_channel(1);
    let task = std::thread::spawn(move || {
        let mut reader = authenticate(&listener, &peer_access);
        wire::write(&mut reader, &Response::Ready).unwrap();
        let mut native = authenticate(&listener, &peer_access);
        wire::write(&mut native, &Response::Ready).unwrap();
        // The caller must finish its local commit before the peer handles any
        // additional request. An IPC operation in commit would deadlock here.
        proceed.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(wire::read::<Request>(&mut reader).unwrap(), Request::Offer);
        let offer = Offer {
            runtime: "runtime".into(),
            revision: 1,
            origin: Origin::Empty,
            formats: vec![],
        };
        wire::write(&mut reader, &Response::Offer(offer.clone())).unwrap();
        assert_eq!(
            wire::read::<Request>(&mut native).unwrap(),
            Request::NativePoll
        );
        wire::write(
            &mut native,
            &Response::Provider(ProviderPoll {
                lease: None,
                offer,
                requests: vec![],
            }),
        )
        .unwrap();
    });
    let cancellation = PreparationCancellation::default();
    let prepared = PreparedClipboard::prepare(
        access,
        Instant::now() + Duration::from_secs(2),
        &cancellation,
    )
    .unwrap();
    let (reader, native) = prepared.commit().unwrap();
    cancellation.cancel(); // Preparation cannot revoke an admitted viewer.
    committed.send(()).unwrap();
    assert!(reader.offer().unwrap().formats.is_empty());
    assert!(native.poll().unwrap().lease.is_none());
    task.join().unwrap();
}
#[test]
fn cancellation_interrupts_either_authentication_and_closes_both_sockets() {
    for blocked in 0..2 {
        let (_directory, access, listener) = peer();
        let peer_access = access.clone();
        let (arrived, waiting) = mpsc::sync_channel(1);
        let server = std::thread::spawn(move || {
            let mut first = authenticate(&listener, &peer_access);
            let mut second = if blocked == 1 {
                wire::write(&mut first, &Response::Ready).unwrap();
                Some(authenticate(&listener, &peer_access))
            } else {
                None
            };
            arrived.send(()).unwrap();
            ended(&mut first);
            if let Some(second) = &mut second {
                ended(second);
            }
        });
        let cancellation = PreparationCancellation::default();
        let pending = cancellation.clone();
        let (done, result) = mpsc::sync_channel(1);
        let task = std::thread::spawn(move || {
            done.send(
                PreparedClipboard::prepare(
                    access,
                    Instant::now() + Duration::from_secs(5),
                    &pending,
                )
                .is_err(),
            )
            .unwrap();
        });
        waiting.recv_timeout(Duration::from_secs(2)).unwrap();
        cancellation.cancel();
        cancellation.cancel();
        assert!(result.recv_timeout(Duration::from_millis(500)).unwrap());
        task.join().unwrap();
        server.join().unwrap();
    }
}
#[test]
fn cancellation_before_commit_or_dropping_preparation_closes_both_handles() {
    for cancel in [false, true] {
        let (_directory, access, listener) = peer();
        let peer_access = access.clone();
        let server = std::thread::spawn(move || {
            let mut first = authenticate(&listener, &peer_access);
            wire::write(&mut first, &Response::Ready).unwrap();
            let mut second = authenticate(&listener, &peer_access);
            wire::write(&mut second, &Response::Ready).unwrap();
            ended(&mut first);
            ended(&mut second);
        });
        let cancellation = PreparationCancellation::default();
        let prepared = PreparedClipboard::prepare(
            access.clone(),
            Instant::now() + Duration::from_secs(2),
            &cancellation,
        )
        .unwrap();
        if cancel {
            cancellation.cancel();
            assert!(prepared.commit().is_err());
        } else {
            drop(prepared);
        }
        assert!(
            PreparedClipboard::prepare(
                access,
                Instant::now() + Duration::from_secs(2),
                &cancellation
            )
            .is_err()
        );
        server.join().unwrap();
    }
}
#[test]
fn both_authentications_share_the_original_deadline() {
    let (_directory, access, listener) = peer();
    let peer_access = access.clone();
    let server = std::thread::spawn(move || {
        let mut first = authenticate(&listener, &peer_access);
        std::thread::sleep(Duration::from_millis(70));
        // A slow runner can pass the deadline before this reply; the client
        // has then given up on the first connection and opens no second.
        if wire::write(&mut first, &Response::Ready).is_err() {
            return;
        }
        // The second connection may also come too late to authenticate.
        listener.set_nonblocking(true).unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        let mut second = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if Instant::now() >= until {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("{error}"),
            }
        };
        // macOS refuses socket options once the client has given up and
        // closed its end, which is the outcome this test expects anyway.
        if second.set_nonblocking(false).is_err()
            || second
                .set_read_timeout(Some(Duration::from_secs(2)))
                .is_err()
        {
            return;
        }
        let hello = Response::Hello(Hello {
            version: VERSION,
            runtime: "runtime".into(),
        });
        if wire::write(&mut second, &hello).is_err() {
            return;
        }
        if let Ok(request) = wire::read::<Request>(&mut second) {
            assert_eq!(request, Request::Authenticate(peer_access.clone()));
            ended(&mut second);
        }
        ended(&mut first);
    });
    let started = Instant::now();
    let result = PreparedClipboard::prepare(
        access,
        started + Duration::from_millis(140),
        &PreparationCancellation::default(),
    );
    assert!(result.is_err());
    assert!(started.elapsed() < Duration::from_millis(500));
    server.join().unwrap();
}
#[test]
fn metadata_retains_the_final_typed_remote_error() {
    let (_directory, _service, _producer, reader, _) = fixture();
    let error = reader.export_offer().unwrap_err();
    let error = error
        .get_ref()
        .unwrap()
        .downcast_ref::<common::Error>()
        .expect("typed clipboard failure");
    assert_eq!(error.code, "operation");
    assert_eq!(error.retry, common::Retry::No);
}

#[test]
fn revoked_prepared_access_cannot_displace_another_live_reader() {
    let (_directory, service, _producer, reader, _) = fixture();
    let access = service
        .authority()
        .grant("candidate", "remote-apps")
        .unwrap();
    let prepared = PreparedClipboard::prepare(
        access,
        Instant::now() + Duration::from_secs(2),
        &PreparationCancellation::default(),
    )
    .unwrap();
    service.authority().revoke("candidate");
    let error = prepared
        .commit()
        .err()
        .expect("revoked preparation must fail");
    assert!(matches!(
        error.kind(),
        io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset
    ));
    assert!(reader.offer().unwrap().formats.is_empty());
}
#[test]
fn expired_preparation_or_unsolicited_data_cannot_commit() {
    for unsolicited in [false, true] {
        let (_directory, access, listener) = peer();
        let peer_access = access.clone();
        let (queued, waiting) = mpsc::sync_channel(1);
        let server = std::thread::spawn(move || {
            let mut first = authenticate(&listener, &peer_access);
            wire::write(&mut first, &Response::Ready).unwrap();
            let mut second = authenticate(&listener, &peer_access);
            wire::write(&mut second, &Response::Ready).unwrap();
            if unsolicited {
                wire::write(&mut first, &Response::Ready).unwrap();
            }
            queued.send(()).unwrap();
            ended(&mut first);
            ended(&mut second);
        });
        let deadline = Instant::now() + Duration::from_millis(120);
        let prepared =
            PreparedClipboard::prepare(access, deadline, &PreparationCancellation::default())
                .unwrap();
        waiting.recv_timeout(Duration::from_secs(1)).unwrap();
        if !unsolicited {
            std::thread::sleep(
                deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(5),
            );
        }
        let error = prepared
            .commit()
            .err()
            .expect("invalid preparation must fail");
        assert_eq!(
            error.kind(),
            if unsolicited {
                io::ErrorKind::InvalidData
            } else {
                io::ErrorKind::TimedOut
            }
        );
        server.join().unwrap();
    }
}
