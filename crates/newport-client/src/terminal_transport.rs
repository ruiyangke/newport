//! Dedicated PTY I/O; no terminal read occupies a general UI request worker.
use crate::backend::Backend;
use crate::request_error::RequestError;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, PartialEq, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
pub enum Event {
    Ready,
    Data(Vec<u8>),
    Reconnecting { attempt: u32, message: String },
    Exit(Option<u32>),
    Error(String),
}

trait Rpc: Send + Sync + 'static {
    fn call(&self, method: &str, args: Value) -> Result<Value, RequestError>;
    fn open(&self, args: Value) -> Result<Box<dyn Send>, String>;
}
impl Rpc for Backend {
    fn open(&self, args: Value) -> Result<Box<dyn Send>, String> {
        self.open_terminal_lease(args)
            .map(|stream| Box::new(stream) as Box<dyn Send>)
    }
    fn call(&self, method: &str, args: Value) -> Result<Value, RequestError> {
        self.call_detailed_with_timeout(method, args, Duration::from_secs(10))
    }
}

/// How long a terminal keeps retrying a refused or interrupted request before
/// it reports the failure and ends the session.
const RETRY_FOR: Duration = if cfg!(test) {
    Duration::from_millis(400)
} else {
    Duration::from_secs(10)
};

/// Retries with growing pauses, starting over after each success.
#[derive(Default)]
struct Retry {
    since: Option<Instant>,
    attempt: u32,
}
impl Retry {
    /// Wait before the next attempt, or `false` once the time is up.
    fn wait(&mut self, state: &Lifecycle) -> bool {
        let since = *self.since.get_or_insert_with(Instant::now);
        if since.elapsed() >= RETRY_FOR || state.stopped.load(Ordering::Acquire) {
            return false;
        }
        let pause = Duration::from_millis(50 << self.attempt.min(4));
        self.attempt += 1;
        thread::sleep(pause);
        true
    }
    fn succeeded(&mut self) {
        *self = Self::default();
    }
}

/// The backend refused the request before running it: always safe to repeat.
/// An interrupted request is not repeated: a write may have reached the shell,
/// and a read has taken its output off the session's queue.
fn refused(error: &RequestError) -> bool {
    error.code.as_deref() == Some(crate::backend::BUSY)
}

enum Input {
    Data { bytes: Vec<u8>, epoch: u64 },
    Resize(u16, u16),
}
#[derive(Default)]
struct Lifecycle {
    stopped: AtomicBool,
    write_failed: AtomicBool,
    ready: AtomicBool,
    epoch: AtomicU64,
}

/// Dropping a session cancels readers and closes its owned remote PTY off-thread.
/// The live ownership socket also closes the PTY after process termination.
/// Input is never retried after an uncertain RPC result.
pub struct Session {
    id: uuid::Uuid,
    input: SyncSender<Input>,
    output: Receiver<Event>,
    lifecycle: Arc<Lifecycle>,
}
impl Session {
    pub fn open(server: &str, columns: u16, rows: u16) -> Result<Self, String> {
        Self::open_with_backend(Backend::discover(), server, columns, rows)
    }

    /// Open against an explicitly owned backend without changing process-wide
    /// discovery. Retains the same terminal lease and bounded I/O transport.
    pub fn open_with_backend(
        backend: Backend,
        server: &str,
        columns: u16,
        rows: u16,
    ) -> Result<Self, String> {
        Self::start(Arc::new(backend), server, columns, rows)
    }

