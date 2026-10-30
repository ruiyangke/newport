//! A terminal tab's I/O: one duplex stream on the socket that holds its
//! remote PTY open (`protocol::terminal`). No terminal I/O occupies a general
//! UI request worker, and nothing on the stream times out.
use crate::backend::Backend;
use newport_platform::ipc::Stream;
use newport_platform::ipc::protocol::terminal::{self as wire, ToApp, ToBackend};
use serde_json::{Value, json};
use std::{
    io::Write,
    net::Shutdown,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
};

#[derive(Debug, PartialEq)]
pub enum Event {
    Ready,
    Data(Vec<u8>),
    Reconnecting { attempt: u32, message: String },
    Exit(Option<u32>),
    Error(String),
}
impl From<wire::Event> for Event {
    fn from(event: wire::Event) -> Self {
        match event {
            wire::Event::Ready => Self::Ready,
            wire::Event::Reconnecting { attempt, message } => {
                Self::Reconnecting { attempt, message }
            }
            wire::Event::Exit(code) => Self::Exit(code),
            wire::Event::Error(message) => Self::Error(message),
        }
    }
}

trait Lease: Send + Sync + 'static {
    fn open(&self, args: Value) -> Result<Stream, String>;
}
impl Lease for Backend {
    fn open(&self, args: Value) -> Result<Stream, String> {
        self.open_terminal_lease(args)
    }
}

enum Outgoing {
    Input { epoch: u64, bytes: Vec<u8> },
    Resize(u16, u16),
    Ack(u64),
    Close,
}

#[derive(Default)]
struct Lifecycle {
    stopped: AtomicBool,
    ready: AtomicBool,
    epoch: AtomicU64,
    /// Input bytes accepted from the caller that the backend hasn't taken.
    unapplied: AtomicU64,
}
impl Lifecycle {
    fn current(&self, epoch: u64) -> bool {
        self.ready.load(Ordering::Acquire)
            && !self.stopped.load(Ordering::Acquire)
            && self.epoch.load(Ordering::Acquire) == epoch
    }
    fn end_shell(&self) {
        self.ready.store(false, Ordering::Release);
        self.epoch.fetch_add(1, Ordering::AcqRel);
    }
    fn applied(&self, bytes: u64) {
        let _ = self
            .unapplied
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |unapplied| {
                Some(unapplied.saturating_sub(bytes))
            });
    }
}

/// Dropping a session closes its socket, which closes its remote PTY; so does
/// the end of this process.
pub struct Session {
    id: uuid::Uuid,
    outgoing: Sender<Outgoing>,
    output: Receiver<Event>,
    lifecycle: Arc<Lifecycle>,
}
impl Session {
    pub fn open(server: &str, columns: u16, rows: u16) -> Result<Self, String> {
        Self::open_with_backend(Backend::discover(), server, columns, rows)
    }

    /// Open against an explicitly owned backend without changing process-wide
    /// discovery.
    pub fn open_with_backend(
        backend: Backend,
        server: &str,
        columns: u16,
        rows: u16,
    ) -> Result<Self, String> {
        Self::start(Arc::new(backend), server, columns, rows)
    }

    fn start(lease: Arc<dyn Lease>, server: &str, columns: u16, rows: u16) -> Result<Self, String> {
        dimensions(columns, rows)?;
        uuid::Uuid::parse_str(server).map_err(|_| "Invalid terminal server ID")?;
        let id = uuid::Uuid::new_v4();
        let (outgoing, commands) = mpsc::channel();
        let (events, output) = mpsc::channel();
        let lifecycle = Arc::new(Lifecycle::default());
        let state = lifecycle.clone();
        let closer = outgoing.clone();
        let server = server.to_owned();
        thread::spawn(move || {
            // The backend joins the server's shared display when it can; the
            // shell never waits for it and never fails because of it.
            let request = json!({"id":server,"session":id,"cols":columns,"rows":rows});
            let opened = lease.open(request).and_then(|stream| {
                let reader = stream.try_clone().map_err(|error| error.to_string())?;
                Ok((stream, reader))
            });
            let (stream, reader) = match opened {
                Ok(pair) => pair,
                Err(error) => {
                    let _ = events.send(Event::Error(error));
                    state.stopped.store(true, Ordering::Release);
                    return;
                }
            };
            let reader = {
                let state = state.clone();
                thread::spawn(move || read(reader, &state, &events, &closer))
            };
            write(stream, &commands, &state);
            let _ = reader.join();
        });
        Ok(Self {
            id,
            outgoing,
            output,
            lifecycle,
        })
    }

