use super::deadline::{Deadline, remaining};
use super::{PreparationCancellation, ReadCancellation};
use newport_protocol::clipboard::{
    Access, Chunk, CopyLease, ExportOffer, NativeRequest, Offer, ProviderLease, ProviderPoll,
    service::{self, ExportRead, Request, Response, wire},
};
use std::{
    io,
    os::fd::AsRawFd,
    os::unix::{
        fs::{FileTypeExt, MetadataExt},
        net::UnixStream,
    },
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

struct Connection {
    stream: UnixStream,
    runtime: String,
    worker: String,
    closed: bool,
    _cancellation: Option<Arc<UnixStream>>,
}
#[derive(Debug)]
struct GreetingError(io::Error);
impl std::fmt::Display for GreetingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.kind() == io::ErrorKind::InvalidData {
            write!(
                f,
                "Clipboard service uses an incompatible protocol. Update Newport and reconnect: {}",
                self.0
            )
        } else {
            write!(
                f,
                "Clipboard service did not complete its greeting: {}",
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
fn greeting_error(error: wire::Error) -> io::Error {
    let error = io::Error::from(error);
    io::Error::new(error.kind(), GreetingError(error))
}
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(7);
enum Cancellation<'a> {
    Read(&'a ReadCancellation),
    Preparation(&'a PreparationCancellation),
}
impl Cancellation<'_> {
    fn attach(&self, stream: &UnixStream) -> io::Result<Arc<UnixStream>> {
        match self {
            Self::Read(value) => value.attach(stream),
            Self::Preparation(value) => value.attach(stream),
        }
    }
}
impl Connection {
    fn connect(access: &Access) -> io::Result<Self> {
        Self::connect_until(access, Instant::now() + HANDSHAKE_TIMEOUT, None)
    }
    fn connect_with_cancellation(
        access: &Access,
        cancellation: Option<&ReadCancellation>,
    ) -> io::Result<Self> {
        if let Some(cancellation) = cancellation {
            cancellation.begin()?;
        }
        Self::connect_until(
            access,
            Instant::now() + HANDSHAKE_TIMEOUT,
            cancellation.map(Cancellation::Read),
        )
    }
    fn connect_until(
        access: &Access,
        until: Instant,
        cancellation: Option<Cancellation<'_>>,
    ) -> io::Result<Self> {
        remaining(until)?;
        let parent = std::fs::symlink_metadata(
            access
                .path
                .parent()
                .ok_or_else(|| io::Error::other("Invalid clipboard endpoint"))?,
        )?;
        let socket = std::fs::symlink_metadata(&access.path)?;
        if !access.path.is_absolute()
            || !parent.is_dir()
            || parent.uid() != crate::paths::effective_uid()
            || parent.mode() & 0o077 != 0
            || !socket.file_type().is_socket()
            || socket.uid() != parent.uid()
        {
            return Err(io::Error::other(
                "Clipboard endpoint is not private to this account",
            ));
        }
        let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
        socket.connect_timeout(
            &socket2::SockAddr::unix(&access.path)?,
            remaining(until)?.min(Duration::from_secs(2)),
        )?;
        let fd: std::os::fd::OwnedFd = socket.into();
        let stream = UnixStream::from(fd);
        stream.set_nonblocking(false)?;
        let control = cancellation
            .as_ref()
            .map(|cancellation| cancellation.attach(&stream))
            .transpose()?;
        let mut connection = Self {
            stream,
            runtime: access.runtime.clone(),
            worker: access.worker.clone(),
            closed: false,
            _cancellation: control,
        };
        let hello: Response = wire::read(&mut Deadline::until(&mut connection.stream, until))
            .map_err(greeting_error)?;
        match hello {
            Response::Hello(hello) if hello.version == service::VERSION && hello.runtime == access.runtime => {},
            _ => return Err(io::Error::new(io::ErrorKind::InvalidData, newport_protocol::common::Error {
                code: "incompatible".into(), message: "Clipboard service did not announce this runtime. Update Newport and reconnect.".into(), retry: newport_protocol::common::Retry::No,
            })),
        }
        ready(connection.call_until(&Request::Authenticate(access.clone()), until)?)?;
        Ok(connection)
    }
    // A prepared, authenticated connection is idle. Observe already-visible
    // revocation or unsolicited bytes without consuming data or sending IPC.
    fn ready_to_commit(&self, deadline: Instant) -> io::Result<()> {
        loop {
            remaining(deadline)?;
            let mut byte = 0u8;
            // SAFETY: one borrowed live socket and a writable byte. MSG_PEEK
            // consumes nothing, and MSG_DONTWAIT cannot block or alter its mode.
            let count = unsafe {
                libc::recv(
                    self.stream.as_raw_fd(),
                    (&mut byte as *mut u8).cast(),
                    1,
                    libc::MSG_PEEK | libc::MSG_DONTWAIT,
                )
            };
            match count {
                0 => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Prepared clipboard access ended",
                    ));
                }
                count if count > 0 => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Prepared clipboard sent unsolicited data",
                    ));
                }
                _ => {
                    let error = io::Error::last_os_error();
                    match error.kind() {
                        io::ErrorKind::WouldBlock => return Ok(()),
                        io::ErrorKind::Interrupted => continue,
                        _ => return Err(error),
                    }
                }
            }
        }
    }
    fn call(&mut self, request: &Request) -> io::Result<Response> {
        self.call_until(request, Instant::now() + response_timeout(request))
    }
    fn call_until(&mut self, request: &Request, until: Instant) -> io::Result<Response> {
        if self.closed {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "Clipboard service connection closed after an incomplete exchange",
            ));
        }
        let result = self.exchange(request, until);
        match result {
            Err(error) => {
                self.retire();
                Err(error)
            }
            Ok(Response::Error(error)) => Err(io::Error::other(error)),
            Ok(response) => {
                if let Err(error) = self.validate_response(request, &response) {
                    self.retire();
                    return Err(io::Error::new(io::ErrorKind::InvalidData, error));
                }
                Ok(response)
            }
        }
    }
    fn exchange(&mut self, request: &Request, until: Instant) -> io::Result<Response> {
        let writing = until.min(Instant::now() + Duration::from_secs(2));
        wire::write(&mut Deadline::until(&mut self.stream, writing), request)
            .map_err(io::Error::from)?;
        wire::read(&mut Deadline::until(&mut self.stream, until)).map_err(io::Error::from)
    }
    fn retire(&mut self) {
        // A later reply could belong to the failed mutation. Never reuse or
        // reconnect this connection to guess whether that mutation succeeded.
        self.closed = true;
        crate::local_socket::shutdown(&self.stream);
    }
    fn validate_response(
        &self,
        request: &Request,
        response: &Response,
    ) -> Result<(), service::Invalid> {
        let valid = match (request, response) {
            (
                Request::Authenticate(_)
                | Request::Claim
                | Request::Enabled(_)
                | Request::Publish { .. }
                | Request::Reply(_)
                | Request::NativeReply { .. },
                Response::Ready,
            )
            | (Request::Poll, Response::Requests(_))
            | (Request::NativePublish { .. }, Response::Published(_)) => true,
            (Request::Offer, Response::Offer(offer)) => offer.runtime == self.runtime,
            (Request::Read { offset, limit, .. }, Response::Chunk(chunk)) => {
                chunk.bytes.len() <= *limit
                    && offset
                        .checked_add(chunk.bytes.len())
                        .is_some_and(|end| end <= newport_protocol::clipboard::imports::LIMIT)
            }
            (Request::NativePoll, Response::Provider(poll)) => {
                poll.offer.runtime == self.runtime
                    && poll.lease.as_ref().is_none_or(|lease| {
                        lease.runtime == self.runtime && lease.worker == self.worker
                    })
            }
            (Request::ExportOffer, Response::Export(offer)) => offer
                .as_ref()
                .is_none_or(|offer| offer.offer.runtime == self.runtime),
            (Request::CommitCopy(offer), Response::Copy(copy)) => {
                copy.as_ref().is_none_or(|copy| {
                    copy.offer == *offer && copy.offer.offer.runtime == self.runtime
                })
            }
            (Request::CopyStatus | Request::ReleaseCopy(_), Response::Copy(copy)) => copy
                .as_ref()
                .is_none_or(|copy| copy.offer.offer.runtime == self.runtime),
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(service::Invalid("response for this request and runtime"))
        }
    }
}
/// Only a files read may outlast the broker's silence limit: the broker
/// keeps it alive with the desktop's keep-alives and fails it when they stop.
fn response_timeout(request: &Request) -> Duration {
    match request {
        Request::Read { format, .. } if format == newport_protocol::clipboard::FILES_FORMAT => {
            Duration::from_secs(30 * 60)
        }
        Request::Read { .. } => Duration::from_secs(17),
        _ => Duration::from_secs(3),
    }
}
fn ready(response: Response) -> io::Result<()> {
    if matches!(response, Response::Ready) {
        Ok(())
    } else {
        Err(io::Error::other("Unexpected clipboard response"))
    }
}
#[derive(Clone)]
pub struct Reader {
    access: Access,
    metadata: Arc<Mutex<Option<Connection>>>,
}
impl Reader {
    /// Reads as the worker `access` admits, which may have handed it over.
    pub fn new(access: Access) -> Self {
        Self {
            access,
            metadata: Default::default(),
        }
    }
    /// Metadata only. No native display or payload request is created.
    pub fn offer(&self) -> io::Result<Offer> {
        match self.metadata(Request::Offer)? {
            Response::Offer(offer) if offer.runtime == self.access.runtime => Ok(offer),
            _ => Err(io::Error::other("Invalid clipboard metadata response")),
        }
    }
    /// One cancellable metadata exchange under its caller's whole deadline.
    pub(crate) fn offer_until(
        &self,
        deadline: Instant,
        cancellation: &ReadCancellation,
    ) -> io::Result<Offer> {
        cancellation.begin()?;
        let mut connection = Connection::connect_until(
            &self.access,
            deadline,
            Some(Cancellation::Read(cancellation)),
        )?;
        match connection.call_until(&Request::Offer, deadline)? {
            Response::Offer(offer) => Ok(offer),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid clipboard metadata response",
            )),
        }
    }
    /// A payload is never reconnected or replayed after an ambiguous failure.
    pub(crate) fn read_until_with_cancellation(
        &self,
        offer: &Offer,
        format: &str,
        cancellation: ReadCancellation,
        deadline: Instant,
    ) -> io::Result<Read> {
        self.open_until(offer, format, cancellation, None, Some(deadline))
    }
    /// Only the owning integration producer can observe export offers. Ordinary
    /// local reads confer no authorization to write the host clipboard.
    pub fn export_offer(&self) -> io::Result<Option<ExportOffer>> {
        match self.metadata(Request::ExportOffer)? {
            Response::Export(offer)
                if offer
                    .as_ref()
                    .is_none_or(|offer| offer.offer.runtime == self.access.runtime) =>
            {
                Ok(offer)
            }
            _ => Err(io::Error::other(
                "Invalid clipboard export metadata response",
            )),
        }
    }
    /// One non-reconnecting connection owns at most one accepted copy. Poll its
    /// status with the integration loop; dropping it revokes outstanding reads.
    pub fn copy_session(&self) -> io::Result<CopySession> {
        let mut session = CopySession {
            connection: Some(Connection::connect(&self.access)?),
            runtime: self.access.runtime.clone(),
        };
        session.current()?; // Require the integration producer before returning.
        Ok(session)
    }
    fn metadata(&self, request: Request) -> io::Result<Response> {
        let mut slot = self
            .metadata
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for attempt in 0..2 {
            if slot.is_none() {
                *slot = Some(Connection::connect(&self.access)?);
            }
            let result = slot
                .as_mut()
                .ok_or_else(|| io::Error::other("Clipboard metadata connection missing"))?
                .call(&request);
            match result {
                Ok(response) => return Ok(response),
                Err(error) => {
                    *slot = None;
                    // Only idempotent metadata retries a closed idle transport.
                    // Preserve the final typed failure, including remote errors.
                    if attempt == 1
                        || !matches!(
                            error.kind(),
                            io::ErrorKind::BrokenPipe
                                | io::ErrorKind::ConnectionReset
                                | io::ErrorKind::ConnectionAborted
                                | io::ErrorKind::UnexpectedEof
                                | io::ErrorKind::NotConnected
                        )
                    {
                        return Err(error);
                    }
                }
            }
        }
        Err(io::Error::other("Clipboard metadata retry ended"))
    }
    /// Create a bounded local IPC reader. `next` can block for the producer;
    /// use a worker task, not the compositor or SSH event-loop thread.
    pub fn read(&self, offer: &Offer, format: &str) -> io::Result<Read> {
        self.read_with_cancellation(offer, format, ReadCancellation::default())
    }
    /// Like `read`, with a cancellation handle created before starting the task.
    /// A pending connect is bounded to two seconds; authentication and payload
    /// waits are interrupted by cancellation as soon as the socket connects.
    pub fn read_with_cancellation(
        &self,
        offer: &Offer,
        format: &str,
        cancellation: ReadCancellation,
    ) -> io::Result<Read> {
        self.open(offer, format, cancellation, None)
    }
    /// Fetch for a host export under its exact route/focus lease. The desktop
    /// must still validate its native write lease immediately before applying.
    pub fn read_export(
        &self,
        offer: &ExportOffer,
        format: &str,
        cancellation: ReadCancellation,
    ) -> io::Result<Read> {
        self.open(
            &offer.offer,
            format,
            cancellation,
            Some(ExportRead::Focused(offer.clone())),
        )
    }
    /// Read an acknowledged copy without requiring its source window to retain
    /// focus. The runtime checks the exact live copy authority on every chunk.
    pub fn read_committed(
        &self,
        copy: &CopyLease,
        format: &str,
        cancellation: ReadCancellation,
    ) -> io::Result<Read> {
        self.open(
            &copy.offer.offer,
            format,
            cancellation,
            Some(ExportRead::Committed(copy.clone())),
        )
    }
    fn open(
        &self,
        offer: &Offer,
        format: &str,
        cancellation: ReadCancellation,
        export: Option<ExportRead>,
    ) -> io::Result<Read> {
        self.open_until(offer, format, cancellation, export, None)
    }
    fn open_until(
        &self,
        offer: &Offer,
        format: &str,
        cancellation: ReadCancellation,
        export: Option<ExportRead>,
        deadline: Option<Instant>,
    ) -> io::Result<Read> {
        if offer.runtime != self.access.runtime
            || !offer.formats.iter().any(|name| name == format)
            || !newport_protocol::clipboard::imports::valid_format(format)
        {
            return Err(io::Error::other(
                "Clipboard offer belongs to another runtime or format",
            ));
        }
        Ok(Read {
            connection: Some(if let Some(deadline) = deadline {
                cancellation.begin()?;
                Connection::connect_until(
                    &self.access,
                    deadline,
                    Some(Cancellation::Read(&cancellation)),
                )?
            } else {
                Connection::connect_with_cancellation(&self.access, Some(&cancellation))?
            }),
            runtime: offer.runtime.clone(),
            revision: offer.revision,
            format: format.into(),
            offset: 0,
            done: false,
            export,
            deadline,
        })
    }
    pub(crate) fn get(&self, revision: i64, format: &str) -> io::Result<Vec<u8>> {
        let offer = self.offer()?;
        if offer.revision != revision {
            return Err(io::Error::other("Clipboard offer changed"));
        }
        let mut read = self.read(&offer, format)?;
        let mut bytes = vec![];
        loop {
            let chunk = read.next(newport_protocol::clipboard::imports::CHUNK)?;
            bytes.extend(chunk.bytes);
            if chunk.eof {
                return Ok(bytes);
            }
        }
    }
}
pub struct Read {
    connection: Option<Connection>,
    runtime: String,
    revision: i64,
    format: String,
    offset: usize,
    done: bool,
    export: Option<ExportRead>,
    deadline: Option<Instant>,
}
impl Read {
    pub fn next(&mut self, limit: usize) -> io::Result<Chunk> {
        if self.done {
            return Ok(Chunk {
                bytes: vec![],
                eof: true,
            });
        }
        if limit == 0 || limit > newport_protocol::clipboard::imports::CHUNK {
            return Err(io::Error::other("Invalid clipboard chunk limit"));
        }
        let request = Request::Read {
            runtime: self.runtime.clone(),
            revision: self.revision,
            format: self.format.clone(),
            offset: self.offset,
            limit,
            export: self.export.clone(),
        };
        let connection = self
            .connection
            .as_mut()
            .ok_or_else(|| io::Error::other("Clipboard read already failed"))?;
        let response = match self.deadline {
            Some(deadline) => connection.call_until(&request, deadline),
            None => connection.call(&request),
        };
        match response {
            Ok(Response::Chunk(chunk))
                if chunk.bytes.len() <= limit
                    && self.offset + chunk.bytes.len()
                        <= newport_protocol::clipboard::imports::LIMIT
                    && (chunk.eof || !chunk.bytes.is_empty()) =>
            {
                self.offset += chunk.bytes.len();
                self.done = chunk.eof;
                if self.done {
                    self.connection = None;
                }
                Ok(chunk)
            }
            other => {
                self.connection = None;
                Err(match other {
                    Err(error) => error,
                    _ => io::Error::other("Invalid clipboard data response"),
                })
            }
        }
    }
}

