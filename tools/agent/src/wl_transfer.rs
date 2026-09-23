//! Bounded, cancellable Wayland pipe transfers.
use std::{
    fs::File,
    io::{self, Write},
    os::fd::{AsRawFd, OwnedFd},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

/// Never let an unread pipe stall the Wayland event loop or shutdown.
pub(crate) fn write_pipe(fd: OwnedFd, bytes: &[u8], stop: &AtomicBool) -> io::Result<()> {
    let raw = fd.as_raw_fd();
    // Preserve flags on the received descriptor. Client-side pipes can arrive blocking.
    let flags = unsafe { libc::fcntl(raw, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
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
                unsafe {
                    libc::poll(&mut poll, 1, 50);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => (),
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
