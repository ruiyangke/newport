//! Connections project identity and listener aggregation.
//!
//! One owner's dual-stack listeners form one service, saved project roots absorb
//! nested discoveries, and saved forwards stay visible even when no listener
//! accounts for them. This module has no dependency on a UI, connection state,
//! or transport.

use crate::forwarding;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub const OTHER: &str = "";

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// The preferred listener, or the saved tunnel when `saved` is true.
    pub port: Value,
    /// Every listening address, in destination preference order.
    pub addresses: Vec<String>,
    pub forwards: Vec<Value>,
    pub saved: bool,
}

/// A rendered set of listeners, without retaining their saved forwarding state.
/// Resolve it against freshly grouped entries before planning a batch action.
/// An empty selection stays empty; listeners discovered later are never added.
#[derive(Clone, Debug, Default)]
pub struct ListenerSelection {
    identities: HashSet<ListenerIdentity>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ListenerIdentity {
    port: u64,
    owner: String,
    address: String,
    worktree: Option<String>,
}

impl ListenerIdentity {
    fn of(entry: &Entry) -> Option<Self> {
        (!entry.saved).then(|| Self {
            port: entry.port["port"].as_u64().unwrap_or(0),
            owner: owner(&entry.port),
            address: text(&entry.port, "address").to_owned(),
            worktree: worktree_of(&entry.port).map(|worktree| worktree.path),
        })
    }
}

impl ListenerSelection {
    pub fn capture(entries: &[Entry]) -> Self {
        Self {
            identities: entries.iter().filter_map(ListenerIdentity::of).collect(),
        }
    }

