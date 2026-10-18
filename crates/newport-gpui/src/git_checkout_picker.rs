//! Bounded worktree navigation, independent of the active workspace tab.
use super::*;
use gpui_kit::component::popover::Popover;
pub(super) const SUMMARY: &str = "git-checkout-summary";
/// Toolbar context survives menu filtering and tab changes, but not a checkout change.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Summary {
    metadata: Value,
}
impl Summary {
    pub(super) fn accept(&mut self, page: &Value) {
        if page["metadata"].is_object() {
            self.metadata = page["metadata"].clone();
        }
    }
    fn title(&self) -> Option<String> {
        let current = &self.metadata["current"];
        (current["current"] == true).then(|| git_worktree_view::row_labels(current).0)
    }
    fn count(&self) -> Option<u64> {
        self.metadata["totalEntries"]
            .as_u64()
            .filter(|total| *total > 1)
    }
}
const FIRST: &str = "git-checkouts";
const MORE: &str = "git-checkouts-more";
#[derive(Default)]
pub(super) struct Picker {
    pub(super) open: bool,
    pub(super) page: Value,
    pub(super) selected: Value,
    cursor: Option<String>,
    error: Option<String>,
    scroll: UniformListScrollHandle,
    input: Option<Entity<InputState>>,
    query: String,
    search_pending: Option<Instant>,
    _subscription: Option<Subscription>,
    menu: Option<WeakEntity<PopupMenu>>,
    highlighted: Option<usize>,
    keyboard_menu: Option<usize>,
}
impl Picker {
    fn near_scroll_end(&self) -> bool {
        git_page_demand::near_uniform_end(&self.scroll, rows(&self.page["entries"]).len(), 48.)
    }
    fn begin_search(&mut self, query: String) -> bool {
        if self.query == query {
            return false;
        }
        self.query = query;
        self.search_pending = Some(Instant::now());
        self.page = Value::Null;
        self.selected = Value::Null;
        self.highlighted = None;
        self.keyboard_menu = None;
        self.cursor = None;
        self.error = None;
        self.scroll = UniformListScrollHandle::default();
        true
    }
    fn selected_path(&self, entry: &Value) -> Option<Value> {
        self.contains(entry)
            .then(|| git::worktree_path(entry))
            .flatten()
    }
    pub(super) fn contains(&self, entry: &Value) -> bool {
        self.open
            && self.search_pending.is_none()
            && self.error.is_none()
            && rows(&self.page["entries"]).contains(entry)
    }
}
#[derive(Clone, Copy)]
enum CheckoutAction {
    Open,
    Files,
    Copy,
    Lock,
    Repair,
    Remove,
}
#[derive(Clone)]
struct MenuScope {
    server: String,
    repo: Value,
    snapshot: Value,
}
impl MenuScope {
    fn matches(&self, server: &str, repo: &Value, page: &Value) -> bool {
        self.server == server
            && self.repo.is_string()
            && self.repo == repo["repoId"]
            && self.snapshot.is_string()
            && self.snapshot == page["snapshot"]
    }
}
fn checkout_menu(
    mut menu: PopupMenu,
    owner: WeakEntity<Newport>,
    entry: Value,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(view) = owner.upgrade() else {
        return menu;
    };
    let this = view.read(cx);
    if !this.git.checkouts.contains(&entry) {
        return menu;
    }
    let busy = this.git_write_pending();
    let scope = MenuScope {
        server: this.server.clone(),
        repo: this.git.repo["repoId"].clone(),
        snapshot: this.git.checkouts.page["snapshot"].clone(),
    };
    for (title, action, enabled) in [
        (
            "Open worktree",
            CheckoutAction::Open,
            !busy && this.git.checkouts.selected_path(&entry).is_some(),
        ),
        (
            "Open in Files",
            CheckoutAction::Files,
            entry["path"]["display"].is_string(),
        ),
        (
            "Copy path",
            CheckoutAction::Copy,
            entry["path"]["display"].is_string(),
        ),
    ] {
        let owner = owner.clone();
        let entry = entry.clone();
        let scope = scope.clone();
        menu = menu.item(PopupMenuItem::new(title).disabled(!enabled).on_click(
            move |_, window, cx| {
                let _ = owner.update(cx, |this, cx| {
                    this.checkout_action(&scope, &entry, action, window, cx)
                });
            },
        ));
    }
    if newport_client::git_worktree::lock_target(&entry).is_some() {
        let ready = !busy
            && !this.git_mutation_blocks_write()
            && this.git.repo["capabilities"]["readOnly"] == false
            && this
                .resources
                .get(FIRST)
                .is_some_and(|r| !r.pending && r.error.is_none());
        menu = menu.separator();
        let mut actions = vec![(
            if entry["locked"] == true {
                "Unlock…"
            } else {
                "Lock…"
            },
            CheckoutAction::Lock,
        )];
        if entry["state"] == "missing" && entry["current"] == false {
            actions.push(("Locate moved worktree…", CheckoutAction::Repair));
        }
        if newport_client::git_worktree::removal_target(&entry).is_some() {
            actions.push((
                if entry["state"] == "missing" {
                    "Remove missing registration…"
                } else {
                    "Remove worktree…"
                },
                CheckoutAction::Remove,
            ));
        }
        for (title, action) in actions {
            let owner = owner.clone();
            let entry = entry.clone();
            let scope = scope.clone();
            menu = menu.item(PopupMenuItem::new(title).disabled(!ready).on_click(
                move |_, window, cx| {
                    let _ = owner.update(cx, |this, cx| {
                        this.checkout_action(&scope, &entry, action, window, cx)
                    });
                },
            ));
        }
    }
    let menu_view = cx.weak_entity();
    let menu_id = menu_view.entity_id();
    let observer = owner.clone();
    window
        .subscribe(&cx.entity(), cx, move |_, _: &DismissEvent, _, cx| {
            let _ = observer.update(cx, |this, cx| {
                if this
                    .git
                    .checkouts
                    .menu
                    .as_ref()
                    .is_some_and(|menu| menu.entity_id() == menu_id)
                {
                    this.git.checkouts.menu = None;
                    cx.notify();
                }
            });
        })
        .detach();
    cx.defer(move |cx| {
        let _ = owner.update(cx, |this, cx| {
            if this.git.checkouts.open
                && scope.matches(&this.server, &this.git.repo, &this.git.checkouts.page)
            {
                this.git.checkouts.menu = Some(menu_view);
                cx.notify();
            }
        });
    });
    menu
}
impl Newport {
    fn checkout_action(
        &mut self,
        scope: &MenuScope,
        entry: &Value,
        action: CheckoutAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !scope.matches(&self.server, &self.git.repo, &self.git.checkouts.page)
            || !self.git.checkouts.contains(entry)
            || self.project_dialog_open
            || self.git_worktree_review_open()
        {
            return;
        }
        match action {
            CheckoutAction::Open => {
                if !self.git_write_pending()
                    && let Some(path) = self.git.checkouts.selected_path(entry)
                {
                    self.git_open_worktree_path(path, cx);
                }
            }
            CheckoutAction::Files => {
                let project = json!({"serverId":self.server,"path":entry["path"]});
                self.close_checkout_picker();
                self.project_open_files(&project, window, cx);
            }
            CheckoutAction::Copy => {
                if let Some(path) = entry["path"]["display"].as_str() {
                    cx.write_to_clipboard(ClipboardItem::new_string(path.to_owned()));
                }
            }
            action => {
                self.git.checkouts.selected = entry.clone();
                match action {
                    CheckoutAction::Lock => self.review_worktree_lock(window, cx),
                    CheckoutAction::Repair => self.review_worktree_repair(window, cx),
                    CheckoutAction::Remove => self.review_worktree_removal(cx),
                    _ => unreachable!(),
                }
            }
        }
        cx.notify();
    }
    pub(super) fn refresh_checkout_picker(&mut self) {
        if !self.git.checkouts.open {
            self.load_checkout_summary();
            return;
        }
        self.resources.invalidate(FIRST);
        self.resources.invalidate(MORE);
        self.git.checkouts.search_pending = None;
        self.git.checkouts.cursor = None;
        self.git.checkouts.scroll = UniformListScrollHandle::default();
        self.load_checkouts(false);
    }
    pub(super) fn load_checkout_summary(&mut self) {
        if !self.git.repo["repoId"].is_string() {
            return;
        }
        self.resources.invalidate(SUMMARY);
        self.request(
            SUMMARY,
            "git_request",
            json!({"serverId":self.server,
            "request":{"method":"repo.worktrees","params":{
                "repoId":self.git.repo["repoId"],"pageSize":1
            }}}),
        );
    }
    pub(super) fn checkout_summary_completed(&mut self) {
        if self
            .resources
            .get(SUMMARY)
            .is_some_and(|read| read.error.is_none())
        {
            self.git.checkout_summary.accept(&self.value(SUMMARY));
        }
    }
    pub(super) fn close_checkout_picker(&mut self) {
        self.resources.invalidate(FIRST);
        self.resources.invalidate(MORE);
        self.git.checkouts = Picker::default();
    }
    fn open_checkout_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_checkout_picker();
        self.git_branch_picker_close();
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter by name, branch or path"));
        let subscription = cx.observe(&input, |this, input, cx| {
            if !this.git.checkouts.open || this.git.checkouts.input.as_ref() != Some(&input) {
                return;
            }
            let query = input.read(cx).value().trim().to_owned();
            if !this.git.checkouts.begin_search(query) {
                return;
            }
            this.resources.invalidate(FIRST);
            this.resources.invalidate(MORE);
            cx.notify();
        });
        crate::controls::focus_editor(&input, window, cx);
        self.git.checkouts = Picker {
            open: true,
            input: Some(input),
            _subscription: Some(subscription),
            ..Default::default()
        };
        self.load_checkouts(false);
    }
    pub(super) fn poll_checkout_search(&mut self) -> bool {
        if self.page != Page::Projects
            || !self.git.checkouts.open
            || !self
                .git
                .checkouts
                .search_pending
                .is_some_and(|time| time.elapsed() >= Duration::from_millis(200))
        {
            return false;
        }
        self.git.checkouts.search_pending = None;
        self.load_checkouts(false);
        true
    }
    pub(super) fn poll_checkout_scroll(&mut self) -> bool {
        let picker = &mut self.git.checkouts;
        // The list consumes wheel events; observe its committed position instead.
        let near_end = picker.near_scroll_end();
        let ready = self.page == Page::Projects
            && picker.open
            && !self.project_dialog_open
            && picker.search_pending.is_none()
            && picker.error.is_none()
            && picker.page["nextCursor"].is_string()
            && ![FIRST, MORE].iter().any(|key| {
                self.resources
                    .get(key)
                    .is_some_and(|read| read.pending || read.error.is_some())
            });
        if !ready || !near_end {
            return false;
        }
        self.load_checkouts(true);
        true
    }
    pub(super) fn checkout_picker_completed(&mut self, key: &str) {
        if !self.git.checkouts.open {
            return;
        }
        if self.resources.get(key).is_some_and(|r| r.error.is_some()) {
            self.git.checkouts.cursor = None;
            return;
        }
        let page = self.value(key);
        if key == FIRST {
            // The visible listing is newer than an outstanding toolbar read.
            self.resources.invalidate(SUMMARY);
            self.git.checkout_summary.accept(&page);
            self.git.checkouts.page = page;
            self.git.checkouts.highlighted = None;
            self.git.checkouts.keyboard_menu = None;
        } else if let Some(cursor) = self.git.checkouts.cursor.take() {
            match newport_client::git_page::merge(&self.git.checkouts.page, page, &cursor) {
                Ok(page) => self.git.checkouts.page = page,
                Err(error) => self.git.checkouts.error = Some(error),
            }
        }
    }
    fn load_checkouts(&mut self, more: bool) {
        let key = if more { MORE } else { FIRST };
        if !self.git.checkouts.open
            || self.git.checkouts.search_pending.is_some()
            || self.resources.get(key).is_some_and(|r| r.pending)
        {
            return;
        }
        let cursor = if more {
            self.git.checkouts.page["nextCursor"]
                .as_str()
                .map(str::to_owned)
        } else {
            None
        };
        if more && cursor.is_none() {
            return;
        }
        let mut params = json!({"repoId": self.git.repo["repoId"], "pageSize":50, "filter": self.git.checkouts.query});
        if let Some(cursor) = &cursor {
            params["cursor"] = json!(cursor);
        }
        self.git.checkouts.cursor = cursor;
        if !more {
            self.git.checkouts.error = None;
            self.git.checkouts.page = Value::Null;
            self.resources.invalidate(MORE);
        }
        self.resources.invalidate(key);
        self.request(
            key,
            "git_request",
            json!({"serverId":self.server,"request":{"method":"repo.worktrees","params":params}}),
        );
    }
    pub(super) fn checkout_picker_trigger(&self, cx: &mut Context<Self>) -> AnyElement {
        let label = self.git.checkout_summary.title().unwrap_or_else(|| {
            if self
                .selection
                .as_ref()
                .is_some_and(|p| p["path"]["bytesB64"] == self.git.opening_path["bytesB64"])
            {
                "Main worktree".into()
            } else {
                self.git.opening_path["display"]
                    .as_str()
                    .and_then(|s| s.trim_end_matches('/').rsplit('/').next())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Worktree")
                    .to_owned()
            }
        });
        let owner = cx.weak_entity();
        let content = owner.clone();
        Popover::new("git-checkout-picker")
            .trigger_style(git_layout::picker_container())
            .open(self.git.checkouts.open && !self.project_dialog_open)
            // Child menus extend beyond this surface. Keep the reviewed rows
            // alive until the child has dismissed and dispatched its action.
            .overlay_closable(
                self.git
                    .checkouts
                    .menu
                    .as_ref()
                    .and_then(WeakEntity::upgrade)
                    .is_none(),
            )
            .p_0()
            .offset(px(6.))
            .trigger(
                crate::project_ui::button("choose-worktree")
                    .ghost()
                    .w_full()
                    .min_w_0()
                    .px(px(8.))
                    .icon(IconName::FolderTree)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .min_w_0()
                            .text_size(px(crate::project_ui::FONT_SIZE))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(div().min_w_0().truncate().child(label.clone()))
                            .when_some(self.git.checkout_summary.count(), |view, count| {
                                view.child(
                                    div()
                                        .flex_shrink_0()
                                        .px(px(5.))
                                        .rounded_full()
                                        .bg(cx.theme().muted)
                                        .text_color(cx.theme().muted_foreground)
                                        .text_size(px(crate::project_ui::FONT_SIZE))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(count.to_string()),
                                )
                            }),
                    )
                    .accessibility_label(format!("Worktrees: {label}"))
                    .hover_path("Repository", text(&self.git.repo["root"], "display"))
                    .max_w(px(280.))
                    .dropdown_caret(true)
                    .disabled(
                        self.git_write_pending()
                            || self.project_dialog_open
                            || self.git.switching_path.is_some(),
                    ),
            )
            .on_open_change(move |open, window, cx| {
                let _ = owner.update(cx, |this, cx| {
                    if *open {
                        this.open_checkout_picker(window, cx);
                    } else if !this.checkout_worktree_review_open()
                        && !this
                            .git
                            .worktree_create
                            .as_ref()
                            .is_some_and(|form| form.is_checkout_picker())
                    {
                        this.close_checkout_picker();
                    }
                    cx.notify();
                });
            })
            .content(move |_, window, cx| {
                content
                    .update(cx, |this, cx| this.checkout_picker_view(window, cx))
                    .unwrap_or_else(|_| div().into_any_element())
            })
            .into_any_element()
    }
    fn checkout_picker_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if git_picker_keys::asks_for_menu(&event.keystroke.key, event.keystroke.modifiers)
            && self
                .git
                .checkouts
                .input
                .as_ref()
                .is_some_and(|input| input.read(cx).focus_handle(cx).is_focused(window))
            && let Some(index) = self.git.checkouts.highlighted
            && let Some(entry) = rows(&self.git.checkouts.page["entries"]).get(index)
            && self.git.checkouts.contains(entry)
        {
            self.git.checkouts.keyboard_menu = Some(index);
            window.prevent_default();
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if !event.keystroke.modifiers.modified() {
            self.checkout_picker_navigate(&event.keystroke.key, window, cx);
        }
    }
    fn checkout_picker_navigate(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self
            .git
            .checkouts
            .input
            .as_ref()
            .is_some_and(|input| input.read(cx).focus_handle(cx).is_focused(window))
        {
            return;
        }
        // Kit dispatches the input's Enter action before raw key events. Do
        // not let an empty/loading filter bubble into the popover's Confirm.
        if key == "enter" {
            window.prevent_default();
            cx.stop_propagation();
        }
        if !self.git.checkouts.open
            || self.project_dialog_open
            || self.git_write_pending()
            || self.git.checkouts.search_pending.is_some()
            || self.git.checkouts.error.is_some()
            || !self
                .resources
                .get(FIRST)
                .is_some_and(|r| !r.pending && r.error.is_none())
        {
            return;
        }
        let entries = rows(&self.git.checkouts.page["entries"]);
        let Some(step) = git_picker_keys::step(key, self.git.checkouts.highlighted, entries.len())
        else {
            return;
        };
        match step {
            git_picker_keys::Step::Highlight(index) => {
                self.git.checkouts.highlighted = Some(index);
                self.git
                    .checkouts
                    .scroll
                    .scroll_to_item(index, ScrollStrategy::Nearest);
            }
            git_picker_keys::Step::Activate(index) => {
                if let Some(path) = self.git.checkouts.selected_path(&entries[index]) {
                    self.git_open_worktree_path(path, cx);
                }
            }
        }
        window.prevent_default();
        cx.stop_propagation();
        cx.notify();
    }
    fn checkout_picker_view(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let entries = rows(&self.git.checkouts.page["entries"]);
        let pending = self.git.checkouts.search_pending.is_some()
            || [FIRST, MORE]
                .iter()
                .any(|key| self.resources.get(key).is_some_and(|r| r.pending));
        let count = project_loading::picker_count(entries.len(), pending);
        div()
            .capture_key_down(cx.listener(Self::checkout_picker_key))
            .capture_action(cx.listener(
                |this, action: &gpui_kit::component::input::Enter, window, cx| {
                    if !action.secondary && !action.shift {
                        this.checkout_picker_navigate("enter", window, cx);
                    }
                },
            ))
            .w(px(440.).min(window.viewport_size().width - px(32.)))
            .max_h((window.viewport_size().height * 0.7).min(px(560.)))
            .flex()
            .flex_col()
            .min_h_0()
            .child(
                div()
                    .p_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when_some(self.git.checkouts.input.as_ref(), |view, input| {
                        view.child(
                            div().flex_1().min_w_0().child(
                                crate::project_ui::input(input).aria_label("Filter worktrees"),
                            ),
                        )
                    })
                    .child(
                        crate::project_ui::button("checkout-new-worktree")
                            .icon(IconName::Plus)
                            .label("New worktree")
                            .disabled(pending || !self.can_create_picker_worktree())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_picker_worktree(window, cx)
                            })),
                    ),
            )
            .when(!pending, |view| view.child(self.status(FIRST, cx)))
            .when(
                self.resources.get(MORE).is_some_and(|r| r.error.is_some()),
                |view| {
                    view.child(self.status(MORE, cx)).child(
                        crate::project_ui::button("retry-more-checkouts")
                            .label("Retry loading worktrees")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.load_checkouts(true);
                                cx.notify();
                            })),
                    )
                },
            )
            .when_some(self.git.checkouts.error.clone(), |view, error| {
                view.child(crate::project_ui::label(error).p_3())
            })
            .when(
                !pending
                    && (self.git.checkouts.error.is_some()
                        || self.resources.get(FIRST).is_some_and(|r| r.error.is_some())),
                |view| {
                    view.child(
                        crate::project_ui::button("retry-checkouts")
                            .label("Reload worktrees")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.git.checkouts.error = None;
                                this.load_checkouts(false);
                                cx.notify();
                            })),
                    )
                },
            )
            .when(
                entries.is_empty() && !pending && self.git.checkouts.page["entries"].is_array(),
                |view| view.child(empty::compact("No worktrees match this search.", cx)),
            )
            .child(
                uniform_list(
                    "checkout-entries",
                    count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                        range
                            .map(|index| {
                                let Some(entry) = entries.get(index).cloned() else {
                                    return project_loading::picker_row("worktrees", index);
                                };
                                let (title, subtitle) = git_worktree_view::row_labels(&entry);
                                let current = entry["current"] == true;
                                let available = this.git.checkouts.selected_path(&entry).is_some();
                                let menu_entry = entry.clone();
                                let context_entry = entry.clone();
                                let hover_entry = entry.clone();
                                let menu_owner = cx.weak_entity();
                                let context_owner = cx.weak_entity();
                                git_layout::picker_row(
                                    ("checkout-row", index),
                                    current,
                                    this.git.checkouts.highlighted == Some(index),
                                    cx,
                                )
                                .child(
                                    git_layout::picker_choice(("checkout", index), cx)
                                        .accessibility_label(format!("{title} · {subtitle}"))
                                        .toggled(current)
                                        .disabled(
                                            (!available && !current) || this.git_write_pending(),
                                        )
                                        .child(
                                            div()
                                                .id(("checkout-hover", index))
                                                .when(!text(&hover_entry["path"], "display").trim().is_empty(), |view| {
                                                    view.hoverable_tooltip(move |window, cx| {
                                                        git_worktree_view::hover(&hover_entry, window, cx)
                                                    })
                                                })
                                                .flex_1()
                                                .min_w_0()
                                                .flex()
                                                .flex_col()
                                                .items_start()
                                                .gap_1()
                                                .child(div().w_full().truncate().child(title))
                                                .child(
                                                    crate::project_ui::label(subtitle)
                                                        .text_size(px(crate::project_ui::FONT_SIZE))
                                                        .w_full()
                                                        .truncate(),
                                                ),
                                        )
                                        .when(current, |b| b.icon(IconName::Check))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if this.git_write_pending() {
                                                return;
                                            }
                                            if let Some(path) =
                                                this.git.checkouts.selected_path(&entry)
                                            {
                                                this.git_open_worktree_path(path, cx);
                                            }
                                        })),
                                )
                                .child(
                                    crate::controls::compact_icon_button((
                                        "checkout-actions",
                                        index,
                                    ))
                                    .ghost()
                                    .icon(IconName::Ellipsis)
                                    .accessibility_label(format!(
                                        "Actions for {}",
                                        git_worktree_view::row_labels(&menu_entry).0
                                    ))
                                    .dropdown_menu(move |menu, window, cx| {
                                        checkout_menu(
                                            menu,
                                            menu_owner.clone(),
                                            menu_entry.clone(),
                                            window,
                                            cx,
                                        )
                                    })
                                    .when(this.git.checkouts.keyboard_menu == Some(index), |menu| {
                                        menu.open(true)
                                    })
                                    .on_open_change(
                                        cx.listener(move |this, open: &bool, window, cx| {
                                            if !*open
                                                && this.git.checkouts.keyboard_menu == Some(index)
                                            {
                                                this.git.checkouts.keyboard_menu = None;
                                                if !this.project_dialog_open
                                                    && let Some(input) = &this.git.checkouts.input
                                                {
                                                    crate::controls::focus_editor(
                                                        input, window, cx,
                                                    );
                                                }
                                                cx.notify();
                                            }
                                        }),
                                    ),
                                )
                                .context_menu(move |menu, window, cx| {
                                    checkout_menu(
                                        menu,
                                        context_owner.clone(),
                                        context_entry.clone(),
                                        window,
                                        cx,
                                    )
                                })
                                .into_any_element()
                            })
                            .collect()
                    }),
                )
                .track_scroll(&self.git.checkouts.scroll)
                .h(px((count as f32 * 48.).min(300.).min(
                    (f32::from(window.viewport_size().height) * 0.7 - 140.).max(48.),
                )))
                .min_h_0(),
            )
            .when(
                self.resources.get(MORE).is_some_and(|r| r.pending),
                |view| view.child(project_loading::picker_row("more worktrees", 0)),
            )
            .child(
                div()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .p_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        crate::project_ui::label(format!(
                            "{} loaded",
                            rows(&self.git.checkouts.page["entries"]).len()
                        ))
                        .text_size(px(crate::project_ui::FONT_SIZE))
                        .flex_1(),
                    ),
            )
            .into_any_element()
    }
}
#[cfg(test)]
mod tests {
    use super::{MenuScope, Picker, Summary};
    use gpui_kit::{
        self as gpui, Context, IntoElement, Render, ScrollStrategy, TestAppContext, Window, div,
        prelude::*, px, size, uniform_list,
    };
    use serde_json::json;

