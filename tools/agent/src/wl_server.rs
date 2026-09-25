//! Wayland socket and connection lifecycle.
use crate::{local_socket::Endpoint, wl_protocol::State};
use std::{
    io,
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use wayland_server::{
    backend::{ClientData, ClientId, DisconnectReason},
    Display,
};

pub struct Server {
    endpoint: Endpoint,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Server {
    pub fn start(snapshot: &Path) -> io::Result<Self> {
        let (listener, endpoint) = Endpoint::temporary()?;
        Self::listen(
            crate::clipboard_source::Source::new(snapshot),
            listener,
            endpoint,
        )
    }
    pub fn start_fixed(snapshot: &Path, socket: &Path) -> io::Result<Self> {
        let (listener, endpoint) = Endpoint::fixed(socket)?;
        Self::listen(
            crate::clipboard_source::Source::new(snapshot),
            listener,
            endpoint,
        )
    }
    pub(crate) fn start_source(
        source: crate::clipboard_source::Source,
        socket: &Path,
    ) -> io::Result<Self> {
        let (listener, endpoint) = Endpoint::fixed(socket)?;
        Self::listen(source, listener, endpoint)
    }
    fn listen(
        source: crate::clipboard_source::Source,
        listener: UnixListener,
        endpoint: Endpoint,
    ) -> io::Result<Self> {
        let mut display = Display::<State>::new().map_err(io::Error::other)?;
        let mut handle = display.handle();
        State::register(&handle);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let mut state = State::new(source, stop.clone());
        let clients = Arc::new(AtomicUsize::new(0));
        let thread = thread::spawn(move || {
            let mut checked = Instant::now() - Duration::from_secs(1);
            while !stopped.load(Ordering::Relaxed) {
                // Bound admission work so a connection flood cannot starve dispatch.
                for _ in 0..16 {
                    match listener.accept() {
                        Ok((socket, _)) => {
                            if clients.load(Ordering::Relaxed) >= 16 {
                                continue;
                            }
                            clients.fetch_add(1, Ordering::Relaxed);
                            if handle
                                .insert_client(
                                    socket,
                                    Arc::new(Connection {
                                        count: clients.clone(),
                                    }),
                                )
                                .is_err()
                            {
                                clients.fetch_sub(1, Ordering::Relaxed);
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                        Err(_) => {
                            stopped.store(true, Ordering::Relaxed);
                            break;
                        }
                    }
                }
                if checked.elapsed() >= Duration::from_millis(100) {
                    state.refresh(&handle);
                    checked = Instant::now();
                }
                if display.dispatch_clients(&mut state).is_err() {
                    break;
                }
                if display.flush_clients().is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
            // Dropping Display closes clients. Pipe workers notice this flag too.
            stopped.store(true, Ordering::Relaxed);
        });
        Ok(Self {
            endpoint,
            stop,
            thread: Some(thread),
        })
    }
    pub fn is_running(&self) -> bool {
        !self.stop.load(Ordering::Relaxed)
    }
    pub fn socket(&self) -> PathBuf {
        self.endpoint.path().to_owned()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Debug)]
struct Connection {
    count: Arc<AtomicUsize>,
}
impl ClientData for Connection {
    fn initialized(&self, _: ClientId) {}
    fn disconnected(&self, _: ClientId, _: DisconnectReason) {
        self.count.fetch_sub(1, Ordering::Relaxed);
    }
}
