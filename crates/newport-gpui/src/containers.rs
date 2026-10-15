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
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PortTarget {
    pub container: String,
    pub name: String,
    pub port: domain::ContainerPort,
}
/// Session-only choices, keyed by stable server ID by the host. Match the
/// Tauri workspace store; never include inspection reveals or action drafts.
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
            project_tab: 0,
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
}
#[derive(Default)]
struct ListCache {
    key: Option<ListKey>,
    rows: Rc<Vec<Row>>,
    fallback: Option<String>,
}
pub(super) struct ContainersPanel {
    pub(super) client: Containers,
    compose_state: compose_jobs::ComposeState,
    activity: activity::Activity,
    files: compose_files::Files,
    search: Entity<InputState>,
    selected: Option<String>,
    collapsed: HashSet<String>,
    jobs_open: HashSet<String>,
    tab: usize,
    detail_tab: usize,
    project_tab: usize,
    live: bool,
    width: f32,
    list_focus: FocusHandle,
    detail_focus: FocusHandle,
    tab_focus: [FocusHandle; 3],
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
        _: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_client(
            Containers::new(backend::Backend::discover(), server),
            window,
            cx,
        )
    }
    pub(crate) fn with_client(
        mut client: Containers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
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
            client,
            compose_state: compose_jobs::ComposeState::new(window, cx),
            activity: activity::Activity::new(window, cx),
            files: compose_files::Files::new(window, cx),
            search,
            selected: None,
            collapsed: HashSet::new(),
            jobs_open: HashSet::new(),
            tab: 0,
            detail_tab: 0,
            project_tab: 0,
            live: true,
            width: 800.,
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
            _subscriptions: vec![subscription],
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
        self.project_tab = p.project_tab.min(2);
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
            Some(key) if key.starts_with("p:") => (self.detail_tab, self.tab.min(2)),
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
        };
        let mut cache = self.list_cache.borrow_mut();
        if cache.key.as_ref() != Some(&key) {
            let (rows, fallback) = self
                .client
                .snapshot()
                .map(|snapshot| {
                    let groups =
                        domain::group_matching_containers(&snapshot.containers, &key.query);
                    let fallback = Some(domain::default_selection(&groups));
                    (
                        master_rows(
                            &snapshot.containers,
                            &key.query,
                            &key.collapsed,
                            &key.jobs_open,
                            self.updated.unwrap_or_default(),
                        ),
                        fallback,
                    )
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
            .position(|(_, k)| Some(k) == selected.as_ref())
            .unwrap_or(0);
        let next = match event.keystroke.key.as_str() {
            "down" => (current + 1).min(choices.len() - 1),
            "up" => current.saturating_sub(1),
            "home" => 0,
            "end" => choices.len() - 1,
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
            .p(px(crate::controls::PAGE_PADDING))
            .when(self.width > 600., |v| {
                v.border_r_1().border_color(cx.theme().border)
            })
            .min_h_0()
            .min_w_0()
            .when(self.width > 600., |v| v.w(px(280.)).flex_shrink_0())
            .when(self.width <= 600., |v| v.flex_1())
            .child(
                div().h(px(28.)).flex_shrink_0().mb(px(8.)).child(
                    crate::controls::input(&self.search)
                        .prefix(Icon::new(IconName::Search).size(px(14.)))
                        .aria_label("Find a container"),
                ),
            )
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
                        .rounded(px(6.))
                        .border_1()
                        .border_color(cx.theme().ring.opacity(0.))
                        .focus_visible(|s| s.border_color(cx.theme().ring))
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
    fn master_row(
        &self,
        index: usize,
        row: &Row,
        selected: Option<&str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if row.group && row.project.is_none() {
            return div()
                .h(px(28.))
                .px(px(8.))
                .pt(px(8.))
                .text_size(px(10.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(cx.theme().muted_foreground)
                .child(row.name.to_uppercase())
                .into_any_element();
        }
        if row.jobs {
            let project = row.project.clone().unwrap_or_default();
            return div()
                .h(px(28.))
                .pl(px(30.))
                .child(
                    crate::controls::compact_button(SharedString::from(format!("jobs-{project}")))
                        .ghost()
                        .label(row.name.clone())
                        .icon(if row.open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .expanded(row.open)
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
                .px(px(8.))
                .overflow_hidden()
                .into_any_element();
        };
        let active = selected == Some(key.as_str());
        let mut outer = div().h(px(28.)).w_full().flex().items_center();
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
                    .rounded(px(6.))
                    .pl(px(row.indent))
                    .pr(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(12.5))
                    .cursor_pointer()
                    .when(active, |v| v.bg(cx.theme().sidebar_accent))
                    .when(!active, |v| v.hover(|v| v.bg(theme::row_hover(cx))))
                    .child(if key == domain::DISK {
                        Icon::new(IconName::HardDrive)
                            .size(px(13.))
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
                                .flex_shrink_0()
                                .rounded(px(4.))
                                .bg(cx.theme().muted)
                                .px(px(4.))
                                .text_size(px(10.))
                                .text_color(cx.theme().muted_foreground)
                                .child("job"),
                        )
                    })
                    .child(
                        quiet(
                            row.clock.map_or_else(
                                || row.note.clone(),
                                |(at, ago)| {
                                    format!(
                                        "{}{}",
                                        newport_domain::services::duration(
                                            self.updated.unwrap_or_default().saturating_sub(at)
                                        ),
                                        if ago { " ago" } else { "" }
                                    )
                                },
                            ),
                            cx,
                        )
                        .flex_shrink_0()
                        .text_size(px(11.))
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
        self.disk_view.sync(&self.client, self.width);
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
            .when(snapshot.is_some(), |v| {
                v.when_some(error.clone(), |v, error| {
                    v.child(
                        alert(
                            "containers-stale",
                            format!("{error} Showing the last reading."),
                            cx,
                        )
                        .mb(px(12.)),
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
                v.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .flex()
                        .when(!compact || self.selected.is_none(), |v| {
                            v.child(self.list(cx))
                        })
                        .when(!compact || self.selected.is_some(), |v| {
                            v.child(self.detail(window, cx))
                        }),
                )
            })
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
fn master_rows(
    all: &[Container],
    query: &str,
    collapsed: &HashSet<String>,
    jobs_open: &HashSet<String>,
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
    let section = |name: String| Row {
        key: None,
        label: name.clone(),
        name,
        note: String::new(),
        clock: None,
        tone: domain::Tone::Muted,
        indent: 8.,
        group: true,
        project: None,
        jobs: false,
        oneoff: false,
        open: true,
    };
    if !groups.projects.is_empty() {
        rows.push(section(format!("Projects  {}", groups.projects.len())));
    }
    for project in groups.projects {
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
                format!("{}/{}", project.running, project.services.len())
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
        });
        if open {
            rows.extend(project.services.iter().map(|c| container_row(c, 30., now)));
            if !project.jobs.is_empty() {
                let jobs_visible = !needle.is_empty() || jobs_open.contains(project.name);
                rows.push(Row {
                    key: None,
                    name: format!(
                        "{} one-off {}",
                        project.jobs.len(),
                        if project.jobs.len() == 1 {
                            "job"
                        } else {
                            "jobs"
                        }
                    ),
                    note: String::new(),
                    clock: None,
                    tone: domain::Tone::Muted,
                    label: String::new(),
                    indent: 30.,
                    group: false,
                    project: Some(project.name.to_owned()),
                    jobs: true,
                    oneoff: false,
                    open: jobs_visible,
                });
                if jobs_visible {
                    rows.extend(project.jobs.iter().map(|c| container_row(c, 44., now)));
                }
            }
        }
    }
    if !groups.standalone.is_empty() {
        rows.push(section(format!("Standalone  {}", groups.standalone.len())));
        rows.extend(groups.standalone.iter().map(|c| container_row(c, 8., now)));
    }
    if rows.is_empty() {
        let mut row = section(
            if needle.is_empty() {
                "No containers on this server yet."
            } else {
                "No projects or containers match."
            }
            .into(),
        );
        row.group = false;
        rows.push(row);
    }
    rows.insert(
        0,
        Row {
            key: Some(domain::DISK.into()),
            name: "Disk usage".into(),
            label: "Disk usage".into(),
            note: String::new(),
            clock: None,
            tone: domain::Tone::Muted,
            indent: 8.,
            group: false,
            project: None,
            jobs: false,
            oneoff: false,
            open: false,
        },
    );
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
    }
}
fn heading(value: impl Into<SharedString>, level: u8) -> Stateful<Div> {
    let value = value.into();
    div()
        .id(SharedString::from(format!("container-heading-{value}")))
        .role(accesskit::Role::Heading)
        .aria_level(level as usize)
        .aria_label(value.clone())
        .text_size(px(if level == 1 { 18. } else { 17. }))
        .font_weight(FontWeight::SEMIBOLD)
        .child(value)
}
fn quiet(value: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_size(px(12.))
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
fn alert(id: impl Into<SharedString>, value: impl Into<SharedString>, cx: &App) -> Stateful<Div> {
    let id = id.into();
    let value = value.into();
    div()
        .id(id.clone())
        .debug_selector(move || id.to_string())
        .role(accesskit::Role::Alert)
        .aria_label(value.clone())
        .text_size(px(12.))
        .text_color(cx.theme().danger)
        .child(value)
}
fn tone_color(tone: domain::Tone, cx: &App) -> Hsla {
    match tone {
        domain::Tone::Success => theme::success(cx.theme().is_dark()),
        domain::Tone::Warning => theme::warning(cx.theme().is_dark()),
        domain::Tone::Danger => theme::error(cx.theme().is_dark()),
        domain::Tone::Muted => cx.theme().muted_foreground,
    }
}
fn dot(tone: domain::Tone, cx: &App) -> Div {
    div()
        .size(px(7.))
        .flex_shrink_0()
        .rounded_full()
        .when(tone != domain::Tone::Muted, |v| v.bg(tone_color(tone, cx)))
        .when(tone == domain::Tone::Muted, |v| {
            v.border(px(1.5))
                .border_color(cx.theme().muted_foreground.opacity(0.4))
        })
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
