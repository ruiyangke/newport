//! Persistent protobuf framing over OS-authenticated desktop byte streams.
use super::Stream;
use newport_protocol::encoding::protobuf::{EncodedFrame, FrameDecoder, FrameError};
#[cfg(windows)]
use std::io::Read;
use std::{
    fmt,
    io::{self, Write},
    net::Shutdown,
    time::{Duration, Instant},
};
const READ_SIZE: usize = 32 * 1024;

/// Partial framing and read-ahead move together when a negotiated stream changes codecs.
#[derive(Default)]
pub struct ReadBuffer {
    decoder: Option<FrameDecoder>,
    limit: usize,
    bytes: Vec<u8>,
    offset: usize,
    arrived: Option<Instant>,
    frame_started: Option<Instant>,
    frame_deadline: Option<Instant>,
    ended: bool,
    retired: bool,
}
impl fmt::Debug for ReadBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadBuffer")
            .field("pending", &(self.bytes.len() - self.offset))
            .field("ended", &self.ended)
            .field("retired", &self.retired)
            .finish_non_exhaustive()
    }
}
pub struct Reader {
    stream: Stream,
    buffer: ReadBuffer,
}
impl Reader {
    pub fn new(stream: Stream) -> Self {
        Self::from_parts(stream, ReadBuffer::default())
    }
    /// Both values must come from the same owned connection's into_parts call.
    pub fn from_parts(stream: Stream, buffer: ReadBuffer) -> Self {
        Self { stream, buffer }
    }
    pub fn into_parts(self) -> (Stream, ReadBuffer) {
        (self.stream, self.buffer)
    }
    pub fn try_clone_stream(&self) -> io::Result<Stream> {
        self.stream.try_clone()
    }
    /// Timeouts retain partial framing. Callers retain the absolute deadline when resuming.
    pub fn next_until(
        &mut self,
        limit: usize,
        deadline: Instant,
    ) -> io::Result<Option<EncodedFrame>> {
        self.next(limit, Wait::Until(deadline))
    }
    /// Idle streams may wait at a frame boundary. Once the first byte arrives,
    /// all reads for that frame share its original absolute completion deadline.
    /// Read-ahead retains the arrival time even if the next caller is delayed.
    pub fn next_idle(
        &mut self,
        limit: usize,
        frame_timeout: Duration,
    ) -> io::Result<Option<EncodedFrame>> {
        if frame_timeout.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Frame timeout must be positive",
            ));
        }
        self.next(limit, Wait::Idle(frame_timeout))
    }
    fn next(&mut self, limit: usize, wait: Wait) -> io::Result<Option<EncodedFrame>> {
        if self.buffer.retired {
            return Err(retired());
        }
        if self.buffer.ended {
            return Ok(None);
        }
        if self.buffer.decoder.is_none() || self.buffer.limit != limit {
            if self
                .buffer
                .decoder
                .as_ref()
                .is_some_and(|value| !value.is_at_boundary())
            {
                return self.fail(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Cannot change a frame limit during a partial frame",
                ));
            }
            self.buffer.decoder = Some(FrameDecoder::new(limit));
            self.buffer.limit = limit;
        }
        loop {
            // The caller's unary/admission budget remains absolute even with buffered data.
            if let Wait::Until(deadline) = wait {
                remaining(deadline)?;
            }
            if self.buffer.frame_started.is_none() && self.buffer.offset < self.buffer.bytes.len() {
                self.buffer.frame_started = self.buffer.arrived;
            }
            if let Wait::Idle(duration) = wait
                && self.buffer.frame_deadline.is_none()
                && let Some(started) = self.buffer.frame_started
            {
                self.buffer.frame_deadline =
                    Some(started.checked_add(duration).ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidInput, "Frame deadline overflow")
                    })?);
            }
            let deadline = wait.deadline(self.buffer.frame_deadline);
            if self.buffer.offset < self.buffer.bytes.len() {
                // A complete buffered frame was already received at `arrived`; UI delay
                // must not make it expire. Late continuation bytes can never complete it.
                if self
                    .buffer
                    .frame_deadline
                    .zip(self.buffer.arrived)
                    .is_some_and(|(deadline, arrived)| arrived > deadline)
                {
                    return self.fail(timeout());
                }
                let Some(decoder) = self.buffer.decoder.as_mut() else {
                    return self.fail(io::Error::other("Missing frame decoder"));
                };
                let mut pending = &self.buffer.bytes[self.buffer.offset..];
                let result = decoder.consume(&mut pending);
                self.buffer.offset = self.buffer.bytes.len() - pending.len();
                match result {
                    Ok(Some(frame)) => {
                        self.buffer.frame_started = None;
                        self.buffer.frame_deadline = None;
                        return Ok(Some(frame));
                    }
                    Ok(None) => {}
                    Err(error) => return self.fail(frame_error(error)),
                }
            }
            if let Some(deadline) = deadline {
                remaining(deadline)?;
            }
            self.buffer.bytes.resize(READ_SIZE, 0);
            self.buffer.offset = 0;
            let result = read_until(&mut self.stream, &mut self.buffer.bytes, deadline);
            let arrived = Instant::now();
            // OS timeout rounding or scheduling must not accept a late continuation.
            if result.is_ok() && deadline.is_some_and(|deadline| arrived > deadline) {
                return self.fail(timeout());
            }
            match result {
                Ok(0) => {
                    self.buffer.bytes.clear();
                    let Some(decoder) = self.buffer.decoder.as_mut() else {
                        return self.fail(io::Error::other("Missing frame decoder"));
                    };
                    if let Err(error) = decoder.finish() {
                        return self.fail(frame_error(error));
                    }
                    self.buffer.ended = true;
                    return Ok(None);
                }
                Ok(length) => {
                    self.buffer.bytes.truncate(length);
                    self.buffer.arrived = Some(arrived);
                    if self.buffer.frame_started.is_none() {
                        self.buffer.frame_started = Some(arrived);
                    }
                }
                Err(error) => {
                    self.buffer.bytes.clear();
                    if error.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) {
                        return Err(io::Error::new(io::ErrorKind::TimedOut, error));
                    }
                    return self.fail(error);
                }
            }
        }
    }

    fn fail<T>(&mut self, error: io::Error) -> io::Result<T> {
        self.buffer.retired = true;
        self.buffer.bytes.clear();
        self.buffer.offset = 0;
        self.buffer.decoder = None;
        self.buffer.frame_started = None;
        self.buffer.frame_deadline = None;
        let _ = self.stream.shutdown(Shutdown::Both);
        Err(error)
    }
}

