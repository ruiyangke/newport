use super::ReadCancellation;
use super::protocol::{
    self, Access, Chunk, CopyLease, Deadline, ExportOffer, ExportRead, NativeRequest, Offer,
    ProviderLease, ProviderPoll, Request, Response,
};
use std::{
    io,
    os::unix::{
        fs::{FileTypeExt, MetadataExt},
        net::UnixStream,
    },
    sync::{Arc, Mutex},
    time::Duration,
};

struct Connection {
    stream: UnixStream,
    _cancellation: Option<Arc<UnixStream>>,
}
impl Connection {
    fn connect(access: &Access) -> io::Result<Self> {
        Self::connect_with_cancellation(access, None)
    }
    fn connect_with_cancellation(
        access: &Access,
        cancellation: Option<&ReadCancellation>,
    ) -> io::Result<Self> {
        if let Some(cancellation) = cancellation {
            cancellation.begin()?;
        }
        let parent = std::fs::symlink_metadata(
            access
                .path
                .parent()
                .ok_or_else(|| io::Error::other("Invalid clipboard endpoint"))?,
        )?;
        let socket = std::fs::symlink_metadata(&access.path)?;
        if !access.path.is_absolute()
            || !parent.is_dir()
            || parent.uid() != unsafe { libc::geteuid() }
            || parent.mode() & 0o077 != 0
            || !socket.file_type().is_socket()
            || socket.uid() != parent.uid()
        {
            return Err(io::Error::other(
                "Clipboard endpoint is not private to this account",
            ));
        }
        let stream = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
        stream.connect_timeout(
            &socket2::SockAddr::unix(&access.path)?,
            Duration::from_secs(2),
        )?;
        let fd: std::os::fd::OwnedFd = stream.into();
        let stream = UnixStream::from(fd);
        stream.set_nonblocking(false)?;
        let control = cancellation
            .map(|cancellation| cancellation.attach(&stream))
            .transpose()?;
        let mut connection = Self {
            stream,
            _cancellation: control,
        };
        ready(connection.call(&Request::Authenticate(access.clone()))?)?;
        Ok(connection)
    }
    fn call(&mut self, request: &Request) -> io::Result<Response> {
        protocol::write(
            &mut Deadline::new(&mut self.stream, Duration::from_secs(2)),
            request,
        )?;
        let response = protocol::read(&mut Deadline::new(
            &mut self.stream,
            response_timeout(request),
        ))?;
        match response {
            Response::Error(error) => Err(io::Error::other(error)),
            response => Ok(response),
        }
    }
}
/// Only a files read may outlast the broker's silence limit: the broker
/// keeps it alive with the desktop's keep-alives and fails it when they stop.
fn response_timeout(request: &Request) -> Duration {
    match request {
        Request::Read { format, .. }
            if format == crate::runtime_protocol::clipboard::FILES_FORMAT =>
        {
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
    pub(crate) fn new(access: Access) -> Self {
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
        let mut slot = self.metadata.lock().unwrap();
        let mut response = None;
        // Metadata is safe to retry after an idle socket expires. Never replay
        // producer writes or range reads after an ambiguous transport failure.
        for _ in 0..2 {
            if slot.is_none() {
                *slot = Some(Connection::connect(&self.access)?);
            }
            match slot.as_mut().unwrap().call(&request) {
                Ok(value) => {
                    response = Some(value);
                    break;
                }
                Err(error) => {
                    *slot = None;
                    response = Some(Response::Error(error.to_string()));
                }
            }
        }
        match response {
            Some(Response::Error(error)) => Err(io::Error::other(error)),
            Some(response) => Ok(response),
            _ => Err(io::Error::other("Invalid clipboard metadata response")),
        }
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
        if offer.runtime != self.access.runtime
            || !offer.formats.iter().any(|name| name == format)
            || !crate::clipboard_wire::valid_format(format)
        {
            return Err(io::Error::other(
                "Clipboard offer belongs to another runtime or format",
            ));
        }
        Ok(Read {
            connection: Some(Connection::connect_with_cancellation(
                &self.access,
                Some(&cancellation),
            )?),
            runtime: offer.runtime.clone(),
            revision: offer.revision,
            format: format.into(),
            offset: 0,
            done: false,
            export,
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
            let chunk = read.next(crate::clipboard_wire::CHUNK)?;
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
}
impl Read {
    pub fn next(&mut self, limit: usize) -> io::Result<Chunk> {
        if self.done {
            return Ok(Chunk {
                bytes: vec![],
                eof: true,
            });
        }
        if limit == 0 || limit > crate::clipboard_wire::CHUNK {
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
        let response = self
            .connection
            .as_mut()
            .ok_or_else(|| io::Error::other("Clipboard read already failed"))?
            .call(&request);
        match response {
            Ok(Response::Chunk(chunk))
                if chunk.bytes.len() <= limit
                    && self.offset + chunk.bytes.len() <= crate::clipboard_wire::LIMIT
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
    pub(crate) fn new(access: Access) -> io::Result<Self> {
        Ok(Self {
            connection: Mutex::new(Connection::connect(&access)?),
        })
    }
    /// Metadata and demand requests only. An unassigned worker gets no lease or
    /// requests and cannot publish. No host export permission is implied.
    pub fn poll(&self) -> io::Result<ProviderPoll> {
        match self.connection.lock().unwrap().call(&Request::NativePoll)? {
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
        if bytes.len() > crate::clipboard_wire::CHUNK || (bytes.is_empty() && !eof) {
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
    pub(crate) fn update(&self, offer: crate::clipboard_wire::Offer) -> io::Result<()> {
        ready(self.connection.lock().unwrap().call(&Request::Publish {
            revision: offer.revision,
            formats: offer.formats,
        })?)
    }
    pub(crate) fn reply(&self, bytes: &[u8]) -> io::Result<()> {
        ready(
            self.connection
                .lock()
                .unwrap()
                .call(&Request::Reply(bytes.into()))?,
        )
    }
    pub(crate) fn poll(&self, output: &mut impl io::Write) -> io::Result<()> {
        match self.connection.lock().unwrap().call(&Request::Poll)? {
            Response::Requests(requests) if requests.len() <= 8 => {
                for (id, revision, format) in requests {
                    crate::wire::write(
                        output,
                        b'C',
                        &crate::clipboard_wire::Request {
                            id,
                            revision,
                            format,
                        }
                        .encode(),
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
            response_timeout(&read(crate::runtime_protocol::clipboard::FILES_FORMAT)),
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
