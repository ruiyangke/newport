//! Per-server durable journals with a one-time, recoverable legacy migration.
use super::{Journal, Loaded, RemovalPhase, Store};
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
struct Legacy {
    journal: Journal,
    bytes: Option<Vec<u8>>,
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
    migrated: AtomicBool,
    migration: Mutex<Option<Legacy>>,
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
        Self::loaded(None, HashMap::new(), None, None)
    }
}
impl Store {
    pub fn new(path: PathBuf) -> Self {
        match load(&path) {
            Ok((journals, legacy)) => Self::loaded(Some(path), journals, legacy, None),
            Err(error) => Self::loaded(Some(path), HashMap::new(), None, Some(error)),
        }
    }
    fn loaded(
        path: Option<PathBuf>,
        mut journals: Journals,
        legacy: Option<Legacy>,
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
                migrated: AtomicBool::new(legacy.is_none()),
                migration: Mutex::new(legacy),
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
        } // No registry lock is held during migration, file I/O or publication.
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
        let saved = self.migrate().and_then(|_| self.save(server, &journal));
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
    fn migrate(&self) -> Result<(), String> {
        if self.inner.migrated.load(Ordering::Acquire) {
            return Ok(());
        }
        // One-time format conversion only. Once published, every server writes
        // independently and this gate is never visited again.
        let mut migration = self.inner.migration.lock().unwrap();
        if self.inner.migrated.load(Ordering::Acquire) {
            return Ok(());
        }
        let legacy = migration
            .as_ref()
            .ok_or("Missing Needle migration source")?;
        let path = self
            .inner
            .path
            .as_ref()
            .ok_or("Missing Needle journal path")?;
        let parent = path.parent().ok_or("Invalid Needle journal path")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let destination = directory(path);
        let journals = partition(&legacy.journal);
        if destination.try_exists().map_err(|e| e.to_string())? {
            // A crash may leave the fully staged tree before the descriptor is
            // committed. Reuse it only when it exactly matches the legacy input.
            let existing = load_servers(&destination)?;
            if existing != journals
                || read_optional(&destination.join("legacy.json"))? != legacy.bytes
            {
                return Err(
                    "Needle migration directory does not match its preserved source".into(),
                );
            }
        } else {
            let staging = tempfile::Builder::new()
                .prefix(".needle-migrate-")
                .tempdir_in(parent)
                .map_err(|e| e.to_string())?;
            if let Some(bytes) = &legacy.bytes {
                write_atomic(&staging.path().join("legacy.json"), bytes)?;
            }
            for (server, journal) in &journals {
                let root = staging.path().join(server.to_string());
                fs::create_dir(&root).map_err(|e| e.to_string())?;
                write_atomic(
                    &root.join("journal.json"),
                    &serde_json::to_vec(journal.as_ref().map_err(Clone::clone)?)
                        .map_err(|e| e.to_string())?,
                )?;
            }
            sync_directory(staging.path())?;
            fs::rename(staging.path(), &destination).map_err(|e| e.to_string())?;
            sync_directory(parent)?;
        }
        // Older clients reject this descriptor rather than reopening an obsolete
        // global journal. Its source bytes remain inside the committed tree.
        write_atomic(
            path,
            &serde_json::to_vec(&Descriptor {
                version: 2,
                storage: LAYOUT.into(),
            })
            .map_err(|e| e.to_string())?,
        )?;
        self.inner.migrated.store(true, Ordering::Release);
        *migration = None;
        Ok(())
    }
}

type Journals = HashMap<Uuid, Result<Journal, String>>;
fn partition(journal: &Journal) -> Journals {
    journal
        .identities()
        .map(|(_, server)| server)
        .collect::<HashSet<_>>()
        .into_iter()
        .map(|server| (server, Ok(journal.for_owner(server))))
        .collect()
}
fn directory(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".servers");
    PathBuf::from(name)
}
fn load(path: &Path) -> Result<(Journals, Option<Legacy>), String> {
    let bytes = read_optional(path)?;
    if let Some(bytes) = &bytes
        && let Ok(descriptor) = serde_json::from_slice::<Descriptor>(bytes)
    {
        if descriptor.version != 2 || descriptor.storage != LAYOUT {
            return Err("Unknown Needle journal layout".into());
        }
        return Ok((load_servers(&directory(path))?, None));
    }
    let journal = match &bytes {
        None => Journal::default(),
        Some(bytes) => match serde_json::from_slice::<Loaded>(bytes)
            .map_err(|e| format!("Cannot read Needle recovery journal: {e}"))?
        {
            Loaded::Versioned(journal) => journal,
            Loaded::Legacy(operations) => Journal {
                operations,
                ..Default::default()
            },
        },
    };
    journal.validate()?;
    let journals = partition(&journal);
    let staged = directory(path);
    if staged.try_exists().map_err(|error| error.to_string())?
        && (load_servers(&staged)? != journals
            || read_optional(&staged.join("legacy.json"))? != bytes)
    {
        // Never reopen stale/empty admission after a descriptor was lost or
        // replaced with an older global file. Only an exact pre-commit staging
        // tree is a recoverable interrupted migration.
        return Err("Needle journal descriptor and server storage disagree; recovery evidence was preserved".into());
    }
    Ok((journals, Some(Legacy { journal, bytes })))
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
        if entry.file_name() == "legacy.json" {
            continue;
        }
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