/// The lifetime of a committed copy belongs to this exact IPC connection.
/// Ambiguous errors close it; commit/release are never replayed on a new socket.
pub struct CopySession {
    connection: Option<Connection>,
    runtime: String,
}
impl CopySession {
    fn call(&mut self, request: Request) -> io::Result<Option<CopyLease>> {
        let result = self
            .connection
            .as_mut()
            .ok_or_else(|| io::Error::other("Clipboard copy connection closed"))?
            .call(&request)
            .and_then(|response| match response {
                Response::Copy(copy)
                    if copy.as_ref().is_none_or(|copy| {
                        copy.offer.offer.runtime == self.runtime
                            && uuid::Uuid::parse_str(&copy.id).is_ok()
                    }) =>
                {
                    Ok(copy)
                }
                _ => Err(io::Error::other("Invalid clipboard copy response")),
            });
        if result.is_err() {
            self.connection.take();
        }
        result
    }
    pub fn commit(&mut self, offer: &ExportOffer) -> io::Result<Option<CopyLease>> {
        self.call(Request::CommitCopy(offer.clone()))
    }
    pub fn current(&mut self) -> io::Result<Option<CopyLease>> {
        self.call(Request::CopyStatus)
    }
    pub fn release(&mut self, copy: &CopyLease) -> io::Result<()> {
        self.call(Request::ReleaseCopy(copy.clone())).map(|_| ())
    }
}

