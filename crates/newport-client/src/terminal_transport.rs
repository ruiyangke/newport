//! A terminal tab's I/O: one duplex stream on the socket that holds its
//! remote shell. Terminal I/O has independent workers and no idle timeout.
use crate::backend::{Backend, exchange::Opened};
use newport_platform::ipc::{
    Stream,
    framed::{Reader, Writer},
};
use newport_protocol::{
    common::Present,
    desktop::{
        self,
        stream::{self, TerminalClient, TerminalEvent, TerminalServer},
        wire,
    },
};
use std::{
    collections::VecDeque,
    net::Shutdown,
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

/// How long ending a held shell waits to hear that it ended, before the
/// session closes anyway.
const END_WAIT: Duration = Duration::from_secs(5);

/// Whether the session's shell is held on the server apart from it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hold {
    /// The session's own shell, which ends with it.
    No,
    /// Held when the server can hold it.
    WhenAvailable(Colors),
    /// The held shell of this terminal, never a new one.
    Attach(uuid::Uuid, Colors),
}

/// A tab's foreground, background and cursor colours, which the server
/// answers a held shell's colour queries with while this tab is its latest.
pub type Colors = Option<[[u8; 3]; 3]>;

#[derive(Debug, PartialEq)]
pub enum Event {
    Ready,
    Data(Vec<u8>),
    Reconnecting {
        attempt: u32,
        message: String,
    },
    Exit(Option<u32>),
    Error(String),
    /// This session's stream to Newport's backend broke before the shell's
    /// outcome. A held shell carries on on the server, and
    /// attaching again finds it.
    Lost(String),
    /// The terminal's whole state: reset the view and replay it. `prompted`
    /// says whether the shell has prompted, which the replay can't show.
    Restore {
        state: Vec<u8>,
        prompted: bool,
    },
    /// The shell runs on the server apart from this session: dropping the
    /// session detaches from it, and [`Session::attach`] finds it again.
    Persistent,
    /// The shell to attach to is gone, as after a server restart.
    Ended,
    /// Whether this tab is its held shell's active one: only that tab acts on
    /// what goes to one Mac, such as a program's clipboard copy.
    Active(bool),
    /// The account's display stopped showing on this Mac while the tab is
    /// its held shell's active one: the tab's next use shows it again.
    DisplayLost,
    /// Needle couldn't hold this new tab's shell, for this reason: an
    /// ordinary shell follows, and ends when the connection does.
    NotHeld(String),
    /// The held shell's size: the tab draws the shell at it, whatever its own
    /// size, since its output is drawn for it. The computer last used sets it.
    Size {
        columns: u16,
        rows: u16,
    },
    /// Nothing was opened: this computer already has the tab open, in
    /// another tab, or in this one's earlier attachment still letting go.
    AlreadyOpen,
}
impl From<TerminalEvent> for Event {
    fn from(event: TerminalEvent) -> Self {
        match event {
            TerminalEvent::Ready(_) => Self::Ready,
            TerminalEvent::Persistent(_) => Self::Persistent,
            TerminalEvent::Reconnecting(value) => Self::Reconnecting {
                attempt: value.attempt,
                message: value.message,
            },
            TerminalEvent::Exit(value) => Self::Exit(value.code),
            TerminalEvent::Error(value) => Self::Error(value.error.message),
            TerminalEvent::Ended(_) => Self::Ended,
            TerminalEvent::Active(value) => Self::Active(value.active),
            TerminalEvent::DisplayLost(_) => Self::DisplayLost,
            TerminalEvent::NotHeld(value) => Self::NotHeld(value.reason),
            TerminalEvent::Size(value) => Self::Size {
                columns: value.columns,
                rows: value.rows,
            },
        }
    }
}

/// Why an open opened nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Refusal {
    /// Another tab has the terminal open.
    AlreadyOpen,
    /// A passing failure: the server may still hold the shell.
    Passing(String),
    Failed(String),
}
impl Refusal {
    fn of(code: &str, retry: newport_protocol::common::Retry, message: String) -> Self {
        if code == newport_protocol::terminal::ALREADY_OPEN {
            Self::AlreadyOpen
        } else if retry == newport_protocol::common::Retry::Yes {
            Self::Passing(message)
        } else {
            Self::Failed(message)
        }
    }
    /// What the tab hears. An attach that failed in passing tries again, as
    /// after a lost stream; a new shell that failed is gone.
    fn event(self, attach: bool) -> Event {
        match self {
            Self::AlreadyOpen => Event::AlreadyOpen,
            Self::Passing(error) if attach => Event::Lost(error),
            Self::Passing(error) | Self::Failed(error) => Event::Error(error),
        }
    }
}
impl From<String> for Refusal {
    fn from(error: String) -> Self {
        Self::Failed(error)
    }
}
impl From<&str> for Refusal {
    fn from(error: &str) -> Self {
        Self::Failed(error.into())
    }
}

