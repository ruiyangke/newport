//! Container presentation. The shared client owns polling, caches and operations.
use super::*;
use gpui_kit::component::{input::InputEvent, skeleton::Skeleton};
use newport_client::containers::{Action, ActionTarget, Containers};
use newport_domain::containers::{self as domain, Container, ContainerVerb, ProjectVerb};
use std::{collections::HashSet, rc::Rc};

#[path = "containers_actions.rs"]
mod actions;
#[path = "compose_activity.rs"]
mod activity;
#[path = "compose_files.rs"]
mod compose_files;
#[path = "compose_jobs.rs"]
mod compose_jobs;
#[path = "compose_services.rs"]
mod compose_services;
#[path = "containers_detail.rs"]
mod detail;
#[path = "containers_disk.rs"]
mod disk;
#[path = "containers_inspect.rs"]
mod inspect;
#[path = "containers_logs.rs"]
mod logs;
#[path = "containers_ports.rs"]
mod ports;
#[cfg(test)]
#[path = "containers_preferences_tests.rs"]
mod preference_tests;
#[path = "container_spark.rs"]
mod spark;
#[cfg(test)]
#[path = "containers_status_tests.rs"]
mod status_tests;
#[cfg(test)]
#[path = "containers_tests.rs"]
pub(crate) mod tests;

