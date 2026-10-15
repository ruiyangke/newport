//! Durable routing history, scoped to the desktop's per-server client identity.
//! A new publication supersedes the old route before enabling any permissions.
use super::Record;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const MAX_ALIASES: usize = 64;
const MAX_BYTES: u64 = 16 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Head {
    current: Record,
    aliases: Vec<String>,
}

fn directory() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is required"))?;
    crate::paths::private_home_directory(Path::new(&home), ".cache/newport/routes")
}

fn path(directory: &Path, client: &str, extension: &str) -> io::Result<PathBuf> {
    if client.is_empty()
        || client.len() > 64
        || !client
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(io::Error::other("Invalid integration client identity"));
    }
    Ok(directory.join(format!("{client}.{extension}")))
}

fn check(file: &File) -> io::Result<()> {
    let m = file.metadata()?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o077 != 0
        || m.mode() & 0o600 != 0o600
        || m.nlink() != 1
    {
        return Err(io::Error::other(
            "Integration route is not private to this account",
        ));
    }
    Ok(())
}

fn read(directory: &Path, client: &str) -> io::Result<Option<Head>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path(directory, client, "json")?)
    {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    check(&file)?;
    if file.metadata()?.len() > MAX_BYTES {
        return Err(io::Error::other("Integration route exceeds its size limit"));
    }
    let head: Head = serde_json::from_reader(file.take(MAX_BYTES)).map_err(io::Error::other)?;
    if head.current.client != client
        || head.aliases.len() > MAX_ALIASES
        || !head.current.root.is_absolute()
        || [&head.current.session, &head.current.runtime]
            .into_iter()
            .chain(&head.aliases)
            .any(|id| uuid::Uuid::parse_str(id).is_err())
    {
        return Err(io::Error::other("Invalid integration route history"));
    }
    Ok(Some(head))
}

pub(super) fn publish(current: &Record, previous: &[Record]) -> io::Result<()> {
    publish_in(&directory()?, current, previous)
}

fn lock_client(directory: &Path, client: &str) -> io::Result<File> {
    // This per-client lock protects only a short local read/atomic-write. It is
    // never held across SSH, socket I/O, or permission acknowledgement.
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path(directory, client, "lock")?)?;
    check(&lock)?;
    // Several shells may recover together while the preceding atomic write is
    // syncing on a busy disk. Keep a bound without treating normal contention
    // as a disconnected integration.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match fs2::FileExt::try_lock_exclusive(&lock) {
            Ok(()) => break,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(e) => return Err(e),
        }
    }
    Ok(lock)
}

fn publish_in(directory: &Path, current: &Record, previous: &[Record]) -> io::Result<()> {
    let _lock = lock_client(directory, &current.client)?;
    let old = read(directory, &current.client)?;
    let mut aliases = old.as_ref().map(|h| h.aliases.clone()).unwrap_or_default();
    let mut remember = |id: &str| {
        aliases.retain(|a| a != id && a != &current.session);
        if id != current.session {
            aliases.push(id.to_owned());
        }
        if aliases.len() > MAX_ALIASES {
            aliases.remove(0);
        }
    };
    for record in previous.iter().filter(|r| r.client == current.client) {
        remember(&record.session);
    }
    if let Some(old) = old {
        remember(&old.current.session);
    }
    // Publication is the commit point; readers see either complete generation.
    // Keep history after worker exit so an old terminal can find its own client
    // on the next reconnect. History alone never makes an endpoint usable.
    write(
        directory,
        &Head {
            current: current.clone(),
            aliases,
        },
    )
}

fn write(directory: &Path, head: &Head) -> io::Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(&mut file, head)?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(path(directory, &head.current.client, "json")?)
        .map_err(|e| e.error)?;
    Ok(())
}

