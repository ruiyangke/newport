//! Content snapshots are a read-through cache, never editor buffers or save
//! authority. Scope includes the server incarnation, canonical root and path.
//! Cache hits may be stale and are explicitly marked for background validation.
use super::editor::{Document, load_document};
use super::vfs_prefetch as policy;
use crate::server_session::ServerSession;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};

type Key = (String, String);
const BUDGET: usize = 64 * 1024 * 1024;
const CAPACITY: usize = 128;
const PREFETCH_LIMIT: u64 = 256 * 1024;
struct Entry {
    document: Arc<Document>,
    touched: u64,
    fetched: Instant,
    stale: bool,
}
#[derive(Default)]
struct State {
    entries: HashMap<Key, Entry>,
    flights: HashMap<Key, Weak<tokio::sync::Mutex<()>>>,
    bytes: usize,
    clock: u64,
    epoch: u64,
}
pub(crate) struct Documents {
    state: Mutex<State>,
    prefetch: Arc<tokio::sync::Semaphore>,
}
impl Default for Documents {
    fn default() -> Self {
        Self {
            state: Default::default(),
            prefetch: Arc::new(tokio::sync::Semaphore::new(2)),
        }
    }
}
impl State {
    fn insert(&mut self, key: Key, document: Document, stale: bool) {
        self.clock += 1;
        if let Some(old) = self.entries.remove(&key) {
            self.bytes -= old.document.content.len();
        }
        let size = document.content.len();
        if size > BUDGET {
            return;
        }
        while self.bytes + size > BUDGET || self.entries.len() >= CAPACITY {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.touched)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.bytes -= self.entries.remove(&oldest).unwrap().document.content.len();
        }
        self.bytes += size;
        self.entries.insert(
            key,
            Entry {
                document: Arc::new(document),
                touched: self.clock,
                fetched: Instant::now(),
                stale,
            },
        );
    }
}
impl Documents {
    pub fn invalidate(&self, matches: impl Fn(&str, &str) -> bool) {
        let mut state = self.state.lock().unwrap();
        if !state
            .entries
            .keys()
            .chain(state.flights.keys())
            .any(|(r, p)| matches(r, p))
        {
            return;
        }
        // Fence in-flight reads too, including entries not yet published.
        state.epoch = state.epoch.wrapping_add(1);
        for ((root, path), entry) in &mut state.entries {
            if matches(root, path) {
                entry.stale = true;
            }
        }
    }
    pub fn saved(&self, root: &str, document: Document) {
        self.state
            .lock()
            .unwrap()
            .insert((root.into(), document.path.clone()), document, true);
    }
    fn cached(&self, key: &Key) -> Option<Document> {
        let mut state = self.state.lock().unwrap();
        state.clock += 1;
        let clock = state.clock;
        let entry = state.entries.get_mut(key)?;
        entry.touched = clock;
        let mut document = (*entry.document).clone();
        document.cached = true;
        Some(document)
    }
    pub async fn load(
        &self,
        owner: &Arc<ServerSession>,
        root: &str,
        path: &str,
        allow_cached: bool,
    ) -> Result<Document, String> {
        let key = (root.to_owned(), path.to_owned());
        if allow_cached && let Some(document) = self.cached(&key) {
            return Ok(document);
        }
        self.fetch(key,|| async {
            let mut cancelled=owner.cancellation();
            if *cancelled.borrow() { return Err("Server session closed.".into()); }
            tokio::select! {
                biased;
                _=cancelled.changed()=>Err("Server session closed.".into()),
                result=tokio::time::timeout(Duration::from_secs(60),load_document(owner,root,path))=>
                    result.map_err(|_|"File read timed out.".to_owned())?.map_err(|e|e.to_string()),
            }
        }).await
    }
    async fn fetch<F, Fut>(&self, key: Key, load: F) -> Result<Document, String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Document, String>>,
    {
        let (lock, started) = {
            let mut state = self.state.lock().unwrap();
            state.flights.retain(|_, w| w.strong_count() > 0);
            let lock = state
                .flights
                .get(&key)
                .and_then(Weak::upgrade)
                .unwrap_or_else(|| {
                    let lock = Arc::new(tokio::sync::Mutex::new(()));
                    state.flights.insert(key.clone(), Arc::downgrade(&lock));
                    lock
                });
            (lock, Instant::now())
        };
        let _guard = lock.lock().await;
        let epoch = {
            let state = self.state.lock().unwrap();
            // Concurrent foreground opens and prefetches share the completed read.
            if let Some(entry) = state.entries.get(&key)
                && !entry.stale
                && entry.fetched >= started
            {
                return Ok((*entry.document).clone());
            }
            state.epoch
        };
        let result = load().await;
        let mut state = self.state.lock().unwrap();
        match &result {
            Ok(document) if state.epoch == epoch => state.insert(key, document.clone(), false),
            Err(_) => {
                if let Some(entry) = state.entries.get_mut(&key) {
                    entry.stale = true;
                }
            }
            _ => {} // An event raced the fetch: do not resurrect stale cache data.
        }
        result
    }
    pub fn prefetch(&self, owner: &Arc<ServerSession>, root: &str, listing: &super::Listing) {
        let candidates: Vec<_> = listing
            .entries
            .iter()
            .filter(|e| {
                e.kind == "file"
                    && e.size.is_some_and(|s| s <= PREFETCH_LIMIT)
                    && policy::candidate(root, &e.path)
            })
            .take(64)
            .map(|e| e.path.clone())
            .collect();
        if candidates.is_empty() {
            return;
        }
        let Ok(permit) = self.prefetch.clone().try_acquire_owned() else {
            return;
        };
        let weak = Arc::downgrade(owner);
        let root = root.to_owned();
        tokio::spawn(async move {
            let _permit = permit;
            let Some(owner) = weak.upgrade() else {
                return;
            };
            let Some(input) = candidates
                .iter()
                .map(|p| policy::relative(&root, p))
                .collect::<Option<Vec<_>>>()
            else {
                return;
            };
            let input = format!("{}\0", input.join("\0"));
            let check = async {
                let lease = owner.acquire(owner.server()).await?;
                lease
                    .execute(&policy::command(&root), Some(input.as_bytes()))
                    .await
            };
            let Ok(Ok(output)) = tokio::time::timeout(Duration::from_secs(10), check).await else {
                return;
            };
            let Some(candidates) = policy::allowed(&root, candidates, &output) else {
                return;
            };
            drop(owner);
            for path in candidates.into_iter().take(8) {
                let Some(owner) = weak.upgrade() else {
                    break;
                };
                if owner.is_revoked() {
                    break;
                }
                let key = (root.clone(), path.clone());
                let fresh = owner
                    .files
                    .documents
                    .state
                    .lock()
                    .unwrap()
                    .entries
                    .get(&key)
                    .is_some_and(|e| !e.stale && e.fetched.elapsed() < Duration::from_secs(30));
                if fresh {
                    continue;
                }
                let read = owner.files.documents.fetch(key, || async {
                    super::editor::prefetch_document(&owner, &root, &path, PREFETCH_LIMIT as usize)
                        .await
                        .map_err(|e| e.to_string())
                });
                let mut cancelled = owner.cancellation();
                tokio::select! {
                    biased;
                    _ = cancelled.changed() => break,
                    _ = tokio::time::timeout(Duration::from_secs(10), read) => {}
                }
            }
        });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn doc(path: &str, content: &str) -> Document {
        Document {
            path: path.into(),
            content: content.into(),
            revision: content.into(),
            page: None,
            cached: false,
        }
    }
    #[tokio::test]
    async fn hits_are_local_stale_is_explicit_and_events_fence_late_reads() {
        let cache = Documents::default();
        let key = ("/repo".into(), "/repo/a.rs".into());
        cache
            .fetch(key.clone(), || async { Ok(doc("/repo/a.rs", "base")) })
            .await
            .unwrap();
        let cached = cache.cached(&key).unwrap();
        assert!(cached.cached);
        assert_eq!(cached.content, "base");
        cache.invalidate(|_, _| true);
        assert_eq!(cache.cached(&key).unwrap().content, "base"); // stale display, never save authority
        cache
            .fetch(key.clone(), || async {
                cache.invalidate(|_, _| true);
                Ok(doc("/repo/a.rs", "late"))
            })
            .await
            .unwrap();
        assert_eq!(cache.cached(&key).unwrap().content, "base");
        cache
            .fetch(key.clone(), || async { Ok(doc("/repo/a.rs", "fresh")) })
            .await
            .unwrap();
        assert_eq!(cache.cached(&key).unwrap().content, "fresh");
        assert!(
            cache
                .cached(&("/other".into(), "/repo/a.rs".into()))
                .is_none()
        );
    }
    #[tokio::test]
    async fn simultaneous_reads_share_one_fetch() {
        let cache = Documents::default();
        let key = ("/r".into(), "/r/a".into());
        let first = cache.fetch(key.clone(), || async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            Ok(doc("/r/a", "same"))
        });
        let second = cache.fetch(key, || async { panic!("must join first fetch") });
        let (a, b) = tokio::join!(first, second);
        assert_eq!(a.unwrap().revision, b.unwrap().revision);
    }
    #[test]
    fn cache_has_byte_and_entry_budgets_and_prefetch_excludes_secrets() {
        let mut state = State::default();
        for index in 0..150 {
            state.insert(
                ("/r".into(), index.to_string()),
                doc("/r/a", &"x".repeat(512 * 1024)),
                false,
            );
        }
        assert!(state.bytes <= BUDGET);
        assert!(state.entries.len() <= CAPACITY);
        for name in [
            ".env",
            "private.key",
            "credentials.py",
            "token.ts",
            "app.min.js",
            "huge.bin",
        ] {
            assert!(!policy::candidate("/repo", &format!("/repo/{name}")));
        }
        assert!(policy::candidate("/repo", "/repo/main.rs"));
        assert!(policy::candidate("/repo", "/repo/README.md"));
    }
}
