//! Services presentation. The retained client owns RPC, polling and mutations.
use super::*;
use crate::controls::alert;
use gpui_kit::component::{input::InputEvent, skeleton::Skeleton};
use newport_client::services::{LogTarget, Services};
use newport_domain::services::{
    self as domain, Privilege, Scope, ServiceUnit, ServiceVerb, ServicesSnapshot, TimerUnit, Tone,
};
use std::rc::Rc;

#[path = "services_detail.rs"]
mod detail;
#[cfg(test)]
#[path = "services_detail_tests.rs"]
mod detail_tests;
#[path = "services_logs.rs"]
mod logs;
#[cfg(test)]
#[path = "services_selection_tests.rs"]
mod selection_tests;
#[cfg(test)]
#[path = "services_tests.rs"]
pub(crate) mod tests;

pub(super) enum ServicesEvent {
    OpenTerminal { command: String },
    Notification { message: String },
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum Selection {
    Journal,
    Unit(String),
}
/// Presentation choices retained in memory for a saved server. Remote data,
/// operation state and credentials always belong to the current client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Preferences {
    query: String,
    selected: Option<Selection>,
    system_open: bool,
    collapsed: std::collections::BTreeSet<String>,
    tab: usize,
    log_level: String,
    live: bool,
    log_query: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum ActiveData {
    None,
    Logs(LogTarget, Option<String>),
    File(String, Scope),
}
#[derive(Clone)]
struct Row {
    key: Option<Selection>,
    name: String,
    note: String,
    clock: Option<RowClock>,
    tone: Tone,
    user: bool,
    heading: bool,
    count: usize,
    reveal: bool,
    /// For headings: whether the group's rows follow.
    open: bool,
    accessible: String,
}
#[derive(Clone)]
enum RowClock {
    Since(i64),
    Timer(TimerUnit),
}
#[derive(Default)]
struct ListCache {
    key: Option<(u64, String, bool, std::collections::BTreeSet<String>)>,
    rows: Rc<Vec<Row>>,
    fallback: Option<Selection>,
}
impl Row {
    fn accessible_at(&self, now: i64) -> String {
        match &self.clock {
            Some(RowClock::Timer(timer)) => format!(
                "{}{}: timer, {}",
                timer.name,
                if timer.scope == Scope::User {
                    " (user)"
                } else {
                    ""
                },
                self.note_at(now)
            ),
            _ => self.accessible.clone(),
        }
    }
    fn note_at(&self, now: i64) -> String {
        match &self.clock {
            Some(RowClock::Since(since)) => {
                format!("up {}", domain::duration(now.saturating_sub(*since)))
            }
            Some(RowClock::Timer(timer)) => domain::timer_note(timer, now),
            None => self.note.clone(),
        }
    }

