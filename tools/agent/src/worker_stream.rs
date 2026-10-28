//! Runtime-owned admission for forwarded workers. No private service code or
//! feature data enters the runtime control loop.
use crate::{
    local_socket, owned_directory,
    runtime_protocol::{self as control, worker_stream as wire},
    transport::Cancellation,
};
use std::{
    collections::{HashMap, HashSet},
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

struct Ticket {
    secret: String,
    service: String,
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
    fn prepare(&mut self, request: &wire::Open) -> io::Result<()> {
        request.validate()?;
        self.expire();
        if self.seen.contains(&request.worker_id)
            || self.seen.len() >= control::MAX_REGISTRATIONS
            || self.pending.len() + self.active >= control::MAX_WORKERS
        {
            return Err(io::Error::other(
                "Worker stream admission is busy or already consumed",
            ));
        }
        self.seen.insert(request.worker_id.clone());
        self.pending.insert(
            request.worker_id.clone(),
            Ticket {
                secret: request.ticket.clone(),
                service: request.service.clone(),
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
    ) -> Result<(), wire::Refusal> {
        if open.runtime_id != runtime || open.build_sha256 != build {
            return Err(wire::Refusal::Stale);
        }
        let ticket = self.pending.get(&open.worker_id).ok_or_else(|| {
            if self.seen.contains(&open.worker_id) {
                wire::Refusal::Consumed
            } else {
                wire::Refusal::Invalid
            }
        })?;
        if !same_ticket(&ticket.secret, &open.ticket) || ticket.service != open.service {
            return Err(wire::Refusal::Invalid);
        }
        if ticket.expires <= Instant::now() {
            self.pending.remove(&open.worker_id);
            return Err(wire::Refusal::Expired);
        }
        self.pending.remove(&open.worker_id);
        self.active += 1;
        Ok(())
    }
}
fn same_ticket(expected: &str, actual: &str) -> bool {
    if expected.len() != actual.len() {
        return false;
    }
    let difference = expected
        .bytes()
        .zip(actual.bytes())
        .fold(0_u8, |difference, (a, b)| {
            difference | std::hint::black_box(a ^ b)
        });
    difference == 0
}
struct Active(Arc<Mutex<Tickets>>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.lock().unwrap().active -= 1;
    }
}

pub(crate) struct Server {
    path: PathBuf,
    runtime: String,
    build: String,
    tickets: Arc<Mutex<Tickets>>,
    stop: Cancellation,
    task: Option<JoinHandle<()>>,
}
impl Server {
    pub(crate) fn cancel_pending(&self, worker: &str) {
        // Keep `seen`: a retired ticket must never authorize another process.
        self.tickets.lock().unwrap().pending.remove(worker);
    }
    pub(crate) fn start(
        worker_socket: &Path,
        runtime: &str,
        build: &str,
        stop: Cancellation,
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
                    registry.lock().unwrap().expire();
                    if tasks.len() < CONNECTIONS {
                        if let Ok((stream, _)) = listener.accept() {
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
            stop,
            task: Some(task),
        })
    }
    pub(crate) fn prepare(&self, worker: &str, service: &str) -> io::Result<wire::Endpoint> {
        self.stop.check()?;
        let mut random = [0_u8; 32];
        getrandom::fill(&mut random).map_err(|error| io::Error::other(error.to_string()))?;
        let ticket: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let open = wire::Open {
            version: wire::VERSION,
            runtime_id: self.runtime.clone(),
            worker_id: worker.into(),
            build_sha256: self.build.clone(),
            ticket: ticket.clone(),
            service: service.into(),
            arguments: vec![],
        };
        let endpoint = wire::Endpoint::new(
            self.path
                .to_str()
                .ok_or_else(|| io::Error::other("Invalid worker stream path"))?
                .into(),
            ticket,
        )?;
        self.tickets.lock().unwrap().prepare(&open)?;
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
    let open = wire::Open::read(&mut handshake)?;
    let admission = tickets.lock().unwrap().admit(&open, runtime, build);
    if let Err(reason) = admission {
        return control::write(&mut handshake, &wire::Admission::Refused { reason });
    }
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
            return control::write(
                &mut handshake,
                &wire::Admission::Refused {
                    reason: wire::Refusal::Unavailable,
                },
            );
        }
    };
    let mut process = crate::worker_stream_io::Process::new(child);
    control::write(
        &mut handshake,
        &wire::Admission::Started {
            runtime_id: runtime.into(),
            worker_id: open.worker_id,
            build_sha256: build.into(),
        },
    )?;
    crate::worker_stream_io::relay(stream, &mut process, &stop)
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
            ticket: "b".repeat(64),
            service: "fixture".into(),
            arguments: vec![],
        }
    }
    #[test]
    fn tickets_bind_single_use_identity_and_expiry_without_consuming_invalid_attempts() {
        let mut tickets = Tickets::default();
        let open = opening();
        tickets.prepare(&open).unwrap();
        for mutate in [
            |v: &mut wire::Open| v.ticket = "c".repeat(64),
            |v: &mut wire::Open| v.runtime_id = uuid::Uuid::new_v4().to_string(),
            |v: &mut wire::Open| v.build_sha256 = "d".repeat(64),
            |v: &mut wire::Open| v.service = "other".into(),
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
        tickets
            .admit(&open, &open.runtime_id, &open.build_sha256)
            .unwrap();
        assert_eq!(tickets.active, 1);
        assert_eq!(
            tickets.admit(&open, &open.runtime_id, &open.build_sha256),
            Err(wire::Refusal::Consumed)
        );
        let expired = opening();
        tickets.prepare(&expired).unwrap();
        tickets.pending.get_mut(&expired.worker_id).unwrap().expires = Instant::now();
        assert_eq!(
            tickets.admit(&expired, &expired.runtime_id, &expired.build_sha256),
            Err(wire::Refusal::Expired)
        );
        assert!(tickets.prepare(&expired).is_err());
        for _ in 1..control::MAX_WORKERS {
            tickets.prepare(&opening()).unwrap();
        }
        assert!(tickets.prepare(&opening()).is_err());
        tickets.pending.clear();
        while tickets.seen.len() < control::MAX_REGISTRATIONS {
            tickets.seen.insert(uuid::Uuid::new_v4().to_string());
        }
        assert!(tickets.prepare(&opening()).is_err());
    }
}