pub(super) enum ContainersEvent {
    OpenTerminal { command: String },
    OpenShell { command: String, title: String },
    OpenPort(PortTarget),
    Notification { message: String },
    ComposeCompleted(newport_domain::compose::ComposeJob),
}
/// Project tabs keep their historical indices (0 Logs, 1 Activity, 2 Compose
/// files); the services table was added last but is shown first.
pub(super) const PROJECT_SERVICES: usize = 3;
/// Prefix for folded list sections in `collapsed`, beside project names.
const SECTION: &str = "section:";
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PortTarget {
    pub container: String,
    pub name: String,
    pub port: domain::ContainerPort,
}
/// Session-only choices, keyed by stable server ID by the host. Never include
/// inspection reveals or action drafts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Preferences {
    query: String,
    selected: Option<String>,
    collapsed: HashSet<String>,
    detail_tab: usize,
    project_tab: usize,
    live: bool,
    log_level: newport_domain::containers_log::Level,
    log_query: String,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            query: String::new(),
            selected: None,
            collapsed: HashSet::new(),
            detail_tab: 0,
            project_tab: PROJECT_SERVICES,
            live: true,
            log_level: newport_domain::containers_log::Level::All,
            log_query: String::new(),
        }
    }
}
#[derive(Clone)]
struct Row {
    key: Option<String>,
    name: String,
    note: String,
    clock: Option<(i64, bool)>,
    tone: domain::Tone,
    label: String,
    indent: f32,
    group: bool,
    project: Option<String>,
    jobs: bool,
    oneoff: bool,
    open: bool,
    /// A folded "Stopped" group, or the hint that stopped ones are hidden.
    stopped: Option<StoppedRow>,
}
#[derive(Clone, Copy, PartialEq)]
enum StoppedRow {
    Group,
    Hidden,
}
#[derive(Clone, Copy)]
struct Stopped {
    shown: bool,
    open: bool,
}
struct ReviewDialog {
    token: u64,
    compose: bool,
    window: AnyWindowHandle,
    handle: gpui_kit::component::DialogHandle,
}
#[derive(PartialEq)]
struct ListKey {
    revision: u64,
    query: String,
    collapsed: HashSet<String>,
    jobs_open: HashSet<String>,
    show_stopped: bool,
    stopped_open: bool,
}
#[derive(Default)]
struct ListCache {
    key: Option<ListKey>,
    rows: Rc<Vec<Row>>,
    fallback: Option<String>,
}
pub(super) struct ContainersPanel {
    connection: Value,
    pub(super) client: Containers,
    compose_state: compose_jobs::ComposeState,
    activity: activity::Activity,
    files: compose_files::Files,
    search: Entity<InputState>,
    selected: Option<String>,
    collapsed: HashSet<String>,
    jobs_open: HashSet<String>,
    /// The list shows every container, not only running ones and problems.
    show_stopped: bool,
    /// Under All, stopped standalone containers fold into their own group.
    stopped_open: bool,
    tab: usize,
    detail_tab: usize,
    project_tab: usize,
    live: bool,
    width: f32,
    /// The detail pane's measured width; the list beside it is resizable.
    detail_width: f32,
    list_focus: FocusHandle,
    detail_focus: FocusHandle,
    tab_focus: [FocusHandle; 4],
    list_scroll: UniformListScrollHandle,
    detail_scroll: ScrollHandle,
    logs: logs::Logs,
    list_cache: std::cell::RefCell<ListCache>,
    service_table: std::cell::RefCell<compose_services::TableCache>,
    snapshot_revision: u64,
    presentation_pending: bool,
    updated: Option<i64>,
    active_selection: Option<String>,
    inspected_id: Option<String>,
    reveal_values: bool,
    revealed: HashSet<String>,
    labels_open: bool,
    review: Option<Action>,
    review_focus: FocusHandle,
    review_confirm_focus: FocusHandle,
    review_dialog: Option<ReviewDialog>,
    review_dialog_token: u64,
    disk_view: disk::DiskView,
    ports: ports::PortsView,
    retired: bool,
    _subscriptions: Vec<Subscription>,
}
impl EventEmitter<ContainersEvent> for ContainersPanel {}
impl ContainersPanel {
    pub(super) fn new(
        server: String,
        connection: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut panel = Self::with_client(
            Containers::new(backend::Backend::discover(), server),
            window,
            cx,
        );
        panel.connection = connection;
        panel
    }
    pub(crate) fn with_client(
        mut client: Containers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let connection_subscription = crate::connectivity::observe(
            cx,
            |this: &Self| this.client.server_id(),
            |this, cx| {
                if !this.retired
                    && crate::connectivity::for_scope(&this.connection, cx)
                        .is_some_and(|state| state.phase == crate::connectivity::Phase::Connected)
                {
                    this.client.refresh();
                }
                cx.notify();
            },
        );
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Find a container, project or image")
        });
        let subscription = cx.subscribe(&search, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.list_scroll.scroll_to_item(0, ScrollStrategy::Top);
                this.sync_selection(cx);
                cx.notify();
            }
        });
        client.refresh();
        Self {
            connection: Value::Null,
            client,
            compose_state: compose_jobs::ComposeState::new(window, cx),
            activity: activity::Activity::new(window, cx),
            files: compose_files::Files::new(window, cx),
            search,
            selected: None,
            collapsed: HashSet::new(),
            jobs_open: HashSet::new(),
            show_stopped: false,
            stopped_open: false,
            tab: 0,
            detail_tab: 0,
            project_tab: PROJECT_SERVICES,
            live: true,
            width: 800.,
            detail_width: 0.,
            list_focus: cx.focus_handle(),
            detail_focus: cx.focus_handle(),
            tab_focus: std::array::from_fn(|_| cx.focus_handle()),
            list_scroll: UniformListScrollHandle::new(),
            detail_scroll: ScrollHandle::new(),
            logs: logs::Logs::new(window, cx),
            list_cache: Default::default(),
            service_table: Default::default(),
            snapshot_revision: 0,
            presentation_pending: false,
            updated: None,
            active_selection: None,
            inspected_id: None,
            reveal_values: false,
            revealed: HashSet::new(),
            labels_open: false,
            review: None,
            review_focus: cx.focus_handle(),
            review_confirm_focus: cx.focus_handle(),
            review_dialog: None,
            review_dialog_token: 0,
            disk_view: disk::DiskView::new(cx),
            ports: Default::default(),
            retired: false,
            _subscriptions: vec![subscription, connection_subscription],
        }
    }
    pub(super) fn preferences(&self, cx: &App) -> Preferences {
        let (detail_tab, project_tab) = self.preferred_tabs();
        Preferences {
            query: self.search.read(cx).value().to_string(),
            selected: self.selected.clone(),
            collapsed: self.collapsed.clone(),
            detail_tab,
            project_tab,
            live: self.live,
            log_level: self.logs.level,
            log_query: self.logs.find.read(cx).value().to_string(),
        }
    }
    pub(super) fn restore_preferences(
        &mut self,
        p: Preferences,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search
            .update(cx, |search, cx| search.set_value(p.query, window, cx));
        self.selected = p.selected;
        self.collapsed = p.collapsed;
        self.detail_tab = p.detail_tab.min(1);
        self.project_tab = p.project_tab.min(PROJECT_SERVICES);
        self.tab = self.preferred_tab(self.active_selection.as_deref());
        self.live = p.live;
        self.logs.level = p.log_level;
        self.logs
            .find
            .update(cx, |find, cx| find.set_value(p.log_query, window, cx));
        self.logs.rebuild(cx, true);
        self.sync_selection(cx);
        cx.notify();
    }
    fn preferred_tabs(&self) -> (usize, usize) {
        match self.active_selection.as_deref() {
            Some(key) if key.starts_with("c:") => (self.tab.min(1), self.project_tab),
            Some(key) if key.starts_with("p:") => (self.detail_tab, self.tab.min(PROJECT_SERVICES)),
            _ => (self.detail_tab, self.project_tab),
        }
    }
    fn preferred_tab(&self, selection: Option<&str>) -> usize {
        match selection {
            Some(key) if key.starts_with("c:") => self.detail_tab,
            Some(key) if key.starts_with("p:") => self.project_tab,
            _ => 0,
        }
    }
    pub(super) fn retire(&mut self, cx: &mut Context<Self>) {
        self.retired = true;
        self.client.retire();
        self.review = None;
        self.compose_state.proposed = None;
        self.sync_review_dialog(cx);
        cx.notify();
    }
    pub(super) fn has_pending(&self) -> bool {
        self.client.has_pending()
    }
    pub(super) fn has_review_dialog(&self) -> bool {
        self.review_dialog
            .as_ref()
            .is_some_and(|dialog| dialog.handle.is_active())
    }
    fn next_review_dialog_token(&mut self) -> u64 {
        self.review_dialog_token += 1;
        self.review_dialog_token
    }
    fn own_review_dialog(&mut self, token: u64, compose: bool, window: &mut Window, cx: &App) {
        self.review_dialog = window.active_dialog_handle(cx).map(|handle| ReviewDialog {
            token,
            compose,
            window: window.window_handle(),
            handle,
        });
    }
    fn review_dialog_matches(&self, token: u64) -> bool {
        self.review_dialog
            .as_ref()
            .is_some_and(|dialog| dialog.token == token)
    }
    fn review_dialog_current(&self, token: u64) -> bool {
        self.review_dialog_matches(token) && self.has_review_dialog()
    }
    fn forget_review_dialog(&mut self, token: u64) {
        if self.review_dialog_matches(token) {
            self.review_dialog = None;
        }
    }
    fn close_review_dialog(&mut self, token: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.review_dialog_matches(token) {
            let dialog = self.review_dialog.take().unwrap();
            window.close_dialog_if(dialog.handle.focus_handle(), cx);
        }
    }
    fn sync_review_dialog(&mut self, cx: &mut Context<Self>) {
        let invalid = self.review_dialog.as_ref().is_some_and(|dialog| {
            self.retired
                || if dialog.compose {
                    self.compose_state.proposed.is_none()
                } else {
                    self.review.is_none()
                }
        });
        if invalid {
            let dialog = self.review_dialog.take().unwrap();
            // Selection, retirement and background polling have no Window.
            // The captured Kit identity can never dismiss a later dialog.
            cx.defer(move |cx| {
                let _ = dialog.window.update(cx, |_, window, cx| {
                    window.close_dialog_if(dialog.handle.focus_handle(), cx);
                });
            });
        }
    }
    pub(super) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.client.refresh();
        cx.notify();
    }
    pub(super) fn poll(&mut self, visible: bool, cx: &mut Context<Self>) {
        // An accepted start may open Activity only while the user remains here.
        // Clear this even when the controller has no changed state to publish.
        if !visible {
            self.compose_state.show_activity_for = None;
            if self.review_dialog.is_some() {
                self.review = None;
                self.compose_state.proposed = None;
                self.sync_review_dialog(cx);
            }
        }
        if !self.client.poll(visible) && !(visible && self.presentation_pending) {
            return;
        }
        // Completion receipts still reach the host immediately. The retained
        // panel only needs one redraw when it becomes visible again.
        self.presentation_pending = !visible;
        let revision = self.client.resource("snapshot").map_or(0, |r| r.revision);
        if revision != self.snapshot_revision {
            self.snapshot_revision = revision;
            self.updated = Some(chrono::Local::now().timestamp_millis());
        }
        for receipt in self.client.take_action_completions() {
            if self.retired {
                continue;
            }
            if receipt.result.is_ok() && self.review.as_ref() == Some(&receipt.action) {
                if matches!(self.review, Some(Action::Prune { .. })) {
                    self.disk_view.show_summary();
                }
                self.review = None;
            }
            let subject = match &receipt.action {
                Action::Container { name, .. } => name.clone(),
                Action::Project { project, .. } => project.clone(),
                Action::Remove { names, .. } => names.join(", "),
                Action::Prune { .. } => "Docker cleanup".into(),
            };
            cx.emit(ContainersEvent::Notification {
                message: match receipt.result {
                    Ok(value) => match receipt.action {
                        Action::Prune { .. } => serde_json::from_value::<domain::Pruned>(value)
                            .map(|result| {
                                if result.reclaimed > 0 {
                                    format!("Freed {}", bytes(&json!(result.reclaimed)))
                                } else {
                                    "Nothing to remove".into()
                                }
                            })
                            .unwrap_or_else(|_| "Docker cleanup completed.".into()),
                        _ => format!("{subject}: operation completed."),
                    },
                    Err(error) => format!("{subject}: {error}"),
                },
            });
        }
        for job in self.client.take_compose_completions() {
            if !self.retired {
                cx.emit(ContainersEvent::ComposeCompleted(job));
            }
        }
        if !self.retired && visible {
            if !self.client.compose_run_pending()
                && let Some(project) = self.compose_state.show_activity_for.take()
                && self.active_selection.as_deref() == Some(domain::project_key(&project).as_str())
                && self
                    .client
                    .resource("compose-start")
                    .is_some_and(|r| r.error.is_none() && r.value["job"].is_string())
            {
                self.tab = 1;
            }
            self.sync_selection(cx);
            // Cache hidden completions in the client; build output rows only
            // when this panel is visible. sync_selection also syncs its logs.
            self.activity.sync(&self.client, cx);
        }
        self.sync_review_dialog(cx);
        if visible {
            cx.notify();
        }
    }
    pub(super) fn pending(&self, key: &str) -> bool {
        self.client
            .resource(key)
            .is_some_and(|r| r.display_pending())
    }
    fn error(&self, key: &str) -> Option<String> {
        if key == "action" {
            let target = self
                .client
                .selected_container()
                .map(|c| ActionTarget::Container(c.id.clone()))
                .or_else(|| {
                    self.client
                        .selected_project()
                        .map(|p| ActionTarget::Project(p.to_owned()))
                })?;
            return self
                .client
                .action_status(&target)
                .and_then(|status| status.error.clone());
        }
        self.client.resource(key).and_then(|r| r.error.clone())
    }
    pub(super) fn status(&self, key: &str, cx: &App) -> AnyElement {
        self.error(key)
            .map(|error| alert(format!("containers-error-{key}"), error, cx).into_any_element())
            .unwrap_or_else(|| div().into_any_element())
    }
    pub(super) fn compose_value(&self) -> Value {
        self.client
            .compose_project()
            .cloned()
            .unwrap_or(Value::Null)
    }
    fn prepared_list(&self, cx: &App) -> (Rc<Vec<Row>>, Option<String>) {
        let key = ListKey {
            revision: self
                .client
                .resource("snapshot")
                .map_or(0, |r| r.data_revision),
            query: self.search.read(cx).value().trim().to_lowercase(),
            collapsed: self.collapsed.clone(),
            jobs_open: self.jobs_open.clone(),
            show_stopped: self.show_stopped,
            stopped_open: self.stopped_open,
        };
        let mut cache = self.list_cache.borrow_mut();
        if cache.key.as_ref() != Some(&key) {
            let (rows, fallback) = self
                .client
                .snapshot()
                .map(|snapshot| {
                    let groups =
                        domain::group_matching_containers(&snapshot.containers, &key.query);
                    let preferred = domain::default_selection(&groups);
                    let rows = master_rows(
                        &snapshot.containers,
                        &key.query,
                        &key.collapsed,
                        &key.jobs_open,
                        Stopped {
                            shown: key.show_stopped,
                            open: key.stopped_open,
                        },
                        self.updated.unwrap_or_default(),
                    );
                    // The Running filter and folded groups hide rows; the
                    // default never selects one the list does not show.
                    let fallback = if rows.iter().any(|row| row.key.as_ref() == Some(&preferred)) {
                        preferred
                    } else {
                        rows.iter()
                            .find_map(|row| row.key.clone())
                            .unwrap_or_else(|| domain::DISK.into())
                    };
                    (rows, Some(fallback))
                })
                .unwrap_or_default();
            cache.rows = Rc::new(rows);
            cache.fallback = fallback;
            cache.key = Some(key);
        }
        (cache.rows.clone(), cache.fallback.clone())
    }
    fn rows(&self, cx: &App) -> Rc<Vec<Row>> {
        self.prepared_list(cx).0
    }
    fn effective_selection(&self, cx: &App) -> Option<String> {
        self.selected.clone().or_else(|| self.prepared_list(cx).1)
    }
    fn sync_selection(&mut self, cx: &mut Context<Self>) {
        let next = self.effective_selection(cx);
        (self.detail_tab, self.project_tab) = self.preferred_tabs();
        if self.active_selection != next {
            self.tab = self.preferred_tab(next.as_deref());
            if matches!(self.review, Some(Action::Prune { .. })) {
                self.disk_view.show_summary();
            }
            self.review = None;
            self.compose_state.proposed = None;
            self.compose_state.show_activity_for = None;
            match next.as_deref() {
                Some(key) if key.starts_with("c:") => {
                    self.client.select_container(key[2..].into());
                }
                Some(key) if key.starts_with("p:") => {
                    self.client.select_project(key[2..].into());
                }
                _ => self.client.clear_selection(),
            }
            self.active_selection = next;
        }
        self.client.set_logs_active(
            self.active_selection
                .as_deref()
                .is_some_and(|key| key != domain::DISK)
                && self.tab == 0,
            self.live,
        );
        self.client
            .set_disk_active(self.active_selection.as_deref() == Some(domain::DISK));
        let id = self.client.selected_container().map(|c| c.id.clone());
        if self.inspected_id != id {
            self.inspected_id = id;
            self.reveal_values = false;
            self.revealed.clear();
            self.labels_open = false;
            self.review = None;
        }
        self.logs.sync(self.client.log_window(), cx);
        self.sync_activity(cx);
        self.sync_review_dialog(cx);
    }
    fn select(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.as_ref() != Some(&key) {
            if matches!(self.review, Some(Action::Prune { .. })) {
                self.disk_view.show_summary();
            }
            self.selected = Some(key);
            self.review = None;
            self.compose_state.proposed = None;
            self.detail_scroll.set_offset(point(px(0.), px(0.)));
            self.sync_selection(cx);
        }
        if self.width <= 600. {
            self.detail_focus.focus(window, cx)
        } else {
            self.list_focus.focus(window, cx)
        }
        cx.notify();
    }
    fn navigate(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.modifiers.modified() || window.has_active_dialog(cx) {
            return;
        }
        let choices: Vec<_> = self
            .rows(cx)
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.key.clone().map(|key| (i, key)))
            .collect();
        if choices.is_empty() {
            return;
        }
        let selected = self.effective_selection(cx);
        let current = choices
            .iter()
            .position(|(_, k)| Some(k) == selected.as_ref());
        let last = choices.len() - 1;
        // Without a selected row (Disk usage, or one the filter hides), Down
        // enters the list at its first row and Up at its last.
        let next = match (event.keystroke.key.as_str(), current) {
            ("down", Some(current)) => (current + 1).min(last),
            ("up", Some(current)) => current.saturating_sub(1),
            ("down" | "home", _) => 0,
            ("up" | "end", _) => last,
            _ => return,
        };
        self.list_scroll
            .scroll_to_item(choices[next].0, ScrollStrategy::Center);
        self.select(choices[next].1.clone(), window, cx);
        window.prevent_default();
        cx.stop_propagation();
    }
    fn list(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows = self.rows(cx);
        let selected = self.effective_selection(cx);
        div()
            .id("containers-master")
            .debug_selector(|| "containers-master".into())
            .role(accesskit::Role::Region)
            .aria_label("Container list")
            .flex()
            .flex_col()
            .h_full()
            .py(px(6.))
            .min_h_0()
            .min_w_0()
            // Beside the detail, the resizable split owns width and divider.
            .when(self.width > 600., |v| v.size_full())
            .when(self.width <= 600., |v| v.flex_1())
            .when(self.client.snapshot().is_none(), |v| {
                v.child(list_skeleton())
            })
            .when(self.client.snapshot().is_some(), |v| {
                v.child(
                    div()
                        .id("containers-listbox")
                        .role(accesskit::Role::ListBox)
                        .aria_label("Containers")
                        .track_focus(&self.list_focus)
                        .focus_visible(|style| crate::controls::inset_focus_ring(style, cx))
                        .on_key_down(cx.listener(Self::navigate))
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .child(
                            uniform_list(
                                "containers-rows",
                                rows.len(),
                                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                                    range
                                        .map(|i| {
                                            this.master_row(i, &rows[i], selected.as_deref(), cx)
                                        })
                                        .collect()
                                }),
                            )
                            .track_scroll(&self.list_scroll)
                            .flex_1()
                            .min_h_0()
                            .min_w_0(),
                        ),
                )
            })
            .into_any_element()
    }
    /// Search, the Running/All filter, Disk usage and Refresh for the page.
    fn toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::connections_selector::{Choice, Selector};
        let (running, total) = self.client.snapshot().map_or((0, 0), |snapshot| {
            (
                snapshot.containers.iter().filter(|c| active(c)).count(),
                snapshot.containers.len(),
            )
        });
        let owner = cx.weak_entity();
        let disk = self.active_selection.as_deref() == Some(domain::DISK);
        crate::controls::page_toolbar(cx)
            .id("containers-toolbar")
            .debug_selector(|| "containers-toolbar".into())
            .child(
                div()
                    .w(px(crate::controls::SEARCH_WIDTH))
                    .min_w(px(120.))
                    .flex_shrink(1.)
                    .child(
                        crate::controls::search_input(&self.search, cx)
                            .aria_label("Find a container"),
                    ),
            )
            .child(Selector::new(
                "containers-filter",
                "Show",
                usize::from(self.show_stopped),
                vec![
                    Choice {
                        count: Some(running),
                        tooltip: Some("Running containers and any that need attention"),
                        ..Choice::text(0, "Running")
                    },
                    Choice {
                        count: Some(total),
                        ..Choice::text(1, "All")
                    },
                ],
                move |index, _, cx| {
                    let _ = owner.update(cx, |this, cx| {
                        this.show_stopped = index == 1;
                        cx.notify();
                    });
                },
            ))
            .child(div().flex_1())
            .child(
                crate::controls::button("containers-disk")
                    .ghost()
                    .icon(IconName::HardDrive)
                    .label("Disk usage")
                    .selected(disk)
                    .toggled(disk)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.select(domain::DISK.into(), window, cx)
                    })),
            )
            .child(
                crate::controls::icon_button("containers-refresh")
                    .ghost()
                    .icon(IconName::RefreshCw)
                    .tooltip("Refresh")
                    .accessibility_label("Refresh containers")
                    .loading(self.pending("snapshot"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.client.refresh();
                        if this.active_selection.as_deref() == Some(domain::DISK) {
                            this.client.retry_disk();
                        } else if this.tab == 0 {
                            // Logs have no refresh of their own, even when paused.
                            this.client.retry_logs();
                        }
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
    /// The shared page status bar: what the engine runs, and which engine.
    fn status_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(snapshot) = self.client.snapshot() else {
            return div().into_any_element();
        };
        let running = snapshot
            .containers
            .iter()
            .filter(|c| c.state == domain::ContainerState::Running)
            .count();
        let stopped = snapshot.containers.len() - running;
        let projects = domain::group_containers(&snapshot.containers)
            .projects
            .len();
        let plural =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let summary = format!(
            "{} running · {} stopped · {}",
            running,
            stopped,
            plural(projects, "project", "projects")
        );
        let engine = &snapshot.engine;
        let engine = if engine.version.is_empty() {
            String::new()
        } else {
            format!(
                "{} {}{}",
                if engine.podman { "Podman" } else { "Docker" },
                engine.version,
                if engine.rootless { " · rootless" } else { "" }
            )
        };
        div()
            .id("containers-status")
            .debug_selector(|| "containers-status".into())
            .flex_shrink_0()
            .child(
                crate::controls::status_bar(cx)
                    .left(
                        div()
                            .id("containers-summary")
                            .role(accesskit::Role::Status)
                            .aria_label(summary.clone())
                            .child(summary),
                    )
                    .right(engine),
            )
            .into_any_element()
    }
    fn master_row(
        &self,
        index: usize,
        row: &Row,
        selected: Option<&str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if row.group && row.project.is_none() {
            let key = format!("{SECTION}{}", row.name);
            return div()
                .h(px(28.))
                .pt(px(4.))
                .child(
                    crate::controls::group_heading(
                        SharedString::from(format!(
                            "containers-section-{}",
                            row.name.to_lowercase()
                        )),
                        &row.name,
                        row.note.parse().unwrap_or_default(),
                        row.open,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.collapsed.remove(&key) {
                            this.collapsed.insert(key.clone());
                        }
                        cx.notify();
                    })),
                )
                .into_any_element();
        }
        if row.stopped == Some(StoppedRow::Group) {
            return div()
                .h(px(28.))
                .pt(px(4.))
                .child(
                    crate::controls::group_heading(
                        "containers-stopped-group",
                        &row.name,
                        row.note.parse().unwrap_or_default(),
                        row.open,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.stopped_open = !this.stopped_open;
                        cx.notify();
                    })),
                )
                .into_any_element();
        }
        if row.stopped == Some(StoppedRow::Hidden) {
            return div()
                .id("containers-stopped-hidden")
                .role(accesskit::Role::Button)
                .aria_label(row.label.clone())
                .test_support()
                .h(px(28.))
                .pl(px(row.indent))
                .pr(px(8.))
                .flex()
                .items_center()
                .gap(px(6.))
                .cursor_pointer()
                .text_size(px(crate::ui::FONT_SIZE))
                .text_color(cx.theme().muted_foreground)
                .hover(|v| v.bg(theme::row_hover(cx)).text_color(cx.theme().foreground))
                .child(div().flex_1().min_w_0().truncate().child(row.name.clone()))
                .child(row.note.clone())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_stopped = true;
                    this.stopped_open = true;
                    cx.notify();
                }))
                .into_any_element();
        }
        if row.jobs {
            let project = row.project.clone().unwrap_or_default();
            return div()
                .h(px(28.))
                .pt(px(4.))
                .pl(px(19.))
                .child(
                    crate::controls::group_heading(
                        SharedString::from(format!("jobs-{project}")),
                        &row.name,
                        row.note.parse().unwrap_or_default(),
                        row.open,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.jobs_open.remove(&project) {
                            this.jobs_open.insert(project.clone());
                        }
                        cx.notify();
                    })),
                )
                .into_any_element();
        }
        let Some(key) = row.key.clone() else {
            return crate::empty::compact(row.name.clone(), cx)
                .h(px(28.))
                .py(px(5.))
                .px(px(14.))
                .overflow_hidden()
                .into_any_element();
        };
        let active = selected == Some(key.as_str());
        // The whole line highlights, including a project's fold chevron.
        let mut outer = div()
            .debug_selector(move || format!("containers-line-{index}"))
            .h(px(28.))
            .w_full()
            .flex()
            .items_center()
            .when(active, |v| v.bg(theme::selected_fill(cx.theme().is_dark())))
            .when(!active, |v| v.hover(|v| v.bg(theme::row_hover(cx))));
        if let Some(project) = row.project.clone() {
            outer = outer.child(
                crate::controls::compact_icon_button(SharedString::from(format!("fold-{project}")))
                    .ghost()
                    .icon(if row.open {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .expanded(row.open)
                    .accessibility_label(format!(
                        "{} {project}",
                        if row.open { "Fold" } else { "Unfold" }
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.collapsed.remove(&project) {
                            this.collapsed.insert(project.clone());
                        }
                        cx.notify();
                    })),
            );
        }
        outer
            .child(
                div()
                    .id(SharedString::from(format!("container-option-{key}")))
                    .debug_selector(move || format!("containers-row-{index}"))
                    .role(accesskit::Role::ListBoxOption)
                    .aria_label(row.label.clone())
                    .aria_selected(active)
                    .when(active, |v| v.aria_active_descendant())
                    .h(px(28.))
                    .flex_1()
                    .min_w_0()
                    .rounded(px(crate::ui::RADIUS))
                    .pl(px(row.indent))
                    .pr(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(crate::ui::FONT_SIZE))
                    .cursor_pointer()
                    .child(if key == domain::DISK {
                        Icon::new(IconName::HardDrive)
                            .size(px(crate::ui::SMALL_ICON))
                            .text_color(cx.theme().muted_foreground)
                            .into_any_element()
                    } else {
                        dot(row.tone, cx).into_any_element()
                    })
                    .child(div().flex_1().min_w_0().truncate().child(row.name.clone()))
                    .when(row.oneoff, |v| {
                        let badge_id = format!("container-job-badge-{key}");
                        v.child(
                            div()
                                .id(SharedString::from(badge_id.clone()))
                                .debug_selector(move || badge_id.clone())
                                .child(crate::controls::tag("job", cx)),
                        )
                    })
                    .child(
                        quiet(
                            row.clock.map_or_else(
                                || row.note.clone(),
                                |(at, ago)| {
                                    let span = newport_domain::services::duration(
                                        self.updated.unwrap_or_default().saturating_sub(at),
                                    );
                                    // Say which clock this is: uptime, or time since exit.
                                    if ago {
                                        format!("exited {span} ago")
                                    } else {
                                        format!("up {span}")
                                    }
                                },
                            ),
                            cx,
                        )
                        .flex_shrink_0()
                        .text_size(px(crate::ui::FONT_SIZE))
                        .text_color(tone_color(
                            if matches!(row.tone, domain::Tone::Danger | domain::Tone::Warning) {
                                row.tone
                            } else {
                                domain::Tone::Muted
                            },
                            cx,
                        )),
                    )
                    .on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.select(key.clone(), window, cx)
                        }),
                    )
                    .test_support(),
            )
            .into_any_element()
    }
}
impl Render for ContainersPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.files.sync(&self.client, window, cx);
        self.disk_view
            .sync(&self.client, self.detail_content_width());
        let snapshot = self.client.snapshot();
        let error = self.error("snapshot");
        let initial = snapshot.is_none() && error.is_none();
        let owner = cx.weak_entity();
        let compact = self.width <= 600.;
        div()
            .id("containers-page")
            .debug_selector(|| "containers-page".into())
            .relative()
            .flex_1()
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let width = f32::from(bounds.size.width);
                        let _ = owner.update(cx, |this, cx| {
                            if this.width != width {
                                let was_list = this.list_focus.is_focused(window);
                                this.width = width;
                                if width <= 600. && this.selected.is_some() && was_list {
                                    this.detail_focus.focus(window, cx);
                                }
                                window.refresh();
                                cx.notify();
                            }
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full()
                .left_0()
                .top_0(),
            )
            .when(snapshot.is_some() || initial, |v| v.child(self.toolbar(cx)))
            .when(snapshot.is_some(), |v| {
                v.when_some(error.clone(), |v, error| {
                    v.child(
                        alert(
                            "containers-stale",
                            format!("{error} Showing the last reading."),
                            cx,
                        )
                        .flex_shrink_0()
                        .px(px(crate::controls::TOOLBAR_INSET))
                        .py(px(8.)),
                    )
                })
            })
            .when(snapshot.is_some_and(|s| s.truncated), |v| {
                v.child(quiet("Showing the newest 500 containers.", cx).mb(px(12.)))
            })
            .when(snapshot.is_none() && error.is_some(), |v| {
                v.child(self.unavailable(error.as_deref().unwrap_or_default(), cx))
            })
            .when(snapshot.is_some() || initial, |v| {
                v.child(div().flex_1().min_h_0().min_w_0().flex().map(|v| {
                    if compact {
                        v.when(self.selected.is_none(), |v| v.child(self.list(cx)))
                            .when(self.selected.is_some(), |v| {
                                v.child(self.detail(window, cx))
                            })
                    } else {
                        let key = SharedString::from("containers-split");
                        let layout = crate::controls::split_state(&key, window, cx);
                        v.child(
                            gpui_kit::component::resizable::h_resizable(key)
                                .with_state(&layout)
                                .child(
                                    gpui_kit::component::resizable::resizable_panel()
                                        .size(px(280.))
                                        .size_range(px(220.)..px(520.))
                                        .flex_none()
                                        .child(self.list(cx)),
                                )
                                .child(
                                    gpui_kit::component::resizable::resizable_panel()
                                        .size_range(px(360.)..Pixels::MAX)
                                        .child(self.detail(window, cx)),
                                ),
                        )
                    }
                }))
            })
            .when(snapshot.is_some(), |v| v.child(self.status_bar(cx)))
    }
}
impl ContainersPanel {
    fn unavailable(&self, error: &str, cx: &mut Context<Self>) -> AnyElement {
        let denied = error.starts_with("This account can't use Docker");
        let title = if denied {
            "This account can’t use Docker"
        } else if error.starts_with("Docker isn't installed") {
            "No Docker on this server"
        } else {
            "Docker isn’t answering"
        };
        crate::empty::pane(IconName::Boxes, title, if denied{"Docker’s socket only lets root and the docker group in. Adding this account to the group gives it Docker access (which is root-equivalent); it takes effect at the next login."}else{error},cx)
            .child(div().flex().gap(px(8.)).when(denied,|v|v.child(crate::controls::button("docker-permission").icon(IconName::Terminal).label("Add me to the docker group…").on_click(cx.listener(|_,_,_,cx|cx.emit(ContainersEvent::OpenTerminal{command:"sudo usermod -aG docker $USER".into()})))))
                .child(crate::controls::button("containers-retry").label("Try again").disabled(self.pending("snapshot")).loading(self.pending("snapshot")).on_click(cx.listener(|this,_,_,cx|this.refresh(cx)))))
            .into_any_element()
    }
}
/// A container that belongs in the default (Running) list: anything alive,
/// and anything that stopped badly enough to need attention.
fn active(c: &Container) -> bool {
    matches!(
        c.state,
        domain::ContainerState::Running
            | domain::ContainerState::Paused
            | domain::ContainerState::Restarting
    ) || domain::is_problem(c)
}