    pub fn id(&self) -> uuid::Uuid {
        self.id
    }
    pub fn is_closed(&self) -> bool {
        self.lifecycle.stopped.load(Ordering::Acquire)
    }
    pub fn ready(&self) -> bool {
        self.lifecycle.ready.load(Ordering::Acquire) && !self.is_closed()
    }
    pub fn send(&self, bytes: Vec<u8>) -> Result<(), String> {
        self.send_for_epoch(bytes, self.epoch())
    }
    pub fn epoch(&self) -> u64 {
        self.lifecycle.epoch.load(Ordering::Acquire)
    }
    /// Input for the shell of `epoch`. While the shell is slower than the
    /// input, this refuses it and the caller keeps it queued.
    pub fn send_for_epoch(&self, bytes: Vec<u8>, epoch: u64) -> Result<(), String> {
        if bytes.len() > wire::MAX_DATA {
            return Err("Terminal input is too large.".into());
        }
        if !self.lifecycle.current(epoch) {
            return Err("Terminal is not ready.".into());
        }
        let length = bytes.len() as u64;
        self.lifecycle
            .unapplied
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |unapplied| {
                (unapplied + length <= wire::INPUT_WINDOW).then_some(unapplied + length)
            })
            .map_err(|_| "Terminal input is busy.")?;
        self.outgoing
            .send(Outgoing::Input { epoch, bytes })
            .map_err(|_| {
                self.lifecycle.applied(length);
                "Terminal is closed.".into()
            })
    }
    pub fn resize(&self, columns: u16, rows: u16) -> Result<(), String> {
        dimensions(columns, rows)?;
        if self.is_closed() {
            return Err("Terminal is closed.".into());
        }
        self.outgoing
            .send(Outgoing::Resize(columns, rows))
            .map_err(|_| "Terminal is closed.".into())
    }
    /// The next event. Output read here is acknowledged: the caller draws
    /// what it reads, and the backend sends more as it does.
    pub fn try_read(&self) -> Result<Event, mpsc::TryRecvError> {
        let event = self.output.try_recv()?;
        if let Event::Data(bytes) = &event {
            let _ = self.outgoing.send(Outgoing::Ack(bytes.len() as u64));
        }
        Ok(event)
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.lifecycle.stopped.store(true, Ordering::Release);
        self.lifecycle.ready.store(false, Ordering::Release);
        let _ = self.outgoing.send(Outgoing::Close);
    }
}

fn dimensions(columns: u16, rows: u16) -> Result<(), String> {
    if (2..=500).contains(&columns) && (1..=300).contains(&rows) {
        Ok(())
    } else {
        Err("Invalid terminal dimensions.".into())
    }
}

