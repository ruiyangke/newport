use super::{GIT, Runtime, fixture_home, protocol};
use newport_agent::{git::protocol as git, runtime_protocol::worker_stream as wire};
use std::{
    io::{self, Read, Write},
    os::unix::{fs::MetadataExt, net::UnixStream},
    path::Path,
    time::{Duration, Instant},
};

fn prepare(runtime: &mut Runtime, arguments: Vec<String>) -> (wire::Open, String) {
    let worker = uuid::Uuid::new_v4().to_string();
    let reply = runtime.request(
        runtime.hello.instance_id.clone(),
        protocol::Operation::PrepareWorkerStream {
            worker_id: worker.clone(),
            service: "git".into(),
            version: GIT,
        },
    );
    assert!(reply.error.is_none(), "{:?}", reply.error);
    let endpoint = reply.worker_stream.unwrap();
    endpoint.validate().unwrap();
    (
        wire::Open {
            version: wire::VERSION,
            runtime_id: runtime.hello.instance_id.clone(),
            worker_id: worker,
            build_sha256: runtime.hello.build_sha256.clone(),
            ticket: endpoint.ticket().into(),
            service: "git".into(),
            arguments,
        },
        endpoint.path,
    )
}
fn connect(open: &wire::Open, path: &str) -> (UnixStream, wire::Admission) {
    let mut socket = UnixStream::connect(path).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    socket
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    open.write(&mut socket).unwrap();
    let reply: wire::Admission = protocol::read(&mut socket).unwrap();
    reply.validate_for(open).unwrap();
    (socket, reply)
}
struct Stream {
    socket: UnixStream,
    pending: io::Cursor<Vec<u8>>,
    stderr: Vec<u8>,
    status: Option<u32>,
}
impl Stream {
    fn start(open: &wire::Open, path: &str) -> Self {
        let (socket, reply) = connect(open, path);
        assert!(matches!(reply, wire::Admission::Started { .. }));
        Self {
            socket,
            pending: io::Cursor::new(vec![]),
            stderr: vec![],
            status: None,
        }
    }
    fn finish_input(&mut self) {
        wire::Frame {
            kind: wire::Kind::InputEof,
            data: vec![],
        }
        .write(&mut self.socket, wire::Direction::ToWorker)
        .unwrap();
    }
    fn initialize_git(&mut self) {
        assert!(matches!(
            git::read(self).unwrap(),
            git::Message::Hello { .. }
        ));
        let id = uuid::Uuid::new_v4().to_string();
        git::write(
            self,
            &git::Message::Initialize {
                id: id.clone(),
                version: GIT,
                client_id: uuid::Uuid::new_v4().to_string(),
                client_version: "1".into(),
                command_logs: false,
            },
        )
        .unwrap();
        assert!(
            matches!(git::read(self).unwrap(), git::Message::Ready { id: actual, .. } if actual == id)
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
            let frame = wire::Frame::read(&mut self.socket, wire::Direction::FromWorker)?;
            match frame.kind {
                wire::Kind::Output => self.pending = io::Cursor::new(frame.data),
                wire::Kind::Error => {
                    self.stderr.extend(frame.data);
                    assert!(self.stderr.len() <= 65536);
                }
                wire::Kind::ExitStatus => {
                    self.status = Some(u32::from_be_bytes(frame.data.try_into().unwrap()))
                }
                _ => return Err(io::Error::other("Unexpected worker signal or frame")),
            }
        }
    }
}
impl Write for Stream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let count = bytes.len().min(wire::MAX_CHUNK);
        wire::Frame {
            kind: wire::Kind::Input,
            data: bytes[..count].to_vec(),
        }
        .write(&mut self.socket, wire::Direction::ToWorker)?;
        Ok(count)
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
            .error
            .is_none()
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn forwarded_workers_reexec_the_pinned_binary_and_keep_control_independent() {
    let home = fixture_home();
    let mut runtime = Runtime::start(home.path(), "/usr/bin:/bin");
    runtime
        .hello
        .require(wire::CAPABILITY, wire::VERSION)
        .unwrap();
    runtime.initialize();
    let (open, path) = prepare(&mut runtime, vec![]);
    let metadata = std::fs::symlink_metadata(&path).unwrap();
    assert_eq!(metadata.mode() & 0o777, 0o600);
    assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
    assert_eq!(
        std::fs::metadata(Path::new(&path).parent().unwrap())
            .unwrap()
            .mode()
            & 0o777,
        0o700
    );
    for mutate in [
        |v: &mut wire::Open| v.ticket = "f".repeat(64),
        |v: &mut wire::Open| v.runtime_id = uuid::Uuid::new_v4().to_string(),
        |v: &mut wire::Open| v.build_sha256 = "e".repeat(64),
        |v: &mut wire::Open| v.service = "integration".into(),
    ] {
        let mut rejected = open.clone();
        mutate(&mut rejected);
        assert!(matches!(
            connect(&rejected, &path).1,
            wire::Admission::Refused { .. }
        ));
    }
    let mut first = Stream::start(&open, &path);
    first.initialize_git();
    assert_eq!(
        connect(&open, &path).1,
        wire::Admission::Refused {
            reason: wire::Refusal::Consumed
        }
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
        .write_all(&[wire::Kind::Output as u8, 0, 0, 0, 1, 0])
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
        },
    );
    assert!(full.error.is_some());
    assert!(full.worker_stream.is_none());
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
    let started = Instant::now();
    for _ in 0..protocol::MAX_WORKERS + 4 {
        let (open, path) = prepare(&mut runtime, vec![]);
        let mut worker = runtime.worker(&open.worker_id, "git", home.path());
        let mut output = worker.stdout.take().unwrap();
        assert!(matches!(
            git::read(&mut output).unwrap(),
            git::Message::Hello { .. }
        ));
        let id = uuid::Uuid::new_v4().to_string();
        git::write(
            worker.stdin.as_mut().unwrap(),
            &git::Message::Initialize {
                id: id.clone(),
                version: GIT,
                client_id: uuid::Uuid::new_v4().to_string(),
                client_version: "1".into(),
                command_logs: false,
            },
        )
        .unwrap();
        assert!(
            matches!(git::read(&mut output).unwrap(), git::Message::Ready { id: actual, .. } if actual == id)
        );
        assert_eq!(
            connect(&open, &path).1,
            wire::Admission::Refused {
                reason: wire::Refusal::Consumed
            }
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
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "Test crossed ticket expiry"
    );
    ping(&mut runtime);
    runtime.input.take();
    runtime.stopped();
}
