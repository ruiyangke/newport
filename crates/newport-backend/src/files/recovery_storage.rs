//! Independent upload/download journals per saved server. The only shared
//! mutex is a short owner lookup; loading and all subsequent I/O are scoped.
use super::{cleanup::UploadCleanup, download_cleanup::DownloadCleanup};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};
use uuid::Uuid;

type Owner<Q> = Result<Arc<Mutex<Q>>, String>;
type Slot<Q> = Arc<OnceLock<Owner<Q>>>;

pub(super) trait Queue: Default + Send {
    const KIND: Kind;
    fn open(path: PathBuf) -> Self;
    fn validate(bytes: &[u8], server: Option<Uuid>) -> Result<()>;
}
impl Queue for UploadCleanup {
    const KIND: Kind = Kind::Upload;
    fn open(path: PathBuf) -> Self {
        Self::open(path)
    }
    fn validate(bytes: &[u8], server: Option<Uuid>) -> Result<()> {
        Self::validate(bytes, server)
    }
}
impl Queue for DownloadCleanup {
    const KIND: Kind = Kind::Download;
    fn open(path: PathBuf) -> Self {
        Self::open(path)
    }
    fn validate(bytes: &[u8], _: Option<Uuid>) -> Result<()> {
        Self::validate(bytes)
    }
}

pub(super) struct Owners<Q> {
    directory: Option<PathBuf>,
    error: Option<String>,
    owners: Mutex<HashMap<Uuid, Slot<Q>>>,
}
impl<Q> Default for Owners<Q> {
    fn default() -> Self {
        Self {
            directory: None,
            error: None,
            owners: Mutex::new(HashMap::new()),
        }
    }
}
impl<Q: Queue> Owners<Q> {
    pub fn open(path: PathBuf) -> Self {
        let directory = directory(&path);
        let mut owners = Self {
            directory: Some(directory.clone()),
            ..Self::default()
        };
        let loaded = (|| -> Result<()> {
            migrate::<Q>(&path)?;
            for item in fs::read_dir(&directory)?.collect::<std::io::Result<Vec<_>>>()? {
                let name = item.file_name();
                if name == "source.json" {
                    continue;
                }
                if Q::KIND == Kind::Download && name == "unassigned" {
                    // Old downloads did not record a server. Recover their exact
                    // local paths at startup; no new transfer writes this file.
                    check_directory(&item.path())?;
                    let bytes = read(&item.path().join("journal.json"))?
                        .context("Missing legacy download recovery")?;
                    DownloadCleanup::validate(&bytes)?;
                    drop(DownloadCleanup::open(item.path().join("journal.json")));
                    continue;
                }
                let server = file_server(&name)?;
                // Preload before any live transfer, because download open()
                // recovers abandoned files. A bad shard affects only its owner.
                let _ = owners.get(server);
            }
            Ok(())
        })();
        if let Err(error) = loaded {
            owners.error = Some(format!("Cannot load file recovery storage: {error:#}"));
        }
        owners
    }

