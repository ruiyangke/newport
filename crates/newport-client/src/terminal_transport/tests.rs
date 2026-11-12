use super::*;
use newport_protocol::encoding::protobuf;
use std::{
    io::{Read, Write},
    sync::mpsc,
};

struct Fake {
    opens: Mutex<Vec<stream::TerminalOpenArgs>>,
    peers: mpsc::Sender<Stream>,
    refuse: Option<String>,
}
impl Lease for Fake {
    fn open(&self, args: stream::TerminalOpenArgs) -> Result<Opened, String> {
        let session = args.session;
        self.opens.lock().unwrap().push(args);
        if let Some(error) = &self.refuse {
            return Err(error.clone());
        }
        let (client, peer) = Stream::pair().map_err(|error| error.to_string())?;
        let writer = Writer::new(client.try_clone().map_err(|error| error.to_string())?);
        self.peers.send(peer).unwrap();
        Ok(Opened {
            identity: stream::Opened::Terminal(Box::new(stream::TerminalOpened {
                version: desktop::VERSION,
                session,
            })),
            reader: Reader::new(client),
            writer,
        })
    }
}
struct Peer {
    stream: Stream,
}
impl Peer {
    fn send(&mut self, value: TerminalServer) {
        self.stream
            .write_all(&wire::encode_terminal_server(&value).unwrap())
            .unwrap();
    }
    fn event(&mut self, value: TerminalEvent) {
        self.send(TerminalServer::Event(Box::new(value)));
    }
    fn output(&mut self, bytes: &[u8]) {
        self.send(TerminalServer::Output(Box::new(stream::Output {
            data: bytes.to_vec().into(),
        })));
    }
    fn next(&mut self) -> Option<TerminalClient> {
        protobuf::read_frame(&mut self.stream, desktop::TERMINAL_CLIENT_LIMITS.message)
            .unwrap()
            .map(|body| wire::decode_terminal_client(body).unwrap())
    }
    fn close(self) {
        drop(self);
    }
    fn quiet(&mut self) -> bool {
        self.stream
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let result =
            protobuf::read_frame(&mut self.stream, desktop::TERMINAL_CLIENT_LIMITS.message);
        self.stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        matches!(result, Err(protobuf::FrameError::Io(error)) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut))
    }
}
fn start_holding(hold: Hold, refuse: Option<String>) -> (Session, Arc<Fake>, Receiver<Stream>) {
    let (peers, receiver) = mpsc::channel();
    let fake = Arc::new(Fake {
        opens: Mutex::default(),
        peers,
        refuse,
    });
    let session = Session::start(
        fake.clone(),
        &uuid::Uuid::new_v4().to_string(),
        hold,
        80,
        24,
    )
    .unwrap();
    (session, fake, receiver)
}
fn start() -> (Session, Peer) {
    let (session, _, receiver) = start_holding(Hold::No, None);
    let stream = receiver.recv_timeout(Duration::from_secs(3)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    (session, Peer { stream })
}
fn ready() -> (Session, Peer) {
    let (session, mut peer) = start();
    peer.event(TerminalEvent::Ready(Box::new(Present {})));
    assert_eq!(next(&session), Event::Ready);
    assert!(session.ready());
    (session, peer)
}
fn next(session: &Session) -> Event {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match session.try_read() {
            Ok(event) => return event,
            Err(mpsc::TryRecvError::Disconnected) => panic!("terminal event stream ended"),
            Err(mpsc::TryRecvError::Empty) => {}
        }
        assert!(Instant::now() < deadline, "terminal event stalled");
        thread::sleep(Duration::from_millis(2));
    }
}
fn wait(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !condition() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_terminal_opens_with_one_typed_request_carrying_its_identity_size_and_holding() {
    for hold in [
        Hold::No,
        Hold::WhenAvailable(Some([[1, 2, 3]; 3])),
        Hold::Attach(uuid::Uuid::new_v4(), None),
    ] {
        let (session, fake, receiver) = start_holding(hold, None);
        let peer = receiver.recv_timeout(Duration::from_secs(3)).unwrap();
        let requests = fake.opens.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.session, session.id());
        assert_eq!((request.columns, request.rows), (80, 24));
        assert_eq!(request.persist, matches!(hold, Hold::WhenAvailable(_)));
        assert_eq!(request.attach, matches!(hold, Hold::Attach(..)));
        if let Hold::Attach(id, _) = hold {
            assert_eq!(request.session, id);
        }
        drop(requests);
        drop(session);
        drop(peer);
    }
}
#[test]
fn ending_sends_end_and_keeps_the_socket_until_confirmed_exit_and_closure() {
    let (session, mut peer) = ready();
    session.end();
    assert!(matches!(peer.next(), Some(TerminalClient::End(_))));
    peer.output(b"bye");
    assert!(matches!(peer.next(), Some(TerminalClient::Ack(value)) if value.bytes == 3));
    peer.event(TerminalEvent::Exit(Box::new(stream::Exit {
        code: Some(0),
    })));
    peer.close();
}
#[test]
fn restore_and_persistent_events_preserve_order_and_only_ready_allows_input() {
    let (session, mut peer) = start();
    assert!(session.send(vec![1]).is_err());
    peer.event(TerminalEvent::Persistent(Box::new(Present {})));
    peer.send(TerminalServer::Restore(Box::new(stream::Restore {
        state: vec![27, b'[', b'H'].into(),
        prompted: true,
    })));
    peer.event(TerminalEvent::Ready(Box::new(Present {})));
    assert_eq!(next(&session), Event::Persistent);
    assert_eq!(
        next(&session),
        Event::Restore {
            state: vec![27, b'[', b'H'],
            prompted: true
        }
    );
    assert_eq!(next(&session), Event::Ready);
    session.send(vec![1]).unwrap();
    assert!(
        matches!(peer.next(), Some(TerminalClient::Input(value)) if value.epoch == 0 && value.data.as_ref() == [1])
    );
}
#[test]
fn a_refused_open_ends_the_session_with_its_reason() {
    let (session, _, _) = start_holding(Hold::No, Some("holder requires reopening".into()));
    assert_eq!(
        next(&session),
        Event::Error("holder requires reopening".into())
    );
    wait(|| session.is_closed());
}
#[test]
fn input_and_resize_reach_the_backend_in_order() {
    let (session, mut peer) = ready();
    session.send(vec![1, 2]).unwrap();
    session.resize(120, 40).unwrap();
    session.send(vec![3]).unwrap();
    assert!(
        matches!(peer.next(), Some(TerminalClient::Input(value)) if value.epoch == 0 && value.data.as_ref() == [1,2])
    );
    assert!(
        matches!(peer.next(), Some(TerminalClient::Resize(value)) if (value.columns,value.rows) == (120,40))
    );
    assert!(
        matches!(peer.next(), Some(TerminalClient::Input(value)) if value.data.as_ref() == [3])
    );
    assert!(session.send(vec![0; desktop::MAX_DATA + 1]).is_err());
}
#[test]
fn stale_input_is_refused_and_new_input_names_the_replacement_epoch() {
    let (session, mut peer) = ready();
    let previous = session.epoch();
    peer.event(TerminalEvent::Reconnecting(Box::new(
        stream::Reconnecting {
            attempt: 1,
            message: "reconnecting".into(),
        },
    )));
    assert!(matches!(next(&session), Event::Reconnecting { .. }));
    assert!(session.send_for_epoch(vec![1], previous).is_err());
    peer.event(TerminalEvent::Ready(Box::new(Present {})));
    assert_eq!(next(&session), Event::Ready);
    session.send(vec![2]).unwrap();
    assert!(
        matches!(peer.next(), Some(TerminalClient::Input(value)) if value.epoch == previous + 1 && value.data.as_ref() == [2])
    );
}
#[test]
fn output_is_acknowledged_only_when_the_view_reads_it() {
    let (session, mut peer) = ready();
    peer.output(&[0, 255, 13, 10]);
    assert!(peer.quiet());
    assert_eq!(next(&session), Event::Data(vec![0, 255, 13, 10]));
    assert!(matches!(peer.next(), Some(TerminalClient::Ack(value)) if value.bytes == 4));
}
#[test]
fn input_credit_remains_with_caller_until_backend_takes_bytes() {
    let (session, mut peer) = ready();
    let chunk = vec![b'p'; desktop::MAX_DATA];
    for _ in 0..desktop::INPUT_WINDOW as usize / desktop::MAX_DATA {
        session.send(chunk.clone()).unwrap();
        assert!(
            matches!(peer.next(), Some(TerminalClient::Input(value)) if value.data.as_ref() == chunk)
        );
    }
    assert!(session.send(vec![1]).is_err());
    peer.send(TerminalServer::Taken(Box::new(stream::Taken {
        bytes: desktop::MAX_DATA as u64,
    })));
    wait(|| session.lifecycle.unapplied.load(Ordering::Acquire) < desktop::INPUT_WINDOW);
    session.send(vec![1]).unwrap();
    assert!(
        matches!(peer.next(), Some(TerminalClient::Input(value)) if value.data.as_ref() == [1])
    );
}
#[test]
fn final_events_require_eof_and_no_trailing_frames() {
    let (session, mut peer) = ready();
    let mut bytes = wire::encode_terminal_server(&TerminalServer::Event(Box::new(
        TerminalEvent::Exit(Box::new(stream::Exit { code: Some(7) })),
    )))
    .unwrap();
    bytes.extend(
        wire::encode_terminal_server(&TerminalServer::Output(Box::new(stream::Output {
            data: vec![1].into(),
        })))
        .unwrap(),
    );
    peer.stream.write_all(&bytes).unwrap();
    peer.close();
    assert!(matches!(next(&session), Event::Error(message) if message.contains("final outcome")));
    let (session, mut peer) = ready();
    peer.event(TerminalEvent::Exit(Box::new(stream::Exit {
        code: Some(7),
    })));
    peer.close();
    assert_eq!(next(&session), Event::Exit(Some(7)));
}
#[test]
fn excess_taken_credit_closes_the_stream_without_restoring_input_capacity() {
    let (session, mut peer) = ready();
    peer.send(TerminalServer::Taken(Box::new(stream::Taken { bytes: 1 })));
    assert!(matches!(next(&session), Event::Error(message) if message.contains("acknowledgement")));
    assert!(session.send(vec![1]).is_err());
}
#[test]
fn truncated_or_closed_peer_reports_failure_instead_of_exit() {
    for truncated in [false, true] {
        let (session, mut peer) = ready();
        if truncated {
            peer.stream.write_all(&[10, 8]).unwrap();
        }
        peer.close();
        assert!(matches!(next(&session), Event::Error(_)));
        wait(|| session.is_closed());
    }
}
#[test]
fn dropping_the_session_interrupts_a_blocked_reader_and_releases_the_lease() {
    let (session, mut peer) = ready();
    drop(session);
    let mut byte = [0];
    assert_eq!(peer.stream.read(&mut byte).unwrap(), 0);
}