    /// Keep current data for the same listeners, in current display order.
    /// Owner, destination, or checkout changes invalidate a captured listener;
    /// descriptive metadata and saved forward changes do not.
    pub fn resolve(&self, latest: Vec<Entry>) -> Vec<Entry> {
        latest
            .into_iter()
            .filter(|entry| {
                ListenerIdentity::of(entry)
                    .is_some_and(|identity| self.identities.contains(&identity))
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
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    pub path: String,
    pub branch: Option<String>,
    pub main: bool,
}

/// Last unambiguous checkout observed for a saved forward during this app session.
/// This is display metadata, never a routing or authorization decision. Entries
/// are bounded by the saved forwards and invalidated when their destination or
/// explicitly assigned project changes. Names are deliberately not copied.
#[derive(Default)]
pub struct WorktreeAssociations {
    entries: HashMap<(String, String), WorktreeAssociation>,
}

struct WorktreeAssociation {
    signature: Value,
    project: String,
    worktree: Worktree,
}

fn association_signature(tunnel: &Value) -> Value {
    serde_json::json!([
        tunnel["direction"],
        tunnel["remoteHost"],
        tunnel["remotePort"],
        tunnel["remotePortEnd"],
        tunnel["project"]
    ])
}

impl WorktreeAssociations {
    pub fn group(
        &mut self,
        ports: Vec<Value>,
        saved: &[Value],
        server: &str,
        known: &[Value],
    ) -> Vec<Project> {
        // The supplied saved configuration contains all servers. Prune deleted
        // forwards globally, but never infer an association from another server.
        self.entries.retain(|(server, id), association| {
            saved.iter().any(|tunnel| {
                text(tunnel, "serverId") == server
                    && text(tunnel, "id") == id
                    && association.signature == association_signature(tunnel)
            })
        });
        group_inner(ports, saved, server, known, Some(self))
    }

    fn observe(&mut self, server: &str, projects: &[Project]) {
        let mut observed: HashMap<&str, Option<(&Value, &str, Worktree)>> = HashMap::new();
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
                        .entry(text(tunnel, "id"))
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
            let key = (server.to_owned(), id.to_owned());
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
            if entry_server == server {
                if let Some(worktree) = live_checkouts.get(&(
                    association.project.as_str(),
                    association.worktree.path.clone(),
                )) {
                    association.worktree = worktree.clone();
                }
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

fn text<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field].as_str().unwrap_or("")
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
pub fn project_key(port: &Value) -> String {
    for field in ["projectRoot", "composeWorkingDir"] {
        let path = text(port, field);
        if !path.is_empty() {
            return trim(path).to_owned();
        }
    }
    let compose = text(port, "composeProject");
    if compose.is_empty() {
        OTHER.to_owned()
    } else {
        format!("compose:{compose}")
    }
}

pub fn worktree_of(port: &Value) -> Option<Worktree> {
    let path = text(port, "worktreePath");
    (!path.is_empty()).then(|| Worktree {
        path: trim(path).to_owned(),
        branch: port["worktreeBranch"].as_str().map(str::to_owned),
        main: port["worktreeMain"] != false,
    })
}

fn known_path(project: &Value) -> Option<&str> {
    project["path"]
        .as_str()
        .or_else(|| project["path"]["display"].as_str())
        .filter(|path| !path.is_empty())
        .map(trim)
}

fn describe(key: &str, port: Option<&Value>, known: &[&Value]) -> Project {
    let marker = port
        .and_then(|p| p["projectMarker"].as_str())
        .map(str::to_owned);
    let (name, path, source, marker) = if key == OTHER {
        ("Other services".to_owned(), None, Source::Other, None)
    } else if let Some(name) = key.strip_prefix("compose:") {
        (
            name.to_owned(),
            None,
            Source::Compose,
            Some("compose".to_owned()),
        )
    } else if let Some(saved) = known.iter().find(|p| known_path(p) == Some(key)) {
        (
            text(saved, "name").to_owned(),
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
        let compose = port.map(|p| text(p, "composeProject")).unwrap_or("");
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

fn rank(port: &Value) -> u8 {
    match forwarding::host(text(port, "address")).as_str() {
        "127.0.0.1" => 0,
        "::1" => 1,
        _ => 2,
    }
}

fn owner(port: &Value) -> String {
    ["pid", "containerName", "processName"]
        .iter()
        .find_map(|field| match &port[field] {
            Value::Null => None,
            Value::String(value) => Some(value.clone()),
            value => Some(value.to_string()),
        })
        .unwrap_or_default()
}

/// Group the current server's discoveries and saved forwards.
///
/// Known projects may use the UI's string path or the protocol's `path.display`
/// form. Explicitly foreign `serverId` records are ignored. Input order is
/// preserved for equal-rank listeners and equal-name entries (stable sorting);
/// hash maps are only indexes and never determine row order.
pub fn group(ports: Vec<Value>, saved: &[Value], server: &str, known: &[Value]) -> Vec<Project> {
    group_inner(ports, saved, server, known, None)
}

fn group_inner(
    ports: Vec<Value>,
    saved: &[Value],
    server: &str,
    known: &[Value],
    mut associations: Option<&mut WorktreeAssociations>,
) -> Vec<Project> {
    let known: Vec<_> = known
        .iter()
        .filter(|p| p["serverId"].as_str().is_none_or(|id| id == server) && known_path(p).is_some())
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
    let mine: Vec<_> = saved.iter().filter(|t| t["serverId"] == server).collect();
    let mut listener_indexes = HashMap::new();
    let mut listeners: Vec<Vec<Value>> = Vec::new();
    for port in ports {
        let identity = (port["port"].as_u64().unwrap_or(0), owner(&port));
        let index = *listener_indexes.entry(identity).or_insert_with(|| {
            listeners.push(Vec::new());
            listeners.len() - 1
        });
        listeners[index].push(port);
    }
    let mut projects = Vec::<Project>::new();
    let mut project_indexes = HashMap::new();
    let mut placed = HashSet::new();
    for mut addresses in listeners {
        addresses.sort_by_key(rank);
        let mut forwards = Vec::new();
        let mut seen = HashSet::new();
        for listener in &addresses {
            for &tunnel in &mine {
                if forwarding::reaches(tunnel, server, listener) && seen.insert(text(tunnel, "id"))
                {
                    forwards.push(tunnel.clone());
                    placed.insert(text(tunnel, "id"));
                }
            }
        }
        let port = addresses[0].clone();
        let own = fold(&project_key(&port));
        let key = if own == OTHER {
            forwards
                .iter()
                .map(|t| text(t, "project"))
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
            port,
            addresses: addresses
                .iter()
                .map(|p| text(p, "address").to_owned())
                .collect(),
            forwards,
            saved: false,
        });
    }
    if let Some(associations) = associations.as_mut() {
        associations.observe(server, &projects);
    }
    for tunnel in mine {
        if placed.contains(text(tunnel, "id")) {
            continue;
        }
        let association = associations.as_ref().and_then(|associations| {
            associations
                .entries
                .get(&(server.to_owned(), text(tunnel, "id").to_owned()))
        });
        let key = fold(
            association
                .map(|a| a.project.as_str())
                .unwrap_or_else(|| text(tunnel, "project")),
        );
        let mut port = tunnel.clone();
        if let Some(association) = association {
            port["worktreePath"] = Value::String(association.worktree.path.clone());
            port["worktreeBranch"] = serde_json::to_value(&association.worktree.branch).unwrap();
            port["worktreeMain"] = Value::Bool(association.worktree.main);
        }
        let index = *project_indexes.entry(key.clone()).or_insert_with(|| {
            projects.push(describe(&key, None, &known));
            projects.len() - 1
        });
        projects[index].entries.push(Entry {
            port,
            addresses: Vec::new(),
            forwards: vec![tunnel.clone()],
            saved: true,
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
            a.saved.cmp(&b.saved).then_with(|| {
                if a.saved {
                    return std::cmp::Ordering::Equal;
                }
                a.port["port"]
                    .as_u64()
                    .cmp(&b.port["port"].as_u64())
                    .then_with(|| text(&a.port, "address").cmp(text(&b.port, "address")))
            })
        });
    }
    projects.sort_by(|a, b| {
        (a.key == OTHER)
            .cmp(&(b.key == OTHER))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.key.cmp(&b.key))
    });
    projects
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn listener(port: u16, pid: u32, address: &str, project: &str) -> Value {
        json!({"port":port,"pid":pid,"address":address,"projectRoot":project})
    }

    fn forward(id: &str, port: u16, project: &str) -> Value {
        json!({"id":id,"serverId":"s","remoteHost":"127.0.0.1","remotePort":port,"project":project})
    }

    fn checkout_listener(port: u16, path: &str, branch: &str) -> Value {
        json!({"port":port,"pid":port,"address":"127.0.0.1","projectRoot":"/repo",
            "worktreePath":path,"worktreeBranch":branch,"worktreeMain":false})
    }

    #[test]
    fn saved_forward_keeps_observed_checkout_and_custom_name_when_listener_disappears() {
        let mut associations = WorktreeAssociations::default();
        let mut tunnel = forward("web", 3000, "/repo");
        tunnel["name"] = json!("My custom name");
        let saved = [tunnel.clone()];
        associations.group(
            vec![checkout_listener(3000, "/checkouts/ui/", "ui")],
            &saved,
            "s",
            &[],
        );
        let projects = associations.group(vec![], &saved, "s", &[]);
        let project = &projects[0];
        assert_eq!(project.key, "/repo");
        assert_eq!(project.worktrees[0].path, "/checkouts/ui");
        assert_eq!(project.worktrees[0].branch.as_deref(), Some("ui"));
        assert!(project.entries[0].saved);
        assert_eq!(project.entries[0].port["name"], "My custom name");
        assert_eq!(project.entries[0].forwards, [tunnel]);

        associations.group(
            vec![checkout_listener(3000, "/checkouts/ui", "renamed")],
            &saved,
            "s",
            &[],
        );
        let project = associations.group(vec![], &saved, "s", &[]).remove(0);
        assert_eq!(project.worktrees[0].branch.as_deref(), Some("renamed"));
    }

    #[test]
    fn checkout_associations_are_server_scoped_and_invalidated_by_destination_changes() {
        let mut associations = WorktreeAssociations::default();
        let tunnel = forward("web", 3000, "/repo");
        let mut foreign = tunnel.clone();
        foreign["serverId"] = json!("other");
        let saved = [tunnel.clone(), foreign];
        associations.group(
            vec![checkout_listener(3000, "/checkouts/ui", "ui")],
            &saved,
            "s",
            &[],
        );
        assert!(
            associations.group(vec![], &saved, "other", &[])[0]
                .worktrees
                .is_empty()
        );
        assert_eq!(
            associations.group(vec![], &saved, "s", &[])[0]
                .worktrees
                .len(),
            1
        );

        for (field, replacement) in [
            ("remotePort", json!(3001)),
            ("remotePortEnd", json!(3002)),
            ("remoteHost", json!("192.0.2.1")),
            ("direction", json!("remote")),
            ("project", json!("/another")),
        ] {
            associations.group(
                vec![checkout_listener(3000, "/checkouts/ui", "ui")],
                &saved,
                "s",
                &[],
            );
            let mut edited = tunnel.clone();
            edited[field] = replacement;
            assert!(
                associations.group(vec![], &[edited], "s", &[])[0]
                    .worktrees
                    .is_empty(),
                "{field}"
            );
        }
        associations.group(
            vec![checkout_listener(3000, "/checkouts/ui", "ui")],
            &saved,
            "s",
            &[],
        );
        associations.group(vec![], &[], "s", &[]);
        assert!(associations.entries.is_empty());
        assert!(
            associations.group(vec![], &saved, "s", &[])[0]
                .worktrees
                .is_empty()
        );
    }

    #[test]
    fn live_checkout_rename_updates_stopped_forwards_and_survives_all_listeners_stopping() {
        let mut associations = WorktreeAssociations::default();
        let saved = [forward("web", 3000, "/repo"), forward("api", 3001, "/repo")];
        associations.group(
            vec![
                checkout_listener(3000, "/checkouts/ui", "old"),
                checkout_listener(3001, "/checkouts/ui", "old"),
            ],
            &saved,
            "s",
            &[],
        );
        // Even an unsaved listener carries authoritative checkout metadata.
        let live = associations.group(
            vec![checkout_listener(3002, "/checkouts/ui", "new")],
            &saved,
            "s",
            &[],
        );
        assert_eq!(live[0].entries.len(), 3);
        assert!(
            live[0]
                .entries
                .iter()
                .all(|entry| entry.port["worktreeBranch"] == "new")
        );
        assert_eq!(live[0].worktrees.len(), 1);
        let stopped = associations.group(vec![], &saved, "s", &[]);
        assert_eq!(stopped[0].entries.len(), 2);
        assert!(
            stopped[0]
                .entries
                .iter()
                .all(|entry| entry.saved && entry.port["worktreeBranch"] == "new")
        );
        assert_eq!(stopped[0].worktrees[0].branch.as_deref(), Some("new"));
    }

    #[test]
    fn ambiguous_range_or_unknown_replacement_drops_checkout_association() {
        let mut associations = WorktreeAssociations::default();
        let mut tunnel = forward("range", 3000, "/repo");
        tunnel["remotePortEnd"] = json!(3002);
        let saved = [tunnel];
        let first = checkout_listener(3000, "/checkouts/ui", "ui");
        associations.group(vec![first.clone()], &saved, "s", &[]);
        assert_eq!(
            associations.group(vec![], &saved, "s", &[])[0]
                .worktrees
                .len(),
            1
        );
        associations.group(
            vec![
                first.clone(),
                checkout_listener(3001, "/checkouts/api", "api"),
                first.clone(),
            ],
            &saved,
            "s",
            &[],
        );
        assert!(
            associations.group(vec![], &saved, "s", &[])[0]
                .worktrees
                .is_empty()
        );
        associations.group(vec![first], &saved, "s", &[]);
        associations.group(
            vec![listener(3000, 9000, "127.0.0.1", "/repo")],
            &saved,
            "s",
            &[],
        );
        assert!(
            associations.group(vec![], &saved, "s", &[])[0]
                .worktrees
                .is_empty()
        );
    }

    #[test]
    fn listener_selection_does_not_expand_to_hidden_new_or_saved_only_rows() {
        let ports = (8000..8003)
            .map(|port| listener(port, u32::from(port), "127.0.0.1", "/repo"))
            .collect();
        let original = group(ports, &[], "s", &[]).remove(0);
        let selection = ListenerSelection::capture(&original.entries[..2]);
        // The first selected listener disappeared. The hidden listener, a new
        // discovery, and a saved-only forward must not replace it in the batch.
        let latest = group(
            (8001..8004)
                .map(|port| listener(port, u32::from(port), "127.0.0.1", "/repo"))
                .collect(),
            &[forward("saved-only", 8000, "/repo")],
            "s",
            &[],
        )
        .remove(0);
        let selected = selection.resolve(latest.entries.clone());
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].port["port"], 8001);
        assert!(
            ListenerSelection::capture(&[])
                .resolve(latest.entries)
                .is_empty()
        );
    }

    #[test]
    fn listener_selection_rejects_replaced_process_destination_and_checkout() {
        let mut port = listener(8000, 123, "127.0.0.1", "/repo");
        port["worktreePath"] = json!("/repo/feature/");
        let original = group(vec![port.clone()], &[], "s", &[]).remove(0);
        let selection = ListenerSelection::capture(&original.entries);
        for (field, replacement) in [
            ("pid", json!(456)),
            ("port", json!(8001)),
            ("address", json!("192.0.2.1")),
            ("worktreePath", json!("/repo/other")),
        ] {
            let mut changed = port.clone();
            changed[field] = replacement;
            let latest = group(vec![changed], &[], "s", &[]).remove(0);
            assert!(selection.resolve(latest.entries).is_empty(), "{field}");
        }
        port["worktreePath"] = json!("/repo/feature");
        port["processName"] = json!("node");
        port["worktreeBranch"] = json!("renamed-branch");
        let latest = group(vec![port], &[], "s", &[]).remove(0);
        assert_eq!(selection.resolve(latest.entries.clone()), latest.entries);
    }

    #[test]
    fn listener_selection_plans_with_latest_saved_forward_and_runtime() {
        use crate::{forwarding::discovered_tunnel, forwarding_batch, model::Tunnel};
        use uuid::Uuid;

        let server = Uuid::new_v4();
        let ports: Vec<_> = (8000..8003)
            .map(|port| listener(port, u32::from(port), "127.0.0.1", "/repo"))
            .collect();
        let original_forward = discovered_tunnel(&[], server, "127.0.0.1", 8000).unwrap();
        let original = group(
            ports.clone(),
            &[serde_json::to_value(&original_forward).unwrap()],
            &server.to_string(),
            &[],
        )
        .remove(0);
        let selection = ListenerSelection::capture(&original.entries[..2]);
        let mut latest_forward = original_forward.clone();
        latest_forward.id = Uuid::new_v4();
        latest_forward.local_port = 18000;
        latest_forward.name = "Changed after render".into();
        let latest = group(
            ports,
            &[serde_json::to_value(&latest_forward).unwrap()],
            &server.to_string(),
            &[],
        )
        .remove(0);
        let entries = selection
            .resolve(latest.entries)
            .into_iter()
            .map(|entry| forwarding_batch::Entry {
                destination: Some(forwarding_batch::Destination {
                    address: text(&entry.port, "address").to_owned(),
                    port: entry.port["port"].as_u64().unwrap() as u16,
                    name: "Discovered listener".into(),
                }),
                forwards: entry
                    .forwards
                    .into_iter()
                    .map(|value| serde_json::from_value::<Tunnel>(value).unwrap())
                    .collect(),
            })
            .collect::<Vec<_>>();
        let allocated = [latest_forward.clone()];
        let connected = forwarding_batch::plan(
            server,
            "/repo",
            &entries,
            &allocated,
            &HashSet::from([latest_forward.id]),
        )
        .unwrap();
        assert!(connected.connect.is_empty());
        assert_eq!(connected.stop, [latest_forward.clone()]);
        assert_eq!(connected.create.len(), 1);
        assert_eq!(connected.create[0].remote_port, 8001);
        let disconnected =
            forwarding_batch::plan(server, "/repo", &entries, &allocated, &HashSet::new()).unwrap();
        assert_eq!(disconnected.connect, [latest_forward]);
        assert!(disconnected.stop.is_empty());
        assert_eq!(disconnected.create.len(), 1);
        assert_eq!(disconnected.create[0].remote_port, 8001);
    }

    #[test]
    fn project_identity_prefers_repository_root_then_compose_directory_and_name() {
        let mut port = json!({"projectRoot":"/srv/repo/","composeWorkingDir":"/srv/repo/db/","composeProject":"database"});
        assert_eq!(project_key(&port), "/srv/repo");
        port["projectRoot"] = Value::Null;
        assert_eq!(project_key(&port), "/srv/repo/db");
        port["composeWorkingDir"] = Value::Null;
        assert_eq!(project_key(&port), "compose:database");
        port["composeProject"] = Value::Null;
        assert_eq!(project_key(&port), OTHER);
        assert_eq!(project_key(&json!({"projectRoot":"/"})), "/");
    }

    #[test]
    fn project_names_sources_and_catch_all() {
        let ports = vec![
            listener(22, 1, "127.0.0.1", ""),
            json!({"port":5432,"address":"0.0.0.0","containerName":"ledger-db-1","projectRoot":"/srv/odd-directory","composeProject":"ledger","projectMarker":"compose"}),
            json!({"port":3000,"pid":3,"projectRoot":"/srv/atlas.git/","projectMarker":".git"}),
            json!({"port":6379,"pid":4,"composeProject":"cache"}),
            json!({"port":7000,"pid":5,"projectRoot":"/srv/package","projectMarker":"Cargo.toml"}),
        ];
        let projects = group(ports, &[], "s", &[]);
        assert_eq!(
            projects
                .iter()
                .map(|p| (p.name.as_str(), p.source))
                .collect::<Vec<_>>(),
            vec![
                ("atlas", Source::Vcs),
                ("cache", Source::Compose),
                ("ledger", Source::Compose),
                ("package", Source::Marker),
                ("Other services", Source::Other)
            ]
        );
        assert_eq!(projects[0].marker.as_deref(), Some(".git"));
        assert_eq!(projects[1].path, None);
        assert_eq!(projects[2].path.as_deref(), Some("/srv/odd-directory"));
        assert_eq!(projects[4].marker, None);
    }

    #[test]
    fn known_names_fold_nested_roots_at_the_longest_path_boundary() {
        let ports = vec![
            listener(1, 1, "127.0.0.1", "/srv/repo/packages/api"),
            listener(2, 2, "127.0.0.1", "/srv/repo/ui"),
            listener(3, 3, "127.0.0.1", "/srv/repository"),
        ];
        let known = vec![
            json!({"name":"Repository", "path":"/srv/repo/"}),
            json!({"name":"API", "path":{"display":"/srv/repo/packages/","bytesB64":""}, "serverId":"s"}),
            json!({"name":"Wrong server", "path":"/srv/repository", "serverId":"elsewhere"}),
        ];
        let projects = group(ports, &[], "s", &known);
        assert_eq!(
            projects
                .iter()
                .map(|p| (p.key.as_str(), p.name.as_str(), p.source))
                .collect::<Vec<_>>(),
            vec![
                ("/srv/repo/packages", "API", Source::Saved),
                ("/srv/repo", "Repository", Source::Saved),
                ("/srv/repository", "repository", Source::Marker),
            ]
        );
    }

    #[test]
    fn known_root_slash_absorbs_absolute_paths_but_not_other_or_compose_names() {
        let projects = group(
            vec![
                listener(1, 1, "127.0.0.1", "/"),
                listener(2, 2, "127.0.0.1", "/srv/app"),
                listener(3, 3, "127.0.0.1", ""),
                json!({"port":4,"composeProject":"named"}),
            ],
            &[],
            "s",
            &[json!({"name":"Root", "path":"/"})],
        );
        let root = projects.iter().find(|p| p.key == "/").unwrap();
        assert_eq!(root.name, "Root");
        assert_eq!(root.entries.len(), 2);
        assert_eq!(projects.len(), 3);
        assert_eq!(
            group(vec![listener(1, 1, "127.0.0.1", "/")], &[], "s", &[])[0].name,
            "/"
        );
    }

    #[test]
    fn dual_stack_preserves_all_addresses_and_owner_isolation_and_destination_order() {
        let ports = vec![
            listener(3000, 7, "::", ""),
            listener(3000, 7, "192.0.2.2", ""),
            listener(3000, 8, "::1", ""),
            listener(3000, 7, "0.0.0.0", ""),
            listener(3000, 7, "127.0.0.1", ""),
        ];
        let mut ipv6 = forward("v6", 3000, "");
        ipv6["remoteHost"] = json!("::1");
        let projects = group(ports, &[ipv6, forward("v4", 3000, "")], "s", &[]);
        let entries = &projects[0].entries;
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].port["pid"], 7);
        assert_eq!(entries[0].port["address"], "0.0.0.0");
        assert_eq!(
            entries[0].addresses,
            ["0.0.0.0", "127.0.0.1", "::", "192.0.2.2"]
        );
        assert_eq!(
            entries[0]
                .forwards
                .iter()
                .map(|t| text(t, "id"))
                .collect::<Vec<_>>(),
            ["v4", "v6"]
        );
        assert_eq!(entries[1].port["pid"], 8);
        assert_eq!(entries[1].forwards.len(), 1);
    }

    #[test]
    fn unknown_pids_fall_back_to_container_then_process_without_merging_other_owners() {
        let ports = vec![
            json!({"port":3000,"pid":null,"containerName":"api","processName":"node","address":"::"}),
            json!({"port":3000,"containerName":"api","address":"0.0.0.0"}),
            json!({"port":3000,"containerName":"web","address":"0.0.0.0"}),
            json!({"port":3001,"processName":"node","address":"::"}),
            json!({"port":3001,"processName":"node","address":"0.0.0.0"}),
        ];
        let groups = group(ports, &[], "s", &[]);
        assert_eq!(groups[0].entries.len(), 3);
        assert_eq!(groups[0].entries[0].addresses.len(), 2);
        assert_eq!(groups[0].entries[1].addresses.len(), 1);
        assert_eq!(groups[0].entries[2].addresses.len(), 2);
    }

    #[test]
    fn saved_leftovers_keep_filed_projects_and_exclude_foreign_servers() {
        let mut reverse = forward("reverse", 5432, "");
        reverse["direction"] = json!("remote");
        let mut foreign = forward("foreign", 9999, "/srv/foreign");
        foreign["serverId"] = json!("elsewhere");
        let projects = group(
            vec![listener(5432, 1, "0.0.0.0", "/srv/db")],
            &[
                forward("db", 5432, "/srv/other"),
                forward("filed", 9999, "/srv/repo/packages/api"),
                reverse.clone(),
                foreign,
            ],
            "s",
            &[json!({"path":"/srv/repo", "name":"My application"})],
        );
        assert_eq!(projects.len(), 3);
        assert_eq!(projects[0].key, "/srv/db");
        assert!(!projects[0].entries[0].saved);
        assert_eq!(projects[0].entries[0].forwards[0]["id"], "db");
        assert_eq!(projects[1].name, "My application");
        assert!(projects[1].entries[0].saved);
        assert!(projects[1].entries[0].addresses.is_empty());
        assert_eq!(projects[1].entries[0].port["id"], "filed");
        assert_eq!(projects[2].entries[0].forwards, [reverse]);
    }

    #[test]
    fn filed_forward_attributes_unknown_listener_and_ranges_attach_to_each_reached_port() {
        let mut range = forward("range", 3000, "/srv/repo/pkg");
        range["remotePortEnd"] = json!(3002);
        let projects = group(
            vec![
                listener(3002, 2, "127.0.0.1", ""),
                listener(3000, 1, "127.0.0.1", ""),
            ],
            &[range.clone()],
            "s",
            &[json!({"name":"Repo", "path":"/srv/repo"})],
        );
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].key, "/srv/repo");
        assert_eq!(projects[0].entries.len(), 2);
        assert_eq!(projects[0].entries[0].port["port"], 3000);
        assert!(
            projects[0]
                .entries
                .iter()
                .all(|e| !e.saved && e.forwards == [range.clone()])
        );
    }

    #[test]
    fn linked_worktrees_share_repository_and_main_checkout_sorts_first() {
        let ports = vec![
            json!({"port":3001,"pid":1,"address":"127.0.0.1","projectRoot":"/srv/repo","worktreePath":"/srv/search/","worktreeBranch":"feature/search","worktreeMain":false}),
            json!({"port":3000,"pid":2,"address":"127.0.0.1","projectRoot":"/srv/repo","worktreePath":"/srv/repo","worktreeBranch":"main"}),
            json!({"port":3002,"pid":3,"address":"127.0.0.1","projectRoot":"/srv/repo","worktreePath":"/srv/search","worktreeBranch":"feature/search","worktreeMain":false}),
        ];
        let projects = group(ports, &[], "s", &[]);
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].entries.len(), 3);
        assert_eq!(
            projects[0].worktrees,
            [
                Worktree {
                    path: "/srv/repo".into(),
                    branch: Some("main".into()),
                    main: true
                },
                Worktree {
                    path: "/srv/search".into(),
                    branch: Some("feature/search".into()),
                    main: false
                }
            ]
        );
    }
}