/// Unknown pre-history selectors may be rebound only in a single-client
/// account. Count offline clients too; one live endpoint is not enough proof.
pub(super) fn recover_unknown(
    records: &[Record],
    selector: &str,
    capability: Option<&str>,
) -> io::Result<Option<Record>> {
    let directory = directory()?;
    let Some(candidate) = sole_unknown_route(&directory, records, selector)? else {
        return Ok(None);
    };
    if !super::active(&candidate.root) || !super::enabled(&candidate.root, capability) {
        return Ok(None);
    }
    let _lock = lock_client(&directory, &candidate.client)?;
    let fresh = super::records()?;
    let resolved = reconcile_in(&directory, fresh.clone(), Some(selector))?;
    if resolved.len() == 1
        && resolved[0].client == candidate.client
        && super::active(&resolved[0].root)
        && super::enabled(&resolved[0].root, capability)
    {
        return Ok(resolved.into_iter().next());
    }
    // Recheck under the client lock: never overwrite a reconnect or another
    // recovery's aliases, and never revive a superseded generation.
    let Some(current) = sole_unknown_route(&directory, &fresh, selector)? else {
        return Ok(None);
    };
    if current.runtime != candidate.runtime
        || current.root != candidate.root
        || current.session != candidate.session
        || !super::active(&current.root)
        || !super::enabled(&current.root, capability)
    {
        return Ok(None);
    }
    let Some(mut head) = read(&directory, &current.client)? else {
        return Ok(None);
    };
    head.aliases.push(selector.to_owned());
    if head.aliases.len() > MAX_ALIASES {
        head.aliases.remove(0);
    }
    write(&directory, &head)?;
    Ok(Some(current))
}

fn sole_unknown_route(
    directory: &Path,
    records: &[Record],
    selector: &str,
) -> io::Result<Option<Record>> {
    let mut clients: HashSet<_> = records.iter().map(|r| r.client.clone()).collect();
    if clients.len() > 1 {
        return Ok(None);
    }
    if records.iter().any(|r| r.session == selector) {
        return Ok(None);
    }
    let mut heads = Vec::new();
    for (index, entry) in fs::read_dir(directory)?.enumerate() {
        if index >= 4096 {
            return Err(io::Error::other("Integration route history needs cleanup"));
        }
        let path = entry?.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let client = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| io::Error::other("Invalid integration route filename"))?;
        // Corrupt/unsafe history blocks guessing; it does not count as absence.
        let Some(head) = read(directory, client)? else {
            continue;
        };
        if head.current.session == selector || head.aliases.iter().any(|s| s == selector) {
            return Ok(None);
        }
        clients.insert(head.current.client.clone());
        if clients.len() > 1 {
            return Ok(None);
        }
        heads.push(head);
    }
    if clients.len() != 1 || heads.len() != 1 {
        return Ok(None);
    }
    let head = heads.pop().unwrap();
    Ok(records
        .iter()
        .find(|r| {
            r.client == head.current.client
                && r.session == head.current.session
                && r.runtime == head.current.runtime
                && r.root == head.current.root
        })
        .cloned())
}

pub(super) fn reconcile(records: Vec<Record>, selector: Option<&str>) -> io::Result<Vec<Record>> {
    reconcile_in(&directory()?, records, selector)
}

pub(super) fn is_current(record: &Record) -> io::Result<bool> {
    Ok(read(&directory()?, &record.client)?.is_some_and(|head| {
        head.current.runtime == record.runtime
            && head.current.root == record.root
            && head.current.session == record.session
    }))
}

