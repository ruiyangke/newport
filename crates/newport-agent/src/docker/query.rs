use super::{MAX_BODY, MAX_HEAD, Response, complete, find, negotiate, parse_response};
use serde::de::DeserializeOwned;
use serde_json::Value;
use socket2::{Domain, SockAddr, Socket, Type};
use std::{
    fmt,
    io::{self, Read, Write},
    os::fd::{AsRawFd, OwnedFd, RawFd},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub(crate) struct Query<'a> {
    deadline: Instant,
    body_limit: usize,
    cancelled: &'a dyn Fn() -> bool,
}

impl<'a> Query<'a> {
    pub fn new(deadline: Instant, body_limit: usize, cancelled: &'a dyn Fn() -> bool) -> Self {
        Self {
            deadline,
            body_limit: body_limit.min(MAX_BODY),
            cancelled,
        }
    }

    fn remaining(&self) -> io::Result<Duration> {
        if (self.cancelled)() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Docker query cancelled",
            ));
        }
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "Docker query timed out"))
    }

    pub fn check(&self) -> Result<(), QueryError> {
        self.remaining().map(|_| ()).map_err(QueryError::Io)
    }

    fn wait(&self, fd: RawFd, events: i16) -> io::Result<()> {
        loop {
            let left = self.remaining()?.min(Duration::from_millis(25));
            let timeout = i32::try_from(left.as_millis()).unwrap_or(25).max(1);
            let mut poll = libc::pollfd {
                fd,
                events,
                revents: 0,
            };
            // SAFETY: poll points to one initialized entry for the duration of the call.
            let result = unsafe { libc::poll(&mut poll, 1, timeout) };
            if result > 0 {
                return Ok(());
            }
            if result < 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
    }

    fn connect(&self, path: &Path) -> io::Result<UnixStream> {
        let address = SockAddr::unix(path)?;
        loop {
            self.remaining()?;
            let socket = Socket::new(Domain::UNIX, Type::STREAM, None)?;
            socket.set_nonblocking(true)?;
            match socket.connect(&address) {
                Ok(()) => return Ok(UnixStream::from(OwnedFd::from(socket))),
                Err(error) if error.raw_os_error() == Some(libc::EINPROGRESS) => {
                    self.wait(socket.as_raw_fd(), libc::POLLOUT)?;
                    if let Some(error) = socket.take_error()? {
                        return Err(error);
                    }
                    return Ok(UnixStream::from(OwnedFd::from(socket)));
                }
                // A full Unix listener backlog cannot accept a connection yet.
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(self.remaining()?.min(Duration::from_millis(5)));
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }

    fn write(&self, stream: &mut UnixStream, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            self.remaining()?;
            match stream.write(bytes) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "Docker request closed",
                    ));
                }
                Ok(count) => bytes = &bytes[count..],
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    self.wait(stream.as_raw_fd(), libc::POLLOUT)?
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn check_size(&self, raw: &[u8]) -> io::Result<()> {
        let oversized = raw.len() > self.body_limit + MAX_HEAD
            || match find(raw, b"\r\n\r\n") {
                Some(end) => {
                    end > MAX_HEAD
                        || String::from_utf8_lossy(&raw[..end])
                            .split("\r\n")
                            .filter_map(|line| line.split_once(':'))
                            .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                            .any(|(_, value)| {
                                value.trim().parse::<u64>().is_ok_and(|length| {
                                    usize::try_from(length)
                                        .map_or(true, |length| length > self.body_limit)
                                })
                            })
                }
                None => raw.len() > MAX_HEAD,
            };
        if oversized {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "Docker response exceeds its byte limit",
            ));
        }
        Ok(())
    }

    pub fn request(&self, socket: &Path, method: &str, target: &str) -> io::Result<Response> {
        let mut stream = self.connect(socket)?;
        let request = format!(
            "{method} {target} HTTP/1.1\r\nHost: docker\r\nUser-Agent: newport-agent\r\nAccept: application/json\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
        );
        self.write(&mut stream, request.as_bytes())?;
        let mut raw = Vec::new();
        let mut buffer = vec![0; 64 << 10];
        loop {
            self.remaining()?;
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => raw.extend_from_slice(&buffer[..count]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    self.wait(stream.as_raw_fd(), libc::POLLIN)?;
                    continue;
                }
                Err(error) => return Err(error),
            }
            self.check_size(&raw)?;
            if complete(&raw) {
                break;
            }
        }
        let response = parse_response(&raw)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if response.body.len() > self.body_limit {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "Docker response exceeds its byte limit",
            ));
        }
        Ok(response)
    }

    pub fn get<T: DeserializeOwned>(&self, socket: &Path, target: &str) -> Result<T, QueryError> {
        let response = self
            .request(socket, "GET", target)
            .map_err(QueryError::Io)?;
        if !response.success() {
            return Err(QueryError::Http(response.status));
        }
        let value = serde_json::from_slice(&response.body).map_err(QueryError::Json)?;
        self.check()?;
        Ok(value)
    }
}

