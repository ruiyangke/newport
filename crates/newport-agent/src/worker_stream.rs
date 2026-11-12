//! Runtime-owned admission for forwarded workers. No private service code or
//! feature data enters the runtime control loop.
use crate::{local_socket, owned_directory, transport::Cancellation};
use newport_protocol::{
    common,
    runtime::{self as control, wire as codec},
    worker::{
        Transport,
        admission::{self as wire, Endpoint},
    },
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::{self, Read, Write},
    os::{fd::AsRawFd, unix::net::UnixStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::{Duration, Instant},
};

const ADMISSION_TIME: Duration = Duration::from_secs(1);
const TICKET_TIME: Duration = Duration::from_secs(30);
const CONNECTIONS: usize = control::MAX_WORKERS + 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refusal {
    Invalid,
    Incompatible,
    Stale,
    Expired,
    Consumed,
    Unavailable,
}
impl From<Refusal> for common::Error {
    fn from(reason: Refusal) -> Self {
        let (code, message) = match reason {
            Refusal::Invalid => (
                "invalid",
                "Worker admission does not match its registration",
            ),
            Refusal::Incompatible => (
                "incompatible",
                "Newport and Needle use different service protocols. Update Newport and Needle.",
            ),
            Refusal::Stale => (
                "stale",
                "Worker admission belongs to a different runtime or build",
            ),
            Refusal::Expired => ("expired", "Worker admission ticket expired"),
            Refusal::Consumed => ("consumed", "Worker registration was already consumed"),
            Refusal::Unavailable => ("unavailable", "Worker process could not start"),
        };
        Self {
            code: code.into(),
            message: message.into(),
            retry: common::Retry::No,
        }
    }
}

struct Ticket {
    secret: common::Secret,
    service: String,
    service_version: u32,
    contract_id: Option<String>,
    transport: Transport,
    expires: Instant,
}
#[derive(Default)]
struct Tickets {
    pending: HashMap<String, Ticket>,
    seen: HashSet<String>,
    active: usize,
}
impl Tickets {
    fn expire(&mut self) {
        self.pending
            .retain(|_, ticket| ticket.expires > Instant::now());
    }
    fn prepare(&mut self, request: &wire::Open, transport: Transport) -> io::Result<()> {
        request.validate().map_err(io::Error::other)?;
        self.expire();
        if self.seen.contains(&request.worker_id)
            || self.seen.len() >= control::MAX_REGISTRATIONS
            || self.pending.len() + self.active >= control::MAX_WORKERS
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Worker stream admission is busy or already consumed",
            ));
        }
        self.seen.insert(request.worker_id.clone());
        self.pending.insert(
            request.worker_id.clone(),
            Ticket {
                secret: request.ticket.clone(),
                service: request.service.clone(),
                service_version: request.service_version,
                contract_id: request.contract_id.clone(),
                transport,
                expires: Instant::now() + TICKET_TIME,
            },
        );
        Ok(())
    }
    fn admit(
        &mut self,
        open: &wire::Open,
        runtime: &str,
        build: &str,
    ) -> Result<Transport, Refusal> {
        if open.runtime_id != runtime || open.build_sha256 != build {
            return Err(Refusal::Stale);
        }
        let ticket = self.pending.get(&open.worker_id).ok_or_else(|| {
            if self.seen.contains(&open.worker_id) {
                Refusal::Consumed
            } else {
                Refusal::Invalid
            }
        })?;
        if !same_ticket(&ticket.secret.value, &open.ticket.value)
            || ticket.service != open.service
            || ticket.service_version != open.service_version
        {
            return Err(Refusal::Invalid);
        }
        if ticket.expires <= Instant::now() {
            self.pending.remove(&open.worker_id);
            return Err(Refusal::Expired);
        }
        if ticket.contract_id != open.contract_id {
            return Err(Refusal::Incompatible);
        }
        let transport = ticket.transport;
        self.pending.remove(&open.worker_id);
        self.active += 1;
        Ok(transport)
    }
}
fn same_ticket(expected: &[u8], actual: &[u8]) -> bool {
    if expected.len() != actual.len() {
        return false;
    }
    let difference = expected
        .iter()
        .zip(actual.iter())
        .fold(0_u8, |difference, (a, b)| {
            difference | std::hint::black_box(a ^ b)
        });
    difference == 0
}
struct Active(Arc<Mutex<Tickets>>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active -= 1;
    }
}

