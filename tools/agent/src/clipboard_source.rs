//! Clipboard readers use the live agent socket, or a standalone snapshot file.
use crate::{
    clipboard_wire::{Offer, Request},
    local_socket::Endpoint,
    snapshot, wire,
};
use std::{
    collections::BTreeMap,
    io,
    os::unix::{fs::MetadataExt, net::UnixStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};
/// Backends share this handle; enabling demand mode never requires reconnecting clients.
#[derive(Clone)]
pub(crate) struct Source {
    pub path: PathBuf,
    store: Arc<std::sync::OnceLock<Arc<crate::demand::Store>>>,
}
impl Source {
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.into(),
            store: Arc::new(std::sync::OnceLock::new()),
        }
    }
    pub fn enable(&self, store: Arc<crate::demand::Store>) {
        let _ = self.store.set(store);
    }
    pub fn offer(&self) -> io::Result<(Revision, BTreeMap<String, Vec<u8>>)> {
        if let Some(store) = self.store.get() {
            let offer = store.offer();
            Ok((
                (0, offer.revision as u64, 0),
                offer.formats.into_iter().map(|s| (s, vec![])).collect(),
            ))
        } else {
            offer(&self.path)
        }
    }
    pub fn get(&self, revision: Revision, format: &str) -> io::Result<Vec<u8>> {
        if let Some(store) = self.store.get() {
            store.get(revision.1 as i64, format)
        } else {
            get(&self.path, revision, format)
        }
    }
}

pub(crate) type Revision = (u64, u64, u64);
fn socket(path: &Path) -> PathBuf {
    path.with_file_name("clipboard.sock")
}
fn connect(path: &Path) -> io::Result<UnixStream> {
    let stream = UnixStream::connect(socket(path))?;
    stream.set_read_timeout(Some(Duration::from_secs(16)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    Ok(stream)
}
pub(crate) fn offer(path: &Path) -> io::Result<(Revision, BTreeMap<String, Vec<u8>>)> {
    if socket(path).exists() {
        let mut stream = connect(path)?;
        wire::write(&mut stream, b'M', &[])?;
        let (kind, bytes) = wire::read(&mut stream)?;
        if kind != b'M' {
            return Err(io::Error::other("clipboard metadata unavailable"));
        }
        let offer = Offer::decode(&bytes)?;
        Ok((
            (0, offer.revision as u64, 0),
            offer.formats.into_iter().map(|s| (s, vec![])).collect(),
        ))
    } else {
        // Confirm the file didn't change during the read.
        let before = revision(path)?;
        let values = snapshot::read(path)?;
        if revision(path)? != before {
            return Err(io::Error::other("clipboard changed"));
        }
        Ok((before, values))
    }
}
fn revision(path: &Path) -> io::Result<Revision> {
    let m = std::fs::symlink_metadata(path)?;
    Ok((m.dev(), m.ino(), m.len()))
}
pub(crate) fn get(path: &Path, revision: Revision, format: &str) -> io::Result<Vec<u8>> {
    if revision.0 == 0 {
        let mut stream = connect(path)?;
        wire::write(
            &mut stream,
            b'C',
            &Request {
                id: 0,
                revision: revision.1 as i64,
                format: format.into(),
            }
            .encode(),
        )?;
        let (kind, bytes) = wire::read(&mut stream)?;
        if kind != b'D' {
            return Err(io::Error::other("clipboard request failed"));
        }
        Ok(bytes)
    } else {
        let (current, mut values) = offer(path)?;
        if current != revision {
            return Err(io::Error::other("clipboard changed"));
        }
        values
            .remove(format)
            .ok_or_else(|| io::Error::other("clipboard format unavailable"))
    }
}
pub(crate) struct Service {
    stop: Arc<AtomicBool>,
    store: Arc<crate::demand::Store>,
    thread: Option<thread::JoinHandle<()>>,
    _endpoint: Endpoint,
}
impl Service {
    pub fn start(path: &Path, store: Arc<crate::demand::Store>) -> io::Result<Self> {
        let (listener, endpoint) = Endpoint::fixed(&socket(path))?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let data = store.clone();
        let active = Arc::new(AtomicUsize::new(0));
        let thread = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                if let Ok((mut stream, _)) = listener.accept() {
                    if active.load(Ordering::Relaxed) >= 24 {
                        continue;
                    }
                    active.fetch_add(1, Ordering::Relaxed);
                    let active = active.clone();
                    let data = data.clone();
                    thread::spawn(move || {
                        // Accepted sockets may inherit nonblocking mode on macOS.
                        // Worker transfers use bounded blocking I/O so large replies are complete.
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
                        // Local requests are tiny: never allocate a full wire frame from a client.
                        let result = (|| -> io::Result<(u8, Vec<u8>)> {
                            use std::io::Read;
                            let mut header = [0; 5];
                            stream.read_exact(&mut header)?;
                            let len = u32::from_be_bytes(header[1..].try_into().unwrap()) as usize;
                            if len > 512 {
                                return Err(io::Error::other("invalid clipboard request"));
                            }
                            let mut bytes = vec![0; len];
                            stream.read_exact(&mut bytes)?;
                            match header[0] {
                                b'M' if bytes.is_empty() => Ok((b'M', data.offer().encode())),
                                b'C' => {
                                    let request = Request::decode(&bytes)?;
                                    let bytes = data.get(request.revision, &request.format)?;
                                    Ok((b'D', bytes))
                                }
                                _ => Err(io::Error::other("invalid clipboard request")),
                            }
                        })();
                        let (kind, bytes) = result
                            .unwrap_or_else(|_| (b'E', b"clipboard unavailable; retry".to_vec()));
                        // Never append an error frame after a partially written data frame.
                        let _ = wire::write(&mut stream, kind, &bytes);
                        active.fetch_sub(1, Ordering::Relaxed);
                    });
                } else {
                    thread::sleep(Duration::from_millis(5));
                }
            }
        });
        Ok(Self {
            stop,
            store,
            thread: Some(thread),
            _endpoint: endpoint,
        })
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.store.close();
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
