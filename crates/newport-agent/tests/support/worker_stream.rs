use super::runtime_reply::Expected;
use super::{GIT, Runtime, fixture_home, protocol};
use newport_protocol::git;
use newport_protocol::{
    encoding::protobuf::{EncodedFrame, FrameDecoder},
    runtime::wire as codec,
    worker::{self, ClientControl, Direction, Exit, ServerControl, Transport, admission, wire},
};
use std::{
    io::{self, Read, Write},
    os::unix::{fs::MetadataExt, net::UnixStream},
    path::Path,
    time::{Duration, Instant},
};

fn configure_git(input: &mut impl Write) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    git::wire::write(
        input,
        &git::ClientMessage::Configure(git::Configure {
            id: id.clone(),
            client_id: uuid::Uuid::new_v4().to_string(),
            client_version: "test".into(),
            command_logs: false,
        }),
    )
    .unwrap();
    id
}
fn prepare(runtime: &mut Runtime, arguments: Vec<String>) -> (admission::Open, String) {
    let worker = uuid::Uuid::new_v4().to_string();
    let reply = runtime.request(
        runtime.hello.instance_id.clone(),
        protocol::Operation::PrepareWorkerStream {
            worker_id: worker.clone(),
            service: "git".into(),
            version: GIT,
            contract_id: None,
        },
    );
    assert!(reply.result.is_ok(), "{:?}", reply.result.as_ref().err());
    let endpoint = reply.result.unwrap().endpoint();
    endpoint.validate().unwrap();
    (
        admission::Open {
            version: admission::VERSION,
            runtime_id: runtime.hello.instance_id.clone(),
            worker_id: worker,
            build_sha256: runtime.hello.build_sha256.clone(),
            ticket: endpoint.ticket.clone(),
            service: "git".into(),
            service_version: GIT,
            contract_id: None,
            arguments,
        },
        endpoint.path,
    )
}
fn connect(open: &admission::Open, path: &str) -> (UnixStream, admission::Admission) {
    let mut socket = UnixStream::connect(path).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    socket
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    codec::write(&mut socket, open).unwrap();
    let reply: admission::Admission = codec::read(&mut socket).unwrap();
    reply.validate_for(open).unwrap();
    (socket, reply)
}
struct Stream {
    socket: UnixStream,
    pending: io::Cursor<bytes::Bytes>,
    frames: FrameDecoder,
    buffered: Vec<u8>,
    consumed: usize,
    decoder: wire::ServerDecoder,
    stderr: Vec<u8>,
    status: Option<u32>,
}
impl Stream {
    fn start(open: &admission::Open, path: &str) -> Self {
        let (socket, reply) = connect(open, path);
        assert!(matches!(reply, admission::Admission::Started(_)));
        Self {
            socket,
            pending: io::Cursor::new(bytes::Bytes::new()),
            frames: FrameDecoder::new(Transport::GIT.limits(Direction::FromWorker).message),
            buffered: vec![],
            consumed: 0,
            decoder: wire::ServerDecoder::default(),
            stderr: vec![],
            status: None,
        }
    }
    fn finish_input(&mut self) {
        self.socket
            .write_all(&wire::encode_client(ClientControl::InputComplete).unwrap())
            .unwrap();
    }
    fn frame(&mut self) -> io::Result<EncodedFrame> {
        loop {
            let mut input = &self.buffered[self.consumed..];
            let frame = self.frames.consume(&mut input).map_err(io::Error::other)?;
            self.consumed = self.buffered.len() - input.len();
            if let Some(frame) = frame {
                return Ok(frame);
            }
            self.buffered.resize(worker::MAX_CHUNK, 0);
            let count = self.socket.read(&mut self.buffered)?;
            if count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Worker ended without final status",
                ));
            }
            self.buffered.truncate(count);
            self.consumed = 0;
        }
    }
    fn initialize_git(&mut self) {
        let id = configure_git(self);
        assert!(
            matches!(git::wire::read::<git::ServerMessage>(self).unwrap(), git::ServerMessage::Ready(git::Ready { id: actual, .. }) if actual == id)
        );
    }
}
impl Read for Stream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        loop {
            let count = self.pending.read(bytes)?;
            if count != 0 || self.status.is_some() {
                return Ok(count);
            }
            let frame = self.frame()?;
            match self
                .decoder
                .decode(frame.body(), &Transport::GIT.limits(Direction::FromWorker))
                .map_err(io::Error::other)?
            {
                wire::Frame::Service(_) => self.pending = io::Cursor::new(frame.into_bytes()),
                wire::Frame::Control(ServerControl::Stderr(data)) => {
                    self.stderr.extend(data.into_data());
                    assert!(self.stderr.len() <= 65536);
                }
                wire::Frame::Control(ServerControl::Exit(Exit::Status(status))) => {
                    self.status = Some(status)
                }
                wire::Frame::Control(ServerControl::Exit(Exit::Signal(_))) => {
                    return Err(io::Error::other("Unexpected worker signal"));
                }
            }
        }
    }
}
impl Write for Stream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        self.socket.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.socket.flush()
    }
}
fn ping(runtime: &mut Runtime) {
    let started = Instant::now();
    assert!(
        runtime
            .request(runtime.hello.instance_id.clone(), protocol::Operation::Ping)
            .result
            .is_ok()
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn forwarded_workers_reexec_the_pinned_binary_and_keep_control_independent() {
    let home = fixture_home();
    let mut runtime = Runtime::start(home.path(), "/usr/bin:/bin");
    runtime
        .hello
        .require(admission::CAPABILITY, admission::VERSION)
        .unwrap();
    runtime.initialize();
    let (open, path) = prepare(&mut runtime, vec![]);
    let metadata = std::fs::symlink_metadata(&path).unwrap();
    assert_eq!(metadata.mode() & 0o777, 0o600);
    // SAFETY: geteuid has no preconditions and cannot fail.
    assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
    assert_eq!(
        std::fs::metadata(Path::new(&path).parent().unwrap())
            .unwrap()
            .mode()
            & 0o777,
        0o700
    );
    for mutate in [
        |v: &mut admission::Open| v.ticket.value = "f".repeat(64).into_bytes().into(),
        |v: &mut admission::Open| v.runtime_id = uuid::Uuid::new_v4().to_string(),
        |v: &mut admission::Open| v.build_sha256 = "e".repeat(64),
        |v: &mut admission::Open| v.service = "integration".into(),
        |v: &mut admission::Open| v.service_version += 1,
        |v: &mut admission::Open| v.contract_id = Some("0".repeat(64)),
    ] {
        let mut rejected = open.clone();
        mutate(&mut rejected);
        assert!(matches!(
            connect(&rejected, &path).1,
            admission::Admission::Refused(_)
        ));
    }
    let mut first = Stream::start(&open, &path);
    first.initialize_git();
    assert!(
        matches!(connect(&open, &path).1, admission::Admission::Refused(error) if error.code == "consumed")
    );
    let (other, other_path) = prepare(&mut runtime, vec![]);
    assert_eq!(path, other_path);
    assert_ne!(open.ticket, other.ticket);
    let mut second = Stream::start(&other, &path);
    second.initialize_git();
    first.finish_input();
    let mut remaining = vec![];
    first.read_to_end(&mut remaining).unwrap();
    assert_eq!(first.status, Some(0));
    assert!(first.stderr.is_empty());
    ping(&mut runtime);
    second.finish_input();
    second.read_to_end(&mut remaining).unwrap();
    assert_eq!(second.status, Some(0));
    runtime.input.take();
    runtime.stopped();
    assert!(!Path::new(&path).exists());
}

#[test]
fn forwarded_spawn_admission_is_distinct_from_service_success_and_disconnect() {
    let home = fixture_home();
    let mut runtime = Runtime::start(home.path(), "/usr/bin:/bin");
    runtime.initialize();
    let (invalid, path) = prepare(&mut runtime, vec!["unexpected-argument".into()]);
    let mut rejected = Stream::start(&invalid, &path);
    let mut bytes = vec![];
    rejected.read_to_end(&mut bytes).unwrap();
    assert_eq!(rejected.status, Some(1));
    assert!(!rejected.stderr.is_empty());
    ping(&mut runtime);
    let (open, _) = prepare(&mut runtime, vec![]);
    let mut active = Stream::start(&open, &path);
    active.initialize_git();
    // Wrong-direction framing closes this worker without a success exit frame.
    active
        .socket
        .write_all(&wire::encode_server(&ServerControl::Exit(Exit::Status(0))).unwrap())
        .unwrap();
    assert!(active.read(&mut [0; 1]).is_err());
    assert!(active.status.is_none());
    ping(&mut runtime);
    let (next, _) = prepare(&mut runtime, vec![]);
    let mut active = Stream::start(&next, &path);
    active.initialize_git();
    runtime.input.take();
    runtime.stopped();
    // Runtime control EOF revokes a still-open forwarded worker.
    let _ = active.read_to_end(&mut bytes);
    assert!(!Path::new(&path).exists());
}

#[test]
fn forwarded_slow_admissions_and_worker_capacity_do_not_stall_control() {
    let home = fixture_home();
    let mut runtime = Runtime::start(home.path(), "/usr/bin:/bin");
    runtime.initialize();
    let (open, path) = prepare(&mut runtime, vec![]);
    let mut slow = vec![];
    for _ in 0..protocol::MAX_WORKERS + 4 {
        let mut socket = UnixStream::connect(&path).unwrap();
        socket.write_all(&[0]).unwrap();
        slow.push(socket);
    }
    ping(&mut runtime);
    // Each opening has a one-second whole-handshake deadline, including partial
    // headers; capacity becomes usable again without restarting the runtime.
    let mut active = Stream::start(&open, &path);
    active.initialize_git();
    for _ in 1..protocol::MAX_WORKERS {
        prepare(&mut runtime, vec![]);
    }
    let full = runtime.request(
        runtime.hello.instance_id.clone(),
        protocol::Operation::PrepareWorkerStream {
            worker_id: uuid::Uuid::new_v4().to_string(),
            service: "git".into(),
            version: GIT,
            contract_id: None,
        },
    );
    assert!(full.result.is_err());
    assert!(full.result.is_err());
    ping(&mut runtime);
    drop(slow);
    runtime.input.take();
    runtime.stopped();
    assert!(!Path::new(&path).exists());
}

#[test]
fn exec_fallback_retires_forwarding_tickets_without_waiting_for_expiry() {
    let home = fixture_home();
    let mut runtime = Runtime::start(home.path(), "/usr/bin:/bin");
    runtime.initialize();
    // More fallback workers than the pending ticket limit must be admitted
    // sequentially without the ticket expiry reclaiming their capacity.
    for _ in 0..protocol::MAX_WORKERS + 4 {
        let prepared = Instant::now();
        let (open, path) = prepare(&mut runtime, vec![]);
        let mut worker = runtime.worker(&open.worker_id, "git", home.path());
        let mut output = worker.stdout.take().unwrap();

        let id = configure_git(worker.stdin.as_mut().unwrap());
        assert!(
            matches!(git::wire::read::<git::ServerMessage>(&mut output).unwrap(), git::ServerMessage::Ready(git::Ready { id: actual, .. }) if actual == id)
        );
        assert!(
            matches!(connect(&open, &path).1, admission::Admission::Refused(error) if error.code == "consumed")
        );
        // An expired ticket is also refused as consumed; only a fresh one proves retirement.
        assert!(
            prepared.elapsed() < Duration::from_secs(20),
            "Test crossed ticket expiry"
        );
        worker.stdin.take();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = worker.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < deadline, "Fallback worker did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    ping(&mut runtime);
    runtime.input.take();
    runtime.stopped();
}
