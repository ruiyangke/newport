mod directory;
mod mount;

use super::{Budget, Failure, budget::record};
use directory::Directory;
use mount::Mount;
use newport_protocol::monitor::ports::discovery::{Namespace, Report, Scope, valid_cgroup_path};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::Path,
    time::{Duration, Instant},
};

const DIRECTORY_LIMIT: usize = 8192;
const ENTRY_LIMIT: usize = 131072;
const PATH_BYTES: usize = 1 << 20;
const POSITIVE_TTL: Duration = Duration::from_secs(30);
const NEGATIVE_TTL: Duration = Duration::from_secs(5);

type CacheScope = (Scope, Namespace, Mount, u64);

#[derive(Default)]
pub(super) struct Cache {
    scope: Option<CacheScope>,
    paths: BTreeMap<u64, (String, Instant)>,
    misses: BTreeMap<u64, Instant>,
}

pub(super) struct Resolution {
    pub paths: BTreeMap<u64, String>,
    pub report: Report,
    pub visited: usize,
}

impl Cache {
    pub fn invalidate(&mut self) {
        *self = Self::default();
    }

    fn set_scope(&mut self, key: Option<CacheScope>) {
        if self.scope != key || key.is_none() {
            self.invalidate();
            self.scope = key;
        }
    }

    pub fn resolve(
        &mut self,
        root: &Path,
        scope: &Scope,
        wanted: &BTreeSet<u64>,
        hints: &[String],
        budget: &Budget<'_>,
    ) -> Resolution {
        let mut result = Resolution {
            paths: BTreeMap::new(),
            report: Report::complete(),
            visited: 0,
        };
        if wanted.is_empty() {
            result.report = Report::not_applicable();
            return result;
        }
        if let Err(error) = self.resolve_inner(root, scope, wanted, hints, budget, &mut result) {
            record(&mut result.report, &error);
        }
        if result.paths.len() != wanted.len() {
            record(
                &mut result.report,
                &Failure::Unavailable(
                    "Some socket cgroups could not be resolved in the visible hierarchy",
                ),
            );
        }
        result
    }

    fn resolve_inner(
        &mut self,
        root: &Path,
        scope: &Scope,
        wanted: &BTreeSet<u64>,
        hints: &[String],
        budget: &Budget<'_>,
        result: &mut Resolution,
    ) -> Result<(), Failure> {
        budget.remaining()?;
        let mount =
            mount::discover(root)?.ok_or(Failure::Unsupported("No visible cgroup-v2 mount"))?;
        let directory = Directory::open(
            &root.join(
                mount
                    .path
                    .strip_prefix("/")
                    .map_err(|_| Failure::Invalid("Invalid cgroup mount path"))?,
            ),
        )?;
        if directory.mount != mount.id {
            return Err(Failure::Interrupted);
        }
        let namespace = fs::metadata(root.join("proc/self/ns/cgroup"))
            .ok()
            .map(|m| Namespace {
                device: m.dev(),
                inode: m.ino(),
            });
        let key = namespace
            .filter(|_| {
                scope.network.is_some() && scope.processes.is_some() && scope.boot_id.is_some()
            })
            .map(|namespace| (scope.clone(), namespace, mount.clone(), directory.identity));
        self.set_scope(key);
        let now = Instant::now();
        self.paths.retain(|_, (_, expires)| *expires > now);
        self.misses.retain(|_, expires| *expires > now);
        let mut remaining = wanted.clone();
        for id in wanted {
            budget.remaining()?;
            if let Some((path, _)) = self.paths.get(id) {
                if open_path(&directory, &mount, path).is_ok_and(|found| found.identity == *id) {
                    result.paths.insert(*id, path.clone());
                    remaining.remove(id);
                } else {
                    self.paths.remove(id);
                }
            }
        }
        for path in hints.iter().take(DIRECTORY_LIMIT) {
            if remaining.is_empty() {
                break;
            }
            budget.remaining()?;
            if let Ok(found) = open_path(&directory, &mount, path)
                && remaining.remove(&found.identity)
            {
                result.paths.insert(found.identity, path.clone());
            }
        }
        remaining.retain(|id| !self.misses.contains_key(id));
        let mut candidate = BTreeMap::new();
        let walk_result = walk(
            &directory,
            &mount,
            &mut remaining,
            budget,
            &mut candidate,
            &mut result.visited,
        );
        for (id, path) in candidate {
            budget.remaining()?;
            // Names can change during a walk. Reopen the path before publishing its identity.
            if open_path(&directory, &mount, &path).is_ok_and(|found| found.identity == id) {
                result.paths.insert(id, path);
            } else {
                record(&mut result.report, &Failure::Interrupted);
            }
        }
        if !unchanged(root, &mount, &directory, namespace) {
            result.paths.clear();
            self.invalidate();
            return Err(Failure::Interrupted);
        }
        if self.scope.is_some() {
            for (id, path) in &result.paths {
                self.paths.insert(*id, (path.clone(), now + POSITIVE_TTL));
            }
            if walk_result.is_ok() {
                for id in remaining {
                    self.misses.insert(id, now + NEGATIVE_TTL);
                }
            }
            while self.paths.len() > DIRECTORY_LIMIT {
                self.paths.pop_first();
            }
            while self.misses.len() > DIRECTORY_LIMIT {
                self.misses.pop_first();
            }
        }
        walk_result
    }
}

