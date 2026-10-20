//! An anchored branch browser with reads independent of the visible Git page.
use super::*;
use gpui_kit::component::popover::Popover;

/// Branch row facts, read from the listing without another request.
/// Keep the remote label because this picker has one mixed list.
fn branch_details(entry: &Value) -> String {
    let status = if entry["current"] == true {
        "Current branch".to_owned()
    } else if entry["remote"] == true {
        "Remote branch".to_owned()
    } else {
        entry["oid"]["hex"]
            .as_str()
            .filter(|oid| !oid.is_empty())
            .map(|oid| oid.chars().take(8).collect())
            .unwrap_or_else(|| "Unavailable".into())
    };
    let upstream = entry["upstream"]["display"]
        .as_str()
        .filter(|name| !name.is_empty());
    match upstream {
        Some(name) => {
            let short = name
                .strip_prefix("refs/remotes/")
                .or_else(|| name.strip_prefix("refs/heads/"))
                .unwrap_or(name);
            format!("{status} · Tracks {short}")
        }
        None => status,
    }
}

#[derive(Default)]
pub(super) struct Picker {
    pub open: bool,
    pub page: Value,
    pub selected: Value,
    pub error: Option<String>,
    scroll: UniformListScrollHandle,
    cursor: Option<String>,
    checkout: Option<Value>,
    worktree: bool,
    highlighted: Option<usize>,
    keyboard_menu: Option<usize>,
    menu: Option<WeakEntity<PopupMenu>>,
    prefetched: Option<PrefetchedPage>,
}

struct PrefetchedPage {
    cursor: String,
    result: Option<Result<Value, String>>,
}

impl Picker {
    fn reset_listing(&mut self) {
        self.page = Value::Null;
        self.selected = Value::Null;
        self.highlighted = None;
        self.keyboard_menu = None;
        self.cursor = None;
        self.checkout = None;
        self.error = None;
        self.prefetched = None;
        self.scroll = UniformListScrollHandle::default();
    }

    /// Keep one page ahead without adding rows or reserving a checkout action.
    fn next_prefetch(&mut self) -> Option<String> {
        if !self.open || self.error.is_some() || self.cursor.is_some() {
            return None;
        }
        let cursor = self.page["nextCursor"].as_str()?.to_owned();
        if self
            .prefetched
            .as_ref()
            .is_some_and(|page| page.cursor == cursor)
        {
            return None;
        }
        self.prefetched = Some(PrefetchedPage {
            cursor: cursor.clone(),
            result: None,
        });
        Some(cursor)
    }

    fn demand_prefetch(&mut self) -> bool {
        let Some(page) = self.prefetched.as_mut() else {
            return false;
        };
        if !self.open || self.page["nextCursor"].as_str() != Some(&page.cursor) {
            return false;
        }
        self.cursor = Some(page.cursor.clone());
        if let Some(result) = page.result.take() {
            self.consume_prefetch(result);
        }
        true
    }

    fn consume_prefetch(&mut self, result: Result<Value, String>) {
        self.prefetched = None;
        if let Err(error) = &result {
            // Speculative failures surface only once this page is requested.
            self.error = Some(error.clone());
        }
        self.accept_page("git-branches-more", result);
    }

