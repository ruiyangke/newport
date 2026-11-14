//! Commit files and diffs are independent paged reads bound to immutable identities.
use super::*;
use git_listing::Entry;
use gpui_kit::component::input::TextareaState;
use gpui_kit::component::resizable_panel;
use newport_client::git_history::{MainlineReview, commit_action, complete_commit_message};
use newport_protocol::git::{
    Action, Commit, Delta, DiffMetadata, Page as GitPage, Path as GitPath, Reply, Request, Tag,
};

/// A page of the files a commit changed.
pub(super) type CommitFiles = GitPage<Delta, DiffMetadata>;

#[derive(Default)]
pub(super) struct History {
    pub(super) oid: String,
    stash: Option<Commit>,
    tag: Option<Tag>,
    stash_view: usize,
    pub(super) files: Option<CommitFiles>,
    pub diff: Option<newport_client::git_diff_page::DiffPage>,
    pub path: Option<GitPath>,
    pub(super) parent: usize,
    pub(super) file_cursor: Option<String>,
    pub(super) diff_cursor: Option<String>,
    pub scroll: UniformListScrollHandle,
    pub(super) error: Option<String>,
    pub(super) header: git_commit_header::State,
    pub(super) file_demand: git_page_demand::Demand<git_history_files::Identity>,
}
const MISMATCHED_COMPARISON: &str =
    "Commit response does not match the selected comparison. Select the commit again.";
const KEYS: [&str; 5] = [
    "git-history-detail",
    "git-history-files",
    "git-history-files-more",
    "git-history-diff",
    "git-history-diff-more",
];
impl History {
    fn accept_tag(&mut self, selected: Option<&Tag>, detail: Tag) -> Result<bool, String> {
        let oid = selected
            .map(|tag| tag.oid.hex.as_str())
            .filter(|oid| !oid.is_empty());
        if oid.is_none() || Some(detail.oid.hex.as_str()) != oid {
            return Err("Tag detail does not match the selection.".into());
        }
        let target = tag_commit_oid(&detail).unwrap_or_default();
        if !self.oid.is_empty() && self.oid != target {
            return Err("Tag target does not match the inspected commit.".into());
        }
        let inspect = self.oid.is_empty() && !target.is_empty();
        self.oid = target.to_owned();
        self.tag = Some(detail);
        Ok(inspect)
    }
    /// Headers describe the commit being compared, not the stash that owns it.
    /// The original stash remains separate so its other views stay available.
    fn inspection_detail<'a>(&self, detail: Option<&'a Commit>) -> Option<&'a Commit> {
        detail.filter(|detail| !self.oid.is_empty() && detail.oid.hex == self.oid)
    }

    fn select_parent(&mut self, owner: &str, detail: Option<&Commit>, parent: usize) -> bool {
        if self.oid != owner
            || !self
                .inspection_detail(detail)
                .is_some_and(|detail| parent < detail.parents.len())
            || parent == self.parent
        {
            return false;
        }
        self.parent = parent;
        self.files = None;
        self.diff = None;
        self.path = None;
        self.file_cursor = None;
        self.diff_cursor = None;
        self.scroll = UniformListScrollHandle::default();
        self.file_demand = git_page_demand::Demand::default();
        self.error = None;
        true
    }
}
/// A tag listing already contains bounded object, target and annotation data.
/// Fetch again only for a truncated annotation or an incomplete preview.
fn prime_tag_detail(selected: Option<&Tag>, resources: &mut Resources) -> bool {
    let Some(selected) = selected else {
        return true;
    };
    let complete = !selected.oid.hex.is_empty()
        && (selected.details_omitted
            || (selected.peeled_oid.is_some() && selected.peeled_type.is_some()))
        && (!selected.annotated
            || selected.details_omitted
            || (selected.message.is_some() && selected.message_truncated.is_some()));
    if !complete {
        return true;
    }
    if let Some(generation) = resources.begin("git-history-tag") {
        resources.complete_git(
            "git-history-tag",
            generation,
            Ok(Reply::Tag(Box::new(selected.clone()))),
        );
    }
    selected.message_truncated == Some(true)
}
/// History rows already carry commit metadata. Reuse the selected immutable
/// record immediately, fetching detail only when its message was truncated.
/// Stash/tag list entries have different shapes and must still resolve first.
fn prime_commit_detail(
    tab: usize,
    selected: Option<&Commit>,
    oid: &str,
    resources: &mut Resources,
) -> bool {
    let Some(selected) =
        selected.filter(|selected| tab == 1 && !oid.is_empty() && selected.oid.hex == oid)
    else {
        return true;
    };
    if let Some(generation) = resources.begin("git-history-detail") {
        resources.complete_git(
            "git-history-detail",
            generation,
            Ok(Reply::Commit(Box::new(selected.clone()))),
        );
    }
    selected.message_truncated
}

