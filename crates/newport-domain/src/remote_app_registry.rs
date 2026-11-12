//! The remote apps Newport has given an identity of their own, by
//! AppUserModelID (see [`crate::remote_app_identity`]), so a taskbar pin or a
//! shortcut that names only the ID opens the right app on the right server.
//!
//! An app's first window records it as a temporary entry. Apps the user adds
//! (to the Start menu) are kept; temporary ones go when unused, within the
//! limits [`plan_cleanup`] applies.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// The file format this build reads and writes.
pub const VERSION: u32 = 1;
/// A temporary entry unused for this long is removed.
pub const MAX_IDLE_SECONDS: u64 = 30 * 24 * 60 * 60;
/// Entries kept per server; temporary ones go first, least recently used
/// first.
pub const MAX_PER_SERVER: usize = 64;
/// Apps the user may add, over every server.
pub const MAX_ADDED: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    pub version: u32,
    pub entries: Vec<Entry>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            version: VERSION,
            entries: Vec::new(),
        }
    }
}

/// How Newport finds an app on its server again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum App {
    /// An app with a desktop file, by desktop id.
    DesktopId(String),
    /// An app id with no desktop file.
    AppId(String),
    /// Windows with no app: the server's catch-all group.
    Server,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The AppUserModelID.
    pub id: String,
    /// The server's id.
    pub server: String,
    /// The server's name as last seen, for display and collisions.
    #[serde(default)]
    pub server_name: String,
    pub app: App,
    /// The app's name as last seen, for display and collisions.
    pub name: String,
    /// Added by the user, rather than recorded as its window opened.
    pub added: bool,
    /// Seconds since the Unix epoch.
    pub last_used: u64,
    /// The app's desktop actions, such as "New Window", as its catalogue
    /// last listed them: an added app's jump list runs them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
}

/// A desktop action, by its id in the app's desktop file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub id: String,
    pub name: String,
}

impl Registry {
    /// A registry read from a file. A file of another version is refused, as
    /// a damaged one is.
    pub fn from_json(bytes: &[u8]) -> Result<Self, String> {
        let registry: Self = serde_json::from_slice(bytes)
            .map_err(|error| format!("The remote apps registry could not be read: {error}"))?;
        if registry.version != VERSION {
            return Err(format!(
                "The remote apps registry has version {}, not {VERSION}",
                registry.version
            ));
        }
        Ok(registry)
    }

    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).expect("The registry serializes")
    }

    pub fn find(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    /// An app whose window opened: a new temporary entry, or the known one
    /// with its name and last use refreshed. An added app stays added, and
    /// keeps its actions when the window knows none.
    /// Returns whether anything changed.
    pub fn record(&mut self, entry: Entry) -> bool {
        match self.entries.iter_mut().find(|known| known.id == entry.id) {
            Some(known) => {
                let actions = if entry.actions.is_empty() {
                    known.actions.clone()
                } else {
                    entry.actions.clone()
                };
                let refreshed = Entry {
                    added: known.added,
                    last_used: known.last_used.max(entry.last_used),
                    actions,
                    ..entry
                };
                let changed = *known != refreshed;
                *known = refreshed;
                changed
            }
            None => {
                self.entries.push(Entry {
                    added: false,
                    ..entry
                });
                true
            }
        }
    }

    /// The person added the app (`added`), with the actions the page lists,
    /// or took it off the Start menu: it stays as a temporary entry, which
    /// pins and windows may still name. Either way it counts as used.
    /// Returns whether anything changed.
    pub fn set_added(&mut self, entry: Entry, added: bool) -> bool {
        if !added && self.find(&entry.id).is_none() {
            return false;
        }
        let changed = self.record(entry.clone());
        let known = self
            .entries
            .iter_mut()
            .find(|known| known.id == entry.id)
            .expect("Recorded above");
        let was = std::mem::replace(&mut known.added, added);
        let actions_changed = added && known.actions != entry.actions;
        if actions_changed {
            known.actions = entry.actions;
        }
        changed || was != added || actions_changed
    }

    /// The server was renamed: its entries take the new name. Returns
    /// whether anything changed.
    pub fn rename_server(&mut self, server: &str, name: &str) -> bool {
        let mut changed = false;
        for entry in self
            .entries
            .iter_mut()
            .filter(|entry| entry.server == server && entry.server_name != name)
        {
            entry.server_name = name.to_owned();
            changed = true;
        }
        changed
    }

    pub fn remove(&mut self, ids: &[String]) {
        self.entries.retain(|entry| !ids.contains(&entry.id));
    }

    /// Removes every entry of the server; returns their ids.
    pub fn forget_server(&mut self, server: &str) -> Vec<String> {
        let ids: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| entry.server == server)
            .map(|entry| entry.id.clone())
            .collect();
        self.remove(&ids);
        ids
    }

    pub fn added(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|entry| entry.added)
    }
}

