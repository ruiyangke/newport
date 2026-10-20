//! Per-server durable journals behind a versioned layout descriptor.
use super::{Journal, RemovalPhase, Store};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};
use uuid::Uuid;

const LIMIT: u64 = 1024 * 1024;
const SERVER_LIMIT: usize = 1000;
const LAYOUT: &str = "server-journals";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Descriptor {
    version: u32,
    storage: String,
}
#[derive(Default)]
struct Index {
    servers: HashMap<Uuid, Arc<ServerStorage>>,
    // None means an ID was found in multiple persisted server journals. Both
    // owners are quarantined; unrelated owners remain readable and writable.
    operations: HashMap<Uuid, Option<Uuid>>,
}
pub(super) struct Storage {
    path: Option<PathBuf>,
    initial_error: Option<String>,
    index: Mutex<Index>,
    // The descriptor is written once, before the first server journal.
    described: AtomicBool,
    describing: Mutex<()>,
}
pub(super) struct ServerStorage {
    writer: Mutex<()>,
    pub(super) published: RwLock<Published>,
    #[cfg(test)]
    pub(super) before_save: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}
pub(super) struct Published {
    pub(super) journal: Result<Arc<Journal>, String>,
    pub(super) pending_removals: Vec<(Uuid, String)>,
}
impl ServerStorage {
    fn new(journal: Result<Journal, String>) -> Self {
        Self {
            writer: Mutex::new(()),
            published: RwLock::new(Published {
                journal: journal.map(Arc::new),
                pending_removals: Vec::new(),
            }),
            #[cfg(test)]
            before_save: Mutex::new(None),
        }
    }
}
impl Default for Store {
    fn default() -> Self {
        Self::loaded(None, HashMap::new(), true, None)
    }
}
impl Store {
    pub fn new(path: PathBuf) -> Self {
        match load(&path) {
            Ok((journals, described)) => Self::loaded(Some(path), journals, described, None),
            Err(error) => Self::loaded(Some(path), HashMap::new(), false, Some(error)),
        }
    }
    fn loaded(
        path: Option<PathBuf>,
        mut journals: Journals,
        described: bool,
        error: Option<String>,
    ) -> Self {
        let mut index = Index::default();
        let mut conflicts = HashSet::new();
        for (server, journal) in &journals {
            if let Ok(journal) = journal {
                for (id, _) in journal.identities() {
                    match index.operations.entry(id) {
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            entry.insert(Some(*server));
                        }
                        std::collections::hash_map::Entry::Occupied(mut entry) => {
                            if let Some(prior) = entry.get() {
                                conflicts.insert(*prior);
                            }
                            conflicts.insert(*server);
                            entry.insert(None);
                        }
                    }
                }
            }
        }
        for server in conflicts {
            journals.insert(
                server,
                Err("Needle operation ID appears in multiple server journals".into()),
            );
        }
        index.servers = journals
            .into_iter()
            .map(|(id, journal)| (id, Arc::new(ServerStorage::new(journal))))
            .collect();
        Self {
            inner: Arc::new(Storage {
                path,
                initial_error: error,
                index: Mutex::new(index),
                described: AtomicBool::new(described),
                describing: Mutex::new(()),
            }),
        }
    }
    fn check(&self) -> Result<(), String> {
        self.inner.initial_error.clone().map_or(Ok(()), Err)
    }
    pub(super) fn server(&self, id: Uuid) -> Result<Arc<ServerStorage>, String> {
        self.check()?;
        if id.is_nil() {
            return Err("Invalid Needle server identity".into());
        }
        let mut index = self.inner.index.lock().unwrap();
        if let Some(server) = index.servers.get(&id) {
            return Ok(server.clone());
        }
        if index.servers.len() >= SERVER_LIMIT {
            return Err("Needle server journal limit reached".into());
        }
        let server = Arc::new(ServerStorage::new(Ok(Journal::default())));
        index.servers.insert(id, server.clone());
        Ok(server)
    }
    pub(super) fn operation_server(&self, id: Uuid) -> Result<Option<Uuid>, String> {
        self.check()?;
        match self.inner.index.lock().unwrap().operations.get(&id) {
            Some(Some(server)) => Ok(Some(*server)),
            Some(None) => Err("Needle operation identity is ambiguous".into()),
            None => Ok(None),
        }
    }
    pub(super) fn read(&self, server: Uuid) -> Result<Arc<Journal>, String> {
        self.server(server)?
            .published
            .read()
            .unwrap()
            .journal
            .clone()
    }
    pub(super) async fn commit<T: Send + 'static>(
        &self,
        server: Uuid,
        edit: impl FnOnce(&mut Journal) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let owner = self.server(server)?;
        let store = self.clone();
        // The owned worker completes both durable and observable publication
        // even when its caller is cancelled. It holds only this server's writer.
        tokio::task::spawn_blocking(move || store.commit_blocking(server, &owner, edit))
            .await
            .map_err(|error| format!("Needle journal worker failed: {error}"))?
    }
    fn commit_blocking<T>(
        &self,
        server: Uuid,
        owner: &ServerStorage,
        edit: impl FnOnce(&mut Journal) -> Result<T, String>,
    ) -> Result<T, String> {
        let _writer = owner
            .writer
            .lock()
            .map_err(|_| "Needle server journal writer is unavailable")?;
        let current = owner.published.read().unwrap().journal.clone()?;
        let mut journal = (*current).clone();
        let result = edit(&mut journal)?;
        journal.validate()?;
        if journal.identities().any(|(_, id)| id != server) {
            return Err("Needle journal contains another server's operation".into());
        }
        if journal == *current {
            return Ok(result);
        }
        {
            let mut index = self.inner.index.lock().unwrap();
            for (id, _) in journal.identities() {
                if index
                    .operations
                    .get(&id)
                    .is_some_and(|owner| *owner != Some(server))
                {
                    return Err("Needle operation ID belongs to another server".into());
                }
            }
            for (id, _) in journal.identities() {
                index.operations.insert(id, Some(server));
            }
        } // No registry lock is held during file I/O or publication.
        owner.published.write().unwrap().pending_removals = journal
            .removals
            .iter()
            .filter(|row| row.phase != RemovalPhase::Restored)
            .map(|row| (row.server_id, row.connection.clone()))
            .collect();
        #[cfg(test)]
        {
            let before_save = owner.before_save.lock().unwrap().take();
            if let Some(before_save) = before_save {
                before_save();
            }
        }
        let saved = self.describe().and_then(|_| self.save(server, &journal));
        let next = saved
            .as_ref()
            .map(|_| Arc::new(journal))
            .map_err(Clone::clone);
        let mut published = owner.published.write().unwrap();
        // An uncertain replacement quarantines only its own server; preserve
        // operation-ID reservations because the rename may already have happened.
        published.journal = next;
        published.pending_removals.clear();
        saved.map(|_| result)
    }
    pub(super) fn journal_path(&self, server: Uuid) -> Option<PathBuf> {
        self.inner.path.as_ref().map(|path| {
            directory(path)
                .join(server.to_string())
                .join("journal.json")
        })
    }
    fn save(&self, server: Uuid, journal: &Journal) -> Result<(), String> {
        let Some(path) = self.journal_path(server) else {
            return Ok(());
        };
        let parent = path.parent().ok_or("Invalid Needle journal path")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        sync_directory(parent.parent().ok_or("Invalid Needle server directory")?)?;
        write_atomic(
            &path,
            &serde_json::to_vec(journal).map_err(|e| e.to_string())?,
        )
    }
    fn describe(&self) -> Result<(), String> {
        if self.inner.described.load(Ordering::Acquire) {
            return Ok(());
        }
        let _describing = self.inner.describing.lock().unwrap();
        if self.inner.described.load(Ordering::Acquire) {
            return Ok(());
        }
        let path = self
            .inner
            .path
            .as_ref()
            .ok_or("Missing Needle journal path")?;
        let parent = path.parent().ok_or("Invalid Needle journal path")?;
        // The server directory exists before the descriptor that names it.
        fs::create_dir_all(directory(path)).map_err(|e| e.to_string())?;
        sync_directory(parent)?;
        write_atomic(
            path,
            &serde_json::to_vec(&Descriptor {
                version: 2,
                storage: LAYOUT.into(),
            })
            .map_err(|e| e.to_string())?,
        )?;
        self.inner.described.store(true, Ordering::Release);
        Ok(())
    }
}

