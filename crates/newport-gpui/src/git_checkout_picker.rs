//! Bounded worktree navigation, independent of the active workspace tab.
use super::*;
use gpui_kit::component::popover::Popover;
use newport_client::git_worktree::WorktreePage;
use newport_protocol::git::{
    Path as GitPath, Reply, Request as GitRequest, Worktree, WorktreeMetadata,
};
pub(super) const SUMMARY: &str = "git-checkout-summary";
const INVALID: &str = "Invalid repository page. Refresh to retry.";
/// Toolbar context survives menu filtering and tab changes, but not a checkout change.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Summary {
    metadata: Option<WorktreeMetadata>,
}
impl Summary {
    pub(super) fn accept(&mut self, page: &WorktreePage) {
        self.metadata = Some(page.metadata.clone());
    }
    fn title(&self) -> Option<String> {
        let current = self.metadata.as_ref()?.current.as_ref()?;
        current
            .current
            .then(|| git_worktree_view::row_labels(current).0)
    }
    fn count(&self) -> Option<u64> {
        self.metadata
            .as_ref()
            .map(|metadata| metadata.total_entries)
            .filter(|total| *total > 1)
    }
}
const FIRST: &str = "git-checkouts";
const MORE: &str = "git-checkouts-more";
#[derive(Default)]
pub(super) struct Picker {
    pub(super) open: bool,
    pub(super) page: Option<WorktreePage>,
    pub(super) selected: Option<Worktree>,
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
    /// The loaded rows, in order.
    pub(super) fn entries(&self) -> &[Worktree] {
        self.page
            .as_ref()
            .map(|page| page.entries.as_slice())
            .unwrap_or_default()
    }
    /// The snapshot the loaded rows came from.
    pub(super) fn snapshot(&self) -> Option<&str> {
        self.page.as_ref().map(|page| page.snapshot.as_str())
    }
    fn next_cursor(&self) -> Option<&str> {
        self.page.as_ref()?.next_cursor.as_deref()
    }
    fn near_scroll_end(&self) -> bool {
        git_page_demand::near_uniform_end(
            &self.scroll,
            self.entries().len(),
            git_layout::PICKER_ROW_HEIGHT,
        )
    }
    fn begin_search(&mut self, query: String) -> bool {
        if self.query == query {
            return false;
        }
        self.query = query;
        self.search_pending = Some(Instant::now());
        self.page = None;
        self.selected = None;
        self.highlighted = None;
        self.keyboard_menu = None;
        self.cursor = None;
        self.error = None;
        self.scroll = UniformListScrollHandle::default();
        true
    }
    fn selected_path(&self, entry: &Worktree) -> Option<GitPath> {
        self.contains(entry)
            .then(|| git::worktree_destination(entry))
            .flatten()
    }
    pub(super) fn contains(&self, entry: &Worktree) -> bool {
        self.open
            && self.search_pending.is_none()
            && self.error.is_none()
            && self.entries().contains(entry)
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
    repo: Option<String>,
    snapshot: Option<String>,
}
impl MenuScope {
    fn matches(&self, server: &str, repo: Option<&str>, snapshot: Option<&str>) -> bool {
        self.server == server
            && self.repo.is_some()
            && self.repo.as_deref() == repo
            && self.snapshot.is_some()
            && self.snapshot.as_deref() == snapshot
    }
}
fn checkout_menu(
    mut menu: PopupMenu,
    owner: WeakEntity<Newport>,
    entry: Worktree,
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
        repo: this.git.repo_id().map(str::to_owned),
        snapshot: this.git.checkouts.snapshot().map(str::to_owned),
    };
    for (title, action, enabled) in [
        (
            "Open worktree",
            CheckoutAction::Open,
            !busy && this.git.checkouts.selected_path(&entry).is_some(),
        ),
        ("Open in Files", CheckoutAction::Files, true),
        ("Copy path", CheckoutAction::Copy, true),
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
            && this.git.writable()
            && this
                .resources
                .get(FIRST)
                .is_some_and(|r| !r.pending && r.error.is_none());
        menu = menu.separator();
        let mut actions = vec![(
            if entry.locked { "Unlock…" } else { "Lock…" },
            CheckoutAction::Lock,
        )];
        if entry.state == "missing" && !entry.current {
            actions.push(("Locate moved worktree…", CheckoutAction::Repair));
        }
        if newport_client::git_worktree::removal_target(&entry).is_some() {
            actions.push((
                if entry.state == "missing" {
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
                && scope.matches(
                    &this.server,
                    this.git.repo_id(),
                    this.git.checkouts.snapshot(),
                )
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
        entry: &Worktree,
        action: CheckoutAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !scope.matches(
            &self.server,
            self.git.repo_id(),
            self.git.checkouts.snapshot(),
        ) || !self.git.checkouts.contains(entry)
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
                let path = entry.path.clone();
                self.close_checkout_picker();
                self.open_checkout_files(Some(&path), window, cx);
            }
            CheckoutAction::Copy => {
                cx.write_to_clipboard(ClipboardItem::new_string(entry.path.display.clone()));
            }
            action => {
                self.git.checkouts.selected = Some(entry.clone());
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
        if self.git.repo.is_none() {
            return;
        }
        self.resources.invalidate(SUMMARY);
        self.request_git_typed(
            SUMMARY,
            GitRequest::Worktrees {
                repo_id: self.git_repository_id(),
                page_size: 1,
                cursor: None,
                filter: String::new(),
                branch: None,
                name: None,
                at_snapshot: None,
            },
        );
    }
    pub(super) fn checkout_summary_completed(&mut self) {
        if self
            .resources
            .get(SUMMARY)
            .is_some_and(|read| read.error.is_none())
            && let Some(Reply::Worktrees(page)) = self.git_reply(SUMMARY)
        {
            let page = (**page).clone();
            self.git.checkout_summary.accept(&page);
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
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find a worktree"));
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
            && picker.next_cursor().is_some()
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
        let Some(Reply::Worktrees(page)) = self.git_reply(key) else {
            self.git.checkouts.cursor = None;
            self.git.checkouts.error = Some(INVALID.into());
            return;
        };
        let page = (**page).clone();
        if key == FIRST {
            // The visible listing is newer than an outstanding toolbar read.
            self.resources.invalidate(SUMMARY);
            self.git.checkout_summary.accept(&page);
            self.git.checkouts.page = Some(page);
            self.git.checkouts.highlighted = None;
            self.git.checkouts.keyboard_menu = None;
        } else if let Some(cursor) = self.git.checkouts.cursor.take() {
            let merged = match &self.git.checkouts.page {
                Some(previous) => newport_client::git_page::merge_pages(previous, page, &cursor),
                None => Err(INVALID.into()),
            };
            match merged {
                Ok(page) => self.git.checkouts.page = Some(page),
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
            self.git.checkouts.next_cursor().map(str::to_owned)
        } else {
            None
        };
        if more && cursor.is_none() {
            return;
        }
        let request = GitRequest::Worktrees {
            repo_id: self.git_repository_id(),
            page_size: 50,
            filter: self.git.checkouts.query.clone(),
            cursor: cursor.clone(),
            branch: None,
            name: None,
            at_snapshot: None,
        };
        self.git.checkouts.cursor = cursor;
        if !more {
            self.git.checkouts.error = None;
            self.git.checkouts.page = None;
            self.resources.invalidate(MORE);
        }
        self.resources.invalidate(key);
        self.request_git_typed(key, request);
    }
    pub(super) fn checkout_picker_trigger(&self, cx: &mut Context<Self>) -> AnyElement {
        let label = self.git.checkout_summary.title().unwrap_or_else(|| {
            if self.selection.as_ref().is_some_and(|p| {
                self.git.opening_path.as_ref().map(|path| &path.bytes_b64)
                    == Some(&p.path.bytes_b64)
            }) {
                "Main worktree".into()
            } else {
                self.git
                    .opening_path
                    .as_ref()
                    .map(|path| path.display.as_str())
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
                git_layout::picker("choose-worktree", IconName::FolderTree, label.clone())
                    .w_full()
                    .icon(Icon::new(IconName::FolderTree).text_color(cx.theme().muted_foreground))
                    .when_some(self.git.checkout_summary.count(), |view, count| {
                        view.child(
                            div()
                                .flex_shrink_0()
                                .text_color(cx.theme().muted_foreground)
                                .text_size(px(crate::ui::FONT_SIZE))
                                .font_weight(FontWeight::NORMAL)
                                .child(count.to_string()),
                        )
                    })
                    .accessibility_label(format!("Worktrees: {label}"))
                    .hover_path(
                        "Repository",
                        self.git
                            .root()
                            .map(|root| root.display.clone())
                            .unwrap_or_default(),
                    )
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
            && self
                .git
                .checkouts
                .entries()
                .get(index)
                .is_some_and(|entry| self.git.checkouts.contains(entry))
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
        let Some(step) = git_picker_keys::step(
            key,
            self.git.checkouts.highlighted,
            self.git.checkouts.entries().len(),
        ) else {
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
                if let Some(path) = self
                    .git
                    .checkouts
                    .entries()
                    .get(index)
                    .and_then(|entry| self.git.checkouts.selected_path(entry))
                {
                    self.git_open_worktree_path(path, cx);
                }
            }
        }
        window.prevent_default();
        cx.stop_propagation();
        cx.notify();
    }
    fn checkout_picker_view(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let entries = self.git.checkouts.entries().to_vec();
        let pending = self.git.checkouts.search_pending.is_some()
            || [FIRST, MORE]
                .iter()
                .any(|key| self.resources.get(key).is_some_and(|r| r.pending));
        let count = project_loading::picker_count(entries.len(), pending);
        git_layout::picker_surface(window)
            .capture_key_down(cx.listener(Self::checkout_picker_key))
            .capture_action(cx.listener(
                |this, action: &gpui_kit::component::input::Enter, window, cx| {
                    if !action.secondary && !action.shift {
                        this.checkout_picker_navigate("enter", window, cx);
                    }
                },
            ))
            .child(
                git_layout::search_bar()
                    .when_some(self.git.checkouts.input.as_ref(), |view, input| {
                        view.child(div().flex_1().min_w_0().child(
                            crate::controls::search_input(input, cx).aria_label("Filter worktrees"),
                        ))
                    })
                    .child(
                        crate::controls::button("checkout-new-worktree")
                            .icon(IconName::Plus)
                            .label("New worktree…")
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
                        crate::controls::button("retry-more-checkouts")
                            .label("Try again")
                            .accessibility_label("Try loading more worktrees again")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.load_checkouts(true);
                                cx.notify();
                            })),
                    )
                },
            )
            .when_some(self.git.checkouts.error.clone(), |view, error| {
                view.child(crate::label(error).p_3())
            })
            .when(
                !pending
                    && (self.git.checkouts.error.is_some()
                        || self.resources.get(FIRST).is_some_and(|r| r.error.is_some())),
                |view| {
                    view.child(
                        crate::controls::button("retry-checkouts")
                            .label("Try again")
                            .accessibility_label("Try loading worktrees again")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.git.checkouts.error = None;
                                this.load_checkouts(false);
                                cx.notify();
                            })),
                    )
                },
            )
            .when(
                entries.is_empty() && !pending && self.git.checkouts.page.is_some(),
                |view| view.child(empty::compact("No worktrees match.", cx)),
            )
            .child(
                uniform_list(
                    "checkout-entries",
                    count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                        range
                            .map(|index| {
                                let Some(entry) = entries.get(index).cloned() else {
                                    return git_layout::picker_loading_row("worktrees", index);
                                };
                                let (title, subtitle) = git_worktree_view::row_labels(&entry);
                                let current = entry.current;
                                let available = this.git.checkouts.selected_path(&entry).is_some();
                                let menu_entry = entry.clone();
                                let context_entry = entry.clone();
                                let hover_entry = entry.clone();
                                let menu_owner = cx.weak_entity();
                                let context_owner = cx.weak_entity();
                                git_layout::picker_row(
                                    ("checkout-row", index),
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
                                                .when(
                                                    !hover_entry.path.display.trim().is_empty(),
                                                    |view| {
                                                        view.hoverable_tooltip(move |window, cx| {
                                                            git_worktree_view::hover(
                                                                &hover_entry,
                                                                window,
                                                                cx,
                                                            )
                                                        })
                                                    },
                                                )
                                                .flex_1()
                                                .min_w_0()
                                                .flex()
                                                .flex_col()
                                                .items_start()
                                                .gap_1()
                                                .child(
                                                    div()
                                                        .w_full()
                                                        .truncate()
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .child(title),
                                                )
                                                .child(
                                                    crate::label(subtitle)
                                                        .text_size(px(crate::ui::FONT_SIZE))
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
                .h(px((count as f32 * git_layout::PICKER_ROW_HEIGHT)
                    .min(320.)
                    .min(
                        (f32::from(window.viewport_size().height) * 0.7 - 140.)
                            .max(git_layout::PICKER_ROW_HEIGHT),
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
                        crate::label(format!("{} loaded", self.git.checkouts.entries().len()))
                            .text_size(px(crate::ui::FONT_SIZE))
                            .flex_1(),
                    ),
            )
            .into_any_element()
    }
}
#[cfg(test)]
mod tests {
    use super::{MenuScope, Picker, Summary};
    use crate::git_fixture::{worktree, worktree_page};
    use gpui_kit::{
        self as gpui, Context, IntoElement, Render, ScrollStrategy, TestAppContext, Window, div,
        prelude::*, px, size, uniform_list,
    };

    fn rows(count: usize) -> Vec<newport_protocol::git::Worktree> {
        (0..count)
            .map(|index| worktree(Some(&format!("w{index}")), &format!("/w{index}")))
            .collect()
    }
    struct ScrollList(Picker);
    impl Render for ScrollList {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            uniform_list(
                "checkouts",
                self.0.entries().len(),
                cx.processor(|_, range: std::ops::Range<usize>, _, _| {
                    range
                        .map(|_| div().h(px(crate::git_layout::PICKER_ROW_HEIGHT)))
                        .collect()
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
                page: Some(worktree_page("s", rows(50), None)),
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
            view.0.page = Some(worktree_page("s", rows(100), None));
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
    #[gpui::test]
    fn typed_listing_pages_feed_the_rows_summary_and_reviews(cx: &mut TestAppContext) {
        use super::{FIRST, MORE, SUMMARY};
        use newport_protocol::git::Reply;
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
        owner.unwrap().update(visual, |view, _| {
            view.git.repo = Some(crate::git_fixture::repository("repo"));
            view.git.checkouts.open = true;
            let first = worktree(Some("one"), "/one");
            let mut missing = worktree(Some("two"), "/two");
            missing.state = "missing".into();
            let mut page = worktree_page("s", vec![first.clone()], Some("next"));
            page.metadata.total_entries = 2;
            page.metadata.matching_entries = 2;
            let read = view.resources.begin(FIRST).unwrap();
            view.resources
                .complete_git(FIRST, read, Ok(Reply::Worktrees(Box::new(page.clone()))));
            view.checkout_picker_completed(FIRST);
            assert_eq!(view.git.checkouts.entries(), std::slice::from_ref(&first));
            assert_eq!(view.git.checkout_summary.count(), Some(2));

            view.git.checkouts.cursor = Some("next".into());
            let mut more = page.clone();
            more.entries = vec![missing.clone()];
            more.next_cursor = None;
            let read = view.resources.begin(MORE).unwrap();
            view.resources
                .complete_git(MORE, read, Ok(Reply::Worktrees(Box::new(more))));
            view.checkout_picker_completed(MORE);
            assert_eq!(
                view.git.checkouts.entries(),
                [first.clone(), missing.clone()].as_slice()
            );

            view.git.checkouts.selected = Some(missing.clone());
            assert!(view.can_remove_worktree());
            assert!(view.can_repair_worktree());
            assert!(view.worktree_review_current("two", "s"));
            assert!(!view.worktree_review_current("two", "other"));
            view.git.checkouts.selected = Some(first);
            assert!(view.can_lock_worktree());
            assert!(!view.can_repair_worktree());

            // A failed summary read keeps the last known toolbar context.
            let read = view.resources.begin(SUMMARY).unwrap();
            view.resources
                .complete_git(SUMMARY, read, Err("offline".into()));
            view.checkout_summary_completed();
            assert_eq!(view.git.checkout_summary.count(), Some(2));
        });
    }
    #[test]
    fn toolbar_context_is_independent_of_filtered_or_loaded_rows() {
        let mut summary = Summary::default();
        assert_eq!(summary.title(), None);
        assert_eq!(summary.count(), None);
        let mut current = worktree(Some("feature"), "/feature");
        current.current = true;
        let mut filtered = worktree_page("s", vec![], None);
        filtered.metadata.total_entries = 192;
        filtered.metadata.matching_entries = 0;
        filtered.metadata.current = Some(current);
        summary.accept(&filtered);
        assert_eq!(summary.title().as_deref(), Some("feature"));
        assert_eq!(summary.count(), Some(192));
        let mut main = worktree(None, "/repo");
        main.kind = "main".into();
        main.current = true;
        let mut single = worktree_page("s", vec![], None);
        single.metadata.total_entries = 1;
        single.metadata.current = Some(main);
        summary.accept(&single);
        assert_eq!(summary.title().as_deref(), Some("Main worktree"));
        assert_eq!(summary.count(), None);
        // A listing whose current checkout is not marked current names none.
        single.metadata.current.as_mut().unwrap().current = false;
        summary.accept(&single);
        assert_eq!(summary.title(), None);
    }
    #[test]
    fn delayed_row_actions_cannot_cross_server_repository_or_snapshot() {
        let scope = MenuScope {
            server: "sample-server".into(),
            repo: Some("repo-1".into()),
            snapshot: Some("s1".into()),
        };
        assert!(scope.matches("sample-server", Some("repo-1"), Some("s1")));
        assert!(!scope.matches("fixture", Some("repo-1"), Some("s1")));
        assert!(!scope.matches("sample-server", Some("repo-2"), Some("s1")));
        assert!(!scope.matches("sample-server", Some("repo-1"), Some("s2")));
        assert!(!scope.matches("sample-server", Some("repo-1"), None));
        assert!(!scope.matches("sample-server", None, None));
        let unloaded = MenuScope {
            snapshot: None,
            ..scope
        };
        assert!(!unloaded.matches("sample-server", Some("repo-1"), None));
    }
    #[test]
    fn switch_requires_a_current_loaded_available_noncurrent_checkout() {
        let entry = worktree(None, "/repo/feature");
        let mut picker = Picker {
            open: true,
            page: Some(worktree_page("s", vec![entry.clone()], None)),
            ..Default::default()
        };
        assert_eq!(
            picker.selected_path(&entry),
            Some(newport_protocol::git::Path::new(b"/repo/feature"))
        );
        let mut stale = entry.clone();
        stale.path.display = "/other".into();
        assert!(picker.selected_path(&stale).is_none());
        picker.open = false;
        assert!(picker.selected_path(&entry).is_none());
        picker.open = true;
        let page = picker.page.as_mut().unwrap();
        page.entries[0].current = true;
        let current = page.entries[0].clone();
        assert!(picker.selected_path(&current).is_none());
    }
    #[test]
    fn editing_search_cancels_old_selection_and_continuation_before_debounce() {
        let entry = worktree(None, "/topic");
        let mut picker = Picker {
            open: true,
            page: Some(worktree_page("s", vec![entry.clone()], Some("old-cursor"))),
            cursor: Some("old-cursor".into()),
            highlighted: Some(0),
            ..Default::default()
        };
        assert!(picker.selected_path(&entry).is_some());
        assert!(picker.begin_search("another branch".into()));
        assert!(picker.selected_path(&entry).is_none());
        assert!(picker.page.is_none());
        assert!(picker.cursor.is_none());
        assert!(picker.highlighted.is_none());
        assert!(picker.search_pending.is_some());
        assert!(!picker.begin_search("another branch".into()));
        picker.page = Some(worktree_page("s", vec![entry.clone()], None));
        assert!(picker.selected_path(&entry).is_none());
        picker.search_pending = None;
        assert!(picker.selected_path(&entry).is_some());
        picker.error = Some("Worktrees changed".into());
        assert!(picker.selected_path(&entry).is_none());
    }
}