    /// Call on a blocking worker. The lookup is memory-only; initialization
    /// waits only for this server and this transfer direction.
    pub fn get(&self, server: Uuid) -> Result<Arc<Mutex<Q>>> {
        if let Some(error) = &self.error {
            bail!("{error}");
        }
        if server.is_nil() {
            bail!("Invalid file recovery server identity");
        }
        let slot = self
            .owners
            .lock()
            .unwrap()
            .entry(server)
            .or_default()
            .clone();
        slot.get_or_init(|| {
            (|| -> Result<_> {
                let queue = if let Some(root) = &self.directory {
                    check_directory(root)?;
                    let server_root = root.join(server.to_string());
                    match fs::create_dir(&server_root) {
                        Ok(()) => sync_directory(root)?,
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                            check_directory(&server_root)?
                        }
                        Err(error) => return Err(error.into()),
                    }
                    let path = server_root.join("journal.json");
                    if let Some(bytes) = read(&path)? {
                        Q::validate(&bytes, Some(server))?;
                    } else if fs::read_dir(&server_root)?.next().is_some() {
                        bail!("Server file recovery journal is missing; evidence was preserved");
                    }
                    Q::open(path)
                } else {
                    Q::default()
                };
                Ok(Arc::new(Mutex::new(queue)))
            })()
            .map_err(|error| format!("Cannot load this server's file recovery: {error:#}"))
        })
        .clone()
        .map_err(anyhow::Error::msg)
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Kind {
    Upload,
    Download,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Descriptor {
    version: u32,
    storage: String,
}
#[derive(Deserialize, Serialize)]
struct Legacy {
    version: u32,
    entries: HashMap<Uuid, Value>,
}
const LAYOUT: &str = "server-journals";

pub(super) fn scope_server(scope: &Value) -> Result<Uuid> {
    let id = Uuid::parse_str(
        scope[0]
            .as_str()
            .context("Missing file recovery server identity")?,
    )?;
    if id.is_nil() {
        bail!("Invalid file recovery server identity");
    }
    Ok(id)
}
pub(super) fn directory(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".servers");
    name.into()
}
fn file_server(name: &std::ffi::OsStr) -> Result<Uuid> {
    let name = name.to_str().context("Invalid file recovery shard name")?;
    let id = Uuid::parse_str(name)?;
    if id.is_nil() || name != id.to_string() {
        bail!("Invalid file recovery shard identity");
    }
    Ok(id)
}
fn read(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() => bail!("File recovery journal is not a regular file"),
        Ok(_) => Ok(Some(fs::read(path)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("Missing file recovery directory")?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    sync_directory(parent)
}
fn check_directory(path: &Path) -> Result<()> {
    if !fs::symlink_metadata(path)?.is_dir() {
        bail!("File recovery storage is not a directory");
    }
    Ok(())
}

/// Runs once at profile startup, before transfers exist. Stage the complete
/// conversion, preserve exact source bytes, then replace the old journal with a
/// descriptor. Old clients reject it rather than opening stale global records.
fn migrate<Q: Queue>(path: &Path) -> Result<()> {
    let source = read(path)?;
    let destination = directory(path);
    if let Some(bytes) = &source
        && let Ok(descriptor) = serde_json::from_slice::<Descriptor>(bytes)
    {
        if descriptor.version != 2 || descriptor.storage != LAYOUT {
            bail!("Unknown file recovery storage layout");
        }
        return check_directory(&destination);
    }
    let mut files = HashMap::<String, Vec<u8>>::new();
    if let Some(bytes) = &source {
        Q::validate(bytes, None)?;
        files.insert("source.json".into(), bytes.clone());
        match Q::KIND {
            Kind::Upload => {
                let legacy: Legacy = serde_json::from_slice(bytes)?;
                let mut shards = HashMap::<Uuid, HashMap<Uuid, Value>>::new();
                for (id, entry) in legacy.entries {
                    shards
                        .entry(scope_server(&entry["scope"])?)
                        .or_default()
                        .insert(id, entry);
                }
                for (server, entries) in shards {
                    files.insert(
                        format!("{server}/journal.json"),
                        serde_json::to_vec(&Legacy {
                            version: 1,
                            entries,
                        })?,
                    );
                }
            }
            Kind::Download => {
                files.insert("unassigned/journal.json".into(), bytes.clone());
            }
        }
    }
    let parent = path.parent().context("Missing file recovery directory")?;
    if destination.try_exists()? {
        check_directory(&destination)?;
        let mut existing = HashMap::new();
        for item in fs::read_dir(&destination)? {
            let item = item?;
            let name = item
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("Invalid file recovery staging name"))?;
            let (name, file) = if name == "source.json" {
                (name, item.path())
            } else {
                if name != "unassigned" {
                    file_server(std::ffi::OsStr::new(&name))?;
                }
                check_directory(&item.path())?;
                let children = fs::read_dir(item.path())?.collect::<std::io::Result<Vec<_>>>()?;
                if children.len() != 1 || children[0].file_name() != "journal.json" {
                    bail!("File recovery migration contains unexpected records");
                }
                (
                    format!("{name}/journal.json"),
                    item.path().join("journal.json"),
                )
            };
            existing.insert(
                name,
                read(&file)?.context("Missing file recovery staging record")?,
            );
        }
        // Serialized map order can differ, but the saved original bytes must
        // match exactly. Reject rollback/deletion of a descriptor after writes.
        if existing.len() != files.len()
            || files.iter().any(|(name, expected)| {
                existing.get(name).is_none_or(|actual| {
                    if name == "source.json" {
                        actual != expected
                    } else {
                        serde_json::from_slice::<Value>(actual).ok()
                            != serde_json::from_slice::<Value>(expected).ok()
                    }
                })
            })
        {
            bail!("File recovery descriptor and server journals disagree; evidence was preserved");
        }
    } else {
        let staging = tempfile::Builder::new()
            .prefix(".files-migrate-")
            .tempdir_in(parent)?;
        for (name, bytes) in files {
            let file = staging.path().join(name);
            fs::create_dir_all(file.parent().unwrap())?;
            write(&file, &bytes)?;
        }
        sync_directory(staging.path())?;
        fs::rename(staging.path(), &destination)?;
        sync_directory(parent)?;
    }
    write(
        path,
        &serde_json::to_vec(&Descriptor {
            version: 2,
            storage: LAYOUT.into(),
        })?,
    )
}

#[cfg(test)]
#[path = "recovery_storage_tests.rs"]
mod tests;