trait Lease: Send + Sync + 'static {
    fn open(&self, args: stream::TerminalOpenArgs) -> Result<Opened, Refusal>;
}
impl Lease for Backend {
    fn open(&self, args: stream::TerminalOpenArgs) -> Result<Opened, Refusal> {
        self.open(
            stream::Operation::TerminalOpen(Box::new(args)),
            Duration::from_secs(10),
        )
        .map_err(|error| Refusal::of(error.code(), error.retry(), error.to_string()))
    }
}

enum Outgoing {
    Input { epoch: u64, bytes: Vec<u8> },
    Resize(u16, u16),
    Wake,
    End,
    Activate,
    Close,
}

#[derive(Default)]
struct Lifecycle {
    stopped: AtomicBool,
    flow: Mutex<desktop::TerminalFlow>,
    cancellation: Mutex<Option<Stream>>,
    ready: AtomicBool,
    epoch: AtomicU64,
    /// Input bytes accepted from the caller that the backend hasn't taken.
    unapplied: AtomicU64,
    pending_acks: AtomicU64,
}
impl Lifecycle {
    fn current(&self, epoch: u64) -> bool {
        self.ready.load(Ordering::Acquire)
            && !self.stopped.load(Ordering::Acquire)
            && self.epoch.load(Ordering::Acquire) == epoch
    }
    fn end_shell(&self) -> Result<(), desktop::Invalid> {
        self.ready.store(false, Ordering::Release);
        let epoch = self
            .flow
            .lock()
            .map_err(|_| desktop::Invalid("terminal flow state"))?
            .depart()?;
        self.epoch.store(epoch, Ordering::Release);
        Ok(())
    }
    fn cancel(&self) {
        if let Ok(stream) = self.cancellation.lock()
            && let Some(stream) = stream.as_ref()
        {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
    fn applied(&self, bytes: u64) {
        let _ = self
            .unapplied
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |unapplied| {
                Some(unapplied.saturating_sub(bytes))
            });
    }
}

/// The most events waiting for the tab before the reader waits for it to read.
const INBOX_EVENTS: usize = 256;
/// The largest block consecutive output merges into.
const INBOX_BLOCK: usize = 64 * 1024;

/// Events waiting for the tab. Consecutive output merges into one block, so a
/// program's many small writes cost the tab one read, and the reader never
/// waits on the tab for each event. Output stays bounded by the backend's
/// output credit, which only reading returns; other events by count.
#[derive(Default)]
struct Inbox {
    state: Mutex<Waiting>,
    changed: Condvar,
    wake: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}
#[derive(Default)]
struct Waiting {
    events: VecDeque<Event>,
    /// The reader has nothing more to deliver.
    finished: bool,
    /// The session is gone: nothing more is read.
    closed: bool,
}
impl Inbox {
    fn waiting(&self) -> MutexGuard<'_, Waiting> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
    /// Queues an event for the tab, waiting only while the inbox is full.
    /// False once the session is gone.
    fn push(&self, event: Event) -> bool {
        let mut waiting = self.waiting();
        while waiting.events.len() >= INBOX_EVENTS && !waiting.closed {
            waiting = self
                .changed
                .wait(waiting)
                .unwrap_or_else(PoisonError::into_inner);
        }
        if waiting.closed {
            return false;
        }
        match (waiting.events.back_mut(), event) {
            (Some(Event::Data(queued)), Event::Data(more))
                if queued.len() + more.len() <= INBOX_BLOCK =>
            {
                queued.extend_from_slice(&more);
            }
            (_, event) => waiting.events.push_back(event),
        }
        drop(waiting);
        self.changed.notify_all();
        self.wake();
        true
    }
    fn wake(&self) {
        if let Some(wake) = self
            .wake
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            wake();
        }
    }
    fn finish(&self) {
        self.waiting().finished = true;
        self.changed.notify_all();
        self.wake();
    }
    fn close(&self) {
        self.waiting().closed = true;
        self.changed.notify_all();
    }
    fn try_pop(&self) -> Result<Event, mpsc::TryRecvError> {
        let mut waiting = self.waiting();
        let event = waiting.events.pop_front();
        let finished = waiting.finished;
        drop(waiting);
        match event {
            Some(event) => {
                self.changed.notify_all();
                Ok(event)
            }
            None if finished => Err(mpsc::TryRecvError::Disconnected),
            None => Err(mpsc::TryRecvError::Empty),
        }
    }
    fn pop_timeout(&self, timeout: Duration) -> Option<Event> {
        let deadline = Instant::now() + timeout;
        let mut waiting = self.waiting();
        loop {
            if let Some(event) = waiting.events.pop_front() {
                drop(waiting);
                self.changed.notify_all();
                return Some(event);
            }
            if waiting.finished {
                return None;
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            waiting = self
                .changed
                .wait_timeout(waiting, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// Dropping a session closes its socket, which ends its shell, or detaches
/// from a persistent one; so does the end of this process.
pub struct Session {
    id: uuid::Uuid,
    outgoing: SyncSender<Outgoing>,
    inbox: Arc<Inbox>,
    lifecycle: Arc<Lifecycle>,
}
impl Session {
    /// A shell that starts in `directory`, or where the server's shells
    /// start without one.
    /// `attachment` names the tab asking: its next open replaces this one.
    pub fn open(
        server: &str,
        attachment: uuid::Uuid,
        columns: u16,
        rows: u16,
        directory: Option<String>,
    ) -> Result<Self, String> {
        let backend = Arc::new(Backend::discover());
        let size = (columns, rows);
        Self::start(backend, server, attachment, Hold::No, size, directory)
    }

    /// A shell the server holds apart from this session where it can, so
    /// dropping the session detaches from it; [`Event::Persistent`] says so.
    pub fn open_held(
        server: &str,
        attachment: uuid::Uuid,
        columns: u16,
        rows: u16,
        colors: Colors,
        directory: Option<String>,
    ) -> Result<Self, String> {
        let backend = Arc::new(Backend::discover());
        let hold = Hold::WhenAvailable(colors);
        Self::start(
            backend,
            server,
            attachment,
            hold,
            (columns, rows),
            directory,
        )
    }

    /// Attaches to the held shell of terminal `id`, never starting a new one;
    /// [`Event::Ended`] says it is gone.
    pub fn attach(
        server: &str,
        attachment: uuid::Uuid,
        id: uuid::Uuid,
        columns: u16,
        rows: u16,
        colors: Colors,
    ) -> Result<Self, String> {
        let backend = Arc::new(Backend::discover());
        let hold = Hold::Attach(id, colors);
        Self::start(backend, server, attachment, hold, (columns, rows), None)
    }

    /// Open against an explicitly owned backend without changing process-wide
    /// discovery.
    pub fn open_with_backend(
        backend: Backend,
        server: &str,
        columns: u16,
        rows: u16,
    ) -> Result<Self, String> {
        let attachment = uuid::Uuid::new_v4();
        let size = (columns, rows);
        Self::start(Arc::new(backend), server, attachment, Hold::No, size, None)
    }

    fn start(
        lease: Arc<dyn Lease>,
        server: &str,
        attachment: uuid::Uuid,
        hold: Hold,
        (columns, rows): (u16, u16),
        directory: Option<String>,
    ) -> Result<Self, String> {
        dimensions(columns, rows)?;
        let server = uuid::Uuid::parse_str(server).map_err(|_| "Invalid terminal server ID")?;
        let (id, colors) = match hold {
            Hold::Attach(id, colors) => (id, colors),
            Hold::WhenAvailable(colors) => (uuid::Uuid::new_v4(), colors),
            Hold::No => (uuid::Uuid::new_v4(), None),
        };
        let (outgoing, commands) = mpsc::sync_channel(256);
        let inbox = Arc::new(Inbox::default());
        let events = inbox.clone();
        let lifecycle = Arc::new(Lifecycle::default());
        let state = lifecycle.clone();
        let closer = outgoing.clone();
        let attach = matches!(hold, Hold::Attach(..));
        thread::spawn(move || {
            // The backend joins the server's shared display when it can; the
            // shell never waits for it and never fails because of it.
            let request = stream::TerminalOpenArgs {
                server,
                session: id,
                columns,
                rows,
                persist: matches!(hold, Hold::WhenAvailable(_)),
                attach,
                colors,
                directory,
                attachment,
            };
            let opened = lease.open(request).and_then(|opened| {
                let cancellation = opened
                    .reader
                    .try_clone_stream()
                    .map_err(|error| error.to_string())?;
                *state
                    .cancellation
                    .lock()
                    .map_err(|_| "Terminal cancellation state unavailable")? = Some(cancellation);
                if state.stopped.load(Ordering::Acquire) {
                    state.cancel();
                    return Err("Terminal is closed.".into());
                }
                Ok(opened)
            });
            let Opened { reader, writer, .. } = match opened {
                Ok(opened) => opened,
                Err(refusal) => {
                    events.push(refusal.event(attach));
                    state.stopped.store(true, Ordering::Release);
                    events.finish();
                    return;
                }
            };
            let reader = {
                let state = state.clone();
                thread::spawn(move || {
                    read(reader, &state, &events, &closer);
                    events.finish();
                })
            };
            write(writer, &commands, &state);
            let _ = reader.join();
        });
        Ok(Self {
            id,
            outgoing,
            inbox,
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
        if bytes.len() > desktop::MAX_DATA {
            return Err("Terminal input is too large.".into());
        }
        if !self.lifecycle.current(epoch) {
            return Err("Terminal is not ready.".into());
        }
        if bytes.is_empty() {
            return Ok(());
        }
        let length = u64::try_from(bytes.len()).map_err(|_| "Terminal input is too large.")?;
        self.lifecycle
            .unapplied
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |unapplied| {
                unapplied
                    .checked_add(length)
                    .filter(|next| *next <= desktop::INPUT_WINDOW)
            })
            .map_err(|_| "Terminal input is busy.")?;
        self.outgoing
            .try_send(Outgoing::Input { epoch, bytes })
            .map_err(|_| {
                self.lifecycle.applied(length);
                "Terminal input is busy or closed.".into()
            })
    }
    /// Ends the shell. Dropping the session instead leaves a held shell
    /// running. The session stays open in the background until the shell has
    /// ended: closing at once could detach before the end reaches it.
    pub fn end(self) {
        thread::spawn(move || {
            let deadline = Instant::now() + END_WAIT;
            loop {
                match self.outgoing.try_send(Outgoing::End) {
                    Ok(()) => break,
                    Err(mpsc::TrySendError::Disconnected(_)) => return,
                    Err(mpsc::TrySendError::Full(_)) => {
                        if Instant::now() >= deadline {
                            return;
                        }
                        if let Ok(Event::Data(bytes)) = self.inbox.try_pop() {
                            self.acknowledge(bytes.len() as u64);
                        }
                        thread::sleep(Duration::from_millis(2));
                    }
                }
            }
            while let Some(left) = deadline.checked_duration_since(Instant::now()) {
                match self.inbox.pop_timeout(left) {
                    Some(Event::Data(bytes)) => {
                        self.acknowledge(bytes.len() as u64);
                    }
                    Some(
                        Event::Exit(_)
                        | Event::Error(_)
                        | Event::Lost(_)
                        | Event::Ended
                        | Event::AlreadyOpen,
                    )
                    | None => break,
                    Some(_) => {}
                }
            }
            drop(self);
        });
    }
    pub fn resize(&self, columns: u16, rows: u16) -> Result<(), String> {
        dimensions(columns, rows)?;
        if self.is_closed() {
            return Err("Terminal is closed.".into());
        }
        self.outgoing
            .try_send(Outgoing::Resize(columns, rows))
            .map_err(|_| "Terminal is closed.".into())
    }
    /// Someone used the terminal here: this Mac becomes its held shell's
    /// active one, which sets the shell's size and takes its programs'
    /// requests.
    pub fn activate(&self) -> Result<(), String> {
        if self.is_closed() {
            return Err("Terminal is closed.".into());
        }
        self.outgoing
            .try_send(Outgoing::Activate)
            .map_err(|_| "Terminal is closed.".into())
    }
    fn acknowledge(&self, bytes: u64) {
        let Ok(before) = self.lifecycle.pending_acks.fetch_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |pending| pending.checked_add(bytes),
        ) else {
            self.lifecycle.cancel();
            return;
        };
        // Only the first acknowledgment since the writer last took them wakes
        // it; later ones go out with that one. A full queue already wakes the
        // writer, which drains this counter first.
        if before == 0 {
            let _ = self.outgoing.try_send(Outgoing::Wake);
        }
    }
    /// Calls `wake` whenever events arrive for [`Session::try_read`], from the
    /// session's own thread, and once now if some already wait. It replaces
    /// any earlier one and must return quickly.
    pub fn on_output(&self, wake: impl Fn() + Send + Sync + 'static) {
        *self
            .inbox
            .wake
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Box::new(wake));
        let waiting = self.inbox.waiting();
        let ready = !waiting.events.is_empty() || waiting.finished;
        drop(waiting);
        if ready {
            self.inbox.wake();
        }
    }
    /// The next event. Output read here is acknowledged: the caller draws
    /// what it reads, and the backend sends more as it does.
    pub fn try_read(&self) -> Result<Event, mpsc::TryRecvError> {
        let event = self.inbox.try_pop()?;
        if let Event::Data(bytes) = &event {
            self.acknowledge(bytes.len() as u64);
        }
        Ok(event)
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.inbox.close();
        self.lifecycle.stopped.store(true, Ordering::Release);
        self.lifecycle.ready.store(false, Ordering::Release);
        self.lifecycle.cancel();
        let _ = self.outgoing.try_send(Outgoing::Close);
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
fn write(mut writer: Writer, commands: &Receiver<Outgoing>, state: &Lifecycle) {
    let result = (|| -> Result<(), std::io::Error> {
        while let Ok(command) = commands.recv() {
            let acked = state.pending_acks.swap(0, Ordering::AcqRel);
            if acked != 0 {
                state
                    .flow
                    .lock()
                    .map_err(|_| std::io::Error::other("Terminal flow state unavailable"))?
                    .acknowledge(acked)
                    .map_err(std::io::Error::other)?;
                let frame =
                    wire::encode_terminal_client(&TerminalClient::Ack(Box::new(stream::Ack {
                        bytes: acked,
                    })))
                    .map_err(std::io::Error::from)?;
                writer.send_until(&frame, Instant::now() + Duration::from_secs(30))?;
            }
            let frame = match command {
                Outgoing::Input { epoch, bytes } => {
                    if !state.current(epoch) {
                        state.applied(bytes.len() as u64);
                        continue;
                    }
                    state
                        .flow
                        .lock()
                        .map_err(|_| std::io::Error::other("Terminal flow state unavailable"))?
                        .input(epoch, bytes.len())
                        .map_err(std::io::Error::other)?;
                    TerminalClient::Input(Box::new(stream::Input {
                        epoch,
                        data: bytes.into(),
                    }))
                }
                Outgoing::Resize(columns, rows) => {
                    TerminalClient::Resize(Box::new(stream::Resize { columns, rows }))
                }
                Outgoing::Wake => continue,
                Outgoing::End => TerminalClient::End(Box::new(Present {})),
                Outgoing::Activate => TerminalClient::Activate(Box::new(Present {})),
                Outgoing::Close => break,
            };
            let frame = wire::encode_terminal_client(&frame).map_err(std::io::Error::from)?;
            writer.send_until(&frame, Instant::now() + Duration::from_secs(30))?;
        }
        Ok(())
    })();
    let _ = result;
    state.cancel();
}

/// Deliver a final terminal outcome only after its stream has closed cleanly.
fn read(mut reader: Reader, state: &Lifecycle, events: &Inbox, writer: &SyncSender<Outgoing>) {
    let result = (|| -> Result<(), std::io::Error> {
        loop {
            let frame = reader
                .next_idle(
                    desktop::TERMINAL_SERVER_LIMITS.message,
                    Duration::from_secs(30),
                )?
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "Newport’s backend closed the terminal before its final outcome",
                    )
                })?;
            let frame = wire::decode_terminal_server(frame.body()).map_err(std::io::Error::from)?;
            let event = match frame {
                TerminalServer::Output(value) => {
                    state
                        .flow
                        .lock()
                        .map_err(|_| std::io::Error::other("Terminal flow state unavailable"))?
                        .output(value.data.len())
                        .map_err(std::io::Error::other)?;
                    Event::Data(value.data.to_vec())
                }
                TerminalServer::Taken(value) => {
                    state
                        .flow
                        .lock()
                        .map_err(|_| std::io::Error::other("Terminal flow state unavailable"))?
                        .taken(value.bytes)
                        .map_err(std::io::Error::other)?;
                    state.applied(value.bytes);
                    continue;
                }
                TerminalServer::Restore(value) => Event::Restore {
                    state: value.state.to_vec(),
                    prompted: value.prompted,
                },
                TerminalServer::Event(value) => {
                    let final_event = matches!(
                        value.as_ref(),
                        TerminalEvent::Exit(_) | TerminalEvent::Error(_) | TerminalEvent::Ended(_)
                    );
                    if matches!(value.as_ref(), TerminalEvent::Ready(_)) {
                        state.ready.store(true, Ordering::Release);
                    } else if matches!(
                        value.as_ref(),
                        TerminalEvent::Reconnecting(_)
                            | TerminalEvent::Exit(_)
                            | TerminalEvent::Error(_)
                            | TerminalEvent::Ended(_)
                    ) {
                        state.end_shell().map_err(std::io::Error::other)?;
                    }
                    if final_event {
                        if reader
                            .next_until(
                                desktop::TERMINAL_SERVER_LIMITS.message,
                                Instant::now() + desktop::CLOSURE_TIMEOUT,
                            )?
                            .is_some()
                        {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                "Terminal data followed its final outcome",
                            ));
                        }
                        events.push((*value).into());
                        return Ok(());
                    }
                    (*value).into()
                }
            };
            if !events.push(event) {
                return Ok(());
            }
            // Waiting for the tab to take that event is this side's pause, not
            // the backend's: it never ends a frame the backend already sent.
            reader.resume();
        }
    })();
    if let Err(error) = result
        && !state.stopped.load(Ordering::Acquire)
    {
        let _ = state.end_shell();
        events.push(Event::Lost(format!(
            "Lost the terminal’s connection to Newport’s backend: {error}"
        )));
    }
    state.stopped.store(true, Ordering::Release);
    state.ready.store(false, Ordering::Release);
    state.cancel();
    let _ = writer.try_send(Outgoing::Close);
}

/// How long a list, rename or end of held shells may take.
const HELD_TIMEOUT: Duration = Duration::from_secs(30);

/// A server's held shells, which every Mac connected to it shows as tabs.
pub fn held_shells(server: uuid::Uuid) -> Result<newport_domain::terminal::HeldShells, String> {
    use newport_domain::terminal as domain;
    let reply = Backend::discover()
        .execute_with_timeout(
            desktop::Operation::Terminal(Box::new(desktop::terminal::Operation::List(Box::new(
                desktop::terminal::ListArgs { server },
            )))),
            HELD_TIMEOUT,
        )
        .map_err(|error| error.to_string())?;
    let desktop::Reply::Terminal(reply) = reply else {
        return Err("Invalid terminal listing response".into());
    };
    let desktop::terminal::Reply::List(value) = *reply else {
        return Err("Invalid terminal listing response".into());
    };
    Ok(domain::HeldShells {
        desktop: value.desktop,
        holds: value.holds,
        shells: value.shells.map(|value| {
            value
                .shells
                .into_iter()
                .map(|value| domain::HeldShell {
                    terminal: value.terminal,
                    name: value.name,
                    title: value.title,
                    agent: value.agent,
                    creator: domain::Mac {
                        id: value.creator.id,
                        name: value.creator.name,
                    },
                    attached: value
                        .attached
                        .into_iter()
                        .map(|value| domain::AttachedMac {
                            id: value.id,
                            name: value.name,
                            active: value.active,
                        })
                        .collect(),
                    started: value.started,
                    exited: value.exited,
                })
                .collect()
        }),
    })
}

/// Renames a held shell for every Mac's tab; an empty name clears it.
pub fn rename_held(
    server: uuid::Uuid,
    terminal: uuid::Uuid,
    name: &str,
) -> Result<(), crate::request_error::RequestError> {
    Backend::discover()
        .execute_with_timeout(
            desktop::Operation::Terminal(Box::new(desktop::terminal::Operation::Rename(Box::new(
                desktop::terminal::RenameArgs {
                    server,
                    terminal,
                    name: name.into(),
                },
            )))),
            HELD_TIMEOUT,
        )
        .map(drop)
        .map_err(Into::into)
}

/// Ends a held shell through its worker and preserves uncertain-outcome errors.
pub fn end_held(
    server: uuid::Uuid,
    terminal: uuid::Uuid,
) -> Result<(), crate::request_error::RequestError> {
    Backend::discover()
        .execute_with_timeout(
            desktop::Operation::Terminal(Box::new(desktop::terminal::Operation::End(Box::new(
                desktop::terminal::EndArgs { server, terminal },
            )))),
            HELD_TIMEOUT,
        )
        .map(drop)
        .map_err(Into::into)
}

#[cfg(test)]
mod tests;