fn empty_inspection(tab: usize, cx: &App) -> AnyElement {
    let (icon, message) = match tab {
        3 => (IconName::Archive, "Select a stash to review its changes."),
        4 => (IconName::Tag, "Select a tag to review its target."),
        _ => (
            IconName::GitCommitHorizontal,
            "Select a commit to inspect its changes.",
        ),
    };
    empty::pane(
        icon,
        match tab {
            3 => "Choose a stash",
            4 => "Choose a tag",
            _ => "Choose a commit",
        },
        message,
        cx,
    )
    .into_any_element()
}
fn matches_comparison(page: &CommitFiles, oid: &str, parent: usize) -> bool {
    let meta = &page.metadata;
    meta.commit_oid.as_ref().map(|oid| oid.hex.as_str()) == Some(oid)
        && meta.parent_index.unwrap_or(0) == parent as u64
}
fn tag_commit_oid(tag: &Tag) -> Option<&str> {
    (tag.peeled_type.as_deref() == Some("commit"))
        .then(|| tag.peeled_oid.as_ref().map(|oid| oid.hex.as_str()))
        .flatten()
        .filter(|oid| !oid.is_empty())
}
// A stash's second parent is the saved index; its own parent is the stash base.
// The optional third parent contains untracked files and has no parents.
fn stash_view_oid(stash: &Commit, view: usize) -> Option<&str> {
    match view {
        0 => Some(stash.oid.hex.as_str()),
        1 | 2 => stash.parents.get(view).map(|parent| parent.hex.as_str()),
        _ => None,
    }
    .filter(|oid| !oid.is_empty())
}
/// The path a commit file row compares: its new path, which a deletion
/// keeps as the original path.
pub(super) fn file_path(entry: &Delta) -> GitPath {
    entry.new_path.clone()
}
impl Newport {
    fn history_commit_action(&self, kind: &str) -> Option<Action> {
        if self.git.tab != 1
            || !self.integration_ready()
            || self.git.integration_review.is_some()
            || self.git.tag_form.is_some()
            || self.git_status("git-tag-status").is_some_and(|status| {
                status.metadata.integration.is_some() || status.metadata.group_counts.conflicted > 0
            })
            || !self
                .resources
                .get("git-history-detail")
                .is_some_and(|r| !r.pending && r.error.is_none())
        {
            return None;
        }
        let head = self
            .git_status("git-tag-status")
            .and_then(|status| status.metadata.head.oid.as_ref())
            .map(|oid| oid.hex.as_str());
        if kind == "commit.amend" && head != Some(&self.git.history.oid) {
            return None;
        }
        let mut action = commit_action(
            kind,
            self.git_commit_detail()?,
            &self.git.history.oid,
            self.git.history.parent,
        )?;
        if kind == "reset" {
            let oid = head?;
            if oid.is_empty() {
                return None;
            }
            let Action::Reset { expected_oid, .. } = &mut action else {
                return None;
            };
            *expected_oid = oid.into();
        }
        Some(action)
    }
    fn review_history_action(&mut self, kind: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(action) = self.history_commit_action(kind) else {
            return;
        };
        let title = match kind {
            "checkout" => "Check out commit",
            "commit.amend" => "Amend latest commit",
            "reset" => "Reset to commit",
            "merge" => "Merge commit into current checkout",
            "merge.fast_forward" => "Fast-forward to commit",
            "rebase" => "Rebase current checkout onto commit",
            "cherry_pick" => "Cherry-pick commit",
            _ => "Revert commit",
        };
        let mainline = if matches!(
            action,
            Action::CherryPick { mainline: 1.., .. } | Action::Revert { mainline: 1.., .. }
        ) {
            let Some(review) = self
                .git_commit_detail()
                .and_then(|detail| MainlineReview::from_commit(detail, &self.git.history.oid))
            else {
                return;
            };
            Some(review)
        } else {
            None
        };
        let original = self.git_commit_detail().and_then(complete_commit_message);
        let message = (kind == "commit.amend").then(|| {
            let input = cx.new(|cx| {
                TextareaState::new(window, cx).placeholder("Complete replacement commit message")
            });
            crate::controls::focus_editor(&input, window, cx);
            if let Some(value) = &original {
                input.update(cx, |input, cx| input.set_value(value.clone(), window, cx));
            }
            input
        });
        let subscription = message
            .as_ref()
            .map(|input| cx.observe(input, |_, _, cx| cx.notify()));
        self.git.integration_review = Some(git_integration::Review {
            id: uuid::Uuid::new_v4(),
            origin: git_integration::ReviewOrigin::History,
            action,
            snapshot: self
                .git_status("git-tag-status")
                .map_or_else(|| "—".into(), |status| status.snapshot.clone()),
            title: title.into(),
            detail: if let Some(description) = history_integration_description(kind) {
                format!("{} Selected commit: {}", description, self.git.history.oid)
            } else if kind == "reset" {
                format!(
                    "Move the current checkout to {}. This changes local history; no remote branch is updated.",
                    self.git.history.oid
                )
            } else if kind == "commit.amend" {
                format!(
                    "Replace latest commit {}. All staged changes are included; unstaged files remain. This rewrites local history and does not update a remote.{}",
                    self.git.history.oid,
                    if original.is_none() {
                        " The original message is truncated or cannot be represented exactly. Enter a complete replacement."
                    } else {
                        ""
                    }
                )
            } else if kind == "checkout" {
                format!(
                    "Check out {} in detached HEAD state. No branch will move. To keep new commits on a branch, create one from Branches before committing. Git will refuse to overwrite conflicting local changes.",
                    self.git.history.oid
                )
            } else {
                format!(
                    "{} {} onto the current checkout. Git may stop for conflicts.",
                    if kind == "cherry_pick" {
                        "Apply commit"
                    } else {
                        "Apply the inverse of commit"
                    },
                    self.git.history.oid
                )
            },
            reset_confirmed: false,
            mainline,
            message,
            _subscription: subscription,
        });
        cx.notify();
    }
    pub(super) fn git_history_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let can_modify = self.history_commit_action("cherry_pick").is_some();
        let can_tag = self.can_create_tag() && self.git.tag_form.is_none();
        crate::controls::button("history-commit-actions")
            .label("Commit actions")
            .disabled(!can_modify && !can_tag)
            .dropdown_menu({
                let weak = cx.entity().downgrade();
                let can_amend = self.history_commit_action("commit.amend").is_some();
                move |mut menu, _, _| {
                    for (kind, title) in [
                        ("merge", "Merge into current checkout…"),
                        ("merge.fast_forward", "Fast-forward to commit…"),
                        ("rebase", "Rebase onto commit…"),
                        ("cherry_pick", "Cherry-pick commit…"),
                        ("revert", "Revert commit…"),
                        ("checkout", "Check out commit…"),
                        ("commit.amend", "Amend latest commit…"),
                        ("reset", "Reset to commit…"),
                    ] {
                        let weak = weak.clone();
                        menu = menu.item(
                            PopupMenuItem::new(title)
                                .disabled(!can_modify || (kind == "commit.amend" && !can_amend))
                                .on_click(move |_, window, cx| {
                                    let _ = weak.update(cx, |this, cx| {
                                        this.review_history_action(kind, window, cx)
                                    });
                                }),
                        );
                    }
                    let weak = weak.clone();
                    menu.separator().item(
                        PopupMenuItem::new("Tag this commit…")
                            .disabled(!can_tag)
                            .on_click(move |_, window, cx| {
                                let _ = weak.update(cx, |this, cx| this.new_tag(window, cx));
                            }),
                    )
                }
            })
            .into_any_element()
    }
    /// The commit detail the last detail read returned.
    fn git_commit_detail(&self) -> Option<&Commit> {
        match self.git_reply("git-history-detail") {
            Some(Reply::Commit(commit)) => Some(commit),
            _ => None,
        }
    }
    fn git_selected_tag(&self) -> Option<&Tag> {
        match &self.git.selected {
            Some(Entry::Tag(tag)) => Some(tag),
            _ => None,
        }
    }
    pub(super) fn git_is_inspection(&self) -> bool {
        matches!(self.git.tab, 1 | 3 | 4)
    }
    pub(super) fn git_history_reset(&mut self) {
        self.resources.invalidate("git-history-tag");
        for key in KEYS {
            self.resources.invalidate(key);
        }
        self.git.history = History::default();
        self.git.diff_scroll = UniformListScrollHandle::default();
    }
    pub(super) fn git_history_select(&mut self) {
        self.git_history_reset();
        if self.git.tab == 4 {
            let selected = self.git_selected_tag().cloned();
            let fetch = prime_tag_detail(selected.as_ref(), &mut self.resources);
            if self
                .resources
                .get("git-history-tag")
                .is_some_and(|row| row.has_value())
            {
                self.git_history_completed("git-history-tag");
            }
            if fetch {
                self.git_load_tag_detail();
            }
            return;
        }
        self.git.history.oid = match &self.git.selected {
            Some(Entry::Stash(stash)) => stash.oid.clone(),
            Some(Entry::Commit(commit)) => commit.oid.hex.clone(),
            _ => String::new(),
        };
        self.git_inspection_load();
    }
    fn git_inspection_load(&mut self) {
        let selected = match &self.git.selected {
            Some(Entry::Commit(commit)) => Some(commit.clone()),
            _ => None,
        };
        if prime_commit_detail(
            self.git.tab,
            selected.as_ref(),
            &self.git.history.oid,
            &mut self.resources,
        ) {
            self.git_load_commit_detail();
        }
        self.git_history_files(false);
    }
    fn git_load_commit_detail(&mut self) {
        self.request_git_typed(
            "git-history-detail",
            Request::Commit {
                repo_id: self.git_repository_id(),
                commit_oid: self.git.history.oid.clone(),
            },
        );
    }
    fn git_load_tag_detail(&mut self) {
        let Some(oid) = self.git_selected_tag().map(|tag| tag.oid.hex.clone()) else {
            return;
        };
        self.request_git_typed(
            "git-history-tag",
            Request::Tag {
                repo_id: self.git_repository_id(),
                oid,
            },
        );
    }
    fn git_stash_view(&mut self, owner: &str, view: usize) {
        if self.page != Page::Projects
            || self.git.tab != 3
            || !matches!(&self.git.selected, Some(Entry::Stash(stash)) if stash.oid == owner)
            || self
                .git
                .history
                .stash
                .as_ref()
                .map(|stash| stash.oid.hex.as_str())
                != Some(owner)
            || self.git.history.stash_view == view
        {
            return;
        }
        let stash = self.git.history.stash.clone();
        let Some(oid) = stash
            .as_ref()
            .and_then(|stash| stash_view_oid(stash, view))
            .map(str::to_owned)
        else {
            return;
        };
        self.git_history_reset();
        self.git.history.stash = stash;
        self.git.history.oid = oid;
        self.git.history.stash_view = view;
        self.git_inspection_load();
    }
    pub(super) fn git_history_parent(&mut self, owner: &str, parent: usize) {
        let detail = self.git_commit_detail().cloned();
        if self.page != Page::Projects
            || !self.git_is_inspection()
            || !self
                .git
                .history
                .select_parent(owner, detail.as_ref(), parent)
        {
            return;
        }
        for key in &KEYS[1..] {
            self.resources.invalidate(key);
        }
        self.git.diff_scroll = UniformListScrollHandle::default();
        self.git_history_files(false);
    }
    pub(super) fn git_history_files(&mut self, more: bool) {
        if !self.git_is_inspection() || self.git.history.oid.is_empty() {
            return;
        }
        let key = if more {
            "git-history-files-more"
        } else {
            "git-history-files"
        };
        if self
            .resources
            .get(key)
            .is_some_and(|r| r.pending || (more && r.error.is_some()))
            || self.git.history.error.is_some()
        {
            return;
        }
        let mut request_cursor = None;
        if more {
            if self.git.history.file_cursor.is_some() {
                return;
            }
            let Some(cursor) = self
                .git
                .history
                .files
                .as_ref()
                .and_then(|files| files.next_cursor.clone())
            else {
                return;
            };
            request_cursor = Some(cursor.clone());
            self.git.history.file_cursor = Some(cursor);
        }
        self.request_git_typed(
            key,
            Request::CommitFiles {
                repo_id: self.git_repository_id(),
                commit_oid: self.git.history.oid.clone(),
                parent_index: self.git.history.parent,
                page_size: 100,
                cursor: request_cursor,
            },
        );
    }
    pub(super) fn git_history_diff(&mut self, more: bool) {
        let Some(path) = self.git.history.path.clone() else {
            return;
        };
        if !self.git_is_inspection() {
            return;
        }
        let key = if more {
            "git-history-diff-more"
        } else {
            "git-history-diff"
        };
        if self
            .resources
            .get(key)
            .is_some_and(|r| r.pending || (more && r.error.is_some()))
            || self.git.history.error.is_some()
        {
            return;
        }
        if path.decode().is_err() {
            return;
        }
        let mut request_cursor = None;
        if more {
            if self.git.history.diff_cursor.is_some() {
                return;
            }
            let Some(cursor) = self
                .git
                .history
                .diff
                .as_ref()
                .and_then(|diff| diff.next_cursor.clone())
            else {
                return;
            };
            request_cursor = Some(cursor.clone());
            self.git.history.diff_cursor = Some(cursor);
        }
        self.request_git_typed(
            key,
            Request::CommitDiffPage {
                repo_id: self.git_repository_id(),
                commit_oid: self.git.history.oid.clone(),
                parent_index: self.git.history.parent,
                path,
                page_size: 200,
                max_bytes: Some(65536),
                context_lines: 3,
                cursor: request_cursor,
                line_encoding: None,
            },
        );
    }
    pub(super) fn git_history_completed(&mut self, key: &str) {
        if self.resources.get(key).is_some_and(|r| r.error.is_some()) {
            return;
        }
        let reply = self.git_reply(key).cloned();
        if key == "git-history-tag" {
            let result = match reply {
                Some(Reply::Tag(tag)) => {
                    let selected = self.git_selected_tag().cloned();
                    self.git.history.accept_tag(selected.as_ref(), *tag)
                }
                _ => Err("Tag detail does not match the selection.".into()),
            };
            match result {
                Ok(true) => self.git_inspection_load(),
                Ok(false) => {}
                Err(error) => self.git.history.error = Some(error),
            }
            return;
        }
        let oid = self.git.history.oid.clone();
        if key == "git-history-detail" {
            match reply {
                Some(Reply::Commit(detail)) if detail.oid.hex == oid => {
                    if self.git.tab == 3 && self.git.history.stash_view == 0 {
                        self.git.history.stash = Some(*detail);
                    }
                }
                _ => {
                    self.resources.invalidate(key);
                    self.git.history.error =
                        Some("Commit detail does not match the selection.".into());
                }
            }
            return;
        }
        if key.starts_with("git-history-diff") {
            self.git_history_diff_completed(key);
            return;
        }
        let page = match reply {
            Some(Reply::CommitFiles(page))
                if matches_comparison(&page, &oid, self.git.history.parent) =>
            {
                *page
            }
            _ => {
                self.git.history.error = Some(MISMATCHED_COMPARISON.into());
                return;
            }
        };
        let result = if key.ends_with("-more") {
            let Some(cursor) = self.git.history.file_cursor.take() else {
                return;
            };
            match &self.git.history.files {
                Some(files) => newport_client::git_page::merge_pages(files, page, &cursor),
                None => Err("Invalid repository page. Refresh to retry.".into()),
            }
        } else {
            Ok(page)
        };
        match result {
            Ok(page) => {
                let first = self.git.history.accept_files(page, key.ends_with("-more"));
                if let Some(first) = first {
                    let owner = self
                        .git
                        .history
                        .files
                        .as_ref()
                        .map(|files| files.metadata.clone());
                    if let Some(owner) = owner {
                        self.git_history_select_file(&owner, &first);
                    }
                }
            }
            Err(error) => self.git.history.error = Some(error),
        }
    }
    /// Accepts a page of the selected file's diff in the inspected commit.
    fn git_history_diff_completed(&mut self, key: &str) {
        let history = &self.git.history;
        let page = match self.git_reply(key) {
            Some(newport_protocol::git::Reply::CommitDiffPage(page))
                if page
                    .metadata
                    .commit_oid
                    .as_ref()
                    .map(|oid| oid.hex.as_str())
                    == Some(history.oid.as_str())
                    && page.metadata.parent_index.unwrap_or(0) == history.parent as u64
                    && page
                        .metadata
                        .selected_path
                        .as_ref()
                        .map(|path| path.bytes_b64.as_str())
                        == history.path.as_ref().map(|path| path.bytes_b64.as_str())
                    && page.metadata.read_only
                    && page.metadata.context_lines == Some(3) =>
            {
                (**page).clone()
            }
            _ => {
                self.git.history.error = Some(MISMATCHED_COMPARISON.into());
                return;
            }
        };
        let result = if key.ends_with("-more") {
            let Some(cursor) = self.git.history.diff_cursor.take() else {
                return;
            };
            match &self.git.history.diff {
                Some(diff) => newport_client::git_diff_page::merge(diff, page, &cursor, false),
                None => Err("Invalid repository page. Refresh to retry.".into()),
            }
        } else {
            Ok(page)
        };
        match result {
            Ok(page) => self.git.history.diff = Some(page),
            Err(error) => self.git.history.error = Some(error),
        }
    }
    fn git_tag_inspection(&self, cx: &mut Context<Self>) -> AnyElement {
        let tag = self.git.history.tag.as_ref();
        let pending = self
            .resources
            .get("git-history-tag")
            .is_some_and(|r| r.pending);
        let failed = self
            .resources
            .get("git-history-tag")
            .is_some_and(|r| r.error.is_some() && !r.pending);
        let owner = self.git.selected.clone();
        let repo = self.git_repository_id();
        let server = self.server.clone();
        div().flex().flex_col().flex_shrink_0().min_w_0()
            .child(self.status("git-history-tag", cx))
            .when(failed, |view| view.child(
                crate::controls::compact_button("retry-tag-detail")
                    .label("Try again").accessibility_label("Try loading the tag annotation again").self_start().m_3()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.page == Page::Projects && this.git.tab == 4
                            && this.server == server && this.git_repository_id() == repo
                            && this.git.selected == owner {
                            this.git_load_tag_detail();
                            cx.notify();
                        }
                    }))
            ))
            .when_some(tag, |view, tag| view.child(
                div().id("git-tag-annotation").max_h(px(260.)).overflow_y_scroll()
                    .p_3().flex().flex_col().gap_2().min_w_0()
                    .border_b_1().border_color(cx.theme().border)
                    .child(div().flex().flex_wrap().gap_2().items_center()
                        .child(div().flex_1().min_w_0().font_weight(FontWeight::SEMIBOLD)
                            .child(self.git_selected_tag().and_then(|tag| tag.name.as_ref())
                                .map_or_else(|| "—".into(), |name| name.display.clone())))
                        .child(self.git_tag_push_trigger(cx)).child(self.git_tag_delete_trigger(cx)))
                    .child(crate::label(if tag.annotated {"Annotated tag"} else {"Lightweight tag"}).text_size(px(crate::ui::FONT_SIZE)))
                    .child(tag_metadata("object", format!("Object: {}", tag.oid.hex)))
                    .when_some(tag.peeled_oid.as_ref(), |view, peeled| view.child(tag_metadata(
                        "target", format!("Target: {} ({})", peeled.hex, tag.peeled_type.as_deref().unwrap_or("—")))))
                    .when_some(tag.tagger.as_ref(), |view, tagger| view.child(tag_metadata(
                        "tagger", format!("{} <{}>", tagger.name, tagger.email))))
                    .when_some(tag.message.as_ref(), |view, message| view.child(
                        div().id("git-tag-message").role(accesskit::Role::Paragraph)
                            .aria_label(message.display.clone())
                            .min_w_0().text_size(px(crate::ui::FONT_SIZE))
                            .child(message.display.clone())))
                    .when(tag.message_truncated == Some(true), |view| view.child(project_loading::picker_detail(
                        "full tag annotation", Some("The annotation is truncated.".into()), pending)))
                    .when(tag.details_omitted, |view| view.child(crate::label("Some tag details could not be loaded.").text_size(px(crate::ui::FONT_SIZE))))
                    .when(self.git.history.oid.is_empty() && !tag.details_omitted, |view| view.child(crate::label(
                        "This tag points to a non-commit object; no commit diff is available.").text_size(px(crate::ui::FONT_SIZE))))
            )).into_any_element()
    }
    pub(super) fn git_history_view(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = !self.git.history.oid.is_empty();
        let detail = self.git.history.inspection_detail(self.git_commit_detail());
        let parents = self
            .git
            .history
            .files
            .as_ref()
            .and_then(|files| files.metadata.parents.clone())
            .unwrap_or_default();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .when(self.git.tab == 4, |view| {
                view.child(self.git_tag_inspection(cx))
            })
            .child(self.status("git-history-detail", cx))
            .when(
                selected
                    && self
                        .resources
                        .get("git-history-detail")
                        .is_some_and(|read| read.error.is_some() && !read.pending),
                |view| {
                    view.child(
                        crate::controls::compact_button("retry-commit-detail")
                            .label("Try again")
                            .accessibility_label("Try loading commit details again")
                            .self_start()
                            .m_3()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.git_load_commit_detail();
                                cx.notify();
                            })),
                    )
                },
            )
            .when_some(detail, |view, detail| {
                view.child(self.git_commit_header(detail, &parents, self.git.history.parent, cx))
            })
            .when_some(
                self.git
                    .history
                    .stash
                    .as_ref()
                    .filter(|stash| self.git.tab == 3 && stash.parents.len() > 1),
                |view, stash| {
                    let mut kinds = vec!["Tracked files", "Staged files"];
                    let owner = stash.oid.hex.clone();
                    if stash_view_oid(stash, 2).is_some() {
                        kinds.push("Untracked files");
                    }
                    view.child(
                        crate::controls::tabs("stash-files-kind")
                            .gap_0()
                            .selected_index(self.git.history.stash_view)
                            .children(
                                kinds
                                    .into_iter()
                                    .map(|name| gpui_kit::component::tab::Tab::new().label(name)),
                            )
                            .on_click(cx.listener(move |this, index: &usize, _, cx| {
                                this.git_stash_view(&owner, *index);
                                cx.notify();
                            })),
                    )
                },
            )
            .when(
                !selected && (self.git.tab != 4 || self.git.selected.is_none()),
                |view| view.child(empty_inspection(self.git.tab, cx)),
            )
            .when_some(self.git.history.error.clone(), |view, error| {
                view.child(crate::label(error).p_3())
            })
            .when(selected, |view| {
                view.child(inspection_panels(
                    self.project_history_stacked(window.viewport_size().width <= px(1100.), cx),
                    self.git_history_file_height(),
                    self.git_history_file_list(cx),
                    self.git_diff_view(window, cx),
                    cx,
                ))
            })
            .into_any_element()
    }
}

