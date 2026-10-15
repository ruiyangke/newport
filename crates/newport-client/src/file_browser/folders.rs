//! Lazy, scoped directory-tree reads. Expansion is explicit; cached results do
//! not start reads, retry failures, or interfere with the main file listing.
use super::{Backend, Intent, ReadSlot, Value, next, valid_listing};
use std::collections::{BTreeMap, HashSet, VecDeque};

const PARALLEL_READS: usize = 2;
const CACHED_FOLDERS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Directory {
    pub name: String,
    pub path: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Folder {
    pub directories: Vec<Directory>,
    /// A successful read exists, including an empty directory.
    pub loaded: bool,
    /// Includes a cancelled physical read while its workers drain.
    pub pending: bool,
    pub error: Option<String>,
    pub truncated: bool,
}

struct Entry {
    folder: Folder,
    token: u64,
    wanted: bool,
}

pub struct Folders {
    backend: Backend,
    server: String,
    connection: Value,
    retired: bool,
    changed: bool,
    token: u64,
    cache: BTreeMap<String, Entry>,
    pinned: HashSet<String>,
    queued: VecDeque<Intent>,
    reads: [ReadSlot; PARALLEL_READS],
}

impl Folders {
    /// Construction is idle. A controller belongs to one immutable connection.
    pub fn new(backend: Backend, server: String, connection: Value) -> Self {
        Self {
            backend,
            server,
            connection,
            retired: false,
            changed: false,
            token: 0,
            cache: BTreeMap::new(),
            pinned: HashSet::new(),
            queued: VecDeque::new(),
            reads: std::array::from_fn(|_| ReadSlot::default()),
        }
    }

    pub fn get(&self, path: &str) -> Option<&Folder> {
        self.cache.get(path).map(|entry| &entry.folder)
    }

    /// Schedules a fresh read for an explicit expansion or retry. Coalesces a
    /// current pending request, but permits re-expansion after cancellation.
    /// Returns false for invalid paths, retirement, or a fully occupied cache.
    /// Idle entries are evicted by least recent request; busy entries and paths
    /// retained by `cancel_except` stay put.
    pub fn request(&mut self, path: String) -> bool {
        if self.retired || !valid_path(&path) {
            return false;
        }
        if self
            .cache
            .get(&path)
            .is_some_and(|entry| entry.wanted && entry.folder.pending)
        {
            return false;
        }
        if !self.cache.contains_key(&path) && self.cache.len() == CACHED_FOLDERS {
            let oldest = self
                .cache
                .iter()
                .filter(|(path, entry)| !entry.folder.pending && !self.pinned.contains(*path))
                .min_by_key(|(_, entry)| entry.token)
                .map(|(path, _)| path.clone());
            let Some(oldest) = oldest else {
                return false;
            };
            self.cache.remove(&oldest);
        }
        self.token = next(self.token);
        let entry = self.cache.entry(path.clone()).or_insert_with(|| Entry {
            folder: Folder::default(),
            token: self.token,
            wanted: true,
        });
        entry.token = self.token;
        entry.wanted = true;
        entry.folder.pending = true;
        if !entry.folder.loaded {
            entry.folder.error = None;
        }
        self.queued.push_back(Intent {
            refresh: true,
            token: self.token,
            path,
        });
        self.launch();
        self.changed = true;
        true
    }

    /// Pins the currently expanded branches and cancels other reads. A
    /// cancellation acknowledgement alone does not free a physical read;
    /// both workers must finish first.
    pub fn cancel_except(&mut self, paths: &[String]) {
        self.pinned = paths.iter().cloned().collect();
        let keep: HashSet<&str> = paths.iter().map(String::as_str).collect();
        self.queued
            .retain(|intent| keep.contains(intent.path.as_str()));
        for (path, entry) in &mut self.cache {
            if !keep.contains(path.as_str()) {
                entry.wanted = false;
            }
        }
        for slot in &mut self.reads {
            if slot
                .active
                .as_ref()
                .is_some_and(|work| !keep.contains(work.intent.path.as_str()))
            {
                slot.clear(&self.backend);
            }
        }
        self.update_pending();
    }

    /// Invalidates all cached results and queued intentions. Draining workers
    /// still occupy their slots, and cannot refill the cleared cache.
    pub fn clear(&mut self) {
        self.suspend();
        if !self.cache.is_empty() {
            self.cache.clear();
            self.changed = true;
        }
    }

    /// Preserve successful data without restarting anything on return.
    pub fn suspend(&mut self) {
        self.cancel_except(&[]);
    }

    pub fn retire(&mut self) {
        self.suspend();
        self.retired = true;
    }

    pub fn has_pending(&self) -> bool {
        !self.queued.is_empty() || self.reads.iter().any(ReadSlot::pending)
    }

    pub fn poll(&mut self) -> bool {
        let mut changed = std::mem::take(&mut self.changed);
        for slot in &mut self.reads {
            if let Some((intent, result)) = slot.poll(&self.backend, &mut changed)
                && !self.retired
                && let Some(entry) = self.cache.get_mut(&intent.path)
                && entry.wanted
                && entry.token == intent.token
            {
                match result.and_then(directories) {
                    Ok((directories, truncated)) => {
                        entry.folder.loaded = true;
                        entry.folder.directories = directories;
                        entry.folder.truncated = truncated;
                        entry.folder.error = None;
                    }
                    Err(error) => entry.folder.error = Some(error),
                }
                changed = true;
            }
        }
        self.launch();
        self.update_pending();
        changed | std::mem::take(&mut self.changed)
    }

    fn launch(&mut self) {
        if self.retired {
            return;
        }
        for slot in &mut self.reads {
            if slot.pending() {
                continue;
            }
            let Some(intent) = self.queued.pop_front() else {
                break;
            };
            slot.replace(&self.backend, intent);
            slot.launch(&self.backend, &self.server, &self.connection, "files_list");
        }
    }

    fn update_pending(&mut self) {
        for (path, entry) in &mut self.cache {
            let pending = self.queued.iter().any(|intent| &intent.path == path)
                || self.reads.iter().any(|slot| {
                    slot.active
                        .as_ref()
                        .is_some_and(|work| &work.intent.path == path)
                });
            if entry.folder.pending != pending {
                entry.folder.pending = pending;
                self.changed = true;
            }
        }
    }
}

impl Drop for Folders {
    fn drop(&mut self) {
        for slot in &mut self.reads {
            slot.clear(&self.backend);
        }
    }
}

fn valid_path(path: &str) -> bool {
    !path.is_empty() && path.len() <= 16_384 && !path.contains('\0')
}

fn directories(value: Value) -> Result<(Vec<Directory>, bool), String> {
    let value = valid_listing(value)?;
    let parent = value["path"].as_str().unwrap();
    if !parent.starts_with('/') {
        return Err("Invalid canonical folder path from the backend.".into());
    }
    let truncated = match value.get("truncated") {
        None => false,
        Some(Value::Bool(value)) => *value,
        _ => return Err("Invalid folder truncation flag from the backend.".into()),
    };
    let mut directories = Vec::new();
    let mut seen = HashSet::new();
    for entry in value["entries"].as_array().unwrap() {
        // Symlinks are not traversed implicitly, matching the original tree.
        if entry["kind"].as_str() != Some("directory") {
            continue;
        }
        let Some((name, path)) = entry["name"].as_str().zip(entry["path"].as_str()) else {
            return Err("Invalid directory entry from the backend.".into());
        };
        if name.is_empty()
            || matches!(name, "." | "..")
            || name.contains('/')
            || !valid_path(path)
            || path != format!("{}/{name}", parent.trim_end_matches('/'))
            || !seen.insert(path)
        {
            return Err("Invalid child directory path from the backend.".into());
        }
        directories.push(Directory {
            name: name.to_owned(),
            path: path.to_owned(),
        });
    }
    Ok((directories, truncated))
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_platform::ipc::protocol::{Request, Response, VERSION};
    use newport_platform::ipc::{Listener, Stream};
    use serde_json::json;
    use std::{
        io::{Read, Write},
        path::PathBuf,
        thread,
        time::{Duration, Instant},
    };

    struct Host {
        path: PathBuf,
        listener: Listener,
    }
    impl Host {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("np-ft-{}.sock", uuid::Uuid::new_v4().simple()));
            let listener = Listener::bind(&path).unwrap();
            listener.set_nonblocking(true).unwrap();
            Self { path, listener }
        }
        fn folders(&self) -> Folders {
            Folders::new(
                Backend::at_path(self.path.clone()),
                "server-a".into(),
                json!({"host":"original.example","port":22}),
            )
        }
        fn receive(&self, method: &str) -> (Stream, Request) {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match self.listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let mut header = [0; 4];
                        stream.read_exact(&mut header).unwrap();
                        let size = u32::from_be_bytes(header) as usize;
                        assert!(size < 1024 * 1024);
                        let mut bytes = vec![0; size];
                        stream.read_exact(&mut bytes).unwrap();
                        let request: Request = rmp_serde::from_slice(&bytes).unwrap();
                        assert_eq!(request.version, VERSION);
                        assert_eq!(request.method, method);
                        return (stream, request);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "missing {method} RPC");
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("{error}"),
                }
            }
        }
        fn quiet(&self) {
            let deadline = Instant::now() + Duration::from_millis(15);
            while Instant::now() < deadline {
                match self.listener.accept() {
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    _ => panic!("unexpected RPC"),
                }
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
    impl Drop for Host {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
    fn reply(mut stream: Stream, result: Result<Value, String>) {
        let (result, error) = match result {
            Ok(value) => (value, None),
            Err(error) => (Value::Null, Some(json!(error))),
        };
        let bytes = rmp_serde::to_vec_named(&Response {
            version: VERSION,
            result,
            error,
        })
        .unwrap();
        stream
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .unwrap();
        stream.write_all(&bytes).unwrap();
    }
    fn listing(path: &str, child: &str) -> Value {
        json!({"path":path,"entries":[{
            "name":child,"path":format!("{}/{child}",path.trim_end_matches('/')),"kind":"directory"
        }]})
    }
    fn until(folders: &mut Folders, done: impl Fn(&Folders) -> bool) {
        for _ in 0..1000 {
            folders.poll();
            if done(folders) {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("folders did not reach expected state");
    }
    fn load(host: &Host, folders: &mut Folders, path: &str, child: &str) {
        assert!(folders.request(path.into()));
        let (stream, request) = host.receive("files_list");
        assert_eq!(request.args["path"], path);
        reply(stream, Ok(listing(path, child)));
        until(folders, |folders| !folders.has_pending());
    }

    #[test]
    fn lazy_reads_keep_exact_scope_paths_directory_order_and_truncation() {
        let host = Host::new();
        let mut folders = host.folders();
        assert!(!folders.has_pending());
        assert!(!folders.poll());
        assert!(folders.get("/").is_none());
        for path in ["", "bad\0path"] {
            assert!(!folders.request(path.into()));
        }
        host.quiet();
        let path = "/資料/it's here";
        assert!(folders.request(path.into()));
        assert!(!folders.request(path.into()));
        let (stream, request) = host.receive("files_list");
        assert_eq!(request.args["path"], path);
        assert_eq!(request.args["id"], "server-a");
        assert_eq!(request.args["expectedConnection"], folders.connection);
        assert!(uuid::Uuid::parse_str(request.args["operation"].as_str().unwrap()).is_ok());
        reply(
            stream,
            Ok(json!({"path":"/canonical","truncated":true,"entries":[
                {"name":"z","path":"/canonical/z","kind":"directory"},
                {"name":"link","path":"/canonical/link","kind":"symlink"},
                {"name":"a.txt","path":"/canonical/a.txt","kind":"file"},
                {"name":"a","path":"/canonical/a","kind":"directory"}
            ]})),
        );
        until(&mut folders, |folders| !folders.has_pending());
        let folder = folders.get(path).unwrap();
        assert!(folder.error.is_none() && folder.truncated && !folder.pending);
        assert_eq!(
            folder.directories,
            vec![
                Directory {
                    name: "z".into(),
                    path: "/canonical/z".into()
                },
                Directory {
                    name: "a".into(),
                    path: "/canonical/a".into()
                }
            ]
        );
        assert!(!folders.poll());
        host.quiet();
        // A subsequent expansion refreshes the same folder exactly once.
        load(&host, &mut folders, path, "fresh");
        assert_eq!(folders.get(path).unwrap().directories[0].name, "fresh");
    }

    #[test]
    fn parallelism_is_two_and_queued_paths_are_fifo_and_coalesced() {
        let host = Host::new();
        let mut folders = host.folders();
        for path in ["/a", "/b", "/c", "/d"] {
            assert!(folders.request(path.into()));
            assert!(!folders.request(path.into()));
        }
        let first = host.receive("files_list");
        let second = host.receive("files_list");
        let paths = [
            first.1.args["path"].as_str().unwrap(),
            second.1.args["path"].as_str().unwrap(),
        ];
        assert!(paths.contains(&"/a") && paths.contains(&"/b"));
        host.quiet();
        reply(first.0, Ok(listing(paths[0], "one")));
        until(&mut folders, |folders| {
            folders.reads.iter().any(|slot| {
                slot.active
                    .as_ref()
                    .is_some_and(|work| work.intent.path == "/c")
            })
        });
        let third = host.receive("files_list");
        assert_eq!(third.1.args["path"], "/c");
        host.quiet();
        reply(second.0, Ok(listing(paths[1], "two")));
        until(&mut folders, |folders| {
            folders.reads.iter().any(|slot| {
                slot.active
                    .as_ref()
                    .is_some_and(|work| work.intent.path == "/d")
            })
        });
        let fourth = host.receive("files_list");
        assert_eq!(fourth.1.args["path"], "/d");
        reply(third.0, Ok(listing("/c", "three")));
        reply(fourth.0, Ok(listing("/d", "four")));
        until(&mut folders, |folders| !folders.has_pending());
        assert_eq!(folders.get("/d").unwrap().directories[0].name, "four");
        host.quiet();
    }

    #[test]
    fn cancellation_acknowledgement_cannot_release_slot_or_accept_old_reply() {
        let host = Host::new();
        let mut folders = host.folders();
        folders.request("/a".into());
        let (old, old_request) = host.receive("files_list");
        folders.request("/b".into());
        let (busy, _) = host.receive("files_list");
        folders.cancel_except(&["/b".into()]);
        let (cancel, request) = host.receive("files_cancel");
        assert_eq!(request.args["operation"], old_request.args["operation"]);
        assert!(folders.get("/a").unwrap().pending);
        assert!(folders.request("/a".into()));
        reply(cancel, Ok(Value::Null));
        until(&mut folders, |folders| {
            folders.reads[0].active.as_ref().unwrap().cancel.is_none()
        });
        host.quiet();
        reply(old, Ok(listing("/a", "stale")));
        until(&mut folders, |folders| folders.queued.is_empty());
        assert!(folders.get("/a").unwrap().directories.is_empty());
        let (fresh, request) = host.receive("files_list");
        assert_eq!(request.args["path"], "/a");
        assert_ne!(request.args["operation"], old_request.args["operation"]);
        reply(fresh, Ok(listing("/a", "fresh")));
        reply(busy, Ok(listing("/b", "other")));
        until(&mut folders, |folders| !folders.has_pending());
        assert_eq!(folders.get("/a").unwrap().directories[0].name, "fresh");
    }

    #[test]
    fn cancelled_authoritative_reply_stays_pending_until_cancel_worker_drains() {
        let host = Host::new();
        let mut folders = host.folders();
        folders.request("/a".into());
        let (read, _) = host.receive("files_list");
        folders.cancel_except(&[]);
        let (cancel, _) = host.receive("files_cancel");
        reply(read, Ok(listing("/a", "stale")));
        until(&mut folders, |folders| {
            folders.reads[0].active.as_ref().unwrap().reply.is_none()
        });
        assert!(folders.get("/a").unwrap().pending);
        assert!(folders.get("/a").unwrap().directories.is_empty());
        assert!(folders.has_pending());
        reply(cancel, Ok(Value::Null));
        until(&mut folders, |folders| !folders.has_pending());
        assert!(!folders.get("/a").unwrap().pending);
        host.quiet();
    }

    #[test]
    fn clear_invalidates_same_path_and_waits_for_old_workers() {
        let host = Host::new();
        let mut folders = host.folders();
        load(&host, &mut folders, "/cached", "retained");
        folders.request("/a".into());
        let (old, _) = host.receive("files_list");
        folders.clear();
        assert!(folders.get("/cached").is_none());
        assert!(folders.get("/a").is_none());
        assert!(folders.has_pending());
        let (cancel, _) = host.receive("files_cancel");
        folders.request("/a".into());
        // The other free physical slot may serve the new generation safely.
        let (fresh, _) = host.receive("files_list");
        reply(fresh, Ok(listing("/a", "new")));
        until(&mut folders, |folders| {
            !folders.get("/a").unwrap().directories.is_empty()
        });
        reply(old, Ok(listing("/a", "old")));
        reply(cancel, Ok(Value::Null));
        until(&mut folders, |folders| !folders.has_pending());
        assert_eq!(folders.get("/a").unwrap().directories[0].name, "new");
        assert!(folders.get("/cached").is_none());
    }

    #[test]
    fn failure_retains_success_and_requires_explicit_retry() {
        let host = Host::new();
        let mut folders = host.folders();
        load(&host, &mut folders, "/a", "retained");
        folders.request("/a".into());
        let (read, request) = host.receive("files_list");
        reply(read, Err("Permission denied".into()));
        until(&mut folders, |folders| {
            folders.get("/a").unwrap().error.is_some()
        });
        let (cancel, cancellation) = host.receive("files_cancel");
        assert_eq!(cancellation.args["operation"], request.args["operation"]);
        assert!(folders.get("/a").unwrap().pending);
        assert!(!folders.request("/a".into()));
        reply(cancel, Ok(Value::Null));
        until(&mut folders, |folders| !folders.has_pending());
        let folder = folders.get("/a").unwrap();
        assert_eq!(folder.directories[0].name, "retained");
        assert_eq!(folder.error.as_deref(), Some("Permission denied"));
        for _ in 0..5 {
            folders.poll();
        }
        host.quiet();
        load(&host, &mut folders, "/a", "fresh");
        assert!(folders.get("/a").unwrap().error.is_none());
    }

    #[test]
    fn suspend_cancels_inflight_and_queued_without_losing_success_and_retire_is_final() {
        let host = Host::new();
        let mut folders = host.folders();
        load(&host, &mut folders, "/cached", "retained");
        folders.request("/a".into());
        folders.request("/b".into());
        folders.request("/queued".into());
        let first = host.receive("files_list");
        let second = host.receive("files_list");
        folders.suspend();
        assert!(!folders.get("/queued").unwrap().pending);
        assert!(folders.queued.is_empty());
        let cancel1 = host.receive("files_cancel");
        let cancel2 = host.receive("files_cancel");
        reply(
            first.0,
            Ok(listing(first.1.args["path"].as_str().unwrap(), "stale")),
        );
        reply(
            second.0,
            Ok(listing(second.1.args["path"].as_str().unwrap(), "stale")),
        );
        reply(cancel1.0, Ok(Value::Null));
        reply(cancel2.0, Ok(Value::Null));
        until(&mut folders, |folders| !folders.has_pending());
        assert_eq!(
            folders.get("/cached").unwrap().directories[0].name,
            "retained"
        );
        assert!(folders.get("/a").unwrap().directories.is_empty());
        host.quiet();
        load(&host, &mut folders, "/a", "fresh");
        folders.retire();
        assert!(!folders.request("/a".into()));
        assert!(!folders.request("/other".into()));
        host.quiet();
    }

    #[test]
    fn cache_never_evicts_busy_or_expanded_folders_and_reuses_oldest_idle_entry() {
        let host = Host::new();
        let mut folders = host.folders();
        let paths: Vec<String> = (0..CACHED_FOLDERS)
            .map(|n| format!("/folder-{n}"))
            .collect();
        for path in &paths {
            load(&host, &mut folders, path, "retained");
        }
        folders.cancel_except(&paths);
        assert!(!folders.request("/overflow".into()));
        assert!(paths.iter().all(|path| folders.get(path).is_some()));
        folders.cancel_except(&paths[1..]);
        folders.request(paths[0].clone());
        let (busy, _) = host.receive("files_list");
        assert!(!folders.request("/overflow".into()));
        reply(busy, Ok(listing(&paths[0], "fresh")));
        until(&mut folders, |folders| !folders.has_pending());
        assert!(folders.request("/overflow".into()));
        assert!(folders.get(&paths[0]).is_none());
        assert!(paths[1..].iter().all(|path| folders.get(path).is_some()));
        let (new, _) = host.receive("files_list");
        reply(new, Ok(listing("/overflow", "fresh")));
        until(&mut folders, |folders| !folders.has_pending());
        assert_eq!(folders.cache.len(), CACHED_FOLDERS);
    }

    #[test]
    fn directories_reject_malformed_or_nonchild_paths_without_rewriting_names() {
        for (name, path) in [
            ("..", "/root/.."),
            (".", "/root/."),
            ("", "/root/"),
            ("a/b", "/root/a/b"),
            ("child", "/elsewhere/child"),
            ("child", "/root/sub/../child"),
            ("bad\0name", "/root/bad\0name"),
        ] {
            assert!(
                directories(json!({"path":"/root","entries":[
                    {"name":name,"path":path,"kind":"directory"}
                ]}))
                .is_err(),
                "accepted {name:?} {path:?}"
            );
        }
        let name = "資料's\\folder";
        let (entries, truncated) = directories(listing("/", name)).unwrap();
        assert_eq!(entries[0].name, name);
        assert_eq!(entries[0].path, format!("/{name}"));
        assert!(!truncated);
        assert!(directories(json!({"path":"relative","entries":[]})).is_err());
        assert!(directories(json!({"path":"/","entries":[],"truncated":"yes"})).is_err());
        assert!(
            directories(json!({"path":"/","entries":[
                {"name":"a","path":"/a","kind":"directory"},
                {"name":"a","path":"/a","kind":"directory"}
            ]}))
            .is_err()
        );
    }

    #[test]
    fn tree_cancellation_never_touches_main_listing_or_preview_slots() {
        let host = Host::new();
        let mut folders = host.folders();
        let mut browser = super::super::Browser::new(
            Backend::at_path(host.path.clone()),
            folders.server.clone(),
            folders.connection.clone(),
        );
        browser.navigate("/listing".into());
        let (listing_read, listing_request) = host.receive("files_list");
        let token = browser.open_preview("/listing/file.txt".into()).unwrap();
        let (preview_read, preview_request) = host.receive("files_preview");
        folders.request("/tree".into());
        let (tree_read, tree_request) = host.receive("files_list");
        folders.retire();
        let (cancel, cancellation) = host.receive("files_cancel");
        assert_eq!(
            cancellation.args["operation"],
            tree_request.args["operation"]
        );
        assert_ne!(
            cancellation.args["operation"],
            listing_request.args["operation"]
        );
        assert_ne!(
            cancellation.args["operation"],
            preview_request.args["operation"]
        );
        reply(tree_read, Ok(listing("/tree", "stale")));
        reply(cancel, Ok(Value::Null));
        until(&mut folders, |folders| !folders.has_pending());
        assert!(browser.listing_pending() && browser.preview().unwrap().pending);
        host.quiet();
        reply(listing_read, Ok(listing("/listing", "main")));
        reply(
            preview_read,
            Ok(json!({"kind":"text","mime":"text/plain","content":"preview"})),
        );
        for _ in 0..1000 {
            browser.poll();
            if !browser.has_pending() {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(!browser.has_pending());
        assert_eq!(browser.canonical_path(), Some("/listing"));
        assert_eq!(
            browser.take_preview_data(token).unwrap()["content"],
            "preview"
        );
        assert!(folders.get("/tree").unwrap().directories.is_empty());
    }
}