fn unchanged(
    root: &Path,
    mount: &Mount,
    directory: &Directory,
    namespace: Option<Namespace>,
) -> bool {
    let current_namespace = fs::metadata(root.join("proc/self/ns/cgroup"))
        .ok()
        .map(|m| Namespace {
            device: m.dev(),
            inode: m.ino(),
        });
    if current_namespace != namespace
        || !mount::discover(root).is_ok_and(|value| value.as_ref() == Some(mount))
    {
        return false;
    }
    let Ok(relative) = mount.path.strip_prefix("/") else {
        return false;
    };
    Directory::open(&root.join(relative)).is_ok_and(|current| {
        current.mount == directory.mount && current.identity == directory.identity
    })
}

fn open_path(root: &Directory, mount: &Mount, path: &str) -> Result<Directory, Failure> {
    if !valid_cgroup_path(path) {
        return Err(Failure::Invalid("Invalid cgroup path"));
    }
    let relative = Path::new(path)
        .strip_prefix(&mount.root)
        .map_err(|_| Failure::Invalid("Cgroup lies outside the visible mount"))?;
    let mut current = None;
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(Failure::Invalid("Invalid cgroup path component"));
        };
        current = Some(current.as_ref().unwrap_or(root).child(name)?);
    }
    match current {
        Some(directory) => Ok(directory),
        None => Ok(Directory {
            file: root.file.try_clone()?,
            identity: root.identity,
            mount: root.mount,
        }),
    }
}

fn walk(
    root: &Directory,
    mount: &Mount,
    wanted: &mut BTreeSet<u64>,
    budget: &Budget<'_>,
    found: &mut BTreeMap<u64, String>,
    visited: &mut usize,
) -> Result<(), Failure> {
    let mut pending = VecDeque::from([mount.root.clone()]);
    let mut path_bytes = mount.root.len();
    let mut entries = 0usize;
    let mut failure = None;
    while let Some(path) = pending.pop_front() {
        if wanted.is_empty() {
            break;
        }
        budget.remaining()?;
        if *visited >= DIRECTORY_LIMIT {
            return Err(Failure::Exhausted("Cgroup directory limit reached"));
        }
        path_bytes = path_bytes.saturating_sub(path.len());
        let directory = match open_path(root, mount, &path) {
            Ok(directory) => directory,
            Err(error) => {
                failure.get_or_insert(error);
                continue;
            }
        };
        *visited += 1;
        if wanted.remove(&directory.identity) {
            found.insert(directory.identity, path.clone());
        }
        if wanted.is_empty() {
            break;
        }
        let children = match fs::read_dir(format!("/proc/self/fd/{}", directory.file.as_raw_fd())) {
            Ok(children) => children,
            Err(error) => {
                failure.get_or_insert(error.into());
                continue;
            }
        };
        for entry in children {
            budget.remaining()?;
            entries += 1;
            if entries > ENTRY_LIMIT || pending.len() >= DIRECTORY_LIMIT {
                return Err(Failure::Exhausted("Cgroup traversal limit reached"));
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    failure.get_or_insert(error.into());
                    continue;
                }
            };
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => {}
                Ok(_) => continue,
                Err(error) => {
                    failure.get_or_insert(error.into());
                    continue;
                }
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                failure.get_or_insert(Failure::Invalid("Cgroup name is not UTF-8"));
                continue;
            };
            let path = format!("{}/{name}", path.trim_end_matches('/'));
            if !valid_cgroup_path(&path) {
                failure.get_or_insert(Failure::Invalid("Invalid cgroup path"));
                continue;
            }
            if path.len() > PATH_BYTES.saturating_sub(path_bytes) {
                return Err(Failure::Exhausted("Cgroup path memory limit reached"));
            }
            path_bytes += path.len();
            pending.push_back(path);
        }
    }
    match failure {
        Some(error) if !wanted.is_empty() => Err(error),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolving_no_socket_cgroups_does_not_query_a_mount() {
        let root = tempfile::tempdir().unwrap();
        let result = Cache::default().resolve(
            root.path(),
            &Scope::default(),
            &BTreeSet::new(),
            &[],
            &Budget::new(Instant::now() + Duration::from_secs(1), &|| false),
        );
        assert_eq!(result.report, Report::not_applicable());
        assert_eq!(result.visited, 0);
    }
    #[test]
    fn ordinary_directories_cannot_supply_cgroup_identities() {
        let root = tempfile::tempdir().unwrap();
        assert!(matches!(
            Directory::open(root.path()),
            Err(Failure::Invalid(
                "Discovered mount is not a cgroup-v2 filesystem"
            ))
        ));
    }
    #[test]
    fn cache_invalidation_clears_positive_and_negative_identity_state() {
        let mut cache = Cache::default();
        cache.paths.insert(
            1,
            (
                "/system.slice/ssh.socket".into(),
                Instant::now() + POSITIVE_TTL,
            ),
        );
        cache.misses.insert(2, Instant::now() + NEGATIVE_TTL);
        cache.invalidate();
        assert!(cache.paths.is_empty());
        assert!(cache.misses.is_empty());
        assert!(cache.scope.is_none());
    }
    #[test]
    fn live_cgroup_root_is_resolved_by_handle_without_elevation() {
        let Some(mount) = mount::discover(Path::new("/")).unwrap() else {
            return;
        };
        let directory = Directory::open(&mount.path).unwrap();
        let result = Cache::default().resolve(
            Path::new("/"),
            &Scope::default(),
            &BTreeSet::from([directory.identity]),
            &[],
            &Budget::new(Instant::now() + Duration::from_secs(1), &|| false),
        );
        assert_eq!(result.paths.get(&directory.identity), Some(&mount.root));
        assert_eq!(result.report, Report::complete());
    }
}
