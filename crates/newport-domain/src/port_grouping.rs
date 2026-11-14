//! Connections project identity and listener aggregation.
//!
//! Compatible observations share a row, saved project roots absorb
//! nested discoveries, and saved forwards stay visible even when no listener
//! accounts for them. This module has no dependency on a UI, connection state,
//! or transport.

use crate::{forwarding, port_discovery as discovery};
mod port;
use newport_protocol::desktop::{
    configuration::{Direction, Tunnel},
    preferences::Project as Bookmark,
};
pub use port::Port;
use std::collections::{HashMap, HashSet};

pub const OTHER: &str = "";
pub const SYSTEM: &str = "system-services:";

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// All observed members, or a saved forward without a matching observation.
    pub port: Port,
    /// Every observed listening address, sorted for display.
    pub addresses: Vec<String>,
    pub forwards: Vec<Tunnel>,
}

impl Entry {
    pub fn is_saved(&self) -> bool {
        matches!(self.port, Port::Saved { .. })
    }
}

/// A rendered set of listeners, without retaining their saved forwarding state.
/// Resolve it against freshly grouped entries before planning a batch action.
/// An empty selection stays empty; listeners discovered later are never added.
#[derive(Clone, Debug, Default)]
pub struct ListenerSelection {
    members: discovery::Selection,
}
impl ListenerSelection {
    pub fn capture(entries: &[Entry]) -> Self {
        Self {
            members: discovery::Selection::capture(
                entries
                    .iter()
                    .filter(|entry| !entry.is_saved())
                    .filter_map(|entry| entry.port.row())
                    .flat_map(|row| &row.members),
            ),
        }
    }
    pub fn resolve(&self, latest: Vec<Entry>) -> Vec<Entry> {
        latest
            .into_iter()
            .filter_map(|mut entry| {
                let Port::Observed(row) = &mut entry.port else {
                    return None;
                };
                row.members.retain(|member| self.members.contains(member));
                if row.members.is_empty() {
                    return None;
                }
                entry.forwards.retain(|forward| {
                    row.members.iter().any(|member| {
                        forwarding::reaches_member(forward, forward.server_id, member)
                    })
                });
                entry.addresses = entry.port.addresses();
                Some(entry)
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Saved,
    Compose,
    Vcs,
    Marker,
    System,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    pub path: String,
    pub branch: Option<String>,
    pub main: Option<bool>,
}

/// Last unambiguous checkout observed for a saved forward during this app session.
/// This is display metadata, never a routing or authorization decision. Entries
/// are bounded by the saved forwards and invalidated when their destination or
/// explicitly assigned project changes. Names are deliberately not copied.
#[derive(Default)]
pub struct WorktreeAssociations {
    entries: HashMap<(uuid::Uuid, uuid::Uuid), WorktreeAssociation>,
}

struct WorktreeAssociation {
    signature: Signature,
    project: String,
    worktree: Worktree,
}

/// The parts of a saved forward that decide which checkout it reaches.
type Signature = (Direction, String, u16, Option<u16>, Option<String>);

fn association_signature(tunnel: &Tunnel) -> Signature {
    (
        tunnel.direction,
        tunnel.remote_host.clone(),
        tunnel.remote_port,
        tunnel.remote_port_end,
        tunnel.project.clone(),
    )
}

impl WorktreeAssociations {
    pub fn group(
        &mut self,
        ports: Vec<discovery::Member>,
        saved: &[Tunnel],
        server: uuid::Uuid,
        known: &[Bookmark],
    ) -> Vec<Project> {
        // The supplied saved configuration contains all servers. Prune deleted
        // forwards globally, but never infer an association from another server.
        self.entries.retain(|(server, id), association| {
            saved.iter().any(|tunnel| {
                tunnel.server_id == *server
                    && tunnel.id == *id
                    && association.signature == association_signature(tunnel)
            })
        });
        group_inner(ports, saved, server, known, Some(self))
    }

    fn observe(&mut self, server: uuid::Uuid, projects: &[Project]) {
        let mut observed: HashMap<uuid::Uuid, Option<(&Tunnel, &str, Worktree)>> = HashMap::new();
        let mut live_checkouts = HashMap::new();
        for project in projects {
            for entry in &project.entries {
                if let Some(worktree) = worktree_of(&entry.port) {
                    live_checkouts
                        .entry((project.key.as_str(), worktree.path.clone()))
                        .or_insert(worktree);
                }
                for tunnel in &entry.forwards {
                    let candidate = worktree_of(&entry.port)
                        .map(|worktree| (tunnel, project.key.as_str(), worktree));
                    observed
                        .entry(tunnel.id)
                        .and_modify(|previous| {
                            if previous
                                .as_ref()
                                .map(|(_, project, worktree)| (*project, &worktree.path))
                                != candidate
                                    .as_ref()
                                    .map(|(_, project, worktree)| (*project, &worktree.path))
                            {
                                *previous = None;
                            }
                        })
                        .or_insert(candidate);
                }
            }
        }
        for (id, observed) in observed {
            let key = (server, id);
            if let Some((tunnel, project, worktree)) = observed {
                self.entries.insert(
                    key,
                    WorktreeAssociation {
                        signature: association_signature(tunnel),
                        project: project.to_owned(),
                        worktree,
                    },
                );
            } else {
                // A port range spanning checkouts, or a replacement listener
                // with unknown checkout, must not inherit a guessed identity.
                self.entries.remove(&key);
            }
        }
        // The branch belongs to the checkout, not an individual listener. A
        // live sibling can report a branch switch while this forward's process
        // is stopped. Refresh the remembered value as well as this render, so
        // the old branch cannot return after the final listener disappears.
        for ((entry_server, _), association) in &mut self.entries {
            if *entry_server == server
                && let Some(worktree) = live_checkouts.get(&(
                    association.project.as_str(),
                    association.worktree.path.clone(),
                ))
            {
                association.worktree = worktree.clone();
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub key: String,
    pub name: String,
    pub path: Option<String>,
    pub source: Source,
    pub marker: Option<String>,
    /// Listening services first, followed by unmatched saved forwards.
    pub entries: Vec<Entry>,
    /// Distinct Git checkouts, with the main checkout first.
    pub worktrees: Vec<Worktree>,
}

fn trim(path: &str) -> &str {
    let value = path.trim_end_matches('/');
    if value.is_empty() { "/" } else { value }
}

fn within(path: &str, root: &str) -> bool {
    path == root
        || (root == "/" && path.starts_with('/'))
        || path
            .strip_prefix(root)
            .is_some_and(|tail| tail.starts_with('/'))
}

/// The discovered repository root wins over a nested Compose directory.
pub fn project_key(port: &Port) -> String {
    if let Some(project) = port.project() {
        return trim(&project.root).to_owned();
    }
    if let Some(publication) = port.publication() {
        if let Some(path) = publication
            .directory
            .as_deref()
            .filter(|path| !path.is_empty())
        {
            return trim(path).to_owned();
        }
        if let Some(project) = publication
            .compose
            .as_deref()
            .filter(|name| !name.is_empty())
        {
            return format!("compose:{project}");
        }
    }
    if port.system() {
        SYSTEM.to_owned()
    } else {
        OTHER.to_owned()
    }
}

pub fn worktree_of(port: &Port) -> Option<Worktree> {
    port.worktree()
}

fn known_path(project: &Bookmark) -> Option<&str> {
    Some(project.path.display.as_str())
        .filter(|path| !path.is_empty())
        .map(trim)
}

fn describe(key: &str, port: Option<&Port>, known: &[&Bookmark]) -> Project {
    let marker = port.and_then(|p| {
        p.project()
            .map(|project| project.marker.clone())
            .or_else(|| {
                p.publication()
                    .filter(|publication| publication.compose.is_some())
                    .map(|_| "compose".into())
            })
    });
    let (name, path, source, marker) = if key == OTHER {
        ("Other services".to_owned(), None, Source::Other, None)
    } else if key == SYSTEM {
        ("System services".to_owned(), None, Source::System, None)
    } else if let Some(name) = key.strip_prefix("compose:") {
        (
            name.to_owned(),
            None,
            Source::Compose,
            Some("compose".to_owned()),
        )
    } else if let Some(saved) = known.iter().find(|p| known_path(p) == Some(key)) {
        (
            saved.name.clone(),
            Some(key.to_owned()),
            Source::Saved,
            marker,
        )
    } else {
        let basename = key.rsplit('/').find(|part| !part.is_empty()).unwrap_or(key);
        let short = basename
            .strip_suffix(".git")
            .filter(|name| !name.is_empty())
            .unwrap_or(basename);
        let compose = port
            .and_then(|p| p.publication())
            .and_then(|publication| publication.compose.as_deref())
            .unwrap_or("");
        let name = if marker.as_deref() == Some("compose") && !compose.is_empty() {
            compose
        } else {
            short
        };
        let source = match marker.as_deref() {
            Some("compose") => Source::Compose,
            Some(
                ".git" | ".hg" | ".jj" | ".sl" | ".fslckout" | "_FOSSIL_" | ".bzr" | "_darcs"
                | ".svn" | ".projectile",
            ) => Source::Vcs,
            _ => Source::Marker,
        };
        (name.to_owned(), Some(key.to_owned()), source, marker)
    };
    Project {
        key: key.to_owned(),
        name,
        path,
        source,
        marker,
        entries: Vec::new(),
        worktrees: Vec::new(),
    }
}

/// Group the current server's discoveries and saved forwards.
///
/// Known projects name the roots they hold by their path's display form;
/// another server's projects are ignored. Input order is preserved for
/// equal-rank listeners and equal-name entries (stable sorting); hash maps are
/// only indexes and never determine row order.
pub fn group(
    ports: Vec<discovery::Member>,
    saved: &[Tunnel],
    server: uuid::Uuid,
    known: &[Bookmark],
) -> Vec<Project> {
    group_inner(ports, saved, server, known, None)
}

fn group_inner(
    ports: Vec<discovery::Member>,
    saved: &[Tunnel],
    server: uuid::Uuid,
    known: &[Bookmark],
    mut associations: Option<&mut WorktreeAssociations>,
) -> Vec<Project> {
    let known: Vec<_> = known
        .iter()
        .filter(|p| p.server == server && known_path(p).is_some())
        .collect();
    let mut roots: Vec<_> = known.iter().filter_map(|p| known_path(p)).collect();
    roots.sort_by_key(|path| std::cmp::Reverse(path.len()));
    let fold = |key: &str| -> String {
        if key.starts_with('/') {
            roots
                .iter()
                .find(|root| within(key, root))
                .copied()
                .unwrap_or(key)
                .to_owned()
        } else {
            key.to_owned()
        }
    };
    let mine: Vec<_> = saved.iter().filter(|t| t.server_id == server).collect();
    let listeners = discovery::group(ports)
        .into_iter()
        .flat_map(|group| group.rows);
    let mut projects = Vec::<Project>::new();
    let mut project_indexes = HashMap::new();
    let mut placed = HashSet::new();
    for row in listeners {
        let port = Port::Observed(row);
        let mut forwards = Vec::new();
        let mut seen = HashSet::new();
        for listener in port.row().into_iter().flat_map(|row| &row.members) {
            for &tunnel in &mine {
                if forwarding::reaches_member(tunnel, server, listener) && seen.insert(tunnel.id) {
                    forwards.push(tunnel.clone());
                    placed.insert(tunnel.id);
                }
            }
        }
        let own = fold(&project_key(&port));
        let key = if own == OTHER {
            forwards
                .iter()
                .map(|t| t.project.as_deref().unwrap_or(""))
                .find(|key| !key.is_empty())
                .map(&fold)
                .unwrap_or(own)
        } else {
            own
        };
        let index = *project_indexes.entry(key.clone()).or_insert_with(|| {
            projects.push(describe(&key, Some(&port), &known));
            projects.len() - 1
        });
        projects[index].entries.push(Entry {
            addresses: port.addresses(),
            port,
            forwards,
        });
    }
    if let Some(associations) = associations.as_mut() {
        associations.observe(server, &projects);
    }
    for tunnel in mine {
        if placed.contains(&tunnel.id) {
            continue;
        }
        let association = associations
            .as_ref()
            .and_then(|associations| associations.entries.get(&(server, tunnel.id)));
        let key = fold(
            association
                .map(|a| a.project.as_str())
                .unwrap_or_else(|| tunnel.project.as_deref().unwrap_or("")),
        );
        let port = Port::Saved {
            definition: tunnel.clone(),
            worktree: association.map(|value| value.worktree.clone()),
        };
        let index = *project_indexes.entry(key.clone()).or_insert_with(|| {
            projects.push(describe(&key, None, &known));
            projects.len() - 1
        });
        projects[index].entries.push(Entry {
            port,
            addresses: Vec::new(),
            forwards: vec![tunnel.clone()],
        });
    }
    for project in &mut projects {
        let mut checkouts = HashMap::new();
        for entry in &project.entries {
            if let Some(worktree) = worktree_of(&entry.port) {
                // Live metadata wins over a remembered branch label.
                checkouts.entry(worktree.path.clone()).or_insert(worktree);
            }
        }
        project.worktrees = checkouts.into_values().collect();
        project.worktrees.sort_by(|a, b| {
            b.main
                .cmp(&a.main)
                .then_with(|| {
                    a.branch
                        .as_deref()
                        .unwrap_or("")
                        .cmp(b.branch.as_deref().unwrap_or(""))
                })
                .then_with(|| a.path.cmp(&b.path))
        });
        project.entries.sort_by(|a, b| {
            a.is_saved().cmp(&b.is_saved()).then_with(|| {
                if a.is_saved() {
                    return std::cmp::Ordering::Equal;
                }
                a.port
                    .number()
                    .cmp(&b.port.number())
                    .then_with(|| a.addresses.cmp(&b.addresses))
            })
        });
    }
    projects.sort_by(|a, b| {
        (a.source == Source::Other, a.source == Source::System)
            .cmp(&(b.source == Source::Other, b.source == Source::System))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.key.cmp(&b.key))
    });
    projects
}

#[cfg(test)]
mod tests;
