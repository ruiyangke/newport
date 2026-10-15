//! Commit files and diffs are independent paged reads bound to immutable identities.
use super::*;
use gpui_kit::component::input::TextareaState;
use gpui_kit::component::resizable_panel;
use newport_client::git_history::{MainlineReview, commit_action, complete_commit_message};
#[derive(Default)]
pub(super) struct History {
    pub(super) oid: String,
    stash: Value,
    tag: Value,
    stash_view: usize,
    pub(super) files: Value,
    pub diff: Value,
    pub path: Value,
    pub(super) parent: usize,
    pub(super) file_cursor: Option<String>,
    pub(super) diff_cursor: Option<String>,
    pub scroll: UniformListScrollHandle,
    pub(super) error: Option<String>,
    pub(super) header: git_commit_header::State,
    pub(super) file_demand: git_page_demand::Demand,
}
const KEYS: [&str; 5] = [
    "git-history-detail",
    "git-history-files",
    "git-history-files-more",
    "git-history-diff",
    "git-history-diff-more",
];
impl History {
    fn accept_tag(&mut self, selected: &Value, detail: Value) -> Result<bool, String> {
        let oid = selected["oid"]["hex"]
            .as_str()
            .filter(|oid| !oid.is_empty());
        if oid.is_none() || detail["oid"]["hex"].as_str() != oid {
            return Err("Tag detail does not match the selection.".into());
        }
        let target = tag_commit_oid(&detail).unwrap_or_default();
        if !self.oid.is_empty() && self.oid != target {
            return Err("Tag target does not match the inspected commit.".into());
        }
        let inspect = self.oid.is_empty() && !target.is_empty();
        self.oid = target.to_owned();
        self.tag = detail;
        Ok(inspect)
    }
    /// Headers describe the commit being compared, not the stash that owns it.
    /// The original stash remains separate so its other views stay available.
    fn inspection_detail<'a>(&self, detail: &'a Value) -> Option<&'a Value> {
        (!self.oid.is_empty() && detail["oid"]["hex"].as_str() == Some(&self.oid)).then_some(detail)
    }

    fn select_parent(&mut self, owner: &str, detail: &Value, parent: usize) -> bool {
        if self.oid != owner
            || self.inspection_detail(detail).is_none()
            || !detail["parents"]
                .as_array()
                .is_some_and(|parents| parent < parents.len())
            || parent == self.parent
        {
            return false;
        }
        self.parent = parent;
        self.files = Value::Null;
        self.diff = Value::Null;
        self.path = Value::Null;
        self.file_cursor = None;
        self.diff_cursor = None;
        self.scroll = UniformListScrollHandle::default();
        self.file_demand = git_page_demand::Demand::default();
        self.error = None;
        true
    }
}
/// A tag listing already contains bounded object, target and annotation data.
/// Fetch again only for a truncated annotation or an incomplete preview shape.
fn prime_tag_detail(selected: &Value, resources: &mut Resources) -> bool {
    let complete_shape = selected["oid"]["hex"]
        .as_str()
        .is_some_and(|oid| !oid.is_empty())
        && selected["annotated"].is_boolean()
        && selected["detailsOmitted"].is_boolean()
        && (selected["detailsOmitted"] == true
            || (selected["peeledOid"]["hex"].is_string() && selected["peeledType"].is_string()))
        && (selected["annotated"] == false
            || selected["detailsOmitted"] == true
            || (selected["message"]["display"].is_string()
                && selected["messageTruncated"].is_boolean()));
    if !complete_shape {
        return true;
    }
    if let Some(generation) = resources.begin("git-history-tag") {
        resources.complete("git-history-tag", generation, Ok(selected.clone()));
    }
    selected["messageTruncated"] == true
}
/// History rows already carry commit metadata. Reuse the selected immutable
/// record immediately, fetching detail only when its message was truncated.
/// Stash/tag list entries have different shapes and must still resolve first.
fn prime_commit_detail(tab: usize, selected: &Value, oid: &str, resources: &mut Resources) -> bool {
    if tab != 1
        || oid.is_empty()
        || selected["oid"]["hex"].as_str() != Some(oid)
        || !selected["message"]["display"].is_string()
        || !selected["messageTruncated"].is_boolean()
        || !selected["parents"].is_array()
        || !selected["author"].is_object()
        || !selected["time"].is_i64()
    {
        return true;
    }
    if let Some(generation) = resources.begin("git-history-detail") {
        resources.complete("git-history-detail", generation, Ok(selected.clone()));
    }
    selected["messageTruncated"] == true
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
fn matches_comparison(value: &Value, oid: &str, parent: usize) -> bool {
    let meta = &value["metadata"];
    meta["commitOid"]["hex"].as_str() == Some(oid)
        && meta["parentIndex"].as_u64().unwrap_or(0) == parent as u64
        && value["entries"].is_array()
}
fn tag_commit_oid(tag: &Value) -> Option<&str> {
    (tag["peeledType"] == "commit")
        .then(|| tag["peeledOid"]["hex"].as_str())
        .flatten()
        .filter(|oid| !oid.is_empty())
}
// A stash's second parent is the saved index; its own parent is the stash base.
// The optional third parent contains untracked files and has no parents.
fn stash_view_oid(stash: &Value, view: usize) -> Option<&str> {
    match view {
        0 => stash["oid"]["hex"].as_str(),
        1 | 2 => stash["parents"][view]["hex"].as_str(),
        _ => None,
    }
    .filter(|oid| !oid.is_empty())
}
pub(super) fn file_path(entry: &Value) -> Value {
    if entry["newPath"].is_object() {
        entry["newPath"].clone()
    } else {
        entry["oldPath"].clone()
    }
}
impl Newport {
    fn history_commit_action(&self, kind: &str) -> Option<Value> {
        if self.git.tab != 1
            || !self.integration_ready()
            || self.git.integration_review.is_some()
            || self.git.tag_form.is_some()
            || !self.value("git-tag-status")["metadata"]["integration"].is_null()
            || self.value("git-tag-status")["metadata"]["groupCounts"]["conflicted"]
                .as_u64()
                .unwrap_or(0)
                > 0
            || !self
                .resources
                .get("git-history-detail")
                .is_some_and(|r| !r.pending && r.error.is_none())
        {
            return None;
        }
        if kind == "commit.amend"
            && self.value("git-tag-status")["metadata"]["head"]["oid"]["hex"].as_str()
                != Some(&self.git.history.oid)
        {
            return None;
        }
        let mut action = commit_action(
            kind,
            &self.value("git-history-detail"),
            &self.git.history.oid,
            self.git.history.parent,
        )?;
        if kind == "reset" {
            let status = self.value("git-tag-status");
            let oid = status["metadata"]["head"]["oid"]["hex"].as_str()?;
            if oid.is_empty() {
                return None;
            }
            action["expectedOid"] = json!(oid);
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
        let mainline = if action["mainline"].is_number() {
            let Some(review) = MainlineReview::from_commit(
                &self.value("git-history-detail"),
                &self.git.history.oid,
            ) else {
                return;
            };
            Some(review)
        } else {
            None
        };
        let original = complete_commit_message(&self.value("git-history-detail"));
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
            snapshot: text(&self.value("git-tag-status"), "snapshot"),
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
            let fetch = prime_tag_detail(&self.git.selected, &mut self.resources);
            if !self.value("git-history-tag").is_null() {
                self.git_history_completed("git-history-tag");
            }
            if fetch {
                self.git_load_tag_detail();
            }
            return;
        }
        self.git.history.oid = self.git.selected["oid"]
            .as_str()
            .or_else(|| self.git.selected["oid"]["hex"].as_str())
            .unwrap_or_default()
            .to_owned();
        self.git_inspection_load();
    }
    fn git_inspection_load(&mut self) {
        if prime_commit_detail(
            self.git.tab,
            &self.git.selected,
            &self.git.history.oid,
            &mut self.resources,
        ) {
            self.git_load_commit_detail();
        }
        self.git_history_files(false);
    }
    fn git_load_commit_detail(&mut self) {
        self.request("git-history-detail", "git_request", json!({"serverId":self.server,"request":{"method":"repo.commit","params":{"repoId":self.git_repository_id(),"commitOid":self.git.history.oid}}}));
    }
    fn git_load_tag_detail(&mut self) {
        self.request("git-history-tag", "git_request", json!({"serverId":self.server,"request":{"method":"repo.tag","params":{"repoId":self.git_repository_id(),"oid":self.git.selected["oid"]["hex"]}}}));
    }
    fn git_stash_view(&mut self, owner: &str, view: usize) {
        if self.page != Page::Projects
            || self.git.tab != 3
            || self.git.selected["oid"].as_str() != Some(owner)
            || self.git.history.stash["oid"]["hex"].as_str() != Some(owner)
            || self.git.history.stash_view == view
        {
            return;
        }
        let stash = self.git.history.stash.clone();
        let Some(oid) = stash_view_oid(&stash, view).map(str::to_owned) else {
            return;
        };
        self.git_history_reset();
        self.git.history.stash = stash;
        self.git.history.oid = oid;
        self.git.history.stash_view = view;
        self.git_inspection_load();
    }
    pub(super) fn git_history_parent(&mut self, owner: &str, parent: usize) {
        let detail = self.value("git-history-detail");
        if self.page != Page::Projects
            || !self.git_is_inspection()
            || !self.git.history.select_parent(owner, &detail, parent)
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
        let mut params = json!({"repoId":self.git_repository_id(),"commitOid":self.git.history.oid,"parentIndex":self.git.history.parent,"pageSize":100});
        if more {
            if self.git.history.file_cursor.is_some() {
                return;
            }
            let Some(cursor) = self.git.history.files["nextCursor"]
                .as_str()
                .map(str::to_owned)
            else {
                return;
            };
            params["cursor"] = json!(cursor);
            self.git.history.file_cursor = Some(cursor);
        }
        self.request(key,"git_request",json!({"serverId":self.server,"request":{"method":"repo.commit_files","params":params}}));
    }
    pub(super) fn git_history_diff(&mut self, more: bool) {
        if !self.git_is_inspection() || !self.git.history.path["bytesB64"].is_string() {
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
        let mut params = json!({"repoId":self.git_repository_id(),"commitOid":self.git.history.oid,"parentIndex":self.git.history.parent,"path":self.git.history.path,"pageSize":200,"maxBytes":65536,"contextLines":3});
        if more {
            if self.git.history.diff_cursor.is_some() {
                return;
            }
            let Some(cursor) = self.git.history.diff["nextCursor"]
                .as_str()
                .map(str::to_owned)
            else {
                return;
            };
            params["cursor"] = json!(cursor);
            self.git.history.diff_cursor = Some(cursor);
        }
        self.request(key,"git_request",json!({"serverId":self.server,"request":{"method":"repo.commit_diff_page","params":params}}));
    }
    pub(super) fn git_history_completed(&mut self, key: &str) {
        if self.resources.get(key).is_some_and(|r| r.error.is_some()) {
            return;
        }
        let value = self.value(key);
        if key == "git-history-tag" {
            match self.git.history.accept_tag(&self.git.selected, value) {
                Ok(true) => self.git_inspection_load(),
                Ok(false) => {}
                Err(error) => self.git.history.error = Some(error),
            }
            return;
        }
        let oid = self.git.history.oid.clone();
        if key == "git-history-detail" {
            if value["oid"]["hex"].as_str() != Some(oid.as_str()) {
                self.resources.invalidate(key);
                self.git.history.error = Some("Commit detail does not match the selection.".into());
            } else if self.git.tab == 3 && self.git.history.stash_view == 0 {
                self.git.history.stash = value;
            }
            return;
        }
        let is_diff = key.starts_with("git-history-diff");
        if !matches_comparison(&value, &oid, self.git.history.parent)
            || (is_diff
                && (value["metadata"]["selectedPath"]["bytesB64"]
                    != self.git.history.path["bytesB64"]
                    || value["metadata"]["readOnly"] != true
                    || value["metadata"]["contextLines"] != 3))
        {
            self.git.history.error = Some(
                "Commit response does not match the selected comparison. Select the commit again."
                    .into(),
            );
            return;
        }
        let result = if key.ends_with("-more") {
            let (page, cursor) = if is_diff {
                (&self.git.history.diff, self.git.history.diff_cursor.take())
            } else {
                (&self.git.history.files, self.git.history.file_cursor.take())
            };
            let Some(cursor) = cursor else {
                return;
            };
            if is_diff {
                newport_client::git_diff_page::merge(page, value, &cursor, false)
            } else {
                newport_client::git_page::merge(page, value, &cursor)
            }
        } else {
            Ok(value)
        };
        match result {
            Ok(value) => {
                if is_diff {
                    self.git.history.diff = value;
                } else {
                    let first = self.git.history.accept_files(value, key.ends_with("-more"));
                    if let Some(first) = first {
                        let owner = self.git.history.files["metadata"].clone();
                        self.git_history_select_file(&owner, &first);
                    }
                }
            }
            Err(error) => self.git.history.error = Some(error),
        }
    }
    fn git_tag_inspection(&self, cx: &mut Context<Self>) -> AnyElement {
        let tag = &self.git.history.tag;
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
                    .label("Retry annotation").self_start().m_3()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.page == Page::Projects && this.git.tab == 4
                            && this.server == server && this.git_repository_id() == repo
                            && this.git.selected == owner {
                            this.git_load_tag_detail();
                            cx.notify();
                        }
                    }))
            ))
            .when(tag.is_object(), |view| view.child(
                div().id("git-tag-annotation").max_h(px(260.)).overflow_y_scroll()
                    .p_3().flex().flex_col().gap_2().min_w_0()
                    .border_b_1().border_color(cx.theme().border)
                    .child(div().flex().flex_wrap().gap_2().items_center()
                        .child(div().flex_1().min_w_0().font_weight(FontWeight::SEMIBOLD)
                            .child(text(&self.git.selected["name"], "display")))
                        .child(self.git_tag_push_trigger(cx)).child(self.git_tag_delete_trigger(cx)))
                    .child(label(if tag["annotated"] == true {"Annotated tag"} else {"Lightweight tag"}).text_xs())
                    .child(tag_metadata("object", format!("Object: {}", text(&tag["oid"], "hex"))))
                    .when(tag["peeledOid"]["hex"].is_string(), |view| view.child(tag_metadata(
                        "target", format!("Target: {} ({})", text(&tag["peeledOid"], "hex"), text(tag, "peeledType")))))
                    .when(tag["tagger"].is_object(), |view| view.child(tag_metadata(
                        "tagger", format!("{} <{}>", text(&tag["tagger"], "name"), text(&tag["tagger"], "email")))))
                    .when(tag["message"]["display"].is_string(), |view| view.child(
                        div().id("git-tag-message").role(accesskit::Role::Paragraph)
                            .aria_label(text(&tag["message"], "display"))
                            .min_w_0().text_size(px(12.))
                            .child(text(&tag["message"], "display"))))
                    .when(tag["messageTruncated"] == true, |view| view.child(project_loading::picker_detail(
                        "full tag annotation", Some("The annotation is truncated.".into()), pending)))
                    .when(tag["detailsOmitted"] == true, |view| view.child(label("Some tag details could not be loaded.").text_xs()))
                    .when(self.git.history.oid.is_empty() && tag["detailsOmitted"] != true, |view| view.child(label(
                        "This tag points to a non-commit object; no commit diff is available.").text_xs()))
            )).into_any_element()
    }
    pub(super) fn git_history_view(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = !self.git.history.oid.is_empty();
        let current_detail = self.value("git-history-detail");
        let detail = self.git.history.inspection_detail(&current_detail);
        let parents = rows(&self.git.history.files["metadata"]["parents"]);
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
                            .label("Retry commit details")
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
            .when(
                self.git.tab == 3 && self.git.history.stash["parents"][1]["hex"].is_string(),
                |view| {
                    let mut kinds = vec!["Tracked files", "Staged files"];
                    let owner = text(&self.git.history.stash["oid"], "hex");
                    if stash_view_oid(&self.git.history.stash, 2).is_some() {
                        kinds.push("Untracked files");
                    }
                    view.child(
                        crate::controls::tabs("stash-files-kind")
                            .segmented()
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
                !selected && (self.git.tab != 4 || self.git.selected.is_null()),
                |view| view.child(empty_inspection(self.git.tab, cx)),
            )
            .when_some(self.git.history.error.clone(), |view, error| {
                view.child(label(error).p_3())
            })
            .when(selected, |view| {
                view.child(inspection_panels(
                    window.viewport_size().width <= px(1100.),
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
    label(content.clone())
        .id(SharedString::from(format!("git-tag-{id}")))
        .role(accesskit::Role::Label)
        .aria_label(content.clone())
        .text_xs()
        .truncate()
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(content.clone()).build(window, cx)
        })
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
    #[test]
    fn tag_previews_avoid_redundant_reads_and_keep_truncated_annotations_during_failure() {
        use serde_json::json;
        let mut resources = super::Resources::default();
        let mut tag = json!({"oid":{"hex":"tag"},"annotated":false,"detailsOmitted":false,
            "peeledOid":{"hex":"target"},"peeledType":"commit"});
        assert!(!super::prime_tag_detail(&tag, &mut resources));
        assert_eq!(resources.get("git-history-tag").unwrap().value, tag);
        resources.invalidate("git-history-tag");
        tag["annotated"] = json!(true);
        tag["message"] = json!({"display":"Partial annotation"});
        tag["messageTruncated"] = json!(false);
        assert!(!super::prime_tag_detail(&tag, &mut resources));
        resources.invalidate("git-history-tag");
        tag["messageTruncated"] = json!(true);
        assert!(super::prime_tag_detail(&tag, &mut resources));
        let generation = resources.begin("git-history-tag").unwrap();
        assert_eq!(resources.get("git-history-tag").unwrap().value, tag);
        resources.complete("git-history-tag", generation, Err("Disconnected".into()));
        assert_eq!(resources.get("git-history-tag").unwrap().value, tag);
        let retry = resources.begin("git-history-tag").unwrap();
        tag["messageTruncated"] = json!(false);
        tag["message"]["display"] = json!("Complete annotation");
        resources.complete("git-history-tag", retry, Ok(tag.clone()));
        assert_eq!(resources.get("git-history-tag").unwrap().value, tag);
    }

    #[test]
    fn annotation_completion_preserves_comparison_and_rejects_other_tag_or_target() {
        use serde_json::json;
        let tag = json!({"oid":{"hex":"tag"},"peeledOid":{"hex":"target"},"peeledType":"commit"});
        let mut history = super::History::default();
        assert_eq!(history.accept_tag(&tag, tag.clone()), Ok(true));
        history.parent = 1;
        history.files = json!({"entries":[{}],"nextCursor":"more"});
        history.path = json!({"bytesB64":"ZmlsZQ=="});
        history.diff = json!({"entries":[{}]});
        let mut full = tag.clone();
        full["message"] = json!({"display":"Complete annotation"});
        assert_eq!(history.accept_tag(&tag, full.clone()), Ok(false));
        assert_eq!(history.parent, 1);
        assert_eq!(history.files["nextCursor"], "more");
        assert!(history.path.is_object() && history.diff.is_object());
        let mut wrong = full.clone();
        wrong["oid"]["hex"] = json!("another-tag");
        assert!(history.accept_tag(&tag, wrong).is_err());
        let mut wrong = full.clone();
        wrong["peeledOid"]["hex"] = json!("another-target");
        assert!(history.accept_tag(&tag, wrong).is_err());
        assert_eq!(history.tag, full);
        assert_eq!(history.oid, "target");
    }

    #[test]
    fn incomplete_tag_previews_fetch_detail_but_non_commit_tags_do_not_open_history() {
        use serde_json::json;
        let mut resources = super::Resources::default();
        assert!(super::prime_tag_detail(
            &json!({"oid":{"hex":"tag"}}),
            &mut resources
        ));
        assert!(resources.get("git-history-tag").is_none());
        let tree = json!({"oid":{"hex":"tree"},"annotated":false,"detailsOmitted":false,
            "peeledOid":{"hex":"tree"},"peeledType":"tree"});
        assert!(!super::prime_tag_detail(&tree, &mut resources));
        let mut history = super::History::default();
        assert_eq!(history.accept_tag(&tree, tree.clone()), Ok(false));
        assert!(history.oid.is_empty());
        assert_eq!(history.tag, tree);
        assert!(history.accept_tag(&json!({}), json!({})).is_err());
    }

    #[test]
    fn stash_inspection_header_follows_the_selected_commit() {
        use serde_json::json;
        let stash = json!({"oid":{"hex":"stash"},"parents":[{"hex":"base"},{"hex":"index"},{"hex":"untracked"}],"message":{"display":"Tracked changes"}});
        let mut history = super::History {
            stash: stash.clone(),
            ..Default::default()
        };
        for (view, oid) in [(0, "stash"), (1, "index"), (2, "untracked")] {
            history.oid = super::stash_view_oid(&stash, view).unwrap().into();
            let detail = json!({"oid":{"hex":oid},"message":{"display":format!("View {view}")},"parents":[]});
            assert_eq!(history.inspection_detail(&detail), Some(&detail));
            if view != 0 {
                assert!(
                    history.inspection_detail(&stash).is_none(),
                    "the header and copy action must not use the owning stash ID"
                );
            }
            assert!(
                history
                    .inspection_detail(&serde_json::Value::Null)
                    .is_none()
            );
            assert_eq!(
                history.stash, stash,
                "switching inspection cannot lose the original stash's view targets"
            );
        }
    }

    #[test]
    fn parent_selection_retains_stash_and_tag_but_resets_comparison() {
        use serde_json::json;
        let detail = json!({"oid":{"hex":"stash"},"parents":[{"hex":"base"},{"hex":"index"}]});
        let tag = json!({"oid":{"hex":"tag"},"message":{"display":"Annotation"}});
        let mut history = super::History {
            oid: "stash".into(),
            stash: detail.clone(),
            tag: tag.clone(),
            ..Default::default()
        };
        history.files = json!({"entries":[{}],"nextCursor":"files"});
        history.path = json!({"bytesB64":"ZmlsZQ=="});
        history.diff = json!({"entries":[{}],"nextCursor":"diff"});
        history.file_cursor = Some("files".into());
        history.diff_cursor = Some("diff".into());
        assert!(history.select_parent("stash", &detail, 1));
        assert_eq!(history.parent, 1);
        assert_eq!(history.stash, detail);
        assert_eq!(history.tag, tag);
        assert!(history.files.is_null() && history.path.is_null() && history.diff.is_null());
        assert!(history.file_cursor.is_none() && history.diff_cursor.is_none());
    }

    #[test]
    fn stale_invalid_and_repeated_parent_selections_preserve_current_comparison() {
        use serde_json::json;
        let detail = json!({"oid":{"hex":"current"},"parents":[{"hex":"first"},{"hex":"second"}]});
        let files = json!({"entries":[{}]});
        let mut history = super::History {
            oid: "current".into(),
            files: files.clone(),
            ..Default::default()
        };
        assert!(!history.select_parent("previous", &detail, 1));
        assert!(!history.select_parent(
            "current",
            &json!({"oid":{"hex":"previous"},"parents":[{},{}]}),
            1
        ));
        assert!(!history.select_parent("current", &detail, 2));
        assert!(!history.select_parent("current", &detail, 0));
        assert_eq!(history.files, files);
        assert_eq!(history.parent, 0);
    }

    #[test]
    fn history_preview_avoids_redundant_detail_reads_but_expands_truncated_messages() {
        use newport_client::resources::Resources;
        use serde_json::json;
        let mut resources = Resources::default();
        let mut preview = json!({"oid":{"hex":"abc"},"parents":[],
            "message":{"display":"Subject"},"messageTruncated":false,
            "author":{"name":"Author","email":"author@example.test"},"time":1});
        assert!(!super::prime_commit_detail(
            1,
            &preview,
            "abc",
            &mut resources
        ));
        assert_eq!(resources.get("git-history-detail").unwrap().value, preview);
        assert!(!resources.get("git-history-detail").unwrap().pending);

        resources.invalidate("git-history-detail");
        preview["messageTruncated"] = json!(true);
        assert!(super::prime_commit_detail(
            1,
            &preview,
            "abc",
            &mut resources
        ));
        let generation = resources.begin("git-history-detail").unwrap();
        assert_eq!(resources.get("git-history-detail").unwrap().value, preview);
        assert!(resources.complete("git-history-detail", generation, Err("offline".into())));
        assert_eq!(resources.get("git-history-detail").unwrap().value, preview);
        let retry = resources.begin("git-history-detail").unwrap();
        let mut complete = preview.clone();
        complete["message"]["display"] = json!("Subject\n\nComplete body");
        complete["messageTruncated"] = json!(false);
        assert!(resources.complete("git-history-detail", retry, Ok(complete.clone())));
        assert_eq!(resources.get("git-history-detail").unwrap().value, complete);
    }
    #[test]
    fn previews_cannot_cross_commit_identity_or_replace_stash_and_tag_resolution() {
        use newport_client::resources::Resources;
        use serde_json::json;
        let preview = json!({"oid":{"hex":"abc"},"parents":[],
            "message":{"display":"Subject"},"messageTruncated":false,
            "author":{},"time":1});
        for (tab, oid) in [(1, "other"), (1, ""), (3, "abc"), (4, "abc")] {
            let mut resources = Resources::default();
            assert!(super::prime_commit_detail(
                tab,
                &preview,
                oid,
                &mut resources
            ));
            assert!(resources.get("git-history-detail").is_none());
        }
        let mut resources = Resources::default();
        assert!(super::prime_commit_detail(
            1,
            &json!({"oid":{"hex":"abc"}}),
            "abc",
            &mut resources
        ));
        assert!(resources.get("git-history-detail").is_none());
    }
    use super::{file_path, matches_comparison, stash_view_oid, tag_commit_oid};
    use serde_json::json;
    #[test]
    fn tags_inspect_only_resolved_commit_targets() {
        let commit =
            json!({"peeledType":"commit","peeledOid":{"hex":"commit"},"oid":{"hex":"tag"}});
        assert_eq!(tag_commit_oid(&commit), Some("commit"));
        for kind in ["tree", "blob", "tag"] {
            assert_eq!(
                tag_commit_oid(&json!({"peeledType":kind,"peeledOid":{"hex":"object"}})),
                None
            );
        }
        assert_eq!(tag_commit_oid(&json!({"detailsOmitted":true})), None);
    }
    #[test]
    fn stash_views_select_worktree_index_and_optional_untracked_commits() {
        let stash = json!({"oid":{"hex":"worktree"},"parents":[{"hex":"base"},{"hex":"index"},{"hex":"untracked"}]});
        assert_eq!(stash_view_oid(&stash, 0), Some("worktree"));
        assert_eq!(stash_view_oid(&stash, 1), Some("index"));
        assert_eq!(stash_view_oid(&stash, 2), Some("untracked"));
        assert_eq!(stash_view_oid(&stash, 3), None);
        let tracked_only =
            json!({"oid":{"hex":"worktree"},"parents":[{"hex":"base"},{"hex":"index"}]});
        assert_eq!(stash_view_oid(&tracked_only, 1), Some("index"));
        assert_eq!(stash_view_oid(&tracked_only, 2), None);
        assert_eq!(stash_view_oid(&json!({}), 0), None);
    }
    #[test]
    fn comparison_rejects_other_commit_and_parent() {
        let page = json!({"entries":[],"metadata":{"commitOid":{"hex":"commit"},"parentIndex":1}});
        assert!(matches_comparison(&page, "commit", 1));
        assert!(!matches_comparison(&page, "other", 1));
        assert!(!matches_comparison(&page, "commit", 0));
    }
    #[test]
    fn deleted_files_keep_original_byte_path() {
        let path = json!({"display":"deleted","bytesB64":"/w=="});
        assert_eq!(file_path(&json!({"oldPath":path,"newPath":null})), path);
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
