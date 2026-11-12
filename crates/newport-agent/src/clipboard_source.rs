//! Clipboard readers use the live agent socket, or a standalone snapshot file.
use crate::{local_socket::Endpoint, snapshot};
use newport_protocol::clipboard::imports::Offer;
use newport_protocol::clipboard::{
    Chunk, imports,
    source::{self as protocol, Request, Response, wire},
};
use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
    os::fd::AsRawFd,
    os::unix::{fs::MetadataExt, net::UnixStream},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
/// Backends share this handle; enabling demand mode never requires reconnecting clients.
#[derive(Clone)]
pub(crate) struct Source {
    pub path: PathBuf,
    pub(crate) gui: Arc<crate::gui_notice::Notices>,
    store: Arc<std::sync::OnceLock<Data>>,
    route: Option<Route>,
    copy: Option<Copy>,
    copied: Arc<Copied>,
}
/// The text a program last copied, which reads return until the computer's
/// clipboard changes: the computer gets it only after the terminal has sent
/// it on, and a program that copies and pastes at once expects its own copy.
#[derive(Default)]
struct Copied(std::sync::Mutex<Option<LastCopy>>);
struct LastCopy {
    /// The computer's revision when it was copied, or when first read after
    /// if that wasn't known.
    over: Option<Revision>,
    /// Counts copies, so each is offered under a revision of its own.
    number: u64,
    text: Vec<u8>,
}
/// The first part of a copy's revision, which no computer's revision has.
const COPIED: u64 = u64::MAX;
/// Whether `revision` is a program's copy, not the computer's clipboard.
pub(crate) fn is_copy(revision: Revision) -> bool {
    revision.0 == COPIED
}
impl Copied {
    /// Records `text` as the last copy, made while the computer's clipboard
    /// is at `current`; false when it is that copy already.
    fn record(&self, text: &[u8], current: Option<Revision>) -> bool {
        let mut copied = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if copied
            .as_ref()
            .is_some_and(|last| last.text == text && (last.over.is_none() || last.over == current))
        {
            return false;
        }
        let number = copied
            .as_ref()
            .map_or(0, |last| last.number.wrapping_add(1));
        *copied = Some(LastCopy {
            over: current,
            number,
            text: text.to_vec(),
        });
        true
    }
    /// The copy, over the computer's clipboard at `revision`, while that is
    /// the one it was copied over.
    fn over(
        &self,
        (revision, formats): (Revision, BTreeMap<String, Vec<u8>>),
    ) -> (Revision, BTreeMap<String, Vec<u8>>) {
        let mut copied = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(copy) = copied.as_mut() {
            if *copy.over.get_or_insert(revision) == revision {
                return (
                    (COPIED, copy.number, 0),
                    BTreeMap::from([(TEXT.to_owned(), copy.text.clone())]),
                );
            }
            *copied = None;
        }
        (revision, formats)
    }
    /// The copy's `format`, for its `revision`; None for the computer's.
    fn get(&self, revision: Revision, format: &str) -> Option<io::Result<Vec<u8>>> {
        if revision.0 != COPIED {
            return None;
        }
        let copied = self.0.lock().unwrap_or_else(|p| p.into_inner());
        Some(match &*copied {
            Some(copy) if copy.number == revision.1 && format == TEXT => Ok(copy.text.clone()),
            Some(copy) if copy.number == revision.1 => {
                Err(io::Error::other("clipboard format unavailable"))
            }
            _ => Err(io::Error::other("clipboard changed")),
        })
    }
}
/// What became of a program's copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Taken {
    /// It went to the computer.
    Sent,
    /// It is the copy reads return already, so it didn't go again.
    Repeated,
    /// It wasn't text the terminal takes, or the display takes no copies.
    Refused,
}
/// The name a copy is offered and read by.
const TEXT: &str = "text/plain";
/// Finds the snapshot path to read at each request, as a held shell's
/// clipboard display does: the Mac someone last used the shell on.
pub(crate) type Route = Arc<dyn Fn() -> io::Result<PathBuf> + Send + Sync>;
/// Takes the text a program on the display copied to CLIPBOARD, for the
/// computer the display's terminal is shown on.
pub(crate) type Copy = Arc<dyn Fn(String) + Send + Sync>;
#[derive(Clone)]
pub(crate) enum Data {
    Local(Arc<crate::demand::Store>),
    Runtime(crate::clipboard_service::Reader),
}
impl Data {
    fn offer(&self) -> io::Result<Offer> {
        match self {
            Self::Local(store) => Ok(store.offer()),
            Self::Runtime(reader) => reader.offer().map(|offer| Offer {
                revision: offer.revision,
                formats: offer.formats,
            }),
        }
    }
    fn get(&self, revision: i64, format: &str) -> io::Result<Vec<u8>> {
        match self {
            Self::Local(store) => store.get(revision, format),
            Self::Runtime(reader) => reader.get(revision, format),
        }
    }
    fn close(&self) {
        if let Self::Local(store) = self {
            store.close();
        }
    }
}
impl Source {
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.into(),
            gui: Default::default(),
            store: Arc::new(std::sync::OnceLock::new()),
            route: None,
            copy: None,
            copied: Default::default(),
        }
    }
    /// The same source, sending programs' copies to `copy`. A display
    /// without one refuses them.
    pub(crate) fn with_copies(self, copy: Copy) -> Self {
        Self {
            copy: Some(copy),
            ..self
        }
    }
    pub(crate) fn takes_copies(&self) -> bool {
        self.copy.is_some()
    }
    /// Sends a program's copy on. Only text goes: it is what a terminal
    /// applies to its computer's clipboard, within its limit. The copy that
    /// reads return already is not sent again, so a program that takes the
    /// selection back each time it loses it, as clipboard managers do, can't
    /// send it in a loop.
    pub(crate) fn copy(&self, bytes: Vec<u8>) -> Taken {
        let Some(copy) = &self.copy else {
            return Taken::Refused;
        };
        match String::from_utf8(bytes) {
            Ok(text) if !text.is_empty() && text.len() <= newport_terminal::MAX_COPY => {
                let current = self.computer().ok().map(|(revision, _)| revision);
                if !self.copied.record(text.as_bytes(), current) {
                    return Taken::Repeated;
                }
                copy(text);
                Taken::Sent
            }
            _ => {
                self.event("copy_refused", "not text within the terminal's limit");
                Taken::Refused
            }
        }
    }
    /// A source that reads wherever `route` points at each request.
    pub(crate) fn routed(route: Route) -> Self {
        Self {
            route: Some(route),
            ..Self::new(Path::new(""))
        }
    }
    /// Logs `event` beside the snapshot this source reads: for a routed one,
    /// the Mac in use's, so a held shell leaves no log of its own behind.
    pub(crate) fn event(&self, event: &str, details: &str) {
        match &self.route {
            Some(route) => {
                if let Ok(path) = route() {
                    crate::diagnostics::event(&path, event, details);
                }
            }
            None => crate::diagnostics::event(&self.path, event, details),
        }
    }
    pub(crate) fn enable_data(&self, data: Data) {
        let _ = self.store.set(data);
    }
    /// The computer's clipboard, or the copy a program made since.
    pub fn offer(&self) -> io::Result<(Revision, BTreeMap<String, Vec<u8>>)> {
        Ok(self.copied.over(self.computer()?))
    }
    pub fn get(&self, revision: Revision, format: &str) -> io::Result<Vec<u8>> {
        match self.copied.get(revision, format) {
            Some(copied) => copied,
            None => self.computer_get(revision, format),
        }
    }
    fn computer(&self) -> io::Result<(Revision, BTreeMap<String, Vec<u8>>)> {
        if let Some(route) = &self.route {
            let path = route()?;
            let (revision, values) = offer(&path)?;
            return Ok((tagged(revision, &path), values));
        }
        if let Some(store) = self.store.get() {
            let offer = store.offer()?;
            Ok((
                (0, offer.revision as u64, 0),
                offer.formats.into_iter().map(|s| (s, vec![])).collect(),
            ))
        } else {
            offer(&self.path)
        }
    }
    fn computer_get(&self, revision: Revision, format: &str) -> io::Result<Vec<u8>> {
        if let Some(route) = &self.route {
            let path = route()?;
            // The Mac changed since the offer: its revisions aren't comparable.
            let revision =
                untagged(revision, &path).ok_or_else(|| io::Error::other("clipboard changed"))?;
            return get(&path, revision, format);
        }
        if let Some(store) = self.store.get() {
            store.get(revision.1 as i64, format)
        } else {
            get(&self.path, revision, format)
        }
    }
}

