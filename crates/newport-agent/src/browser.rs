//! Browser commands and the agent-side request broker.
use newport_protocol::{
    browser::{self, Reply, Request, ServerMessage as LocalMessage, wire},
    common::{Error as RemoteError, Retry},
    integration::{self, BrowserReply, BrowserRequest, ServerMessage},
};
use std::{
    fmt,
    io::{self, Read, Write},
    net::Shutdown,
    os::unix::net::{UnixListener, UnixStream},
    process::Command,
    time::{Duration, Instant},
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(25);
const LOCAL_TIMEOUT: Duration = Duration::from_millis(100);
const PENDING_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug)]
enum OpenError {
    Connect(io::Error),
    Owner(wire::Error),
}
impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(error) => write!(
                f,
                "Turn on Browser links on Newport's Integrations page first: {error}"
            ),
            Self::Owner(error) => {
                let reason = match error {
                    wire::Error::Frame(newport_protocol::encoding::protobuf::FrameError::Io(_)) => {
                        "The browser owner did not respond"
                    }
                    wire::Error::Ended
                    | wire::Error::Frame(
                        newport_protocol::encoding::protobuf::FrameError::Truncated,
                    ) => "The browser owner disconnected before announcing its protocol",
                    _ => "The browser owner has an incompatible protocol",
                };
                write!(f, "{reason}. Update Needle and reconnect Newport: {error}")
            }
        }
    }
}
impl std::error::Error for OpenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(match self {
            Self::Connect(error) => error,
            Self::Owner(error) => error,
        })
    }
}

pub fn open(args: &[String], desktop: bool) -> io::Result<()> {
    if args.len() != 1 {
        return Err(io::Error::other("usage: newport-agent open URL"));
    }
    let selection = crate::routing::select_for("browser");
    let graphical = crate::native::real_desktop(selection.as_ref().ok().map(|s| s.root.as_path()));
    if desktop
        && graphical
        && let Some(native) = crate::native::find("xdg-open")
    {
        use std::os::unix::process::CommandExt;
        return Err(Command::new(native).args(args).env_remove("BROWSER").exec());
    }
    let request = Request {
        url: args[0].clone(),
    };
    request
        .validate()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let root = selection?.root;
    let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
    socket
        .connect_timeout(
            &socket2::SockAddr::unix(root.join("agent.sock"))?,
            Duration::from_secs(2),
        )
        .map_err(|error| io::Error::new(error.kind(), OpenError::Connect(error)))?;
    let fd: std::os::fd::OwnedFd = socket.into();
    let mut stream = UnixStream::from(fd);
    stream.set_nonblocking(false)?;
    if let Some(warning) = exchange(&mut stream, &request, REQUEST_TIMEOUT)? {
        eprintln!("Newport: {warning}");
    }
    Ok(())
}

fn exchange(
    stream: &mut UnixStream,
    request: &Request,
    timeout: Duration,
) -> io::Result<Option<String>> {
    let mut io = Deadline::new(stream, timeout);
    let announced = wire::read_server(&mut io).and_then(|message| match message {
        LocalMessage::Hello { .. } => Ok(()),
        _ => Err(browser::Invalid("owner announcement").into()),
    });
    announced.map_err(|error| {
        let kind = match &error {
            wire::Error::Frame(newport_protocol::encoding::protobuf::FrameError::Io(error)) => {
                error.kind()
            }
            wire::Error::Ended
            | wire::Error::Frame(newport_protocol::encoding::protobuf::FrameError::Truncated) => {
                io::ErrorKind::UnexpectedEof
            }
            _ => io::ErrorKind::InvalidData,
        };
        io::Error::new(kind, OpenError::Owner(error))
    })?;
    wire::write_request(&mut io, request)?;
    io.stream.shutdown(Shutdown::Write)?;
    match wire::read_server(&mut io)? {
        LocalMessage::Reply(Reply::Opened { warning }) => Ok(warning),
        LocalMessage::Reply(Reply::Failed(error)) => Err(io::Error::other(error)),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            browser::Invalid("response operation"),
        )),
    }
}

/// Every read or write uses the remaining budget, including fragmented frames.
struct Deadline<'a> {
    stream: &'a mut UnixStream,
    end: Instant,
}
impl<'a> Deadline<'a> {
    fn new(stream: &'a mut UnixStream, timeout: Duration) -> Self {
        Self {
            stream,
            end: Instant::now() + timeout,
        }
    }
    fn remaining(&self) -> io::Result<Duration> {
        self.end
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "Browser request timed out"))
    }
}
impl Read for Deadline<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        crate::local_socket::read_timeout(self.stream, self.remaining()?)?;
        self.stream.read(bytes)
    }
}
impl Write for Deadline<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        crate::local_socket::write_timeout(
            self.stream,
            self.remaining()?.min(Duration::from_secs(2)),
        )?;
        self.stream.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

