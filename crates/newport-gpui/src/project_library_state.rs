//! Per-server library presentation and suspended query resources.
use super::*;
use newport_client::resources::ResourceGroup;

/// An open checkout set aside while another page shows: its workspace,
/// selection and settled reads, its repository still open, so returning
/// draws it at once and then refreshes it. Dropping it releases the
/// repository.
pub(super) struct Checkout {
    selection: desktop::preferences::Project,
    git: git::Workspace,
    reads: Vec<ResourceGroup>,
}
impl Drop for Checkout {
    fn drop(&mut self) {
        self.git.release();
    }
}

#[derive(Default)]
pub(super) struct State {
    connection: Option<configuration::ConnectionScope>,
    /// This server's saved projects; read them through
    /// [`Newport::library_projects`], which checks the `projects` read still
    /// shows them.
    projects: Option<desktop::preferences::Projects>,
    reads: Option<(ResourceGroup, ResourceGroup)>,
    checkout: Option<Checkout>,
    reload: bool,
    pub(super) input: Option<Entity<InputState>>,
    subscription: Option<Subscription>,
    pub(super) list: project_library_rows::LibraryList,
}

impl State {
    /// The rows the library shows for its `projects` read.
    pub(super) fn prepare_list(
        &mut self,
        read: Option<&newport_client::resources::Resource>,
        preferences: &project_preferences::Preferences,
        query: &str,
        width: f32,
    ) -> std::rc::Rc<Vec<desktop::preferences::Project>> {
        let projects = self
            .projects
            .as_ref()
            .filter(|_| read.is_some_and(newport_client::resources::Resource::has_value))
            .map_or(&[][..], |projects| projects.projects.as_slice());
        let revision = read.map_or(0, |read| read.data_revision);
        self.list
            .prepare(revision, projects, preferences, query, width)
    }

    /// Forgets the checkout set aside, releasing its repository.
    pub(super) fn drop_checkout(&mut self) {
        self.checkout = None;
    }
}

/// The saved projects a project list read replied with.
pub(super) fn projects_reply(
    reply: desktop::Reply,
) -> Result<desktop::preferences::Projects, newport_client::request_error::RequestError> {
    match reply {
        desktop::Reply::Preferences(reply) => match *reply {
            desktop::preferences::Reply::ProjectsList(projects) => Ok(*projects),
            _ => Err("The backend answered a project list with another reply.".into()),
        },
        _ => Err("The backend answered a project list with another reply.".into()),
    }
}

impl Newport {
    pub(super) fn project_library_state(&mut self) -> &mut State {
        if !self.project_libraries.contains_key(&self.server) {
            let connection = self.selected_connection();
            self.project_libraries.insert(
                self.server.clone(),
                State {
                    connection,
                    ..Default::default()
                },
            );
        }
        self.project_libraries.get_mut(&self.server).unwrap()
    }

    pub(super) fn project_query(&self, cx: &App) -> String {
        self.project_libraries
            .get(&self.server)
            .and_then(|state| state.input.as_ref())
            .map(|input| input.read(cx).value().to_lowercase())
            .unwrap_or_default()
    }

    /// The selected server's saved projects, while its `projects` read shows
    /// them.
    pub(super) fn library_projects(&self) -> Option<&[desktop::preferences::Project]> {
        let state = self.project_libraries.get(&self.server)?;
        self.owned("projects", &state.projects)
            .map(|projects| projects.projects.as_slice())
    }

    /// Settles a read of the selected server's saved projects.
    pub(super) fn complete_projects(
        &mut self,
        generation: u64,
        result: Result<desktop::Reply, newport_client::request_error::RequestError>,
    ) -> bool {
        let result = result.and_then(projects_reply).map(|projects| {
            let changed = self.project_library_state().projects.as_ref() != Some(&projects);
            (projects, changed)
        });
        self.complete_owned("projects", generation, result, |this| {
            &mut this.project_library_state().projects
        })
    }

