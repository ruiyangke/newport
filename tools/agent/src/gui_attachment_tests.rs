use super::*;
use protocol::attachment as check;
fn request() -> check::Check {
    check::Check {
        id: uuid::Uuid::new_v4(),
        session: uuid::Uuid::new_v4(),
        terminal: uuid::Uuid::new_v4(),
        epoch: uuid::Uuid::new_v4(),
        endpoints: check::Endpoints {
            display: Some(":77".into()),
            authority: Some("/tmp/auth".into()),
            wayland_socket: "/tmp/wayland-0".into(),
        },
    }
}
fn connect(root: &Path, request: &check::Check) -> UnixStream {
    let mut socket = UnixStream::connect(root.join("gui.sock")).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    wire::write(
        &mut socket,
        check::REQUEST,
        &check::encode(request).unwrap(),
    )
    .unwrap();
    socket
}
#[test]
fn checks_require_independent_negotiation_and_emit_once() {
    let root = tempfile::tempdir_in("/tmp").unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut broker = Broker::start(root.path()).unwrap();
    broker.enable_native_wayland();
    let request = request();
    let mut unnegotiated = connect(root.path(), &request);
    let mut output = Vec::new();
    broker.poll(&mut output).unwrap();
    assert!(output.is_empty());
    assert!(wire::read(&mut unnegotiated).is_err());
    broker.enable_attachment_checks();
    let mut socket = connect(root.path(), &request);
    broker.poll(&mut output).unwrap();
    broker.poll(&mut output).unwrap();
    let mut input = output.as_slice();
    let (kind, bytes) = wire::read(&mut input).unwrap();
    assert_eq!(kind, check::REQUEST);
    assert_eq!(check::decode::<check::Check>(&bytes).unwrap(), request);
    assert!(input.is_empty());
    let mut duplicate = connect(root.path(), &request);
    output.clear();
    broker.poll(&mut output).unwrap();
    assert!(output.is_empty());
    let (kind, bytes) = wire::read(&mut duplicate).unwrap();
    assert_eq!(kind, check::REPLY);
    assert!(matches!(
        check::decode::<check::Checked>(&bytes)
            .unwrap()
            .accept(&request)
            .unwrap(),
        check::Outcome::Error { .. }
    ));
    assert_eq!(broker.pending.len(), 1);
    for key in ["id", "session", "terminal", "epoch"] {
        let mut value =
            serde_json::to_value(check::Checked::new(&request, check::Outcome::Current {}))
                .unwrap();
        value[key] = uuid::Uuid::new_v4().to_string().into();
        broker
            .reply(check::REPLY, &serde_json::to_vec(&value).unwrap())
            .unwrap();
        assert_eq!(broker.pending.len(), 1);
    }
    // A GUI launch response cannot satisfy a check sharing its UUID/epoch.
    broker
        .reply(
            b'j',
            &protocol::encode(&protocol::SharedReply {
                id: request.id,
                epoch: request.epoch,
                environment: Some(protocol::SharedGuiEnvironment {
                    session: request.session,
                    display: None,
                    authority: None,
                    wayland_socket: "/tmp/unrelated-wayland".into(),
                    fallback_bus: Some("unix:path=/tmp/unrelated-bus".into()),
                    toolkit: Default::default(),
                    application: Default::default(),
                }),
                error: None,
            })
            .unwrap(),
        )
        .unwrap();
    assert_eq!(broker.pending.len(), 1);
    let reply = check::Checked::new(&request, check::Outcome::Current {});
    broker
        .reply(check::REPLY, &check::encode(&reply).unwrap())
        .unwrap();
    assert_eq!(wire::read(&mut socket).unwrap().0, check::REPLY);
    assert!(broker.pending.is_empty());
    broker
        .reply(check::REPLY, &check::encode(&reply).unwrap())
        .unwrap();
    assert!(wire::read(&mut socket).is_err(), "no duplicate response");
}
#[test]
fn checks_expire_without_cancelling_launches_and_pending_limit_recovers() {
    let root = tempfile::tempdir_in("/tmp").unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut broker = Broker::start(root.path()).unwrap();
    broker.enable_attachment_checks();
    let mut sockets = Vec::new();
    let mut output = Vec::new();
    let now = Instant::now();
    for _ in 0..protocol::MAX_PENDING {
        sockets.push(connect(root.path(), &request()));
        broker.poll_at(&mut output, now).unwrap();
    }
    assert_eq!(broker.pending.len(), protocol::MAX_PENDING);
    let extra = request();
    let mut rejected = connect(root.path(), &extra);
    output.clear();
    broker.poll_at(&mut output, now).unwrap();
    assert!(output.is_empty());
    let (kind, bytes) = wire::read(&mut rejected).unwrap();
    assert_eq!(kind, check::REPLY);
    assert!(matches!(
        check::decode::<check::Checked>(&bytes)
            .unwrap()
            .accept(&extra)
            .unwrap(),
        check::Outcome::Error { .. }
    ));
    broker
        .poll_at(&mut output, now + Duration::from_secs(check::WAIT_SECONDS))
        .unwrap();
    assert!(
        output.is_empty(),
        "check timeout must not send launch cancellation"
    );
    assert!(broker.pending.is_empty());
    let _socket = connect(root.path(), &request());
    broker.poll(&mut output).unwrap();
    assert_eq!(broker.pending.len(), 1);
}