fn reconcile_in(
    directory: &Path,
    records: Vec<Record>,
    selector: Option<&str>,
) -> io::Result<Vec<Record>> {
    let mut heads = HashMap::new();
    let mut result = Vec::new();
    for record in records {
        if !heads.contains_key(&record.client) {
            let head = match read(directory, &record.client) {
                Ok(head) => head,
                // An unrelated damaged client must not block an explicit route.
                // Its aliases cannot be trusted, so never guess through it.
                Err(_) if selector.is_some_and(|s| s != record.session) => continue,
                Err(error) => return Err(error),
            };
            heads.insert(record.client.clone(), head);
        }
        let selected = if let Some(head) = &heads[&record.client] {
            // Filter superseded generations BEFORE checking capabilities: a
            // disabled replacement must never resurrect its enabled predecessor.
            head.current.runtime == record.runtime
                && head.current.root == record.root
                && head.current.session == record.session
                && selector
                    .is_none_or(|s| s == record.session || head.aliases.iter().any(|a| a == s))
        } else {
            selector.is_none_or(|s| s == record.session)
        };
        if selected {
            result.push(record);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record(client: &str) -> Record {
        Record {
            client: client.into(),
            session: uuid::Uuid::new_v4().to_string(),
            runtime: uuid::Uuid::new_v4().to_string(),
            root: PathBuf::from("/private-fixture/endpoint"),
        }
    }

    #[test]
    fn history_reconciles_only_its_client_and_never_revives_a_superseded_route() {
        let dir = tempfile::tempdir().unwrap();
        let old = record("desktop-one");
        let other = record("desktop-two");
        let current = record(&old.client);
        publish_in(dir.path(), &current, &[old.clone(), other.clone()]).unwrap();
        let records = vec![old.clone(), other.clone(), current.clone()];
        let result = reconcile_in(dir.path(), records, Some(&old.session)).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].runtime, current.runtime);
        // Even if the replacement dies, the old enabled endpoint must not return.
        assert!(reconcile_in(
            dir.path(),
            vec![old.clone(), other.clone()],
            Some(&old.session)
        )
        .unwrap()
        .is_empty());
        let result = reconcile_in(dir.path(), vec![other.clone()], Some(&other.session)).unwrap();
        assert_eq!(result.len(), 1);
        // A later reconnect preserves the alias without any old live publications.
        let next = record(&old.client);
        publish_in(dir.path(), &next, &[]).unwrap();
        let result = reconcile_in(dir.path(), vec![next.clone()], Some(&old.session)).unwrap();
        assert_eq!(result[0].session, next.session);
    }

    #[test]
    fn concurrent_publications_preserve_history_and_keep_it_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let records: Vec<_> = (0..16).map(|_| record("same-client")).collect();
        std::thread::scope(|scope| {
            for record in &records {
                let path = dir.path();
                scope.spawn(move || publish_in(path, record, &[]).unwrap());
            }
        });
        let head = read(dir.path(), "same-client").unwrap().unwrap();
        assert_eq!(head.aliases.len(), records.len() - 1);
        for record in records {
            assert!(
                record.session == head.current.session || head.aliases.contains(&record.session)
            );
        }
        for _ in 0..MAX_ALIASES + 2 {
            publish_in(dir.path(), &record("same-client"), &[]).unwrap();
        }
        assert_eq!(
            read(dir.path(), "same-client")
                .unwrap()
                .unwrap()
                .aliases
                .len(),
            MAX_ALIASES
        );
    }

    #[test]
    fn unknown_recovery_requires_one_known_client_including_offline_history() {
        let dir = tempfile::tempdir().unwrap();
        let current = record("first");
        let unknown = uuid::Uuid::new_v4().to_string();
        assert!(
            sole_unknown_route(dir.path(), std::slice::from_ref(&current), &unknown)
                .unwrap()
                .is_none()
        );
        publish_in(dir.path(), &current, &[]).unwrap();
        assert_eq!(
            sole_unknown_route(dir.path(), std::slice::from_ref(&current), &unknown)
                .unwrap()
                .unwrap()
                .client,
            current.client
        );
        assert!(
            sole_unknown_route(dir.path(), std::slice::from_ref(&current), &current.session)
                .unwrap()
                .is_none()
        );
        let other = record("offline-desktop");
        publish_in(dir.path(), &other, &[]).unwrap();
        assert!(
            sole_unknown_route(dir.path(), std::slice::from_ref(&current), &unknown)
                .unwrap()
                .is_none()
        );
        assert!(sole_unknown_route(dir.path(), &[current], &other.session)
            .unwrap()
            .is_none());
    }

    #[test]
    fn unsafe_or_corrupt_history_is_preserved_and_rejected() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let current = record("client");
        let target = path(dir.path(), "client", "json").unwrap();
        std::fs::write(&target, b"broken history").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(publish_in(dir.path(), &current, &[]).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"broken history");
        assert!(sole_unknown_route(
            dir.path(),
            std::slice::from_ref(&current),
            &uuid::Uuid::new_v4().to_string()
        )
        .is_err());
        let other = record("unrelated-client");
        let routed = reconcile_in(
            dir.path(),
            vec![current.clone(), other.clone()],
            Some(&other.session),
        )
        .unwrap();
        assert_eq!(routed.len(), 1);
        assert_eq!(routed[0].client, other.client);
        std::fs::remove_file(&target).unwrap();
        publish_in(dir.path(), &current, &[]).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o400)).unwrap();
        assert!(publish_in(dir.path(), &record("client"), &[]).is_err());
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o400
        );
        std::fs::remove_file(&target).unwrap();
        let outside = dir.path().join("outside");
        std::fs::write(&outside, b"untouched").unwrap();
        std::os::unix::fs::symlink(&outside, &target).unwrap();
        assert!(publish_in(dir.path(), &current, &[]).is_err());
        assert_eq!(std::fs::read(outside).unwrap(), b"untouched");
    }
}
