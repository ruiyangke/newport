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
/// Silence limit: a read fails when no frame arrived for this long.
const TIMEOUT: Duration = Duration::from_secs(15);
/// Hard limit from the first request, however many keep-alives arrive.
const CEILING: Duration = Duration::from_secs(30 * 60);
struct Entry {
    request: Request,
    sent: bool,
    /// First request; bounds the whole read at `CEILING`.
    first: Instant,
    /// Last progress: the request, or the latest not-done reply frame.
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
impl Entry {
    /// The instant after which an unfinished read has failed.
    fn expires(&self) -> Instant {
        (self.started + TIMEOUT).min(self.first + CEILING)
    }
}
pub(crate) struct Store {
    inner: Mutex<Inner>,
    changed: Condvar,
    #[cfg(test)]
    skew: Mutex<Duration>,
}
impl Store {
    #[cfg(not(test))]
    fn now(&self) -> Instant {
        Instant::now()
    }
    /// Tests move this clock forward instead of sleeping.
    #[cfg(test)]
    fn now(&self) -> Instant {
        Instant::now() + *self.skew.lock().unwrap()
    }
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
            #[cfg(test)]
            skew: Mutex::new(Duration::ZERO),
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
        self.get_shared(revision, format, None)
            .map(|bytes| bytes.as_ref().clone())
    }
    pub(crate) fn get_shared(
        &self,
        revision: i64,
        format: &str,
        cancelled: Option<&dyn Fn() -> bool>,
    ) -> io::Result<Arc<Vec<u8>>> {
        let mut inner = self.inner.lock().unwrap();
        loop {
            let now = self.now();
            if cancelled.is_some_and(|stop| stop())
                || inner.closed
                || inner.offer.revision != revision
                || !inner.offer.formats.iter().any(|s| s == format)
            {
                return Err(io::Error::other("clipboard changed or unavailable"));
            }
            if let Some(entry) = inner.entries.get(format) {
                if let Some(result) = &entry.result {
                    return result
                        .as_ref()
                        .cloned()
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
                        first: now,
                        started: now,
                        bytes: vec![],
                        result: None,
                    },
                );
            }
            let remaining = inner.entries[format]
                .expires()
                .saturating_duration_since(now);
            if remaining.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "clipboard request timed out",
                ));
            }
            let remaining = if cancelled.is_some() {
                remaining.min(Duration::from_millis(50))
            } else {
                remaining
            };
            inner = self.changed.wait_timeout(inner, remaining).unwrap().0;
        }
    }
    pub fn poll(&self, output: &mut impl io::Write) -> io::Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let now = self.now();
        inner.entries.retain(|_, e| {
            !(matches!(e.result, Some(Err(())))
                && now.saturating_duration_since(e.started) > Duration::from_secs(1))
        });
        let mut requests = Vec::new();
        for entry in inner.entries.values_mut() {
            if entry.result.is_none() && now >= entry.expires() {
                entry.bytes.clear();
                entry.result = Some(Err(()));
                entry.started = now;
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
        let now = self.now();
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
        let decoded = clipboard_wire::decompress_chunk(status, &bytes);
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
                entry.started = now;
            } else if done {
                entry.bytes.extend_from_slice(&bytes);
                entry.result = Some(Ok(Arc::new(std::mem::take(&mut entry.bytes))));
            } else if now < entry.expires() {
                // A not-done frame adds data only when it carries some. For the
                // files, empty keep-alives included, it is also progress, and
                // never past the ceiling; other formats keep the fixed window.
                entry.bytes.extend_from_slice(&bytes);
                if entry.request.format == crate::runtime_protocol::clipboard::FILES_FORMAT {
                    entry.started = now;
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
    const FILES: &str = "application/x-newport-files";
    fn files_store() -> Arc<Store> {
        let store = Store::new();
        store.update(Offer {
            revision: 1,
            formats: vec![FILES.into()],
        });
        store
    }
    /// Moves the store's clock forward and wakes its waiters.
    fn advance(store: &Store, by: Duration) {
        *store.skew.lock().unwrap() += by;
        store.changed.notify_all();
    }
    fn keep_alive(store: &Store, req: &Request) {
        store
            .reply(&clipboard_wire::reply(req, 0, false, b""))
            .unwrap();
    }
    fn spawn_read(store: &Arc<Store>) -> std::thread::JoinHandle<io::Result<Arc<Vec<u8>>>> {
        let store = store.clone();
        std::thread::spawn(move || store.get_shared(1, FILES, Some(&|| false)))
    }
    fn settle() {
        std::thread::sleep(Duration::from_millis(150));
    }
    #[test]
    fn keep_alives_hold_a_long_read_open_and_are_not_data() {
        let store = files_store();
        let read = spawn_read(&store);
        let req = request(&store);
        store
            .reply(&clipboard_wire::reply(&req, 0, false, b"ab"))
            .unwrap();
        for _ in 0..12 {
            advance(&store, Duration::from_secs(5));
            keep_alive(&store, &req);
            store.poll(&mut vec![]).unwrap();
        }
        settle();
        assert!(!read.is_finished(), "60 s of keep-alives failed the read");
        store
            .reply(&clipboard_wire::reply(&req, 0, true, b"cd"))
            .unwrap();
        assert_eq!(read.join().unwrap().unwrap().as_slice(), b"abcd");
    }
    #[test]
    fn other_formats_keep_the_fixed_window_whatever_frames_arrive() {
        let store = Store::new();
        store.update(Offer {
            revision: 1,
            formats: vec!["text/plain".into()],
        });
        let reader = store.clone();
        let read = std::thread::spawn(move || reader.get_shared(1, "text/plain", Some(&|| false)));
        let req = request(&store);
        advance(&store, Duration::from_secs(10));
        keep_alive(&store, &req);
        advance(&store, Duration::from_secs(5));
        store.poll(&mut vec![]).unwrap();
        assert!(read.join().unwrap().is_err(), "a keep-alive renewed text");
    }
    #[test]
    fn a_read_without_keep_alives_fails_at_fifteen_seconds() {
        let store = files_store();
        let read = spawn_read(&store);
        let req = request(&store);
        advance(&store, Duration::from_secs(14));
        store.poll(&mut vec![]).unwrap();
        settle();
        assert!(!read.is_finished());
        keep_alive(&store, &req);
        advance(&store, Duration::from_secs(14));
        store.poll(&mut vec![]).unwrap();
        settle();
        assert!(
            !read.is_finished(),
            "a keep-alive did not restart the window"
        );
        advance(&store, Duration::from_secs(1));
        store.poll(&mut vec![]).unwrap();
        assert!(read.join().unwrap().is_err());
    }
    #[test]
    fn keep_alives_cannot_extend_a_read_past_thirty_minutes() {
        let store = files_store();
        let read = spawn_read(&store);
        let req = request(&store);
        for _ in 0..359 {
            advance(&store, Duration::from_secs(5));
            keep_alive(&store, &req);
            store.poll(&mut vec![]).unwrap();
        }
        settle();
        assert!(!read.is_finished(), "failed before the ceiling");
        advance(&store, Duration::from_secs(5));
        keep_alive(&store, &req);
        store.poll(&mut vec![]).unwrap();
        let error = read.join().unwrap().unwrap_err();
        assert_ne!(error.kind(), io::ErrorKind::Interrupted);
        // A late completion cannot resurrect the failed read.
        store
            .reply(&clipboard_wire::reply(&req, 0, true, b"late"))
            .unwrap();
        assert!(store.get(1, FILES).is_err());
    }
    #[test]
    fn dropping_the_reader_cancels_a_long_read() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let store = files_store();
        let gone = Arc::new(AtomicBool::new(false));
        let flag = gone.clone();
        let reader = store.clone();
        let read = std::thread::spawn(move || {
            reader.get_shared(1, FILES, Some(&|| flag.load(Ordering::Acquire)))
        });
        let req = request(&store);
        keep_alive(&store, &req);
        settle();
        assert!(!read.is_finished());
        gone.store(true, Ordering::Release);
        assert!(read.join().unwrap().is_err());
    }
    #[test]
    fn ordinary_replies_keep_the_fifteen_second_window() {
        let store = Store::new();
        store.update(Offer {
            revision: 1,
            formats: vec!["text/plain".into()],
        });
        let reader = store.clone();
        let read = std::thread::spawn(move || reader.get_shared(1, "text/plain", Some(&|| false)));
        request(&store);
        advance(&store, TIMEOUT);
        assert!(read.join().unwrap().is_err());
    }
}