    fn heading(name: &str, count: usize, open: bool) -> Self {
        Self {
            key: None,
            name: name.into(),
            note: String::new(),
            clock: None,
            tone: Tone::Muted,
            user: false,
            heading: true,
            count,
            reveal: false,
            open,
            accessible: format!("{name} {count}"),
        }
    }
}
#[derive(Clone)]
struct Review {
    unit: String,
    scope: Scope,
    verb: ServiceVerb,
}
/// The open Stop/Disable confirmation. Its Kit identity is captured so a
/// later reconciliation can never dismiss a different dialog.
struct ReviewDialog {
    token: u64,
    window: AnyWindowHandle,
    handle: gpui_kit::component::DialogHandle,
}

pub(super) struct ServicesPanel {
    connection: Value,
    client: Services,
    search: Entity<InputState>,
    selected: Option<Selection>,
    reveal_restored_selection: bool,
    system_open: bool,
    /// Folded list groups by heading name; a search shows every group.
    collapsed: std::collections::BTreeSet<String>,
    tab: usize,
    width: f32,
    detail_tab_focus: [FocusHandle; 2],
    list_scroll: UniformListScrollHandle,
    list_focus: FocusHandle,
    detail_focus: FocusHandle,
    detail_scroll: ScrollHandle,
    logs: logs::Logs,
    review: Option<Review>,
    review_dialog: Option<ReviewDialog>,
    review_token: u64,
    review_cancel_focus: FocusHandle,
    review_confirm_focus: FocusHandle,
    list_cache: std::cell::RefCell<ListCache>,
    snapshot_revision: u64,
    presentation_pending: bool,
    clock: crate::presentation_time::Clock,
    retired: bool,
    active_data: ActiveData,
    _subscriptions: Vec<Subscription>,
}
impl EventEmitter<ServicesEvent> for ServicesPanel {}

impl ServicesPanel {
    pub(super) fn new(
        server: String,
        connection: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut panel = Self::with_client(
            Services::new(backend::Backend::discover(), server),
            window,
            cx,
        );
        panel.connection = connection;
        panel
    }
    pub(crate) fn with_client(
        mut client: Services,
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
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Find a service or timer"));
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
            search,
            selected: None,
            reveal_restored_selection: false,
            system_open: false,
            collapsed: Default::default(),
            tab: 0,
            width: 800.,
            detail_tab_focus: std::array::from_fn(|_| cx.focus_handle()),
            list_scroll: UniformListScrollHandle::new(),
            list_focus: cx.focus_handle(),
            detail_focus: cx.focus_handle(),
            detail_scroll: ScrollHandle::new(),
            logs: logs::Logs::new(window, cx),
            review: None,
            review_dialog: None,
            review_token: 0,
            review_cancel_focus: cx.focus_handle(),
            review_confirm_focus: cx.focus_handle(),
            list_cache: Default::default(),
            snapshot_revision: 0,
            presentation_pending: false,
            clock: Default::default(),
            retired: false,
            active_data: ActiveData::None,
            _subscriptions: vec![subscription, connection_subscription],
        }
    }
    pub(super) fn has_pending(&self) -> bool {
        self.client.has_pending()
    }
    pub(super) fn preferences(&self, cx: &App) -> Preferences {
        Preferences {
            query: self.search.read(cx).value().to_string(),
            selected: self.selected.clone(),
            system_open: self.system_open,
            collapsed: self.collapsed.clone(),
            tab: self.tab,
            log_level: self.logs.priority().unwrap_or_else(|| "all".into()),
            live: self.logs.live,
            log_query: self.logs.find.read(cx).value().to_string(),
        }
    }
    pub(super) fn restore_preferences(
        &mut self,
        preferences: Preferences,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search.update(cx, |search, cx| {
            search.set_value(preferences.query, window, cx)
        });
        self.selected = preferences.selected;
        self.reveal_restored_selection = true;
        self.system_open = preferences.system_open;
        self.collapsed = preferences.collapsed;
        self.tab = preferences.tab.min(1);
        self.logs.restore_preferences(
            preferences.log_level,
            preferences.live,
            preferences.log_query,
            window,
            cx,
        );
        cx.notify();
    }
    #[cfg(test)]
    pub(crate) fn test_action(
        &mut self,
        unit: &str,
        scope: Scope,
        verb: ServiceVerb,
        cx: &mut Context<Self>,
    ) {
        self.start_action(unit.into(), scope, verb, cx);
    }
    pub(super) fn retire(&mut self, cx: &mut Context<Self>) {
        self.retired = true;
        self.client.retire();
        self.review = None;
        self.sync_review_dialog(cx);
        cx.notify();
    }
    fn review_dialog_current(&self, token: u64) -> bool {
        self.review_dialog
            .as_ref()
            .is_some_and(|dialog| dialog.token == token && dialog.handle.is_active())
    }
    /// Close the confirmation once its review is settled elsewhere: a
    /// completed action, a new selection or retirement.
    fn sync_review_dialog(&mut self, cx: &mut Context<Self>) {
        if self.review_dialog.is_some() && (self.retired || self.review.is_none()) {
            let dialog = self.review_dialog.take().unwrap();
            // Polling and retirement have no Window. The captured Kit identity
            // can never dismiss a later dialog.
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
        let changed = self.client.poll(visible);
        let clock_changed = visible && self.clock.changed(chrono::Local::now().timestamp_millis());
        if !changed && !(visible && self.presentation_pending) {
            if clock_changed {
                cx.notify();
            }
            return;
        }
        // Retain completed reads and deliver action receipts while hidden.
        // Repaint once on return, even if the next query observation is unchanged.
        self.presentation_pending = !visible;
        let revision = self.client.resource("snapshot").map_or(0, |r| r.revision);
        if revision != self.snapshot_revision {
            self.snapshot_revision = revision;
        }
        for completion in self.client.take_action_completions() {
            if let Ok(result) = completion.result {
                if (result.ok || result.needs_password == Some(true))
                    && self.review.as_ref().is_some_and(|review| {
                        review.unit == completion.unit
                            && review.scope == completion.scope
                            && review.verb == completion.verb
                    })
                {
                    self.review = None;
                }
                if result.ok && !self.retired {
                    let prefix = match completion.verb {
                        ServiceVerb::Start => "Started",
                        ServiceVerb::Stop => "Stopped",
                        ServiceVerb::Restart => "Restarted",
                        ServiceVerb::Reload => "Reloaded",
                        ServiceVerb::Enable => "Will start at boot:",
                        ServiceVerb::Disable => "Won’t start at boot:",
                    };
                    cx.emit(ServicesEvent::Notification {
                        message: format!("{prefix} {}", domain::short_name(&completion.unit)),
                    });
                }
            }
        }
        self.sync_review_dialog(cx);
        // Completing a hidden snapshot must not select a detail pane and start
        // new log/file reads. Reconcile once on return using the retained data.
        if !self.retired && visible {
            self.sync_selection(cx);
            // A restored row has no geometry until its first snapshot arrives.
            // Reveal it once; periodic reads must preserve manual scrolling.
            if self.reveal_restored_selection && self.client.snapshot().is_some() {
                self.reveal_restored_selection = false;
                self.reveal_selection(&self.effective_selection(cx), cx);
            }
        }
        if visible {
            cx.notify();
        }
    }
    fn pending(&self, key: &str) -> bool {
        self.client
            .resource(key)
            .is_some_and(|r| r.display_pending())
    }
    fn error(&self, key: &str) -> Option<String> {
        self.client.resource(key).and_then(|r| r.error.clone())
    }
    fn prepared_list(&self, cx: &App) -> (Rc<Vec<Row>>, Selection) {
        let key = (
            self.client
                .resource("snapshot")
                .map_or(0, |r| r.data_revision),
            self.search.read(cx).value().trim().to_lowercase(),
            self.system_open,
            self.collapsed.clone(),
        );
        let mut cache = self.list_cache.borrow_mut();
        if cache.key.as_ref() != Some(&key) {
            let (rows, fallback) = self
                .client
                .snapshot()
                .map(|snapshot| {
                    let now = chrono::Local::now().timestamp_millis();
                    // The default follows what the filter shows, not what's
                    // folded, so folding a group never changes the detail.
                    let fallback = master_rows(snapshot, &key.1, key.2, &Default::default(), now)
                        .into_iter()
                        .find_map(|row| row.key)
                        .unwrap_or(Selection::Journal);
                    (master_rows(snapshot, &key.1, key.2, &key.3, now), fallback)
                })
                .unwrap_or_else(|| (vec![], Selection::Journal));
            cache.rows = Rc::new(rows);
            cache.fallback = Some(fallback);
            cache.key = Some(key);
        }
        (
            cache.rows.clone(),
            cache.fallback.clone().unwrap_or(Selection::Journal),
        )
    }
    fn rows(&self, cx: &App) -> Rc<Vec<Row>> {
        self.prepared_list(cx).0
    }
    fn effective_selection(&self, cx: &App) -> Selection {
        self.selected
            .clone()
            .unwrap_or_else(|| self.prepared_list(cx).1)
    }
    fn reveal_selection(&self, selection: &Selection, cx: &App) {
        if let Some(index) = self
            .rows(cx)
            .iter()
            .position(|row| row.key.as_ref() == Some(selection))
        {
            self.list_scroll
                .scroll_to_item(index, ScrollStrategy::Nearest);
        }
    }
    fn select(&mut self, selection: Selection, cx: &mut Context<Self>) {
        self.reveal_restored_selection = false;
        self.reveal_selection(&selection, cx);
        self.selected = Some(selection);
        self.review = None;
        self.sync_review_dialog(cx);
        self.detail_scroll.set_offset(point(px(0.), px(0.)));
        self.sync_selection(cx);
        cx.notify();
    }
    fn sync_selection(&mut self, cx: &mut Context<Self>) {
        let selection = self.effective_selection(cx);
        let chosen = self
            .client
            .snapshot()
            .and_then(|snapshot| match &selection {
                Selection::Unit(key) => snapshot
                    .services
                    .iter()
                    .find(|u| u.key() == *key)
                    .map(|u| (u.name.clone(), u.scope, Some(u.name.clone())))
                    .or_else(|| {
                        snapshot
                            .timers
                            .iter()
                            .find(|t| t.key() == *key)
                            .map(|t| (t.name.clone(), t.scope, t.unit.clone()))
                    }),
                _ => None,
            });
        let next = if self.client.snapshot().is_none() {
            ActiveData::None
        } else if let Some((name, scope, target)) = chosen {
            if self.tab == 1 || target.is_none() {
                ActiveData::File(name, scope)
            } else {
                ActiveData::Logs(
                    LogTarget {
                        unit: target,
                        scope,
                    },
                    self.logs.priority(),
                )
            }
        } else if selection == Selection::Journal {
            ActiveData::Logs(
                LogTarget {
                    unit: None,
                    scope: Scope::System,
                },
                self.logs.priority(),
            )
        } else {
            ActiveData::None
        };
        // A priority change keeps the same target's placeholder. Changing the
        // active pane stops its old polling without repeatedly clearing caches.
        if std::mem::discriminant(&next) != std::mem::discriminant(&self.active_data) {
            self.client.clear_selection();
        }
        match &next {
            ActiveData::Logs(target, priority) => {
                self.logs
                    .change_dataset(Some((target.clone(), priority.clone())), cx);
                self.client
                    .select_logs(target.clone(), priority.clone(), self.logs.live);
            }
            ActiveData::File(name, scope) => {
                self.logs.change_dataset(None, cx);
                self.client.select_unit_file(name.clone(), *scope);
            }
            ActiveData::None => {}
        }
        self.active_data = next;
        self.logs.sync(&self.client, cx);
    }
    fn select_from_list(
        &mut self,
        selection: Selection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select(selection, cx);
        // The compact layout removes the master list. Keep keyboard focus in
        // the visible pane so Tab reaches its Back button and detail controls.
        if self.width <= 600. {
            self.detail_focus.focus(window, cx);
        } else {
            self.list_focus.focus(window, cx);
        }
    }
    fn navigate(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.modifiers.modified() {
            return;
        }
        let rows = self.rows(cx);
        let keys: Vec<_> = rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.key.clone().map(|k| (i, k)))
            .collect();
        if keys.is_empty() {
            return;
        }
        let selected = self.effective_selection(cx);
        let current = keys.iter().position(|(_, k)| *k == selected).unwrap_or(0);
        let next = match event.keystroke.key.as_str() {
            "down" => (current + 1).min(keys.len() - 1),
            "up" => current.saturating_sub(1),
            "home" => 0,
            "end" => keys.len() - 1,
            _ => return,
        };
        self.list_scroll
            .scroll_to_item(keys[next].0, ScrollStrategy::Center);
        self.select_from_list(keys[next].1.clone(), window, cx);
        cx.stop_propagation();
    }
    fn start_action(
        &mut self,
        unit: String,
        scope: Scope,
        verb: ServiceVerb,
        cx: &mut Context<Self>,
    ) {
        self.client.action(unit, scope, verb);
        cx.notify();
    }
    fn toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::connections_selector::{Choice, Selector};
        let (yours, total) = self.client.snapshot().map_or((0, 0), |snapshot| {
            let groups = domain::group_services(&snapshot.services);
            (
                groups.problems.len() + groups.owned.len() + snapshot.timers.len(),
                snapshot.services.len() + snapshot.timers.len(),
            )
        });
        let owner = cx.weak_entity();
        let journal = self.effective_selection(cx) == Selection::Journal;
        crate::controls::page_toolbar(cx)
            .id("services-toolbar")
            .debug_selector(|| "services-toolbar".into())
            .child(crate::controls::page_search(
                &self.search,
                "Find a service",
                cx,
            ))
            .child(Selector::new(
                "services-filter",
                "Show",
                usize::from(self.system_open),
                vec![
                    Choice {
                        count: Some(yours),
                        tooltip: Some(
                            "Your services, timers, and any service that needs attention",
                        ),
                        ..Choice::text(0, "Yours")
                    },
                    Choice {
                        count: Some(total),
                        tooltip: Some("Include the services the system ships with"),
                        ..Choice::text(1, "All")
                    },
                ],
                move |index, _, cx| {
                    let _ = owner.update(cx, |this, cx| {
                        this.system_open = index == 1;
                        cx.notify();
                    });
                },
            ))
            .child(div().flex_1())
            .child(
                crate::controls::button("services-journal")
                    .ghost()
                    .icon(IconName::ScrollText)
                    .label("System journal")
                    .selected(journal)
                    .toggled(journal)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.select_from_list(Selection::Journal, window, cx)
                    })),
            )
            .child(
                crate::controls::icon_button("services-refresh")
                    .ghost()
                    .icon(IconName::RefreshCw)
                    .tooltip("Refresh")
                    .accessibility_label("Refresh services")
                    .loading(self.pending("snapshot"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.client.refresh();
                        // Logs and unit files have no refresh of their own.
                        match this.active_data {
                            ActiveData::Logs(..) => this.client.retry_logs(),
                            ActiveData::File(..) => this.client.retry_unit_file(),
                            ActiveData::None => {}
                        }
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
    fn status_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(snapshot) = self.client.snapshot() else {
            return div().into_any_element();
        };
        let running = snapshot
            .services
            .iter()
            .filter(|u| u.active == "active" && u.sub == "running")
            .count();
        let failed = snapshot
            .services
            .iter()
            .filter(|u| u.active == "failed")
            .count();
        let timers = snapshot.timers.len();
        let summary = format!(
            "{running} running · {failed} failed · {timers} {}",
            if timers == 1 { "timer" } else { "timers" }
        );
        // Who may change what; the full explanation is a hover away.
        let (access, explanation) = match snapshot.privilege {
            Privilege::Root => (
                "root",
                "Newport signs in as root and can change any service.",
            ),
            Privilege::Sudo => (
                "sudo",
                "System services change through sudo without a password.",
            ),
            Privilege::Password => (
                "sudo needs your password",
                "System services need your sudo password to change; those actions open in the Terminal. Your user services change directly.",
            ),
            Privilege::None => (
                "system services read-only",
                "This account can’t change system services. Your user services change directly.",
            ),
        };
        let user = (!snapshot.user.available).then(|| {
            snapshot
                .user
                .reason
                .clone()
                .filter(|reason| !reason.is_empty())
                .map_or_else(
                    || "User services aren’t shown.".to_owned(),
                    |reason| format!("User services aren’t shown: {reason}"),
                )
        });
        div()
            .id("services-status")
            .debug_selector(|| "services-status".into())
            .flex_shrink_0()
            .child(
                crate::controls::status_bar(cx)
                    .left(
                        div()
                            .id("services-summary")
                            .role(accesskit::Role::Status)
                            .aria_label(summary.clone())
                            .child(summary),
                    )
                    .right(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .when_some(user, |v, reason| {
                                v.child(
                                    div()
                                        .id("services-user-unavailable")
                                        .aria_label(reason.clone())
                                        .hover_hint(reason)
                                        .child("no user services ·"),
                                )
                            })
                            .child(
                                div()
                                    .id("services-privilege")
                                    .aria_label(explanation)
                                    .hover_hint(explanation)
                                    .child(access),
                            ),
                    ),
            )
            .into_any_element()
    }
    fn list(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows = self.rows(cx);
        let selected = self.effective_selection(cx);
        let initial = self.client.snapshot().is_none();
        div()
            .id("services-master")
            .debug_selector(|| "services-master".into())
            .role(accesskit::Role::Region)
            .aria_label("Service list")
            .flex()
            .flex_col()
            .min_h_0()
            .min_w_0()
            .h_full()
            .py(px(6.))
            // Beside the detail, the resizable split owns width and divider.
            .when(self.width > 600., |v| v.size_full())
            .when(self.width <= 600., |v| v.flex_1())
            .when(initial, |v| {
                v.child(div().px(px(14.)).child(list_skeleton()))
            })
            .when(!initial, |v| {
                v.child(
                    div()
                        .id("services-listbox")
                        .flex()
                        .flex_col()
                        .role(accesskit::Role::ListBox)
                        .aria_label("Services")
                        .track_focus(&self.list_focus)
                        .focus_visible(|style| crate::controls::inset_focus_ring(style, cx))
                        .on_key_down(cx.listener(Self::navigate))
                        .flex_1()
                        .min_h_0()
                        .child(
                            uniform_list(
                                "services-rows",
                                rows.len(),
                                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                                    range
                                        .map(|i| this.master_row(i, &rows[i], &selected, cx))
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
        selected: &Selection,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if row.heading {
            let name = row.name.clone();
            return div()
                .h(px(28.))
                .pt(px(4.))
                .child(
                    crate::controls::group_heading(
                        SharedString::from(format!("services-group-{}", name.to_lowercase())),
                        &row.name,
                        row.count,
                        row.open,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.collapsed.remove(&name) {
                            this.collapsed.insert(name.clone());
                        }
                        cx.notify();
                    })),
                )
                .into_any_element();
        }
        if row.reveal {
            return div()
                .id("services-system-hidden")
                .role(accesskit::Role::Button)
                .aria_label(row.accessible.clone())
                .test_support()
                .h(px(28.))
                .pl(px(14.))
                .pr(px(8.))
                .flex()
                .items_center()
                .gap(px(6.))
                .cursor_pointer()
                .text_size(px(crate::ui::FONT_SIZE))
                .text_color(cx.theme().muted_foreground)
                .hover(|v| v.bg(theme::row_hover(cx)).text_color(cx.theme().foreground))
                .child(div().flex_1().min_w_0().truncate().child(row.name.clone()))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.system_open = true;
                    cx.notify();
                }))
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
        let now = self.clock.now();
        let note = match &row.clock {
            Some(RowClock::Since(at)) => format!("up {}", self.clock.elapsed(*at)),
            Some(RowClock::Timer(timer)) => self.clock.timer(timer),
            None => row.note.clone(),
        };
        let active = selected == &key;
        let name = row.name.clone();
        let timer = matches!(row.clock, Some(RowClock::Timer(_)));
        let id = match &key {
            Selection::Journal => "services-option-journal".to_owned(),
            Selection::Unit(key) => format!("services-option-{key}"),
        };
        div()
            .id(SharedString::from(id))
            .debug_selector(move || format!("services-row-{index}"))
            .role(accesskit::Role::ListBoxOption)
            .aria_label(row.accessible_at(now))
            .aria_selected(active)
            .when(active, |v| v.aria_active_descendant())
            .h(px(28.))
            .w_full()
            .pl(px(14.))
            .pr(px(8.))
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(crate::ui::FONT_SIZE))
            .cursor_pointer()
            .when(active, |v| v.bg(theme::selected_fill(cx.theme().is_dark())))
            .when(!active, |v| v.hover(|v| v.bg(theme::row_hover(cx))))
            .child(if timer {
                Icon::new(IconName::Clock)
                    .size(px(crate::ui::SMALL_ICON))
                    .text_color(cx.theme().muted_foreground)
                    .into_any_element()
            } else {
                crate::controls::status_dot(
                    (row.tone != Tone::Muted).then(|| tone_color(row.tone, cx)),
                    cx,
                )
                .into_any_element()
            })
            .child(div().flex_1().min_w_0().truncate().child(name))
            .when(row.user, |v| v.child(crate::controls::tag("user", cx)))
            .when(!note.is_empty(), |v| {
                v.child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(crate::ui::FONT_SIZE))
                        .text_color(tone_color(
                            if matches!(row.tone, Tone::Danger | Tone::Warning) {
                                row.tone
                            } else {
                                Tone::Muted
                            },
                            cx,
                        ))
                        .child(note),
                )
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select_from_list(key.clone(), window, cx);
            }))
            .into_any_element()
    }
}
impl Render for ServicesPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.clock
            .begin_frame(chrono::Local::now().timestamp_millis());
        self.sync_review_dialog(cx);
        let editor_identity = match &self.active_data {
            ActiveData::File(name, scope) => Some(domain::unit_key(*scope, name)),
            _ => None,
        };
        self.logs
            .sync_editor(&self.client, editor_identity, window, cx);
        let snapshot = self.client.snapshot();
        let problem = self.error("snapshot");
        let initial = snapshot.is_none() && problem.is_none();
        let owner = cx.weak_entity();
        let compact = self.width <= 600.;
        let show_detail = !compact || self.selected.is_some();
        let show_list = !compact || self.selected.is_none();
        div()
            .id("services-page")
            .debug_selector(|| "services-page".into())
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
                                this.width = width;
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
            .when(snapshot.is_none() && problem.is_some(), |v| {
                v.child(
                    crate::empty::pane(
                        IconName::ServerOff,
                        "Services aren’t available here",
                        problem.clone().unwrap_or_default(),
                        cx,
                    )
                    .child(
                        crate::controls::button("services-retry")
                            .label("Try again")
                            .loading(self.pending("snapshot"))
                            .disabled(self.pending("snapshot"))
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
                )
            })
            .when(snapshot.is_some() || initial, |v| v.child(self.toolbar(cx)))
            .when(snapshot.is_some(), |v| {
                v.when_some(problem.clone(), |v, error| {
                    v.child(
                        alert(
                            "services-stale",
                            format!("{error} Showing the last reading."),
                            cx,
                        )
                        .flex_shrink_0()
                        .px(px(crate::controls::TOOLBAR_INSET))
                        .py(px(8.)),
                    )
                })
            })
            .when(snapshot.is_some() || initial, |v| {
                v.child(div().flex_1().min_h_0().min_w_0().flex().map(|v| {
                    if compact {
                        v.when(show_list, |v| v.child(self.list(cx)))
                            .when(show_detail, |v| v.child(self.detail(cx)))
                    } else {
                        let key = SharedString::from("services-split");
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
                                        .child(self.detail(cx)),
                                ),
                        )
                    }
                }))
            })
            .when(snapshot.is_some(), |v| v.child(self.status_bar(cx)))
    }
}
/// Problems, your services and timers. System services follow under "All",
/// or behind one row that reveals them; a search always includes them.
fn master_rows(
    snapshot: &ServicesSnapshot,
    query: &str,
    system_open: bool,
    collapsed: &std::collections::BTreeSet<String>,
    now: i64,
) -> Vec<Row> {
    let needle = query.trim().to_lowercase();
    let services: Vec<_> = snapshot
        .services
        .iter()
        .filter(|u| {
            format!("{} {} {} {}", u.name, u.description, u.active, u.sub)
                .to_lowercase()
                .contains(&needle)
        })
        .cloned()
        .collect();
    let groups = domain::group_services(&services);
    let open = |title: &str| !needle.is_empty() || !collapsed.contains(title);
    let mut rows = vec![];
    for (title, units) in [("Problems", &groups.problems), ("Services", &groups.owned)] {
        if !units.is_empty() {
            rows.push(Row::heading(title, units.len(), open(title)));
            if open(title) {
                rows.extend(units.iter().map(|u| service_row(u, now)));
            }
        }
    }
    let mut timers: Vec<_> = snapshot
        .timers
        .iter()
        .filter(|u| {
            format!("{} {} {}", u.name, u.description, u.active)
                .to_lowercase()
                .contains(&needle)
        })
        .collect();
    timers.sort_by_key(|t| t.next.unwrap_or(i64::MAX));
    if !timers.is_empty() {
        rows.push(Row::heading("Timers", timers.len(), open("Timers")));
        rows.extend(timers.iter().filter(|_| open("Timers")).map(|t| Row {
            key: Some(Selection::Unit(t.key())),
            name: domain::short_name(&t.name).into(),
            note: domain::timer_note(t, now),
            clock: Some(RowClock::Timer((*t).clone())),
            tone: if t.active == "active" {
                Tone::Success
            } else {
                Tone::Muted
            },
            user: t.scope == Scope::User,
            heading: false,
            count: 0,
            reveal: false,
            open: true,
            accessible: format!(
                "{}{}: timer, {}",
                t.name,
                if t.scope == Scope::User {
                    " (user)"
                } else {
                    ""
                },
                domain::timer_note(t, now)
            ),
        }));
    }
    if !groups.system.is_empty() {
        if system_open || !needle.is_empty() {
            rows.push(Row::heading("System", groups.system.len(), open("System")));
            if open("System") {
                rows.extend(groups.system.iter().map(|u| service_row(u, now)));
            }
        } else {
            let count = groups.system.len();
            let name = format!(
                "{count} system {} · Show all",
                if count == 1 { "service" } else { "services" }
            );
            rows.push(Row {
                key: None,
                name: name.clone(),
                note: String::new(),
                clock: None,
                tone: Tone::Muted,
                user: false,
                heading: false,
                count,
                reveal: true,
                open: true,
                accessible: name,
            });
        }
    }
    if services.is_empty() && timers.is_empty() {
        let name = if needle.is_empty() {
            "No services or timers here."
        } else {
            "No services or timers match."
        };
        rows.push(Row {
            key: None,
            name: name.into(),
            note: String::new(),
            clock: None,
            tone: Tone::Muted,
            user: false,
            heading: false,
            count: 0,
            reveal: false,
            open: true,
            accessible: name.into(),
        });
    }
    rows
}
fn service_row(unit: &ServiceUnit, now: i64) -> Row {
    Row {
        key: Some(Selection::Unit(unit.key())),
        name: domain::short_name(&unit.name).into(),
        note: domain::list_note(unit, now),
        clock: (unit.active == "active" && unit.sub == "running" && !domain::is_problem(unit))
            .then_some(unit.since)
            .flatten()
            .map(RowClock::Since),
        tone: domain::state_of(unit).tone,
        user: unit.scope == Scope::User,
        heading: false,
        count: 0,
        reveal: false,
        open: true,
        accessible: format!(
            "{}{}: {}",
            unit.name,
            if unit.scope == Scope::User {
                " (user)"
            } else {
                ""
            },
            domain::state_of(unit).label
        ),
    }
}
fn tone_color(tone: Tone, cx: &App) -> Hsla {
    match tone {
        Tone::Success => theme::success(cx.theme().is_dark()),
        Tone::Warning => theme::warning(cx.theme().is_dark()),
        Tone::Danger => theme::error(cx.theme().is_dark()),
        _ => cx.theme().muted_foreground,
    }
}
fn quiet(value: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_size(px(crate::ui::FONT_SIZE))
        .text_color(cx.theme().muted_foreground)
        .child(value.into())
}
fn list_skeleton() -> AnyElement {
    div()
        .id("services-list-loading")
        .role(accesskit::Role::Status)
        .aria_label("Reading services")
        .px(px(8.))
        .children((0..12).map(|i| {
            div()
                .h(px(28.))
                .flex()
                .items_center()
                .gap(px(8.))
                .child(Skeleton::new().size(px(7.)).rounded_full())
                .child(
                    Skeleton::new()
                        .h(px(10.))
                        .w(relative([0.46, 0.38, 0.58, 0.52][i % 4]))
                        .rounded(px(crate::ui::RADIUS)),
                )
        }))
        .into_any_element()
}
