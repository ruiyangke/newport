//! Per-server presentation state shared by the Projects and Ports views.
use super::*;
use gpui_kit::component::input::InputEvent;
pub(super) mod diagnostics;
mod discovery;
#[cfg(test)]
pub(crate) mod fixtures;
mod forwarding;

#[derive(Clone, Debug)]
pub(super) struct DiscoveryCompletion {
    pub(super) scope: configuration::ConnectionScope,
    pub(super) reading: newport_client::discovery::Reading,
}

#[derive(Default)]
pub(super) struct State {
    scope: Option<configuration::ConnectionScope>,
    pub(super) reading: Option<newport_client::discovery::Reading>,
    pub(super) snapshot_revision: u64,
    cached_reads: Option<(
        configuration::ConnectionScope,
        newport_client::resources::ResourceGroup,
    )>,
    /// The reading shown came from the cache, not from a scan since.
    pub(super) restored: bool,
    refresh_requested: bool,
    projects_reload: bool,
    /// The server's saved projects, which name the ports' projects; read them
    /// through [`Newport::connection_bookmarks`].
    bookmarks: Option<desktop::preferences::Projects>,
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

impl State {
    fn set_scope(&mut self, scope: configuration::ConnectionScope) -> bool {
        if self.scope.as_ref() == Some(&scope) {
            return false;
        }
        self.scope = Some(scope);
        self.reading = None;
        self.cached_reads = None;
        self.refresh_requested = false;
        self.projects_reload = true;
        self.bookmarks = None;
        self.worktree_associations = Default::default();
        self.projects = Default::default();
        self.visible = Default::default();
        self.prepared = Default::default();
        self.viewport = Default::default();
        true
    }
}

fn forwarding_snapshot_changed(
    before: Option<&configuration::Snapshot>,
    after: &configuration::Snapshot,
    server: &str,
) -> bool {
    let Ok(server) = uuid::Uuid::parse_str(server) else {
        return false;
    };
    let Some(before) = before else {
        return after.config.tunnels_of(server).next().is_some();
    };
    let mut new = after.config.tunnels_of(server);
    !before.config.tunnels_of(server).eq(new.clone())
        || new.any(|tunnel| {
            before.runtime.tunnel(tunnel.id) != after.runtime.tunnel(tunnel.id)
                || before.runtime.tunnel_health(tunnel.id) != after.runtime.tunnel_health(tunnel.id)
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
    parts.push(entry.port.search_text());
    parts.extend(
        entry
            .forwards
            .iter()
            .map(|t| format!("{} {}", t.name, t.local_port)),
    );
    parts.join(" ").to_lowercase().contains(query)
}

impl Newport {
    pub(super) fn sync_connections_snapshot(&mut self, next: &configuration::Snapshot) {
        // A snapshot accepted on another page must invalidate its server's
        // retained projection, while unrelated server or feature updates
        // leave it reusable. Do not use the whole snapshot's data revision.
        for (server, state) in &mut self.connections_state {
            if forwarding_snapshot_changed(self.snapshot.as_ref(), next, server) {
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
            || self.needle_missing()
            || !self
                .resources
                .take_poll("ports", now, Duration::from_secs(1), self.window_visible)
        {
            return false;
        }
        let before = self
            .resources
            .get("ports")
            .map(|r| (r.manual_pending, r.error.clone(), r.presentation_revision));
        self.discover_connections(false);
        let after = self
            .resources
            .get("ports")
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

    /// The selected server's saved projects, while their read shows them.
    pub(super) fn connection_bookmarks(&self) -> &[desktop::preferences::Project] {
        self.connections_state
            .get(&self.server)
            .and_then(|state| self.owned("connection-projects", &state.bookmarks))
            .map_or(&[], |projects| projects.projects.as_slice())
    }

    /// Settles a read of the saved projects that name the ports' projects.
    pub(super) fn complete_connection_projects(
        &mut self,
        generation: u64,
        result: Result<desktop::Reply, newport_client::request_error::RequestError>,
    ) -> bool {
        let result = result
            .and_then(project_library_state::projects_reply)
            .map(|projects| {
                let changed = self
                    .connections_state
                    .get(&self.server)
                    .and_then(|state| state.bookmarks.as_ref())
                    != Some(&projects);
                (projects, changed)
            });
        self.complete_owned("connection-projects", generation, result, |this| {
            &mut this
                .connections_state
                .entry(this.server.clone())
                .or_default()
                .bookmarks
        })
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
        self.dispatch_typed(
            "connection-projects",
            desktop_server(&self.server).map(|server| {
                desktop::Operation::Preferences(Box::new(
                    desktop::preferences::Operation::ProjectsList(Box::new(
                        desktop::preferences::ProjectsListArgs { server },
                    )),
                ))
            }),
            generation,
        );
    }

    pub(super) fn prune_connections(&mut self) {
        if !self.configuration_available() {
            return;
        }
        let scopes: std::collections::HashMap<String, configuration::ConnectionScope> = self
            .servers()
            .iter()
            .filter_map(|server| Some((server.id.to_string(), self.connection(server)?)))
            .collect();
        let mut invalidate_active = false;
        self.connections_state.retain(|id, state| {
            let Some(scope) = scopes.get(id) else {
                return false;
            };
            let scope = scope.clone();
            if state.set_scope(scope) && id == &self.server {
                invalidate_active = true;
            }
            true
        });
        if invalidate_active && self.page == Page::Ports {
            self.resources.invalidate("ports");
            self.resources.invalidate("connection-projects");
        }
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
            && let Some(scope) = self.selected_connection()
        {
            let state = self
                .connections_state
                .entry(self.server.clone())
                .or_default();
            if state.set_scope(scope.clone()) {
                self.resources.invalidate("ports");
                self.resources.invalidate("connection-projects");
            }
            let reads = self
                .resources
                .take_cached(&["ports", "connection-projects"]);
            state.cached_reads = Some((scope, reads));
        }
    }

    pub(super) fn restore_connections(&mut self) {
        if self.page != Page::Ports || !self.configuration_available() {
            return;
        }
        let scope = self.selected_connection();
        if let Some((saved, reads)) = self
            .connections_state
            .get_mut(&self.server)
            .and_then(|state| state.cached_reads.take())
            && scope.as_ref() == Some(&saved)
        {
            self.resources.restore(reads);
            if let Some(state) = self.connections_state.get_mut(&self.server) {
                state.restored = true;
            }
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
    /// The saved project that names `/srv/app`.
    fn console() -> newport_protocol::desktop::preferences::Project {
        crate::project_library_state::typed_fixture::bookmark(
            1,
            crate::project_library_state::typed_fixture::TYPED_SERVER,
            "Customer Console",
            "/srv/app",
        )
    }

    #[test]
    fn search_keeps_secondary_addresses_kinds_saved_names_and_curated_paths() {
        let port = super::fixtures::Listener::new(8080)
            .pid(9)
            .process(
                "MainThread",
                "/usr/bin/node",
                &["node", "/srv/app/node_modules/vite/bin/vite.js"],
                Some("/srv/app/packages/web"),
            )
            .project("/srv/app/packages/web")
            .member();
        let mut secondary = port.clone();
        secondary.endpoint.address = "2001:db8::7".parse().unwrap();
        let projects = group(
            vec![port, secondary],
            &[],
            crate::snapshot_fixture::SERVER,
            &[console()],
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
        let reports = newport_protocol::desktop::configuration::Tunnel {
            name: "Reports".into(),
            remote_host: "private.internal".into(),
            project: Some("/srv/app".into()),
            ..crate::snapshot_fixture::tunnel(
                uuid::Uuid::from_u128(1),
                crate::snapshot_fixture::SERVER,
                9100,
                8080,
            )
        };
        let projects = group(
            vec![],
            &[reports],
            crate::snapshot_fixture::SERVER,
            &[console()],
        );
        let p = &projects[0];
        for query in ["customer console", "reports 9100", "private.internal"] {
            assert!(entry_matches(p, &p.entries[0], query));
        }
        assert!(!entry_matches(p, &p.entries[0], "docker"));
    }
}
