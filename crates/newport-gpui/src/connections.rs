//! Per-server presentation state shared by the Projects and Ports views.
use super::*;
use gpui_kit::component::input::InputEvent;

#[derive(Default)]
pub(super) struct State {
    pub(super) snapshot_revision: u64,
    cached_reads: Option<(Value, newport_client::resources::ResourceGroup)>,
    refresh_requested: bool,
    projects_reload: bool,
    pub(super) input: Option<Entity<InputState>>,
    pub(super) projects: std::cell::RefCell<super::connections_grouped::ProjectsCache>,
    pub(super) visible: std::cell::RefCell<super::connections_grouped::VisibleCache>,
    pub(super) prepared: std::cell::RefCell<super::connections_grouped::PreparedCache>,
    pub(super) viewport: std::cell::RefCell<super::connections_grouped::Viewport>,
    subscription: Option<Subscription>,
    pub(super) filter: usize,
    pub(super) table: bool,
    pub(super) sort: usize,
    pub(super) descending: bool,
    pub(super) collapsed_worktrees: std::collections::HashSet<(String, String)>,
    pub(super) worktree_associations:
        std::cell::RefCell<newport_domain::port_grouping::WorktreeAssociations>,
    pub(super) collapsed: std::collections::HashSet<String>,
    pub(super) expanded: std::collections::HashSet<String>,
}

fn forwarding_snapshot_changed(before: &Value, after: &Value, server: &str) -> bool {
    fn tunnels<'a>(
        snapshot: &'a Value,
        server: &'a str,
    ) -> impl Iterator<Item = &'a Value> + Clone {
        snapshot["config"]["tunnels"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .filter(move |tunnel| tunnel["serverId"] == server)
    }
    let old = tunnels(before, server);
    let mut new = tunnels(after, server);
    !old.eq(new.clone())
        || new.any(|tunnel| {
            let id = tunnel["id"].as_str().unwrap_or_default();
            ["tunnels", "tunnelHealth"]
                .iter()
                .any(|field| before["runtime"][field][id] != after["runtime"][field][id])
        })
}

/// Search the combined, curated context of a project and its entry.
/// All listeners remain searchable after dual-stack rows are merged.
pub(super) fn entry_matches(
    project: &newport_domain::port_grouping::Project,
    entry: &newport_domain::port_grouping::Entry,
    query: &str,
) -> bool {
    if query.is_empty() {
        return true;
    }
    let mut parts = vec![project.name.clone(), project.key.clone()];
    if entry.saved {
        for key in [
            "name",
            "localPort",
            "remotePort",
            "remoteHost",
            "worktreePath",
            "worktreeBranch",
        ] {
            if let Some(value) = entry.port.get(key).filter(|v| !v.is_null()) {
                parts.push(
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string()),
                );
            }
        }
    } else {
        parts.push(newport_domain::port_context::search_text(
            &entry.port,
            &entry.addresses,
        ));
        parts.extend(entry.forwards.iter().map(|t| {
            format!(
                "{} {}",
                t["name"].as_str().unwrap_or_default(),
                t["localPort"]
            )
        }));
    }
    parts.join(" ").to_lowercase().contains(query)
}

impl Newport {
    pub(super) fn sync_connections_snapshot(&mut self, next: &Value) {
        // A snapshot accepted on another page must invalidate its server's
        // retained projection, while unrelated server or feature updates
        // leave it reusable. Do not use the whole snapshot's data revision.
        for (server, state) in &mut self.connections_state {
            if forwarding_snapshot_changed(&self.snapshot, next, server) {
                state.snapshot_revision += 1;
            }
        }
    }
    pub(super) fn poll_connections(&mut self, now: Instant) -> bool {
        if self.window_visible {
            self.refresh_connection_projects(now);
        }
        if self.page != Page::Ports
            || !self.configuration_available()
            || !self
                .resources
                .take_poll("page", now, Duration::from_secs(1), self.window_visible)
        {
            return false;
        }
        let before = self
            .resources
            .get("page")
            .map(|r| (r.manual_pending, r.error.clone(), r.presentation_revision));
        self.discover_connections(false);
        let after = self
            .resources
            .get("page")
            .map(|r| (r.manual_pending, r.error.clone(), r.presentation_revision));
        before != after
    }

    pub(super) fn connection_projects_changed(&mut self, server: &str) {
        if let Some(state) = self.connections_state.get_mut(server) {
            state.projects_reload = true;
        }
        if self.server == server && self.page == Page::Ports {
            self.connections_state
                .entry(server.into())
                .or_default()
                .projects_reload = true;
            // A pre-mutation read must not acknowledge a later bookmark write.
            // Retain its displayed value while fencing the worker generation.
            let reads = self.resources.take_cached(&["connection-projects"]);
            self.resources.restore(reads);
        }
    }

    pub(super) fn refresh_connection_projects(&mut self, now: Instant) {
        if self.page != Page::Ports || self.server.is_empty() || !self.configuration_available() {
            return;
        }
        let state = self
            .connections_state
            .entry(self.server.clone())
            .or_default();
        let due = state.projects_reload
            || self.resources.get("connection-projects").is_none_or(|row| {
                row.finished_at.is_none_or(|at| {
                    now.checked_duration_since(at)
                        .is_some_and(|elapsed| elapsed >= Duration::from_secs(60))
                })
            });
        if !due {
            return;
        }
        let generation = self.resources.begin_refresh("connection-projects", false);
        if generation.is_some() {
            state.projects_reload = false;
        }
        self.dispatch_request(
            "connection-projects",
            "git_projects_list",
            json!({"serverId":self.server}),
            generation,
        );
    }

