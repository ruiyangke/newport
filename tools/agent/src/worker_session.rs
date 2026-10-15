//! Session registration and lifetime leases on a private local socket. An
//! executable match alone is insufficient to join a running runtime.
use crate::{local_socket::Endpoint, runtime_protocol as protocol, transport::Cancellation};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{self, Read},
    net::Shutdown,
    os::fd::AsRawFd,
    os::unix::{
        fs::{FileTypeExt, MetadataExt},
        net::UnixStream,
    },
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Mutex},
    thread::JoinHandle,
    time::{Duration, Instant},
};

struct Pending {
    service: String,
    expires: Instant,
}
#[derive(Default)]
struct Registrations {
    pending: HashMap<String, Pending>,
    active: HashSet<String>,
    seen: HashSet<String>,
}
impl Registrations {
    fn expire(&mut self) {
        self.pending.retain(|_, p| Instant::now() < p.expires);
    }
    fn register(&mut self, id: &str, service: &str) -> io::Result<()> {
        self.expire();
        if uuid::Uuid::parse_str(id).is_err() || self.seen.contains(id) {
            return Err(io::Error::other("Invalid or reused worker registration"));
        }
        if self.pending.len() + self.active.len() >= protocol::MAX_WORKERS
            || self.seen.len() >= protocol::MAX_REGISTRATIONS
        {
            return Err(io::Error::other("Needle worker registration limit reached"));
        }
        self.seen.insert(id.into());
        self.pending.insert(
            id.into(),
            Pending {
                service: service.into(),
                expires: Instant::now() + Duration::from_secs(30),
            },
        );
        Ok(())
    }
    fn attach(&mut self, request: &protocol::Attach, runtime: &str, build: &str) -> io::Result<()> {
        self.expire();
        if request.runtime_id != runtime
            || request.build_sha256 != build
            || !self
                .pending
                .get(&request.worker_id)
                .is_some_and(|p| p.service == request.service)
        {
            return Err(io::Error::other(
                "Stale, unregistered, or mismatched Needle worker",
            ));
        }
        self.pending.remove(&request.worker_id);
        self.active.insert(request.worker_id.clone());
        Ok(())
    }
}
// One deadline covers the whole handshake, including clients that trickle bytes.
struct Handshake<'a> {
    stream: &'a mut UnixStream,
    deadline: Instant,
}
impl Read for Handshake<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Worker handshake timed out",
            ));
        }
        self.stream.set_read_timeout(Some(remaining))?;
        self.stream.read(bytes)
    }
}
pub(crate) struct PermissionCall {
    pub(crate) permissions: protocol::integration::Permissions,
    pub(crate) reply: mpsc::SyncSender<protocol::integration::Applied>,
}
struct Update {
    worker: String,
    permissions: protocol::integration::Permissions,
    reply: mpsc::SyncSender<io::Result<protocol::integration::Applied>>,
}
struct Active {
    id: String,
    service: String,
    stream: UnixStream,
}
impl Drop for Active {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}
pub(crate) struct Broker {
    session: Arc<Mutex<Option<String>>>,
    registrations: Arc<Mutex<Registrations>>,
    updates: mpsc::SyncSender<Update>,
    stop: Cancellation,
    task: Option<JoinHandle<()>>,
    path: PathBuf,
    _endpoint: Endpoint,
    _directory: crate::owned_directory::Directory,
}
impl Broker {
    pub(crate) fn new(runtime: &str, build: &str) -> io::Result<Self> {
        let directory = crate::owned_directory::Directory::new(&crate::paths::runtime()?, "r-")?;
        let path = directory.path().join("workers.sock");
        let (listener, endpoint) = Endpoint::fixed(&path)?;
        let registrations = Arc::new(Mutex::new(Registrations::default()));
        let registry = registrations.clone();
        let stop = Cancellation::default();
        let stopping = stop.clone();
        let runtime = runtime.to_owned();
        let build = build.to_owned();
        let (updates, incoming) = mpsc::sync_channel::<Update>(16);
        let session = Arc::new(Mutex::new(None));
        let identity = session.clone();
        let task = std::thread::spawn(move || {
            let mut active: Vec<Active> = Vec::new();
            while stopping.check().is_ok() {
                active.retain_mut(|worker| match worker.stream.read(&mut [0; 1]) {
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => true,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => true,
                    _ => {
                        registry.lock().unwrap().active.remove(&worker.id);
                        false
                    }
                });
                if let Ok(update) = incoming.try_recv() {
                    let result = if let Some(index) = active
                        .iter()
                        .position(|w| w.id == update.worker && w.service == "integration")
                    {
                        let worker = &mut active[index];
                        let request = protocol::WorkerPermissions {
                            runtime_id: runtime.clone(),
                            worker_id: worker.id.clone(),
                            permissions: update.permissions,
                        };
                        let result = (|| {
                            worker.stream.set_nonblocking(false)?;
                            worker
                                .stream
                                .set_write_timeout(Some(Duration::from_millis(250)))?;
                            protocol::write(&mut worker.stream, &request)?;
                            let applied: protocol::integration::Applied =
                                protocol::read(&mut Handshake {
                                    stream: &mut worker.stream,
                                    deadline: Instant::now() + Duration::from_millis(1500),
                                })?;
                            if applied.permissions.revision != update.permissions.revision
                                || applied.permissions.clipboard && !update.permissions.clipboard
                                || applied.permissions.browser && !update.permissions.browser
                            {
                                return Err(io::Error::other(
                                    "Invalid worker permission acknowledgement",
                                ));
                            }
                            worker.stream.set_nonblocking(true)?;
                            Ok(applied)
                        })();
                        if result.is_err() {
                            registry.lock().unwrap().active.remove(&worker.id);
                            active.remove(index);
                        }
                        result
                    } else {
                        Err(io::Error::other(
                            "Integration worker is not attached to this runtime",
                        ))
                    };
                    let _ = update.reply.send(result);
                }
                if let Ok((mut stream, _)) = listener.accept() {
                    let request = protocol::read::<protocol::Attach>(&mut Handshake {
                        stream: &mut stream,
                        deadline: Instant::now() + Duration::from_secs(1),
                    });
                    if let Ok(request) = request {
                        let result = registry.lock().unwrap().attach(&request, &runtime, &build);
                        let accepted = result.is_ok();
                        let reply = protocol::Attached {
                            session_id: identity.lock().unwrap().clone(),
                            runtime_id: runtime.clone(),
                            worker_id: request.worker_id.clone(),
                            error: result.err().map(|e| e.to_string()),
                        };
                        let sent = stream
                            .set_write_timeout(Some(Duration::from_millis(250)))
                            .and_then(|_| protocol::write(&mut stream, &reply))
                            .and_then(|_| stream.set_nonblocking(true));
                        if accepted && sent.is_ok() {
                            active.push(Active {
                                id: request.worker_id,
                                service: request.service,
                                stream,
                            });
                        } else if accepted {
                            registry.lock().unwrap().active.remove(&request.worker_id);
                        }
                    }
                } else {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            // Closing every lease revokes idle reads even if its SSH feature
            // channel is still open. Already running commands retain their journal.
            drop(active);
        });
        Ok(Self {
            session,
            registrations,
            updates,
            stop,
            task: Some(task),
            path,
            _endpoint: endpoint,
            _directory: directory,
        })
    }
    pub(crate) fn initialize(&self, session: String) {
        *self.session.lock().unwrap() = Some(session);
    }
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
    pub(crate) fn permissions(
        &self,
        worker: String,
        permissions: protocol::integration::Permissions,
    ) -> io::Result<protocol::integration::Applied> {
        if permissions.revision == 0 {
            return Err(io::Error::other("Invalid permission revision"));
        }
        let (reply, result) = mpsc::sync_channel(1);
        self.updates
            .try_send(Update {
                worker,
                permissions,
                reply,
            })
            .map_err(|_| io::Error::other("Worker control is busy or closed"))?;
        result
            .recv_timeout(Duration::from_secs(3))
            .map_err(|_| io::Error::other("Worker control timed out"))?
    }
    pub(crate) fn register(&self, worker: &str, service: &str) -> io::Result<()> {
        self.registrations.lock().unwrap().register(worker, service)
    }
}
impl Drop for Broker {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(task) = self.task.take() {
            let _ = task.join();
        }
    }
}
pub(crate) struct Lease {
    scope: crate::routing::Scope,
    permissions: Option<mpsc::Receiver<PermissionCall>>,
    stream: UnixStream,
    stop: Cancellation,
    task: Option<JoinHandle<()>>,
    _directory: crate::owned_directory::Directory,
}
impl Lease {
    pub(crate) fn attach(path: &Path, request: protocol::Attach) -> io::Result<Self> {
        let parent = fs::symlink_metadata(
            path.parent()
                .ok_or_else(|| io::Error::other("Invalid worker socket"))?,
        )?;
        let socket = fs::symlink_metadata(path)?;
        if !path.is_absolute()
            || !parent.is_dir()
            || parent.uid() != unsafe { libc::geteuid() }
            || parent.mode() & 0o077 != 0
            || !socket.file_type().is_socket()
            || socket.uid() != parent.uid()
        {
            return Err(io::Error::other(
                "Needle worker socket is not private to this account",
            ));
        }
        // Keep transient files alive for admitted work even if control exits.
        let directory = crate::owned_directory::Directory::join(path.parent().unwrap())?;
        let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
        socket.connect_timeout(&socket2::SockAddr::unix(path)?, Duration::from_secs(1))?;
        let descriptor: std::os::fd::OwnedFd = socket.into();
        let mut stream = UnixStream::from(descriptor);
        stream.set_write_timeout(Some(Duration::from_secs(1)))?;
        protocol::write(&mut stream, &request)?;
        let reply: protocol::Attached = protocol::read(&mut Handshake {
            stream: &mut stream,
            deadline: Instant::now() + Duration::from_secs(2),
        })?;
        if reply.runtime_id != request.runtime_id || reply.worker_id != request.worker_id {
            return Err(io::Error::other(
                "Worker registration response does not match this session",
            ));
        }
        if let Some(error) = reply.error {
            return Err(io::Error::other(error));
        }
        let session = reply
            .session_id
            .filter(|id| uuid::Uuid::parse_str(id).is_ok())
            .ok_or_else(|| io::Error::other("Runtime session identity is missing"))?;
        let scope = crate::routing::Scope {
            session,
            runtime: request.runtime_id.clone(),
            directory: path.parent().unwrap().into(),
        };
        stream.set_read_timeout(Some(Duration::from_millis(100)))?;
        let mut watch = stream.try_clone()?;
        let stop = Cancellation::default();
        let stopping = stop.clone();
        let (requests, permissions) = mpsc::sync_channel::<PermissionCall>(1);
        let task = std::thread::spawn(move || {
            let result = (|| -> io::Result<()> {
                while stopping.check().is_ok() {
                    let mut fd = libc::pollfd {
                        fd: watch.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    };
                    let ready = unsafe { libc::poll(&mut fd, 1, 50) };
                    if ready == 0 {
                        continue;
                    }
                    if ready < 0 {
                        let error = io::Error::last_os_error();
                        if error.kind() == io::ErrorKind::Interrupted {
                            continue;
                        }
                        return Err(error);
                    }
                    let update: protocol::WorkerPermissions = protocol::read(&mut Handshake {
                        stream: &mut watch,
                        deadline: Instant::now() + Duration::from_secs(1),
                    })?;
                    if request.service != "integration"
                        || update.runtime_id != request.runtime_id
                        || update.worker_id != request.worker_id
                        || update.permissions.revision == 0
                    {
                        return Err(io::Error::other("Invalid worker permission request"));
                    }
                    let (reply, applied) = mpsc::sync_channel(1);
                    requests
                        .try_send(PermissionCall {
                            permissions: update.permissions,
                            reply,
                        })
                        .map_err(|_| io::Error::other("Worker cannot apply permissions"))?;
                    let applied = applied
                        .recv_timeout(Duration::from_secs(1))
                        .map_err(|_| io::Error::other("Worker permission application timed out"))?;
                    watch.set_write_timeout(Some(Duration::from_millis(250)))?;
                    protocol::write(&mut watch, &applied)?;
                }
                Ok(())
            })();
            let _ = result;
            stopping.cancel();
            let _ = watch.shutdown(Shutdown::Both);
        });
        Ok(Self {
            scope,
            permissions: Some(permissions),
            stream,
            stop,
            task: Some(task),
            _directory: directory,
        })
    }
    pub(crate) fn scope(&self) -> crate::routing::Scope {
        self.scope.clone()
    }
    pub(crate) fn take_permissions(&mut self) -> mpsc::Receiver<PermissionCall> {
        self.permissions.take().expect("worker permission receiver")
    }
    pub(crate) fn cancellation(&self) -> Cancellation {
        self.stop.clone()
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.stop.cancel();
        let _ = self.stream.shutdown(Shutdown::Both);
        if let Some(task) = self.task.take() {
            let _ = task.join();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn admission_is_bounded_one_time_and_checks_runtime_build_and_service() {
        let mut registry = Registrations::default();
        let worker = uuid::Uuid::new_v4().to_string();
        registry.register(&worker, "git").unwrap();
        let mut request = protocol::Attach {
            runtime_id: "runtime".into(),
            worker_id: worker.clone(),
            build_sha256: "build".into(),
            service: "monitor".into(),
        };
        assert!(registry.attach(&request, "runtime", "build").is_err());
        request.service = "git".into();
        assert!(registry.attach(&request, "other-runtime", "build").is_err());
        assert!(registry.attach(&request, "runtime", "other-build").is_err());
        registry.attach(&request, "runtime", "build").unwrap();
        assert!(registry.attach(&request, "runtime", "build").is_err());
        registry.active.remove(&worker);
        assert!(registry.register(&worker, "git").is_err());
        for _ in 0..protocol::MAX_WORKERS {
            registry
                .register(&uuid::Uuid::new_v4().to_string(), "git")
                .unwrap();
        }
        assert!(registry
            .register(&uuid::Uuid::new_v4().to_string(), "git")
            .is_err());
    }

    #[test]
    fn expired_admission_frees_capacity_without_making_old_tickets_reusable() {
        let mut registry = Registrations::default();
        let expired = uuid::Uuid::new_v4().to_string();
        registry.register(&expired, "git").unwrap();
        registry.pending.get_mut(&expired).unwrap().expires = Instant::now();
        for _ in 0..protocol::MAX_WORKERS {
            registry
                .register(&uuid::Uuid::new_v4().to_string(), "git")
                .unwrap();
        }
        assert_eq!(registry.pending.len(), protocol::MAX_WORKERS);
        assert!(registry
            .attach(
                &protocol::Attach {
                    runtime_id: "runtime".into(),
                    worker_id: expired.clone(),
                    build_sha256: "build".into(),
                    service: "git".into(),
                },
                "runtime",
                "build"
            )
            .is_err());
        registry.pending.clear();
        assert!(registry.register(&expired, "git").is_err());
        registry
            .register(&uuid::Uuid::new_v4().to_string(), "git")
            .unwrap();
    }
}
