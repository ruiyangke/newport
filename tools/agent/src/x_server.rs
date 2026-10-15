//! X11 listener, authentication, and connection lifecycle.
use crate::x_protocol;
use std::{
    collections::HashMap,
    io::{self, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

/// Private, local-only X transport with recorded ownership and crash recovery.
pub struct Server {
    pub display: String,
    authority: tempfile::NamedTempFile,
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
        let authority = tempfile::NamedTempFile::new_in(endpoint.directory())?;
        // Construct the guard before fallible initialization so failed startup cleans up.
        let mut server = Self {
            display: format!(":{number}"),
            authority,
            _endpoint: endpoint,
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
            clients: Arc::new(Mutex::new(HashMap::new())),
        };
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
    }
}
