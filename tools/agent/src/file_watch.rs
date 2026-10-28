//! Bounded, leased OS watches. Notifications invalidate metadata; they never read
//! file contents or authorize writes. One registry belongs to a monitor channel.
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Deserialize)]
struct Request {
    watch: uuid::Uuid,
    root: PathBuf,
    directories: BTreeSet<PathBuf>,
    cursor: Option<String>,
}
#[derive(Default)]
struct Pending {
    paths: BTreeSet<PathBuf>,
    reset: bool,
}
impl Pending {
    fn receive(&mut self, event: notify::Result<notify::Event>) {
        match event {
            Ok(event) if !matches!(event.kind, notify::EventKind::Access(_)) => {
                self.reset |= event.need_rescan();
                for path in event.paths {
                    if path.to_str().is_none_or(|p| p.len() > 16384) {
                        self.reset = true;
                        continue;
                    }
                    if self.paths.len() < 2048 {
                        self.paths.insert(path);
                    } else {
                        self.reset = true;
                    }
                }
            }
            Err(_) => self.reset = true,
            _ => {}
        }
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
    pub fn handle(&self, value: &Value) -> Value {
        self.poll(value).unwrap_or_else(|e| json!({"error":e}))
    }
    fn poll(&self, value: &Value) -> Result<Value, String> {
        let request: Request = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if request.directories.len() > 64 || !request.root.is_absolute() {
            return Err("Invalid project watch root or watch limit.".into());
        }
        for directory in &request.directories {
            if !directory.starts_with(&request.root)
                || directory.as_os_str().len() > 16384
                || directory
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err("Watch directory is outside the project.".into());
            }
        }
        let mut entries = self.0.lock().unwrap();
        if request.directories.is_empty() {
            entries.remove(&request.watch);
            return Ok(json!({"mode":"native", "cursor":null,"paths":[],"reset":false}));
        }
        if entries
            .get(&request.watch)
            .is_some_and(|e| e.root != request.root)
        {
            entries.remove(&request.watch);
        }
        if !entries.contains_key(&request.watch) {
            if request.root.canonicalize().ok().as_ref() != Some(&request.root) {
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
            let watcher =
                notify::recommended_watcher(move |event| callback.lock().unwrap().receive(event))
                    .map_err(|e| e.to_string())?;
            entries.insert(
                request.watch,
                Entry {
                    watcher,
                    root: request.root.clone(),
                    directories: BTreeSet::new(),
                    pending,
                    cursor: uuid::Uuid::new_v4().to_string(),
                    touched: Instant::now(),
                },
            );
        }
        let entry = entries.get_mut(&request.watch).unwrap();
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
            entry.pending.lock().unwrap().paths.insert(path);
        }
        for removed in entry.directories.difference(&request.directories) {
            let _ = entry.watcher.unwatch(removed);
        }
        entry
            .directories
            .retain(|p| request.directories.contains(p));
        for added in request
            .directories
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
            entry.pending.lock().unwrap().reset = true;
        }
        let mut pending = std::mem::take(&mut *entry.pending.lock().unwrap());
        pending.reset |= request.cursor.as_deref() != Some(&entry.cursor);
        // Advance on every drain. A lost reply causes a full rescan on retry.
        entry.cursor = uuid::Uuid::new_v4().to_string();
        Ok(
            json!({"mode":"native","cursor":entry.cursor,"reset":pending.reset,
            "paths":pending.paths.into_iter().filter(|p| p.starts_with(&entry.root)).collect::<Vec<_>>()}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_events_atomic_replace_delete_scope_and_lost_reply() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let watches = Watches::new();
        let mut request =
            json!({"watch":uuid::Uuid::new_v4(),"root":root,"directories":[root],"cursor":null});
        let first = watches.handle(&request);
        assert_eq!(first["reset"], true, "{first}");
        request["cursor"] = first["cursor"].clone();
        let file = root.join("hello.rs");
        std::fs::write(&file, "hello").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let reply = watches.handle(&request);
            request["cursor"] = reply["cursor"].clone();
            if reply["paths"].as_array().unwrap().contains(&json!(file)) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "No native creation event: {reply}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        std::fs::write(root.join("temp"), "replacement").unwrap();
        std::fs::rename(root.join("temp"), &file).unwrap();
        std::fs::remove_file(&file).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        let reply = watches.handle(&request);
        assert!(reply["paths"].as_array().unwrap().contains(&json!(file)));
        assert_eq!(watches.handle(&request)["reset"], true); // lost reply
        request["directories"] = json!([root.parent().unwrap()]);
        assert!(watches.handle(&request)["error"].is_string());
        request["directories"] = json!([]);
        watches.handle(&request);
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
    fn deleted_folders_are_rearmed_and_deleted_roots_can_release() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("project");
        let child = root.join("src");
        std::fs::create_dir_all(&child).unwrap();
        let watches = Watches::new();
        let mut request = json!({"watch":uuid::Uuid::new_v4(),"root":root,"directories":[root,child],"cursor":null});
        let first = watches.handle(&request);
        request["cursor"] = first["cursor"].clone();
        std::fs::remove_dir(&child).unwrap();
        let deleted = watches.handle(&request);
        request["cursor"] = deleted["cursor"].clone();
        assert!(deleted["paths"].as_array().unwrap().contains(&json!(child)));
        std::fs::create_dir(&child).unwrap();
        let rearmed = watches.handle(&request);
        request["cursor"] = rearmed["cursor"].clone();
        assert_eq!(rearmed["reset"], true);
        let file = child.join("after.rs");
        std::fs::write(&file, "hello").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let reply = watches.handle(&request);
            request["cursor"] = reply["cursor"].clone();
            if reply["paths"].as_array().unwrap().contains(&json!(file)) {
                break;
            }
            assert!(Instant::now() < deadline, "Recreated folder is not watched");
            std::thread::sleep(Duration::from_millis(50));
        }
        std::fs::remove_dir_all(&root).unwrap();
        let removed = watches.handle(&request);
        assert!(removed["paths"].as_array().unwrap().contains(&json!(root)));
        request["directories"] = json!([]);
        assert!(watches.handle(&request)["error"].is_null());
        assert!(watches.0.lock().unwrap().is_empty());
    }
}