fn tag_metadata(id: &'static str, content: String) -> AnyElement {
    crate::label(content.clone())
        .id(SharedString::from(format!("git-tag-{id}")))
        .role(accesskit::Role::Label)
        .aria_label(content.clone())
        .text_size(px(crate::ui::FONT_SIZE))
        .whitespace_normal()
        .into_any_element()
}

fn inspection_panels(
    stacked: bool,
    file_height: f32,
    files: AnyElement,
    diff: AnyElement,
    cx: &App,
) -> AnyElement {
    if stacked {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(
                div()
                    .debug_selector(|| "history-files-pane".into())
                    .flex()
                    .flex_col()
                    .h(px(file_height))
                    .flex_shrink_0()
                    .min_w_0()
                    .overflow_hidden()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(files),
            )
            .child(
                div()
                    .debug_selector(|| "history-diff-pane".into())
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(diff),
            )
            .into_any_element()
    } else {
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(
                git_split::group(
                    "git-history-inspection-split",
                    "Resize the commit file list",
                    240.,
                )
                .child(
                    resizable_panel()
                        .size(px(git_layout::LIST_WIDTH))
                        .size_range(px(git_layout::MIN_LIST_WIDTH)..px(git_layout::MAX_LIST_WIDTH))
                        .flex_none()
                        .child(
                            div()
                                .debug_selector(|| "history-files-pane".into())
                                .flex()
                                .flex_col()
                                .size_full()
                                .child(files),
                        ),
                )
                .child(
                    resizable_panel().size_range(px(240.)..Pixels::MAX).child(
                        div()
                            .debug_selector(|| "history-diff-pane".into())
                            .flex()
                            .flex_col()
                            .size_full()
                            .child(diff),
                    ),
                ),
            )
            .into_any_element()
    }
}