    fn start(rpc: Arc<dyn Rpc>, server: &str, columns: u16, rows: u16) -> Result<Self, String> {
        dimensions(columns, rows)?;
        uuid::Uuid::parse_str(server).map_err(|_| "Invalid terminal server ID")?;
        let id = uuid::Uuid::new_v4();
        let (input, commands) = mpsc::sync_channel(64);
        let (events, output) = mpsc::sync_channel(64);
        let lifecycle = Arc::new(Lifecycle::default());
        let state = lifecycle.clone();
        let server = server.to_owned();
        thread::spawn(move || {
            // The backend joins the server's shared display when it can; the
            // shell never waits for it and never fails because of it.
            let request = json!({"id":server,"session":id,"cols":columns,"rows":rows});
            let lease = match rpc.open(request) {
                Ok(lease) => Some(lease),
                Err(error) => {
                    let _ = events.try_send(Event::Error(error));
                    state.stopped.store(true, Ordering::Release);
                    None
                }
            };
            let reader = if !state.stopped.load(Ordering::Acquire) {
                let rpc = rpc.clone();
                let state = state.clone();
                let events = events.clone();
                Some(thread::spawn(move || read(rpc, id, state, events)))
            } else {
                None
            };
            let mut retry = Retry::default();
            while !state.stopped.load(Ordering::Acquire) {
                let command = match commands.recv_timeout(Duration::from_millis(50)) {
                    Ok(command) => command,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                let request = match command {
                    Input::Data { bytes, epoch } => {
                        // Never send queued keystrokes to a replacement shell.
                        if !state.ready.load(Ordering::Acquire)
                            || state.epoch.load(Ordering::Acquire) != epoch
                        {
                            continue;
                        }
                        ("terminal_write", json!({"session":id,"data":bytes}))
                    }
                    Input::Resize(columns, rows) => (
                        "terminal_resize",
                        json!({"session":id,"cols":columns,"rows":rows}),
                    ),
                };
                // Input is repeated only when the backend refused it unrun,
                // so a keystroke never reaches the shell twice.
                let result = loop {
                    match rpc.call(request.0, request.1.clone()) {
                        Err(error) if refused(&error) && retry.wait(&state) => continue,
                        result => break result,
                    }
                };
                retry.succeeded();
                if let Err(error) = result.map_err(|error| error.message) {
                    // A full output queue is normal while the UI catches up.
                    // Stop accepting input immediately, but retain the failure
                    // until the consumer reads it (or drops this session).
                    state.write_failed.store(true, Ordering::Release);
                    state.ready.store(false, Ordering::Release);
                    state.epoch.fetch_add(1, Ordering::AcqRel);
                    emit(Event::Error(error), &events, &state);
                    break;
                }
            }
            state.stopped.store(true, Ordering::Release);
            state.ready.store(false, Ordering::Release);
            // Closing the lease releases the PTY even if this process dies
            // before an explicit close RPC can run. No open is retried.
            drop(lease);
            if let Some(reader) = reader {
                let _ = reader.join();
            }
        });
        Ok(Self {
            id,
            input,
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
        self.lifecycle.ready.load(Ordering::Acquire)
            && !self.lifecycle.write_failed.load(Ordering::Acquire)
            && !self.lifecycle.stopped.load(Ordering::Acquire)
    }
    pub fn send(&self, bytes: Vec<u8>) -> Result<(), String> {
        self.send_for_epoch(bytes, self.epoch())
    }
    pub fn epoch(&self) -> u64 {
        self.lifecycle.epoch.load(Ordering::Acquire)
    }
    pub fn send_for_epoch(&self, bytes: Vec<u8>, epoch: u64) -> Result<(), String> {
        if bytes.len() > 65536 {
            return Err("Terminal input is too large.".into());
        }
        if !self.ready() || self.epoch() != epoch {
            return Err("Terminal is not ready.".into());
        }
        self.input
            .try_send(Input::Data { bytes, epoch })
            .map_err(|_| "Terminal input is busy or disconnected.".into())
    }
    pub fn resize(&self, columns: u16, rows: u16) -> Result<(), String> {
        dimensions(columns, rows)?;
        if self.lifecycle.stopped.load(Ordering::Acquire) {
            return Err("Terminal is closed.".into());
        }
        self.input
            .try_send(Input::Resize(columns, rows))
            .map_err(|_| "Terminal input is busy or disconnected.".into())
    }
    pub fn try_read(&self) -> Result<Event, mpsc::TryRecvError> {
        self.output.try_recv()
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.lifecycle.stopped.store(true, Ordering::Release);
        self.lifecycle.ready.store(false, Ordering::Release);
    }
}

fn dimensions(columns: u16, rows: u16) -> Result<(), String> {
    if (2..=500).contains(&columns) && (1..=300).contains(&rows) {
        Ok(())
    } else {
        Err("Invalid terminal dimensions.".into())
    }
}

fn emit(mut event: Event, events: &SyncSender<Event>, state: &Lifecycle) -> bool {
    loop {
        if state.stopped.load(Ordering::Acquire) {
            return false;
        }
        match events.try_send(event) {
            Ok(()) => return true,
            Err(TrySendError::Disconnected(_)) => return false,
            Err(TrySendError::Full(returned)) => {
                event = returned;
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
}
fn read(rpc: Arc<dyn Rpc>, id: uuid::Uuid, state: Arc<Lifecycle>, events: SyncSender<Event>) {
    let mut retry = Retry::default();
    while !state.stopped.load(Ordering::Acquire) {
        let event = match rpc.call("terminal_read", json!({"session":id})) {
            Err(error) if refused(&error) && retry.wait(&state) => continue,
            Ok(value) if value.is_null() => {
                retry.succeeded();
                continue;
            }
            Ok(value) => {
                retry.succeeded();
                serde_json::from_value(value).unwrap_or_else(|error| {
                    Event::Error(format!("Invalid terminal event: {error}"))
                })
            }
            Err(error) => Event::Error(error.message),
        };
        match &event {
            Event::Ready => {
                state.ready.store(true, Ordering::Release);
            }
            Event::Reconnecting { .. } | Event::Exit(_) | Event::Error(_) => {
                state.ready.store(false, Ordering::Release);
                state.epoch.fetch_add(1, Ordering::AcqRel);
            }
            Event::Data(_) => {}
        }
        let terminal = matches!(event, Event::Exit(_) | Event::Error(_));
        if !emit(event, &events, &state) || terminal {
            break;
        }
    }
    state.stopped.store(true, Ordering::Release);
    state.ready.store(false, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, atomic::AtomicUsize};
    struct Fake {
        opens: Mutex<Vec<Value>>,
        calls: Mutex<Vec<String>>,
        writes: Mutex<Vec<Value>>,
        events: Mutex<Receiver<Value>>,
        closed: SyncSender<()>,
        fail_writes: bool,
        read_count: AtomicUsize,
        full: Option<SyncSender<()>>,
        /// Errors returned, in order, by calls to the named method.
        failures: Mutex<Vec<(&'static str, RequestError)>>,
    }
    impl Rpc for Fake {
        fn open(&self, args: Value) -> Result<Box<dyn Send>, String> {
            self.opens.lock().unwrap().push(args.clone());
            self.call(newport_platform::ipc::protocol::TERMINAL_OPEN_LEASE, args)
                .map_err(|error| error.message)?;
            struct Lease(SyncSender<()>);
            impl Drop for Lease {
                fn drop(&mut self) {
                    let _ = self.0.try_send(());
                }
            }
            Ok(Box::new(Lease(self.closed.clone())))
        }
        fn call(&self, method: &str, args: Value) -> Result<Value, RequestError> {
            self.calls.lock().unwrap().push(method.into());
            if method == "terminal_write" {
                self.writes.lock().unwrap().push(args["data"].clone());
            }
            {
                let mut failures = self.failures.lock().unwrap();
                if let Some(index) = failures.iter().position(|(name, _)| *name == method) {
                    return Err(failures.remove(index).1);
                }
            }
            if method == "terminal_write" && self.fail_writes {
                return Err("Fixture write failed".into());
            }
            match method {
                "terminal_read" => {
                    // A new read starts only after the preceding event has
                    // reached the session's bounded output queue.
                    if self.read_count.load(Ordering::Acquire) == 64
                        && let Some(full) = &self.full
                    {
                        let _ = full.try_send(());
                    }
                    let value = self
                        .events
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_millis(20))
                        .unwrap_or(Value::Null);
                    if !value.is_null() {
                        self.read_count.fetch_add(1, Ordering::AcqRel);
                    }
                    Ok(value)
                }
                "terminal_close" => {
                    let _ = self.closed.try_send(());
                    Ok(Value::Null)
                }
                _ => Ok(Value::Null),
            }
        }
    }
    fn fake(events: Receiver<Value>, closed: SyncSender<()>) -> Fake {
        Fake {
            opens: Mutex::default(),
            calls: Mutex::default(),
            writes: Mutex::default(),
            events: Mutex::new(events),
            closed,
            fail_writes: false,
            read_count: AtomicUsize::new(0),
            full: None,
            failures: Mutex::default(),
        }
    }
    fn coded(code: &str) -> RequestError {
        RequestError {
            message: format!("fixture {code}"),
            code: Some(code.into()),
        }
    }
    fn wait_until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !condition() {
            assert!(Instant::now() < deadline, "condition not reached");
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn refused_reads_are_retried_without_ending_the_session() {
        let (tx, rx) = mpsc::sync_channel(8);
        let (closed, _closed_rx) = mpsc::sync_channel(2);
        let rpc = Arc::new(fake(rx, closed));
        *rpc.failures.lock().unwrap() = vec![
            ("terminal_read", coded(crate::backend::BUSY)),
            ("terminal_read", coded(crate::backend::BUSY)),
        ];
        let session =
            Session::start(rpc.clone(), &uuid::Uuid::new_v4().to_string(), 80, 24).unwrap();
        tx.send(json!({"type":"ready"})).unwrap();
        tx.send(json!({"type":"data","data":b"still here".to_vec()}))
            .unwrap();
        let mut events = Vec::new();
        wait_until(|| {
            events.extend(session.try_read().ok());
            events.len() == 2
        });
        assert_eq!(events, [Event::Ready, Event::Data(b"still here".to_vec())]);
        assert!(!session.is_closed());
    }

    #[test]
    fn refused_input_is_delivered_once_and_interrupted_input_is_not_repeated() {
        let (tx, rx) = mpsc::sync_channel(8);
        let (closed, _closed_rx) = mpsc::sync_channel(2);
        let rpc = Arc::new(fake(rx, closed));
        let session =
            Session::start(rpc.clone(), &uuid::Uuid::new_v4().to_string(), 80, 24).unwrap();
        tx.send(json!({"type":"ready"})).unwrap();
        wait_until(|| session.ready());
        *rpc.failures.lock().unwrap() = vec![
            ("terminal_write", coded(crate::backend::BUSY)),
            ("terminal_write", coded(crate::backend::BUSY)),
        ];
        session.send(b"ls".to_vec()).unwrap();
        // Two refusals, then the one write that ran.
        wait_until(|| rpc.writes.lock().unwrap().len() == 3);
        assert!(session.ready());
        rpc.failures
            .lock()
            .unwrap()
            .push(("terminal_write", coded(crate::backend::INTERRUPTED)));
        session.send(b"rm".to_vec()).unwrap();
        let mut events = Vec::new();
        wait_until(|| {
            events.extend(session.try_read().ok());
            events.iter().any(|event| matches!(event, Event::Error(_)))
        });
        assert_eq!(rpc.writes.lock().unwrap().len(), 4);
    }

    #[test]
    fn an_interrupted_read_ends_the_session_instead_of_losing_output() {
        let (_tx, rx) = mpsc::sync_channel(8);
        let (closed, _closed_rx) = mpsc::sync_channel(2);
        let rpc = Arc::new(fake(rx, closed));
        *rpc.failures.lock().unwrap() = vec![("terminal_read", coded(crate::backend::INTERRUPTED))];
        let session =
            Session::start(rpc.clone(), &uuid::Uuid::new_v4().to_string(), 80, 24).unwrap();
        let mut events = Vec::new();
        wait_until(|| {
            events.extend(session.try_read().ok());
            !events.is_empty()
        });
        assert_eq!(
            events,
            [Event::Error("fixture CONNECTION_INTERRUPTED".into())]
        );
        let reads = |rpc: &Fake| {
            rpc.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|call| call.as_str() == "terminal_read")
                .count()
        };
        assert_eq!(reads(&rpc), 1);
    }

    #[test]
    fn reads_that_keep_failing_end_the_session_after_the_retry_window() {
        let (_tx, rx) = mpsc::sync_channel(8);
        let (closed, _closed_rx) = mpsc::sync_channel(2);
        let rpc = Arc::new(fake(rx, closed));
        *rpc.failures.lock().unwrap() = (0..200)
            .map(|_| ("terminal_read", coded(crate::backend::BUSY)))
            .collect();
        let started = Instant::now();
        let session =
            Session::start(rpc.clone(), &uuid::Uuid::new_v4().to_string(), 80, 24).unwrap();
        let mut events = Vec::new();
        wait_until(|| {
            events.extend(session.try_read().ok());
            !events.is_empty()
        });
        assert_eq!(events, [Event::Error("fixture BUSY".into())]);
        assert!(started.elapsed() >= RETRY_FOR);
        wait_until(|| session.is_closed());
    }

    #[test]
    fn a_terminal_opens_with_one_request_that_names_no_display_mode() {
        let (tx, rx) = mpsc::sync_channel(8);
        let (closed, closed_rx) = mpsc::sync_channel(2);
        let rpc = Arc::new(Fake {
            opens: Mutex::default(),
            calls: Mutex::default(),
            writes: Mutex::default(),
            events: Mutex::new(rx),
            closed,
            fail_writes: false,
            read_count: AtomicUsize::new(0),
            full: None,
            failures: Mutex::default(),
        });
        let server = uuid::Uuid::new_v4().to_string();
        let session = Session::start(rpc.clone(), &server, 80, 24).unwrap();
        tx.send(json!({"type":"ready"})).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !session.ready() {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        let requests = rpc.opens.lock().unwrap().clone();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0],
            json!({"id":server,"session":session.id(),"cols":80,"rows":24})
        );
        drop(session);
        closed_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    }

    #[test]
    fn protocol_preserves_binary_output_and_reconnect_events() {
        assert_eq!(
            serde_json::from_value::<Event>(json!({"type":"data","data":[0,255,13,10]})).unwrap(),
            Event::Data(vec![0, 255, 13, 10])
        );
        assert!(dimensions(1, 24).is_err());
        assert!(dimensions(80, 301).is_err());
    }
    #[test]
    fn dropping_session_closes_its_pty_and_input_waits_for_ready() {
        let (tx, rx) = mpsc::sync_channel(8);
        let (closed, closed_rx) = mpsc::sync_channel(1);
        let rpc = Arc::new(Fake {
            opens: Mutex::default(),
            calls: Mutex::new(vec![]),
            writes: Mutex::new(vec![]),
            events: Mutex::new(rx),
            closed,
            fail_writes: false,
            read_count: AtomicUsize::new(0),
            full: None,
            failures: Mutex::default(),
        });
        let session =
            Session::start(rpc.clone(), &uuid::Uuid::new_v4().to_string(), 80, 24).unwrap();
        assert!(session.send(vec![b'x']).is_err());
        tx.send(json!({"type":"ready"})).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !session.ready() {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        session.send(b"first".to_vec()).unwrap();
        session.send(b"second".to_vec()).unwrap();
        while rpc.writes.lock().unwrap().len() < 2 {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            *rpc.writes.lock().unwrap(),
            vec![json!(b"first".to_vec()), json!(b"second".to_vec())]
        );
        let old_epoch = session.epoch();
        tx.send(json!({"type":"reconnecting","data":{"attempt":1,"message":"retry"}}))
            .unwrap();
        tx.send(json!({"type":"ready"})).unwrap();
        while session.epoch() == old_epoch || !session.ready() {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        assert!(
            session
                .send_for_epoch(b"stale".to_vec(), old_epoch)
                .is_err()
        );
        assert_eq!(rpc.writes.lock().unwrap().len(), 2);
        drop(session);
        closed_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            rpc.calls.lock().unwrap().first().unwrap(),
            newport_platform::ipc::protocol::TERMINAL_OPEN_LEASE
        );
    }

    struct FailedSession {
        session: Session,
        rpc: Arc<Fake>,
        closed: Receiver<()>,
        _events: SyncSender<Value>,
    }

    fn fail_with_full_output_queue() -> FailedSession {
        let (tx, rx) = mpsc::sync_channel(64);
        let (closed, closed_rx) = mpsc::sync_channel(1);
        let (full, full_rx) = mpsc::sync_channel(1);
        let rpc = Arc::new(Fake {
            opens: Mutex::default(),
            calls: Mutex::new(vec![]),
            writes: Mutex::new(vec![]),
            events: Mutex::new(rx),
            closed,
            fail_writes: true,
            read_count: AtomicUsize::new(0),
            full: Some(full),
            failures: Mutex::default(),
        });
        let session =
            Session::start(rpc.clone(), &uuid::Uuid::new_v4().to_string(), 80, 24).unwrap();
        tx.send(json!({"type":"ready"})).unwrap();
        for byte in 0..63u8 {
            tx.send(json!({"type":"data","data":[byte]})).unwrap();
        }
        full_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        session.send(b"x".to_vec()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while session.ready() {
            assert!(
                std::time::Instant::now() < deadline,
                "failed write remained ready"
            );
            thread::sleep(Duration::from_millis(1));
        }
        FailedSession {
            session,
            rpc,
            closed: closed_rx,
            _events: tx,
        }
    }

    #[test]
    fn write_failure_survives_a_full_output_queue() {
        let FailedSession {
            session,
            rpc,
            closed,
            _events,
        } = fail_with_full_output_queue();
        // A late reader Ready event must not revive a failed input stream.
        session.lifecycle.ready.store(true, Ordering::Release);
        assert!(!session.ready());
        let mut received = vec![];
        loop {
            match session.output.recv_timeout(Duration::from_secs(2)) {
                Ok(event @ Event::Error(_)) => {
                    received.push(event);
                    break;
                }
                Ok(event) => received.push(event),
                Err(error) => panic!(
                    "write failure was lost after {} events: {error}",
                    received.len()
                ),
            }
        }
        assert_eq!(received.first(), Some(&Event::Ready));
        assert_eq!(
            received.last(),
            Some(&Event::Error("Fixture write failed".into()))
        );
        assert_eq!(received.len(), 65);
        assert!(session.send(b"retry".to_vec()).is_err());
        closed.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(rpc.writes.lock().unwrap().len(), 1);
    }

    #[test]
    fn dropping_a_session_cancels_failure_delivery_to_a_full_queue() {
        let FailedSession {
            session,
            rpc,
            closed,
            _events,
        } = fail_with_full_output_queue();
        drop(session);
        closed.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(rpc.writes.lock().unwrap().len(), 1);
    }
}
