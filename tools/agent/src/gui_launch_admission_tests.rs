use super::*;

fn fixture() -> (tempfile::TempDir, Broker) {
    let root = tempfile::tempdir_in("/tmp").unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let broker = Broker::start(root.path()).unwrap();
    (root, broker)
}
fn request() -> (uuid::Uuid, Vec<u8>) {
    let request = protocol::Request {
        id: uuid::Uuid::new_v4(),
        terminal: uuid::Uuid::new_v4(),
        application: "fixture".into(),
    };
    (
        request.id,
        wire::encode(b'L', &protocol::encode(&request).unwrap()).unwrap(),
    )
}
fn connect(root: &Path) -> UnixStream {
    let socket = UnixStream::connect(root.join("gui.sock")).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    socket
        .set_write_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    socket
}
fn requests(output: &[u8]) -> Vec<uuid::Uuid> {
    let mut input = output;
    let mut ids = Vec::new();
    while !input.is_empty() {
        let (kind, bytes) = wire::read(&mut input).unwrap();
        assert_eq!(kind, b'L');
        ids.push(protocol::decode::<protocol::Request>(&bytes).unwrap().id);
    }
    ids
}
fn closed(socket: &mut UnixStream) {
    match socket.read(&mut [0]) {
        Ok(0) => (),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe
            ) => {}
        result => panic!("Rejected helper remained connected: {result:?}"),
    }
}
fn no_reply(socket: &mut UnixStream) {
    socket.set_nonblocking(true).unwrap();
    assert_eq!(
        socket.read(&mut [0]).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    socket.set_nonblocking(false).unwrap();
}

#[test]
fn fragmented_header_and_body_emit_one_request_only() {
    let (root, mut broker) = fixture();
    let mut socket = connect(root.path());
    let (id, frame) = request();
    let mut output = Vec::new();
    let mut start = 0;
    for end in [1, 4, 5, 8, frame.len() - 1, frame.len()] {
        socket.write_all(&frame[start..end]).unwrap();
        broker.poll(&mut output).unwrap();
        assert_eq!(output.is_empty(), end < frame.len());
        start = end;
    }
    assert_eq!(requests(&output), vec![id]);
    output.clear();
    for _ in 0..3 {
        broker.poll(&mut output).unwrap();
    }
    assert!(output.is_empty());
    assert!(broker.admitting.is_empty());
}

#[test]
fn incomplete_queue_has_a_hard_limit_and_recovers_after_eof() {
    let (root, mut broker) = fixture();
    let mut idle = Vec::new();
    let mut output = Vec::new();
    for _ in 0..MAX_ADMITTING / ACCEPTS_PER_POLL {
        idle.extend((0..ACCEPTS_PER_POLL).map(|_| connect(root.path())));
        broker.poll(&mut output).unwrap();
    }
    assert_eq!(broker.admitting.len(), MAX_ADMITTING);
    let mut rejected = connect(root.path());
    rejected.write_all(&request().1).unwrap();
    broker.poll(&mut output).unwrap();
    closed(&mut rejected);
    assert_eq!(broker.admitting.len(), MAX_ADMITTING);
    assert!(output.is_empty());
    drop(idle);
    broker.poll_until(&mut output, |broker, _| broker.admitting.is_empty());
    assert!(broker.admitting.is_empty());
    let mut prompt = connect(root.path());
    let (id, frame) = request();
    prompt.write_all(&frame).unwrap();
    broker.poll(&mut output).unwrap();
    assert_eq!(requests(&output), vec![id]);
}

#[test]
fn trickled_bytes_never_extend_the_absolute_deadline() {
    let (root, mut broker) = fixture();
    let mut socket = connect(root.path());
    let (_, frame) = request();
    let started = Instant::now();
    let mut output = Vec::new();
    socket.write_all(&frame[..1]).unwrap();
    broker.poll_at(&mut output, started).unwrap();
    socket.write_all(&frame[1..4]).unwrap();
    broker
        .poll_at(
            &mut output,
            started + ADMISSION_DEADLINE - Duration::from_millis(1),
        )
        .unwrap();
    assert_eq!(broker.admitting.len(), 1);
    socket.write_all(&frame[4..]).unwrap();
    broker
        .poll_at(&mut output, started + ADMISSION_DEADLINE)
        .unwrap();
    assert!(broker.admitting.is_empty());
    assert!(output.is_empty());
    closed(&mut socket);
    let mut prompt = connect(root.path());
    let (id, frame) = request();
    prompt.write_all(&frame).unwrap();
    broker
        .poll_at(&mut output, started + ADMISSION_DEADLINE)
        .unwrap();
    assert_eq!(requests(&output), vec![id]);
}

#[test]
fn malformed_lengths_frames_and_partial_eof_release_admission() {
    let (root, mut broker) = fixture();
    for bytes in [
        vec![b'M', 0, 0, 0, 0],
        vec![b'M', 255, 255, 255, 255],
        vec![b'X', 0, 0, 0, 1, 0],
        vec![b'M', 0, 0, 0, 1, 0],
        wire::encode(b'H', &[]).unwrap(),
    ] {
        let mut socket = connect(root.path());
        socket.write_all(&bytes).unwrap();
        let mut output = Vec::new();
        broker.poll(&mut output).unwrap();
        assert!(broker.admitting.is_empty());
        assert!(broker.pending.is_empty());
        assert!(output.is_empty());
        closed(&mut socket);
    }
    for count in [0, 3, 8] {
        let mut socket = connect(root.path());
        socket.write_all(&request().1[..count]).unwrap();
        let mut output = Vec::new();
        broker.poll(&mut output).unwrap();
        drop(socket);
        broker.poll_until(&mut output, |broker, _| broker.admitting.is_empty());
        assert!(broker.admitting.is_empty());
        assert!(output.is_empty());
    }
    let mut socket = connect(root.path());
    let (id, frame) = request();
    socket.write_all(&frame).unwrap();
    let mut output = Vec::new();
    broker.poll(&mut output).unwrap();
    assert_eq!(requests(&output), vec![id]);
}

#[test]
fn every_existing_client_progresses_under_four_new_accepts_per_turn() {
    let (root, mut broker) = fixture();
    let mut old = connect(root.path());
    let (old_id, frame) = request();
    let mut output = Vec::new();
    broker.poll(&mut output).unwrap();
    let mut clients = Vec::new();
    let mut expected = Vec::new();
    for part in frame.chunks(frame.len().div_ceil(6)) {
        old.write_all(part).unwrap();
        for _ in 0..ACCEPTS_PER_POLL {
            let mut socket = connect(root.path());
            let (id, frame) = request();
            expected.push(id);
            socket.write_all(&frame).unwrap();
            clients.push(socket);
        }
        broker.poll(&mut output).unwrap();
    }
    expected.push(old_id);
    let mut actual = requests(&output);
    assert_eq!(actual.iter().filter(|id| **id == old_id).count(), 1);
    actual.sort();
    expected.sort();
    assert_eq!(actual, expected);
    output.clear();
    broker.poll(&mut output).unwrap();
    assert!(output.is_empty());
}

#[test]
fn large_incomplete_body_is_bounded_and_does_not_starve_prompt_clients() {
    let (root, mut broker) = fixture();
    let mut slow = connect(root.path());
    let frame = wire::encode(b'L', &vec![0; protocol::MAX_MESSAGE]).unwrap();
    slow.write_all(&frame[..5 + ADMISSION_READ_BYTES]).unwrap();
    let mut output = Vec::new();
    broker.poll(&mut output).unwrap();
    assert_eq!(broker.admitting.len(), 1);
    assert_eq!(broker.admitting[0].body_read, ADMISSION_READ_BYTES);
    assert!(broker.admitting[0].body.len() <= protocol::MAX_MESSAGE + 16);
    let mut prompt = connect(root.path());
    let (id, bytes) = request();
    prompt.write_all(&bytes).unwrap();
    slow.write_all(&frame[5 + ADMISSION_READ_BYTES..5 + ADMISSION_READ_BYTES * 2])
        .unwrap();
    broker.poll(&mut output).unwrap();
    assert_eq!(requests(&output), vec![id]);
    assert_eq!(broker.admitting[0].body_read, ADMISSION_READ_BYTES * 2);
    output.clear();
    for part in frame[5 + ADMISSION_READ_BYTES * 2..].chunks(ADMISSION_READ_BYTES) {
        slow.write_all(part).unwrap();
        broker.poll(&mut output).unwrap();
    }
    assert!(broker.admitting.is_empty());
    assert!(output.is_empty());
    closed(&mut slow);
}

#[test]
fn fragmented_shared_requests_keep_reply_id_epoch_and_negotiation() {
    let (root, mut broker) = fixture();
    broker.enable_native_wayland();
    let mut waiting = Vec::new();
    let mut output = Vec::new();
    for _ in 0..2 {
        let mut socket = connect(root.path());
        let request = protocol::SharedRequest {
            id: uuid::Uuid::new_v4(),
            terminal: uuid::Uuid::new_v4(),
            epoch: uuid::Uuid::new_v4(),
            application: "fixture".into(),
            session_bus: true,
            native_wayland: true,
            application_environment: false,
            application_environment_v2: false,
        };
        let frame = wire::encode(b'l', &protocol::encode(&request).unwrap()).unwrap();
        socket.write_all(&frame[..8]).unwrap();
        broker.poll(&mut output).unwrap();
        socket.write_all(&frame[8..]).unwrap();
        broker.poll(&mut output).unwrap();
        waiting.push((socket, request));
    }
    let mut reply = protocol::SharedReply {
        id: waiting[0].1.id,
        epoch: uuid::Uuid::new_v4(),
        environment: None,
        error: Some("fixture reply".into()),
    };
    broker
        .reply(b'j', &protocol::encode(&reply).unwrap())
        .unwrap();
    for (socket, _) in &mut waiting {
        no_reply(socket);
    }
    reply.id = uuid::Uuid::new_v4();
    reply.epoch = waiting[0].1.epoch;
    broker
        .reply(b'j', &protocol::encode(&reply).unwrap())
        .unwrap();
    for (socket, _) in &mut waiting {
        no_reply(socket);
    }
    reply.id = waiting[1].1.id;
    reply.epoch = waiting[1].1.epoch;
    broker
        .reply(b'j', &protocol::encode(&reply).unwrap())
        .unwrap();
    let (_, bytes) = wire::read(&mut waiting[1].0).unwrap();
    assert_eq!(
        protocol::decode::<protocol::SharedReply>(&bytes)
            .unwrap()
            .id,
        reply.id
    );
    broker
        .reply(b'j', &protocol::encode(&reply).unwrap())
        .unwrap();
    no_reply(&mut waiting[0].0);
    reply.id = waiting[0].1.id;
    reply.epoch = waiting[0].1.epoch;
    broker
        .reply(b'j', &protocol::encode(&reply).unwrap())
        .unwrap();
    let (_, bytes) = wire::read(&mut waiting[0].0).unwrap();
    assert_eq!(
        protocol::decode::<protocol::SharedReply>(&bytes)
            .unwrap()
            .epoch,
        reply.epoch
    );
    assert!(broker.pending.is_empty());
}

#[test]
fn unnegotiated_or_invalid_shared_requests_never_reach_the_backend() {
    let (root, mut broker) = fixture();
    let mut request = protocol::SharedRequest {
        id: uuid::Uuid::new_v4(),
        terminal: uuid::Uuid::new_v4(),
        epoch: uuid::Uuid::new_v4(),
        application: "fixture".into(),
        session_bus: true,
        native_wayland: true,
        application_environment: true,
        application_environment_v2: false,
    };
    for mode in 0..5 {
        match mode {
            1 => broker.enable_shared(),
            2 => broker.enable_session_bus(),
            3 => broker.enable_native_wayland(),
            4 => {
                broker.enable_application_environment();
                request.epoch = uuid::Uuid::nil();
            }
            _ => (),
        }
        let mut socket = connect(root.path());
        wire::write(&mut socket, b'l', &protocol::encode(&request).unwrap()).unwrap();
        let mut output = Vec::new();
        broker.poll(&mut output).unwrap();
        assert!(output.is_empty());
        assert!(broker.admitting.is_empty());
        assert!(broker.pending.is_empty());
        closed(&mut socket);
    }
}

#[test]
fn a_second_frame_on_one_connection_cancels_without_replay() {
    let (root, mut broker) = fixture();
    let mut socket = connect(root.path());
    let (id, frame) = request();
    socket.write_all(&frame).unwrap();
    socket.write_all(&frame).unwrap();
    let mut output = Vec::new();
    broker.poll(&mut output).unwrap();
    assert_eq!(requests(&output), vec![id]);
    output.clear();
    broker.poll(&mut output).unwrap();
    let (kind, bytes) = wire::read(&mut output.as_slice()).unwrap();
    assert_eq!(kind, b'K');
    assert_eq!(protocol::decode::<uuid::Uuid>(&bytes).unwrap(), id);
    output.clear();
    broker.poll(&mut output).unwrap();
    assert!(output.is_empty());
    closed(&mut socket);
}

#[test]
fn expanded_application_policy_requires_its_own_broker_negotiation() {
    let (root, mut broker) = fixture();
    broker.enable_application_environment();
    let request = protocol::SharedRequest {
        id: uuid::Uuid::new_v4(),
        terminal: uuid::Uuid::new_v4(),
        epoch: uuid::Uuid::new_v4(),
        application: "fixture".into(),
        session_bus: true,
        native_wayland: true,
        application_environment: true,
        application_environment_v2: true,
    };
    let mut rejected = connect(root.path());
    wire::write(&mut rejected, b'l', &protocol::encode(&request).unwrap()).unwrap();
    let mut output = Vec::new();
    broker.poll(&mut output).unwrap();
    assert!(output.is_empty());
    assert!(broker.pending.is_empty());
    closed(&mut rejected);
    broker.enable_application_environment_v2();
    let mut accepted = connect(root.path());
    wire::write(&mut accepted, b'l', &protocol::encode(&request).unwrap()).unwrap();
    broker.poll(&mut output).unwrap();
    assert!(broker.pending.contains_key(&request.id));
    let (kind, bytes) = wire::read(&mut output.as_slice()).unwrap();
    assert_eq!(kind, b'l');
    assert!(
        protocol::decode::<protocol::SharedRequest>(&bytes)
            .unwrap()
            .application_environment_v2
    );
}