fn history_integration_description(kind: &str) -> Option<&'static str> {
    match kind {
        "merge" => Some(
            "Combine the selected commit’s history with the current checkout. Diverged history creates a merge commit; conflicts may require resolution.",
        ),
        "merge.fast_forward" => Some(
            "Move the current checkout forward only if the selected commit contains its history. Diverged history is refused; no merge commit is created.",
        ),
        "rebase" => Some(
            "Replay the current checkout’s commits onto the selected commit. This rewrites commit IDs and may stop for conflicts. No remote branch is updated.",
        ),
        _ => None,
    }
}
#[cfg(test)]
mod tests {
    use super::{
        Commit, Reply, Tag, file_path, matches_comparison, stash_view_oid, tag_commit_oid,
    };
    use crate::git_fixture::{commit, object, tag as tag_fixture};
    use newport_client::resources::Resources;
    use newport_protocol::git::{Path, TagObjectId};

    fn tag(oid: &str, target: &str, kind: &str) -> Tag {
        let mut tag = tag_fixture("v1", target);
        tag.oid = TagObjectId {
            format: "sha1".into(),
            hex: oid.into(),
        };
        tag.peeled_type = Some(kind.into());
        tag
    }
    fn stored_tag(resources: &Resources) -> Option<&Tag> {
        match resources.get("git-history-tag")?.git()? {
            Reply::Tag(tag) => Some(tag),
            _ => None,
        }
    }
    fn stored_commit(resources: &Resources) -> Option<&Commit> {
        match resources.get("git-history-detail")?.git()? {
            Reply::Commit(commit) => Some(commit),
            _ => None,
        }
    }
    fn with_parents(oid: &str, parents: &[&str]) -> Commit {
        let mut commit = commit(oid, "Subject");
        commit.parents = parents.iter().map(|parent| object(parent)).collect();
        commit
    }

