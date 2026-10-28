//! Filter the agent's paged branch listing, never just the loaded prefix.
use super::*;
use gpui_kit::component::tab::Tab;

pub(super) struct Search {
    pub(super) input: Entity<InputState>,
    pub query: String,
    pub kind: usize,
    pub(super) pending: Option<Instant>,
    _subscription: Subscription,
}
pub(super) fn kind(index: usize) -> &'static str {
    ["all", "local", "remote"]
        .get(index)
        .copied()
        .unwrap_or("all")
}
impl Newport {
    pub(super) fn sync_git_branch_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page != Page::Projects
            || !self.git.branches.open
            || self.git.branch_search.is_some()
        {
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find a branch"));
        let subscription = cx.observe(&input, |this, input, cx| {
            if this.page != Page::Projects || !this.git.branches.open {
                return;
            }
            let query = input.read(cx).value().trim().to_string();
            let Some(search) = this.git.branch_search.as_mut() else {
                return;
            };
            if search.input != input || search.query == query {
                return;
            }
            search.query = query;
            search.pending = Some(Instant::now());
            this.git_reset_branch_listing();
            cx.notify();
        });
        self.git.branch_search = Some(Search {
            input,
            query: String::new(),
            kind: 0,
            pending: None,
            _subscription: subscription,
        });
    }
    pub(super) fn poll_git_branch_search(&mut self) -> bool {
        if self.page != Page::Projects || !self.git.branches.open {
            return false;
        }
        let Some(search) = self.git.branch_search.as_mut() else {
            return false;
        };
        if !search
            .pending
            .is_some_and(|time| time.elapsed() >= Duration::from_millis(200))
        {
            return false;
        }
        search.pending = None;
        self.git_branch_picker_load(false);
        true
    }
    pub(super) fn git_branch_search_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(search) = &self.git.branch_search else {
            return div().into_any_element();
        };
        git_layout::search_bar()
            .flex_col()
            .items_stretch()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div().flex_1().min_w_0().child(
                            crate::controls::search_input(&search.input, cx)
                                .aria_label("Filter branches"),
                        ),
                    )
                    .child(
                        crate::controls::button("branch-picker-new")
                            .icon(IconName::Plus)
                            .label("New branch…")
                            .disabled(!self.git_can_write_branch())
                            .on_click(
                                cx.listener(|this, _, window, cx| this.git_new_branch(window, cx)),
                            ),
                    ),
            )
            .child(
                crate::controls::tabs("branch-kind")
                    .gap_0()
                    .selected_index(search.kind)
                    .children(
                        ["All", "Local", "Remote"]
                            .into_iter()
                            .map(|name| Tab::new().label(name)),
                    )
                    .on_click(cx.listener({
                        let input = search.input.clone();
                        move |this, index: &usize, _, cx| {
                            if this.page != Page::Projects
                                || !this.git.branches.open
                                || this.project_dialog_open
                            {
                                return;
                            }
                            let Some(search) = this.git.branch_search.as_mut() else {
                                return;
                            };
                            if search.input != input || *index > 2 || search.kind == *index {
                                return;
                            }
                            search.kind = *index;
                            search.pending = None;
                            this.git_reset_branch_listing();
                            this.git_branch_picker_load(false);
                            cx.notify();
                        }
                    })),
            )
            .into_any_element()
    }
}