/// The entries to remove at `now`, in seconds since the Unix epoch:
/// - temporary entries unused for [`MAX_IDLE_SECONDS`];
/// - past [`MAX_PER_SERVER`] on a server, its least recently used temporary
///   entries;
/// - past [`MAX_ADDED`] added apps, the least recently used of them.
///
/// An entry whose ID a taskbar pin or a Start shortcut uses (`in_use`) is
/// never removed, so the pin keeps working however long it goes unused; it
/// still counts towards the limits.
pub fn plan_cleanup(registry: &Registry, now: u64, in_use: &HashSet<String>) -> Vec<String> {
    let mut remove = Vec::new();
    let mut kept: Vec<&Entry> = Vec::new();
    let removable = |entry: &Entry| !in_use.contains(&entry.id);
    for entry in &registry.entries {
        if !entry.added
            && removable(entry)
            && now.saturating_sub(entry.last_used) > MAX_IDLE_SECONDS
        {
            remove.push(entry.id.clone());
        } else {
            kept.push(entry);
        }
    }
    // Least recently used first; the id breaks ties so the plan is stable.
    kept.sort_by(|a, b| (a.last_used, &a.id).cmp(&(b.last_used, &b.id)));
    let added = kept.iter().filter(|entry| entry.added).count();
    let mut surplus_added = added.saturating_sub(MAX_ADDED);
    kept.retain(|entry| {
        if entry.added && removable(entry) && surplus_added > 0 {
            surplus_added -= 1;
            remove.push(entry.id.clone());
            false
        } else {
            true
        }
    });
    let mut servers: Vec<&str> = kept.iter().map(|entry| entry.server.as_str()).collect();
    servers.sort_unstable();
    servers.dedup();
    for server in servers {
        let on_server = kept.iter().filter(|entry| entry.server == server).count();
        let surplus = on_server.saturating_sub(MAX_PER_SERVER);
        remove.extend(
            kept.iter()
                .filter(|entry| entry.server == server && !entry.added && removable(entry))
                .take(surplus)
                .map(|entry| entry.id.clone()),
        );
    }
    remove
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 24 * 60 * 60;
    const NOW: u64 = 1_800_000_000;

    fn entry(id: &str, server: &str, added: bool, last_used: u64) -> Entry {
        Entry {
            id: id.into(),
            server: server.into(),
            server_name: format!("{server} name"),
            app: App::DesktopId(format!("{id}.desktop")),
            name: id.into(),
            added,
            last_used,
            actions: Vec::new(),
        }
    }

    #[test]
    fn a_registry_round_trips_through_its_file_format() {
        let registry = Registry {
            version: VERSION,
            entries: vec![
                entry("Newport.Remote.a.b", "s1", true, NOW),
                Entry {
                    app: App::AppId("xterm".into()),
                    ..entry("Newport.Remote.a.c", "s1", false, NOW - 5)
                },
                Entry {
                    app: App::Server,
                    ..entry("Newport.Remote.a.d", "s1", false, NOW - 9)
                },
            ],
        };
        let json = registry.to_json();
        assert_eq!(Registry::from_json(&json).unwrap(), registry);
        let value: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(value["version"], 1);
        assert_eq!(
            value["entries"][0]["app"]["desktop_id"],
            "Newport.Remote.a.b.desktop"
        );
        assert_eq!(value["entries"][1]["app"]["app_id"], "xterm");
        assert_eq!(value["entries"][2]["app"], "server");
    }

    #[test]
    fn damaged_files_and_other_versions_are_refused() {
        for bytes in [
            &b""[..],
            b"{",
            b"[]",
            br#"{"version":1}"#,
            br#"{"version":2,"entries":[]}"#,
            br#"{"version":1,"entries":[{"id":"x"}]}"#,
        ] {
            assert!(Registry::from_json(bytes).is_err(), "{bytes:?}");
        }
        assert_eq!(
            Registry::from_json(br#"{"version":1,"entries":[]}"#).unwrap(),
            Registry::default()
        );
    }

    #[test]
    fn recording_refreshes_an_entry_and_keeps_it_added() {
        let mut registry = Registry::default();
        assert!(registry.record(entry("a", "s1", true, NOW - 10)));
        assert!(!registry.find("a").unwrap().added, "a window adds nothing");
        registry.entries[0].added = true;
        assert!(registry.record(Entry {
            name: "Calculator".into(),
            ..entry("a", "s1", false, NOW)
        }));
        let known = registry.find("a").unwrap();
        assert!(known.added);
        assert_eq!(known.name, "Calculator");
        assert_eq!(known.last_used, NOW);
        assert!(!registry.record(Entry {
            name: "Calculator".into(),
            ..entry("a", "s1", false, NOW - 1)
        }));
        assert_eq!(registry.find("a").unwrap().last_used, NOW);
        assert_eq!(registry.entries.len(), 1);
        registry.remove(&["a".into()]);
        assert!(registry.find("a").is_none());
    }

    #[test]
    fn unused_temporary_entries_go_after_thirty_days() {
        let registry = Registry {
            version: VERSION,
            entries: vec![
                entry("old", "s1", false, NOW - 31 * DAY),
                entry("recent", "s1", false, NOW - 29 * DAY),
                entry("added", "s1", true, NOW - 400 * DAY),
                // A clock set back never removes anything.
                entry("future", "s1", false, NOW + DAY),
            ],
        };
        assert_eq!(
            plan_cleanup(&registry, NOW, &HashSet::new()),
            vec!["old".to_owned()]
        );
    }

    #[test]
    fn a_server_keeps_its_most_recent_entries_and_every_added_one() {
        let mut entries: Vec<_> = (0..70)
            .map(|n| entry(&format!("t{n:02}"), "s1", false, NOW - 70 + n))
            .collect();
        entries.push(entry("added-oldest", "s1", true, NOW - 1000));
        entries.extend((0..10).map(|n| entry(&format!("other{n}"), "s2", false, NOW - 2000)));
        let registry = Registry {
            version: VERSION,
            entries,
        };
        let removed = plan_cleanup(&registry, NOW, &HashSet::new());
        // 71 entries on s1: the 7 least recently used temporary ones go.
        assert_eq!(
            removed,
            (0..7).map(|n| format!("t{n:02}")).collect::<Vec<_>>()
        );
    }

    #[test]
    fn past_thirty_two_added_apps_the_least_recently_used_go() {
        let entries: Vec<_> = (0..34)
            .map(|n| {
                entry(
                    &format!("a{n:02}"),
                    &format!("s{}", n % 3),
                    true,
                    NOW - 100 + n,
                )
            })
            .collect();
        let registry = Registry {
            version: VERSION,
            entries,
        };
        assert_eq!(
            plan_cleanup(&registry, NOW, &HashSet::new()),
            vec!["a00".to_owned(), "a01".to_owned()]
        );
        let mut within = registry.clone();
        within.entries.truncate(MAX_ADDED);
        assert!(plan_cleanup(&within, NOW, &HashSet::new()).is_empty());
    }

    #[test]
    fn entries_a_pin_or_shortcut_uses_are_never_removed() {
        let mut entries: Vec<_> = (0..66)
            .map(|n| entry(&format!("t{n:02}"), "s1", false, NOW - 66 + n))
            .collect();
        entries.push(entry("pinned-old", "s1", false, NOW - 400 * DAY));
        entries.extend((0..34).map(|n| entry(&format!("a{n:02}"), "s2", true, NOW - 100 + n)));
        let registry = Registry {
            version: VERSION,
            entries,
        };
        let in_use: HashSet<String> = ["pinned-old", "t00", "a00"].map(String::from).into();
        let removed = plan_cleanup(&registry, NOW, &in_use);
        // 34 added, 2 over; 67 on s1, 3 over: the oldest unprotected go.
        assert_eq!(removed, ["a01", "a02", "t01", "t02", "t03"]);
        assert!(!removed.iter().any(|id| in_use.contains(id)));
        let unprotected = plan_cleanup(&registry, NOW, &HashSet::new());
        assert!(unprotected.contains(&"pinned-old".to_owned()));
        assert!(unprotected.contains(&"a00".to_owned()));
    }

    #[test]
    fn adding_marks_an_entry_and_taking_it_off_keeps_it_temporary() {
        let mut registry = Registry::default();
        assert!(!registry.set_added(entry("a", "s1", false, NOW), false));
        assert!(registry.entries.is_empty(), "Nothing to take off");
        assert!(registry.set_added(entry("a", "s1", false, NOW), true));
        assert!(registry.find("a").unwrap().added);
        assert!(!registry.set_added(entry("a", "s1", false, NOW), true));
        assert_eq!(registry.added().count(), 1);
        assert!(registry.set_added(entry("a", "s1", true, NOW + 5), false));
        let known = registry.find("a").unwrap();
        assert!(!known.added);
        assert_eq!(known.last_used, NOW + 5);
        assert_eq!(registry.added().count(), 0);
    }

    #[test]
    fn forgetting_a_server_removes_only_its_entries() {
        let mut registry = Registry {
            version: VERSION,
            entries: vec![
                entry("a", "s1", true, NOW),
                entry("b", "s2", false, NOW),
                entry("c", "s1", false, NOW),
            ],
        };
        assert_eq!(registry.forget_server("s1"), ["a", "c"]);
        assert_eq!(registry.entries, [entry("b", "s2", false, NOW)]);
        assert!(registry.forget_server("s1").is_empty());
    }

    #[test]
    fn an_entry_without_a_server_name_reads_as_unnamed() {
        let registry = Registry::from_json(
            br#"{"version":1,"entries":[{"id":"x","server":"s1","app":"server","name":"lab apps","added":false,"last_used":1}]}"#,
        )
        .unwrap();
        assert_eq!(registry.entries[0].server_name, "");
        assert!(registry.entries[0].actions.is_empty());
    }

    fn action(id: &str, name: &str) -> Action {
        Action {
            id: id.into(),
            name: name.into(),
        }
    }

    #[test]
    fn actions_come_from_the_catalogue_and_stay_when_a_window_knows_none() {
        let mut registry = Registry::default();
        let window = Entry {
            actions: vec![action("new-window", "New Window")],
            ..entry("a", "s1", false, NOW)
        };
        assert!(registry.record(window.clone()));
        assert!(!registry.record(entry("a", "s1", false, NOW)));
        assert_eq!(registry.find("a").unwrap().actions, window.actions);
        let json = registry.to_json();
        assert_eq!(Registry::from_json(&json).unwrap(), registry);
        // The page lists them exactly, none included.
        assert!(registry.set_added(entry("a", "s1", true, NOW), true));
        assert!(registry.find("a").unwrap().actions.is_empty());
        let value: serde_json::Value = serde_json::from_slice(&registry.to_json()).unwrap();
        assert!(value["entries"][0].get("actions").is_none());
        let both = vec![
            action("new-window", "New Window"),
            action("new-private-window", "New Incognito Window"),
        ];
        assert!(registry.set_added(
            Entry {
                actions: both.clone(),
                ..entry("a", "s1", true, NOW)
            },
            true
        ));
        assert_eq!(registry.find("a").unwrap().actions, both);
    }

    #[test]
    fn a_renamed_server_renames_its_entries() {
        let mut registry = Registry {
            version: VERSION,
            entries: vec![
                entry("a", "s1", true, NOW),
                entry("b", "s2", false, NOW),
                entry("c", "s1", false, NOW),
            ],
        };
        assert!(registry.rename_server("s1", "build box"));
        assert!(!registry.rename_server("s1", "build box"));
        assert!(!registry.rename_server("s3", "other"));
        let names: Vec<_> = registry
            .entries
            .iter()
            .map(|entry| entry.server_name.as_str())
            .collect();
        assert_eq!(names, ["build box", "s2 name", "build box"]);
    }
}