#[test]
fn full_command_queue_preserves_acknowledgements_and_refuses_unqueued_input() {
    let (outgoing, commands) = mpsc::sync_channel(2);
    let (events, output) = mpsc::sync_channel(1);
    let lifecycle = Arc::new(Lifecycle::default());
    lifecycle.ready.store(true, Ordering::Release);
    lifecycle.flow.lock().unwrap().output(4).unwrap();
    let session = Session {
        id: uuid::Uuid::new_v4(),
        outgoing,
        output,
        lifecycle: lifecycle.clone(),
    };
    session.resize(80, 24).unwrap();
    session.resize(90, 30).unwrap();
    assert!(session.send(vec![1]).is_err());
    assert_eq!(lifecycle.unapplied.load(Ordering::Acquire), 0);
    events.send(Event::Data(vec![0; 4])).unwrap();
    assert_eq!(session.try_read().unwrap(), Event::Data(vec![0; 4]));
    assert_eq!(lifecycle.pending_acks.load(Ordering::Acquire), 4);
    let (client, peer) = Stream::pair().unwrap();
    *lifecycle.cancellation.lock().unwrap() = Some(client.try_clone().unwrap());
    let writer = thread::spawn(move || write(Writer::new(client), &commands, &lifecycle));
    let mut peer = Peer { stream: peer };
    peer.stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    assert!(matches!(peer.next(), Some(TerminalClient::Ack(value)) if value.bytes == 4));
    drop(session);
    writer.join().unwrap();
}

#[test]
fn a_large_restore_survives_the_bounded_event_queue_without_truncation() {
    let (session, mut peer) = start();
    let state = vec![b'x'; newport_protocol::terminal::holder::MAX_SNAPSHOT];
    let expected = state.clone();
    let sending = thread::spawn(move || {
        peer.send(TerminalServer::Restore(Box::new(stream::Restore {
            state: state.into(),
            prompted: true,
        })));
        peer.event(TerminalEvent::Ready(Box::new(Present {})));
        peer
    });
    assert_eq!(
        next(&session),
        Event::Restore {
            state: expected,
            prompted: true
        }
    );
    assert_eq!(next(&session), Event::Ready);
    drop(session);
    drop(sending.join().unwrap());
}