fn master_rows(
    all: &[Container],
    query: &str,
    collapsed: &HashSet<String>,
    jobs_open: &HashSet<String>,
    stopped: Stopped,
    now: i64,
) -> Vec<Row> {
    let needle = query.trim().to_lowercase();
    let filtered: Vec<_> = all
        .iter()
        .filter(|c| domain::matches_query(c, &needle))
        .cloned()
        .collect();
    let groups = domain::group_containers(&filtered);
    let mut rows = Vec::new();
    // Sections fold like projects; their keys can't clash with Compose names.
    let section_open =
        |name: &str| !needle.is_empty() || !collapsed.contains(&format!("{SECTION}{name}"));
    let section = |name: &str, count: usize| Row {
        key: None,
        label: format!("{name}, {count}"),
        name: name.to_owned(),
        note: count.to_string(),
        clock: None,
        tone: domain::Tone::Muted,
        indent: 8.,
        group: true,
        project: None,
        jobs: false,
        oneoff: false,
        open: section_open(name),
        stopped: None,
    };
    if !groups.projects.is_empty() {
        rows.push(section("Projects", groups.projects.len()));
    }
    let projects = if section_open("Projects") {
        groups.projects
    } else {
        vec![]
    };
    for project in projects {
        let state = domain::project_state(&project);
        let open = !needle.is_empty() || !collapsed.contains(project.name);
        rows.push(Row {
            key: Some(domain::project_key(project.name)),
            name: project.name.to_owned(),
            clock: None,
            note: if project.problems > 0 {
                format!(
                    "{} {}",
                    project.problems,
                    if project.problems == 1 {
                        "issue"
                    } else {
                        "issues"
                    }
                )
            } else if !project.services.is_empty() {
                format!("{}/{} running", project.running, project.services.len())
            } else {
                String::new()
            },
            tone: state.tone,
            label: format!("{}: Compose project, {}", project.name, state.label),
            indent: 6.,
            group: false,
            project: Some(project.name.to_owned()),
            jobs: false,
            oneoff: false,
            open,
            stopped: None,
        });
        if open {
            rows.extend(project.services.iter().map(|c| container_row(c, 30., now)));
            if !project.jobs.is_empty() {
                let jobs_visible = !needle.is_empty() || jobs_open.contains(project.name);
                rows.push(Row {
                    key: None,
                    name: "One-off jobs".into(),
                    note: project.jobs.len().to_string(),
                    clock: None,
                    tone: domain::Tone::Muted,
                    label: String::new(),
                    indent: 30.,
                    group: false,
                    project: Some(project.name.to_owned()),
                    jobs: true,
                    oneoff: false,
                    open: jobs_visible,
                    stopped: None,
                });
                if jobs_visible {
                    rows.extend(project.jobs.iter().map(|c| container_row(c, 44., now)));
                }
            }
        }
    }
    // Leftover stopped containers usually outnumber live ones. They stay out
    // of the default list; a search always shows every match.
    let (live, idle): (Vec<&Container>, Vec<&Container>) = groups
        .standalone
        .iter()
        .copied()
        .partition(|c| active(c) || !needle.is_empty());
    let listed = if stopped.shown {
        live.len() + idle.len()
    } else {
        live.len()
    };
    if listed > 0 || !idle.is_empty() {
        rows.push(section("Containers", listed));
    }
    let standalone = section_open("Containers");
    if standalone {
        rows.extend(live.iter().map(|c| container_row(c, 14., now)));
    }
    if standalone && !idle.is_empty() {
        let toggle = |name: String, kind: StoppedRow, open: bool| Row {
            key: None,
            label: name.clone(),
            name,
            note: String::new(),
            clock: None,
            tone: domain::Tone::Muted,
            indent: 14.,
            group: false,
            project: None,
            jobs: false,
            oneoff: false,
            open,
            stopped: Some(kind),
        };
        if stopped.shown {
            let mut group = toggle("Stopped".into(), StoppedRow::Group, stopped.open);
            group.note = idle.len().to_string();
            rows.push(group);
            if stopped.open {
                rows.extend(idle.iter().map(|c| container_row(c, 34., now)));
            }
        } else {
            rows.push(toggle(
                format!("{} stopped hidden · Show all", idle.len()),
                StoppedRow::Hidden,
                false,
            ));
        }
    }
    if rows.is_empty() {
        let mut row = section(
            if needle.is_empty() {
                "No containers on this server yet."
            } else {
                "No projects or containers match."
            },
            0,
        );
        row.group = false;
        row.note = String::new();
        rows.push(row);
    }
    rows
}
fn container_row(c: &Container, indent: f32, now: i64) -> Row {
    let state = domain::state_of(c);
    let oneoff = c.compose.as_ref().is_some_and(|compose| compose.oneoff);
    Row {
        key: Some(domain::container_key(c)),
        name: domain::display_name(c).into(),
        note: domain::list_note(c, now),
        clock: match c.state {
            domain::ContainerState::Running
                if c.cpu.is_none() && c.health != Some(domain::ContainerHealth::Unhealthy) =>
            {
                c.started_at.map(|at| (at, false))
            }
            domain::ContainerState::Exited if !c.oom_killed && !domain::is_problem(c) => {
                c.finished_at.map(|at| (at, true))
            }
            _ => None,
        },
        tone: state.tone,
        label: if oneoff {
            format!("{}: one-off job, {}", c.name, state.label)
        } else {
            format!("{}: {}", c.name, state.label)
        },
        indent,
        group: false,
        project: None,
        jobs: false,
        oneoff,
        open: false,
        stopped: None,
    }
}
/// A detail pane's title: the shared 18px heading.
fn heading(value: impl Into<SharedString>) -> Stateful<Div> {
    let value = value.into();
    crate::controls::detail_title(
        SharedString::from(format!("container-heading-{value}")),
        value,
    )
}
fn quiet(value: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_size(px(crate::ui::FONT_SIZE))
        .text_color(cx.theme().muted_foreground)
        .child(value.into())
}
/// Plain GPUI text is not automatically included in the accessibility tree.
/// Keep review instructions navigable alongside the modal's title and controls.
fn action_description(value: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    let value = value.into();
    quiet(value.clone(), cx)
        .id("container-action-description")
        .role(accesskit::Role::Label)
        .aria_label(value)
        .test_support()
}
/// The shared inline error, also findable by its id in layout tests.
fn alert(id: impl Into<SharedString>, value: impl Into<SharedString>, cx: &App) -> Stateful<Div> {
    let id = id.into();
    crate::controls::alert(id.clone(), value, cx).debug_selector(move || id.to_string())
}
fn tone_color(tone: domain::Tone, cx: &App) -> Hsla {
    match tone {
        domain::Tone::Success => theme::success(cx.theme().is_dark()),
        domain::Tone::Warning => theme::warning(cx.theme().is_dark()),
        domain::Tone::Danger => theme::error(cx.theme().is_dark()),
        domain::Tone::Muted => cx.theme().muted_foreground,
    }
}
/// The shared status dot; muted states draw the hollow ring.
fn dot(tone: domain::Tone, cx: &App) -> Div {
    crate::controls::status_dot(
        (tone != domain::Tone::Muted).then(|| tone_color(tone, cx)),
        cx,
    )
}
fn list_skeleton() -> impl IntoElement {
    div()
        .id("containers-list-loading")
        .role(accesskit::Role::Status)
        .aria_label("Loading containers")
        .a11y_synthetic_children(|tree| tree.parent_node().set_busy())
        .flex()
        .flex_col()
        .gap(px(16.))
        .pt(px(8.))
        .children([120., 150., 110., 170., 100., 160., 140.].map(|width| {
            div()
                .h(px(12.))
                .px(px(8.))
                .child(Skeleton::new().w(px(width)).h(px(10.)))
        }))
}