    pub(super) fn prune_connections(&mut self) {
        if !self.configuration_available() {
            return;
        }
        let servers = rows(&self.snapshot["config"]["servers"]);
        self.connections_state.retain(|id, state| {
            let Some(server) = servers.iter().find(|server| server["id"] == *id) else {
                return false;
            };
            let scope = terminal_scope::connection(server, &self.snapshot);
            if state
                .cached_reads
                .as_ref()
                .is_some_and(|(saved, _)| *saved != scope)
            {
                state.cached_reads = None;
                state.refresh_requested = false;
                state.projects_reload = false;
            }
            true
        });
    }

    pub(super) fn suspend_connections(&mut self) {
        // An unavailable profile can prevent restoration. Preserve the saved
        // owner and readings instead of replacing them with an empty route.
        if self
            .connections_state
            .get(&self.server)
            .is_some_and(|state| state.cached_reads.is_some())
        {
            return;
        }
        if self.page == Page::Ports
            && let Some(server) = self.selected_server()
        {
            let scope = terminal_scope::connection(server, &self.snapshot);
            let reads = self.resources.take_cached(&["page", "connection-projects"]);
            self.connections_state
                .entry(self.server.clone())
                .or_default()
                .cached_reads = Some((scope, reads));
        }
    }

    pub(super) fn restore_connections(&mut self) {
        if self.page != Page::Ports || !self.configuration_available() {
            return;
        }
        let scope = self
            .selected_server()
            .map(|server| terminal_scope::connection(server, &self.snapshot));
        if let Some((saved, reads)) = self
            .connections_state
            .get_mut(&self.server)
            .and_then(|state| state.cached_reads.take())
            && scope.as_ref() == Some(&saved)
        {
            self.resources.restore(reads);
        }
    }

    pub(super) fn discover_connections(&mut self, manual: bool) {
        if self.page == Page::Ports && !self.server.is_empty() && self.configuration_available() {
            if self.resources.get("page").is_some_and(|row| row.pending) {
                if manual {
                    self.connections_state
                        .entry(self.server.clone())
                        .or_default()
                        .refresh_requested = true;
                }
                return;
            }
            let manual = manual
                | std::mem::take(
                    &mut self
                        .connections_state
                        .entry(self.server.clone())
                        .or_default()
                        .refresh_requested,
                );
            let generation = self.resources.begin_refresh("page", manual);
            self.dispatch_request(
                "page",
                "connections_query",
                json!({"id":self.server,"refresh":manual}),
                generation,
            );
        }
    }

    pub(super) fn sync_connections(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page != Page::Ports || self.server.is_empty() {
            return;
        }
        let server = self.server.clone();
        let state = self
            .connections_state
            .entry(self.server.clone())
            .or_default();
        if state.input.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find a connection"));
            state.subscription = Some(cx.subscribe(&input, move |this, _, event, cx| {
                if matches!(event, InputEvent::Change)
                    && this.page == Page::Ports
                    && this.server == server
                    && this.window_visible
                {
                    cx.notify();
                }
            }));
            state.input = Some(input);
        }
    }

    pub(super) fn ports(&self, cx: &mut Context<Self>) -> AnyElement {
        self.grouped_ports(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::entry_matches;
    use newport_domain::port_grouping::group;
    use serde_json::json;

    #[test]
    fn search_keeps_secondary_addresses_kinds_saved_names_and_curated_paths() {
        let port = json!({"port":8080,"pid":9,"address":"127.0.0.1",
            "executable":"/usr/bin/node","arguments":["node","/srv/app/node_modules/vite/bin/vite.js"],
            "processName":"MainThread","workingDirectory":"/srv/app/packages/web",
            "projectRoot":"/srv/app/packages/web"});
        let mut secondary = port.clone();
        secondary["address"] = json!("2001:db8::7");
        let projects = group(
            vec![port, secondary],
            &[],
            "s",
            &[json!({"name":"Customer Console","path":{"display":"/srv/app"}})],
        );
        let p = &projects[0];
        for query in [
            "customer console",
            "2001:db8::7",
            "vite node",
            "/srv/app/packages/web",
            "mainthread",
        ] {
            assert!(entry_matches(p, &p.entries[0], query), "{query}");
        }
        assert!(!entry_matches(p, &p.entries[0], "postgres"));
    }

    #[test]
    fn saved_only_search_and_sort_use_the_same_named_project_context() {
        let projects = group(
            vec![],
            &[
                json!({"id":"t","serverId":"s","name":"Reports", "localPort":9100,"remotePort":8080,"remoteHost":"private.internal","project":"/srv/app"}),
            ],
            "s",
            &[json!({"name":"Customer Console","path":"/srv/app"})],
        );
        let p = &projects[0];
        for query in ["customer console", "reports 9100", "private.internal"] {
            assert!(entry_matches(p, &p.entries[0], query));
        }
        assert!(!entry_matches(p, &p.entries[0], "docker"));
    }
}