    struct ScrollList(Picker);
    impl Render for ScrollList {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            uniform_list(
                "checkouts",
                super::rows(&self.0.page["entries"]).len(),
                cx.processor(|_, range: std::ops::Range<usize>, _, _| {
                    range.map(|_| div().h(px(48.))).collect()
                }),
            )
            .track_scroll(&self.0.scroll)
            .h(px(300.))
        }
    }
    #[gpui::test]
    fn scroll_pagination_waits_for_the_current_page_and_does_not_drain(cx: &mut TestAppContext) {
        let (view, visual) = cx.add_window_view(|_, _| {
            ScrollList(Picker {
                page: json!({"entries": vec![0; 50]}),
                ..Default::default()
            })
        });
        visual.simulate_resize(size(px(400.), px(300.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(!view.read_with(visual, |view, _| view.0.near_scroll_end()));
        view.update(visual, |view, cx| {
            view.0.scroll.scroll_to_item(49, ScrollStrategy::Bottom);
            cx.notify();
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(view.read_with(visual, |view, _| view.0.near_scroll_end()));
        view.update(visual, |view, cx| {
            view.0.page = json!({"entries": vec![0; 100]});
            assert!(
                !view.0.near_scroll_end(),
                "stale layout must not drain the next page"
            );
            cx.notify();
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(!view.read_with(visual, |view, _| view.0.near_scroll_end()));
        view.update(visual, |view, cx| {
            view.0.scroll.scroll_to_item(99, ScrollStrategy::Bottom);
            cx.notify();
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(view.read_with(visual, |view, _| view.0.near_scroll_end()));
        view.update(visual, |view, _| {
            view.0.begin_search("topic".into());
            assert!(!view.0.near_scroll_end());
        });
    }
    #[test]
    fn toolbar_context_is_independent_of_filtered_or_loaded_rows() {
        let mut summary = Summary::default();
        assert_eq!(summary.title(), None);
        assert_eq!(summary.count(), None);
        summary.accept(&json!({"entries":[],"metadata":{
            "totalEntries":192,"matchingEntries":0,
            "current":{"current":true,"kind":"linked","name":{"display":"feature"}}
        }}));
        assert_eq!(summary.title().as_deref(), Some("feature"));
        assert_eq!(summary.count(), Some(192));
        // An unavailable response must not erase the last known context.
        summary.accept(&serde_json::Value::Null);
        assert_eq!(summary.count(), Some(192));
        summary.accept(&json!({"metadata":{"totalEntries":1,
            "current":{"current":true,"kind":"main"}}}));
        assert_eq!(summary.title().as_deref(), Some("Main worktree"));
        assert_eq!(summary.count(), None);
    }
    #[test]
    fn delayed_row_actions_cannot_cross_server_repository_or_snapshot() {
        let scope = MenuScope {
            server: "azure".into(),
            repo: json!("repo-1"),
            snapshot: json!("s1"),
        };
        assert!(scope.matches(
            "azure",
            &json!({"repoId":"repo-1"}),
            &json!({"snapshot":"s1"})
        ));
        assert!(!scope.matches(
            "fixture",
            &json!({"repoId":"repo-1"}),
            &json!({"snapshot":"s1"})
        ));
        assert!(!scope.matches(
            "azure",
            &json!({"repoId":"repo-2"}),
            &json!({"snapshot":"s1"})
        ));
        assert!(!scope.matches(
            "azure",
            &json!({"repoId":"repo-1"}),
            &json!({"snapshot":"s2"})
        ));
        assert!(!scope.matches("azure", &json!({}), &json!({})));
    }
    #[test]
    fn switch_requires_a_current_loaded_available_noncurrent_checkout() {
        let entry = json!({"kind":"linked","state":"available","current":false,"path":{"display":"/repo/feature","bytesB64":"L3JlcG8vZmVhdHVyZQ=="}});
        let mut picker = Picker {
            open: true,
            page: json!({"entries":[entry.clone()]}),
            ..Default::default()
        };
        assert_eq!(picker.selected_path(&entry), Some(entry["path"].clone()));
        let mut stale = entry.clone();
        stale["path"]["display"] = json!("/other");
        assert!(picker.selected_path(&stale).is_none());
        picker.open = false;
        assert!(picker.selected_path(&entry).is_none());
        picker.open = true;
        picker.page["entries"][0]["current"] = json!(true);
        assert!(picker.selected_path(&picker.page["entries"][0]).is_none());
    }
    #[test]
    fn editing_search_cancels_old_selection_and_continuation_before_debounce() {
        let entry = json!({"kind":"linked","state":"available","current":false,"path":{"display":"/topic","bytesB64":"L3RvcGlj"}});
        let mut picker = Picker {
            open: true,
            page: json!({"entries":[entry.clone()],"nextCursor":"old-cursor"}),
            cursor: Some("old-cursor".into()),
            highlighted: Some(0),
            ..Default::default()
        };
        assert!(picker.selected_path(&entry).is_some());
        assert!(picker.begin_search("another branch".into()));
        assert!(picker.selected_path(&entry).is_none());
        assert!(picker.page.is_null());
        assert!(picker.cursor.is_none());
        assert!(picker.highlighted.is_none());
        assert!(picker.search_pending.is_some());
        assert!(!picker.begin_search("another branch".into()));
        picker.page = json!({"entries":[entry.clone()]});
        assert!(picker.selected_path(&entry).is_none());
        picker.search_pending = None;
        assert!(picker.selected_path(&entry).is_some());
        picker.error = Some("Worktrees changed".into());
        assert!(picker.selected_path(&entry).is_none());
    }
}
