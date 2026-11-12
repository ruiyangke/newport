use super::*;

fn request() -> Request {
    Request {
        url: "https://example.test/login?next=http%3A%2F%2Flocalhost%2Fcallback#keep".into(),
    }
}
fn broker() -> (tempfile::TempDir, Broker) {
    let directory = tempfile::tempdir().unwrap();
    let listener = UnixListener::bind(directory.path().join("browser.sock")).unwrap();
    listener.set_nonblocking(true).unwrap();
    (directory, Broker::new(listener, true))
}
fn queued(directory: &std::path::Path) -> UnixStream {
    let mut stream = UnixStream::connect(directory.join("browser.sock")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    wire::write_request(&mut stream, &request()).unwrap();
    stream.shutdown(Shutdown::Write).unwrap();
    stream
}
fn hello(stream: &mut UnixStream) {
    assert_eq!(
        wire::read_server(stream).unwrap(),
        LocalMessage::Hello {
            version: browser::VERSION
        }
    );
}
fn failed(stream: &mut UnixStream, expected: &str) -> RemoteError {
    let LocalMessage::Reply(Reply::Failed(error)) = wire::read_server(stream).unwrap() else {
        panic!("Expected failure");
    };
    assert_eq!(error.code, expected);
    error
}

#[test]
fn the_helper_preserves_urls_warnings_and_typed_remote_failures() {
    for reply in [
        Reply::Opened {
            warning: Some("Callback forwarding was unavailable".into()),
        },
        failure("permission_denied", "Disabled", Retry::No),
    ] {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        let expected = reply.clone();
        let peer = std::thread::spawn(move || {
            wire::write_server(
                &mut server,
                &LocalMessage::Hello {
                    version: browser::VERSION,
                },
            )
            .unwrap();
            assert_eq!(wire::read_request(&mut server).unwrap(), request());
            assert_eq!(server.read(&mut [0]).unwrap(), 0);
            wire::write_server(&mut server, &LocalMessage::Reply(reply)).unwrap();
        });
        let result = exchange(&mut client, &request(), Duration::from_secs(2));
        match expected {
            Reply::Opened { warning } => assert_eq!(result.unwrap(), warning),
            Reply::Failed(error) => assert_eq!(
                result
                    .unwrap_err()
                    .get_ref()
                    .unwrap()
                    .downcast_ref::<RemoteError>(),
                Some(&error)
            ),
        }
        peer.join().unwrap();
    }
}

#[test]
fn incompatible_or_silent_owners_receive_no_url_and_no_replay() {
    for bytes in [vec![b'M', 0, 0, 0, 0], vec![4, 10, 2, 8, 2], vec![]] {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        if !bytes.is_empty() {
            server.write_all(&bytes).unwrap();
            server.shutdown(Shutdown::Write).unwrap();
        }
        let error = exchange(&mut client, &request(), Duration::from_millis(20)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Update Needle and reconnect Newport"),
            "{error}"
        );
        assert!(
            error
                .get_ref()
                .unwrap()
                .downcast_ref::<OpenError>()
                .is_some()
        );
        if bytes.is_empty() {
            assert!(matches!(
                error.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            ));
            assert!(error.to_string().contains("did not respond"));
        } else if bytes[0] == b'M' {
            assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        } else {
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        }
        drop(client);
        let mut received = Vec::new();
        server.read_to_end(&mut received).unwrap();
        assert!(received.is_empty());
    }
}

#[test]
fn a_fragmented_announcement_cannot_extend_the_helpers_total_deadline() {
    let (mut client, mut server) = UnixStream::pair().unwrap();
    let peer = std::thread::spawn(move || {
        for byte in [4, 10, 2, 8, 1] {
            if server.write_all(&[byte]).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut received = Vec::new();
        server.read_to_end(&mut received).unwrap();
        received
    });
    let started = Instant::now();
    assert!(exchange(&mut client, &request(), Duration::from_millis(35)).is_err());
    assert!(started.elapsed() < Duration::from_millis(500));
    drop(client);
    assert!(peer.join().unwrap().is_empty());
}

#[test]
fn the_broker_waits_for_a_request_after_announcing_its_protocol() {
    let (directory, mut broker) = broker();
    let mut client = UnixStream::connect(directory.path().join("browser.sock")).unwrap();
    let peer = std::thread::spawn(move || {
        // A helper must validate Hello before sending a URL.
        crate::local_socket::read_timeout(&client, Duration::from_secs(2)).unwrap();
        hello(&mut client);
        std::thread::sleep(Duration::from_millis(10));
        wire::write_request(&mut client, &request()).unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        wire::read_server(&mut client).unwrap()
    });
    let mut output = Vec::new();
    broker.poll(&mut output).unwrap();
    broker.reply(BrowserReply {
        id: 1,
        success: true,
        message: None,
    });
    assert_eq!(
        peer.join().unwrap(),
        LocalMessage::Reply(Reply::Opened { warning: None })
    );
    assert_eq!(
        integration::wire::read::<ServerMessage>(&mut output.as_slice()).unwrap(),
        ServerMessage::Browser(BrowserRequest {
            id: 1,
            url: request().url
        })
    );
}

#[test]
fn the_broker_correlates_one_request_and_keeps_other_clients_bounded() {
    let (directory, mut broker) = broker();
    let mut first = queued(directory.path());
    let mut output = Vec::new();
    broker.poll(&mut output).unwrap();
    hello(&mut first);
    assert_eq!(
        integration::wire::read::<ServerMessage>(&mut output.as_slice()).unwrap(),
        ServerMessage::Browser(BrowserRequest {
            id: 1,
            url: request().url
        })
    );
    broker.last_open = Instant::now() - Duration::from_secs(2);
    let mut other = queued(directory.path());
    let mut other_output = Vec::new();
    broker.poll(&mut other_output).unwrap();
    hello(&mut other);
    failed(&mut other, "busy");
    assert!(other_output.is_empty());
    broker.reply(BrowserReply {
        id: 2,
        success: true,
        message: None,
    });
    assert!(broker.pending.is_some());
    broker.reply(BrowserReply {
        id: 1,
        success: true,
        message: Some("warning".into()),
    });
    assert_eq!(
        wire::read_server(&mut first).unwrap(),
        LocalMessage::Reply(Reply::Opened {
            warning: Some("warning".into())
        })
    );
    assert!(broker.pending.is_none());
}

#[test]
fn revocation_expiry_and_exhausted_ids_refuse_requests_without_wrapping() {
    for reason in ["permission_denied", "timed_out", "exhausted"] {
        let (directory, mut broker) = broker();
        if reason == "exhausted" {
            broker.request_id = u64::MAX;
        }
        let mut client = queued(directory.path());
        let mut output = Vec::new();
        broker.poll(&mut output).unwrap();
        hello(&mut client);
        match reason {
            "permission_denied" => broker.set_enabled(false),
            "timed_out" => {
                broker.pending.as_mut().unwrap().2 = Instant::now();
                broker.expire();
            }
            _ => {
                assert!(output.is_empty());
                assert_eq!(broker.request_id, u64::MAX);
            }
        }
        let error = failed(&mut client, reason);
        if reason == "timed_out" {
            assert_eq!(error.retry, Retry::Unknown);
            assert!(error.message.contains("could not be confirmed"));
        }
        assert!(broker.pending.is_none());
    }
}

#[test]
fn a_request_without_eof_or_with_trailing_data_never_reaches_the_desktop() {
    for trailing in [false, true] {
        let (directory, mut broker) = broker();
        let mut client = UnixStream::connect(directory.path().join("browser.sock")).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        wire::write_request(&mut client, &request()).unwrap();
        if trailing {
            client.write_all(&[0]).unwrap();
            client.shutdown(Shutdown::Write).unwrap();
        }
        let started = Instant::now();
        let mut output = Vec::new();
        broker.poll(&mut output).unwrap();
        assert!(started.elapsed() < Duration::from_millis(500));
        hello(&mut client);
        failed(&mut client, "invalid_request");
        assert!(output.is_empty());
        assert!(broker.pending.is_none());
    }
}
