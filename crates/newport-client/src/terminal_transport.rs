//! Dedicated PTY I/O; no terminal read occupies a general UI request worker.
use crate::backend::Backend;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread,
    time::Duration,
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
    fn call(&self, method: &str, args: Value) -> Result<Value, String>;
    fn open(&self, args: Value) -> Result<Box<dyn Send>, String>;
}
impl Rpc for Backend {
    fn open(&self, args: Value) -> Result<Box<dyn Send>, String> {
        self.open_terminal_lease(args)
            .map(|stream| Box::new(stream) as Box<dyn Send>)
    }
    fn call(&self, method: &str, args: Value) -> Result<Value, String> {
        self.call_with_timeout(method, args, Duration::from_secs(10))
    }
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
            let lease = match rpc.open(json!({"id":server,"session":id,"cols":columns,"rows":rows}))
            {
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
            while !state.stopped.load(Ordering::Acquire) {
                let command = match commands.recv_timeout(Duration::from_millis(50)) {
                    Ok(command) => command,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                let result = match command {
                    Input::Data { bytes, epoch } => {
                        // Never send queued keystrokes to a replacement shell.
                        if !state.ready.load(Ordering::Acquire)
                            || state.epoch.load(Ordering::Acquire) != epoch
                        {
                            continue;
                        }
                        rpc.call("terminal_write", json!({"session":id,"data":bytes}))
                    }
                    Input::Resize(columns, rows) => rpc.call(
                        "terminal_resize",
                        json!({"session":id,"cols":columns,"rows":rows}),
                    ),
                };
                if let Err(error) = result {
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
    while !state.stopped.load(Ordering::Acquire) {
        let event = match rpc.call("terminal_read", json!({"session":id})) {
            Ok(value) if value.is_null() => continue,
            Ok(value) => serde_json::from_value(value)
                .unwrap_or_else(|error| Event::Error(format!("Invalid terminal event: {error}"))),
            Err(error) => Event::Error(error),
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
        calls: Mutex<Vec<String>>,
        writes: Mutex<Vec<Value>>,
        events: Mutex<Receiver<Value>>,
        closed: SyncSender<()>,
        fail_writes: bool,
        read_count: AtomicUsize,
        full: Option<SyncSender<()>>,
    }
    impl Rpc for Fake {
        fn open(&self, args: Value) -> Result<Box<dyn Send>, String> {
            self.call("terminal_open", args)?;
            struct Lease(SyncSender<()>);
            impl Drop for Lease {
                fn drop(&mut self) {
                    let _ = self.0.try_send(());
                }
            }
            Ok(Box::new(Lease(self.closed.clone())))
        }
        fn call(&self, method: &str, args: Value) -> Result<Value, String> {
            self.calls.lock().unwrap().push(method.into());
            if method == "terminal_write" {
                self.writes.lock().unwrap().push(args["data"].clone());
                if self.fail_writes {
                    return Err("Fixture write failed".into());
                }
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
            calls: Mutex::new(vec![]),
            writes: Mutex::new(vec![]),
            events: Mutex::new(rx),
            closed,
            fail_writes: false,
            read_count: AtomicUsize::new(0),
            full: None,
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
        assert_eq!(rpc.calls.lock().unwrap().first().unwrap(), "terminal_open");
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
            calls: Mutex::new(vec![]),
            writes: Mutex::new(vec![]),
            events: Mutex::new(rx),
            closed,
            fail_writes: true,
            read_count: AtomicUsize::new(0),
            full: Some(full),
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
