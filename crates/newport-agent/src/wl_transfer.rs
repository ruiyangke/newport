//! Bounded, cancellable Wayland pipe transfers.
use std::{
    fs::File,
    io::{self, Read, Write},
    os::fd::{AsFd, AsRawFd, OwnedFd},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

/// Never let an unread pipe stall the Wayland event loop or shutdown.
pub(crate) fn write_pipe(fd: OwnedFd, bytes: &[u8], stop: &AtomicBool) -> io::Result<()> {
    let raw = fd.as_raw_fd();
    // Client-side pipes can arrive blocking.
    crate::transport::set_nonblocking(fd.as_fd())?;
    let mut file = File::from(fd);
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut remaining = bytes;
    while !remaining.is_empty() && !stop.load(Ordering::Relaxed) {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "clipboard reader stalled",
            ));
        }
        match file.write(remaining) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => remaining = &remaining[n..],
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                let mut poll = libc::pollfd {
                    fd: raw,
                    events: libc::POLLOUT,
                    revents: 0,
                };
                // SAFETY: one initialized pollfd; `file` owns `raw` and keeps it open.
                unsafe {
                    libc::poll(&mut poll, 1, 50);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => (),
            Err(e) => return Err(e),
        }
    }
    if !remaining.is_empty() {
        return Err(io::ErrorKind::Interrupted.into());
    }
    Ok(())
}

/// Reads what a client writes to `reader` until it closes it, taking at most
/// `limit` bytes and giving up on a writer that stalls.
pub(crate) fn read_pipe(
    reader: impl Into<OwnedFd>,
    limit: usize,
    stop: &AtomicBool,
) -> io::Result<Vec<u8>> {
    let fd: OwnedFd = reader.into();
    let raw = fd.as_raw_fd();
    crate::transport::set_nonblocking(fd.as_fd())?;
    let mut file = File::from(fd);
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut bytes = Vec::new();
    let mut buffer = [0; 64 * 1024];
    while !stop.load(Ordering::Relaxed) {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "clipboard writer stalled",
            ));
        }
        match file.read(&mut buffer) {
            Ok(0) => return Ok(bytes),
            Ok(n) if bytes.len() + n > limit => {
                return Err(io::Error::other("clipboard copy too large"));
            }
            Ok(n) => bytes.extend_from_slice(&buffer[..n]),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                let mut poll = libc::pollfd {
                    fd: raw,
                    events: libc::POLLIN,
                    revents: 0,
                };
                // SAFETY: one initialized pollfd; `file` owns `raw` and keeps it open.
                unsafe {
                    libc::poll(&mut poll, 1, 50);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => (),
            Err(e) => return Err(e),
        }
    }
    Err(io::ErrorKind::Interrupted.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    #[test]
    fn a_copy_is_read_whole_within_its_limit() {
        let (reader, mut writer) = std::io::pipe().unwrap();
        let stop = AtomicBool::new(false);
        writer.write_all(b"copied").unwrap();
        drop(writer);
        assert_eq!(read_pipe(reader, 6, &stop).unwrap(), b"copied");
        let (reader, mut writer) = std::io::pipe().unwrap();
        writer.write_all(b"copied").unwrap();
        drop(writer);
        assert!(read_pipe(reader, 5, &stop).is_err());
    }

    #[test]
    fn canceled_transfer_is_not_reported_as_complete() {
        let (writer, _reader) = UnixStream::pair().unwrap();
        let result = write_pipe(writer.into(), b"image", &AtomicBool::new(true));
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
    }
}