/// Native selection producer assigned by the runtime control connection. All
/// methods block on local IPC; run them outside compositor/transport loops.
pub struct NativePublisher {
    connection: Mutex<Connection>,
}
impl NativePublisher {
    /// Publishes as the worker `access` admits, which may have handed it over.
    pub fn new(access: Access) -> io::Result<Self> {
        Self::new_until(access, Instant::now() + HANDSHAKE_TIMEOUT)
    }
    /// Authenticates within the caller's existing deadline, without publication.
    pub fn new_until(access: Access, deadline: Instant) -> io::Result<Self> {
        Ok(Self {
            connection: Mutex::new(Connection::connect_until(&access, deadline, None)?),
        })
    }
    /// Metadata and demand requests only. An unassigned worker gets no lease or
    /// requests and cannot publish. No host export permission is implied.
    pub fn poll(&self) -> io::Result<ProviderPoll> {
        match self
            .connection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .call(&Request::NativePoll)?
        {
            Response::Provider(poll) if poll.requests.len() <= 8 => Ok(poll),
            _ => Err(io::Error::other(
                "Invalid native clipboard provider response",
            )),
        }
    }
    /// Compare against the last observed runtime offer. False consumes the
    /// stale native revision; only a newer native copy may publish again.
    pub fn publish(
        &self,
        lease: &ProviderLease,
        expected: &Offer,
        revision: u64,
        formats: Vec<String>,
    ) -> io::Result<bool> {
        match self
            .connection
            .lock()
            .unwrap()
            .call(&Request::NativePublish {
                lease: lease.clone(),
                expected_runtime: expected.runtime.clone(),
                expected_revision: expected.revision,
                revision,
                formats,
            })? {
            Response::Published(accepted) => Ok(accepted),
            _ => Err(io::Error::other(
                "Invalid native clipboard publication response",
            )),
        }
    }
    pub fn reply(&self, request: &NativeRequest, bytes: &[u8], eof: bool) -> io::Result<()> {
        if bytes.len() > newport_protocol::clipboard::imports::CHUNK || (bytes.is_empty() && !eof) {
            return Err(io::Error::other("Invalid native clipboard reply chunk"));
        }
        ready(
            self.connection
                .lock()
                .unwrap()
                .call(&Request::NativeReply {
                    request: request.clone(),
                    chunk: Some(Chunk {
                        bytes: bytes.into(),
                        eof,
                    }),
                })?,
        )
    }
    pub fn fail(&self, request: &NativeRequest) -> io::Result<()> {
        ready(
            self.connection
                .lock()
                .unwrap()
                .call(&Request::NativeReply {
                    request: request.clone(),
                    chunk: None,
                })?,
        )
    }
}
/// Authenticated clipboard handles held until the display admits a viewer.
/// Preparation performs no publication or payload read; commit performs no IPC.
pub struct PreparedClipboard {
    reader: Reader,
    publisher: NativePublisher,
    cancellation: PreparationCancellation,
    deadline: Instant,
}
impl PreparedClipboard {
    pub fn prepare(
        access: Access,
        deadline: Instant,
        cancellation: &PreparationCancellation,
    ) -> io::Result<Self> {
        cancellation.begin()?;
        let metadata = Connection::connect_until(
            &access,
            deadline,
            Some(Cancellation::Preparation(cancellation)),
        )?;
        let publisher = Connection::connect_until(
            &access,
            deadline,
            Some(Cancellation::Preparation(cancellation)),
        )?;
        remaining(deadline)?;
        cancellation.check()?;
        Ok(Self {
            reader: Reader {
                access,
                metadata: Arc::new(Mutex::new(Some(metadata))),
            },
            publisher: NativePublisher {
                connection: Mutex::new(publisher),
            },
            cancellation: cancellation.clone(),
            deadline,
        })
    }
    /// Observe the original deadline and both idle sockets before atomically
    /// transferring them. A later connection loss is ordinary live failure.
    pub fn commit(self) -> io::Result<(Reader, NativePublisher)> {
        self.cancellation.commit(|| {
            self.reader
                .metadata
                .lock()
                .unwrap()
                .as_ref()
                .ok_or_else(|| io::Error::other("Prepared clipboard metadata connection missing"))?
                .ready_to_commit(self.deadline)?;
            self.publisher
                .connection
                .lock()
                .unwrap()
                .ready_to_commit(self.deadline)
        })?;
        Ok((self.reader, self.publisher))
    }
}

