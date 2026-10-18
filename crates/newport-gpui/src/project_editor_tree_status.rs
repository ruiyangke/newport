//! Explorer decorations are an unfiltered, snapshot-consistent read. Git's
//! search box and pagination must never turn unrelated files apparently clean.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Status {
    Untracked,
    Added,
    Modified,
    Conflict,
}
impl Status {
    pub(super) fn color(self, cx: &App) -> Hsla {
        match self {
            Self::Untracked | Self::Added => theme::success(cx.theme().is_dark()),
            Self::Modified => theme::warning(cx.theme().is_dark()),
            Self::Conflict => theme::error(cx.theme().is_dark()),
        }
    }
    pub(super) fn description(self) -> &'static str {
        match self {
            Self::Untracked => "Untracked",
            Self::Added => "Added",
            Self::Modified => "Changed",
            Self::Conflict => "Conflict",
        }
    }
    pub(super) fn mark(self) -> &'static str {
        match self {
            Self::Untracked => "?",
            Self::Added => "A",
            Self::Modified => "M",
            Self::Conflict => "!",
        }
    }
}
#[derive(Default)]
pub(super) struct State {
    repo: String,
    loading: bool,
    generation: usize,
    pub(super) read_at: Option<Instant>,
    entries: BTreeMap<String, Status>,
}
impl State {
    pub(super) fn status(&self, root: &str, path: &str) -> Option<Status> {
        if path == root {
            return self.entries.values().copied().max();
        }
        self.entries
            .get(
                path.strip_prefix(root.trim_end_matches('/'))?
                    .strip_prefix('/')?,
            )
            .copied()
    }
}
fn decorations(page: &Value) -> BTreeMap<String, Status> {
    let mut result = BTreeMap::new();
    for entry in rows(&page["entries"]) {
        // Use lossless UTF-8 paths, not escaped display labels. Unsupported
        // byte paths are not editable in this Explorer either.
        let Some(path) = root_path(&entry["path"]) else {
            continue;
        };
        let path = path.trim_end_matches('/');
        if path.is_empty()
            || path.starts_with('/')
            || path.split('/').any(|c| matches!(c, "" | "." | ".."))
        {
            continue;
        }
        let flags = entry["flags"].as_u64().unwrap_or(0);
        let status = if entry["conflicted"] == true {
            Status::Conflict
        } else if entry["untracked"] == true {
            Status::Untracked
        } else if flags & 1 != 0 {
            Status::Added
        } else if entry["staged"] == true || entry["unstaged"] == true {
            Status::Modified
        } else {
            continue;
        };
        let mut current = Some(path);
        while let Some(path) = current {
            result
                .entry(path.to_owned())
                .and_modify(|old: &mut Status| *old = (*old).max(status))
                .or_insert(status);
            current = path.rsplit_once('/').map(|(parent, _)| parent);
        }
    }
    result
}
impl CodeWorkspace {
    pub(super) fn refresh_tree_status(
        &mut self,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tree_status.repo != self.navigator.repo {
            self.tree_status = State {
                repo: self.navigator.repo.clone(),
                generation: self.tree_status.generation + 1,
                ..Default::default()
            };
        }
        if !self.available
            || !self.visible
            || !self.files
            || !self.show_tree
            || self.tree_status.repo.is_empty()
            || self.tree_status.loading
            || (!force
                && self
                    .tree_status
                    .read_at
                    .is_some_and(|at| at.elapsed() < Duration::from_secs(15)))
        {
            return;
        }
        self.tree_status.loading = true;
        self.tree_status.read_at = Some(Instant::now());
        self.tree_status.generation += 1;
        self.read_tree_status(Value::Null, window, cx);
    }
    fn read_tree_status(&mut self, previous: Value, window: &mut Window, cx: &mut Context<Self>) {
        let repo = self.tree_status.repo.clone();
        let connection = self.connection.clone();
        let generation = self.tree_status.generation;
        let cursor = previous["nextCursor"].as_str().map(str::to_owned);
        let mut params = json!({"repoId":repo,"pageSize":200,"filter":{"text":"","group":"all"}});
        if let Some(cursor) = &cursor {
            params["cursor"] = json!(cursor);
        }
        self.rpc(
            "git_request",
            json!({"serverId":self.server,"request":{"method":"repo.status","params":params}}),
            window,
            cx,
            move |this, result, window, cx| {
                if this.tree_status.repo != repo
                    || this.navigator.repo != repo
                    || this.tree_status.generation != generation
                    || this.connection != connection
                {
                    return;
                }
                let result = result.and_then(|page| {
                    if let Some(cursor) = cursor {
                        newport_client::git_page::merge(&previous, page, &cursor)
                    } else {
                        newport_client::git_page::validate(&page).map(|_| page)
                    }
                });
                match result {
                    Ok(page)
                        if page["nextCursor"].is_string()
                            && rows(&page["entries"]).len() < 20_000 =>
                    {
                        this.read_tree_status(page, window, cx)
                    }
                    Ok(page) if page["nextCursor"].is_null() => {
                        this.tree_status.entries = decorations(&page);
                        this.tree_status.loading = false;
                    }
                    _ => {
                        // Unknown status is neutral, never an old or mixed snapshot.
                        this.tree_status.entries.clear();
                        this.tree_status.loading = false;
                    }
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{State, Status, decorations};
    use serde_json::{Value, json};
    fn entry(path: &str, status: &str) -> Value {
        use base64::Engine;
        let mut e = json!({"path":{"bytesB64":base64::engine::general_purpose::STANDARD.encode(path), "display":"not the actual path"}});
        e[status] = json!(true);
        e
    }
    #[test]
    fn status_propagates_to_ancestors_without_prefix_collisions() {
        let entries = decorations(
            &json!({"entries":[entry("src/new.rs","untracked"),entry("src/nested/mod.rs","unstaged"),entry("src/nested/conflict.rs","conflicted"),entry("src-other/clean.rs","unused")]}),
        );
        assert_eq!(entries["src"], Status::Conflict);
        assert_eq!(entries["src/nested"], Status::Conflict);
        assert_eq!(entries["src/new.rs"], Status::Untracked);
        assert_eq!(entries["src/nested/mod.rs"], Status::Modified);
        assert!(!entries.contains_key("src-other"));
        let state = State {
            entries,
            ..Default::default()
        };
        assert_eq!(state.status("/repo", "/repo/src"), Some(Status::Conflict));
        assert_eq!(state.status("/repo", "/repo"), Some(Status::Conflict));
        assert_eq!(state.status("/repo", "/repository/src"), None);
    }
    #[test]
    fn staged_additions_and_directory_entries_preserve_meaning() {
        let mut added = entry("Cargo.toml", "staged");
        added["flags"] = json!(1);
        let entries = decorations(
            &json!({"entries":[added, entry("new-dir/","untracked"), entry("../escape","unstaged"),entry("/absolute","unstaged")]}),
        );
        assert_eq!(entries["Cargo.toml"], Status::Added);
        assert_eq!(entries["new-dir"], Status::Untracked);
        assert_eq!(entries.len(), 2);
    }
}