type Journals = HashMap<Uuid, Result<Journal, String>>;
fn directory(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".servers");
    PathBuf::from(name)
}
/// Returns the server journals and whether the layout descriptor exists.
fn load(path: &Path) -> Result<(Journals, bool), String> {
    let Some(bytes) = read_optional(path)? else {
        let servers = directory(path);
        if servers.try_exists().map_err(|error| error.to_string())?
            && !load_servers(&servers)?.is_empty()
        {
            // Never reopen empty admission after the descriptor was lost.
            return Err(
                "Needle journal descriptor is missing; server storage was preserved".into(),
            );
        }
        return Ok((HashMap::new(), false));
    };
    let descriptor = serde_json::from_slice::<Descriptor>(&bytes)
        .map_err(|e| format!("Cannot read Needle recovery journal: {e}"))?;
    if descriptor.version != 2 || descriptor.storage != LAYOUT {
        return Err("Unknown Needle journal layout".into());
    }
    Ok((load_servers(&directory(path))?, true))
}
fn load_servers(path: &Path) -> Result<Journals, String> {
    if !fs::symlink_metadata(path)
        .map_err(|e| e.to_string())?
        .is_dir()
    {
        return Err("Needle journal directory is not a regular directory".into());
    }
    let mut journals = HashMap::new();
    for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "Invalid Needle server journal name")?;
        let server = Uuid::parse_str(&name).map_err(|_| "Invalid Needle server journal name")?;
        if server.is_nil() || server.to_string() != name || journals.len() >= SERVER_LIMIT {
            return Err("Invalid Needle server journal registry".into());
        }
        let journal = (|| {
            if !entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                return Err("Needle server journal is not a directory".into());
            }
            let bytes = read_optional(&entry.path().join("journal.json"))?
                .ok_or("Needle server journal is missing")?;
            let journal: Journal = serde_json::from_slice(&bytes)
                .map_err(|e| format!("Cannot read Needle server journal: {e}"))?;
            journal.validate()?;
            if journal.identities().any(|(_, id)| id != server) {
                return Err("Needle journal contains another server's operation".into());
            }
            Ok(journal)
        })();
        journals.insert(server, journal);
    }
    Ok(journals)
}
fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_file() => return Err("Needle journal is not a regular file".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
        _ => {}
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > LIMIT {
        return Err("Needle recovery journal exceeds limit".into());
    }
    Ok(Some(bytes))
}
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() as u64 > LIMIT {
        return Err("Needle recovery journal exceeds limit".into());
    }
    let parent = path.parent().ok_or("Invalid Needle journal path")?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.to_string())?;
    sync_directory(parent)
}
fn sync_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    fs::File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