pub(crate) struct Producer {
    reader: Reader,
    connection: Mutex<Connection>,
}
impl Producer {
    pub(crate) fn new(access: Access) -> io::Result<Self> {
        let mut connection = Connection::connect(&access)?;
        ready(connection.call(&Request::Claim)?)?;
        Ok(Self {
            reader: Reader::new(access),
            connection: Mutex::new(connection),
        })
    }
    pub(crate) fn reader(&self) -> Reader {
        self.reader.clone()
    }
    pub(crate) fn enabled(&self, enabled: bool) -> io::Result<()> {
        ready(
            self.connection
                .lock()
                .unwrap()
                .call(&Request::Enabled(enabled))?,
        )
    }
    pub(crate) fn update(
        &self,
        offer: newport_protocol::clipboard::imports::Offer,
    ) -> io::Result<()> {
        ready(
            self.connection
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .call(&Request::Publish {
                    revision: offer.revision,
                    formats: offer.formats,
                })?,
        )
    }
    pub(crate) fn reply(
        &self,
        reply: &newport_protocol::clipboard::imports::Reply,
    ) -> io::Result<()> {
        ready(
            self.connection
                .lock()
                .unwrap()
                .call(&Request::Reply(reply.clone()))?,
        )
    }
    pub(crate) fn poll(&self, output: &mut impl io::Write) -> io::Result<()> {
        match self
            .connection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .call(&Request::Poll)?
        {
            Response::Requests(requests) if requests.len() <= 8 => {
                for request in requests {
                    newport_protocol::integration::wire::write(
                        output,
                        &newport_protocol::integration::ServerMessage::ClipboardRequest(request),
                    )?;
                }
                Ok(())
            }
            _ => Err(io::Error::other("Invalid clipboard producer response")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_protocol::clipboard::{Origin, service::wire::Message};
    use std::{
        io::{Read as _, Write as _},
        sync::mpsc,
    };

    fn connected_pair() -> (Connection, UnixStream) {
        let (stream, peer) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        (
            Connection {
                stream,
                runtime: "runtime".into(),
                worker: "worker".into(),
                closed: false,
                _cancellation: None,
            },
            peer,
        )
    }
    fn no_more_requests(peer: &mut UnixStream) {
        let mut bytes = Vec::new();
        if let Err(error) = peer.read_to_end(&mut bytes) {
            assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
        }
        assert!(bytes.is_empty(), "retired connection sent another request");
    }
    fn offer(runtime: &str) -> Offer {
        Offer {
            runtime: runtime.into(),
            revision: 1,
            origin: Origin::Empty,
            formats: vec![],
        }
    }
    fn lease() -> ProviderLease {
        ProviderLease {
            runtime: "runtime".into(),
            worker: "worker".into(),
            epoch: 1,
        }
    }

    #[test]
    fn a_delayed_publication_ack_cannot_acknowledge_another_mutation() {
        let (connection, mut peer) = connected_pair();
        let publisher = NativePublisher {
            connection: Mutex::new(connection),
        };
        let (release, delayed) = mpsc::sync_channel(1);
        let (sent, acknowledged) = mpsc::sync_channel(1);
        let task = std::thread::spawn(move || {
            assert!(matches!(
                wire::read::<Request>(&mut peer).unwrap(),
                Request::NativePublish { revision: 1, .. }
            ));
            delayed.recv_timeout(Duration::from_secs(2)).unwrap();
            // This result belongs to the timed-out first mutation. A retired
            // socket may already refuse it; neither case permits another send.
            let _ = wire::write(&mut peer, &Response::Published(true));
            sent.send(()).unwrap();
            no_more_requests(&mut peer);
        });
        let request = Request::NativePublish {
            lease: lease(),
            expected_runtime: "runtime".into(),
            expected_revision: 1,
            revision: 1,
            formats: vec!["text/plain".into()],
        };
        let error = publisher
            .connection
            .lock()
            .unwrap()
            .call_until(&request, Instant::now() + Duration::from_millis(100))
            .unwrap_err();
        assert!(matches!(
            error.kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        ));
        release.send(()).unwrap();
        acknowledged.recv_timeout(Duration::from_secs(2)).unwrap();
        let error = publisher
            .publish(&lease(), &offer("runtime"), 2, vec!["text/plain".into()])
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotConnected);
        task.join().unwrap();
    }

    #[test]
    fn malformed_or_mismatched_replies_retire_the_connection() {
        let mut wrong_worker = lease();
        wrong_worker.worker = "other-worker".into();
        let copy_offer = ExportOffer {
            offer: Offer {
                origin: Origin::Native {
                    worker: "worker".into(),
                    revision: 1,
                },
                formats: vec!["text/plain".into()],
                ..offer("runtime")
            },
            route_revision: 1,
        };
        let mut wrong_copy = copy_offer.clone();
        wrong_copy.route_revision = 2;
        let cases = [
            (Request::NativePoll, Response::Ready.encode().unwrap()),
            (
                Request::Offer,
                Response::Offer(offer("other-runtime")).encode().unwrap(),
            ),
            (
                Request::NativePoll,
                Response::Provider(ProviderPoll {
                    lease: Some(wrong_worker),
                    offer: offer("runtime"),
                    requests: vec![],
                })
                .encode()
                .unwrap(),
            ),
            (
                Request::CommitCopy(copy_offer),
                Response::Copy(Some(CopyLease {
                    id: uuid::Uuid::new_v4().to_string(),
                    offer: wrong_copy,
                }))
                .encode()
                .unwrap(),
            ),
            (
                read("text/plain"),
                Response::Chunk(Chunk {
                    bytes: vec![1, 2],
                    eof: true,
                })
                .encode()
                .unwrap(),
            ),
            (Request::Enabled(true), vec![2, 10, 0]), // An invalid announcement.
        ];
        for (request, response) in cases {
            let (mut connection, mut peer) = connected_pair();
            let expected = request.clone();
            let task = std::thread::spawn(move || {
                assert_eq!(wire::read::<Request>(&mut peer).unwrap(), expected);
                peer.write_all(&response).unwrap();
                no_more_requests(&mut peer);
            });
            let error = connection.call(&request).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert_eq!(
                connection
                    .call(&Request::Enabled(false))
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::NotConnected
            );
            task.join().unwrap();
        }
    }

    #[test]
    fn a_truncated_reply_preserves_the_first_failure_and_retires_the_connection() {
        let (mut connection, mut peer) = connected_pair();
        let task = std::thread::spawn(move || {
            assert_eq!(
                wire::read::<Request>(&mut peer).unwrap(),
                Request::Enabled(true)
            );
            peer.write_all(&[2, 18]).unwrap();
            peer.shutdown(std::net::Shutdown::Write).unwrap();
            no_more_requests(&mut peer);
        });
        let error = connection.call(&Request::Enabled(true)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(
            connection
                .call(&Request::Enabled(false))
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotConnected
        );
        task.join().unwrap();
    }

    #[test]
    fn a_failed_write_preserves_the_os_error_and_retires_the_connection() {
        let (mut connection, peer) = connected_pair();
        drop(peer);
        let error = connection.call(&Request::Enabled(true)).unwrap_err();
        assert!(matches!(
            error.kind(),
            io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
        ));
        assert!(error.raw_os_error().is_some());
        assert_eq!(
            connection
                .call(&Request::Enabled(false))
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotConnected
        );
    }

    #[test]
    fn a_complete_remote_error_keeps_its_type_and_the_connection_usable() {
        let (mut connection, mut peer) = connected_pair();
        let failure = newport_protocol::common::Error {
            code: "operation".into(),
            message: "Permission revoked".into(),
            retry: newport_protocol::common::Retry::No,
        };
        let remote_failure = failure.clone();
        let task = std::thread::spawn(move || {
            assert_eq!(
                wire::read::<Request>(&mut peer).unwrap(),
                Request::Enabled(true)
            );
            wire::write(&mut peer, &Response::Error(remote_failure)).unwrap();
            assert_eq!(
                wire::read::<Request>(&mut peer).unwrap(),
                Request::Enabled(false)
            );
            wire::write(&mut peer, &Response::Ready).unwrap();
        });
        let error = connection.call(&Request::Enabled(true)).unwrap_err();
        assert_eq!(
            error
                .get_ref()
                .unwrap()
                .downcast_ref::<newport_protocol::common::Error>(),
            Some(&failure)
        );
        assert_eq!(
            connection.call(&Request::Enabled(false)).unwrap(),
            Response::Ready
        );
        task.join().unwrap();
    }

    #[test]
    fn an_expired_outer_payload_deadline_sends_no_next_request() {
        let (connection, mut peer) = connected_pair();
        let mut read = Read {
            connection: Some(connection),
            runtime: "runtime".into(),
            revision: 1,
            format: "text/plain".into(),
            offset: 0,
            done: false,
            export: None,
            deadline: Some(Instant::now()),
        };
        assert_eq!(read.next(1).unwrap_err().kind(), io::ErrorKind::TimedOut);
        no_more_requests(&mut peer);
    }
    #[test]
    fn a_metadata_deadline_includes_authentication_and_its_response() {
        use std::os::unix::{fs::PermissionsExt, net::UnixListener};
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let access = Access {
            path: root.path().join("clipboard.sock"),
            runtime: "runtime".into(),
            worker: "worker".into(),
            token: uuid::Uuid::new_v4().to_string(),
        };
        let listener = UnixListener::bind(&access.path).unwrap();
        let task = std::thread::spawn(move || {
            let (mut peer, _) = listener.accept().unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            wire::write(
                &mut peer,
                &Response::Hello(service::Hello {
                    version: service::VERSION,
                    runtime: "runtime".into(),
                }),
            )
            .unwrap();
            assert!(matches!(
                wire::read::<Request>(&mut peer).unwrap(),
                Request::Authenticate(_)
            ));
            std::thread::sleep(Duration::from_millis(600));
            wire::write(&mut peer, &Response::Ready).unwrap();
            assert_eq!(wire::read::<Request>(&mut peer).unwrap(), Request::Offer);
            std::thread::sleep(Duration::from_millis(600));
            let _ = wire::write(&mut peer, &Response::Offer(offer("runtime")));
            no_more_requests(&mut peer);
        });
        let reader = Reader::new(access);
        let error = reader
            .offer_until(
                Instant::now() + Duration::from_secs(1),
                &ReadCancellation::default(),
            )
            .unwrap_err();
        assert!(matches!(
            error.kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        ));
        task.join().unwrap();
    }
    fn read(format: &str) -> Request {
        Request::Read {
            runtime: "runtime".into(),
            revision: 1,
            format: format.into(),
            offset: 0,
            limit: 1,
            export: None,
        }
    }
    #[test]
    fn only_a_files_read_outlasts_the_ordinary_deadlines() {
        assert_eq!(
            response_timeout(&read(newport_protocol::clipboard::FILES_FORMAT)),
            Duration::from_secs(30 * 60)
        );
        assert_eq!(
            response_timeout(&read("text/plain")),
            Duration::from_secs(17)
        );
        assert_eq!(
            response_timeout(&read("application/x-newport-files-other")),
            Duration::from_secs(17)
        );
        assert_eq!(response_timeout(&Request::Offer), Duration::from_secs(3));
        assert_eq!(response_timeout(&Request::Poll), Duration::from_secs(3));
    }
}
