//! Per-server library presentation and suspended query resources.
use super::*;
use newport_client::resources::ResourceGroup;

#[derive(Default)]
pub(super) struct State {
    connection: Value,
    reads: Option<(ResourceGroup, ResourceGroup)>,
    reload: bool,
    pub(super) input: Option<Entity<InputState>>,
    subscription: Option<Subscription>,
    pub(super) list: project_library_rows::LibraryList,
}

impl Newport {
    pub(super) fn project_library_state(&mut self) -> &mut State {
        if !self.project_libraries.contains_key(&self.server) {
            let connection = self
                .selected_server()
                .map(|server| terminal_scope::connection(server, &self.snapshot))
                .unwrap_or_default();
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

    pub(super) fn visible_project_rows(&self) -> &[Value] {
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
        let servers = self.snapshot["config"]["servers"].as_array();
        self.project_libraries.retain(|id, state| {
            let Some(server) =
                servers.and_then(|rows| rows.iter().find(|server| server["id"] == *id))
            else {
                return false;
            };
            let connection = terminal_scope::connection(server, &self.snapshot);
            if state.connection != connection {
                state.connection = connection;
                state.reads = None;
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
        let reads = self.resources.take_cached(&["page"]);
        let statuses = self.resources.take_cached_prefix("project-status:");
        self.project_library_state().reads = Some((reads, statuses));
        self.project_checks = None;
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
            || self.resources.get("page").is_none_or(|resource| {
                resource
                    .finished_at
                    .is_none_or(|at| at.elapsed() >= Duration::from_secs(30))
            });
        if manual || due {
            if manual {
                self.resources.invalidate_prefix("project-status:");
                self.project_checks = None;
            }
            let generation = self.resources.begin_refresh("page", manual);
            self.project_library_state().reload = false;
            self.dispatch_request(
                "page",
                "git_projects_list",
                json!({"serverId":self.server}),
                generation,
            );
        }
    }

    pub(super) fn poll_project_library(&mut self) {
        if self.page == Page::Projects
            && self.selection.is_none()
            && self.configuration_available()
            && self.resources.take_poll(
                "page",
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
            let reads = self.resources.take_cached(&["page"]);
            self.resources.restore(reads);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Page, project_library};
    use gpui_kit::{ListOffset, TestAppContext, component::combobox::ComboboxEvent, px};
    use serde_json::{Value, json};

    fn projects(server: &str) -> Value {
        json!((0..100).map(|id| json!({
            "id":format!("{server}-{id}"), "serverId":server, "name":format!("Project {id:03}"),
            "path":{"display":format!("/work/{id}"),"bytesB64":format!("{server}-{id}")}
        })).collect::<Vec<_>>())
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
            let view = cx.new(|cx| crate::page_perf_tests::owner(window, cx));
            owner = Some(view.clone());
            crate::page_perf_tests::PageHost(view)
        });
        let view = owner.unwrap();
        visual.simulate_resize(size(px(1000.), px(600.)));
        let (work, requests) = std::sync::mpsc::sync_channel(64);
        let data = projects("fixture");
        view.update(visual, |view, cx| {
            view.work = work.into();
            view.page = Page::Projects;
            let mut second = view.snapshot["config"]["servers"][0].clone();
            second["id"] = json!("second");
            view.snapshot["config"]["servers"]
                .as_array_mut()
                .unwrap()
                .push(second);
            let generation = view.resources.begin("page").unwrap();
            view.resources
                .complete("page", generation, Ok(data.clone()));
            cx.notify();
        });
        let labels = rendered(visual);
        assert!(
            labels
                .iter()
                .any(|label| label.starts_with("Open Project 000"))
        );
        let first_input = view.read_with(visual, |view, _| {
            view.project_libraries["fixture"]
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
                .all(|work| work.method != "git_projects_list"),
            "fresh navigation must not enqueue another library read"
        );
        let picker = view.read_with(visual, |view, _| view.server_picker.clone());
        picker.update(visual, |_, cx| {
            cx.emit(ComboboxEvent::Activate(vec!["second".into()]))
        });
        let loading = rendered(visual);
        assert!(
            !loading
                .iter()
                .any(|label| label.starts_with("Open Project ")),
            "the second server must not render the first server's rows"
        );
        view.read_with(visual, |view, cx| {
            assert_eq!(view.server, "second");
            assert_eq!(view.project_query(cx), "");
            assert!(view.value("page").is_null());
        });
        let work = requests
            .try_iter()
            .find(|work| work.method == "git_projects_list")
            .unwrap();
        assert_eq!(work.args["serverId"], "second");
        view.update(visual, |view, cx| {
            view.resources
                .complete("page", work.generation, Ok(projects("second")));
            cx.notify();
        });
        assert!(
            rendered(visual)
                .iter()
                .any(|label| label.starts_with("Open Project 000"))
        );
        let second_input = view.read_with(visual, |view, _| {
            view.project_libraries["second"]
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
            cx.emit(ComboboxEvent::Activate(vec!["fixture".into()]))
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
            assert_eq!(view.value("page"), data);
            assert_eq!(view.project_query(cx), "project 0");
            assert_eq!(
                view.project_libraries["fixture"].input.as_ref().unwrap(),
                &first_input
            );
            assert_eq!(
                view.project_libraries["fixture"]
                    .list
                    .state
                    .logical_scroll_top()
                    .item_ix,
                25
            );
            assert_eq!(
                view.project_libraries["second"]
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
                .all(|work| work.method != "git_projects_list")
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
        let (view, visual) = cx.add_window_view(crate::page_perf_tests::owner);
        view.update(visual, |view, cx| {
            let (work, requests) = std::sync::mpsc::sync_channel(64);
            view.work = work.into();
            view.page = Page::Projects;
            let data = projects("fixture");
            let generation = view.resources.begin("page").unwrap();
            view.resources
                .complete("page", generation, Ok(data.clone()));
            let key = project_library::status_key(&data[0]);
            let generation = view.resources.begin(&key).unwrap();
            view.resources
                .complete(&key, generation, Ok(json!({"totalEntries":2})));
            let pending = view.resources.begin_refresh("page", false).unwrap();
            let pending_status = view.resources.begin_refresh(&key, false).unwrap();
            let initial_key = project_library::status_key(&data[1]);
            let pending_initial = view.resources.begin(&initial_key).unwrap();
            view.navigate(Page::Integration, cx);
            view.navigate(Page::Projects, cx);
            assert_eq!(view.value("page"), data);
            assert_eq!(view.value(&key), json!({"totalEntries":2}));
            assert!(!view.resources.complete("page", pending, Ok(json!([]))));
            assert!(!view.resources.complete(&key, pending_status, Ok(json!({}))));
            assert!(
                !view
                    .resources
                    .complete(&initial_key, pending_initial, Ok(json!({})))
            );
            let reads: Vec<_> = requests
                .try_iter()
                .filter(|work| work.method == "git_projects_list")
                .collect();
            assert_eq!(
                reads.len(),
                1,
                "a detached refresh must be reissued exactly once"
            );
            assert!(
                view.resources
                    .complete("page", reads[0].generation, Ok(data.clone()))
            );
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
            assert_eq!(view.value(&key), json!({"totalEntries":2}));
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
        let (view, visual) = cx.add_window_view(crate::page_perf_tests::owner);
        view.update(visual, |view, _| {
            view.page = Page::Projects;
            let data = projects("fixture");
            let generation = view.resources.begin("page").unwrap();
            view.resources
                .complete("page", generation, Ok(data.clone()));
            view.suspend_project_library();
            let original = view.snapshot.clone();
            view.snapshot["loadError"] = json!("Unavailable");
            view.snapshot["config"]["servers"] = json!([]);
            view.prune_project_libraries();
            assert!(view.project_libraries.contains_key("fixture"));
            view.restore_project_library();
            assert!(
                view.value("page").is_null(),
                "cannot restore before validating the connection"
            );
            view.snapshot = original;
            view.restore_project_library();
            assert_eq!(view.value("page"), data);
            view.suspend_project_library();
            view.snapshot["runtime"]["connectionRevisions"]["fixture"] = json!(2);
            view.restore_project_library();
            assert!(
                view.value("page").is_null(),
                "old connection data cannot enter a replacement"
            );
            view.snapshot["config"]["servers"] = json!([]);
            view.prune_project_libraries();
            assert!(!view.project_libraries.contains_key("fixture"));
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
            owner = Some(cx.new(|cx| crate::page_perf_tests::owner(window, cx)));
            IdleHost
        });
        let view = owner.unwrap();
        view.update(visual, |view, cx| {
            let (work, requests) = std::sync::mpsc::sync_channel(64);
            view.work = work.into();
            view.page = Page::Projects;
            let data = projects("fixture");
            let generation = view.resources.begin("page").unwrap();
            view.resources
                .complete("page", generation, Ok(data.clone()));
            view.navigate(Page::Integration, cx);
            let original = view.snapshot.clone();
            view.snapshot["loadError"] = json!("Unavailable");
            view.snapshot["config"]["servers"] = json!([]);
            view.navigate(Page::Projects, cx);
            view.navigate(Page::Integration, cx);
            view.snapshot = original;
            view.navigate(Page::Projects, cx);
            assert_eq!(view.value("page"), data);
            assert!(
                requests
                    .try_iter()
                    .all(|work| work.method != "git_projects_list")
            );
            view.navigate(Page::Integration, cx);
            let connection = view.project_libraries["fixture"].connection.clone();
            view.file_project_completed("fixture", Some(connection), Ok(data[0].clone()));
            view.navigate(Page::Projects, cx);
            assert_eq!(
                view.value("page"),
                data,
                "a saved bookmark refresh retains the previous list"
            );
            let read = requests
                .try_iter()
                .find(|work| work.method == "git_projects_list")
                .unwrap();
            view.resources
                .complete("page", read.generation, Err("Read failed".into()));
            view.refresh_project_library(false);
            view.poll_project_library();
            assert!(
                requests
                    .try_iter()
                    .all(|work| work.method != "git_projects_list"),
                "failed automatic reads must respect their retry interval"
            );
            view.refresh_project_library(true);
            let old = requests.try_recv().unwrap();
            view.project_bookmarks_changed("fixture");
            view.refresh_project_library(true);
            let fresh = requests.try_recv().unwrap();
            assert!(fresh.generation > old.generation);
            assert!(
                !view
                    .resources
                    .complete("page", old.generation, Ok(data.clone()))
            );
            assert!(
                view.resources
                    .complete("page", fresh.generation, Ok(json!([])))
            );
        });
    }
}
