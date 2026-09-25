//! X11 listener, authentication, and connection lifecycle.
use crate::x_protocol;
use std::{
    collections::HashMap,
    fs,
    io::{self, Write},
    net::Shutdown,
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

/// Private, local-only X transport. Existing sockets are never unlinked.
pub struct Server {
    pub display: String,
    authority: tempfile::NamedTempFile,
    socket: PathBuf,
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
        let dir = Path::new("/tmp/.X11-unix");
        match fs::DirBuilder::new().mode(0o1777).create(dir) {
            Ok(()) => fs::set_permissions(dir, fs::Permissions::from_mode(0o1777))?,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let metadata = fs::symlink_metadata(dir)?;
        // An untrusted account must not be able to replace our socket directory.
        let uid = unsafe { libc::geteuid() };
        if !metadata.is_dir()
            || (metadata.uid() != 0 && metadata.uid() != uid)
            || (metadata.mode() & 0o022 != 0 && metadata.mode() & 0o1000 == 0)
        {
            return Err(io::Error::other("invalid X socket directory"));
        }
        let numbers: Box<dyn Iterator<Item = u16>> = match requested {
            Some(n) => Box::new(std::iter::once(n)),
            None => Box::new(100..1000),
        };
        let authority = tempfile::NamedTempFile::new()?;
        let mut bound = None;
        for n in numbers {
            let path = dir.join(format!("X{n}"));
            // Avoid taking an address occupied by a real server's lock or socket.
            if Path::new(&format!("/tmp/.X{n}-lock")).exists() {
                continue;
            }
            match UnixListener::bind(&path) {
                Ok(l) => {
                    bound = Some((n, path, l));
                    break;
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::AddrInUse | io::ErrorKind::AlreadyExists
                    ) => {}
                Err(e) => return Err(e),
            }
        }
        let (number, socket, listener) =
            bound.ok_or_else(|| io::Error::other("no unused X display available"))?;
        // Construct the guard before fallible initialization so failed startup cleans up.
        let mut server = Self {
            display: format!(":{number}"),
            authority,
            socket,
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
            clients: Arc::new(Mutex::new(HashMap::new())),
        };
        fs::set_permissions(&server.socket, fs::Permissions::from_mode(0o600))?;
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
        server.authority.write_all(&auth)?;
        server.authority.flush()?;
        listener.set_nonblocking(true)?;
        let shared = Arc::new(x_protocol::Shared {
            atoms: Mutex::new(Default::default()),
            snapshot: source.path.clone(),
            source,
        });
        let stop = server.stop.clone();
        let clients = server.clients.clone();
        server.thread = Some(thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let mut guard = clients.lock().unwrap();
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
                        thread::spawn(move || {
                            let _ = x_protocol::serve(stream, shared, cookie, id << 20);
                            clients.lock().unwrap().remove(&id);
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
        self.authority.path()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        for stream in self.clients.lock().unwrap().values() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        let _ = fs::remove_file(&self.socket);
    }
}