    #[test]
    fn tag_previews_avoid_redundant_reads_and_keep_truncated_annotations_during_failure() {
        let mut resources = Resources::default();
        let mut tag = tag("tag", "target", "commit");
        assert!(!super::prime_tag_detail(Some(&tag), &mut resources));
        assert_eq!(stored_tag(&resources), Some(&tag));
        resources.invalidate("git-history-tag");
        tag.annotated = true;
        tag.message = Some(Path::new(b"Partial annotation"));
        tag.message_truncated = Some(false);
        assert!(!super::prime_tag_detail(Some(&tag), &mut resources));
        resources.invalidate("git-history-tag");
        tag.message_truncated = Some(true);
        assert!(super::prime_tag_detail(Some(&tag), &mut resources));
        let generation = resources.begin("git-history-tag").unwrap();
        assert_eq!(stored_tag(&resources), Some(&tag));
        resources.complete_git(
            "git-history-tag",
            generation,
            Err("Disconnected".to_owned().into()),
        );
        assert_eq!(stored_tag(&resources), Some(&tag));
        let retry = resources.begin("git-history-tag").unwrap();
        tag.message_truncated = Some(false);
        tag.message = Some(Path::new(b"Complete annotation"));
        resources.complete_git(
            "git-history-tag",
            retry,
            Ok(Reply::Tag(Box::new(tag.clone()))),
        );
        assert_eq!(stored_tag(&resources), Some(&tag));
    }