pub(crate) type Revision = (u64, u64, u64);
/// A live clipboard's revision, marked with the path it came from so two
/// Macs' revisions never compare equal. A snapshot file's already is.
fn tagged(revision: Revision, path: &Path) -> Revision {
    if revision.0 != 0 {
        return revision;
    }
    (revision.0, revision.1, tag(path))
}
/// The revision `tagged` marked, if it came from `path`.
fn untagged(revision: Revision, path: &Path) -> Option<Revision> {
    if revision.0 != 0 {
        return Some(revision);
    }
    (revision.2 == tag(path)).then_some((0, revision.1, 0))
}
fn tag(path: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    hasher.finish()
}
fn socket_path(path: &Path) -> PathBuf {
    path.with_file_name("clipboard.sock")
}
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);
const METADATA_TIMEOUT: Duration = Duration::from_secs(3);
fn payload_timeout(format: &str) -> Duration {
    if format == newport_protocol::clipboard::FILES_FORMAT {
        Duration::from_secs(30 * 60)
    } else {
        Duration::from_secs(17)
    }
}
fn remaining(until: Instant) -> io::Result<Duration> {
    until
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "Clipboard source deadline expired"))
}
struct Deadline<'a> {
    stream: &'a mut UnixStream,
    until: Instant,
}
impl Read for Deadline<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        crate::local_socket::read_timeout(self.stream, remaining(self.until)?)?;
        self.stream.read(bytes)
    }
}
impl Write for Deadline<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        crate::local_socket::write_timeout(self.stream, remaining(self.until)?)?;
        self.stream.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}
