use crate::{
    commands::{private_write, root},
    snapshot, wire, wl_server,
    wl_socket::Endpoint,
    x_server,
};
use std::{
    fs,
    io::{self, Read, Write},
    os::{
        fd::{AsFd, AsRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

fn error_kind(error: &io::Error) -> u8 {
    match error.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted => b'T',
        _ => b'E',
    }
}

// Account for upload progress even before a large frame has completed.
struct TrackedReader<R> {
    input: R,
    activity: Arc<Mutex<Instant>>,
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
struct DeadlineWriter<W> {
    inner: W,
    timeout: Duration,
}
impl<W: Write + AsRawFd> Write for DeadlineWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let deadline = Instant::now() + self.timeout;
        loop {
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

struct Cleanup(std::path::PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        for name in [
            "snapshot.tar",
            "display",
            "Xauthority",
            "agent-client",
            "features",
        ] {
            let _ = fs::remove_file(self.0.join(name));
        }
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
    let root = root()?;
    let (listener, _endpoint) = Endpoint::fixed(&root.join("agent.sock")).map_err(|e| {
        if matches!(
            e.kind(),
            io::ErrorKind::AddrInUse | io::ErrorKind::WouldBlock
        ) && fs::read_to_string(root.join("agent-client"))
            .ok()
            .as_deref()
            == Some(client)
        {
            io::Error::new(io::ErrorKind::WouldBlock, "previous agent is still closing")
        } else if matches!(
            e.kind(),
            io::ErrorKind::AddrInUse | io::ErrorKind::WouldBlock
        ) {
            io::Error::other("Integration is active in another Porthop installation.")
        } else {
            e
        }
    })?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(root.join("lock"))?;
    let meta = lock.metadata()?;
    if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() } || meta.nlink() != 1 {
        return Err(io::Error::other("invalid clipboard lock"));
    }
    fs2::FileExt::try_lock_exclusive(&lock).map_err(|e| {
        if e.kind() != io::ErrorKind::WouldBlock {
            return e;
        }
        if fs::read_to_string(root.join("client"))
            .ok()
            .as_deref()
            .map(str::trim)
            == Some(client)
        {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                "previous clipboard helper is still closing",
            )
        } else {
            io::Error::other("Clipboard is locked by another process.")
        }
    })?;
    let legacy_active = fs::metadata(root.join("session"))
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < Duration::from_secs(120));
    if legacy_active
        && fs::read_to_string(root.join("client"))
            .ok()
            .as_deref()
            .map(str::trim)
            != Some(client)
    {
        return Err(io::Error::other(
            "Clipboard sync is active in another Porthop installation.",
        ));
    }
    let _cleanup = Cleanup(root.clone());
    let snapshot = root.join("snapshot.tar");
    match fs::remove_file(&snapshot) {
        Ok(()) => (),
        Err(e) if e.kind() == io::ErrorKind::NotFound => (),
        Err(e) => return Err(e),
    }
    let wayland = clipboard
        .then(|| {
            wl_server::Server::start_fixed(&snapshot, &root.join("wayland.sock"))
                .map_err(|e| io::Error::other(format!("Cannot start Wayland clipboard: {e}")))
        })
        .transpose()?;
    let x11 = clipboard
        .then(|| x_server::Server::start(&snapshot, None))
        .transpose()?;
    private_write(&root.join("agent-client"), client.as_bytes())?;
    for name in [
        "snapshot.tar",
        "session",
        "client",
        "browser-url",
        "browser-lease",
        "native-display",
    ] {
        let _ = fs::remove_file(root.join(name));
    }
    private_write(
        &root.join("features"),
        format!(
            "{} {}",
            if clipboard { "clipboard" } else { "" },
            if browser { "browser" } else { "" }
        )
        .as_bytes(),
    )?;
    if let Some(x11) = &x11 {
        private_write(&root.join("display"), x11.display.as_bytes())?;
        private_write(&root.join("Xauthority"), &fs::read(x11.authority())?)?;
    } else {
        let _ = fs::remove_file(root.join("display"));
        let _ = fs::remove_file(root.join("Xauthority"));
    }
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
    let mut last_open = Instant::now() - Duration::from_secs(2);
    loop {
        if stopped.load(Ordering::Relaxed) {
            return Ok(());
        }
        if last_input.lock().unwrap().elapsed() > Duration::from_secs(45) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "agent heartbeat expired",
            ));
        }
        if x11.as_ref().is_some_and(|x| !x.is_running()) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "X11 clipboard service stopped",
            ));
        }
        if wayland.as_ref().is_some_and(|w| !w.is_running()) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Wayland clipboard service stopped",
            ));
        }
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(Ok((kind, data))) => match kind {
                b'S' if clipboard => {
                    let mut pending = tempfile::NamedTempFile::new_in(&root)?;
                    pending
                        .as_file()
                        .set_permissions(fs::Permissions::from_mode(0o600))?;
                    pending.write_all(&data)?;
                    snapshot::read(pending.path())?;
                    pending.persist(&snapshot).map_err(|e| e.error)?;
                    let backend = crate::native::update(&snapshot);
                    wire::write(output, b'A', backend.as_bytes())?;
                }
                b'H' if data.is_empty() => {
                    if let Ok(file) = fs::OpenOptions::new().write(true).open(&snapshot) {
                        file.set_modified(std::time::SystemTime::now())?;
                    }
                    wire::write(output, b'A', b"")?;
                }
                b'Q' if data.is_empty() => return Ok(()),
                _ => return Err(io::Error::other("unsupported agent message")),
            },
            Ok(Err(e)) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Ok(Err(e)) => return Err(e),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            Err(mpsc::RecvTimeoutError::Timeout) => (),
        }
        // Private local IPC, never a network listener. One bounded request per turn.
        if let Ok((mut stream, _)) = listener.accept() {
            stream.set_read_timeout(Some(Duration::from_millis(100)))?;
            stream.set_write_timeout(Some(Duration::from_millis(100)))?;
            let mut request = String::new();
            let read = (&mut stream).take(8193).read_to_string(&mut request);
            if browser
                && read.is_ok()
                && wire::web_url(&request).is_some()
                && last_open.elapsed() >= Duration::from_secs(1)
            {
                wire::write(output, b'O', request.as_bytes())?;
                last_open = Instant::now();
                let _ = stream.write_all(b"ok");
            } else {
                let _ = stream.write_all(b"rejected");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_frame_progress_refreshes_inactivity_without_accepting_invalid_data() {
        let activity = Arc::new(Mutex::new(Instant::now() - Duration::from_secs(60)));
        let mut reader = TrackedReader {
            input: &[b'S', 0, 0, 0, 5, 1, 2][..],
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
        };
        let started = Instant::now();
        assert_eq!(
            writer.write(&[1]).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert!(started.elapsed() < Duration::from_secs(1));
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
