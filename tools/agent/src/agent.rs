//! Integration session event loop: dispatch frames, track liveness, and coordinate services.
use crate::{
    snapshot,
    transport::{DeadlineWriter, TrackedReader},
    wire,
};
use std::{
    fs,
    io::{self, Write},
    os::fd::{AsFd, AsRawFd},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

fn error_kind(error: &io::Error) -> u8 {
    match error.kind() {
        io::ErrorKind::WouldBlock
        | io::ErrorKind::TimedOut
        | io::ErrorKind::Interrupted
        | io::ErrorKind::StorageFull
        | io::ErrorKind::QuotaExceeded => b'T',
        _ => b'E',
    }
}

pub fn serve(client: &str, clipboard: bool, browser: bool) -> io::Result<()> {
    if client.len() > 64
        || !client
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(io::Error::other("invalid client identity"));
    }
    let stdout = fs::File::from(io::stdout().as_fd().try_clone_to_owned()?);
    let flags = unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut output = DeadlineWriter {
        inner: stdout,
        timeout: Duration::from_secs(5),
    };
    let result = run(client, clipboard, browser, &mut output);
    if let Err(error) = &result {
        let _ = wire::write(&mut output, error_kind(error), error.to_string().as_bytes());
    }
    result
}
fn run(client: &str, clipboard: bool, browser: bool, output: &mut impl Write) -> io::Result<()> {
    let (session, listener) = crate::session::Session::start(client, clipboard, browser)?;
    let snapshot = &session.snapshot;
    let stopped = Arc::new(AtomicBool::new(false));
    let flag = stopped.clone();
    ctrlc::set_handler(move || flag.store(true, Ordering::Relaxed)).map_err(io::Error::other)?;
    let last_input = Arc::new(Mutex::new(Instant::now()));
    let activity = last_input.clone();
    let (tx, rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut input = TrackedReader {
            input: io::stdin().lock(),
            activity,
        };
        loop {
            let frame = wire::read(&mut input);
            let failed = frame.is_err();
            if tx.send(frame).is_err() || failed {
                break;
            }
        }
    });
    wire::write(output, b'R', wire::VERSION.as_bytes())?;
    let mut browser = crate::browser::Broker::new(listener, browser);
    loop {
        browser.expire();
        if stopped.load(Ordering::Relaxed) {
            return Ok(());
        }
        if last_input.lock().unwrap().elapsed() > Duration::from_secs(45) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "agent heartbeat expired",
            ));
        }
        session.check_backends()?;
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(Ok((kind, data))) => match kind {
                b'S' if clipboard => {
                    snapshot::publish(snapshot, &data)?;
                    let backend = crate::native::update(snapshot);
                    wire::write(output, b'A', backend.as_bytes())?;
                }
                b'H' if data.is_empty() => {
                    if let Ok(file) = fs::OpenOptions::new().write(true).open(snapshot) {
                        file.set_modified(std::time::SystemTime::now())?;
                    }
                    wire::write(output, b'A', b"")?;
                }
                b'B' if browser.enabled() => browser.reply(&data),
                b'Q' if data.is_empty() => return Ok(()),
                _ => return Err(io::Error::other("unsupported agent message")),
            },
            Ok(Err(e)) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Ok(Err(e)) => return Err(e),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            Err(mpsc::RecvTimeoutError::Timeout) => (),
        }
        browser.poll(output)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disk_capacity_errors_are_retryable_including_path_wrappers() {
        for code in [libc::ENOSPC, libc::EDQUOT] {
            let error = io::Error::from_raw_os_error(code);
            assert_eq!(error_kind(&error), b'T');
            // tempfile attaches the failing path, preserving ErrorKind rather
            // than raw_os_error. Classification must survive that wrapper.
            let wrapped = io::Error::new(error.kind(), format!("{error} at path /clipboard/.tmp"));
            assert_eq!(error_kind(&wrapped), b'T');
        }
        for code in [libc::EACCES, libc::EROFS] {
            assert_eq!(error_kind(&io::Error::from_raw_os_error(code)), b'E');
        }
    }
    #[test]
    fn permanent_errors_do_not_inherit_retry_from_their_text() {
        assert_eq!(
            error_kind(&io::Error::new(io::ErrorKind::TimedOut, "heartbeat")),
            b'T'
        );
        assert_eq!(
            error_kind(&io::Error::new(
                io::ErrorKind::PermissionDenied,
                "SSH transport interrupted: permission"
            )),
            b'E'
        );
    }
}
