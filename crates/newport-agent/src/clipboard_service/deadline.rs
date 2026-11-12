use std::{
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

pub(super) fn remaining(until: Instant) -> io::Result<Duration> {
    let left = until.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Clipboard IPC deadline expired",
        ))
    } else {
        Ok(left)
    }
}
pub(super) struct Deadline<'a> {
    stream: &'a mut UnixStream,
    until: Instant,
}
impl<'a> Deadline<'a> {
    pub fn new(stream: &'a mut UnixStream, timeout: Duration) -> Self {
        Self::until(stream, Instant::now() + timeout)
    }
    pub fn until(stream: &'a mut UnixStream, until: Instant) -> Self {
        Self { stream, until }
    }
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