fn ended(input: &mut impl Read) -> io::Result<()> {
    let mut byte = [0];
    match input.read(&mut byte) {
        Ok(0) => Ok(()),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Clipboard source sent trailing data",
        )),
        Err(error) => Err(error),
    }
}
#[derive(Debug)]
struct GreetingError(io::Error);
impl std::fmt::Display for GreetingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.kind() == io::ErrorKind::InvalidData {
            write!(
                f,
                "The clipboard source uses an incompatible protocol; reconnect Newport: {}",
                self.0
            )
        } else {
            write!(
                f,
                "The clipboard source did not complete its greeting: {}",
                self.0
            )
        }
    }
}
impl std::error::Error for GreetingError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}
fn connect(path: &Path, request: &Request, until: Instant) -> io::Result<UnixStream> {
    let greeting = until.min(Instant::now() + HANDSHAKE_TIMEOUT);
    let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
    socket.connect_timeout(
        &socket2::SockAddr::unix(socket_path(path))?,
        remaining(greeting)?,
    )?;
    let descriptor: std::os::fd::OwnedFd = socket.into();
    let mut stream = UnixStream::from(descriptor);
    let hello = wire::read_response(&mut Deadline {
        stream: &mut stream,
        until: greeting,
    })
    .map_err(io::Error::from)
    .map_err(|error| io::Error::new(error.kind(), GreetingError(error)))?;
    if hello
        != (Response::Hello {
            version: protocol::VERSION,
        })
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            GreetingError(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unexpected clipboard greeting",
            )),
        ));
    }
    wire::write_request(
        &mut Deadline {
            stream: &mut stream,
            until: greeting,
        },
        request,
    )?;
    stream.shutdown(std::net::Shutdown::Write)?;
    Ok(stream)
}
fn response(stream: &mut UnixStream, until: Instant) -> io::Result<Response> {
    match wire::read_response(&mut Deadline { stream, until })? {
        Response::Failed(error) => Err(io::Error::other(error)),
        response => Ok(response),
    }
}
pub(crate) fn offer(path: &Path) -> io::Result<(Revision, BTreeMap<String, Vec<u8>>)> {
    if socket_path(path).exists() {
        let until = Instant::now() + METADATA_TIMEOUT;
        let mut stream = connect(path, &Request::Offer, until)?;
        let Response::Offer(offer) = response(&mut stream, until)? else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unexpected clipboard source metadata",
            ));
        };
        ended(&mut Deadline {
            stream: &mut stream,
            until,
        })?;
        Ok((
            (0, offer.revision as u64, 0),
            offer.formats.into_iter().map(|s| (s, vec![])).collect(),
        ))
    } else {
        // Confirm the file didn't change during the read.
        let before = revision(path)?;
        let values = snapshot::read(path)?;
        if revision(path)? != before {
            return Err(io::Error::other("clipboard changed"));
        }
        Ok((before, values))
    }
}
fn revision(path: &Path) -> io::Result<Revision> {
    let m = std::fs::symlink_metadata(path)?;
    Ok((m.dev(), m.ino(), m.len()))
}
pub(crate) fn get(path: &Path, revision: Revision, format: &str) -> io::Result<Vec<u8>> {
    if revision.0 == 0 {
        let until = Instant::now() + payload_timeout(format);
        let request = Request::Read {
            revision: revision.1 as i64,
            format: format.into(),
        };
        let mut stream = connect(path, &request, until)?;
        let mut bytes = Vec::new();
        loop {
            let Response::Chunk(chunk) = response(&mut stream, until)? else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Unexpected clipboard source payload",
                ));
            };
            if bytes.len().saturating_add(chunk.bytes.len()) > imports::LIMIT {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Clipboard source payload exceeds limit",
                ));
            }
            bytes.extend(chunk.bytes);
            if chunk.eof {
                ended(&mut Deadline {
                    stream: &mut stream,
                    until,
                })?;
                return Ok(bytes);
            }
        }
    } else {
        let (current, mut values) = offer(path)?;
        if current != revision {
            return Err(io::Error::other("clipboard changed"));
        }
        values
            .remove(format)
            .ok_or_else(|| io::Error::other("clipboard format unavailable"))
    }
}
pub(crate) struct Service {
    stop: Arc<AtomicBool>,
    store: Data,
    thread: Option<thread::JoinHandle<()>>,
    streams: Arc<std::sync::Mutex<Vec<std::sync::Weak<UnixStream>>>>,
    _endpoint: Endpoint,
}
struct Pending {
    done: Arc<AtomicBool>,
    metadata: crate::clipboard_service::ReadCancellation,
    payload: crate::clipboard_service::ReadCancellation,
    task: Option<thread::JoinHandle<()>>,
}
impl Pending {
    fn new(stream: Arc<UnixStream>, stop: Arc<AtomicBool>, until: Instant) -> io::Result<Self> {
        let done = Arc::new(AtomicBool::new(false));
        let finished = done.clone();
        let metadata = crate::clipboard_service::ReadCancellation::default();
        let payload = crate::clipboard_service::ReadCancellation::default();
        let (cancel_metadata, cancel_payload) = (metadata.clone(), payload.clone());
        let task = thread::Builder::new()
            .name("clipboard-source".into())
            .spawn(move || {
                while !finished.load(Ordering::Acquire) {
                    // Request EOF is intentional. Only full closure cancels reads.
                    let mut poll = libc::pollfd {
                        fd: stream.as_raw_fd(),
                        events: libc::POLLHUP | libc::POLLERR,
                        revents: 0,
                    };
                    // SAFETY: the owned stream keeps this descriptor alive.
                    let result = unsafe { libc::poll(&mut poll, 1, 20) };
                    if result < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted
                    {
                        continue;
                    }
                    let hung_up = poll.revents & libc::POLLHUP != 0;
                    if stop.load(Ordering::Acquire)
                        || Instant::now() >= until
                        || result < 0
                        || poll.revents & (libc::POLLERR | libc::POLLNVAL) != 0
                        || (hung_up && crate::local_socket::hangup_is_disconnect(&stream))
                    {
                        cancel_metadata.cancel();
                        cancel_payload.cancel();
                        crate::local_socket::shutdown(&stream);
                        return;
                    }
                    if hung_up {
                        // Read EOF stays poll-ready while a half-closed peer waits.
                        thread::sleep(Duration::from_millis(20));
                    }
                }
            })?;
        Ok(Self {
            done,
            metadata,
            payload,
            task: Some(task),
        })
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Release);
        if let Some(task) = self.task.take() {
            let _ = task.join();
        }
    }
}
fn send(stream: &mut UnixStream, until: Instant, value: &Response) -> io::Result<()> {
    wire::write_response(
        &mut Deadline {
            stream,
            until: until.min(Instant::now() + Duration::from_secs(5)),
        },
        value,
    )
    .map_err(io::Error::from)
}
fn serve_source(
    mut stream: UnixStream,
    data: Data,
    stop: Arc<AtomicBool>,
    control: Arc<UnixStream>,
) -> io::Result<()> {
    let started = Instant::now();
    let handshake = started + HANDSHAKE_TIMEOUT;
    send(
        &mut stream,
        handshake,
        &Response::Hello {
            version: protocol::VERSION,
        },
    )?;
    let request = wire::read_request(&mut Deadline {
        stream: &mut stream,
        until: handshake,
    })?;
    ended(&mut Deadline {
        stream: &mut stream,
        until: handshake,
    })?;
    let until = started
        + match &request {
            Request::Offer => METADATA_TIMEOUT,
            Request::Read { format, .. } => payload_timeout(format),
        };
    let pending = Pending::new(control.clone(), stop.clone(), until)?;
    let cancelled = || {
        stop.load(Ordering::Acquire)
            || Instant::now() >= until
            || pending.done.load(Ordering::Acquire)
            || disconnected(&control)
    };
    let mut sent = false;
    let result = (|| -> io::Result<()> {
        match request {
            Request::Offer => {
                let offer = match &data {
                    Data::Local(store) => store.offer(),
                    Data::Runtime(reader) => {
                        let value = reader.offer_until(
                            until.min(Instant::now() + METADATA_TIMEOUT),
                            &pending.metadata,
                        )?;
                        Offer {
                            revision: value.revision,
                            formats: value.formats,
                        }
                    }
                };
                sent = true;
                send(&mut stream, until, &Response::Offer(offer))?;
            }
            Request::Read { revision, format } => match &data {
                Data::Local(store) => {
                    let bytes = store.get_shared(revision, &format, Some(&cancelled))?;
                    if bytes.len() > imports::LIMIT {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Clipboard payload exceeds limit",
                        ));
                    }
                    let mut offset = 0;
                    loop {
                        let end = (offset + imports::CHUNK).min(bytes.len());
                        let eof = end == bytes.len();
                        sent = true;
                        send(
                            &mut stream,
                            until,
                            &Response::Chunk(Chunk {
                                bytes: bytes[offset..end].to_vec(),
                                eof,
                            }),
                        )?;
                        if eof {
                            break;
                        }
                        offset = end;
                    }
                }
                Data::Runtime(reader) => {
                    let offer = reader.offer_until(
                        until.min(Instant::now() + METADATA_TIMEOUT),
                        &pending.metadata,
                    )?;
                    if offer.revision != revision {
                        return Err(io::Error::other("Clipboard offer changed"));
                    }
                    let mut read = reader.read_until_with_cancellation(
                        &offer,
                        &format,
                        pending.payload.clone(),
                        until,
                    )?;
                    loop {
                        let chunk = read.next(imports::CHUNK)?;
                        let eof = chunk.eof;
                        sent = true;
                        send(&mut stream, until, &Response::Chunk(chunk))?;
                        if eof {
                            break;
                        }
                    }
                }
            },
        }
        Ok(())
    })();
    if let Err(error) = result {
        if sent {
            return Err(error);
        }
        let mut message = error.to_string();
        if message.len() > 4096 {
            let mut end = 4096;
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            message.truncate(end);
        }
        send(
            &mut stream,
            until,
            &Response::Failed(newport_protocol::common::Error {
                code: "unavailable".into(),
                message,
                retry: newport_protocol::common::Retry::No,
            }),
        )?;
    }
    Ok(())
}
fn disconnected(stream: &UnixStream) -> bool {
    let mut poll = libc::pollfd {
        fd: stream.as_raw_fd(),
        events: libc::POLLHUP | libc::POLLERR,
        revents: 0,
    };
    // SAFETY: one borrowed live descriptor; polling consumes no request bytes.
    let result = unsafe { libc::poll(&mut poll, 1, 0) };
    (result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted)
        || poll.revents & (libc::POLLERR | libc::POLLNVAL) != 0
        || (poll.revents & libc::POLLHUP != 0 && crate::local_socket::hangup_is_disconnect(stream))
}
impl Service {
    pub fn start(path: &Path, store: Data) -> io::Result<Self> {
        let (listener, endpoint) = Endpoint::fixed(&socket_path(path))?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let data = store.clone();
        let streams: Arc<std::sync::Mutex<Vec<std::sync::Weak<UnixStream>>>> = Default::default();
        let registered = streams.clone();
        let thread = thread::Builder::new()
            .name("clipboard-source-listener".into())
            .spawn(move || {
                let mut tasks: Vec<thread::JoinHandle<()>> = Vec::new();
                while !stopped.load(Ordering::Acquire) {
                    let mut index = 0;
                    while index < tasks.len() {
                        if tasks[index].is_finished() {
                            let _ = tasks.swap_remove(index).join();
                        } else {
                            index += 1;
                        }
                    }
                    match listener.accept() {
                        Ok((stream, _)) => {
                            if tasks.len() >= 24 {
                                continue;
                            }
                            if stream.set_nonblocking(false).is_err() {
                                continue;
                            }
                            let Ok(control) = stream.try_clone().map(Arc::new) else {
                                continue;
                            };
                            {
                                let mut streams = registered
                                    .lock()
                                    .unwrap_or_else(|poison| poison.into_inner());
                                if stopped.load(Ordering::Acquire) {
                                    break;
                                }
                                streams.retain(|stream| stream.strong_count() > 0);
                                streams.push(Arc::downgrade(&control));
                            }
                            let (data, stop) = (data.clone(), stopped.clone());
                            if let Ok(task) = thread::Builder::new()
                                .name("clipboard-source-request".into())
                                .spawn(move || {
                                    let _ = serve_source(stream, data, stop, control);
                                })
                            {
                                tasks.push(task);
                            }
                        }
                        _ => {
                            thread::sleep(Duration::from_millis(5));
                        }
                    }
                }
                for task in tasks {
                    let _ = task.join();
                }
            })?;
        Ok(Self {
            stop,
            store,
            thread: Some(thread),
            streams,
            _endpoint: endpoint,
        })
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.store.close();
        for stream in self
            .streams
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .iter()
            .filter_map(std::sync::Weak::upgrade)
        {
            crate::local_socket::shutdown(&stream);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod routed_tests {
    use super::*;
    use std::{fs, sync::Mutex};

    #[test]
    fn local_reads_have_no_integration_request_identity() {
        use newport_protocol::clipboard::imports::{Encoding, Reply, Request as Demand};
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("private/snapshot.tar");
        let store = crate::demand::Store::new();
        store.update(Offer {
            revision: -7,
            formats: vec![TEXT.into()],
        });
        let _service = Service::start(&path, Data::Local(store.clone())).unwrap();
        let (revision, formats) = offer(&path).unwrap();
        assert!(formats.contains_key(TEXT));
        let source = path.clone();
        let reading = thread::spawn(move || get(&source, revision, TEXT));
        let deadline = Instant::now() + Duration::from_secs(2);
        let request = loop {
            if let Some(request) = store.take_requests().pop() {
                break request;
            }
            if reading.is_finished() {
                panic!(
                    "source read ended before requesting data: {:?}",
                    reading.join()
                );
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        };
        request.validate().unwrap();
        assert_ne!(request.id, 0);
        let payload = vec![42; imports::CHUNK + 11];
        for (index, chunk) in payload.chunks(imports::CHUNK).enumerate() {
            store
                .reply(&Reply::new(
                    &request,
                    Encoding::Raw,
                    index == 1,
                    chunk.to_vec(),
                ))
                .unwrap();
        }
        assert_eq!(reading.join().unwrap().unwrap(), payload);
        assert!(
            Demand {
                id: 0,
                revision: -7,
                format: TEXT.into()
            }
            .validate()
            .is_err()
        );
        assert_eq!(get(&path, revision, TEXT).unwrap(), payload);
    }

    #[test]
    fn incompatible_source_receives_no_metadata_or_payload_request() {
        use std::os::unix::net::UnixListener;
        for payload in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("snapshot.tar");
            let listener = UnixListener::bind(socket_path(&path)).unwrap();
            let peer = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(4)))
                    .unwrap();
                let mut bytes = Vec::new();
                stream.read_to_end(&mut bytes).unwrap();
                assert!(bytes.is_empty());
            });
            let result = if payload {
                get(&path, (0, 1, 0), TEXT).map(|_| ())
            } else {
                offer(&path).map(|_| ())
            };
            assert!(result.is_err());
            peer.join().unwrap();
        }
    }
    #[test]
    fn service_drop_cancels_a_pending_source_request() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("private/snapshot.tar");
        let store = crate::demand::Store::new();
        store.update(Offer {
            revision: 1,
            formats: vec![TEXT.into()],
        });
        let service = Service::start(&path, Data::Local(store.clone())).unwrap();
        let source = path.clone();
        let reading = thread::spawn(move || get(&source, (0, 1, 0), TEXT));
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if !store.take_requests().is_empty() {
                break;
            }
            if reading.is_finished() {
                panic!(
                    "source read ended before requesting data: {:?}",
                    reading.join()
                );
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        let started = Instant::now();
        drop(service);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(reading.join().unwrap().is_err());
    }
    #[test]
    fn source_rejects_trailing_request_data_without_fetching_payload() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("private/snapshot.tar");
        let store = crate::demand::Store::new();
        store.update(Offer {
            revision: 1,
            formats: vec![TEXT.into()],
        });
        let _service = Service::start(&path, Data::Local(store.clone())).unwrap();
        let mut stream = UnixStream::connect(socket_path(&path)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        assert_eq!(
            wire::read_response(&mut stream).unwrap(),
            Response::Hello {
                version: protocol::VERSION
            }
        );
        wire::write_request(
            &mut stream,
            &Request::Read {
                revision: 1,
                format: TEXT.into(),
            },
        )
        .unwrap();
        stream.write_all(&[0]).unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        assert!(wire::read_response(&mut stream).is_err());
        assert!(store.take_requests().is_empty());
    }

    /// A Mac's clipboard as a snapshot, which a source reads as it reads a
    /// live connection's.
    fn clipboard(path: &Path, text: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut tar = tar::Builder::new(fs::File::create(path).unwrap());
        let mut header = tar::Header::new_gnu();
        header.set_size(text.len() as u64);
        header.set_mode(0o600);
        header.set_cksum();
        tar.append_data(&mut header, "text/plain", text).unwrap();
        tar.finish().unwrap();
    }

    #[test]
    fn a_held_shells_clipboard_reads_the_mac_it_points_at_each_time() {
        let dir = tempfile::tempdir().unwrap();
        let studio = dir.path().join("studio/snapshot.tar");
        let air = dir.path().join("air/snapshot.tar");
        clipboard(&studio, b"from the studio");
        clipboard(&air, b"from the air");
        let current = Arc::new(Mutex::new(studio));
        let route: Route = {
            let current = current.clone();
            Arc::new(move || Ok(current.lock().unwrap().clone()))
        };
        let source = Source::routed(route);
        let (revision, formats) = source.offer().unwrap();
        assert!(formats.contains_key("text/plain"));
        assert_eq!(
            source.get(revision, "text/plain").unwrap(),
            b"from the studio"
        );

        // Someone uses the shell on the Air: its clipboard is read next, and
        // a read begun on the Studio isn't answered from the Air's.
        *current.lock().unwrap() = air;
        assert!(source.get(revision, "text/plain").is_err());
        let (revision, _) = source.offer().unwrap();
        assert_eq!(source.get(revision, "text/plain").unwrap(), b"from the air");

        // With no Mac to read from, the read says why.
        let nowhere: Route = Arc::new(|| {
            Err(io::Error::other(
                "Open this terminal in Newport to continue.",
            ))
        });
        let error = Source::routed(nowhere).offer().unwrap_err();
        assert_eq!(
            error.to_string(),
            "Open this terminal in Newport to continue."
        );
    }
}