/// Owns one pending local request without blocking clipboard processing.
pub(crate) struct Broker {
    listener: UnixListener,
    enabled: bool,
    last_open: Instant,
    request_id: u64,
    pending: Option<(u64, UnixStream, Instant)>,
}
impl Broker {
    pub(crate) fn new(listener: UnixListener, enabled: bool) -> Self {
        Self {
            listener,
            enabled,
            last_open: Instant::now() - Duration::from_secs(2),
            request_id: 0,
            pending: None,
        }
    }
    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }
    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled && let Some((_, mut stream, _)) = self.pending.take() {
            let _ = local_reply(
                &mut stream,
                failure(
                    "permission_denied",
                    "Browser access was disabled.",
                    Retry::No,
                ),
            );
        }
    }
    pub(crate) fn expire(&mut self) {
        if self
            .pending
            .as_ref()
            .is_some_and(|(_, _, end)| Instant::now() >= *end)
            && let Some((_, mut stream, _)) = self.pending.take()
        {
            let _ = local_reply(
                &mut stream,
                failure(
                    "timed_out",
                    "The browser opening result could not be confirmed.",
                    Retry::Unknown,
                ),
            );
        }
    }
    pub(crate) fn reply(&mut self, reply: BrowserReply) {
        if self
            .pending
            .as_ref()
            .is_some_and(|(expected, _, _)| reply.id == *expected)
            && let Some((_, mut stream, _)) = self.pending.take()
        {
            let reply = if reply.success {
                Reply::Opened {
                    warning: reply.message,
                }
            } else {
                failure(
                    "open_failed",
                    reply.message.as_deref().unwrap_or("Browser request failed"),
                    Retry::Unknown,
                )
            };
            let _ = local_reply(&mut stream, reply);
        }
    }
    pub(crate) fn poll(&mut self, output: &mut impl Write) -> io::Result<()> {
        let mut stream = match self.listener.accept() {
            Ok((stream, _)) => stream,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) => return Err(error),
        };
        // Read one request and its half-close before forwarding any URL.
        let request = (|| {
            // Accepted sockets can inherit the listener's nonblocking mode.
            stream
                .set_nonblocking(false)
                .map_err(newport_protocol::encoding::protobuf::FrameError::Io)?;
            let mut io = Deadline::new(&mut stream, LOCAL_TIMEOUT);
            wire::write_server(
                &mut io,
                &LocalMessage::Hello {
                    version: browser::VERSION,
                },
            )?;
            let request = wire::read_request(&mut io)?;
            let mut trailing = [0];
            if io
                .read(&mut trailing)
                .map_err(newport_protocol::encoding::protobuf::FrameError::Io)?
                != 0
            {
                return Err(browser::Invalid("trailing request data").into());
            }
            Ok::<_, wire::Error>(request)
        })();
        let rejection = if request.is_err() {
            Some(failure(
                "invalid_request",
                "Invalid browser request. Update Needle and reconnect Newport.",
                Retry::No,
            ))
        } else if !self.enabled {
            Some(failure(
                "permission_denied",
                "Browser access is disabled.",
                Retry::No,
            ))
        } else if self.pending.is_some() || self.last_open.elapsed() < Duration::from_secs(1) {
            Some(failure(
                "busy",
                "Another browser request is pending; try again",
                Retry::Yes,
            ))
        } else if self.request_id == u64::MAX {
            Some(failure(
                "exhausted",
                "Browser request IDs are exhausted; reconnect Newport",
                Retry::No,
            ))
        } else {
            None
        };
        if let Some(reply) = rejection {
            let _ = local_reply(&mut stream, reply);
            return Ok(());
        }
        let request = request.map_err(io::Error::from)?;
        let id = self
            .request_id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("Browser request IDs exhausted"))?;
        integration::wire::write(
            output,
            &ServerMessage::Browser(BrowserRequest {
                id,
                url: request.url,
            }),
        )?;
        self.request_id = id;
        self.last_open = Instant::now();
        self.pending = Some((id, stream, Instant::now() + PENDING_TIMEOUT));
        Ok(())
    }
}

fn failure(code: &str, message: &str, retry: Retry) -> Reply {
    Reply::Failed(RemoteError {
        code: code.into(),
        message: message.into(),
        retry,
    })
}
fn local_reply(stream: &mut UnixStream, reply: Reply) -> io::Result<()> {
    wire::write_server(
        &mut Deadline::new(stream, LOCAL_TIMEOUT),
        &LocalMessage::Reply(reply),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