    #[test]
    fn annotation_completion_preserves_comparison_and_rejects_other_tag_or_target() {
        let tag = tag("tag", "target", "commit");
        let mut history = super::History::default();
        assert_eq!(history.accept_tag(Some(&tag), tag.clone()), Ok(true));
        history.parent = 1;
        let mut files = crate::git_fixture::diff_page(
            "files",
            crate::git_fixture::commit_metadata("target", b"file"),
            vec![],
            Some("more"),
        );
        files.entries.clear();
        history.files = Some(newport_protocol::git::Page {
            snapshot: files.snapshot,
            entries: vec![],
            next_cursor: files.next_cursor,
            metadata: files.metadata,
        });
        history.path = Some(Path::new(b"file"));
        history.diff = Some(crate::git_fixture::diff_page(
            "diff",
            crate::git_fixture::commit_metadata("target", b"file"),
            vec![],
            None,
        ));
        let mut full = tag.clone();
        full.message = Some(Path::new(b"Complete annotation"));
        assert_eq!(history.accept_tag(Some(&tag), full.clone()), Ok(false));
        assert_eq!(history.parent, 1);
        assert_eq!(
            history.files.as_ref().unwrap().next_cursor.as_deref(),
            Some("more")
        );
        assert!(history.path.is_some() && history.diff.is_some());
        let mut wrong = full.clone();
        wrong.oid.hex = "another-tag".into();
        assert!(history.accept_tag(Some(&tag), wrong).is_err());
        let mut wrong = full.clone();
        wrong.peeled_oid.as_mut().unwrap().hex = "another-target".into();
        assert!(history.accept_tag(Some(&tag), wrong).is_err());
        assert_eq!(history.tag, Some(full));
        assert_eq!(history.oid, "target");
    }

