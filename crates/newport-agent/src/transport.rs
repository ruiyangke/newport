//! Bounded stdio transport and heartbeat activity tracking.
use std::{
    io::{self, Read, Write},
    os::fd::{AsRawFd, BorrowedFd},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
/// Adds `O_NONBLOCK` to the descriptor's status flags, keeping the others.
pub(crate) fn set_nonblocking(descriptor: BorrowedFd<'_>) -> io::Result<()> {
    // SAFETY: fcntl takes scalar arguments on a live borrowed descriptor.
    let flags = unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETFL) };
    // SAFETY: as above.
    if flags < 0
        || unsafe {
            libc::fcntl(
                descriptor.as_raw_fd(),
                libc::F_SETFL,
                flags | libc::O_NONBLOCK,
            )
        } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

// Account for upload progress even before a large frame has completed.
pub(crate) struct TrackedReader<R> {
    pub(crate) input: R,
    pub(crate) activity: Arc<Mutex<Instant>>,
}
impl<R: Read> Read for TrackedReader<R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let count = self.input.read(bytes)?;
        if count > 0 {
            *self
                .activity
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now();
        }
        Ok(count)
    }
}

// The SSH peer may stop draining stdout without closing it. Keep cleanup reachable.
pub(crate) struct DeadlineWriter<W> {
    pub(crate) inner: W,
    pub(crate) timeout: Duration,
    pub(crate) cancellation: Cancellation,
}
impl<W: Write + AsRawFd> Write for DeadlineWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let deadline = Instant::now() + self.timeout;
        loop {
            self.cancellation.check()?;
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "agent output stalled",
                ));
            }
            match self.inner.write(bytes) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    let mut fd = libc::pollfd {
                        fd: self.inner.as_raw_fd(),
                        events: libc::POLLOUT,
                        revents: 0,
                    };
                    let remaining = deadline
                        .saturating_duration_since(Instant::now())
                        .as_millis()
                        .min(50) as i32;
                    // SAFETY: one initialized pollfd for the call.
                    if unsafe { libc::poll(&mut fd, 1, remaining) } < 0 {
                        let error = io::Error::last_os_error();
                        if error.kind() != io::ErrorKind::Interrupted {
                            return Err(error);
                        }
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => (),
                result => return result,
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_protocol::clipboard::source::wire;
    #[test]
    fn partial_frame_progress_refreshes_inactivity_without_accepting_invalid_data() {
        let activity = Arc::new(Mutex::new(Instant::now() - Duration::from_secs(60)));
        let mut reader = TrackedReader {
            input: &[5, 10, 3, 8][..],
            activity: activity.clone(),
        };
        assert_eq!(
            io::Error::from(wire::read_response(&mut reader).unwrap_err()).kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert!(activity.lock().unwrap().elapsed() < Duration::from_secs(1));
        *activity.lock().unwrap() = Instant::now() - Duration::from_secs(60);
        assert_eq!(reader.read(&mut [0]).unwrap(), 0);
        assert!(activity.lock().unwrap().elapsed() >= Duration::from_secs(60));
    }

    #[test]
    fn blocked_output_has_a_deadline() {
        use std::os::unix::net::UnixStream;
        let (mut output, _reader) = UnixStream::pair().unwrap();
        output.set_nonblocking(true).unwrap();
        loop {
            match output.write(&[0; 65536]) {
                Ok(_) => (),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("{error}"),
            }
        }
        let mut writer = DeadlineWriter {
            inner: output,
            timeout: Duration::from_millis(20),
            cancellation: Cancellation::default(),
        };
        let started = Instant::now();
        assert_eq!(
            writer.write(&[1]).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}

/// Cooperative session cancellation; commands already executing retain their
/// existing completion/journal rules, but no new request is admitted afterward.
#[derive(Clone, Default)]
pub(crate) struct Cancellation(std::sync::Arc<std::sync::atomic::AtomicBool>);
impl Cancellation {
    pub(crate) fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }
    pub(crate) fn check(&self) -> io::Result<()> {
        if self.0.load(std::sync::atomic::Ordering::Acquire) {
            Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "Needle runtime session ended",
            ))
        } else {
            Ok(())
        }
    }
    pub(crate) fn stdin(&self) -> io::Result<CancellableInput> {
        use std::os::fd::AsFd;
        Ok(CancellableInput {
            input: std::fs::File::from(io::stdin().as_fd().try_clone_to_owned()?),
            cancellation: self.clone(),
        })
    }
}
pub(crate) struct CancellableInput {
    input: std::fs::File,
    cancellation: Cancellation,
}
impl Read for CancellableInput {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            self.cancellation.check()?;
            let mut fd = libc::pollfd {
                fd: self.input.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one initialized pollfd for the call.
            let result = unsafe { libc::poll(&mut fd, 1, 50) };
            if result > 0 {
                self.cancellation.check()?;
                return self.input.read(bytes);
            }
            if result < 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
    }
}