    /// Settles a bookmark rename or removal.
    pub(super) fn complete_project_edit(
        &mut self,
        generation: u64,
        result: Result<desktop::Reply, newport_client::request_error::RequestError>,
    ) -> bool {
        let result = result.and_then(|reply| match reply {
            desktop::Reply::Preferences(reply)
                if matches!(
                    *reply,
                    desktop::preferences::Reply::ProjectsSave(_)
                        | desktop::preferences::Reply::ProjectsRemove(_)
                ) =>
            {
                Ok(true)
            }
            _ => Err("The backend answered a project change with another reply.".into()),
        });
        self.resources
            .complete_owned("project-edit", generation, result)
    }

    /// Settles a request that added a project, with the bookmark it saved
    /// while that request was still current.
    pub(super) fn complete_added_project(
        &mut self,
        key: &str,
        generation: u64,
        result: Result<desktop::preferences::Project, newport_client::request_error::RequestError>,
    ) -> (bool, Option<desktop::preferences::Project>) {
        let (project, result) = match result {
            Ok(project) => (Some(project), Ok(true)),
            Err(error) => (None, Err(error)),
        };
        let accepted = self.resources.complete_owned(key, generation, result);
        (accepted, project.filter(|_| accepted))
    }

    pub(super) fn visible_project_rows(&self) -> &[desktop::preferences::Project] {
        self.project_libraries
            .get(&self.server)
            .map(|state| state.list.visible_projects())
            .unwrap_or_default()
    }

