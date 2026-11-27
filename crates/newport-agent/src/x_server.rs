//! X11 listener, authentication, and connection lifecycle.
use crate::x_protocol;
use std::{
    collections::HashMap,
    io::{self, Write},
    net::Shutdown,
    os::{
        fd::{OwnedFd, RawFd},
        unix::{
            io::AsRawFd,
            net::{UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

/// Private, local-only X transport with recorded ownership and crash recovery.
pub struct Server {
    pub display: String,
    authority: tempfile::TempPath,
    /// The listening socket, kept for a handoff.
    listener: OwnedFd,
    cookie: [u8; 16],
    _endpoint: crate::x_endpoint::Endpoint,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    clients: Arc<Mutex<HashMap<u32, UnixStream>>>,
}
impl Server {
    pub fn start(snapshot: &Path, requested: Option<u16>) -> io::Result<Self> {
        Self::start_source(crate::clipboard_source::Source::new(snapshot), requested)
    }
    pub(crate) fn start_source(
        source: crate::clipboard_source::Source,
        requested: Option<u16>,
    ) -> io::Result<Self> {
        let (listener, endpoint) = crate::x_endpoint::Endpoint::bind(requested)?;
        let number = endpoint.display;
        // Removed again if startup fails.
        let mut authority = tempfile::NamedTempFile::new_in(endpoint.directory())?;
        let mut cookie = [0; 16];
        getrandom::fill(&mut cookie).map_err(|e| io::Error::other(e.to_string()))?;
        let mut auth = vec![0xff, 0xff]; // FamilyWild; cookie is unique to this listener.
        for field in [
            b"".as_slice(),
            number.to_string().as_bytes(),
            b"MIT-MAGIC-COOKIE-1",
            &cookie,
        ] {
            auth.extend_from_slice(&(field.len() as u16).to_be_bytes());
            auth.extend_from_slice(field);
        }
        authority.write_all(&auth)?;
        authority.flush()?;
        Self::serve(
            source,
            listener,
            endpoint,
            authority.into_temp_path(),
            cookie,
        )
    }
    /// Serves on what another program of this process served, as across
    /// `exec`: the same socket, display number, authority file and cookie,
    /// which the shell's `DISPLAY` and `XAUTHORITY` name.
    pub(crate) fn adopt(
        source: crate::clipboard_source::Source,
        handoff: Handoff,
        listener: UnixListener,
        directory: crate::owned_directory::Directory,
    ) -> io::Result<Self> {
        let endpoint = crate::x_endpoint::Endpoint::adopt(handoff.display, directory);
        Self::serve(
            source,
            listener,
            endpoint,
            tempfile::TempPath::try_from_path(handoff.authority)?,
            handoff.cookie,
        )
    }
    fn serve(
        source: crate::clipboard_source::Source,
        listener: UnixListener,
        endpoint: crate::x_endpoint::Endpoint,
        authority: tempfile::TempPath,
        cookie: [u8; 16],
    ) -> io::Result<Self> {
        let mut server = Self {
            display: format!(":{}", endpoint.display),
            authority,
            listener: listener.try_clone()?.into(),
            cookie,
            _endpoint: endpoint,
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
            clients: Arc::new(Mutex::new(HashMap::new())),
        };
        listener.set_nonblocking(true)?;
        let shared = Arc::new(x_protocol::Shared {
            atoms: Mutex::new(Default::default()),
            source,
        });
        let stop = server.stop.clone();
        let clients = server.clients.clone();
        server.thread = Some(thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let mut guard = clients
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        let Some(id) = (1..=16).find(|id| !guard.contains_key(id)) else {
                            continue;
                        };
                        let Ok(copy) = stream.try_clone() else {
                            continue;
                        };
                        guard.insert(id, copy);
                        drop(guard);
                        let clients = clients.clone();
                        let shared = shared.clone();
                        let cookie = (!same_account(&stream)).then_some(cookie);
                        thread::spawn(move || {
                            let _ = x_protocol::serve(stream, shared, cookie, id << 20);
                            clients
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .remove(&id);
                        });
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Err(_) => break,
                }
            }
        }));
        Ok(server)
    }
    pub fn is_running(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
    }
    pub fn authority(&self) -> &Path {
        &self.authority
    }
    /// What another program of this process needs to serve on as this one
    /// does, with the descriptors it must keep open: the listening socket and
    /// the endpoint directory's lease.
    pub(crate) fn handoff(&self) -> (Handoff, [RawFd; 2]) {
        (
            Handoff {
                display: self._endpoint.display,
                directory: self._endpoint.directory().to_owned(),
                directory_lease: self._endpoint.owned().lease().as_raw_fd(),
                listener: self.listener.as_raw_fd(),
                authority: self.authority.to_path_buf(),
                cookie: self.cookie,
            },
            [
                self.listener.as_raw_fd(),
                self._endpoint.owned().lease().as_raw_fd(),
            ],
        )
    }
}

/// An X11 clipboard server handed to the next program of its process.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Handoff {
    pub display: u16,
    pub directory: PathBuf,
    pub directory_lease: RawFd,
    pub listener: RawFd,
    pub authority: PathBuf,
    pub cookie: [u8; 16],
}
/// Whether the client runs as this account. Only it can reach the socket
/// (mode 600), and it can read the cookie anyway, so the cookie proves nothing
/// more for it. Admitting it without one keeps the clipboard working for
/// programs started under an earlier session, whose XAUTHORITY names a file
/// that went with it; they keep their environment for as long as they run.
fn same_account(stream: &UnixStream) -> bool {
    peer_uid(stream).is_some_and(|uid| uid == crate::paths::effective_uid())
}

#[cfg(target_os = "linux")]
fn peer_uid(stream: &UnixStream) -> Option<u32> {
    // SAFETY: ucred holds plain integers, for which all zeroes is valid.
    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: the buffer and its length describe a valid ucred for this call.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    (result == 0).then_some(credentials.uid)
}

#[cfg(not(target_os = "linux"))]
fn peer_uid(stream: &UnixStream) -> Option<u32> {
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: both out-pointers are valid for the call.
    let result = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    (result == 0).then_some(uid)
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        for stream in self
            .clients
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
        {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}
