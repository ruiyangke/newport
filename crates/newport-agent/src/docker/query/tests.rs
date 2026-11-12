use super::*;
use crate::docker::testing::Fixture;

#[test]
fn cancellation_interrupts_a_live_response_wait() {
    let fixture = Fixture::new(|_| {
        std::thread::sleep(Duration::from_millis(200));
        (200, "{}".into())
    });
    let cancelled = || !fixture.requests.lock().unwrap().is_empty();
    let start = Instant::now();
    let error = Query::new(start + Duration::from_secs(5), 1024, &cancelled)
        .request(&fixture.socket, "GET", "/slow")
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn response_wait_and_all_requests_share_the_deadline() {
    let fixture = Fixture::new(|_| {
        std::thread::sleep(Duration::from_millis(150));
        (200, "{}".into())
    });
    let query = Query::new(Instant::now() + Duration::from_millis(40), 1024, &|| false);
    assert_eq!(
        query
            .request(&fixture.socket, "GET", "/slow")
            .unwrap_err()
            .kind(),
        io::ErrorKind::TimedOut
    );
    assert_eq!(
        query
            .request(&fixture.socket, "GET", "/again")
            .unwrap_err()
            .kind(),
        io::ErrorKind::TimedOut
    );
    assert_eq!(*fixture.requests.lock().unwrap(), ["/slow"]);
}

#[test]
fn declared_response_size_is_checked_before_decoding() {
    let fixture = Fixture::new(|_| (200, "x".repeat(8192)));
    let error = Query::new(Instant::now() + Duration::from_secs(1), 1024, &|| false)
        .request(&fixture.socket, "GET", "/large")
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::FileTooLarge);
    let oversized = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\nx", usize::MAX);
    assert!(!complete(oversized.as_bytes()));
}

#[test]
fn denied_candidate_does_not_hide_an_accessible_engine() {
    let denied = Fixture::new(|_| (403, "{}".into()));
    let available = Fixture::new(|_| (200, r#"{"ApiVersion":"1.47"}"#.into()));
    let query = Query::new(Instant::now() + Duration::from_secs(1), 1024, &|| false);
    let endpoint =
        Endpoint::discover(&[denied.socket.clone(), available.socket.clone()], &query).unwrap();
    assert_eq!(endpoint.socket, available.socket);
    let error = Endpoint::discover(std::slice::from_ref(&denied.socket), &query)
        .err()
        .unwrap();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
}

#[test]
fn missing_and_invalid_engines_are_distinct_failures() {
    let query = Query::new(Instant::now() + Duration::from_secs(1), 1024, &|| false);
    assert!(matches!(
        Endpoint::discover(&[], &query),
        Err(QueryError::Missing)
    ));
    let fixture = Fixture::new(|_| (200, "not json".into()));
    let error = Endpoint::connect(&fixture.socket, &query).err().unwrap();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(std::error::Error::source(&error).is_some());
}

#[cfg(target_os = "linux")]
#[test]
fn a_full_unix_socket_backlog_does_not_block_cancellation_or_timeout() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("full.sock");
    let listener = Socket::new(Domain::UNIX, Type::STREAM, None).unwrap();
    let address = SockAddr::unix(&path).unwrap();
    listener.bind(&address).unwrap();
    listener.listen(1).unwrap();
    let mut clients = Vec::new();
    let mut full = false;
    for _ in 0..16 {
        let client = Socket::new(Domain::UNIX, Type::STREAM, None).unwrap();
        client.set_nonblocking(true).unwrap();
        if client
            .connect(&address)
            .is_err_and(|error| error.kind() == io::ErrorKind::WouldBlock)
        {
            full = true;
            break;
        }
        clients.push(client);
    }
    assert!(full);
    let start = Instant::now();
    let error = Query::new(start + Duration::from_millis(30), 1024, &|| false)
        .request(&path, "GET", "/version")
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(start.elapsed() < Duration::from_secs(1));
    let error = Query::new(start + Duration::from_secs(10), 1024, &|| true)
        .request(&path, "GET", "/version")
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Interrupted);
}
