//! Bounded, leased OS watches. Notifications invalidate metadata; they never read
//! file contents or authorize writes. One registry belongs to a monitor channel.
use newport_protocol::{
    common::{Error, Retry},
    monitor::file_watch::{MAX_PATHS, MAX_REPLY_TEXT, Reply, Request},
};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
struct Pending {
    paths: BTreeSet<PathBuf>,
    reset: bool,
    bytes: usize,
}
impl Pending {
    fn receive(&mut self, event: notify::Result<notify::Event>) {
        match event {
            Ok(event) if !matches!(event.kind, notify::EventKind::Access(_)) => {
                self.reset |= event.need_rescan();
                for path in event.paths {
                    self.insert(path);
                }
            }
            Err(_) => self.reset = true,
            _ => {}
        }
    }
    fn insert(&mut self, path: PathBuf) {
        let Some(value) = path
            .to_str()
            .filter(|value| newport_protocol::monitor::file_watch::path(value))
        else {
            self.reset = true;
            return;
        };
        if self.paths.contains(&path) {
            return;
        }
        if self.paths.len() == MAX_PATHS || self.bytes.saturating_add(value.len()) > MAX_REPLY_TEXT
        {
            self.reset = true;
            return;
        }
        self.bytes += value.len();
        self.paths.insert(path);
    }
}
struct Entry {
    watcher: RecommendedWatcher,
    root: PathBuf,
    directories: BTreeSet<PathBuf>,
    pending: Arc<Mutex<Pending>>,
    cursor: String,
    touched: Instant,
}
pub struct Watches(Arc<Mutex<HashMap<uuid::Uuid, Entry>>>);
impl Watches {
    pub fn new() -> Self {
        let entries = Arc::new(Mutex::new(HashMap::<uuid::Uuid, Entry>::new()));
        let weak = Arc::downgrade(&entries);
        // Dropping the channel drops every OS watch. Inactive clients also expire
        // even when the channel remains open for other project windows.
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(15));
                let Some(entries) = weak.upgrade() else {
                    break;
                };
                entries
                    .lock()
                    .unwrap()
                    .retain(|_, entry| entry.touched.elapsed() < Duration::from_secs(45));
            }
        });
        Self(entries)
    }
    pub fn handle(&self, request: &Request) -> Result<Reply, Error> {
        request.validate().map_err(|e| Error {
            code: "invalid_request".into(),
            message: e.to_string(),
            retry: Retry::No,
        })?;
        let reply = self.poll(request).map_err(|message| Error {
            code: "file_watch_failed".into(),
            message,
            retry: Retry::No,
        })?;
        reply.validate_for(request).map_err(|e| Error {
            code: "invalid_response".into(),
            message: e.to_string(),
            retry: Retry::No,
        })?;
        Ok(reply)
    }
    fn poll(&self, request: &Request) -> Result<Reply, String> {
        let watch = uuid::Uuid::parse_str(&request.watch).map_err(|e| e.to_string())?;
        let root = PathBuf::from(&request.root);
        let directories: BTreeSet<PathBuf> =
            request.directories.iter().map(PathBuf::from).collect();
        let mut entries = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if directories.is_empty() {
            entries.remove(&watch);
            return Ok(Reply::default());
        }
        if entries.get(&watch).is_some_and(|e| e.root != root) {
            entries.remove(&watch);
        }
        if !entries.contains_key(&watch) {
            if root.canonicalize().ok().as_ref() != Some(&root) {
                return Err("Invalid project watch root.".into());
            }
            if entries.len() >= 8 {
                return Err("Too many project watches.".into());
            }
            let pending = Arc::new(Mutex::new(Pending {
                reset: true,
                ..Default::default()
            }));
            let callback = pending.clone();
            let watcher = notify::recommended_watcher(move |event| {
                callback
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .receive(event)
            })
            .map_err(|e| e.to_string())?;
            entries.insert(
                watch,
                Entry {
                    watcher,
                    root: root.clone(),
                    directories: BTreeSet::new(),
                    pending,
                    cursor: uuid::Uuid::new_v4().to_string(),
                    touched: Instant::now(),
                },
            );
        }
        let entry = entries.get_mut(&watch).ok_or("File watch disappeared")?;
        entry.touched = Instant::now();
        let missing: Vec<_> = entry
            .directories
            .iter()
            .filter(|p| p.canonicalize().ok().as_ref() != Some(*p) || !p.is_dir())
            .cloned()
            .collect();
        for path in missing {
            let _ = entry.watcher.unwatch(&path);
            entry.directories.remove(&path);
            entry
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(path);
        }
        for removed in entry.directories.difference(&directories) {
            let _ = entry.watcher.unwatch(removed);
        }
        entry.directories.retain(|p| directories.contains(p));
        for added in directories
            .difference(&entry.directories)
            .cloned()
            .collect::<Vec<_>>()
        {
            // Refuse links, including intermediate components. A deleted folder
            // remains unwatched and will be retried after its parent reports it.
            if added.canonicalize().ok().as_ref() != Some(&added) || !added.is_dir() {
                continue;
            }
            entry
                .watcher
                .watch(&added, RecursiveMode::NonRecursive)
                .map_err(|e| e.to_string())?;
            entry.directories.insert(added);
            entry
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .reset = true;
        }
        let mut pending = std::mem::take(
            &mut *entry
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        pending.reset |= request.cursor.as_deref() != Some(&entry.cursor);
        // Advance on every drain. A lost reply causes a full rescan on retry.
        entry.cursor = uuid::Uuid::new_v4().to_string();
        Ok(Reply {
            cursor: Some(entry.cursor.clone()),
            reset: pending.reset,
            paths: pending
                .paths
                .into_iter()
                .filter(|path| path.starts_with(&entry.root))
                .filter_map(|path| path.into_os_string().into_string().ok())
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_protocol::monitor::file_watch::MAX_PATH;
    use std::path::Path;
    #[test]
    fn real_events_atomic_replace_delete_scope_and_lost_reply() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let watches = Watches::new();
        let mut request = Request {
            watch: uuid::Uuid::new_v4().to_string(),
            root: root.to_str().unwrap().into(),
            directories: vec![root.to_str().unwrap().into()],
            cursor: None,
        };
        let first = watches.handle(&request).unwrap();
        assert!(first.reset, "{first:?}");
        request.cursor = first.cursor.clone();
        let file = root.join("hello.rs");
        std::fs::write(&file, "hello").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let reply = watches.handle(&request).unwrap();
            request.cursor = reply.cursor.clone();
            if reply.paths.iter().any(|path| Path::new(path) == file) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "No native creation event: {reply:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        std::fs::write(root.join("temp"), "replacement").unwrap();
        std::fs::rename(root.join("temp"), &file).unwrap();
        std::fs::remove_file(&file).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        let reply = watches.handle(&request).unwrap();
        assert!(reply.paths.iter().any(|path| Path::new(path) == file));
        assert!(watches.handle(&request).unwrap().reset); // lost reply
        request.directories = vec![root.parent().unwrap().to_str().unwrap().into()];
        assert!(watches.handle(&request).is_err());
        request.directories.clear();
        watches.handle(&request).unwrap();
        assert!(watches.0.lock().unwrap().is_empty());
    }
    #[test]
    fn overflow_is_bounded_and_requests_rescan() {
        let mut pending = Pending::default();
        for index in 0..3000 {
            pending.receive(Ok(notify::Event::new(notify::EventKind::Any)
                .add_path(PathBuf::from(format!("/repo/{index}")))));
        }
        assert_eq!(pending.paths.len(), 2048);
        assert!(pending.reset);
    }
    #[test]
    fn long_paths_share_one_byte_budget_and_duplicate_events_do_not_overflow() {
        let mut pending = Pending::default();
        for index in 0..MAX_PATHS {
            pending.insert(PathBuf::from(format!("/repo/{index}")));
        }
        assert!(!pending.reset);
        pending.insert(PathBuf::from("/repo/0"));
        assert!(!pending.reset);
        let mut pending = Pending::default();
        for index in 0..MAX_PATHS {
            pending.insert(PathBuf::from(format!(
                "/repo/{index}/{}",
                "x".repeat(MAX_PATH - 30)
            )));
        }
        assert!(pending.reset);
        assert!(pending.bytes <= MAX_REPLY_TEXT);
        assert!(pending.paths.len() < MAX_PATHS);
    }
    #[test]
    fn deleted_folders_are_rearmed_and_deleted_roots_can_release() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("project");
        let child = root.join("src");
        std::fs::create_dir_all(&child).unwrap();
        let watches = Watches::new();
        let mut request = Request {
            watch: uuid::Uuid::new_v4().to_string(),
            root: root.to_str().unwrap().into(),
            directories: vec![
                root.to_str().unwrap().into(),
                child.to_str().unwrap().into(),
            ],
            cursor: None,
        };
        let first = watches.handle(&request).unwrap();
        request.cursor = first.cursor.clone();
        std::fs::remove_dir(&child).unwrap();
        let deleted = watches.handle(&request).unwrap();
        request.cursor = deleted.cursor.clone();
        assert!(deleted.paths.iter().any(|path| Path::new(path) == child));
        std::fs::create_dir(&child).unwrap();
        let rearmed = watches.handle(&request).unwrap();
        request.cursor = rearmed.cursor.clone();
        assert!(rearmed.reset);
        let file = child.join("after.rs");
        std::fs::write(&file, "hello").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let reply = watches.handle(&request).unwrap();
            request.cursor = reply.cursor.clone();
            if reply.paths.iter().any(|path| Path::new(path) == file) {
                break;
            }
            assert!(Instant::now() < deadline, "Recreated folder is not watched");
            std::thread::sleep(Duration::from_millis(50));
        }
        std::fs::remove_dir_all(&root).unwrap();
        let removed = watches.handle(&request).unwrap();
        assert!(removed.paths.iter().any(|path| Path::new(path) == root));
        request.directories.clear();
        watches.handle(&request).unwrap();
        assert!(watches.0.lock().unwrap().is_empty());
    }
}