    pub(super) fn sync_project_library(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let server = self.server.clone();
        let state = self.project_library_state();
        if state.input.is_some() {
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find a project"));
        state.subscription = Some(cx.subscribe(&input, move |this, _, event, cx| {
            if !matches!(event, gpui_kit::component::input::InputEvent::Change)
                || this.server != server
                || this.page != Page::Projects
                || this.selection.is_some()
            {
                return;
            }
            if let Some(state) = this.project_libraries.get_mut(&server) {
                state.list = Default::default();
            }
            this.project_checks = None;
            cx.notify();
        }));
        state.input = Some(input);
    }

    pub(super) fn prune_project_libraries(&mut self) {
        if !self.configuration_available() {
            return;
        }
        let connections: std::collections::HashMap<String, Option<configuration::ConnectionScope>> =
            self.servers()
                .iter()
                .map(|server| (server.id.to_string(), self.connection(server)))
                .collect();
        self.project_libraries.retain(|id, state| {
            let Some(connection) = connections.get(id).cloned() else {
                return false;
            };
            if state.connection != connection {
                state.connection = connection;
                state.projects = None;
                state.reads = None;
                state.checkout = None;
                state.reload = false;
                state.list = Default::default();
            }
            true
        });
    }

    pub(super) fn suspend_project_library(&mut self) {
        if self.page != Page::Projects {
            return;
        }
        // An outage may prevent restoration altogether. Do not overwrite that
        // retained cache with the unavailable route's empty resources.
        if self.project_library_state().reads.is_some() {
            return;
        }
        let reads = self.resources.take_cached(&["projects"]);
        let statuses = self.resources.take_cached_prefix("project-status:");
        self.project_library_state().reads = Some((reads, statuses));
        self.project_checks = None;
    }

    /// Sets the open checkout aside when leaving Projects.
    pub(super) fn suspend_checkout(&mut self) {
        if self.page != Page::Projects || !self.git.settled() {
            return;
        }
        let Some(selection) = self.selection.take() else {
            return;
        };
        let checkout = Checkout {
            selection,
            git: std::mem::take(&mut self.git),
            reads: vec![
                self.resources.take_cached(&["repository"]),
                self.resources.take_cached_prefix("git-"),
            ],
        };
        self.project_library_state().checkout = Some(checkout);
    }

    /// Puts the checkout set aside back, if this server has one. Its reads
    /// refresh in place afterwards; nothing waits on the server first.
    pub(super) fn resume_checkout(&mut self) -> bool {
        if self.page != Page::Projects || !self.configuration_available() {
            return false;
        }
        self.restore_project_library();
        let Some(mut checkout) = self.project_library_state().checkout.take() else {
            return false;
        };
        self.selection = Some(checkout.selection.clone());
        self.git = std::mem::take(&mut checkout.git);
        for reads in std::mem::take(&mut checkout.reads) {
            self.resources.restore(reads);
        }
        self.restore_git = None;
        true
    }

    pub(super) fn restore_project_library(&mut self) {
        if self.page != Page::Projects || !self.configuration_available() {
            return;
        }
        self.prune_project_libraries();
        if let Some((reads, statuses)) = self.project_library_state().reads.take() {
            self.project_library_state().reload |= reads.had_pending();
            self.resources.restore(reads);
            self.resources.restore(statuses);
        }
    }

    pub(super) fn refresh_project_library(&mut self, manual: bool) {
        if self.page != Page::Projects
            || self.selection.is_some()
            || !self.configuration_available()
        {
            return;
        }
        let reload = self.project_library_state().reload;
        // Bookmarks are local preferences. Keep a short freshness window while
        // visible, and coalesce an already pending read through Resources.
        let due = reload
            || self.resources.get("projects").is_none_or(|resource| {
                resource
                    .finished_at
                    .is_none_or(|at| at.elapsed() >= Duration::from_secs(30))
            });
        if manual || due {
            if manual {
                self.resources.invalidate_prefix("project-status:");
                self.project_checks = None;
            }
            let generation = self.resources.begin_refresh("projects", manual);
            self.project_library_state().reload = false;
            let operation = uuid::Uuid::parse_str(&self.server)
                .map_err(|error| {
                    newport_client::request_error::RequestError::from(error.to_string())
                })
                .map(|server| {
                    desktop::Operation::Preferences(Box::new(
                        desktop::preferences::Operation::ProjectsList(Box::new(
                            desktop::preferences::ProjectsListArgs { server },
                        )),
                    ))
                });
            self.dispatch_typed("projects", operation, generation);
        }
    }

    pub(super) fn poll_project_library(&mut self) {
        if self.page == Page::Projects
            && self.selection.is_none()
            && self.configuration_available()
            && !self.needle_missing()
            && self.resources.take_poll(
                "projects",
                Instant::now(),
                Duration::from_secs(30),
                self.window_visible,
            )
        {
            self.refresh_project_library(false);
        }
    }

    pub(super) fn refresh_navigation(&mut self) {
        if self.page == Page::Projects && self.selection.is_none() {
            self.restore_project_library();
            self.refresh_project_library(false);
            self.restore_git_destination();
        } else {
            self.refresh();
        }
    }

    pub(super) fn project_bookmarks_changed(&mut self, server: &str) {
        self.connection_projects_changed(server);
        if let Some(state) = self.project_libraries.get_mut(server) {
            state.reload = true;
        }
        if self.server == server && self.page == Page::Projects {
            // A read started before a bookmark write must not satisfy the
            // refresh acknowledging that write. Keep its old display value.
            let reads = self.resources.take_cached(&["projects"]);
            self.resources.restore(reads);
        }
    }
}

#[cfg(test)]
pub(crate) mod typed_fixture {
    use crate::{Newport, Work};
    pub(crate) use desktop::Kind;
    use newport_protocol::{desktop, git};
    pub(crate) const TYPED_SERVER: &str = "00000000-0000-4000-8000-000000000001";
    pub(crate) const TYPED_SECOND: &str = "00000000-0000-4000-8000-000000000002";
    pub(crate) fn kind(work: &Work) -> Option<desktop::Kind> {
        match &work.request {
            crate::request_dispatch::Request::Discovery { .. } => {
                Some(Kind::ConnectionConnectionsQuery)
            }
            crate::request_dispatch::Request::Git(_) => Some(Kind::GitRequest),
            request => request.operation().map(desktop::Operation::kind),
        }
    }
    pub(crate) fn operation(work: &Work) -> &desktop::Operation {
        work.request.operation().expect("Expected desktop request")
    }
    pub(crate) fn git_request(work: &Work) -> &git::Request {
        if let crate::request_dispatch::Request::Git(args) = &work.request {
            return &args.request;
        }
        let desktop::Operation::Git(operation) = operation(work) else {
            panic!("Expected Git request");
        };
        let desktop::git::Operation::Request(args) = operation.as_ref() else {
            panic!("Expected Git request");
        };
        &args.request
    }
    pub(crate) fn server(work: &Work) -> Option<uuid::Uuid> {
        match &work.request {
            crate::request_dispatch::Request::Discovery { server, .. } => Some(*server),
            crate::request_dispatch::Request::Git(args) => Some(args.server),
            _ => operation(work).server_id(),
        }
    }
    /// A saved project numbered `id` on `server`, at the UTF-8 `path`.
    pub(crate) fn bookmark(
        id: u128,
        server: &str,
        name: &str,
        path: &str,
    ) -> desktop::preferences::Project {
        desktop::preferences::Project {
            id: uuid::Uuid::from_u128(id),
            server: uuid::Uuid::parse_str(server).unwrap(),
            name: name.into(),
            path: git::Path::new(path.as_bytes()),
        }
    }
    /// The reply to a project list read of `projects`.
    pub(crate) fn projects_completion(
        key: &str,
        generation: u64,
        projects: Vec<desktop::preferences::Project>,
    ) -> crate::Completion {
        crate::Completion {
            server: TYPED_SERVER.into(),
            key: key.into(),
            generation,
            result: crate::Outcome::Typed(Ok(desktop::Reply::Preferences(Box::new(
                desktop::preferences::Reply::ProjectsList(Box::new(
                    desktop::preferences::Projects { projects },
                )),
            )))),
        }
    }
    /// Settles the `projects` read `generation` with `projects`.
    pub(crate) fn complete_projects(
        view: &mut Newport,
        generation: u64,
        projects: Vec<desktop::preferences::Project>,
    ) -> bool {
        view.complete_projects(
            generation,
            Ok(desktop::Reply::Preferences(Box::new(
                desktop::preferences::Reply::ProjectsList(Box::new(
                    desktop::preferences::Projects { projects },
                )),
            ))),
        )
    }
    /// A project list reply holding `projects`.
    pub(crate) fn projects_reply(projects: Vec<desktop::preferences::Project>) -> desktop::Reply {
        desktop::Reply::Preferences(Box::new(desktop::preferences::Reply::ProjectsList(
            Box::new(desktop::preferences::Projects { projects }),
        )))
    }
    /// Starts and settles a `projects` read with `projects`.
    pub(crate) fn put_projects(view: &mut Newport, projects: Vec<desktop::preferences::Project>) {
        let generation = view.resources.begin("projects").unwrap();
        assert!(complete_projects(view, generation, projects));
    }
    pub(crate) fn owner(
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Newport>,
    ) -> Newport {
        use crate::snapshot_fixture as fixture;
        let mut value = crate::page_perf_tests::owner(window, cx);
        assert_eq!(value.server, TYPED_SERVER);
        fixture::set(
            &mut value.snapshot_mut().runtime.connection_revisions,
            fixture::SECOND,
            1,
        );
        value.project_libraries.clear();
        value.sync_project_library(window, cx);
        value
    }
}
#[cfg(test)]
pub(crate) use typed_fixture::owner as typed_test_owner;

#[cfg(test)]
mod tests {
    use crate::project_library_state::typed_fixture::*;
    use crate::{Page, project_library};
    use gpui_kit::{ListOffset, TestAppContext, component::combobox::ComboboxEvent, px};
    use newport_protocol::desktop::preferences::Project;