#[derive(Debug)]
pub(crate) enum QueryError {
    Io(io::Error),
    Json(serde_json::Error),
    Http(u16),
    Api(String),
    #[cfg(any(target_os = "linux", test))]
    Missing,
}

#[cfg(any(target_os = "linux", test))]
impl QueryError {
    pub fn kind(&self) -> io::ErrorKind {
        match self {
            Self::Io(error) => error.kind(),
            Self::Json(_) => io::ErrorKind::InvalidData,
            Self::Http(401 | 403) => io::ErrorKind::PermissionDenied,
            Self::Http(_) => io::ErrorKind::Other,
            Self::Api(_) => io::ErrorKind::Unsupported,
            Self::Missing => io::ErrorKind::NotFound,
        }
    }
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "Docker query failed: {error}"),
            Self::Json(_) => f.write_str("Docker returned an invalid JSON response"),
            Self::Http(status) => write!(f, "Docker returned HTTP {status}"),
            Self::Api(message) => f.write_str(message),
            #[cfg(any(target_os = "linux", test))]
            Self::Missing => f.write_str(super::NOT_RUNNING),
        }
    }
}

impl std::error::Error for QueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

pub(crate) struct Endpoint {
    socket: PathBuf,
    pub prefix: String,
    pub api_version: String,
    pub version: Value,
}

impl Endpoint {
    pub fn is_podman(&self) -> bool {
        self.version["Components"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|component| {
                component["Name"]
                    .as_str()
                    .is_some_and(|name| name.contains("Podman"))
            })
            || self.version["Platform"]["Name"]
                .as_str()
                .is_some_and(|name| name.contains("Podman"))
    }

    #[cfg(any(target_os = "linux", test))]
    pub fn discover(candidates: &[PathBuf], query: &Query<'_>) -> Result<Self, QueryError> {
        let mut first_error = None;
        for socket in candidates {
            query.remaining().map_err(QueryError::Io)?;
            match Self::connect(socket, query) {
                Ok(endpoint) => return Ok(endpoint),
                Err(QueryError::Io(error))
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                    ) => {}
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        Err(first_error.unwrap_or(QueryError::Missing))
    }

    pub fn connect(socket: &Path, query: &Query<'_>) -> Result<Self, QueryError> {
        let version: Value = query.get(socket, "/version")?;
        let api = negotiate(
            version["ApiVersion"].as_str().unwrap_or_default(),
            version["MinAPIVersion"].as_str(),
        )
        .map_err(QueryError::Api)?;
        let api_version = format!("{}.{}", api.0, api.1);
        Ok(Self {
            socket: socket.to_owned(),
            prefix: format!("/v{api_version}"),
            api_version,
            version,
        })
    }

    pub fn get<T: DeserializeOwned>(&self, path: &str, query: &Query<'_>) -> Result<T, QueryError> {
        query.get(&self.socket, &format!("{}{path}", self.prefix))
    }
}

#[cfg(test)]
mod tests;
