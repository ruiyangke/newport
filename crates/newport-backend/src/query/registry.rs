//! Bounded parameterized cache. The owning server/feature supplies the scope;
//! this lock protects lookup only, never a loader or an awaited read.
use super::{Policy, Query};
use std::{
    collections::HashMap,
    future::Future,
    hash::Hash,
    sync::{Arc, Mutex},
};
use tokio::time::Instant;

struct Entry<T, E> {
    query: Arc<Query<T, E>>,
    touched: Instant,
}
pub struct Registry<K, T, E = String> {
    entries: Mutex<HashMap<K, Entry<T, E>>>,
    capacity: usize,
    closed: std::sync::atomic::AtomicBool,
}
impl<
    K: Eq + Hash,
    T: Clone + Send + Sync + 'static,
    E: Clone + From<String> + Send + Sync + 'static,
> Registry<K, T, E>
{
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0);
        Self {
            entries: Mutex::new(HashMap::new()),
            capacity,
            closed: Default::default(),
        }
    }
    pub fn get_or_insert<F, Fut>(
        &self,
        key: K,
        policy: Policy,
        loader: F,
    ) -> Result<Arc<Query<T, E>>, E>
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
        K: Clone,
    {
        let mut entries = self.entries.lock().expect("query registry");
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err("Query scope closed.".to_owned().into());
        }
        if let Some(entry) = entries.get_mut(&key) {
            entry.touched = Instant::now();
            return Ok(entry.query.clone());
        }
        if entries.len() >= self.capacity {
            let oldest = entries
                .iter()
                .filter(|(_, entry)| Arc::strong_count(&entry.query) == 1 && !entry.query.active())
                .min_by_key(|(_, entry)| entry.touched)
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else {
                return Err("Too many active queries. Try again shortly."
                    .to_owned()
                    .into());
            };
            entries.remove(&oldest);
        }
        let query = Arc::new(Query::new_with_error(policy, loader));
        entries.insert(
            key,
            Entry {
                query: query.clone(),
                touched: Instant::now(),
            },
        );
        Ok(query)
    }
    pub fn invalidate(&self, matches: impl Fn(&K) -> bool) {
        for (key, entry) in self.entries.lock().expect("query registry").iter() {
            if matches(key) {
                entry.query.invalidate();
            }
        }
    }
    pub fn close(&self) {
        let mut entries = self.entries.lock().expect("query registry");
        self.closed
            .store(true, std::sync::atomic::Ordering::Release);
        for (_, entry) in entries.drain() {
            entry.query.close();
        }
    }
}