pub(crate) struct Server {
    path: PathBuf,
    runtime: String,
    build: String,
    tickets: Arc<Mutex<Tickets>>,
    transports: Arc<BTreeMap<String, Transport>>,
    stop: Cancellation,
    task: Option<JoinHandle<()>>,
}
impl Server {
    pub(crate) fn cancel_pending(&self, worker: &str) {
        // Keep `seen`: a retired ticket must never authorize another process.
        self.tickets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending
            .remove(worker);
    }
    pub(crate) fn start(
        worker_socket: &Path,
        runtime: &str,
        build: &str,
        stop: Cancellation,
        transports: Arc<BTreeMap<String, Transport>>,
    ) -> io::Result<Self> {
        let directory = owned_directory::Directory::join(
            worker_socket
                .parent()
                .ok_or_else(|| io::Error::other("Missing runtime directory"))?,
        )?;
        let path = directory.path().join("streams.sock");
        let (listener, endpoint) = local_socket::Endpoint::fixed(&path)?;
        let tickets = Arc::new(Mutex::new(Tickets::default()));
        let registry = tickets.clone();
        let stopping = stop.clone();
        let identity = runtime.to_owned();
        let digest = build.to_owned();
        let worker_socket = worker_socket.to_owned();
        // Linux re-exec resolves this running inode, even during activation of
        // a different build. Every child also verifies the negotiated hash.
        #[cfg(target_os = "linux")]
        let executable = PathBuf::from("/proc/self/exe");
        #[cfg(not(target_os = "linux"))]
        let executable = std::env::current_exe()?;
        let task = std::thread::Builder::new()
            .name("worker-stream-listener".into())
            .spawn(move || {
                let mut tasks: Vec<JoinHandle<()>> = vec![];
                while stopping.check().is_ok() {
                    let mut index = 0;
                    while index < tasks.len() {
                        if tasks[index].is_finished() {
                            let _ = tasks.swap_remove(index).join();
                        } else {
                            index += 1;
                        }
                    }
                    registry
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .expire();
                    if tasks.len() < CONNECTIONS
                        && let Ok((stream, _)) = listener.accept()
                    {
                        let registry = registry.clone();
                        let stop = stopping.clone();
                        let runtime = identity.clone();
                        let build = digest.clone();
                        let socket = worker_socket.clone();
                        let executable = executable.clone();
                        if let Ok(task) = std::thread::Builder::new()
                            .name("worker-stream".into())
                            .spawn(move || {
                                // Never log admission bytes, credentials or service payloads.
                                let _ = serve(
                                    stream,
                                    registry,
                                    stop,
                                    &runtime,
                                    &build,
                                    &socket,
                                    &executable,
                                );
                            })
                        {
                            tasks.push(task);
                        }
                        continue;
                    }
                    let mut event = libc::pollfd {
                        fd: listener.as_raw_fd(),
                        events: if tasks.len() < CONNECTIONS {
                            libc::POLLIN
                        } else {
                            0
                        },
                        revents: 0,
                    };
                    // SAFETY: one initialized pollfd for the call.
                    if unsafe { libc::poll(&mut event, 1, 50) } < 0 {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
                drop(listener);
                drop(endpoint);
                for task in tasks {
                    let _ = task.join();
                }
                // Preserve ownership if a child or admission is still being cleaned up.
                drop(directory);
            })?;
        Ok(Self {
            path,
            runtime: runtime.into(),
            build: build.into(),
            tickets,
            transports,
            stop,
            task: Some(task),
        })
    }
    pub(crate) fn prepare(
        &self,
        worker: &str,
        service: &str,
        version: u32,
        contract_id: Option<&str>,
    ) -> io::Result<Endpoint> {
        self.stop.check()?;
        let transport = self.transports.get(service).copied().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Service has no registered worker transport",
            )
        })?;
        let mut random = [0_u8; 32];
        getrandom::fill(&mut random).map_err(|error| io::Error::other(error.to_string()))?;
        let ticket: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let open = wire::Open {
            version: wire::VERSION,
            runtime_id: self.runtime.clone(),
            worker_id: worker.into(),
            build_sha256: self.build.clone(),
            ticket: common::Secret {
                value: ticket.clone().into_bytes().into(),
            },
            service: service.into(),
            service_version: version,
            contract_id: contract_id.map(str::to_owned),
            arguments: vec![],
        };
        let endpoint = Endpoint {
            path: self
                .path
                .to_str()
                .ok_or_else(|| io::Error::other("Invalid worker stream path"))?
                .into(),
            ticket: open.ticket.clone(),
        };
        endpoint.validate().map_err(io::Error::other)?;
        self.tickets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .prepare(&open, transport)?;
        Ok(endpoint)
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(task) = self.task.take() {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !task.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if task.is_finished() {
                let _ = task.join();
            } else {
                // A kernel-stuck child cannot be synchronously reaped. Keep its
                // directory lease with the cleanup thread; never report success.
                eprintln!("newport-agent: worker stream cleanup is still pending");
            }
        }
    }
}