    fn projects(server: &str) -> Vec<Project> {
        (0..100)
            .map(|id| {
                bookmark(
                    id,
                    server,
                    &format!("Project {id:03}"),
                    &format!("/work/{id}"),
                )
            })
            .collect()
    }
    fn shown(view: &crate::Newport) -> Option<Vec<Project>> {
        view.library_projects().map(<[_]>::to_vec)
    }

    #[test]
    fn the_list_keeps_its_rows_through_a_failed_refresh_and_empties_when_cleared() {
        use newport_client::resources::Resources;
        use std::rc::Rc;
        let preferences = crate::project_preferences::Preferences::default();
        let mut state = super::State {
            projects: Some(newport_protocol::desktop::preferences::Projects {
                projects: projects(TYPED_SERVER),
            }),
            ..Default::default()
        };
        let mut resources = Resources::default();
        let read = resources.begin("projects").unwrap();
        resources.complete_owned("projects", read, Ok(true));
        let shown = state.prepare_list(resources.get("projects"), &preferences, "", 900.);
        assert_eq!(shown.len(), 100);
        let retry = resources.begin_refresh("projects", false).unwrap();
        resources.complete_owned("projects", retry, Err("Offline".into()));
        assert!(Rc::ptr_eq(
            &shown,
            &state.prepare_list(resources.get("projects"), &preferences, "", 900.)
        ));
        resources.invalidate("projects");
        assert!(
            state
                .prepare_list(resources.get("projects"), &preferences, "", 900.)
                .is_empty()
        );
    }

