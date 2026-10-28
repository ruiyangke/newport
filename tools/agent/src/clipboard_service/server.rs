use super::{
    protocol::{self, Access, Chunk, Deadline, Request, Response, Route},
    state::{Grant, Shared, State},
};
use crate::local_socket::Endpoint;
use std::{
    io,
    net::Shutdown,
    os::fd::AsRawFd,
    os::unix::net::UnixStream,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

/// How often an idle accept loop wakes to notice its service stopping.
const IDLE_WAKE_MS: libc::c_int = 100;

#[derive(Clone)]
pub(crate) struct Authority(Shared);
impl Authority {
    pub(crate) fn grant(&self, worker: &str, service: &str) -> io::Result<Access> {
        self.0.lock().unwrap().grant(worker, service)
    }
    pub(crate) fn revoke(&self, worker: &str) {
        self.0.lock().unwrap().revoke(worker);
    }
    pub(crate) fn configure_route(&self, route: Route) -> io::Result<()> {
        self.0.lock().unwrap().configure_route(route)
    }
    pub(crate) fn configure_route_v2(
        &self,
        route: Route,
    ) -> io::Result<crate::runtime_protocol::clipboard::RouteReply> {
        self.0.lock().unwrap().configure_route_v2(route)
    }
}
pub(crate) struct Service {
    authority: Authority,
    stop: Arc<AtomicBool>,
    task: Option<JoinHandle<()>>,
    _endpoint: Endpoint,
}
impl Service {
    pub(crate) fn start(path: &Path, runtime: &str) -> io::Result<Self> {
        let (listener, endpoint) = Endpoint::fixed(path)?;
        let state = Arc::new(Mutex::new(State::new(runtime, path.into())));
        let authority = Authority(state.clone());
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let task = std::thread::spawn(move || {
            let mut connections: Vec<(UnixStream, JoinHandle<()>)> = vec![];
            while !stopping.load(Ordering::Acquire) {
                let mut index = 0;
                while index < connections.len() {
                    if connections[index].1.is_finished() {
                        let (_, task) = connections.swap_remove(index);
                        let _ = task.join();
                    } else {
                        index += 1;
                    }
                }
                let stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    // Idle: wait for a connection, waking to notice a stop.
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        let mut ready = libc::pollfd {
                            fd: listener.as_raw_fd(),
                            events: libc::POLLIN,
                            revents: 0,
                        };
                        unsafe { libc::poll(&mut ready, 1, IDLE_WAKE_MS) };
                        continue;
                    }
                    Err(_) => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                };
                if connections.len() >= protocol::MAX_CONNECTIONS {
                    continue;
                }
                if stream.set_nonblocking(false).is_err() {
                    continue;
                }
                let Ok(control) = stream.try_clone() else {
                    continue;
                };
                let state = state.clone();
                let task = std::thread::spawn(move || {
                    let _ = serve(stream, state);
                });
                connections.push((control, task));
            }
            state.lock().unwrap().close();
            for (stream, _) in &connections {
                let _ = stream.shutdown(Shutdown::Both);
            }
            for (_, task) in connections {
                let _ = task.join();
            }
        });
        Ok(Self {
            authority,
            stop,
            task: Some(task),
            _endpoint: endpoint,
        })
    }
    pub(crate) fn authority(&self) -> Authority {
        self.authority.clone()
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.authority.0.lock().unwrap().close();
        self.stop.store(true, Ordering::Release);
        if let Some(task) = self.task.take() {
            let _ = task.join();
        }
    }
}
fn serve(mut stream: UnixStream, state: Shared) -> io::Result<()> {
    let request: Request = protocol::read(&mut Deadline::new(&mut stream, Duration::from_secs(2)))?;
    let grant = match request {
        Request::Authenticate(access) => state.lock().unwrap().authenticate(&access),
        _ => Err(io::Error::other("Clipboard worker authentication required")),
    }
    .and_then(|grant| grant.track(&stream).map(|connection| (grant, connection)));
    let response = match &grant {
        Ok(_) => Response::Ready,
        Err(error) => Response::Error(error.to_string()),
    };
    protocol::write(
        &mut Deadline::new(&mut stream, Duration::from_secs(2)),
        &response,
    )?;
    let (grant, _connection) = grant?;
    let copy_connection = CopyConnection {
        state: state.clone(),
        id: uuid::Uuid::new_v4().to_string(),
    };
    loop {
        let request: Request =
            protocol::read(&mut Deadline::new(&mut stream, Duration::from_secs(45)))?;
        let result = handle(&state, &grant, &stream, &copy_connection.id, request);
        let response = result.unwrap_or_else(|error| Response::Error(error.to_string()));
        protocol::write(
            &mut Deadline::new(&mut stream, Duration::from_secs(2)),
            &response,
        )?;
        if grant.cancelled.load(Ordering::Acquire) {
            return Ok(());
        }
    }
}
struct CopyConnection {
    state: Shared,
    id: String,
}
impl Drop for CopyConnection {
    fn drop(&mut self) {
        self.state.lock().unwrap().release_copy_connection(&self.id);
    }
}
fn handle(
    state: &Shared,
    grant: &Grant,
    stream: &UnixStream,
    connection: &str,
    request: Request,
) -> io::Result<Response> {
    if let Request::Read {
        runtime,
        revision,
        format,
        offset,
        limit,
        export,
    } = request
    {
        if !crate::clipboard_wire::valid_format(&format)
            || offset > crate::clipboard_wire::LIMIT
            || limit > crate::clipboard_wire::CHUNK
            || limit == 0
        {
            return Err(io::Error::other("Invalid clipboard range"));
        }
        let (store, export_cancelled) = {
            let state = state.lock().unwrap();
            let cancellation = export
                .as_ref()
                .map(|authority| {
                    let lease = authority.offer();
                    if lease.offer.runtime != runtime || lease.offer.revision != revision {
                        return Err(io::Error::other(
                            "Clipboard export lease does not match range",
                        ));
                    }
                    match authority {
                        protocol::ExportRead::Focused(lease) => state.export_access(grant, lease),
                        protocol::ExportRead::Committed(copy) => state.copy_access(grant, copy),
                    }
                })
                .transpose()?;
            (
                state.readable(grant, &runtime, revision, &format)?,
                cancellation,
            )
        };
        // No runtime-state lock is held while waiting for the host producer.
        let cancelled = || {
            grant.cancelled.load(Ordering::Acquire)
                || export_cancelled
                    .as_ref()
                    .is_some_and(|stop| stop.load(Ordering::Acquire))
                || disconnected(stream)
        };
        let bytes = store.get_shared(revision, &format, Some(&cancelled))?;
        {
            let state = state.lock().unwrap();
            state.readable(grant, &runtime, revision, &format)?;
            if let Some(authority) = &export {
                match authority {
                    protocol::ExportRead::Focused(lease) => {
                        state.export_access(grant, lease)?;
                    }
                    protocol::ExportRead::Committed(copy) => {
                        state.copy_access(grant, copy)?;
                    }
                }
            }
        }
        if offset > bytes.len() {
            return Err(io::Error::other("Clipboard offset exceeds payload"));
        }
        let end = bytes.len().min(offset + limit);
        return Ok(Response::Chunk(Chunk {
            bytes: bytes[offset..end].to_vec(),
            eof: end == bytes.len(),
        }));
    }
    let mut state = state.lock().unwrap();
    state.current(grant)?;
    match request {
        Request::Offer => Ok(Response::Offer(state.offer(grant)?)),
        Request::Claim => {
            state.claim(grant)?;
            Ok(Response::Ready)
        }
        Request::Enabled(enabled) => {
            state.enabled(grant, enabled)?;
            Ok(Response::Ready)
        }
        Request::Publish { revision, formats } => {
            state.publish(grant, revision, formats)?;
            Ok(Response::Ready)
        }
        Request::Poll => Ok(Response::Requests(state.poll(grant)?)),
        Request::Reply(bytes) => {
            state.reply(grant, &bytes)?;
            Ok(Response::Ready)
        }
        Request::NativePoll => Ok(Response::Provider(state.native_poll(grant)?)),
        Request::NativePublish {
            lease,
            expected_runtime,
            expected_revision,
            revision,
            formats,
        } => Ok(Response::Published(state.native_publish(
            grant,
            lease,
            &expected_runtime,
            expected_revision,
            revision,
            formats,
        )?)),
        Request::NativeReply { request, chunk } => {
            state.native_reply(grant, request, chunk)?;
            Ok(Response::Ready)
        }
        Request::ExportOffer => Ok(Response::Export(state.export_offer(grant)?)),
        Request::CommitCopy(offer) => {
            Ok(Response::Copy(state.commit_copy(grant, connection, offer)?))
        }
        Request::CopyStatus => Ok(Response::Copy(state.copy_status(grant, connection)?)),
        Request::ReleaseCopy(copy) => {
            state.release_copy(grant, connection, &copy)?;
            Ok(Response::Copy(state.copy_status(grant, connection)?))
        }
        _ => Err(io::Error::other("Unexpected clipboard IPC request")),
    }
}

// While a payload is pending, EOF must release this handler without waiting for
// the shared host request. Peeking preserves queued bytes and never blocks or
// changes the descriptor mode. Other consumers can keep using the same fetch.
fn disconnected(stream: &UnixStream) -> bool {
    let mut byte = 0u8;
    // SAFETY: one live borrowed socket and one writable byte; no ownership moves.
    let count = unsafe {
        libc::recv(
            stream.as_raw_fd(),
            (&mut byte as *mut u8).cast(),
            1,
            libc::MSG_PEEK | libc::MSG_DONTWAIT,
        )
    };
    count == 0
        || (count < 0
            && !matches!(
                io::Error::last_os_error().kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ))
}