struct Handshake<'a> {
    stream: &'a mut UnixStream,
    deadline: Instant,
    stop: &'a Cancellation,
}
impl Handshake<'_> {
    fn remaining(&self) -> io::Result<Duration> {
        self.stop.check()?;
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Worker admission timed out",
            ));
        }
        Ok(remaining)
    }
}
impl Read for Handshake<'_> {
    fn read(&mut self, data: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(Some(self.remaining()?))?;
        self.stream.read(data)
    }
}
impl Write for Handshake<'_> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.stream.set_write_timeout(Some(self.remaining()?))?;
        self.stream.write(data)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn serve(
    mut stream: UnixStream,
    tickets: Arc<Mutex<Tickets>>,
    stop: Cancellation,
    runtime: &str,
    build: &str,
    worker_socket: &Path,
    executable: &Path,
) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    let mut handshake = Handshake {
        stream: &mut stream,
        deadline: Instant::now() + ADMISSION_TIME,
        stop: &stop,
    };
    let open: wire::Open = codec::read(&mut handshake)?;
    let transport = match tickets
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .admit(&open, runtime, build)
    {
        Ok(transport) => transport,
        Err(reason) => {
            return codec::write(&mut handshake, &wire::Admission::Refused(reason.into()))
                .map_err(io::Error::from);
        }
    };
    let _active = Active(tickets);
    stop.check()?;
    use std::os::unix::process::CommandExt;
    let child = Command::new(executable)
        .args(["worker", build, runtime])
        .arg(worker_socket)
        .arg(&open.worker_id)
        .arg(&open.service)
        .args(&open.arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn();
    let child = match child {
        Ok(child) => child,
        Err(_) => {
            return codec::write(
                &mut handshake,
                &wire::Admission::Refused(Refusal::Unavailable.into()),
            )
            .map_err(io::Error::from);
        }
    };
    let mut process = crate::worker_stream_io::Process::new(child);
    codec::write(
        &mut handshake,
        &wire::Admission::Started(wire::Started {
            runtime_id: runtime.into(),
            worker_id: open.worker_id,
            build_sha256: build.into(),
        }),
    )?;
    crate::worker_stream_io::relay(stream, &mut process, &stop, transport)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn opening() -> wire::Open {
        wire::Open {
            version: wire::VERSION,
            runtime_id: uuid::Uuid::new_v4().to_string(),
            worker_id: uuid::Uuid::new_v4().to_string(),
            build_sha256: "a".repeat(64),
            ticket: common::Secret {
                value: "b".repeat(64).into_bytes().into(),
            },
            service: "fixture".into(),
            service_version: 1,
            contract_id: None,
            arguments: vec![],
        }
    }
    #[test]
    fn mismatched_contracts_cannot_consume_a_ticket_or_start_a_worker() {
        let mut tickets = Tickets::default();
        let mut open = opening();
        open.contract_id = Some(newport_protocol::monitor::CONTRACT_ID.into());
        tickets.prepare(&open, Transport::MONITOR).unwrap();
        for contract_id in [None, Some("0".repeat(64))] {
            let mut invalid = open.clone();
            invalid.contract_id = contract_id;
            let error = tickets
                .admit(&invalid, &open.runtime_id, &open.build_sha256)
                .unwrap_err();
            assert_eq!(error, Refusal::Incompatible);
            assert_eq!(common::Error::from(error).code, "incompatible");
            assert_eq!(tickets.active, 0);
            assert_eq!(tickets.pending.len(), 1);
        }
        assert_eq!(
            tickets
                .admit(&open, &open.runtime_id, &open.build_sha256)
                .unwrap(),
            Transport::MONITOR
        );
    }

    #[test]
    fn tickets_bind_single_use_identity_and_expiry_without_consuming_invalid_attempts() {
        let mut tickets = Tickets::default();
        let open = opening();
        tickets.prepare(&open, Transport::GIT).unwrap();
        for mutate in [
            |v: &mut wire::Open| v.ticket.value = "c".repeat(64).into_bytes().into(),
            |v: &mut wire::Open| v.runtime_id = uuid::Uuid::new_v4().to_string(),
            |v: &mut wire::Open| v.build_sha256 = "d".repeat(64),
            |v: &mut wire::Open| v.service = "other".into(),
            |v: &mut wire::Open| v.service_version += 1,
        ] {
            let mut invalid = open.clone();
            mutate(&mut invalid);
            assert!(
                tickets
                    .admit(&invalid, &open.runtime_id, &open.build_sha256)
                    .is_err()
            );
            assert_eq!(tickets.active, 0);
            assert_eq!(tickets.pending.len(), 1);
        }
        let mut admitted = open.clone();
        admitted.arguments = vec!["opaque".into()];
        assert_eq!(
            tickets
                .admit(&admitted, &open.runtime_id, &open.build_sha256)
                .unwrap(),
            Transport::GIT
        );
        assert_eq!(tickets.active, 1);
        assert_eq!(
            tickets.admit(&open, &open.runtime_id, &open.build_sha256),
            Err(Refusal::Consumed)
        );
        let expired = opening();
        tickets.prepare(&expired, Transport::GIT).unwrap();
        tickets.pending.get_mut(&expired.worker_id).unwrap().expires = Instant::now();
        assert_eq!(
            tickets.admit(&expired, &expired.runtime_id, &expired.build_sha256),
            Err(Refusal::Expired)
        );
        assert!(tickets.prepare(&expired, Transport::GIT).is_err());
        for _ in 1..control::MAX_WORKERS {
            tickets.prepare(&opening(), Transport::GIT).unwrap();
        }
        assert!(tickets.prepare(&opening(), Transport::GIT).is_err());
        tickets.pending.clear();
        while tickets.seen.len() < control::MAX_REGISTRATIONS {
            tickets.seen.insert(uuid::Uuid::new_v4().to_string());
        }
        assert!(tickets.prepare(&opening(), Transport::GIT).is_err());
    }
}