    #[gpui_kit::test]
    fn returning_to_an_open_checkout_draws_it_at_once_then_refreshes_it(cx: &mut TestAppContext) {
        use gpui_kit::AppContext;
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
        });
        let mut owner = None;
        let (_, visual) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| crate::project_library_state::typed_test_owner(window, cx));
            owner = Some(view.clone());
            crate::page_perf_tests::PageHost(view)
        });
        let view = owner.unwrap();
        view.update(visual, |o, cx| {
            let (work, requests) = std::sync::mpsc::sync_channel(64);
            o.work = work.into();
            o.page = Page::Projects;
            let project = bookmark(1, TYPED_SERVER, "Repo", "/repo");
            let repo = crate::git_fixture::read_only_repository("open");
            o.selection = Some(project.clone());
            o.git.repo = Some(repo.clone());
            let generation = o.resources.begin("repository").unwrap();
            o.resources.complete_git(
                "repository",
                generation,
                Ok(newport_protocol::git::Reply::Open(Box::new(repo))),
            );
            let generation = o.resources.begin("git-list").unwrap();
            let seen = crate::git_listing::Listing::Status(Box::new(
                crate::git_fixture::status_page("seen", vec![], None),
            ))
            .into_reply();
            o.resources
                .complete_git("git-list", generation, Ok(seen.clone()));
            requests.try_iter().for_each(drop);

            o.navigate(Page::Terminal, cx);
            assert!(o.selection.is_none() && o.resources.get("git-list").is_none());
            o.navigate(Page::Projects, cx);
            // Everything is there before any reply: no skeleton, no reopening.
            assert_eq!(o.selection, Some(project));
            assert_eq!(o.git.repo_id(), Some("open"));
            assert!(o.resources.get("repository").is_some_and(|r| !r.pending));
            assert_eq!(o.git_reply("git-list"), Some(&seen));
            let sent: Vec<_> = requests.try_iter().collect();
            assert!(
                sent.iter()
                    .any(|r| kind(r) == Some(Kind::GitRefreshRepository)),
                "refreshed"
            );
            assert!(
                !sent.iter().any(|r| kind(r) == Some(Kind::GitRequest)
                    && matches!(git_request(r), newport_protocol::git::Request::Open { .. })),
                "not reopened"
            );
            // A changed connection forgets what was set aside.
            o.navigate(Page::Terminal, cx);
            o.snapshot_mut().config.servers[0].ssh_port = 2222;
            o.navigate(Page::Projects, cx);
            assert!(o.selection.is_none());
        });
    }

    #[gpui_kit::test]
    fn library_navigation_and_server_picker_reuse_scoped_queries_inputs_and_scroll(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::{AppContext, VisualTestContext, base::test_support::snapshots, size};
        // Complete layout/paint and deferred work after every user action. A
        // sequence of owner updates alone cannot verify the rendered route.
        fn rendered(visual: &mut VisualTestContext) -> Vec<String> {
            visual.run_until_parked();
            visual.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
            visual.run_until_parked();
            visual.update(|window, cx| {
                window.draw(cx).clear(cx);
                let mut labels: Vec<_> = snapshots(window)
                    .iter()
                    .filter_map(|node| node.label().map(str::to_owned))
                    .collect();
                // Test-support nodes are keyed by identity, not paint order.
                labels.sort();
                labels
            })
        }
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
        });
        let mut owner = None;
        let (_, visual) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| crate::project_library_state::typed_test_owner(window, cx));
            owner = Some(view.clone());
            crate::page_perf_tests::PageHost(view)
        });
        let view = owner.unwrap();
        visual.simulate_resize(size(px(1000.), px(600.)));
        let (work, requests) = std::sync::mpsc::sync_channel(64);
        let data = projects(TYPED_SERVER);
        view.update(visual, |view, cx| {
            view.work = work.into();
            view.page = Page::Projects;
            let servers = &mut view.snapshot_mut().config.servers;
            let mut second = servers[0].clone();
            second.id = crate::snapshot_fixture::SECOND;
            servers.push(second);
            put_projects(view, data.clone());
            cx.notify();
        });
        let labels = rendered(visual);
        assert!(
            labels
                .iter()
                .any(|label| label.starts_with("Open Project 000"))
        );
        let first_input = view.read_with(visual, |view, _| {
            view.project_libraries[TYPED_SERVER]
                .input
                .as_ref()
                .unwrap()
                .clone()
        });
        visual.update(|window, cx| {
            first_input.update(cx, |input, cx| input.replace_all("Project 0", window, cx))
        });
        rendered(visual);
        view.update(visual, |view, cx| {
            view.project_library_state()
                .list
                .state
                .scroll_to(ListOffset {
                    item_ix: 25,
                    offset_in_item: px(7.),
                });
            cx.notify();
        });
        let before = rendered(visual);
        assert!(
            before
                .iter()
                .any(|label| label.starts_with("Open Project 025"))
        );
        let before_rows: Vec<_> = before
            .iter()
            .filter(|label| label.starts_with("Open Project "))
            .cloned()
            .collect();
        assert!(
            before_rows.len() < 20,
            "the rendered viewport must remain bounded"
        );
        view.update(visual, |view, cx| view.navigate(Page::Integration, cx));
        let away = rendered(visual);
        assert!(!away.iter().any(|label| label.starts_with("Open Project ")));
        view.update(visual, |view, cx| view.navigate(Page::Projects, cx));
        let returned = rendered(visual);
        assert_eq!(
            returned
                .into_iter()
                .filter(|label| label.starts_with("Open Project "))
                .collect::<Vec<_>>(),
            before_rows
        );
        assert!(
            requests
                .try_iter()
                .all(|work| kind(&work) != Some(Kind::PreferencesProjectsList)),
            "fresh navigation must not enqueue another library read"
        );
        let picker = view.read_with(visual, |view, _| view.server_picker.clone());
        picker.update(visual, |_, cx| {
            cx.emit(ComboboxEvent::Activate(vec![TYPED_SECOND.into()]))
        });
        let loading = rendered(visual);
        assert!(
            !loading
                .iter()
                .any(|label| label.starts_with("Open Project ")),
            "the second server must not render the first server's rows"
        );
        view.read_with(visual, |view, cx| {
            assert_eq!(view.server, TYPED_SECOND);
            assert_eq!(view.project_query(cx), "");
            assert!(view.library_projects().is_none());
        });
        let work = requests
            .try_iter()
            .find(|work| kind(work) == Some(Kind::PreferencesProjectsList))
            .unwrap();
        assert_eq!(
            server(&work),
            Some(uuid::Uuid::parse_str(TYPED_SECOND).unwrap())
        );
        view.update(visual, |view, cx| {
            assert!(complete_projects(
                view,
                work.generation,
                projects(TYPED_SECOND)
            ));
            cx.notify();
        });
        assert!(
            rendered(visual)
                .iter()
                .any(|label| label.starts_with("Open Project 000"))
        );
        let second_input = view.read_with(visual, |view, _| {
            view.project_libraries[TYPED_SECOND]
                .input
                .as_ref()
                .unwrap()
                .clone()
        });
        visual.update(|window, cx| {
            second_input.update(cx, |input, cx| input.replace_all("Project", window, cx))
        });
        rendered(visual);
        view.update(visual, |view, cx| {
            view.project_library_state()
                .list
                .state
                .scroll_to(ListOffset {
                    item_ix: 40,
                    offset_in_item: px(0.),
                });
            cx.notify();
        });
        assert!(
            rendered(visual)
                .iter()
                .any(|label| label.starts_with("Open Project 040"))
        );
        picker.update(visual, |_, cx| {
            cx.emit(ComboboxEvent::Activate(vec![TYPED_SERVER.into()]))
        });
        let returned = rendered(visual);
        assert_eq!(
            returned
                .into_iter()
                .filter(|label| label.starts_with("Open Project "))
                .collect::<Vec<_>>(),
            before_rows
        );
        view.read_with(visual, |view, cx| {
            assert_eq!(shown(view), Some(data.clone()));
            assert_eq!(view.project_query(cx), "project 0");
            assert_eq!(
                view.project_libraries[TYPED_SERVER].input.as_ref().unwrap(),
                &first_input
            );
            assert_eq!(
                view.project_libraries[TYPED_SERVER]
                    .list
                    .state
                    .logical_scroll_top()
                    .item_ix,
                25
            );
            assert_eq!(
                view.project_libraries[TYPED_SECOND]
                    .list
                    .state
                    .logical_scroll_top()
                    .item_ix,
                40
            );
            assert_eq!(second_input.read(cx).value().as_ref(), "Project");
        });
        assert!(
            requests
                .try_iter()
                .all(|work| kind(&work) != Some(Kind::PreferencesProjectsList))
        );
    }

    #[gpui_kit::test]
    fn library_suspension_fences_pending_reads_and_reloads_without_losing_cached_rows(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (view, visual) = cx.add_window_view(crate::project_library_state::typed_test_owner);
        view.update(visual, |view, cx| {
            let (work, requests) = std::sync::mpsc::sync_channel(64);
            view.work = work.into();
            view.page = Page::Projects;
            let data = projects(TYPED_SERVER);
            put_projects(view, data.clone());
            let key = project_library::status_key(&data[0]);
            let generation = view.resources.begin(&key).unwrap();
            let mut summary = crate::git_fixture::status_summary();
            summary.total_entries = 2;
            let summary = newport_protocol::git::Reply::StatusSummary(Box::new(summary));
            view.resources
                .complete_git(&key, generation, Ok(summary.clone()));
            let pending = view.resources.begin_refresh("projects", false).unwrap();
            let pending_status = view.resources.begin_refresh(&key, false).unwrap();
            let initial_key = project_library::status_key(&data[1]);
            let pending_initial = view.resources.begin(&initial_key).unwrap();
            view.navigate(Page::Integration, cx);
            view.navigate(Page::Projects, cx);
            assert_eq!(shown(view), Some(data.clone()));
            assert_eq!(view.git_reply(&key), Some(&summary));
            assert!(!complete_projects(view, pending, Vec::new()));
            assert!(!view.resources.complete_git(
                &key,
                pending_status,
                Ok(newport_protocol::git::Reply::StatusSummary(Box::new(
                    crate::git_fixture::status_summary()
                )))
            ));
            assert!(!view.resources.complete_git(
                &initial_key,
                pending_initial,
                Ok(newport_protocol::git::Reply::StatusSummary(Box::new(
                    crate::git_fixture::status_summary()
                )))
            ));
            let reads: Vec<_> = requests
                .try_iter()
                .filter(|work| kind(work) == Some(Kind::PreferencesProjectsList))
                .collect();
            assert_eq!(
                reads.len(),
                1,
                "a detached refresh must be reissued exactly once"
            );
            assert!(complete_projects(view, reads[0].generation, data.clone()));
            view.project_library_state()
                .list
                .sync(&[data[1].clone()], 900.);
            view.window_visible = true;
            view.poll_project_checks(cx);
            let retry = requests.try_recv().unwrap();
            assert_eq!(
                retry.key, initial_key,
                "a detached first status read must remain retryable"
            );
            assert_eq!(view.git_reply(&key), Some(&summary));
        });
    }

    #[gpui_kit::test]
    fn library_cache_survives_outage_but_rejects_replaced_or_deleted_server(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (view, visual) = cx.add_window_view(crate::project_library_state::typed_test_owner);
        view.update(visual, |view, _| {
            view.page = Page::Projects;
            let data = projects(TYPED_SERVER);
            put_projects(view, data.clone());
            view.suspend_project_library();
            let original = view.snapshot.clone();
            view.snapshot_mut().load_error = Some("Unavailable".into());
            view.snapshot_mut().config.servers.clear();
            view.prune_project_libraries();
            assert!(view.project_libraries.contains_key(TYPED_SERVER));
            view.restore_project_library();
            assert!(
                view.library_projects().is_none(),
                "cannot restore before validating the connection"
            );
            view.snapshot = original;
            view.restore_project_library();
            assert_eq!(shown(view), Some(data));
            view.suspend_project_library();
            crate::snapshot_fixture::set(
                &mut view.snapshot_mut().runtime.connection_revisions,
                crate::snapshot_fixture::SERVER,
                2,
            );
            view.restore_project_library();
            assert!(
                view.library_projects().is_none(),
                "old connection data cannot enter a replacement"
            );
            view.snapshot_mut().config.servers.clear();
            view.prune_project_libraries();
            assert!(!view.project_libraries.contains_key(TYPED_SERVER));
        });
    }

    #[gpui_kit::test]
    fn library_navigation_during_outage_and_bookmark_mutations_revalidate_cached_data(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::{AppContext, IntoElement, Render};
        struct IdleHost;
        impl Render for IdleHost {
            fn render(
                &mut self,
                _: &mut gpui_kit::Window,
                _: &mut gpui_kit::Context<Self>,
            ) -> impl IntoElement {
                gpui_kit::div()
            }
        }
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
        });
        let mut owner = None;
        let (_, visual) = cx.add_window_view(|window, cx| {
            owner = Some(cx.new(|cx| crate::project_library_state::typed_test_owner(window, cx)));
            IdleHost
        });
        let view = owner.unwrap();
        view.update(visual, |view, cx| {
            let (work, requests) = std::sync::mpsc::sync_channel(64);
            view.work = work.into();
            view.page = Page::Projects;
            let data = projects(TYPED_SERVER);
            put_projects(view, data.clone());
            view.navigate(Page::Integration, cx);
            let original = view.snapshot.clone();
            view.snapshot_mut().load_error = Some("Unavailable".into());
            view.snapshot_mut().config.servers.clear();
            view.navigate(Page::Projects, cx);
            view.navigate(Page::Integration, cx);
            view.snapshot = original;
            view.navigate(Page::Projects, cx);
            assert_eq!(shown(view), Some(data.clone()));
            assert!(
                requests
                    .try_iter()
                    .all(|work| kind(&work) != Some(Kind::PreferencesProjectsList))
            );
            view.navigate(Page::Integration, cx);
            let connection = view.project_libraries[TYPED_SERVER].connection.clone();
            view.file_project_completed(TYPED_SERVER, connection, Ok(data[0].clone()));
            view.navigate(Page::Projects, cx);
            assert_eq!(
                shown(view),
                Some(data.clone()),
                "a saved bookmark refresh retains the previous list"
            );
            let read = requests
                .try_iter()
                .find(|work| kind(work) == Some(Kind::PreferencesProjectsList))
                .unwrap();
            assert!(view.complete_projects(read.generation, Err("Read failed".into())));
            view.refresh_project_library(false);
            view.poll_project_library();
            assert!(
                requests
                    .try_iter()
                    .all(|work| kind(&work) != Some(Kind::PreferencesProjectsList)),
                "failed automatic reads must respect their retry interval"
            );
            view.refresh_project_library(true);
            let old = requests.try_recv().unwrap();
            view.project_bookmarks_changed(TYPED_SERVER);
            view.refresh_project_library(true);
            let fresh = requests.try_recv().unwrap();
            assert!(fresh.generation > old.generation);
            assert!(!complete_projects(view, old.generation, data.clone()));
            assert!(complete_projects(view, fresh.generation, Vec::new()));
            assert_eq!(shown(view), Some(Vec::new()));
        });
    }
}