    fn accept_prefetch(&mut self, result: Result<Value, String>) {
        let Some(page) = self.prefetched.as_mut() else {
            return;
        };
        if !self.open || self.page["nextCursor"].as_str() != Some(&page.cursor) {
            return;
        }
        if self.cursor.as_ref() == Some(&page.cursor) {
            self.consume_prefetch(result);
        } else {
            page.result = Some(result);
        }
    }
    fn select(&mut self, entry: &Value) -> bool {
        if !self.open || !rows(&self.page["entries"]).contains(entry) {
            return false;
        }
        self.selected = entry.clone();
        self.checkout = None;
        true
    }
    fn accept_page(&mut self, key: &str, result: Result<Value, String>) {
        if !self.open {
            return;
        }
        let Ok(value) = result else {
            self.cursor = None;
            return; // The resource owns the actionable read error.
        };
        if key == "git-branches" {
            self.page = value;
            self.highlighted = None;
            self.keyboard_menu = None;
        } else if key == "git-branches-more"
            && let Some(cursor) = self.cursor.take()
        {
            match newport_client::git_page::merge(&self.page, value, &cursor) {
                Ok(page) => self.page = page,
                Err(error) => self.error = Some(error),
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Action {
    Rename,
    Upstream,
    Delete,
    Copy,
    Merge,
    FastForward,
    Rebase,
    Worktree,
}

#[derive(Clone)]
struct MenuScope {
    server: String,
    repo: Value,
    snapshot: Value,
}
impl MenuScope {
    fn matches(&self, this: &Newport) -> bool {
        this.git.branches.open
            && !this.project_dialog_open
            && !this.branch_picker_reviewing()
            && self.same_listing(&this.server, &this.git.repo, &this.git.branches.page)
    }
    fn same_listing(&self, server: &str, repo: &Value, page: &Value) -> bool {
        self.server == server
            && self.repo.is_string()
            && self.repo == repo["repoId"]
            && self.snapshot.is_string()
            && self.snapshot == page["snapshot"]
    }
}
fn branch_menu(
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
    if !this.git.branches.open || !rows(&this.git.branches.page["entries"]).contains(&entry) {
        return menu;
    }
    let scope = MenuScope {
        server: this.server.clone(),
        repo: this.git.repo["repoId"].clone(),
        snapshot: this.git.branches.page["snapshot"].clone(),
    };
    let writable = this.git_can_write_branch();
    let current = entry["current"] == true;
    let local = git::local_branch_target(&entry).is_some();
    for (action, title, enabled) in [
        (Action::Rename, "Rename…", writable && local),
        (
            Action::Upstream,
            "Set upstream…",
            writable && local && entry["tracking"]["editable"] == true,
        ),
        (Action::Copy, "Copy branch name", true),
        (
            Action::Merge,
            "Merge into current branch…",
            writable && !current,
        ),
        (
            Action::FastForward,
            "Fast-forward to branch…",
            writable && !current,
        ),
        (Action::Rebase, "Rebase onto branch…", writable && !current),
        (
            Action::Worktree,
            "New worktree…",
            writable && local && !current,
        ),
        (
            Action::Delete,
            "Delete local branch…",
            writable && local && !current,
        ),
    ] {
        let owner = owner.clone();
        let entry = entry.clone();
        let scope = scope.clone();
        menu = menu.item(PopupMenuItem::new(title).disabled(!enabled).on_click(
            move |_, window, cx| {
                let _ = owner.update(cx, |this, cx| {
                    if scope.matches(this) {
                        this.branch_picker_action(&entry, action, window, cx)
                    }
                });
            },
        ));
    }

    let menu_view = cx.weak_entity();
    let menu_id = menu_view.entity_id();
    let observer = owner.clone();
    window
        .subscribe(&cx.entity(), cx, move |_, _: &DismissEvent, _, cx| {
            let _ = observer.update(cx, |this, cx| {
                if this
                    .git
                    .branches
                    .menu
                    .as_ref()
                    .is_some_and(|menu| menu.entity_id() == menu_id)
                {
                    this.git.branches.menu = None;
                    cx.notify();
                }
            });
        })
        .detach();
    cx.defer(move |cx| {
        let _ = owner.update(cx, |this, cx| {
            if scope.matches(this) {
                this.git.branches.menu = Some(menu_view);
                cx.notify();
            }
        });
    });
    menu
}

const READS: [&str; 8] = [
    "git-branches",
    "git-branches-more",
    "git-branches-prefetch",
    "git-branch-status",
    "git-branch-worktree-owner",
    "git-branch-upstreams",
    "git-branch-upstreams-more",
    "git-worktree-list",
];

impl Newport {
    pub(super) fn git_branch_picker_close(&mut self) {
        self.git.branches = Picker::default();
        self.git.branch_search = None;
        self.git.branch_form = None;
        self.git.branch_manage = None;
        if self
            .git
            .integration_review
            .as_ref()
            .is_some_and(|r| r.is_branch_picker())
        {
            self.git.integration_review = None;
        }
        if self
            .git
            .worktree_create
            .as_ref()
            .is_some_and(|r| r.is_branch_picker())
        {
            self.git.worktree_create = None;
        }
        for key in READS {
            self.resources.invalidate(key);
        }
    }
    pub(super) fn git_reset_branch_listing(&mut self) {
        for key in [
            "git-branches",
            "git-branches-more",
            "git-branches-prefetch",
            "git-branch-worktree-owner",
        ] {
            self.resources.invalidate(key);
        }
        self.git.branches.reset_listing();
    }
    pub(super) fn git_branch_picker_reload(&mut self) {
        self.git_reset_branch_listing();
        self.git_branch_picker_load(false);
        for (key, method) in [
            ("git-branch-status", "repo.status"),
            ("git-worktree-list", "repo.worktrees"),
        ] {
            self.resources.invalidate(key);
            self.request(key, "git_request", json!({"serverId":self.server,"request":{"method":method,"params":{"repoId":self.git.repo["repoId"],"pageSize":1}}}));
        }
    }
    pub(super) fn git_branch_picker_load(&mut self, more: bool) {
        if !self.git.branches.open
            || self.git.branches.cursor.is_some()
            || self.git.branches.error.is_some()
        {
            return;
        }
        if more && self.git.branches.demand_prefetch() {
            self.git_branch_prefetch_next();
            return;
        }
        let mut params = json!({"repoId":self.git.repo["repoId"],"pageSize":100});
        if let Some(search) = &self.git.branch_search {
            params["filter"] = json!(search.query);
            params["branchKind"] = json!(git_branch_search::kind(search.kind));
        }
        let key = if more {
            "git-branches-more"
        } else {
            "git-branches"
        };
        if more {
            if ["git-branches", key].iter().any(|key| {
                self.resources
                    .get(key)
                    .is_some_and(|r| r.pending || r.error.is_some())
            }) {
                return;
            }
            let Some(cursor) = self.git.branches.page["nextCursor"]
                .as_str()
                .map(str::to_owned)
            else {
                return;
            };
            params["cursor"] = json!(cursor);
            self.git.branches.cursor = Some(cursor);
        }
        self.request(
            key,
            "git_request",
            json!({"serverId":self.server,"request":{"method":"repo.branches","params":params}}),
        );
        if self
            .resources
            .get(key)
            .is_some_and(|r| !r.pending && r.error.is_some())
        {
            self.git_branch_picker_completed(key);
        }
    }
    pub(super) fn git_branch_picker_completed(&mut self, key: &str) {
        if let Some(resource) = self.resources.get(key) {
            if resource.pending {
                return;
            }
            let result = resource
                .error
                .clone()
                .map_or_else(|| Ok(resource.value.clone()), Err);
            if key == "git-branches-prefetch" {
                self.git.branches.accept_prefetch(result);
            } else if matches!(key, "git-branches" | "git-branches-more") {
                self.git.branches.accept_page(key, result);
            }
        }
        self.git_branch_prefetch_next();
    }

    fn git_branch_prefetch_next(&mut self) {
        let Some(cursor) = self.git.branches.next_prefetch() else {
            return;
        };
        let mut params = json!({"repoId":self.git.repo["repoId"],"pageSize":100,"cursor":cursor});
        if let Some(search) = &self.git.branch_search {
            params["filter"] = json!(search.query);
            params["branchKind"] = json!(git_branch_search::kind(search.kind));
        }
        self.resources.invalidate("git-branches-prefetch");
        self.request(
            "git-branches-prefetch",
            "git_request",
            json!({"serverId":self.server,"request":{"method":"repo.branches","params":params}}),
        );
        // Dispatch can fail synchronously (e.g. a full worker queue).
        if self
            .resources
            .get("git-branches-prefetch")
            .is_some_and(|r| !r.pending && r.error.is_some())
        {
            self.git_branch_picker_completed("git-branches-prefetch");
        }
    }
    pub(super) fn git_branch_picker_poll(&mut self, cx: &mut Context<Self>) {
        if !self.git.branches.open {
            return;
        }
        if git_page_demand::near_uniform_end(
            &self.git.branches.scroll,
            self.git.branches.page["entries"]
                .as_array()
                .map_or(0, Vec::len),
            git_layout::PICKER_ROW_HEIGHT,
        ) && self.page == Page::Projects
            && !self.project_dialog_open
            && !self.branch_picker_reviewing()
        {
            self.git_branch_picker_load(true);
        }
        if self.git.branches.checkout.is_some()
            && self
                .resources
                .get("git-branch-worktree-owner")
                .is_some_and(|r| !r.pending)
        {
            let selected = self.git.branches.checkout.take().unwrap();
            if selected == self.git.branches.selected {
                if self.branch_owner_path().is_some() {
                    self.open_branch_worktree(cx);
                } else if self.git_can_checkout() {
                    self.git_checkout(cx);
                    self.git_branch_picker_close();
                }
            }
            cx.notify();
        }
    }
    fn branch_picker_reviewing(&self) -> bool {
        self.git.branch_form.is_some()
            || self.git.branch_manage.is_some()
            || self
                .git
                .integration_review
                .as_ref()
                .is_some_and(|r| r.is_branch_picker())
            || self
                .git
                .worktree_create
                .as_ref()
                .is_some_and(|r| r.is_branch_picker())
            || self.git.branches.worktree
    }
    fn branch_picker_select(&mut self, entry: &Value) -> bool {
        self.git.branches.select(entry)
    }
    fn branch_picker_action(
        &mut self,
        entry: &Value,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.branch_picker_select(entry) {
            return;
        }
        match action {
            Action::Copy => {
                cx.write_to_clipboard(ClipboardItem::new_string(text(&entry["name"], "display")))
            }
            Action::Rename => self.git_rename_branch(window, cx),
            Action::Upstream => {
                self.open_branch_manage(git_branch_manage::Kind::Upstream, window, cx)
            }
            Action::Delete => self.open_branch_manage(git_branch_manage::Kind::Delete, window, cx),
            Action::Merge => self.prepare_branch_integration("merge", cx),
            Action::FastForward => self.prepare_branch_integration("merge.fast_forward", cx),
            Action::Rebase => self.prepare_branch_integration("rebase", cx),
            Action::Worktree => {
                if self.git_can_write_branch() {
                    self.git.branches.worktree = true;
                    self.git_branch_worktree_owner();
                }
            }
        }
        cx.notify();
    }
    pub(super) fn git_branch_picker_trigger(&self, cx: &mut Context<Self>) -> AnyElement {
        let owner = cx.entity().downgrade();
        let content = owner.clone();
        Popover::new("git-branch-picker")
            .trigger_style(git_layout::picker_container())
            .open(self.git.branches.open && !self.project_dialog_open)
            .overlay_closable(
                self.git
                    .branches
                    .menu
                    .as_ref()
                    .and_then(WeakEntity::upgrade)
                    .is_none(),
            )
            .offset(px(6.))
            .p_0()
            .trigger(
                git_layout::picker(
                    "git-branch-navigation",
                    IconName::GitBranch,
                    git::head_label(&self.git.repo["head"]),
                )
                .w_full()
                .accessibility_label(format!(
                    "Branches: {}",
                    git::head_label(&self.git.repo["head"])
                ))
                .icon(Icon::new(IconName::GitBranch).text_color(cx.theme().muted_foreground))
                .disabled(
                    self.git_write_pending()
                        || self.project_dialog_open
                        || self.git.switching_path.is_some(),
                ),
            )
            .on_open_change(move |open, window, cx| {
                let _ = owner.update(cx, |this, cx| {
                    if *open {
                        this.close_checkout_picker();
                        this.git_branch_picker_close();
                        this.git.branches.open = true;
                        this.sync_git_branch_search(window, cx);
                        if let Some(search) = &this.git.branch_search {
                            crate::controls::focus_editor(&search.input, window, cx);
                        }
                        this.git_branch_picker_reload();
                    } else if !this.branch_picker_reviewing() {
                        this.git_branch_picker_close();
                    }
                    cx.notify();
                });
            })
            .content(move |_, window, cx| {
                content
                    .update(cx, |this, cx| this.git_branch_picker_view(window, cx))
                    .unwrap_or_else(|_| div().into_any_element())
            })
            .into_any_element()
    }
    fn activate_picker_branch(&mut self, entry: &Value, cx: &mut Context<Self>) {
        if entry["current"] == true
            || git::local_branch_target(entry).is_none()
            || !self.git_can_write_branch()
            || self.git.branches.checkout.is_some()
            || !self.branch_picker_select(entry)
        {
            return;
        }
        self.git.branches.checkout = Some(entry.clone());
        self.git_branch_worktree_owner();
        cx.notify();
    }
    fn branch_picker_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if git_picker_keys::asks_for_menu(&event.keystroke.key, event.keystroke.modifiers)
            && self
                .git
                .branch_search
                .as_ref()
                .is_some_and(|search| search.input.read(cx).focus_handle(cx).is_focused(window))
            && !self.branch_picker_reviewing()
            && self.git.branches.open
            && !self.project_dialog_open
            && self.git.branches.error.is_none()
            && self
                .resources
                .get("git-branches")
                .is_some_and(|r| !r.pending && r.error.is_none())
            && let Some(index) = self.git.branches.highlighted
            && index < rows(&self.git.branches.page["entries"]).len()
        {
            self.git.branches.keyboard_menu = Some(index);
            window.prevent_default();
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if !event.keystroke.modifiers.modified() {
            self.branch_picker_navigate(&event.keystroke.key, window, cx);
        }
    }
    fn branch_picker_navigate(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self
            .git
            .branch_search
            .as_ref()
            .is_some_and(|search| search.input.read(cx).focus_handle(cx).is_focused(window))
        {
            return;
        }
        if key == "enter" {
            window.prevent_default();
            cx.stop_propagation();
        }
        if !self.git.branches.open
            || self.project_dialog_open
            || self.branch_picker_reviewing()
            || self.git_write_pending()
            || self.git.branches.error.is_some()
            || !self
                .resources
                .get("git-branches")
                .is_some_and(|r| !r.pending && r.error.is_none())
        {
            return;
        }
        let entries = rows(&self.git.branches.page["entries"]);
        let Some(step) = git_picker_keys::step(key, self.git.branches.highlighted, entries.len())
        else {
            return;
        };
        match step {
            git_picker_keys::Step::Highlight(index) => {
                self.git.branches.highlighted = Some(index);
                self.git
                    .branches
                    .scroll
                    .scroll_to_item(index, ScrollStrategy::Nearest);
            }
            git_picker_keys::Step::Activate(index) => {
                self.activate_picker_branch(&entries[index], cx)
            }
        }
        window.prevent_default();
        cx.stop_propagation();
        cx.notify();
    }
    fn git_branch_picker_view(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let reviewing = self.branch_picker_reviewing();
        let entries = rows(&self.git.branches.page["entries"]);
        let failed = self.git.branches.error.is_some()
            || ["git-branches", "git-branches-more", "git-branch-status"]
                .iter()
                .any(|key| self.resources.get(key).is_some_and(|r| r.error.is_some()));
        let pending = self
            .git
            .branch_search
            .as_ref()
            .is_some_and(|search| search.pending.is_some())
            || self
                .resources
                .get("git-branches")
                .is_some_and(|r| r.pending);
        let count = project_loading::picker_count(entries.len(), pending);
        git_layout::picker_surface(window)
            .id("branch-browser")
            .capture_key_down(cx.listener(Self::branch_picker_key))
            .capture_action(cx.listener(
                |this, action: &gpui_kit::component::input::Enter, window, cx| {
                    if !action.secondary && !action.shift {
                        this.branch_picker_navigate("enter", window, cx);
                    }
                },
            ))
            .when(!reviewing, |view| {
                view.child(self.git_branch_search_view(cx))
            })
            .when(reviewing, |view| {
                view.child(
                    git_layout::search_bar().child(
                        crate::controls::compact_button("branch-review-back")
                            .icon(IconName::ArrowLeft)
                            .label("Branches")
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.git.branch_form = None;
                                this.git.branch_manage = None;
                                this.git.branches.worktree = false;
                                if this
                                    .git
                                    .integration_review
                                    .as_ref()
                                    .is_some_and(|r| r.is_branch_picker())
                                {
                                    this.git.integration_review = None;
                                }
                                if this
                                    .git
                                    .worktree_create
                                    .as_ref()
                                    .is_some_and(|r| r.is_branch_picker())
                                {
                                    this.git.worktree_create = None;
                                }
                                cx.notify();
                            })),
                    ),
                )
            })
            .when(!pending, |view| view.child(self.status("git-branches", cx)))
            .child(self.status("git-branch-status", cx))
            .when_some(self.git.branches.error.clone(), |view, error| {
                view.child(crate::label(error).p_3())
            })
            .when(reviewing, |view| {
                view.child(
                    div()
                        .id("branch-form-scroll")
                        .overflow_y_scroll()
                        .min_h_0()
                        .when(self.project_dialog_rendering, |view| {
                            view.child(self.git_branch_form(cx))
                        })
                        .when(self.project_dialog_rendering, |view| {
                            view.child(self.branch_manage_view(cx))
                        })
                        .when(self.project_dialog_rendering, |view| {
                            view.child(self.git_branch_integration_view(cx))
                        })
                        .when(self.git.branches.worktree, |view| {
                            view.child(self.git_branch_worktree_hint(cx)).when(
                                self.git.worktree_create.is_none(),
                                |view| {
                                    view.child(
                                        div().p_3().child(
                                            crate::controls::button("branch-worktree-create")
                                                .label("New worktree…")
                                                .disabled(!self.can_create_branch_worktree())
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.new_branch_worktree(window, cx)
                                                })),
                                        ),
                                    )
                                },
                            )
                        })
                        .when(
                            self.git
                                .worktree_create
                                .as_ref()
                                .is_some_and(|r| r.is_branch_picker()),
                            |view| {
                                view.when(self.project_dialog_rendering, |view| {
                                    view.child(self.git_worktree_create_controls(cx))
                                })
                            },
                        ),
                )
            })
            .when(!reviewing, |view| {
                view.child(
                    uniform_list(
                        "branch-picker-list",
                        count,
                        cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                            range
                                .map(|i| {
                                    let Some(entry) = entries.get(i) else {
                                        return git_layout::picker_loading_row("branches", i);
                                    };
                                    let name = text(&entry["name"], "display");
                                    let current = entry["current"] == true;
                                    let local = git::local_branch_target(entry).is_some();
                                    let selected = entry.clone();
                                    let menu_entry = entry.clone();
                                    let context_entry = entry.clone();
                                    let writable = this.git_can_write_branch();
                                    let weak = cx.entity().downgrade();
                                    let context_owner = cx.weak_entity();
                                    let details = branch_details(entry);
                                    git_layout::picker_row(
                                        ("branch-picker-row", i),
                                        this.git.branches.highlighted == Some(i),
                                        cx,
                                    )
                                    .child(
                                        git_layout::picker_choice(("branch-choice", i), cx)
                                            .toggled(current)
                                            .accessibility_label(format!("{name} · {details}"))
                                            .disabled(
                                                !local
                                                    || !writable
                                                    || this.git.branches.checkout.is_some(),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .items_start()
                                                    .w_full()
                                                    .min_w_0()
                                                    .child(
                                                        div()
                                                            .truncate()
                                                            .w_full()
                                                            .font_weight(FontWeight::MEDIUM)
                                                            .child(name),
                                                    )
                                                    .child(
                                                        crate::label(details)
                                                            .w_full()
                                                            .truncate()
                                                            .text_size(px(crate::ui::FONT_SIZE)),
                                                    ),
                                            )
                                            .when(current, |b| b.icon(IconName::Check))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.activate_picker_branch(&selected, cx);
                                            })),
                                    )
                                    .child(
                                        crate::controls::compact_icon_button((
                                            "branch-row-actions",
                                            i,
                                        ))
                                        .icon(IconName::Ellipsis)
                                        .ghost()
                                        .accessibility_label(format!(
                                            "Actions for {}",
                                            text(&entry["name"], "display")
                                        ))
                                        .dropdown_menu(move |menu, window, cx| {
                                            branch_menu(
                                                menu,
                                                weak.clone(),
                                                menu_entry.clone(),
                                                window,
                                                cx,
                                            )
                                        })
                                        .when(this.git.branches.keyboard_menu == Some(i), |menu| {
                                            menu.open(true)
                                        })
                                        .on_open_change(
                                            cx.listener(move |this, open: &bool, window, cx| {
                                                if !*open
                                                    && this.git.branches.keyboard_menu == Some(i)
                                                {
                                                    this.git.branches.keyboard_menu = None;
                                                    if !this.branch_picker_reviewing()
                                                        && !this.project_dialog_open
                                                        && let Some(search) =
                                                            &this.git.branch_search
                                                    {
                                                        crate::controls::focus_editor(
                                                            &search.input,
                                                            window,
                                                            cx,
                                                        );
                                                    }
                                                    cx.notify();
                                                }
                                            }),
                                        ),
                                    )
                                    .context_menu(move |menu, window, cx| {
                                        branch_menu(
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
                    .track_scroll(&self.git.branches.scroll)
                    .h(px((count as f32 * git_layout::PICKER_ROW_HEIGHT).min(320.)))
                    .min_h_0(),
                )
                .when(
                    !pending && !failed && rows(&self.git.branches.page["entries"]).is_empty(),
                    |view| view.child(empty::compact("No branches match.", cx)),
                )
                .child(self.git_branch_worktree_hint(cx))
                .child(self.status("git-branches-more", cx))
                .when(
                    self.git.branches.cursor.is_some() && self.git.branches.prefetched.is_some(),
                    |view| {
                        view.child(div().px_3().py_2().child(project_loading::picker_detail(
                            "more branches",
                            None,
                            true,
                        )))
                    },
                )
            })
            .when(failed, |view| {
                view.child(
                    div().p_2().child(
                        crate::controls::button("branch-picker-more")
                            .label("Try again")
                            .accessibility_label("Try loading branches again")
                            .disabled(
                                pending
                                    || self.git.branches.cursor.is_some()
                                    || self
                                        .resources
                                        .get("git-branches-more")
                                        .is_some_and(|r| r.pending),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.git_branch_picker_reload();
                                cx.notify();
                            })),
                    ),
                )
            })
            .when(
                !reviewing
                    && !pending
                    && !failed
                    && count > 0
                    && self.git.branches.page["nextCursor"].is_null(),
                |view| {
                    view.child(
                        crate::label("All matching branches loaded")
                            .text_size(px(crate::ui::FONT_SIZE))
                            .px_3()
                            .py_2(),
                    )
                },
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{MenuScope, Picker};
    use serde_json::{Value, json};
    fn page(snapshot: &str, entries: &[&str], cursor: Option<&str>) -> Value {
        json!({"snapshot":snapshot,"entries":entries,"nextCursor":cursor,"metadata":{}})
    }
    #[test]
    fn prefetch_reads_one_page_ahead_and_only_appends_on_demand() {
        let mut picker = Picker {
            open: true,
            page: page("a", &["first"], Some("second")),
            ..Default::default()
        };
        assert_eq!(picker.next_prefetch().as_deref(), Some("second"));
        assert!(picker.next_prefetch().is_none());
        assert!(picker.cursor.is_none()); // Loaded branch actions remain available.
        picker.accept_prefetch(Ok(page("a", &["second"], Some("third"))));
        assert_eq!(picker.page["entries"], json!(["first"]));
        assert!(picker.next_prefetch().is_none()); // Completion cannot drain all pages.
        assert!(picker.demand_prefetch());
        assert_eq!(picker.page["entries"], json!(["first", "second"]));
        assert!(picker.cursor.is_none());
        assert_eq!(picker.next_prefetch().as_deref(), Some("third"));
        assert!(picker.demand_prefetch()); // Join the running read, do not request twice.
        assert_eq!(picker.cursor.as_deref(), Some("third"));
        assert!(picker.next_prefetch().is_none());
        picker.accept_prefetch(Ok(page("a", &["third"], None)));
        assert_eq!(picker.page["entries"], json!(["first", "second", "third"]));
        assert!(picker.cursor.is_none());
        assert!(picker.next_prefetch().is_none());
        assert!(!picker.demand_prefetch());
    }

    #[test]
    fn speculative_failure_surfaces_only_on_demand_and_keeps_loaded_rows() {
        let original = page("a", &["first"], Some("second"));
        let mut picker = Picker {
            open: true,
            page: original.clone(),
            ..Default::default()
        };
        picker.next_prefetch();
        picker.accept_prefetch(Err("offline".into()));
        assert!(picker.error.is_none());
        assert!(picker.cursor.is_none());
        assert!(picker.select(&json!("first")));
        assert!(picker.next_prefetch().is_none());
        assert!(picker.demand_prefetch());
        assert_eq!(picker.error.as_deref(), Some("offline"));
        assert_eq!(picker.page, original);
        assert!(picker.cursor.is_none());
        assert!(picker.next_prefetch().is_none());
        picker.reset_listing();
        picker.accept_prefetch(Ok(page("a", &["late"], None)));
        assert!(picker.page.is_null());
        assert!(picker.error.is_none());
    }

    #[test]
    fn prefetched_snapshot_mismatch_and_dismissal_never_replace_visible_rows() {
        let original = page("a", &["first"], Some("second"));
        let mut picker = Picker {
            open: true,
            page: original.clone(),
            ..Default::default()
        };
        picker.next_prefetch();
        picker.accept_prefetch(Ok(page("b", &["wrong snapshot"], None)));
        assert!(picker.demand_prefetch());
        assert_eq!(picker.page, original);
        assert!(picker.error.is_some());
        picker.reset_listing();
        picker.page = original.clone();
        picker.next_prefetch();
        picker.open = false;
        picker.accept_prefetch(Ok(page("a", &["late"], None)));
        assert!(!picker.demand_prefetch());
        assert!(picker.next_prefetch().is_none());
        assert_eq!(picker.page, original);
    }
    #[test]
    fn retained_menu_rejects_another_server_repository_or_snapshot() {
        let scope = MenuScope {
            server: "fixture".into(),
            repo: json!("repo"),
            snapshot: json!("s1"),
        };
        assert!(scope.same_listing(
            "fixture",
            &json!({"repoId":"repo"}),
            &json!({"snapshot":"s1"})
        ));
        assert!(!scope.same_listing(
            "azure",
            &json!({"repoId":"repo"}),
            &json!({"snapshot":"s1"})
        ));
        assert!(!scope.same_listing(
            "fixture",
            &json!({"repoId":"other"}),
            &json!({"snapshot":"s1"})
        ));
        assert!(!scope.same_listing(
            "fixture",
            &json!({"repoId":"repo"}),
            &json!({"snapshot":"s2"})
        ));
        assert!(!scope.same_listing("fixture", &Value::Null, &Value::Null));
    }
    #[test]
    fn filtering_cancels_checkout_and_rejects_old_menu_targets() {
        let mut picker = Picker {
            open: true,
            page: page("a", &["old"], Some("next")),
            checkout: Some(json!("old")),
            highlighted: Some(0),
            ..Default::default()
        };
        picker.reset_listing();
        assert!(picker.open);
        assert!(picker.checkout.is_none());
        assert!(picker.highlighted.is_none());
        assert!(!picker.select(&json!("old")));
        picker.accept_page("git-branches", Ok(page("b", &["new"], None)));
        assert!(picker.highlighted.is_none());
        assert!(picker.select(&json!("new")));
        assert!(!picker.select(&json!("old")));
        assert_eq!(picker.selected, "new");
    }
    #[test]
    fn pagination_rejects_changed_snapshot_without_replacing_visible_rows() {
        let original = page("a", &["first"], Some("next"));
        let mut picker = Picker {
            open: true,
            page: original.clone(),
            cursor: Some("next".into()),
            ..Default::default()
        };
        picker.accept_page("git-branches-more", Ok(page("b", &["changed"], None)));
        assert_eq!(picker.page, original);
        assert!(picker.error.is_some());
        assert!(picker.cursor.is_none());
    }
    #[test]
    fn continuation_failure_can_retry_and_closed_picker_ignores_results() {
        let mut picker = Picker {
            open: true,
            page: page("a", &["first"], Some("next")),
            cursor: Some("next".into()),
            ..Default::default()
        };
        picker.accept_page("git-branches-more", Err("offline".into()));
        assert!(picker.cursor.is_none());
        picker.cursor = Some("next".into());
        picker.accept_page("git-branches-more", Ok(page("a", &["second"], None)));
        assert_eq!(picker.page["entries"], json!(["first", "second"]));
        picker = Picker::default();
        picker.accept_page("git-branches", Ok(page("b", &["late"], None)));
        assert!(picker.page.is_null());
        assert!(!picker.select(&json!("late")));
    }
}