/// Sends what the session queues, until it closes or the socket fails, then
/// closes the socket so the backend releases the PTY.
fn write(mut stream: Stream, commands: &Receiver<Outgoing>, state: &Lifecycle) {
    while let Ok(first) = commands.recv() {
        let (mut frames, mut acked, mut closing) = (Vec::new(), 0, false);
        for command in std::iter::once(first).chain(commands.try_iter()) {
            match command {
                Outgoing::Input { epoch, bytes } => {
                    // Never send queued keystrokes to a replacement shell.
                    if state.current(epoch) {
                        frames.extend(ToBackend::Input { epoch, data: bytes }.encode());
                    } else {
                        state.applied(bytes.len() as u64);
                    }
                }
                Outgoing::Resize(cols, rows) => {
                    frames.extend(ToBackend::Resize { cols, rows }.encode());
                }
                Outgoing::Ack(bytes) => acked += bytes,
                Outgoing::Close => {
                    closing = true;
                    break;
                }
            }
        }
        if acked > 0 {
            frames.extend(ToBackend::Ack(acked).encode());
        }
        if closing || stream.write_all(&frames).is_err() {
            break;
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
}

/// Delivers the backend's frames until the shell's stream ends or the
/// socket closes.
fn read(mut stream: Stream, state: &Lifecycle, events: &Sender<Event>, writer: &Sender<Outgoing>) {
    let failure = loop {
        let frame = match wire::read(&mut stream) {
            Ok(Some((kind, payload))) => ToApp::decode(kind, payload),
            Ok(None) => break Some("Newport's backend closed the terminal.".to_owned()),
            Err(error) => {
                break Some(format!(
                    "Lost the terminal's connection to Newport's backend: {error}"
                ));
            }
        };
        match frame {
            Ok(ToApp::Output(bytes)) => {
                let _ = events.send(Event::Data(bytes));
            }
            Ok(ToApp::Taken(bytes)) => state.applied(bytes),
            Ok(ToApp::Event(event)) => {
                if event.ends_shell() {
                    state.end_shell();
                } else {
                    state.ready.store(true, Ordering::Release);
                }
                let ends = event.ends_stream();
                let _ = events.send(event.into());
                if ends {
                    break None;
                }
            }
            Err(error) => break Some(error),
        }
    };
    if let Some(error) = failure
        && !state.stopped.load(Ordering::Acquire)
    {
        state.end_shell();
        let _ = events.send(Event::Error(error));
    }
    state.stopped.store(true, Ordering::Release);
    state.ready.store(false, Ordering::Release);
    let _ = writer.send(Outgoing::Close);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::ErrorKind,
        sync::Mutex,
        time::{Duration, Instant},
    };

    struct Fake {
        opens: Mutex<Vec<Value>>,
        backends: Mutex<Sender<Stream>>,
        refuse: Option<String>,
    }
    impl Lease for Fake {
        fn open(&self, args: Value) -> Result<Stream, String> {
            self.opens.lock().unwrap().push(args);
            if let Some(error) = &self.refuse {
                return Err(error.clone());
            }
            let (app, backend) = Stream::pair().map_err(|error| error.to_string())?;
            self.backends.lock().unwrap().send(backend).unwrap();
            Ok(app)
        }
    }

    /// The backend's end of a terminal's socket.
    struct Peer(Stream);
    impl Peer {
        fn send(&mut self, frame: ToApp) {
            self.0.write_all(&frame.encode()).unwrap();
        }
        fn next(&mut self) -> Option<ToBackend> {
            let (kind, payload) = wire::read(&mut self.0).unwrap()?;
            Some(ToBackend::decode(kind, payload).unwrap())
        }
        /// Nothing more arrives from the app for a while.
        fn quiet(&mut self) -> bool {
            self.0
                .set_read_timeout(Some(Duration::from_millis(100)))
                .unwrap();
            let quiet = matches!(
                wire::read(&mut self.0),
                Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
            );
            self.0
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            quiet
        }
    }

    fn start_with(refuse: Option<String>) -> (Session, Arc<Fake>, Receiver<Stream>) {
        let (backends, opened) = mpsc::channel();
        let fake = Arc::new(Fake {
            opens: Mutex::default(),
            backends: Mutex::new(backends),
            refuse,
        });
        let session =
            Session::start(fake.clone(), &uuid::Uuid::new_v4().to_string(), 80, 24).unwrap();
        (session, fake, opened)
    }
    fn start() -> (Session, Peer) {
        let (session, _, opened) = start_with(None);
        let backend = opened.recv_timeout(Duration::from_secs(3)).unwrap();
        backend
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        (session, Peer(backend))
    }
    fn ready() -> (Session, Peer) {
        let (session, mut peer) = start();
        peer.send(ToApp::Event(wire::Event::Ready));
        wait_until(|| session.ready());
        (session, peer)
    }
    fn wait_until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !condition() {
            assert!(Instant::now() < deadline, "condition not reached");
            thread::sleep(Duration::from_millis(2));
        }
    }
    fn events(session: &Session, count: usize) -> Vec<Event> {
        let mut events = Vec::new();
        wait_until(|| {
            events.extend(session.try_read().ok());
            events.len() >= count
        });
        events
    }

    #[test]
    fn a_terminal_opens_with_one_request_carrying_only_its_identity_and_size() {
        let (session, fake, opened) = start_with(None);
        let mut peer = Peer(opened.recv_timeout(Duration::from_secs(3)).unwrap());
        peer.0
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let requests = fake.opens.lock().unwrap().clone();
        assert_eq!(requests.len(), 1);
        let server = &requests[0]["id"];
        assert_eq!(
            requests[0],
            json!({"id":server,"session":session.id(),"cols":80,"rows":24})
        );
        drop(session);
        assert_eq!(peer.next(), None);
    }

    #[test]
    fn a_refused_open_ends_the_session_with_its_reason() {
        let (session, _, _) = start_with(Some("Terminal session already exists.".into()));
        assert_eq!(
            events(&session, 1),
            [Event::Error("Terminal session already exists.".into())]
        );
        wait_until(|| session.is_closed());
        assert!(!session.ready());
    }

    #[test]
    fn input_waits_for_ready_and_reaches_the_backend_in_order() {
        let (session, mut peer) = start();
        assert!(session.send(b"early".to_vec()).is_err());
        peer.send(ToApp::Event(wire::Event::Ready));
        assert_eq!(events(&session, 1), [Event::Ready]);
        session.send(b"first".to_vec()).unwrap();
        session.resize(100, 40).unwrap();
        session.send(b"second".to_vec()).unwrap();
        assert_eq!(
            peer.next(),
            Some(ToBackend::Input {
                epoch: 0,
                data: b"first".to_vec()
            })
        );
        assert_eq!(
            peer.next(),
            Some(ToBackend::Resize {
                cols: 100,
                rows: 40
            })
        );
        assert_eq!(
            peer.next(),
            Some(ToBackend::Input {
                epoch: 0,
                data: b"second".to_vec()
            })
        );
        assert!(session.send(vec![0; wire::MAX_DATA + 1]).is_err());
        assert!(session.resize(1, 24).is_err());
    }

    #[test]
    fn input_typed_for_a_replaced_shell_is_refused_and_new_input_names_the_new_shell() {
        let (session, mut peer) = ready();
        let old = session.epoch();
        peer.send(ToApp::Event(wire::Event::Reconnecting {
            attempt: 1,
            message: "retry".into(),
        }));
        wait_until(|| session.epoch() != old);
        assert!(!session.ready());
        peer.send(ToApp::Event(wire::Event::Ready));
        wait_until(|| session.ready());
        assert!(session.send_for_epoch(b"stale".to_vec(), old).is_err());
        session.send(b"fresh".to_vec()).unwrap();
        assert_eq!(
            peer.next(),
            Some(ToBackend::Input {
                epoch: old + 1,
                data: b"fresh".to_vec()
            })
        );
        assert_eq!(
            events(&session, 3),
            [
                Event::Ready,
                Event::Reconnecting {
                    attempt: 1,
                    message: "retry".into()
                },
                Event::Ready
            ]
        );
    }

    #[test]
    fn output_is_acknowledged_only_as_it_is_read() {
        let (session, mut peer) = ready();
        assert_eq!(events(&session, 1), [Event::Ready]);
        peer.send(ToApp::Output(vec![0, 255, 13, 10]));
        peer.send(ToApp::Output(b"more".to_vec()));
        // Unread output holds its credit.
        assert!(peer.quiet());
        assert_eq!(
            events(&session, 2),
            [
                Event::Data(vec![0, 255, 13, 10]),
                Event::Data(b"more".to_vec())
            ]
        );
        let mut acked = 0;
        while acked < 8 {
            match peer.next() {
                Some(ToBackend::Ack(bytes)) => acked += bytes,
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(acked, 8);
    }

    #[test]
    fn input_beyond_the_window_stays_with_the_caller_until_the_backend_takes_it() {
        let (session, mut peer) = ready();
        let chunk = vec![b'p'; wire::MAX_DATA];
        let fits = wire::INPUT_WINDOW as usize / wire::MAX_DATA;
        for _ in 0..fits {
            session.send(chunk.clone()).unwrap();
        }
        // A shell that isn't reading its input is normal: input is refused
        // for now, and the tab stays open.
        assert_eq!(
            session.send(b"x".to_vec()).unwrap_err(),
            "Terminal input is busy."
        );
        for _ in 0..fits {
            assert!(matches!(peer.next(), Some(ToBackend::Input { data, .. }) if data == chunk));
        }
        thread::sleep(Duration::from_millis(200));
        assert!(!session.is_closed());
        assert!(session.ready());
        assert_eq!(events(&session, 1), [Event::Ready]);
        assert!(session.try_read().is_err());
        peer.send(ToApp::Taken(wire::MAX_DATA as u64));
        wait_until(|| session.send(b"x".to_vec()).is_ok());
        assert_eq!(
            peer.next(),
            Some(ToBackend::Input {
                epoch: 0,
                data: b"x".to_vec()
            })
        );
    }

    #[test]
    fn exit_and_error_events_end_the_session() {
        for (event, expected) in [
            (wire::Event::Exit(Some(7)), Event::Exit(Some(7))),
            (
                wire::Event::Error("SSH failed".into()),
                Event::Error("SSH failed".into()),
            ),
        ] {
            let (session, mut peer) = ready();
            let epoch = session.epoch();
            peer.send(ToApp::Output(b"bye".to_vec()));
            peer.send(ToApp::Event(event));
            assert_eq!(
                events(&session, 3),
                [Event::Ready, Event::Data(b"bye".to_vec()), expected]
            );
            wait_until(|| session.is_closed());
            assert_eq!(session.epoch(), epoch + 1);
            assert!(session.send(b"late".to_vec()).is_err());
            // The session closes its socket once nothing more can arrive.
            loop {
                match peer.next() {
                    Some(ToBackend::Ack(_)) => {}
                    None => break,
                    other => panic!("unexpected {other:?}"),
                }
            }
        }
    }

    #[test]
    fn a_closed_socket_ends_the_session_with_an_error() {
        let (session, peer) = ready();
        let epoch = session.epoch();
        drop(peer);
        assert_eq!(
            events(&session, 2),
            [
                Event::Ready,
                Event::Error("Newport's backend closed the terminal.".into())
            ]
        );
        assert!(session.is_closed());
        assert_eq!(session.epoch(), epoch + 1);
    }

    #[test]
    fn a_malformed_frame_ends_the_session_with_an_error() {
        let (session, mut peer) = ready();
        peer.0.write_all(&[9, 0, 0, 0, 0]).unwrap();
        let events = events(&session, 2);
        assert!(
            matches!(&events[1], Event::Error(error) if error.contains("Invalid terminal frame"))
        );
        wait_until(|| session.is_closed());
        assert_eq!(peer.next(), None);
    }
}
