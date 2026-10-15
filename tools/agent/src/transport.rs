//! Bounded stdio transport and heartbeat activity tracking.
use std::{
    io::{self, Read, Write},
    os::fd::AsRawFd,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
// Account for upload progress even before a large frame has completed.
pub(crate) struct TrackedReader<R> {
    pub(crate) input: R,
    pub(crate) activity: Arc<Mutex<Instant>>,
}
impl<R: Read> Read for TrackedReader<R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let count = self.input.read(bytes)?;
        if count > 0 {
            *self.activity.lock().unwrap() = Instant::now();
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
    use crate::wire;
    #[test]
    fn partial_frame_progress_refreshes_inactivity_without_accepting_invalid_data() {
        let activity = Arc::new(Mutex::new(Instant::now() - Duration::from_secs(60)));
        let mut reader = TrackedReader {
            input: &[b'M', 0, 0, 0, 5, 1, 2][..],
            activity: activity.clone(),
        };
        assert_eq!(
            wire::read(&mut reader).unwrap_err().kind(),
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