    #[test]
    fn incomplete_tag_previews_fetch_detail_but_non_commit_tags_do_not_open_history() {
        let mut resources = Resources::default();
        let mut incomplete = tag("tag", "target", "commit");
        incomplete.peeled_oid = None;
        assert!(super::prime_tag_detail(Some(&incomplete), &mut resources));
        assert!(super::prime_tag_detail(None, &mut resources));
        assert!(resources.get("git-history-tag").is_none());
        let tree = tag("tree", "tree", "tree");
        assert!(!super::prime_tag_detail(Some(&tree), &mut resources));
        let mut history = super::History::default();
        assert_eq!(history.accept_tag(Some(&tree), tree.clone()), Ok(false));
        assert!(history.oid.is_empty());
        assert_eq!(history.tag, Some(tree.clone()));
        assert!(history.accept_tag(None, tree).is_err());
    }

    #[test]
    fn stash_inspection_header_follows_the_selected_commit() {
        let stash = with_parents("stash", &["base", "index", "untracked"]);
        let mut history = super::History {
            stash: Some(stash.clone()),
            ..Default::default()
        };
        for (view, oid) in [(0, "stash"), (1, "index"), (2, "untracked")] {
            history.oid = super::stash_view_oid(&stash, view).unwrap().into();
            let detail = commit(oid, &format!("View {view}"));
            assert_eq!(history.inspection_detail(Some(&detail)), Some(&detail));
            if view != 0 {
                assert!(
                    history.inspection_detail(Some(&stash)).is_none(),
                    "the header and copy action must not use the owning stash ID"
                );
            }
            assert!(history.inspection_detail(None).is_none());
            assert_eq!(
                history.stash.as_ref(),
                Some(&stash),
                "switching inspection cannot lose the original stash's view targets"
            );
        }
    }

    #[test]
    fn parent_selection_retains_stash_and_tag_but_resets_comparison() {
        let detail = with_parents("stash", &["base", "index"]);
        let tag = tag("tag", "target", "commit");
        let mut history = super::History {
            oid: "stash".into(),
            stash: Some(detail.clone()),
            tag: Some(tag.clone()),
            ..Default::default()
        };
        let diff = crate::git_fixture::diff_page(
            "diff",
            crate::git_fixture::commit_metadata("commit", b"file"),
            vec![],
            Some("diff"),
        );
        history.files = Some(newport_protocol::git::Page {
            snapshot: "files".into(),
            entries: vec![],
            next_cursor: Some("files".into()),
            metadata: diff.metadata.clone(),
        });
        history.path = Some(Path::new(b"file"));
        history.diff = Some(diff);
        history.file_cursor = Some("files".into());
        history.diff_cursor = Some("diff".into());
        assert!(history.select_parent("stash", Some(&detail), 1));
        assert_eq!(history.parent, 1);
        assert_eq!(history.stash, Some(detail));
        assert_eq!(history.tag, Some(tag));
        assert!(history.files.is_none() && history.path.is_none() && history.diff.is_none());
        assert!(history.file_cursor.is_none() && history.diff_cursor.is_none());
    }

    #[test]
    fn stale_invalid_and_repeated_parent_selections_preserve_current_comparison() {
        let detail = with_parents("current", &["first", "second"]);
        let files = newport_protocol::git::Page {
            snapshot: "files".into(),
            entries: vec![],
            next_cursor: None,
            metadata: crate::git_fixture::commit_metadata("current", b"file"),
        };
        let mut history = super::History {
            oid: "current".into(),
            files: Some(files.clone()),
            ..Default::default()
        };
        assert!(!history.select_parent("previous", Some(&detail), 1));
        assert!(!history.select_parent(
            "current",
            Some(&with_parents("previous", &["first", "second"])),
            1
        ));
        assert!(!history.select_parent("current", Some(&detail), 2));
        assert!(!history.select_parent("current", Some(&detail), 0));
        assert!(!history.select_parent("current", None, 1));
        assert_eq!(history.files, Some(files));
        assert_eq!(history.parent, 0);
    }