#[derive(Clone, Copy)]
enum Wait {
    Until(Instant),
    Idle(Duration),
}
impl Wait {
    fn deadline(self, frame_deadline: Option<Instant>) -> Option<Instant> {
        match self {
            Self::Until(deadline) => {
                Some(frame_deadline.map_or(deadline, |frame| frame.min(deadline)))
            }
            Self::Idle(_) => frame_deadline,
        }
    }
}

/// Framed I/O owns the socket's nonblocking mode; clones remain suitable for
/// framed reads, framed writes and shutdown, including after codec transitions.
pub struct Writer {
    stream: Stream,
    retired: bool,
}
impl Writer {
    pub fn new(stream: Stream) -> Self {
        Self {
            stream,
            retired: false,
        }
    }
    pub fn try_clone_stream(&self) -> io::Result<Stream> {
        self.stream.try_clone()
    }
    pub fn is_retired(&self) -> bool {
        self.retired
    }
    /// Input is an already validated complete frame, including its original prefix.
    pub fn send_until(&mut self, mut frame: &[u8], deadline: Instant) -> io::Result<()> {
        if self.retired {
            return Err(retired());
        }
        self.retired = true;
        let result = (|| {
            // Darwin's send-buffer wait does not honor MSG_DONTWAIT. Keep the
            // shared descriptor nonblocking; framed readers already poll it.
            #[cfg(unix)]
            self.stream.set_nonblocking(true)?;
            while !frame.is_empty() {
                remaining(deadline)?;
                let result = write_until(&mut self.stream, frame, deadline);
                remaining(deadline)?;
                match result {
                    Ok(0) => {
                        return Err(io::Error::new(
                            io::ErrorKind::WriteZero,
                            "Desktop stream did not accept the frame",
                        ));
                    }
                    Ok(length) => frame = &frame[length..],
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) => return Err(error),
                }
            }
            self.stream.flush()?;
            remaining(deadline)?;
            Ok(())
        })();
        if result.is_ok() {
            self.retired = false;
        } else {
            let _ = self.stream.shutdown(Shutdown::Both);
        }
        result
    }
}
#[cfg(unix)]
fn read_until(
    stream: &mut Stream,
    bytes: &mut [u8],
    deadline: Option<Instant>,
) -> io::Result<usize> {
    use std::os::fd::AsRawFd;
    loop {
        if let Some(deadline) = deadline {
            remaining(deadline)?;
        }
        // Per-call nonblocking flags leave all cloned socket modes unchanged. In
        // particular, Darwin closed peers are drained without another setsockopt.
        // SAFETY: stream owns the descriptor and bytes provides valid writable storage.
        let count = unsafe {
            libc::recv(
                stream.as_raw_fd(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
                libc::MSG_DONTWAIT,
            )
        };
        if count >= 0 {
            return Ok(count as usize);
        }
        let error = io::Error::last_os_error();
        match error.kind() {
            io::ErrorKind::Interrupted => continue,
            io::ErrorKind::WouldBlock => poll_socket(stream.as_raw_fd(), libc::POLLIN, deadline)?,
            _ => return Err(error),
        }
    }
}
#[cfg(unix)]
fn write_until(stream: &mut Stream, bytes: &[u8], deadline: Instant) -> io::Result<usize> {
    use std::os::fd::AsRawFd;
    loop {
        remaining(deadline)?;
        // Keep SIGPIPE suppression local to this send.
        // SAFETY: stream owns the descriptor and bytes remains valid throughout send.
        let count = unsafe {
            libc::send(
                stream.as_raw_fd(),
                bytes.as_ptr().cast(),
                bytes.len(),
                libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
            )
        };
        if count >= 0 {
            return Ok(count as usize);
        }
        let error = io::Error::last_os_error();
        match error.kind() {
            io::ErrorKind::Interrupted => continue,
            io::ErrorKind::WouldBlock => {
                poll_socket(stream.as_raw_fd(), libc::POLLOUT, Some(deadline))?
            }
            _ => return Err(error),
        }
    }
}
#[cfg(unix)]
fn poll_socket(
    fd: std::os::fd::RawFd,
    events: libc::c_short,
    deadline: Option<Instant>,
) -> io::Result<()> {
    loop {
        let milliseconds = match deadline {
            Some(deadline) => i32::try_from(remaining(deadline)?.as_millis().saturating_add(1))
                .unwrap_or(i32::MAX),
            None => -1,
        };
        let mut descriptor = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        // SAFETY: the caller owns the live descriptor, and poll borrows one valid entry.
        let result = unsafe { libc::poll(&mut descriptor, 1, milliseconds) };
        if result > 0 {
            return Ok(());
        }
        if result == 0 {
            if let Some(deadline) = deadline {
                remaining(deadline)?;
            }
            continue;
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}
#[cfg(windows)]
fn read_until(
    stream: &mut Stream,
    bytes: &mut [u8],
    deadline: Option<Instant>,
) -> io::Result<usize> {
    stream.set_read_timeout(deadline.map(remaining).transpose()?)?;
    stream.read(bytes)
}
#[cfg(windows)]
fn write_until(stream: &mut Stream, bytes: &[u8], deadline: Instant) -> io::Result<usize> {
    stream.set_write_timeout(Some(remaining(deadline)?))?;
    stream.write(bytes)
}
fn timeout() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "Desktop frame deadline elapsed")
}
fn frame_error(error: FrameError) -> io::Error {
    match error {
        FrameError::Io(error) => error,
        FrameError::Truncated => io::Error::new(io::ErrorKind::UnexpectedEof, error),
        _ => io::Error::new(io::ErrorKind::InvalidData, error),
    }
}
fn retired() -> io::Error {
    io::Error::new(
        io::ErrorKind::BrokenPipe,
        "Desktop framed connection is retired",
    )
}
pub(super) fn remaining(deadline: Instant) -> io::Result<std::time::Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|v| !v.is_zero())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "Desktop transport deadline elapsed",
            )
        })
}
#[cfg(test)]
mod tests;
