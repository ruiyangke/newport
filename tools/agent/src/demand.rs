//! Shared per-revision memory cache. Readers wait here, never in the SSH event loop.
use crate::{
    clipboard_wire::{self, Offer, Request},
    wire,
};
use std::{
    collections::BTreeMap,
    io,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};
const TIMEOUT: Duration = Duration::from_secs(15);
struct Entry {
    request: Request,
    sent: bool,
    started: Instant,
    bytes: Vec<u8>,
    result: Option<Result<Arc<Vec<u8>>, ()>>,
}
struct Inner {
    offer: Offer,
    entries: BTreeMap<String, Entry>,
    next: u64,
    closed: bool,
}
pub(crate) struct Store {
    inner: Mutex<Inner>,
    changed: Condvar,
}
impl Store {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                offer: Offer {
                    revision: 0,
                    formats: vec![],
                },
                entries: BTreeMap::new(),
                next: 0,
                closed: false,
            }),
            changed: Condvar::new(),
        })
    }
    pub fn offer(&self) -> Offer {
        let inner = self.inner.lock().unwrap();
        if inner.closed {
            Offer {
                revision: inner.offer.revision,
                formats: vec![],
            }
        } else {
            inner.offer.clone()
        }
    }
    pub fn update(&self, offer: Offer) {
        let mut inner = self.inner.lock().unwrap();
        if inner.offer.revision != offer.revision || inner.offer.formats != offer.formats {
            inner.entries.clear();
        }
        inner.offer = offer;
        self.changed.notify_all();
    }
    pub fn close(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.closed = true;
        inner.entries.clear();
        self.changed.notify_all();
    }
    pub fn get(&self, revision: i64, format: &str) -> io::Result<Vec<u8>> {
        let deadline = Instant::now() + TIMEOUT;
        let mut inner = self.inner.lock().unwrap();
        loop {
            if inner.closed
                || inner.offer.revision != revision
                || !inner.offer.formats.iter().any(|s| s == format)
            {
                return Err(io::Error::other("clipboard changed or unavailable"));
            }
            if let Some(entry) = inner.entries.get(format) {
                if let Some(result) = &entry.result {
                    return result
                        .as_ref()
                        .map(|b| b.as_ref().clone())
                        .map_err(|_| io::Error::other("clipboard request failed; retry"));
                }
            } else {
                if inner
                    .entries
                    .values()
                    .filter(|e| e.result.is_none())
                    .count()
                    >= 8
                {
                    return Err(io::Error::other("too many clipboard requests"));
                }
                inner.next += 1;
                let id = inner.next;
                inner.entries.insert(
                    format.into(),
                    Entry {
                        request: Request {
                            id,
                            revision,
                            format: format.into(),
                        },
                        sent: false,
                        started: Instant::now(),
                        bytes: vec![],
                        result: None,
                    },
                );
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "clipboard request timed out",
                ));
            }
            inner = self.changed.wait_timeout(inner, remaining).unwrap().0;
        }
    }
    pub fn poll(&self, output: &mut impl io::Write) -> io::Result<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.entries.retain(|_, e| {
            !(matches!(e.result, Some(Err(()))) && e.started.elapsed() > Duration::from_secs(1))
        });
        let mut requests = Vec::new();
        for entry in inner.entries.values_mut() {
            if entry.result.is_none() && entry.started.elapsed() >= TIMEOUT {
                entry.bytes.clear();
                entry.result = Some(Err(()));
                entry.started = Instant::now();
                self.changed.notify_all();
            }
            if !entry.sent && entry.result.is_none() {
                requests.push(entry.request.encode());
                entry.sent = true;
            }
        }
        drop(inner);
        for request in requests {
            wire::write(output, b'C', &request)?;
        }
        Ok(())
    }
    pub fn reply(&self, bytes: &[u8]) -> io::Result<()> {
        let (id, revision, status, done, bytes) = clipboard_wire::parse_reply(bytes)?;
        let mut inner = self.inner.lock().unwrap();
        if revision != inner.offer.revision {
            return Ok(());
        }
        if !inner
            .entries
            .values()
            .any(|e| e.request.id == id && e.result.is_none())
        {
            return Ok(());
        }
        // Bad compressed data fails this read, without terminating the agent.
        let decoded = clipboard_wire::decompress_chunk(status, bytes);
        let failed = decoded.is_err();
        let bytes = decoded.unwrap_or_default();
        let mut used: usize = inner
            .entries
            .values()
            .map(|e| {
                e.bytes.len()
                    + e.result
                        .as_ref()
                        .and_then(|r| r.as_ref().ok())
                        .map_or(0, |b| b.len())
            })
            .sum();
        if used + bytes.len() > clipboard_wire::LIMIT {
            inner
                .entries
                .retain(|_, e| !matches!(e.result, Some(Ok(_))));
            used = inner.entries.values().map(|e| e.bytes.len()).sum();
        }
        if let Some(entry) = inner
            .entries
            .values_mut()
            .find(|e| e.request.id == id && e.result.is_none())
        {
            if failed || used + bytes.len() > clipboard_wire::LIMIT {
                entry.bytes.clear();
                entry.result = Some(Err(()));
                entry.started = Instant::now();
            } else {
                entry.bytes.extend_from_slice(&bytes);
                if done {
                    entry.result = Some(Ok(Arc::new(std::mem::take(&mut entry.bytes))));
                }
            }
            self.changed.notify_all();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(store: &Store) -> Request {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let mut out = vec![];
            store.poll(&mut out).unwrap();
            if !out.is_empty() {
                let (kind, bytes) = wire::read(&mut out.as_slice()).unwrap();
                assert_eq!(kind, b'C');
                return Request::decode(&bytes).unwrap();
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    #[test]
    fn coalesces_readers_caches_and_rejects_stale_replies() {
        let store = Store::new();
        store.update(Offer {
            revision: 1,
            formats: vec!["image/png".into()],
        });
        let a = store.clone();
        let first = std::thread::spawn(move || a.get(1, "image/png"));
        let req = request(&store);
        let b = store.clone();
        let second = std::thread::spawn(move || b.get(1, "image/png"));
        store
            .reply(&clipboard_wire::reply(&req, 0, false, b"abc"))
            .unwrap();
        let mut out = vec![];
        store.poll(&mut out).unwrap();
        assert!(out.is_empty());
        store
            .reply(&clipboard_wire::reply(&req, 0, true, b"def"))
            .unwrap();
        assert_eq!(first.join().unwrap().unwrap(), b"abcdef");
        assert_eq!(second.join().unwrap().unwrap(), b"abcdef");
        assert_eq!(store.get(1, "image/png").unwrap(), b"abcdef");
        store.update(Offer {
            revision: 2,
            formats: vec!["image/png".into()],
        });
        let a = store.clone();
        let read = std::thread::spawn(move || a.get(2, "image/png"));
        let pending = request(&store);
        store.update(Offer {
            revision: 3,
            formats: vec![],
        });
        store
            .reply(&clipboard_wire::reply(&pending, 0, true, b"stale"))
            .unwrap();
        assert!(read.join().unwrap().is_err());
        assert!(store.inner.lock().unwrap().entries.is_empty());
    }
    #[test]
    fn timeout_failure_and_shutdown_release_waiters() {
        let store = Store::new();
        store.update(Offer {
            revision: 1,
            formats: vec!["text/plain".into()],
        });
        let a = store.clone();
        let read = std::thread::spawn(move || a.get(1, "text/plain"));
        request(&store);
        store
            .inner
            .lock()
            .unwrap()
            .entries
            .get_mut("text/plain")
            .unwrap()
            .started = Instant::now() - TIMEOUT;
        store.poll(&mut vec![]).unwrap();
        assert!(read.join().unwrap().is_err());
        store
            .inner
            .lock()
            .unwrap()
            .entries
            .get_mut("text/plain")
            .unwrap()
            .started = Instant::now() - Duration::from_secs(2);
        store.poll(&mut vec![]).unwrap();
        let a = store.clone();
        let read = std::thread::spawn(move || a.get(1, "text/plain"));
        request(&store);
        store.close();
        assert!(read.join().unwrap().is_err());
    }
}
