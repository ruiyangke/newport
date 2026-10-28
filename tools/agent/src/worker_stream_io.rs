//! One bounded relay per admitted worker. Child pipe flags are independent of
//! the SSH-forwarded socket, and stderr/exit remain separate from service data.
use crate::{
    runtime_protocol::worker_stream::{Direction, Frame, Header, Kind, MAX_CHUNK},
    transport::Cancellation,
};
use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, RawFd},
        unix::{net::UnixStream, process::ExitStatusExt},
    },
    process::{Child, ExitStatus},
    time::{Duration, Instant},
};

const QUEUED_FRAMES: usize = 8;
const OUTPUT_STALL: Duration = Duration::from_secs(5);
const EXIT_DRAIN: Duration = Duration::from_secs(2);

pub(super) struct Process {
    child: Child,
    reaped: bool,
}
impl Process {
    pub(super) fn new(child: Child) -> Self {
        Self {
            child,
            reaped: false,
        }
    }
    fn status(&mut self) -> io::Result<Option<ExitStatus>> {
        let status = self.child.try_wait()?;
        if status.is_some() {
            self.reaped = true;
        }
        Ok(status)
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if !self.reaped {
            // The unreaped leader reserves its PID throughout escalation, so
            // killpg cannot accidentally target a reused process-group ID.
            // The private GUI guardian has its own group and survives to clean
            // display resources and descendants when this launcher disappears.
            let group = self.child.id() as libc::pid_t;
            unsafe {
                libc::kill(-group, libc::SIGTERM);
            }
            std::thread::sleep(Duration::from_millis(250));
            unsafe {
                libc::kill(-group, libc::SIGKILL);
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
fn nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid worker stream sequence")
}
fn pending(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    )
}

#[derive(Default)]
struct Incoming {
    header: [u8; 5],
    used: usize,
    data: Vec<u8>,
    read: usize,
}
impl Incoming {
    fn receive(&mut self, stream: &mut UnixStream) -> io::Result<Option<Frame>> {
        if self.used < self.header.len() {
            let count = stream.read(&mut self.header[self.used..])?;
            if count == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            self.used += count;
            if self.used != self.header.len() {
                return Ok(None);
            }
            let header = Header::decode(self.header, Direction::ToWorker)?;
            self.data.resize(header.length, 0);
        }
        if self.read < self.data.len() {
            let count = stream.read(&mut self.data[self.read..])?;
            if count == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            self.read += count;
            if self.read != self.data.len() {
                return Ok(None);
            }
        }
        let header = Header::decode(self.header, Direction::ToWorker)?;
        let frame = Frame {
            kind: header.kind,
            data: std::mem::take(&mut self.data),
        };
        self.used = 0;
        self.read = 0;
        Ok(Some(frame))
    }
}
struct Bytes {
    data: Vec<u8>,
    used: usize,
}
impl Bytes {
    fn send(&mut self, writer: &mut impl Write) -> io::Result<bool> {
        let count = writer.write(&self.data[self.used..])?;
        if count == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        self.used += count;
        Ok(self.used == self.data.len())
    }
}
fn queue(
    output: &mut VecDeque<Bytes>,
    kind: Kind,
    data: Vec<u8>,
    progress: &mut Instant,
) -> io::Result<()> {
    if output.is_empty() {
        *progress = Instant::now();
    }
    let mut bytes = Vec::with_capacity(5 + data.len());
    Frame { kind, data }.write(&mut bytes, Direction::FromWorker)?;
    output.push_back(Bytes {
        data: bytes,
        used: 0,
    });
    Ok(())
}
fn read_pipe<R: Read>(
    pipe: &mut Option<R>,
    kind: Kind,
    output: &mut VecDeque<Bytes>,
    progress: &mut Instant,
) -> io::Result<()> {
    if output.len() >= QUEUED_FRAMES {
        return Ok(());
    }
    if let Some(reader) = pipe.as_mut() {
        let mut bytes = [0; MAX_CHUNK];
        match reader.read(&mut bytes) {
            Ok(0) => {
                *pipe = None;
            }
            Ok(count) => {
                queue(output, kind, bytes[..count].to_vec(), progress)?;
            }
            Err(error) if pending(&error) => (),
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
fn poll_descriptor(fd: RawFd, events: i16) -> libc::pollfd {
    libc::pollfd {
        fd,
        events,
        revents: 0,
    }
}

pub(super) fn relay(
    mut socket: UnixStream,
    process: &mut Process,
    stop: &Cancellation,
) -> io::Result<()> {
    let mut input = process.child.stdin.take();
    let mut output = process.child.stdout.take();
    let mut error = process.child.stderr.take();
    if input.is_none() || output.is_none() || error.is_none() {
        return Err(io::Error::other("Missing worker pipes"));
    }
    nonblocking(socket.as_raw_fd())?;
    nonblocking(input.as_ref().unwrap().as_raw_fd())?;
    nonblocking(output.as_ref().unwrap().as_raw_fd())?;
    nonblocking(error.as_ref().unwrap().as_raw_fd())?;
    let mut incoming = Incoming::default();
    let mut writing: Option<Bytes> = None;
    let mut queued: VecDeque<Bytes> = VecDeque::with_capacity(QUEUED_FRAMES);
    let mut error_first = false;
    let mut input_eof = false;
    let mut status = None;
    let mut exited = Instant::now();
    let mut sent_exit = false;
    let mut progress = Instant::now();
    loop {
        stop.check()?;
        if let Some(bytes) = queued.front_mut() {
            match bytes.send(&mut socket) {
                Ok(done) => {
                    progress = Instant::now();
                    if done {
                        queued.pop_front();
                    }
                }
                Err(error) if pending(&error) => (),
                Err(error) => return Err(error),
            }
        }
        if sent_exit && queued.is_empty() {
            return Ok(());
        }
        if !queued.is_empty() && progress.elapsed() >= OUTPUT_STALL {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Worker stream output stalled",
            ));
        }
        if let Some(bytes) = writing.as_mut() {
            let writer = input.as_mut().ok_or_else(invalid)?;
            match bytes.send(writer) {
                Ok(true) => writing = None,
                Ok(false) => (),
                Err(error) if pending(&error) => (),
                Err(error) => return Err(error),
            }
        }
        if writing.is_none() && status.is_none() {
            match incoming.receive(&mut socket) {
                Ok(Some(frame)) if input_eof => {
                    let _ = frame;
                    return Err(invalid());
                }
                Ok(Some(frame)) if frame.kind == Kind::InputEof => {
                    input_eof = true;
                    input = None;
                }
                Ok(Some(frame)) => {
                    writing = Some(Bytes {
                        data: frame.data,
                        used: 0,
                    })
                }
                Ok(None) => (),
                Err(error) if pending(&error) => (),
                Err(error) => return Err(error),
            }
        }
        let before = queued.len();
        if error_first {
            read_pipe(&mut error, Kind::Error, &mut queued, &mut progress)?;
            read_pipe(&mut output, Kind::Output, &mut queued, &mut progress)?;
        } else {
            read_pipe(&mut output, Kind::Output, &mut queued, &mut progress)?;
            read_pipe(&mut error, Kind::Error, &mut queued, &mut progress)?;
        }
        error_first = !error_first;
        if queued.len() > before || queued.len() == QUEUED_FRAMES {
            exited = Instant::now();
        }
        if status.is_none() {
            status = process.status()?;
            if status.is_some() {
                exited = Instant::now();
                input = None;
                writing = None;
            }
        }
        if let Some(status) = status {
            if output.is_none() && error.is_none() && !sent_exit && queued.len() < QUEUED_FRAMES {
                let (kind, value) = match status.signal() {
                    Some(signal) => (Kind::ExitSignal, signal as u32),
                    None => (Kind::ExitStatus, status.code().unwrap_or(1) as u32),
                };
                queue(
                    &mut queued,
                    kind,
                    value.to_be_bytes().to_vec(),
                    &mut progress,
                )?;
                sent_exit = true;
            } else if (output.is_some() || error.is_some()) && exited.elapsed() >= EXIT_DRAIN {
                // The leader has been reaped: never signal its recyclable PID.
                // Service-specific supervisors own detached descendants.
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Worker output remained open after exit",
                ));
            }
        }
        let reading = writing.is_none() && status.is_none();
        let mut polls = vec![poll_descriptor(
            socket.as_raw_fd(),
            if reading { libc::POLLIN } else { 0 }
                | if queued.is_empty() { 0 } else { libc::POLLOUT },
        )];
        if let Some(pipe) = input.as_ref().filter(|_| writing.is_some()) {
            polls.push(poll_descriptor(pipe.as_raw_fd(), libc::POLLOUT));
        }
        if queued.len() < QUEUED_FRAMES {
            if let Some(pipe) = &output {
                polls.push(poll_descriptor(pipe.as_raw_fd(), libc::POLLIN));
            }
            if let Some(pipe) = &error {
                polls.push(poll_descriptor(pipe.as_raw_fd(), libc::POLLIN));
            }
        }
        let result = unsafe { libc::poll(polls.as_mut_ptr(), polls.len() as _, 50) };
        if result < 0 {
            let error = io::Error::last_os_error();
            if !pending(&error) {
                return Err(error);
            }
        }
        if polls[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "Worker stream disconnected",
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        net::Shutdown,
        os::unix::process::CommandExt,
        process::{Command, Stdio},
        sync::mpsc,
    };
    struct Fixture {
        socket: UnixStream,
        stop: Cancellation,
        done: mpsc::Receiver<io::Result<()>>,
    }
    impl Fixture {
        fn start(command: &mut Command) -> Self {
            let child = command
                .process_group(0)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let (socket, server) = UnixStream::pair().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(8)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(8)))
                .unwrap();
            let stop = Cancellation::default();
            let stopping = stop.clone();
            let (done, result) = mpsc::sync_channel(1);
            std::thread::spawn(move || {
                let mut process = Process::new(child);
                let result = relay(server, &mut process, &stopping);
                drop(process);
                let _ = done.send(result);
            });
            Self {
                socket,
                stop,
                done: result,
            }
        }
        fn send(&mut self, kind: Kind, data: Vec<u8>) {
            Frame { kind, data }
                .write(&mut self.socket, Direction::ToWorker)
                .unwrap();
        }
        fn read(&mut self) -> Frame {
            Frame::read(&mut self.socket, Direction::FromWorker).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.stop.cancel();
            let _ = self.socket.shutdown(Shutdown::Both);
        }
    }
    #[test]
    fn relay_preserves_binary_io_stderr_half_close_and_final_status() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "cat; printf diagnostic >&2; exit 23"]);
        let mut fixture = Fixture::start(&mut command);
        let expected: Vec<u8> = (0..=255).cycle().take(8192).collect();
        fixture.send(Kind::Input, expected.clone());
        fixture.send(Kind::InputEof, vec![]);
        let mut output = vec![];
        let mut stderr = vec![];
        loop {
            let frame = fixture.read();
            match frame.kind {
                Kind::Output => output.extend(frame.data),
                Kind::Error => stderr.extend(frame.data),
                Kind::ExitStatus => {
                    assert_eq!(frame.data, 23_u32.to_be_bytes());
                    break;
                }
                other => panic!("unexpected frame kind {other:?}"),
            }
        }
        assert_eq!(output, expected);
        assert_eq!(stderr, b"diagnostic");
        fixture
            .done
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
    }
    #[test]
    fn relay_rejects_wrong_direction_and_data_after_half_close() {
        for after_eof in [false, true] {
            let mut command = Command::new("/bin/sleep");
            command.arg("30");
            let mut fixture = Fixture::start(&mut command);
            if after_eof {
                fixture.send(Kind::InputEof, vec![]);
                fixture.send(Kind::Input, vec![1]);
            } else {
                fixture
                    .socket
                    .write_all(&[Kind::Output as u8, 0, 0, 0, 1, 1])
                    .unwrap();
            }
            let error = fixture
                .done
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        }
    }
    #[test]
    fn relay_cancels_when_input_is_blocked_or_the_client_disappears() {
        for disconnect in [false, true] {
            let mut command = Command::new("/bin/sleep");
            command.arg("30");
            let mut fixture = Fixture::start(&mut command);
            fixture.send(Kind::Input, vec![0; MAX_CHUNK]);
            if disconnect {
                fixture.socket.shutdown(Shutdown::Both).unwrap();
            } else {
                fixture.stop.cancel();
            }
            assert!(
                fixture
                    .done
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap()
                    .is_err()
            );
        }
    }
    #[test]
    fn relay_reports_a_signal_and_does_not_treat_transport_eof_as_half_close() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "kill -TERM $$"]);
        let mut fixture = Fixture::start(&mut command);
        let frame = fixture.read();
        assert_eq!(frame.kind, Kind::ExitSignal);
        assert_eq!(frame.data, (libc::SIGTERM as u32).to_be_bytes());
        fixture
            .done
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        let mut command = Command::new("/bin/sleep");
        command.arg("30");
        let mut fixture = Fixture::start(&mut command);
        fixture.send(Kind::InputEof, vec![]);
        fixture.socket.shutdown(Shutdown::Write).unwrap();
        assert!(
            fixture
                .done
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .is_err()
        );
    }
    #[test]
    fn stalled_output_expires_without_blocking_another_worker() {
        let mut loud_command = Command::new("/usr/bin/yes");
        let loud = Fixture::start(&mut loud_command);
        let mut quick_command = Command::new("/bin/echo");
        quick_command.arg("alive");
        let mut quick = Fixture::start(&mut quick_command);
        let mut bytes = vec![];
        loop {
            let frame = quick.read();
            if frame.kind == Kind::ExitStatus {
                assert_eq!(frame.data, [0; 4]);
                break;
            }
            assert_eq!(frame.kind, Kind::Output);
            bytes.extend(frame.data);
        }
        assert_eq!(bytes, b"alive\n");
        quick
            .done
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert_eq!(
            loud.done
                .recv_timeout(Duration::from_secs(8))
                .unwrap()
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
    }
}