    #[test]
    fn history_preview_avoids_redundant_detail_reads_but_expands_truncated_messages() {
        let mut resources = Resources::default();
        let mut preview = commit("abc", "Subject");
        assert!(!super::prime_commit_detail(
            1,
            Some(&preview),
            "abc",
            &mut resources
        ));
        assert_eq!(stored_commit(&resources), Some(&preview));
        assert!(!resources.get("git-history-detail").unwrap().pending);

        resources.invalidate("git-history-detail");
        preview.message_truncated = true;
        assert!(super::prime_commit_detail(
            1,
            Some(&preview),
            "abc",
            &mut resources
        ));
        let generation = resources.begin("git-history-detail").unwrap();
        assert_eq!(stored_commit(&resources), Some(&preview));
        assert!(resources.complete_git(
            "git-history-detail",
            generation,
            Err("offline".to_owned().into())
        ));
        assert_eq!(stored_commit(&resources), Some(&preview));
        let retry = resources.begin("git-history-detail").unwrap();
        let mut complete = preview.clone();
        complete.message = Path::new(b"Subject\n\nComplete body");
        complete.message_truncated = false;
        assert!(resources.complete_git(
            "git-history-detail",
            retry,
            Ok(Reply::Commit(Box::new(complete.clone())))
        ));
        assert_eq!(stored_commit(&resources), Some(&complete));
    }
    #[test]
    fn previews_cannot_cross_commit_identity_or_replace_stash_and_tag_resolution() {
        let preview = commit("abc", "Subject");
        for (tab, oid) in [(1, "other"), (1, ""), (3, "abc"), (4, "abc")] {
            let mut resources = Resources::default();
            assert!(super::prime_commit_detail(
                tab,
                Some(&preview),
                oid,
                &mut resources
            ));
            assert!(resources.get("git-history-detail").is_none());
        }
        let mut resources = Resources::default();
        assert!(super::prime_commit_detail(1, None, "abc", &mut resources));
        assert!(resources.get("git-history-detail").is_none());
    }
    #[test]
    fn tags_inspect_only_resolved_commit_targets() {
        assert_eq!(
            tag_commit_oid(&tag("tag", "commit", "commit")),
            Some("commit")
        );
        for kind in ["tree", "blob", "tag"] {
            assert_eq!(tag_commit_oid(&tag("tag", "object", kind)), None);
        }
        let mut omitted = tag("tag", "commit", "commit");
        omitted.details_omitted = true;
        omitted.peeled_oid = None;
        omitted.peeled_type = None;
        assert_eq!(tag_commit_oid(&omitted), None);
    }
    #[test]
    fn stash_views_select_worktree_index_and_optional_untracked_commits() {
        let stash = with_parents("worktree", &["base", "index", "untracked"]);
        assert_eq!(stash_view_oid(&stash, 0), Some("worktree"));
        assert_eq!(stash_view_oid(&stash, 1), Some("index"));
        assert_eq!(stash_view_oid(&stash, 2), Some("untracked"));
        assert_eq!(stash_view_oid(&stash, 3), None);
        let tracked_only = with_parents("worktree", &["base", "index"]);
        assert_eq!(stash_view_oid(&tracked_only, 1), Some("index"));
        assert_eq!(stash_view_oid(&tracked_only, 2), None);
        assert_eq!(stash_view_oid(&with_parents("", &[]), 0), None);
    }
    #[test]
    fn comparison_rejects_other_commit_and_parent() {
        let mut metadata = crate::git_fixture::commit_metadata("commit", b"file");
        metadata.parent_index = Some(1);
        let page = newport_protocol::git::Page {
            snapshot: "s".into(),
            entries: vec![],
            next_cursor: None,
            metadata,
        };
        assert!(matches_comparison(&page, "commit", 1));
        assert!(!matches_comparison(&page, "other", 1));
        assert!(!matches_comparison(&page, "commit", 0));
    }
    #[test]
    fn deleted_files_keep_original_byte_path() {
        let path = Path {
            display: "deleted".into(),
            bytes_b64: "/w==".into(),
        };
        let deleted = newport_protocol::git::Delta {
            old_path: path.clone(),
            new_path: path.clone(),
            old_oid: Some(object("old")),
            new_oid: None,
            old_mode: 0o100644,
            new_mode: 0,
            status: "Deleted".into(),
        };
        assert_eq!(file_path(&deleted), path);
    }
}

#[cfg(test)]
mod layout_tests {
    use super::inspection_panels;
    use gpui_kit::{
        self as gpui, Context, IntoElement, Render, TestAppContext, Window, div, prelude::*, px,
        size,
    };

    struct Inspector;
    impl Render for Inspector {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().flex().flex_col().child(inspection_panels(
                window.viewport_size().width <= px(1100.),
                130.,
                div().size_full().child("Changed files").into_any_element(),
                div()
                    .flex_1()
                    .min_h_0()
                    .debug_selector(|| "history-diff-content".into())
                    .child("Diff")
                    .into_any_element(),
                cx,
            ))
        }
    }
    #[gpui::test]
    fn compact_inspector_stacks_bounded_files_above_diff_and_wide_inspector_restores_split(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (_, cx) = cx.add_window_view(|_, _| Inspector);
        for (width, height) in [(1280., 600.), (1100., 600.), (420., 360.), (1280., 600.)] {
            cx.simulate_resize(size(px(width), px(height)));
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let files = cx.debug_bounds("history-files-pane").unwrap();
            let diff = cx.debug_bounds("history-diff-pane").unwrap();
            assert_eq!(
                cx.debug_bounds("history-diff-content").unwrap().size.height,
                diff.size.height,
                "the diff renderer must receive all remaining pane height"
            );
            if width <= 1100. {
                assert_eq!(files.size.height, px(130.));
                assert_eq!(files.size.width, px(width));
                assert_eq!(diff.top(), files.bottom());
                assert_eq!(diff.bottom(), px(height));
                assert_eq!(diff.size.width, files.size.width);
            } else {
                assert_eq!(files.top(), diff.top());
                assert!(diff.left() >= files.right());
                assert_eq!(files.size.height, px(height));
                assert_eq!(diff.size.height, px(height));
                assert!(diff.right() <= px(width));
            }
        }
    }
}
