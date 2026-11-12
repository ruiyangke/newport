//! One bounded relay per admitted worker. Child pipe flags are independent of
//! the SSH-forwarded socket, and stderr/exit remain separate from service data.
use crate::transport::Cancellation;
use bytes::Bytes;
use newport_protocol::{
    encoding::protobuf::{EncodedFrame, FrameDecoder},
    worker::{
        self, Chunk, ClientControl, Direction, Exit, ServerControl, Signal, Transport,
        wire::{self, ClientDecoder, Frame},
    },
};
use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    os::{
        fd::{AsFd, AsRawFd, RawFd},
        unix::{net::UnixStream, process::ExitStatusExt},
    },
    process::{Child, ExitStatus},
    time::{Duration, Instant},
};

const QUEUED_FRAMES: usize = 8;
const QUEUED_BYTES: usize = QUEUED_FRAMES * worker::MAX_CHUNK;
const OUTPUT_STALL: Duration = Duration::from_secs(5);
const EXIT_DRAIN: Duration = Duration::from_secs(2);
#[cfg(target_os = "linux")]
const READ_CLOSED: i16 = libc::POLLRDHUP;
#[cfg(not(target_os = "linux"))]
const READ_CLOSED: i16 = 0;

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
    fn signal(&self, signal: Signal) -> io::Result<()> {
        if self.reaped {
            return Err(invalid());
        }
        let group = libc::pid_t::try_from(self.child.id()).map_err(malformed)?;
        let signal = libc::c_int::try_from(signal.number()).map_err(malformed)?;
        // SAFETY: kill takes scalar arguments; the unreaped worker leads this group.
        if unsafe { libc::kill(-group, signal) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        // An exited but unreaped leader still reserves the process-group ID.
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error)
        }
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
            // SAFETY: kill takes scalar arguments; the unreaped worker leads this group.
            unsafe {
                libc::kill(-group, libc::SIGTERM);
            }
            std::thread::sleep(Duration::from_millis(250));
            // SAFETY: kill takes scalar arguments; the unreaped worker leads this group.
            unsafe {
                libc::kill(-group, libc::SIGKILL);
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid worker stream sequence")
}
fn malformed(error: impl std::error::Error + Send + Sync + 'static) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}
fn pending(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    )
}
fn read_closed(socket: &UnixStream, events: i16) -> io::Result<bool> {
    #[cfg(target_os = "linux")]
    if events & READ_CLOSED != 0 {
        return Ok(true);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = events;
    // EOF must cancel a worker even while its stdin cannot accept more data.
    // Peeking leaves any unread frame bytes with the framing reader.
    let mut byte = [0_u8; 1];
    // SAFETY: one live borrowed socket and a writable byte; MSG_PEEK consumes nothing.
    let count = unsafe {
        libc::recv(
            socket.as_raw_fd(),
            byte.as_mut_ptr().cast(),
            byte.len(),
            libc::MSG_PEEK | libc::MSG_DONTWAIT,
        )
    };
    if count >= 0 {
        return Ok(count == 0);
    }
    let error = io::Error::last_os_error();
    if pending(&error) {
        Ok(false)
    } else {
        Err(error)
    }
}

enum ReadFrame {
    Frame(EncodedFrame),
    Partial,
    End,
}
struct FramedInput {
    decoder: FrameDecoder,
    pending: Bytes,
    used: usize,
}
impl FramedInput {
    fn new(limit: usize) -> Self {
        Self {
            decoder: FrameDecoder::new(limit),
            pending: Bytes::new(),
            used: 0,
        }
    }
    fn buffered(&self) -> bool {
        self.used < self.pending.len()
    }
    fn receive(&mut self, reader: &mut impl Read) -> io::Result<ReadFrame> {
        if !self.buffered() {
            let mut buffer = [0; worker::MAX_CHUNK];
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                self.decoder.finish().map_err(malformed)?;
                return Ok(ReadFrame::End);
            }
            self.pending = Bytes::copy_from_slice(&buffer[..count]);
            self.used = 0;
        }
        let mut remaining = &self.pending[self.used..];
        let frame = self.decoder.consume(&mut remaining).map_err(malformed)?;
        self.used = self.pending.len() - remaining.len();
        Ok(match frame {
            Some(frame) => ReadFrame::Frame(frame),
            None => ReadFrame::Partial,
        })
    }
}
struct Writing {
    data: Bytes,
    used: usize,
}
impl Writing {
    fn send(&mut self, writer: &mut impl Write) -> io::Result<bool> {
        let count = writer.write(&self.data[self.used..])?;
        if count == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        self.used += count;
        Ok(self.used == self.data.len())
    }
}
struct Output {
    frames: VecDeque<Writing>,
    bytes: usize,
    progress: Instant,
}
impl Output {
    fn new() -> Self {
        Self {
            frames: VecDeque::with_capacity(QUEUED_FRAMES),
            bytes: 0,
            progress: Instant::now(),
        }
    }
    fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
    fn available(&self) -> bool {
        self.frames.len() < QUEUED_FRAMES && self.bytes < QUEUED_BYTES
    }
    fn push(&mut self, data: Bytes) -> io::Result<()> {
        if !self.available() {
            return Err(invalid());
        }
        if self.is_empty() {
            self.progress = Instant::now();
        }
        // A single complete service frame may exceed the queue's small-frame
        // budget. Count its entire length until removal, including sent bytes.
        self.bytes = self.bytes.checked_add(data.len()).ok_or_else(invalid)?;
        self.frames.push_back(Writing { data, used: 0 });
        Ok(())
    }
    fn control(&mut self, control: ServerControl) -> io::Result<()> {
        self.push(wire::encode_server(&control).map_err(malformed)?.into())
    }
    fn send(&mut self, socket: &mut UnixStream) -> io::Result<()> {
        if let Some(frame) = self.frames.front_mut() {
            match frame.send(socket) {
                Ok(done) => {
                    self.progress = Instant::now();
                    if done {
                        self.bytes -= frame.data.len();
                        self.frames.pop_front();
                    }
                }
                Err(error) if pending(&error) => (),
                Err(error) => return Err(error),
            }
        }
        if !self.is_empty() && self.progress.elapsed() >= OUTPUT_STALL {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Worker stream output stalled",
            ));
        }
        Ok(())
    }
}
fn read_stderr<R: Read>(pipe: &mut Option<R>, output: &mut Output) -> io::Result<bool> {
    if !output.available() {
        return Ok(false);
    }
    if let Some(reader) = pipe.as_mut() {
        let mut buffer = [0; worker::MAX_CHUNK];
        match reader.read(&mut buffer) {
            Ok(0) => *pipe = None,
            Ok(count) => {
                output.control(ServerControl::Stderr(
                    Chunk::new(Bytes::copy_from_slice(&buffer[..count])).map_err(malformed)?,
                ))?;
                return Ok(true);
            }
            Err(error) if pending(&error) => (),
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}
fn read_stdout<R: Read>(
    pipe: &mut Option<R>,
    transport: Transport,
    framed: &mut FramedInput,
    output: &mut Output,
) -> io::Result<bool> {
    if !output.available() {
        return Ok(false);
    }
    let Some(reader) = pipe.as_mut() else {
        return Ok(false);
    };
    match transport {
        Transport::Opaque => {
            let mut buffer = [0; worker::MAX_CHUNK];
            match reader.read(&mut buffer) {
                Ok(0) => *pipe = None,
                Ok(count) => {
                    let chunk =
                        Chunk::new(Bytes::copy_from_slice(&buffer[..count])).map_err(malformed)?;
                    output.push(
                        wire::encode_opaque(&chunk, Direction::FromWorker)
                            .map_err(malformed)?
                            .into(),
                    )?;
                    return Ok(true);
                }
                Err(error) if pending(&error) => (),
                Err(error) => return Err(error),
            }
        }
        Transport::Protobuf { server, .. } => match framed.receive(reader) {
            Ok(ReadFrame::Frame(frame)) => {
                wire::reject_controls(&frame.body(), server.message).map_err(malformed)?;
                output.push(frame.into_bytes())?;
                return Ok(true);
            }
            Ok(ReadFrame::Partial) => return Ok(true),
            Ok(ReadFrame::End) => *pipe = None,
            Err(error) if pending(&error) => (),
            Err(error) => return Err(error),
        },
    }
    Ok(false)
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
    transport: Transport,
) -> io::Result<()> {
    let mut input = process.child.stdin.take();
    let mut stdout = process.child.stdout.take();
    let mut stderr = process.child.stderr.take();
    let (Some(input_pipe), Some(output_pipe), Some(error_pipe)) = (&input, &stdout, &stderr) else {
        return Err(io::Error::other("Missing worker pipes"));
    };
    crate::transport::set_nonblocking(socket.as_fd())?;
    crate::transport::set_nonblocking(input_pipe.as_fd())?;
    crate::transport::set_nonblocking(output_pipe.as_fd())?;
    crate::transport::set_nonblocking(error_pipe.as_fd())?;
    let input_limits = transport.limits(Direction::ToWorker);
    let output_limits = match transport {
        Transport::Protobuf { server, .. } => server,
        Transport::Opaque => transport.limits(Direction::FromWorker),
    };
    let mut incoming = FramedInput::new(input_limits.message);
    let mut client = ClientDecoder::default();
    let mut outgoing = FramedInput::new(output_limits.message);
    let mut writing: Option<Writing> = None;
    let mut queued = Output::new();
    let mut error_first = false;
    let mut status = None;
    let mut exited = Instant::now();
    let mut sent_exit = false;
    loop {
        stop.check()?;
        queued.send(&mut socket)?;
        if sent_exit && queued.is_empty() {
            return Ok(());
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
                Ok(ReadFrame::Frame(frame)) => match client
                    .decode(frame.body(), &input_limits)
                    .map_err(malformed)?
                {
                    Frame::Control(ClientControl::InputComplete) => input = None,
                    Frame::Control(ClientControl::Signal(signal)) => process.signal(signal)?,
                    Frame::Service(body) => {
                        let data = match transport {
                            Transport::Opaque => {
                                wire::decode_opaque(body).map_err(malformed)?.into_data()
                            }
                            Transport::Protobuf { client, .. } => {
                                if body.len() > client.message {
                                    return Err(invalid());
                                }
                                frame.into_bytes()
                            }
                        };
                        writing = Some(Writing { data, used: 0 });
                    }
                },
                Ok(ReadFrame::Partial) => (),
                Ok(ReadFrame::End) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Err(error) if pending(&error) => (),
                Err(error) => return Err(error),
            }
        }
        let progressed = if error_first {
            let error = read_stderr(&mut stderr, &mut queued)?;
            read_stdout(&mut stdout, transport, &mut outgoing, &mut queued)? || error
        } else {
            let output = read_stdout(&mut stdout, transport, &mut outgoing, &mut queued)?;
            read_stderr(&mut stderr, &mut queued)? || output
        };
        error_first = !error_first;
        if progressed || !queued.available() {
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
            if stdout.is_none() && stderr.is_none() && !sent_exit && queued.available() {
                let exit = match status.signal() {
                    Some(signal) => Exit::Signal(
                        Signal::new(u32::try_from(signal).map_err(malformed)?)
                            .map_err(malformed)?,
                    ),
                    None => {
                        Exit::Status(u32::try_from(status.code().unwrap_or(1)).map_err(malformed)?)
                    }
                };
                queued.control(ServerControl::Exit(exit))?;
                sent_exit = true;
            } else if (stdout.is_some() || stderr.is_some()) && exited.elapsed() >= EXIT_DRAIN {
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
                | if queued.is_empty() { 0 } else { libc::POLLOUT }
                | READ_CLOSED,
        )];
        if let Some(pipe) = input.as_ref().filter(|_| writing.is_some()) {
            polls.push(poll_descriptor(pipe.as_raw_fd(), libc::POLLOUT));
        }
        if queued.available() {
            if let Some(pipe) = &stdout {
                polls.push(poll_descriptor(pipe.as_raw_fd(), libc::POLLIN));
            }
            if let Some(pipe) = &stderr {
                polls.push(poll_descriptor(pipe.as_raw_fd(), libc::POLLIN));
            }
        }
        let buffered = reading && incoming.buffered()
            || stdout.is_some() && queued.available() && outgoing.buffered();
        // SAFETY: the pointer and count describe the live `polls` vector.
        let result = unsafe {
            libc::poll(
                polls.as_mut_ptr(),
                polls.len() as _,
                if buffered { 0 } else { 50 },
            )
        };
        if result < 0 {
            let error = io::Error::last_os_error();
            if !pending(&error) {
                return Err(error);
            }
        }
        if polls[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0
            || read_closed(&socket, polls[0].revents)?
        {
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
    use newport_protocol::encoding::protobuf;
    use std::{
        net::Shutdown,
        os::unix::process::CommandExt,
        process::{Command, Stdio},
        sync::mpsc,
    };
    struct Fixture {
        socket: UnixStream,
        transport: Transport,
        frames: FramedInput,
        decoder: wire::ServerDecoder,
        stop: Cancellation,
        done: mpsc::Receiver<io::Result<()>>,
    }
    impl Fixture {
        fn start(command: &mut Command) -> Self {
            Self::with(command, Transport::Opaque)
        }
        fn with(command: &mut Command, transport: Transport) -> Self {
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
                let result = relay(server, &mut process, &stopping, transport);
                drop(process);
                let _ = done.send(result);
            });
            Self {
                socket,
                transport,
                frames: FramedInput::new(transport.limits(Direction::FromWorker).message),
                decoder: wire::ServerDecoder::default(),
                stop,
                done: result,
            }
        }
        fn control(&mut self, control: ClientControl) {
            self.socket
                .write_all(&wire::encode_client(control).unwrap())
                .unwrap();
        }
        fn data(&mut self, data: &[u8]) {
            match self.transport {
                Transport::Opaque => {
                    for bytes in data.chunks(worker::MAX_CHUNK) {
                        self.socket
                            .write_all(
                                &wire::encode_opaque(
                                    &Chunk::new(Bytes::copy_from_slice(bytes)).unwrap(),
                                    Direction::ToWorker,
                                )
                                .unwrap(),
                            )
                            .unwrap();
                    }
                }
                Transport::Protobuf { .. } => self.socket.write_all(data).unwrap(),
            }
        }
        fn read(&mut self) -> Frame<ServerControl> {
            loop {
                match self.frames.receive(&mut self.socket).unwrap() {
                    ReadFrame::Frame(frame) => {
                        return match self
                            .decoder
                            .decode(frame.body(), &self.transport.limits(Direction::FromWorker))
                            .unwrap()
                        {
                            Frame::Control(control) => Frame::Control(control),
                            Frame::Service(body) => Frame::Service(match self.transport {
                                Transport::Opaque => wire::decode_opaque(body).unwrap().into_data(),
                                Transport::Protobuf { .. } => frame.into_bytes(),
                            }),
                        };
                    }
                    ReadFrame::Partial => (),
                    ReadFrame::End => panic!("worker ended without an exit"),
                }
            }
        }
        fn finished(&self) {
            self.done
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap();
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
        fixture.data(&expected);
        fixture.control(ClientControl::InputComplete);
        let mut output = vec![];
        let mut stderr = vec![];
        loop {
            match fixture.read() {
                Frame::Service(data) => output.extend(data),
                Frame::Control(ServerControl::Stderr(data)) => stderr.extend(data.into_data()),
                Frame::Control(ServerControl::Exit(exit)) => {
                    assert_eq!(exit, Exit::Status(23));
                    break;
                }
            }
        }
        assert_eq!(output, expected);
        assert_eq!(stderr, b"diagnostic");
        fixture.finished();
    }
    #[test]
    fn relay_rejects_wrong_direction_and_data_after_half_close() {
        for after_eof in [false, true] {
            let mut command = Command::new("/bin/sleep");
            command.arg("30");
            let mut fixture = Fixture::start(&mut command);
            if after_eof {
                fixture.control(ClientControl::InputComplete);
                fixture.data(&[1]);
            } else {
                fixture
                    .socket
                    .write_all(&wire::encode_server(&ServerControl::Exit(Exit::Status(0))).unwrap())
                    .unwrap();
            }
            assert_eq!(
                fixture
                    .done
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap()
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidData
            );
        }
    }
    #[test]
    fn relay_cancels_when_input_is_blocked_or_the_client_disappears() {
        for disconnect in [false, true] {
            let mut command = Command::new("/bin/sleep");
            command.arg("30");
            let mut fixture = Fixture::start(&mut command);
            fixture.data(&vec![0; worker::MAX_CHUNK]);
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
    fn transport_half_close_cancels_a_worker_with_blocked_stdin() {
        let limits = newport_protocol::monitor::SERVER_LIMITS;
        let mut frame = Vec::new();
        protobuf::write_message(
            &mut frame,
            &protobuf::Secret {
                value: vec![0; worker::MAX_CHUNK * 64].into(),
            },
            &limits,
        )
        .unwrap();
        let mut command = Command::new("/bin/sleep");
        command.arg("30");
        let mut fixture = Fixture::with(
            &mut command,
            Transport::Protobuf {
                client: limits,
                server: limits,
            },
        );
        fixture.data(&frame);
        fixture.socket.shutdown(Shutdown::Write).unwrap();
        assert!(
            fixture
                .done
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .is_err()
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn read_half_close_is_detected_without_consuming_queued_data() {
        let (mut socket, mut peer) = UnixStream::pair().unwrap();
        peer.write_all(b"unread").unwrap();
        assert!(!read_closed(&socket, 0).unwrap());
        peer.shutdown(Shutdown::Write).unwrap();
        let mut event = poll_descriptor(socket.as_raw_fd(), READ_CLOSED);
        // SAFETY: one initialized pollfd for the call.
        assert_eq!(unsafe { libc::poll(&mut event, 1, 1000) }, 1);
        assert!(read_closed(&socket, event.revents).unwrap());
        let mut bytes = [0; 6];
        socket.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"unread");
    }
    #[test]
    fn relay_accepts_signals_after_half_close_and_reports_the_signal() {
        let mut command = Command::new("/bin/sleep");
        command.arg("30");
        let mut fixture = Fixture::start(&mut command);
        fixture.control(ClientControl::InputComplete);
        let signal = Signal::new(15).unwrap();
        fixture.control(ClientControl::Signal(signal));
        assert_eq!(
            fixture.read(),
            Frame::Control(ServerControl::Exit(Exit::Signal(signal)))
        );
        fixture.finished();
    }
    #[test]
    fn relay_reports_a_signal_and_does_not_treat_transport_eof_as_half_close() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "kill -TERM $$"]);
        let mut fixture = Fixture::start(&mut command);
        assert_eq!(
            fixture.read(),
            Frame::Control(ServerControl::Exit(Exit::Signal(Signal::new(15).unwrap())))
        );
        fixture.finished();
        let mut command = Command::new("/bin/sleep");
        command.arg("30");
        let mut fixture = Fixture::start(&mut command);
        fixture.control(ClientControl::InputComplete);
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
            match quick.read() {
                Frame::Control(ServerControl::Exit(exit)) => {
                    assert_eq!(exit, Exit::Status(0));
                    break;
                }
                Frame::Service(data) => bytes.extend(data),
                other => panic!("unexpected frame: {other:?}"),
            }
        }
        assert_eq!(bytes, b"alive\n");
        quick.finished();
        assert_eq!(
            loud.done
                .recv_timeout(Duration::from_secs(8))
                .unwrap()
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
    }
    fn native_frame() -> Vec<u8> {
        use newport_protocol::git::{self, wire::Message};
        git::ClientMessage::Call(git::Call {
            id: uuid::Uuid::new_v4().to_string(),
            request: Box::new(git::Request::Start {
                operation_id: uuid::Uuid::new_v4().to_string(),
                repo_id: "repo".into(),
                expected_snapshot: "snapshot".into(),
                action: git::Action::Stage {
                    entry_ids: vec!["x".repeat(128); 1024],
                    hunks: None,
                },
            }),
        })
        .encode()
        .unwrap()
    }
    #[test]
    fn native_frames_larger_than_a_chunk_keep_their_exact_prefix_and_body() {
        let mut bytes = native_frame();
        assert!(bytes.len() > worker::MAX_CHUNK);
        let last = bytes.iter().position(|byte| byte & 0x80 == 0).unwrap();
        bytes[last] |= 0x80;
        bytes.insert(last + 1, 0);
        let mut command = Command::new("/bin/cat");
        let mut fixture = Fixture::with(&mut command, Transport::GIT);
        fixture.data(&bytes[..1]);
        fixture.data(&bytes[1..]);
        fixture.control(ClientControl::InputComplete);
        assert_eq!(fixture.read(), Frame::Service(bytes.into()));
        assert_eq!(
            fixture.read(),
            Frame::Control(ServerControl::Exit(Exit::Status(0)))
        );
        fixture.finished();
    }
    #[test]
    fn native_worker_output_cannot_forge_controls_or_end_mid_frame() {
        for bytes in [
            wire::encode_server(&ServerControl::Exit(Exit::Status(0))).unwrap(),
            vec![0x80],
            vec![3, 0x0a],
        ] {
            let mut file = tempfile::NamedTempFile::new().unwrap();
            file.write_all(&bytes).unwrap();
            let mut command = Command::new("/bin/cat");
            command.arg(file.path());
            let fixture = Fixture::with(&mut command, Transport::GIT);
            assert_eq!(
                fixture
                    .done
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap()
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidData
            );
        }
    }
    #[test]
    fn mixed_controls_are_rejected_before_native_data_reaches_the_child() {
        let service = protobuf::read_frame(
            &mut native_frame().as_slice(),
            newport_protocol::git::MAX_FRAME,
        )
        .unwrap()
        .unwrap();
        let control = protobuf::read_frame(
            &mut wire::encode_client(ClientControl::InputComplete)
                .unwrap()
                .as_slice(),
            wire::CONTROL_LIMITS.message,
        )
        .unwrap()
        .unwrap();
        let mut mixed = Vec::new();
        protobuf::write_frame(&mut mixed, &[service.as_ref(), control.as_ref()].concat()).unwrap();
        let mut command = Command::new("/bin/cat");
        let mut fixture = Fixture::with(&mut command, Transport::GIT);
        fixture.data(&mixed);
        assert_eq!(
            fixture
                .done
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        let mut output = Vec::new();
        fixture.socket.read_to_end(&mut output).unwrap();
        assert!(output.is_empty());
    }
    #[test]
    fn large_native_frame_exhausts_queue_capacity_until_the_frame_is_removed() {
        let mut output = Output::new();
        let size = QUEUED_BYTES + 1;
        output.push(vec![0; size].into()).unwrap();
        assert!(!output.available());
        assert!(output.push(vec![1].into()).is_err());
        assert_eq!(output.bytes, size);
        output.frames.front_mut().unwrap().used = size - 1;
        assert!(!output.available());
    }
}
