//! The native Git workspace consumes the same bounded, snapshot-based pages.
use super::*;
use base64::Engine as _;
use gpui_kit::base::Selectable;
use gpui_kit::component::resizable::resizable_panel;
use newport_client::git_page::merge as merge_page;
use newport_client::resources::{ResourceGroup, Resources};
#[derive(Default)]
pub(super) struct Workspace {
    pub(super) changes: git_changes::Changes,
    pub(super) listing: git_listing::State,
    pub(super) branches: git_branch_picker::Picker,
    pub(super) checkouts: git_checkout_picker::Picker,
    pub(super) checkout_summary: git_checkout_picker::Summary,
    saved_views: std::collections::HashMap<usize, SavedView>,
    pub(super) conflict: git_conflict::Preview,
    pub(super) integration_review: Option<git_integration::Review>,
    pub(super) remote_ref_review: Option<git_remote_actions::Review>,
    pub(super) remote_refs: Option<git_remote_refs::Browser>,
    pub(super) settings: Option<git_settings::Form>,
    pub(super) settings_open: bool,
    pub(super) remote_edit: Option<git_remote_edit::Form>,
    pub(super) opening_path: Value,
    // The displayed checkout remains authoritative until repo.open succeeds.
    pub(super) switching_path: Option<Value>,
    switching_repo: Value,
    pub(super) worktree_create: Option<git_worktree_create::Form>,
    pub(super) worktree_remove: Option<git_worktrees::RemoveReview>,
    pub(super) worktree_repair: Option<git_worktrees::RepairForm>,
    pub(super) worktree_lock: Option<git_worktrees::LockForm>,
    pub(super) tag_delete: Option<git_tags::DeleteReview>,
    pub(super) tag_form: Option<git_tags::Form>,
    pub(super) tag_push: Option<git_tag_push::Form>,
    pub(super) stash_save: Option<git_stash::SaveForm>,
    pub(super) stash_apply: Option<git_stash::Review>,
    pub(super) pull_form: Option<git_pull::Form>,
    pub(super) sync_form: Option<git_sync::Form>,
    pub(super) branch_search: Option<git_branch_search::Search>,
    pub(super) branch_form: Option<BranchForm>,
    pub(super) branch_manage: Option<git_branch_manage::Form>,
    pub(super) returned_from_missing_checkout: Option<String>,
    server: String,
    pub(super) repo: Value,
    pub(super) restore_document: bool,
    pub(super) tab: usize,
    pub(super) history: git_history::History,
    pub(super) page: Value,
    pub selected: Value,
    pub(super) detail_open: bool,
    pub diff_staged: bool,
    pub(super) diff_context: git_diff::ContextLines,
    pub(super) diff_presentation: git_diff::Cache,
    pub(super) diff_text: git_diff_text::Cache,
    pub(super) diff_find: Option<git_diff_search::Controls>,
    pub(super) diff: Value,
    pub(super) diff_demand: git_diff::ScrollDemand,
    pub(super) diff_recovery: newport_client::git_read_recovery::Recovery,
    pub(super) index_refresh: Option<git_index_refresh::Refresh>,
    diff_cursor: Option<String>,
    pub diff_scroll: UniformListScrollHandle,
    pub(super) continuation: Option<String>,
    pub scroll: UniformListScrollHandle,
    pub error: Option<String>,
}
impl Workspace {
    /// An open repository with no switch under way: it can be set aside
    /// whole while another page shows.
    pub(super) fn settled(&self) -> bool {
        self.repo["repoId"].is_string()
            && self.switching_path.is_none()
            && !self.switching_repo["repoId"].is_string()
    }
    /// Releases the repositories it holds open on the server.
    pub(super) fn release(&mut self) {
        let pending = std::mem::take(&mut self.switching_repo);
        if pending["repoId"].is_string() && pending["repoId"] != self.repo["repoId"] {
            let server = self.server.clone();
            std::thread::spawn(move || {
                backend::Backend::discover().release_repository(&server, &pending)
            });
        }
        let repo = std::mem::take(&mut self.repo);
        let server = self.server.clone();
        if repo["repoId"].is_string() {
            std::thread::spawn(move || {
                backend::Backend::discover().release_repository(&server, &repo)
            });
        }
    }
}
struct SavedView {
    workspace: Workspace,
    resources: ResourceGroup,
}
impl Workspace {
    fn read_params(&self) -> Value {
        let mut params = json!({"repoId":self.repo["repoId"]});
        params["pageSize"] = json!(if self.tab == 6 { 20 } else { 100 });
        if self.tab == 6 {
            params["filter"] = json!(self.listing.remote_query());
        }
        if self.tab == 0 {
            params["filter"] = self.changes.filter();
        }
        if self.tab == 1 || self.tab == 4 {
            params["messageBytes"] = json!(512);
        }
        params
    }

    fn begin_checkout_switch(&mut self, path: Value) -> bool {
        if self.switching_path.is_some()
            || !self.repo["repoId"].is_string()
            || !path["bytesB64"].is_string()
            || path["bytesB64"] == self.opening_path["bytesB64"]
        {
            return false;
        }
        self.switching_path = Some(path);
        true
    }

    /// Preserve presentation, never checkout-specific selections or snapshots.
    fn replace_checkout(&mut self, path: Value) {
        *self = Self {
            tab: self.tab,
            detail_open: self.detail_open,
            diff_context: self.diff_context,
            opening_path: path,
            ..Default::default()
        };
    }

    fn install_checkout(&mut self, path: Value, repo: Value, page: Value) {
        let tab = if repo["bare"] == true && self.tab == 0 {
            1
        } else {
            self.tab
        };
        self.replace_checkout(path);
        self.tab = tab;
        self.repo = repo;
        self.accept_listing(page);
    }

    pub(super) fn invalidate_saved_views(&mut self) {
        self.saved_views.clear();
    }
    pub(super) fn observe_status_snapshot(&mut self, status: &Value) -> bool {
        let previous = if self.tab == 0 {
            Some(&self.page)
        } else {
            self.saved_views.get(&0).map(|view| &view.workspace.page)
        };
        let changed = status["snapshot"].as_str().is_some_and(|snapshot| {
            previous
                .and_then(|page| page["snapshot"].as_str())
                .is_some_and(|previous| previous != snapshot)
        });
        if changed {
            self.invalidate_saved_views();
        }
        changed
    }
    fn accept_listing(&mut self, page: Value) -> bool {
        self.accept_listing_with_scope(page, false)
    }
    pub(super) fn accept_listing_with_scope(&mut self, page: Value, index_only: bool) -> bool {
        let changed_comparison = self.tab == 0 && self.page["snapshot"] != page["snapshot"];
        if changed_comparison {
            let selected = newport_client::git_read_recovery::matching_entry(
                &page,
                &self.selected["path"],
                self.diff_staged,
            );
            // A fully staged/unstaged file moves groups. Follow that same byte
            // path, while partial staging stays on the current comparison side.
            let other = (index_only && selected.is_none())
                .then(|| {
                    newport_client::git_read_recovery::matching_entry(
                        &page,
                        &self.selected["path"],
                        !self.diff_staged,
                    )
                })
                .flatten();
            let moved = other.is_some();
            self.selected = selected.or(other).unwrap_or(Value::Null);
            if moved {
                self.diff_staged = !self.diff_staged;
            }
            // Keep the previous comparison visible during a background read.
            // Its snapshot no longer matches, so all mutation controls reject it.
            if !index_only || moved || self.selected.is_null() {
                self.diff = Value::Null;
            }
            self.diff_cursor = None;
        }
        if self.tab == 0 && page["metadata"]["head"].is_object() {
            self.repo["head"] = page["metadata"]["head"].clone();
        }
        // A background first-page read must not collapse the pages the user
        // already loaded, reset their cursor, or disturb an unchanged diff.
        if self.tab != 0 || changed_comparison || !self.page["entries"].is_array() {
            self.page = page;
        }
        self.error = None;
        changed_comparison
    }
    /// Keep presentation and completed reads together. In-flight reads are
    /// discarded by generation and reloaded on return, never restored as busy.
    fn switch_view(&mut self, tab: usize, resources: &mut Resources) -> Option<bool> {
        if self.switching_path.is_some()
            || tab == self.tab
            || tab_info(tab).is_none()
            || (tab == 0 && self.repo["bare"] == true)
            || !self.repo["repoId"].is_string()
        {
            return None;
        }
        // Unsubmitted toolbar intent must not survive tab suspension.
        if let Some(mut transfer) = self.sync_form.take() {
            transfer.invalidate(resources);
        }
        self.diff_demand = git_diff::ScrollDemand::default();
        self.index_refresh = None;
        self.diff_recovery.reset();
        self.listing.demand = git_page_demand::Demand::default();
        let mut saved = std::mem::take(&mut self.saved_views);
        let restored = saved.remove(&tab);
        let repo = self.repo.clone();
        let server = self.server.clone();
        // Checkout identity belongs to the repository, not to a suspended tab.
        // Keep the requested byte path even when restoring an older view.
        let opening_path = self.opening_path.clone();
        let checkout_summary = self.checkout_summary.clone();
        let current = SavedView {
            workspace: std::mem::take(self),
            resources: resources.take_prefix_except(
                "git-",
                &[
                    "git-log-events",
                    "git-sync-status",
                    git_checkout_picker::SUMMARY,
                ],
            ),
        };
        saved.insert(current.workspace.tab, current);
        let reload = match restored {
            Some(previous) => {
                let reload = previous.resources.had_pending();
                resources.restore(previous.resources);
                *self = previous.workspace;
                reload
            }
            None => true,
        };
        self.repo = repo;
        self.server = server;
        self.opening_path = opening_path;
        self.checkout_summary = checkout_summary;
        self.tab = tab;
        self.saved_views = saved;
        Some(reload)
    }
}
pub(super) struct BranchForm {
    pub id: uuid::Uuid,
    pub name: Entity<InputState>,
    snapshot: String,
    oid: String,
    source: String,
    rename: bool,
    _subscription: Subscription,
}
impl BranchForm {
    pub(super) fn title(&self) -> &'static str {
        if self.rename {
            "Rename branch"
        } else {
            "New branch"
        }
    }
}
// View IDs are stable; page 5 is unused and not routable.
fn tab_info(tab: usize) -> Option<(&'static str, &'static str)> {
    Some(match tab {
        0 => ("Changes", "repo.status"),
        1 => ("History", "repo.history"),
        2 => ("Branches", "repo.branches"),
        3 => ("Stashes", "repo.stashes"),
        4 => ("Tags", "repo.tags"),
        6 => ("Remotes", "repo.remote_names"),
        _ => return None,
    })
}
pub(super) fn worktree_path(entry: &Value) -> Option<Value> {
    if entry["state"] != "available" || entry["current"] != false || entry["kind"] == "bare" {
        return None;
    }
    let path = &entry["path"];
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(path["bytesB64"].as_str()?)
        .ok()?;
    (bytes.starts_with(b"/") && !bytes.contains(&0) && path["display"].is_string())
        .then(|| path.clone())
}
pub(super) fn head_label(head: &Value) -> String {
    if head["detached"] == true {
        format!(
            "Detached HEAD · {}",
            text(&head["oid"], "hex")
                .chars()
                .take(8)
                .collect::<String>()
        )
    } else {
        text(&head["name"], "display")
            .strip_prefix("refs/heads/")
            .unwrap_or(&text(&head["name"], "display"))
            .to_owned()
    }
}
fn listing_feedback(page: &Value, pending: bool, failed: bool) -> String {
    if pending {
        return "Loading repository…".into();
    }
    if failed {
        return "Couldn’t refresh this view".into();
    }
    let Some(entries) = page["entries"].as_array() else {
        return String::new();
    };
    if page["metadata"]["truncated"] == true {
        return format!("{} entries shown · Listing incomplete", entries.len());
    }
    if page["nextCursor"].is_string() {
        return format!("{} entries shown · More available", entries.len());
    }
    format!(
        "{} {}",
        entries.len(),
        if entries.len() == 1 {
            "entry"
        } else {
            "entries"
        }
    )
}
/// Age of Newport's successful read, not a claim that origin was contacted.
fn read_feedback(age: Option<Duration>) -> String {
    let Some(age) = age else {
        return "Not read yet".into();
    };
    let seconds = age.as_secs();
    if seconds < 45 {
        return "Read just now".into();
    }
    let minutes = (seconds + 30) / 60;
    let (amount, unit) = if minutes < 60 {
        (minutes, "minute")
    } else {
        ((minutes + 30) / 60, "hour")
    };
    format!(
        "Read {amount} {unit}{} ago",
        if amount == 1 { "" } else { "s" }
    )
}
fn change_mark(entry: &Value) -> &'static str {
    if entry["conflicted"] == true {
        return "!";
    }
    if entry["untracked"] == true {
        return "?";
    }
    let flags = entry["flags"].as_u64().unwrap_or(0);
    if entry["staged"] == true {
        if flags & 1 != 0 {
            "A"
        } else if flags & (1 << 2) != 0 {
            "D"
        } else if flags & (1 << 3) != 0 {
            "R"
        } else {
            "M"
        }
    } else if flags & (1 << 9) != 0 {
        "D"
    } else if flags & (1 << 11) != 0 {
        "R"
    } else {
        "M"
    }
}
fn checkout_target(branch: &Value) -> Option<(String, String)> {
    if branch["current"] != false {
        return None;
    }
    local_branch_target(branch)
}
pub(super) fn local_branch_target(branch: &Value) -> Option<(String, String)> {
    let name = branch["name"]["display"].as_str()?;
    let oid = branch["oid"]["hex"].as_str()?;
    (branch["remote"] == false
        && !name.is_empty()
        && !oid.is_empty()
        && branch["name"]["bytesB64"] == base64::engine::general_purpose::STANDARD.encode(name))
    .then(|| (name.to_owned(), oid.to_owned()))
}
impl Newport {
    pub(super) fn git_open_worktree_path(&mut self, path: Value, cx: &mut Context<Self>) {
        if self.git_write_pending() || !self.git.begin_checkout_switch(path.clone()) {
            return;
        }
        self.close_checkout_picker();
        self.git_branch_picker_close();
        self.project_checks = None;
        self.resources.invalidate("repository");
        self.request(
            "repository",
            "git_request",
            json!({"serverId":self.server,"request":{"method":"repo.open","params":{"path":path}}}),
        );
        if let Some(error) = self
            .resources
            .get("repository")
            .and_then(|r| r.error.clone())
        {
            self.git_cancel_checkout_switch(error);
        }
        cx.notify();
    }
    pub(super) fn git_can_write_branch(&self) -> bool {
        self.git.branches.open
            && self.git.integration_review.is_none()
            && !self.git_mutation_blocks_write()
            && self.git.repo["capabilities"]["readOnly"] == false
            && self.git.repo["capabilities"]["workingTree"] == true
            && self.git.branches.error.is_none()
            && ["git-branches", "git-branch-status"].iter().all(|key| {
                self.resources
                    .get(key)
                    .is_some_and(|r| !r.pending && r.error.is_none())
            })
            && self.value("git-branch-status")["snapshot"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
            && self.value("git-branch-status")["metadata"]["integration"].is_null()
    }
    pub(super) fn git_can_checkout(&self) -> bool {
        self.git_can_write_branch()
            && !self.git_branch_review_open()
            && checkout_target(&self.git.branches.selected).is_some()
            && self.git_branch_worktree_unoccupied()
    }
    pub(super) fn git_new_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.git_can_write_branch() || self.git_branch_review_open() {
            return;
        }
        let status = self.value("git-branch-status");
        let head = &status["metadata"]["head"];
        let oid = text(&head["oid"], "hex");
        if oid.is_empty() {
            return;
        }
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Branch name"));
        crate::controls::focus_editor(&name, window, cx);
        let subscription = Self::project_form_input(
            &name,
            project_dialog::Kind::Branch,
            Self::git_create_branch,
            cx,
        );
        self.git.branch_form = Some(BranchForm {
            id: uuid::Uuid::new_v4(),
            name,
            snapshot: text(&status, "snapshot"),
            oid,
            source: text(&head["name"], "display"),
            rename: false,
            _subscription: subscription,
        });
        cx.notify();
    }
    pub(super) fn git_rename_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.git_can_write_branch() || self.git_branch_review_open() {
            return;
        }
        let Some((source, oid)) = local_branch_target(&self.git.branches.selected) else {
            return;
        };
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("New branch name"));
        crate::controls::focus_editor(&name, window, cx);
        name.update(cx, |input, cx| input.set_value(source.clone(), window, cx));
        let subscription = Self::project_form_input(
            &name,
            project_dialog::Kind::Branch,
            Self::git_create_branch,
            cx,
        );
        self.git.branch_form = Some(BranchForm {
            id: uuid::Uuid::new_v4(),
            name,
            snapshot: text(&self.value("git-branch-status"), "snapshot"),
            oid,
            source,
            rename: true,
            _subscription: subscription,
        });
        cx.notify();
    }
    fn git_create_branch(&mut self, cx: &mut Context<Self>) {
        if !self.git_can_write_branch() {
            return;
        }
        let Some(form) = &self.git.branch_form else {
            return;
        };
        let name = form.name.read(cx).value().to_string();
        if name.trim().is_empty() {
            return;
        }
        let id = form.id;
        let snapshot = form.snapshot.clone();
        let oid = form.oid.clone();
        let captured_name = name.clone();
        let source = form.source.clone();
        let rename = form.rename;
        if rename && source == name {
            return;
        }
        self.start_git_mutation(
            self.git_repository_id(),
            if rename {
                format!("Rename {source} to {name}")
            } else {
                format!("Create branch {name}")
            },
            move |backend, server, repo, operation| {
                if rename {
                    backend.rename_branch(
                        server,
                        repo,
                        &snapshot,
                        (&source, &captured_name, &oid),
                        operation,
                    )
                } else {
                    backend.create_branch(
                        server,
                        repo,
                        &snapshot,
                        (&captured_name, &oid),
                        operation,
                    )
                }
            },
            cx,
        );
        if let Some(mutation) = self.git_mutation.as_mut() {
            mutation.branch_form = Some((id, name));
        }
    }
    pub(super) fn git_branch_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.git.branch_form else {
            return div().into_any_element();
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .py_2()
            .child(div().font_weight(FontWeight::SEMIBOLD).child(form.title()))
            .child(
                crate::label(format!(
                    "{} {} · {}",
                    if form.rename {
                        "Rename"
                    } else {
                        "New branch from"
                    },
                    form.source,
                    form.oid.chars().take(8).collect::<String>()
                ))
                .text_size(px(crate::ui::FONT_SIZE)),
            )
            .child(
                div().w_full().min_w_0().child(
                    crate::controls::input(&form.name)
                        .aria_label(if form.rename {
                            "Branch name"
                        } else {
                            "New branch name"
                        })
                        .disabled(self.git_mutation_blocks_write()),
                ),
            )
            .child(
                project_dialog::actions()
                    .child(
                        crate::controls::button("create-branch-cancel")
                            .label("Cancel")
                            .disabled(self.git_write_pending())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.git.branch_form = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        crate::controls::button("create-branch-submit")
                            .primary()
                            .loading(self.git_write_pending())
                            .label(if form.rename {
                                "Rename branch"
                            } else {
                                "Create branch"
                            })
                            .disabled(
                                !self.git_can_write_branch()
                                    || form.name.read(cx).value().trim().is_empty()
                                    || (form.rename
                                        && form.name.read(cx).value().as_str() == form.source),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.git_create_branch(cx))),
                    ),
            )
            .into_any_element()
    }
    pub(super) fn git_checkout(&mut self, cx: &mut Context<Self>) {
        if !self.git_can_checkout() {
            return;
        }
        let Some((branch, oid)) = checkout_target(&self.git.branches.selected) else {
            return;
        };
        let snapshot = text(&self.value("git-branch-status"), "snapshot");
        self.start_git_mutation(
            self.git_repository_id(),
            format!("Switch to {branch}"),
            move |backend, server, repo, id| {
                backend.checkout(server, repo, &snapshot, (&branch, &oid), id)
            },
            cx,
        );
    }
    pub(super) fn git_can_change_index(&self, stage: bool) -> bool {
        self.git_can_change_entry(stage, &self.git.selected)
    }
    pub(super) fn git_can_change_entry(&self, stage: bool, entry: &Value) -> bool {
        self.git.tab == 0
            && !self.git_mutation_blocks_write()
            && self.git.repo["capabilities"]["readOnly"] == false
            && self.git.repo["capabilities"]["workingTree"] == true
            && self.git.repo["bare"] == false
            && self.git.page["snapshot"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
            && entry["entryId"].as_str().is_some_and(|s| !s.is_empty())
            && self.git.error.is_none()
            && !self
                .resources
                .get("git-list")
                .is_some_and(|r| r.pending || r.error.is_some())
            && if stage {
                entry["unstaged"] == true
                    || entry["untracked"] == true
                    || entry["conflicted"] == true
            } else {
                entry["staged"] == true && self.git.page["metadata"]["integration"].is_null()
            }
    }
    pub(super) fn git_change_index(&mut self, stage: bool, cx: &mut Context<Self>) {
        if !self.git_can_change_index(stage) {
            return;
        }
        let snapshot = text(&self.git.page, "snapshot");
        let entry = text(&self.git.selected, "entryId");
        let title = format!(
            "{} {}",
            if stage { "Stage" } else { "Unstage" },
            self.diff_title()
        );
        self.start_git_index_mutation(
            self.git_repository_id(),
            title,
            move |backend, server, repo, id| {
                backend.change_index(server, repo, &snapshot, &entry, stage, id)
            },
            cx,
        );
    }
    pub(super) fn git_repository_id(&self) -> String {
        self.git.repo["repoId"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }
    pub(super) fn diff_page(&self) -> &Value {
        if self.git_is_inspection() {
            return &self.git.history.diff;
        }
        &self.git.diff
    }
    pub(super) fn diff_title(&self) -> String {
        if self.git_is_inspection() {
            return text(&self.git.history.path, "display");
        }
        text(&self.git.selected["path"], "display")
    }
    pub(super) fn git_release(&mut self) {
        self.git.release();
    }
    pub(super) fn git_refresh(&mut self) {
        if self.git.switching_path.is_some() || !self.git.repo["repoId"].is_string() {
            return;
        }
        self.git_cancel_index_refresh();
        // Local resource invalidation alone can return the backend's still-fresh
        // cache. Wait for scoped invalidation before dispatching replacement reads.
        self.request_refresh(
            "git-refresh",
            "git_refresh_repository",
            json!({"serverId":self.server,"repoId":self.git.repo["repoId"]}),
        );
        if let Some(error) = self
            .resources
            .get("git-refresh")
            .and_then(|r| r.error.clone())
        {
            self.git.error = Some(error);
        }
    }
    fn git_refresh_completed(&mut self) {
        if let Some(error) = self
            .resources
            .get("git-refresh")
            .and_then(|r| r.error.clone())
        {
            self.git.error = Some(error);
            return;
        }
        if self.git.switching_path.is_some()
            || self.value("git-refresh")["repoId"] != self.git.repo["repoId"]
        {
            return;
        }
        // A write or explicit refresh invalidates every cached view of this
        // checkout, including snapshots captured by inactive review forms.
        self.git.invalidate_saved_views();
        self.resources.invalidate("git-sync-status");
        if let Some(form) = self.git.sync_form.as_mut() {
            form.invalidate(&mut self.resources);
        }
        self.git_reload_view();
        if self.git.branches.open {
            self.git_branch_picker_reload();
        }
        self.refresh_checkout_picker();
    }
    fn git_reload_view(&mut self) {
        self.git.diff_recovery.reset();
        if let Some(form) = self.git.branch_manage.as_mut() {
            form.needs_review = true;
        }
        if let Some(review) = self.git.remote_ref_review.as_mut() {
            review.stale = true;
        }
        if let Some(form) = self.git.settings.as_mut() {
            form.needs_review = true;
        }
        if let Some(form) = self.git.remote_edit.as_mut() {
            form.needs_review = true;
        }
        self.git_history_reset();
        self.reset_conflict_preview();
        for key in [
            "git-list",
            "git-more",
            "git-detail",
            "git-diff",
            "git-diff-more",
            "git-branch-status",
            "git-branch-upstreams",
            "git-branch-upstreams-more",
            "git-worktree-list",
            "git-branch-worktree-owner",
            "git-remote-status",
            "git-remote-detail",
            "git-remote-review",
            "git-repository-settings",
            "git-stash-status",
            "git-tag-status",
        ] {
            self.resources.invalidate(key);
        }
        self.git.page = Value::Null;
        self.git.continuation = None;
        self.git.error = None;
        let remote = self.git.selected["name"].as_str().map(str::to_owned);
        self.git.selected = Value::Null;
        if self.git.tab == 6
            && let Some(name) = remote
        {
            self.git.selected = json!({"name":name});
        }
        self.git.pull_form = None;
        self.git.stash_apply = None;
        self.git.tag_delete = None;
        self.git.tag_push = None;
        self.git.worktree_remove = None;
        self.git.diff = Value::Null;
        self.git.diff_cursor = None;
        self.git_load();
    }
    pub(super) fn git_reset_changes_filter(&mut self) {
        self.git_cancel_index_refresh();
        self.git.diff_recovery.reset();
        for key in ["git-list", "git-more", "git-diff", "git-diff-more"] {
            self.resources.invalidate(key);
        }
        self.reset_conflict_preview();
        self.git.page = Value::Null;
        self.git.selected = Value::Null;
        self.git.diff = Value::Null;
        self.git.continuation = None;
        self.git.diff_cursor = None;
        self.git.error = None;
        self.git.scroll = UniformListScrollHandle::default();
        self.git.listing.demand = git_page_demand::Demand::default();
    }
    fn git_cancel_checkout_switch(&mut self, error: String) {
        let path = self.git.switching_path.take().unwrap_or_default();
        let repo = std::mem::take(&mut self.git.switching_repo);
        if repo["repoId"].is_string() && repo["repoId"] != self.git.repo["repoId"] {
            let server = self.server.clone();
            std::thread::spawn(move || {
                backend::Backend::discover().release_repository(&server, &repo)
            });
        }
        self.resources.invalidate("repository");
        self.resources.invalidate("git-checkout-initial");
        self.notification = Some(Notification::error(format!(
            "Couldn’t open {}: {error}",
            text(&path, "display")
        )));
    }
    pub(super) fn git_completed(&mut self, key: &str) {
        if matches!(key, "git-index-refresh" | "git-index-status") {
            self.git_index_refresh_completed(key);
            return;
        }
        if key == "git-refresh" {
            self.git_refresh_completed();
            return;
        }
        if self.git.switching_path.is_some() && matches!(key, "repository" | "git-checkout-initial")
        {
            if let Some(error) = self.resources.get(key).and_then(|r| r.error.clone()) {
                self.git_cancel_checkout_switch(error);
                return;
            }
            if key == "repository" {
                let repo = self.value(key);
                if !repo["repoId"].is_string() {
                    self.git_cancel_checkout_switch("Invalid repository response.".into());
                    return;
                }
                // Prepare the new checkout behind the current view. Its first
                // page uses the same defaults as the fresh workspace below.
                let next = Workspace {
                    tab: if repo["bare"] == true && self.git.tab == 0 {
                        1
                    } else {
                        self.git.tab
                    },
                    repo: repo.clone(),
                    ..Default::default()
                };
                let (_, method) = tab_info(next.tab).expect("active Git tab");
                self.git.switching_repo = repo;
                self.resources.invalidate("git-checkout-initial");
                self.request(
                    "git-checkout-initial",
                    "git_request",
                    json!({
                        "serverId": self.server,
                        "request": { "method": method, "params": next.read_params() }
                    }),
                );
                // Queue saturation completes synchronously, without a reply.
                if let Some(error) = self
                    .resources
                    .get("git-checkout-initial")
                    .and_then(|r| r.error.clone())
                {
                    self.git_cancel_checkout_switch(error);
                }
                return;
            }
            let first_page = self.value(key);
            if !first_page["entries"].is_array() {
                self.git_cancel_checkout_switch("Invalid repository listing.".into());
                return;
            }
            let repo = std::mem::take(&mut self.git.switching_repo);
            if self.git.tab == 6
                && let Err(error) = newport_client::git_remotes::Names::validate_first(&first_page)
            {
                self.git.switching_repo = repo;
                self.git_cancel_checkout_switch(error);
                return;
            }
            let path = self.git.switching_path.take().unwrap();
            if repo["repoId"] != self.git.repo["repoId"] {
                self.git_release();
            }
            // Commit descriptor and listing in the same UI update; stale reads
            // from the old checkout cannot attach to the new one.
            git_project_picker::clear_checkout(&mut self.resources);
            self.restore_git = None;
            self.git
                .install_checkout(path, repo.clone(), first_page.clone());
            let generation = self.resources.begin("git-list").unwrap();
            self.resources
                .complete("git-list", generation, Ok(first_page));
            self.git_repository_opened(self.git.repo.clone());
            return;
        }
        if self.git_recover_missing_checkout(key) {
            return;
        }
        if key == git_checkout_picker::SUMMARY {
            self.checkout_summary_completed();
            return;
        }
        if key == "git-checkouts" || key == "git-checkouts-more" {
            self.checkout_picker_completed(key);
            return;
        }
        if matches!(
            key,
            "git-branches"
                | "git-branches-more"
                | "git-branches-prefetch"
                | "git-branch-worktree-owner"
        ) {
            self.git_branch_picker_completed(key);
            return;
        }
        if key.starts_with("git-branch-upstreams") {
            self.branch_choices_completed(key);
            return;
        }
        if key.starts_with("git-conflict-") {
            self.conflict_preview_completed(key);
            return;
        }
        if key.starts_with("git-remote-refs-") {
            self.remote_refs_completed(key);
            return;
        }
        if key.starts_with("git-history-") {
            self.git_history_completed(key);
            return;
        }
        if self.resources.get(key).is_some_and(|r| r.error.is_some()) {
            if matches!(key, "git-diff" | "git-diff-more") {
                self.git_recover_stale_diff(key);
            } else if key == "git-list" {
                self.git.diff_recovery.reset();
            }
            return;
        }
        match key {
            "git-sync-status" => {
                self.git_watch_completed();
            }
            "git-branch-status" | "git-remote-status" | "git-tag-status" | "git-stash-status" => {
                let status = self.value(key);
                if status["metadata"]["head"].is_object() {
                    self.git.repo["head"] = status["metadata"]["head"].clone();
                }
            }
            "repository" => {
                self.git_repository_opened(self.value(key));
            }
            "git-list" => {
                if self.git.tab == 6
                    && let Err(error) =
                        newport_client::git_remotes::Names::validate_first(&self.value(key))
                {
                    self.git.error = Some(error);
                    return;
                }
                self.git_listing_completed(false);
            }
            "git-more" => {
                if let Some(cursor) = self.git.continuation.take() {
                    let merged = if self.git.tab == 6 {
                        newport_client::git_remotes::Names::merge_values(
                            &self.git.page,
                            self.value(key),
                            &cursor,
                        )
                    } else {
                        merge_page(&self.git.page, self.value(key), &cursor)
                    };
                    match merged {
                        Ok(p) => self.git.page = p,
                        Err(e) => self.git.error = Some(e),
                    }
                }
            }
            "git-diff" | "git-diff-more" => {
                let result = self.value(key);
                if result["metadata"]["sourceSnapshot"] != self.git.page["snapshot"]
                    || result["metadata"]["entryId"] != self.git.selected["entryId"]
                    || !self.git.diff_context.matches(&result, self.git.diff_staged)
                {
                    self.git.error =
                        Some("The file changed. Refresh before reviewing its diff.".into());
                    return;
                }
                if key == "git-diff" {
                    self.git.diff = result;
                    self.git.diff_recovery.reset();
                } else if let Some(cursor) = self.git.diff_cursor.take() {
                    match newport_client::git_diff_page::merge(
                        &self.git.diff,
                        result,
                        &cursor,
                        true,
                    ) {
                        Ok(page) => self.git.diff = page,
                        Err(error) => self.git.error = Some(error),
                    }
                }
            }
            _ => {}
        }
    }
    pub(super) fn git_listing_completed(&mut self, index_only: bool) {
        let page = self.value("git-list");
        let comparison_changed = if index_only {
            self.git.accept_listing_with_scope(page, true)
        } else {
            self.git.accept_listing(page)
        };
        if comparison_changed {
            for key in ["git-diff", "git-diff-more"] {
                self.resources.invalidate(key);
            }
            self.reset_conflict_preview();
            self.resources.invalidate("git-clean-commit");
        }
        self.load_clean_commit();
        if let Some((entry, staged)) = self.git.diff_recovery.resolve(&self.git.page) {
            self.git.selected = entry;
            self.git.diff_staged = staged;
            self.git_diff_request(false);
        } else if comparison_changed && self.git.selected["entryId"].is_string() {
            self.git_diff_request(false);
        }
    }
    pub(super) fn git_params(&self) -> Value {
        self.git.read_params()
    }

    fn git_recover_stale_diff(&mut self, key: &str) {
        use newport_client::git_read_recovery::Decision;
        if self.page != Page::Projects || self.git.tab != 0 || self.git_mutation_blocks_write() {
            return;
        }
        let code = self
            .resources
            .get(key)
            .and_then(|r| r.error_code.as_deref());
        match self
            .git
            .diff_recovery
            .begin(code, &self.git.selected, self.git.diff_staged)
        {
            Decision::Ignore => return,
            Decision::Exhausted => {
                self.git.error = Some(
                    "This repository keeps changing while its diff is read. Refresh to try again."
                        .into(),
                );
                return;
            }
            Decision::Refresh => {}
        }
        // Invalidate every old comparison generation before starting the status
        // read. The shared recovery state holds only the path + requested side.
        self.git.invalidate_saved_views();
        for key in [
            "git-list",
            "git-more",
            "git-diff",
            "git-diff-more",
            "git-sync-status",
        ] {
            self.resources.invalidate(key);
        }
        self.git.page = Value::Null;
        self.git.selected = Value::Null;
        self.git.diff = Value::Null;
        self.git.continuation = None;
        self.git.diff_cursor = None;
        self.git.diff_demand = git_diff::ScrollDemand::default();
        self.git.diff_scroll = UniformListScrollHandle::default();
        self.git.error = None;
        self.git_load();
    }
    fn git_diff_request(&mut self, more: bool) {
        if self.git.index_refresh.is_some()
            || !self.git.selected["entryId"].is_string()
            || self.git.error.is_some()
        {
            return;
        }
        let side = if self.git.diff_staged {
            "head_to_index"
        } else {
            "index_to_worktree"
        };
        let mut params = json!({"repoId":self.git.repo["repoId"],"snapshot":self.git.page["snapshot"],"entryId":self.git.selected["entryId"],"side":side,"pageSize":200,"maxBytes":65536,"contextLines":self.git.diff_context.value()});
        if more {
            if self.git.diff_cursor.is_some()
                || self
                    .resources
                    .get("git-diff-more")
                    .is_some_and(|r| r.error.is_some())
            {
                return;
            }
            let Some(cursor) = self.git.diff["nextCursor"].as_str().map(str::to_owned) else {
                return;
            };
            params["cursor"] = json!(cursor);
            self.git.diff_cursor = Some(cursor);
        }
        self.request(
            if more { "git-diff-more" } else { "git-diff" },
            "git_request",
            json!({"serverId":self.server,"request":{"method":"repo.diff_page","params":params}}),
        );
    }
    pub(super) fn git_diff_more(&mut self) {
        if self.git_is_inspection() {
            self.git_history_diff(true);
            return;
        }
        self.git_diff_request(true);
    }
    pub(super) fn git_select_side(&mut self, staged: bool) {
        self.git.diff_recovery.reset();
        self.reset_conflict_preview();
        self.git.diff_staged = staged;
        for key in ["git-diff", "git-diff-more"] {
            self.resources.invalidate(key);
        }
        self.git.diff = Value::Null;
        self.git.diff_cursor = None;
        self.git.diff_scroll = UniformListScrollHandle::default();
        self.git.error = None;
        if self.git.selected["conflicted"] == true {
            self.select_conflict_side(0);
        } else {
            self.git_diff_request(false);
        }
    }
    fn git_repository_opened(&mut self, repo: Value) {
        self.git.server = self.server.clone();
        self.git.repo = repo;
        self.git.restore_document = true;
        self.load_checkout_summary();
        if let Some(path) = self.git.returned_from_missing_checkout.take() {
            self.notification = Some(Notification::info(format!(
                "The worktree {path} is no longer available. Opened the saved project."
            )));
        }
        if self.git.repo["bare"] == true && self.git.tab == 0 {
            self.git.tab = 1;
        }
        self.save_workspace_selection();
        if self.git.page["entries"].is_array() {
            self.git_load_related();
            self.load_clean_commit();
        } else {
            self.git_load();
        }
    }

    pub(super) fn git_load(&mut self) {
        if self.git.index_refresh.is_some() {
            return;
        }
        let Some((_, method)) = tab_info(self.git.tab) else {
            return;
        };
        if !self.git.repo["repoId"].is_string() {
            return;
        }
        if self.git.tab == 0 {
            self.git.changes.dispatched();
            self.resources.invalidate("git-clean-commit");
        }
        self.request(
            "git-list",
            "git_request",
            json!({"serverId":self.server,"request":{"method":method,"params":self.git_params()}}),
        );
        self.git_load_related();
    }
    fn git_load_related(&mut self) {
        if self.git.tab == 6 {
            self.git_select_remote();
            self.git_load_remote_review();
            if self.git.settings_open || self.git.settings.is_some() {
                self.git_load_repository_settings();
            }
            self.request("git-remote-status", "git_request", json!({"serverId":self.server,"request":{"method":"repo.status","params":{"repoId":self.git.repo["repoId"],"pageSize":1}}}));
        }
        if matches!(self.git.tab, 1 | 4) {
            self.request("git-tag-status", "git_request", json!({"serverId":self.server,"request":{"method":"repo.status","params":{"repoId":self.git.repo["repoId"],"pageSize":1}}}));
        }
        if self.git.tab == 3 {
            self.request("git-stash-status", "git_request", json!({"serverId":self.server,"request":{"method":"repo.status","params":{"repoId":self.git.repo["repoId"],"pageSize":1}}}));
        }
    }
    pub(super) fn git_listing_scroll(&mut self) {
        let ready = self.page == Page::Projects
            && self.git.remote_refs.is_none()
            && !self.project_dialog_open
            && (!git_layout::compact(self.content_width, self.git.tab) || !self.git.detail_open)
            && self.git.error.is_none()
            && self.git.continuation.is_none()
            && self.git.page["nextCursor"].is_string()
            && ["git-list", "git-more"].iter().all(|key| {
                self.resources
                    .get(key)
                    .is_none_or(|r| !r.pending && r.error.is_none())
            });
        // A short page has no scroll extent. An explicit downward gesture
        // still requests one page; passive layout never arms demand.
        let at_end = self.git.scroll.is_scrolled_to_end() != Some(false);
        let count = if ready {
            self.git_visible_row_count()
        } else {
            0
        };
        let scrolled = self.git.listing.demand.take_scrolled(
            &self.git.scroll,
            count,
            git_listing::row_height(self.git.tab),
            ready,
        );
        if self.git.listing.demand.take(&self.git.page, ready, at_end) || (ready && scrolled) {
            self.git_more();
        }
    }
    pub(super) fn git_more(&mut self) {
        if self.git.index_refresh.is_some() {
            return;
        }
        let Some((_, method)) = tab_info(self.git.tab) else {
            return;
        };
        if self.git.error.is_some()
            || self
                .resources
                .get("git-list")
                .is_some_and(|r| r.pending || r.error.is_some())
            || self.git.continuation.is_some()
            || self
                .resources
                .get("git-more")
                .is_some_and(|r| r.error.is_some())
        {
            return;
        }
        let Some(cursor) = self.git.page["nextCursor"].as_str().map(str::to_owned) else {
            return;
        };
        let mut params = self.git_params();
        params["cursor"] = json!(cursor);
        self.git.continuation = Some(cursor);
        self.request(
            "git-more",
            "git_request",
            json!({"serverId":self.server,"request":{"method":method,"params":params}}),
        );
    }
    pub(super) fn git_navigate(&mut self, tab: usize, cx: &mut Context<Self>) {
        self.project_show_review(cx);
        if let Some(editor) = self.code_workspace() {
            editor.update(cx, |e, cx| {
                if tab > 1 {
                    e.files = false;
                    e.navigator.panel = 3;
                    e.navigator.documents.active = None;
                }
                cx.notify();
            });
        }
        self.git_branch_picker_close();
        let Some(reload) = self.git.switch_view(tab, &mut self.resources) else {
            return;
        };
        if reload {
            self.git_reload_view();
        }
        self.save_workspace_selection();
        cx.notify();
    }
    pub(super) fn git_read_feedback(&self) -> String {
        if let Some(path) = &self.git.switching_path {
            return format!("Opening {}…", text(path, "display"));
        }
        let resource = self.resources.get("git-list");
        let pending = resource.is_some_and(|read| read.pending);
        let failed = resource.is_some_and(|read| read.error.is_some()) || self.git.error.is_some();
        // Keep incomplete/error/filter feedback visible instead of presenting
        // a successful read as if it covered the entire listing.
        if pending || failed || self.git.page["metadata"]["truncated"] == true {
            return listing_feedback(&self.git.page, pending, failed);
        }
        if let Some(filtered) = self.git_filtered_feedback() {
            return filtered;
        }
        let age = read_feedback(
            resource
                .and_then(|read| read.updated_at)
                .map(|at| at.elapsed()),
        );
        if self.git.page["nextCursor"].is_string() {
            format!("{age} · {}", listing_feedback(&self.git.page, false, false))
        } else {
            age
        }
    }
    pub(super) fn git_tools_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        crate::controls::icon_button("git-tools")
            .ghost()
            .icon(IconName::Ellipsis)
            .accessibility_label("Repository tools")
            .tooltip("Repository tools")
            .disabled(self.project_dialog_open || self.git.switching_path.is_some())
            .dropdown_menu({
                let weak = cx.entity().downgrade();
                let server = self.server.clone();
                let repo = self.git.repo["repoId"].clone();
                move |mut menu, _, _| {
                    for (tab, title, icon) in [
                        (3, "Stashes", IconName::Archive),
                        (4, "Tags", IconName::Tag),
                        (6, "Remotes", IconName::Globe),
                    ] {
                        let weak = weak.clone();
                        let server = server.clone();
                        let repo = repo.clone();
                        menu = menu.item(PopupMenuItem::new(title).icon(icon).on_click(
                            move |_, _, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    if this.page == Page::Projects
                                        && this.server == server
                                        && this.git.repo["repoId"] == repo
                                        && !this.project_dialog_open
                                    {
                                        this.git_navigate(tab, cx);
                                    }
                                });
                            },
                        ));
                    }
                    let weak = weak.clone();
                    let server = server.clone();
                    let repo = repo.clone();
                    menu.separator().item(
                        PopupMenuItem::new("Open in Files")
                            .icon(IconName::FolderOpen)
                            .on_click(move |_, window, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    if this.server == server
                                        && this.git.repo["repoId"] == repo
                                        && !this.project_dialog_open
                                    {
                                        this.git_open_files(window, cx);
                                    }
                                });
                            }),
                    )
                }
            })
            .into_any_element()
    }
    pub(super) fn git_workspace_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let tab = self.git.tab;
        let Some((_, _)) = tab_info(tab) else {
            return div().into_any_element();
        };
        let compact = self.project_review_compact(cx);
        let entries = self.git_visible_entries();
        let scroll_key = git_page_demand::identity(&self.git.page);
        let empty = self.git.page["entries"]
            .as_array()
            .is_some_and(|entries| entries.is_empty())
            && !self
                .resources
                .get("git-list")
                .is_some_and(|r| r.pending || r.error.is_some())
            && self.git.error.is_none();
        let empty_message = match tab {
            0 => "No changes.",
            1 => "No commits yet.",
            2 if self
                .git
                .branch_search
                .as_ref()
                .is_some_and(|search| !search.query.is_empty() || search.kind != 0) =>
            {
                "No branches match."
            }
            2 => "No branches yet.",
            3 => "No stashes yet.",
            4 => "No tags yet.",
            6 if !self.git.listing.remote_query().is_empty() => "No remotes match.",
            _ => "No remotes configured.",
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .when(tab == 3, |column| {
                column
                    .child(self.status("git-stash-status", cx))
                    .child(self.git_stash_toolbar(cx))
            })
            .when(tab == 4, |column| {
                column
                    .child(self.status("git-tag-status", cx))
                    .child(self.git_tag_toolbar(cx))
            })
            .when(tab == 6, |column| {
                column
                    .child(self.git_remote_toolbar(cx))
                    .when(compact, |column| {
                        column.child(
                            div()
                                .px_2()
                                .pb_2()
                                .child(self.repository_settings_button(cx)),
                        )
                    })
            })
            .when(matches!(tab, 3 | 4 | 6), |column| {
                column.child(self.git_listing_filter(cx))
            })
            .child(self.status("git-list", cx))
            .child(self.status("git-more", cx))
            .when(empty && tab != 0, |column| {
                column.child(empty::compact(empty_message, cx))
            })
            .when(tab == 0 && self.git_is_clean(), |column| {
                column.child(empty::compact("No changes.", cx).text_center().flex_1())
            })
            .when(tab == 0 && !self.git_is_clean(), |column| {
                column.child(self.git_changes_list(cx))
            })
            .when(tab != 0, |column| {
                column.child(
                    uniform_list(
                        "git-entries",
                        entries.len(),
                        cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                            range
                                .map(|i| {
                                    let e = &entries[i];
                                    let (title, subtitle) = match tab {
                                        0 => (
                                            text(&e["path"], "display"),
                                            if e["conflicted"] == true {
                                                "Conflicted".into()
                                            } else if e["staged"] == true && e["unstaged"] == true {
                                                "Staged and unstaged".into()
                                            } else if e["staged"] == true {
                                                "Staged".into()
                                            } else if e["untracked"] == true {
                                                "Untracked".into()
                                            } else {
                                                "Modified".into()
                                            },
                                        ),
                                        1 => (
                                            git_commit_header::history_subject(e),
                                            git_commit_header::history_subtitle(e),
                                        ),
                                        2 => (
                                            text(&e["name"], "display"),
                                            [
                                                if e["current"] == true {
                                                    "Current branch".into()
                                                } else {
                                                    String::new()
                                                },
                                                e["upstream"]["display"]
                                                    .as_str()
                                                    .map(|name| {
                                                        format!(
                                                            "Tracks {}",
                                                            name.strip_prefix("refs/remotes/")
                                                                .or_else(|| name
                                                                    .strip_prefix("refs/heads/"))
                                                                .unwrap_or(name)
                                                        )
                                                    })
                                                    .unwrap_or_default(),
                                            ]
                                            .into_iter()
                                            .filter(|s| !s.is_empty())
                                            .collect::<Vec<_>>()
                                            .join(" · "),
                                        ),
                                        3 => (
                                            format!(
                                                "{}{}",
                                                if text(e, "message").is_empty() {
                                                    "Unnamed stash".to_owned()
                                                } else {
                                                    text(e, "message")
                                                },
                                                if e["messageTruncated"] == true {
                                                    "…"
                                                } else {
                                                    ""
                                                }
                                            ),
                                            format!(
                                                "stash@{{{}}} · {}",
                                                text(e, "index"),
                                                git_commit_header::relative_time(
                                                    &e["time"],
                                                    chrono::Utc::now().timestamp()
                                                )
                                            ),
                                        ),
                                        4 => (
                                            text(&e["name"], "display"),
                                            format!(
                                                "{} · {}",
                                                if e["annotated"] == true {
                                                    "Annotated tag"
                                                } else {
                                                    "Lightweight tag"
                                                },
                                                text(&e["oid"], "hex")
                                                    .chars()
                                                    .take(7)
                                                    .collect::<String>()
                                            ),
                                        ),
                                        _ => (text(e, "name"), String::new()),
                                    };
                                    let entry = e.clone();
                                    div()
                                        .h(px(git_listing::row_height(tab)))
                                        .w_full()
                                        .min_w_0()
                                        .px(px(6.))
                                        .flex()
                                        .items_center()
                                        .child(
                                            git_file_row::row_focus(
                                                Button::new(("git-entry", i)),
                                                cx,
                                            )
                                            .accessibility_label(if subtitle.is_empty() {
                                                title.clone()
                                            } else {
                                                format!("{title} · {subtitle}")
                                            })
                                            .custom(git_file_row::selection_style(
                                                this.git.selected == *e,
                                                cx,
                                            ))
                                            .selected(this.git.selected == *e)
                                            .w_full()
                                            .h_full()
                                            .min_w_0()
                                            .toggled(this.git.selected == *e)
                                            .when(matches!(tab, 3 | 4), |button| {
                                                button.child(
                                                    Icon::new(if tab == 3 {
                                                        gpui_kit::assets::IconName::Archive
                                                    } else {
                                                        gpui_kit::assets::IconName::Tag
                                                    })
                                                    .size(px(14.))
                                                    .text_color(cx.theme().muted_foreground),
                                                )
                                            })
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .flex()
                                                    .when(tab != 0, |row| {
                                                        row.flex_col().items_start()
                                                    })
                                                    .when(tab == 0, |row| row.items_center())
                                                    .gap(px(if tab == 1 {
                                                        2.
                                                    } else if matches!(tab, 3 | 4) {
                                                        3.
                                                    } else {
                                                        6.
                                                    }))
                                                    .when(tab == 0, |row| {
                                                        row.child(
                                                            div()
                                                                .text_size(px(crate::ui::FONT_SIZE))
                                                                .text_color(
                                                                    cx.theme().muted_foreground,
                                                                )
                                                                .w(px(16.))
                                                                .flex_shrink_0()
                                                                .child(change_mark(e)),
                                                        )
                                                    })
                                                    .child(
                                                        div()
                                                            .w_full()
                                                            .min_w_0()
                                                            .truncate()
                                                            .when(
                                                                matches!(tab, 1 | 3 | 4),
                                                                |text| {
                                                                    text.text_size(px(
                                                                        crate::ui::FONT_SIZE,
                                                                    ))
                                                                },
                                                            )
                                                            .when(tab == 1, |text| {
                                                                text.line_height(px(17.))
                                                            })
                                                            .font_weight(FontWeight::MEDIUM)
                                                            .child(title),
                                                    )
                                                    .when(tab == 1, |row| {
                                                        row.child(
                                                            git_commit_header::history_row_identity(
                                                                e, cx,
                                                            ),
                                                        )
                                                    })
                                                    .when(
                                                        tab != 0 && tab != 1 && tab != 6,
                                                        |row| {
                                                            row.child(
                                                                crate::label(subtitle)
                                                                    .text_size(px(
                                                                        crate::ui::FONT_SIZE,
                                                                    ))
                                                                    .w_full()
                                                                    .truncate(),
                                                            )
                                                        },
                                                    ),
                                            )
                                            .on_click(
                                                cx.listener(move |this, _, _, cx| {
                                                    if this.git.tab != tab
                                                        || !this
                                                            .git_visible_entries()
                                                            .contains(&entry)
                                                    {
                                                        return;
                                                    }
                                                    this.project_show_review(cx);
                                                    this.git.selected = entry.clone();
                                                    this.git.detail_open = true;
                                                    this.git.stash_apply = None;
                                                    if tab == 3 {
                                                        this.git.stash_save = None;
                                                    }
                                                    if matches!(tab, 1 | 4) {
                                                        this.git.tag_form = None;
                                                    }
                                                    this.git.tag_delete = None;
                                                    this.git.tag_push = None;
                                                    this.git.worktree_lock = None;
                                                    this.git.worktree_repair = None;
                                                    this.git.worktree_remove = None;
                                                    if tab == 6 {
                                                        this.git_select_remote();
                                                    }
                                                    if tab == 2 {
                                                        this.git_branch_worktree_owner();
                                                    }
                                                    if tab == 0 {
                                                        this.git_select_side(
                                                            entry["staged"] == true,
                                                        );
                                                    }
                                                    if tab == 1 || tab == 3 || tab == 4 {
                                                        this.git_history_select();
                                                    }
                                                    cx.notify();
                                                }),
                                            ),
                                        )
                                })
                                .collect()
                        }),
                    )
                    .on_scroll_wheel(cx.listener(move |this, event: &ScrollWheelEvent, _, _| {
                        if event.delta.pixel_delta(px(20.)).y < px(0.) {
                            this.git.listing.demand.arm(scroll_key.clone());
                        }
                    }))
                    .track_scroll(&self.git.scroll)
                    .flex_1()
                    .min_w_0(),
                )
            })
            .when(tab == 0 && !self.git_is_clean(), |column| {
                column.child(self.git_commit_composer(cx))
            })
            .when(compact, |column| column.child(self.git_resume_detail(cx)))
            .into_any_element()
    }
    pub(super) fn git_screen(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.git.remote_refs.is_some() {
            return self.remote_refs_view(cx);
        }
        if !self.git.repo["repoId"].is_string() {
            return self.git_open_screen(cx);
        }
        if self.git.repo["bare"] != true && self.code_workspace().is_some() {
            return self.project_workspace_view(window, cx);
        }
        let tab = self.git.tab;
        let Some((title, _)) = tab_info(tab) else {
            return div().into_any_element();
        };
        let compact = git_layout::compact(self.content_width, tab);
        let clean_detail = compact && self.git_is_clean();
        let show_list = !compact || (!self.git.detail_open && !clean_detail);
        let show_detail = !compact || self.git.detail_open || clean_detail;
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(git_layout::toolbar(
                [
                    self.git_project_picker(cx),
                    self.checkout_picker_trigger(cx),
                    self.git_branch_picker_trigger(cx),
                ],
                [
                    self.git_sync_control(cx),
                    crate::controls::icon_button("git-refresh-status")
                        .ghost()
                        .icon(IconName::RefreshCw)
                        .accessibility_label("Refresh repository")
                        .tooltip("Refresh")
                        .disabled(
                            self.git.switching_path.is_some()
                                || ["git-list", "git-refresh"]
                                    .iter()
                                    .any(|key| self.resources.get(key).is_some_and(|r| r.pending)),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.git_refresh();
                            this.reshow_git_failure(cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                    self.git_tools_menu(cx),
                ],
                self.content_width,
                cx,
            ))
            .when(tab > 1, |view| {
                view.child(
                    crate::ui::header()
                        .px(px(12.))
                        .gap(px(8.))
                        .child(
                            crate::controls::compact_button("git-back-to-changes")
                                .icon(IconName::ArrowLeft)
                                .label(if self.git.repo["bare"] == true {
                                    "History"
                                } else {
                                    "Changes"
                                })
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.git_navigate(
                                        usize::from(this.git.repo["bare"] == true),
                                        cx,
                                    )
                                })),
                        )
                        .child(div().min_w_0().truncate().child(title))
                        .child(crate::ui::header_rule(cx)),
                )
            })
            .child(self.status("repository", cx))
            .when(matches!(tab, 0..=2), |view| {
                view.child(self.git_integration_view(cx))
            })
            .when_some(self.git.error.clone(), |s, error| {
                s.child(
                    div()
                        .p_3()
                        .child(crate::controls::alert("git-error", error, cx)),
                )
            })
            .when(tab == 1, |view| {
                view.child(self.status("git-tag-status", cx))
            })
            .when(clean_detail, |view| {
                view.child(
                    crate::ui::header()
                        .px(px(12.))
                        .child("History")
                        .child(crate::ui::header_rule(cx))
                        .into_any_element(),
                )
            })
            .when(compact && show_detail && !clean_detail, |view| {
                view.child(
                    crate::ui::header()
                        .px(px(12.))
                        .child(crate::ui::header_rule(cx))
                        .child(
                            crate::controls::compact_button("git-back-to-list")
                                .icon(IconName::ArrowLeft)
                                .label(format!("All {}", title.to_lowercase()))
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.git.detail_open = false;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .child(
                div().flex_1().min_h_0().child(
                    git_split::group(
                        if compact {
                            if show_detail {
                                "git-compact-detail"
                            } else {
                                "git-compact-list"
                            }
                        } else {
                            "git-workspace-split"
                        },
                        "Resize the file list",
                        if matches!(tab, 1 | 3 | 4) { 390. } else { 240. },
                    )
                    .when(show_list, |view| {
                        view.child(
                            resizable_panel()
                                .when(!compact && matches!(tab, 0 | 1 | 3 | 4 | 6), |panel| {
                                    panel
                                        .size(px(git_layout::LIST_WIDTH))
                                        .size_range(
                                            px(git_layout::MIN_LIST_WIDTH)
                                                ..px(git_layout::MAX_LIST_WIDTH),
                                        )
                                        .flex_none()
                                })
                                .child(self.git_workspace_sidebar(cx)),
                        )
                    })
                    .when(show_detail && tab == 0, |view| {
                        view.child(
                            resizable_panel()
                                .size_range(px(240.)..Pixels::MAX)
                                .child(self.git_diff_view(window, cx)),
                        )
                    })
                    .when(show_detail && tab == 6, |view| {
                        view.child(
                            resizable_panel()
                                .size_range(px(300.)..Pixels::MAX)
                                .child(self.git_remote_view(cx)),
                        )
                    })
                    .when(show_detail && matches!(tab, 1 | 3 | 4), |view| {
                        view.child(
                            resizable_panel()
                                .size_range(px(390.)..Pixels::MAX)
                                .child(self.git_history_view(window, cx)),
                        )
                    }),
                ),
            )
            .child(self.status("git-detail", cx))
            .child(self.git_log_panel(window, cx))
            .child(
                crate::controls::status_bar(cx)
                    .left(
                        crate::label(self.git_read_feedback())
                            .text_size(px(crate::ui::FONT_SIZE))
                            .min_w_0()
                            .truncate(),
                    )
                    .when(
                        self.git.page["nextCursor"].is_string()
                            && self
                                .resources
                                .get("git-more")
                                .is_some_and(|r| r.error.is_some()),
                        |bar| {
                            bar.right(
                                crate::controls::compact_button("git-load-more")
                                    .label("Try again")
                                    .accessibility_label("Try loading more again")
                                    .disabled(
                                        self.git.error.is_some()
                                            || self
                                                .resources
                                                .get("git-more")
                                                .is_some_and(|r| r.pending),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.resources.invalidate("git-more");
                                        this.git.continuation = None;
                                        this.git_more();
                                        cx.notify();
                                    })),
                            )
                        },
                    )
                    .right(
                        crate::controls::compact_button("git-log-toggle")
                            .ghost()
                            .icon(IconName::ScrollText)
                            .label("Log")
                            .track_focus(&self.git_log.view.trigger(cx))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.git_log.open = !this.git_log.open;
                                if this.git_log.open {
                                    this.git_log_follow_tail();
                                } else {
                                    this.git_log_close(window, cx);
                                }
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }
}
#[cfg(test)]
mod tests {
    use super::{
        Workspace, checkout_target, listing_feedback, local_branch_target, merge_page,
        worktree_path,
    };
    use newport_client::resources::Resources;
    use serde_json::json;
    #[test]
    fn worktree_open_retains_the_visible_checkout_until_success() {
        let old_path = json!({"bytesB64":"b2xk", "display":"/old"});
        let next = json!({"bytesB64":"bmV3", "display":"/new"});
        let mut view = Workspace {
            repo: json!({"repoId":"old"}),
            opening_path: old_path.clone(),
            tab: 1,
            page: json!({"snapshot":"old","entries":[{"oid":"old"}]}),
            selected: json!({"oid":"old"}),
            diff: json!({"entries":["visible diff"]}),
            detail_open: true,
            ..Default::default()
        };
        let page = view.page.clone();
        assert!(!view.begin_checkout_switch(old_path.clone()));
        assert!(view.begin_checkout_switch(next.clone()));
        assert_eq!(view.repo["repoId"], "old");
        assert_eq!(view.opening_path, old_path);
        assert_eq!(view.page, page);
        assert_eq!(view.selected["oid"], "old");
        assert_eq!(view.diff["entries"], json!(["visible diff"]));
        assert!(!view.begin_checkout_switch(json!({"bytesB64":"third"})));
        assert_eq!(view.switch_view(0, &mut Resources::default()), None);
        assert_eq!(view.switching_path, Some(next));
        // Failed opens only retire the pending destination, not the old view.
        view.switching_path.take();
        assert_eq!(view.page, page);
        assert_eq!(view.repo["repoId"], "old");
        assert_eq!(view.opening_path, old_path);
    }

    #[test]
    fn successful_worktree_switch_preserves_layout_but_discards_checkout_state() {
        for tab in [0, 1, 2, 3, 4, 6] {
            let mut view = Workspace {
                repo: json!({"repoId":"old"}),
                tab,
                detail_open: true,
                page: json!({"snapshot":"old","entries":[{}]}),
                selected: json!({"entryId":"old-file"}),
                diff: json!({"entries":["old diff"]}),
                error: Some("old error".into()),
                ..Default::default()
            };
            view.saved_views.insert(
                99,
                super::SavedView {
                    workspace: Workspace::default(),
                    resources: Resources::default().take_prefix_except("", &[]),
                },
            );
            let path = json!({"bytesB64":"bmV3", "display":"/new"});
            assert!(view.begin_checkout_switch(path.clone()));
            view.replace_checkout(path.clone());
            assert_eq!(view.tab, tab);
            assert!(view.detail_open);
            assert_eq!(view.opening_path, path);
            assert!(view.repo.is_null());
            assert!(view.page.is_null());
            assert!(view.selected.is_null());
            assert!(view.diff.is_null());
            assert!(view.error.is_none());
            assert!(view.switching_path.is_none());
            assert!(view.saved_views.is_empty());
        }
    }

    #[test]
    fn prepared_checkout_installs_its_first_page_without_an_empty_intermediate_view() {
        for tab in [0, 1, 2, 3, 4, 6] {
            let mut view = Workspace {
                repo: json!({"repoId":"old"}),
                tab,
                detail_open: true,
                page: json!({"snapshot":"old", "entries":[{"entryId":"old-file"}]}),
                selected: json!({"entryId":"old-file"}),
                diff: json!({"entries":["old diff"]}),
                ..Default::default()
            };
            let path = json!({"bytesB64":"bmV3", "display":"/new"});
            assert!(view.begin_checkout_switch(path.clone()));
            // repo.open alone must not clear or relabel the current contents.
            let repo = json!({"repoId":"new", "head":{"name":"topic"}});
            view.switching_repo = repo.clone();
            assert_eq!(view.repo["repoId"], "old");
            assert_eq!(view.page["entries"][0]["entryId"], "old-file");
            let page =
                json!({"snapshot":"new", "entries":[{"entryId":"new-file"}], "nextCursor":"next"});
            view.install_checkout(path.clone(), repo.clone(), page.clone());
            assert_eq!(view.repo, repo);
            assert_eq!(view.page, page);
            assert_eq!(view.opening_path, path);
            assert_eq!(view.tab, tab);
            assert!(view.detail_open);
            assert!(view.selected.is_null());
            assert!(view.diff.is_null());
            assert!(view.switching_path.is_none());
            assert!(view.switching_repo.is_null());
        }
    }

    #[test]
    fn prepared_checkout_keeps_empty_results_and_bare_repository_history() {
        let mut view = Workspace::default();
        let page = json!({"snapshot":"empty", "entries":[]});
        view.install_checkout(
            json!({"bytesB64":"bmV3"}),
            json!({"repoId":"bare","bare":true}),
            page.clone(),
        );
        assert_eq!(view.tab, 1);
        assert_eq!(view.page, page);
        assert_eq!(view.read_params()["messageBytes"], 512);
        assert_eq!(view.read_params()["repoId"], "bare");
    }

    #[test]
    fn removed_worktree_page_cannot_replace_the_active_workspace() {
        let mut view = Workspace {
            repo: json!({"repoId":"repo"}),
            page: json!({"snapshot":"changes","entries":[]}),
            ..Default::default()
        };
        let mut resources = Resources::default();
        assert!(super::tab_info(5).is_none());
        assert_eq!(view.switch_view(5, &mut resources), None);
        assert_eq!(view.tab, 0);
        assert_eq!(view.page["snapshot"], "changes");
        assert!(view.saved_views.is_empty());
        assert_eq!(view.switch_view(6, &mut resources), Some(true));
        assert_eq!(
            super::tab_info(view.tab),
            Some(("Remotes", "repo.remote_names"))
        );
    }
    #[test]
    fn bare_repository_cannot_navigate_to_changes() {
        let mut view = Workspace {
            repo: json!({"repoId":"bare", "bare":true}),
            tab: 1,
            ..Default::default()
        };
        let mut resources = Resources::default();
        assert_eq!(view.switch_view(0, &mut resources), None);
        assert_eq!(view.tab, 1);
        assert_eq!(view.switch_view(4, &mut resources), Some(true));
    }
    #[test]
    fn successful_read_age_uses_the_original_read_timestamp() {
        use std::time::Duration;
        assert_eq!(super::read_feedback(None), "Not read yet");
        for (seconds, expected) in [
            (0, "Read just now"),
            (44, "Read just now"),
            (45, "Read 1 minute ago"),
            (120, "Read 2 minutes ago"),
            (3600, "Read 1 hour ago"),
            (7200, "Read 2 hours ago"),
        ] {
            assert_eq!(
                super::read_feedback(Some(Duration::from_secs(seconds))),
                expected
            );
        }
    }
    #[test]
    fn changed_status_snapshot_cannot_reuse_a_displayed_comparison() {
        let mut view = Workspace {
            page: json!({"snapshot":"old"}),
            selected: json!({"entryId":"same-path"}),
            diff: json!({"entries":["old contents"]}),
            ..Default::default()
        };
        assert!(view.accept_listing(json!({"snapshot":"new","entries":[{"entryId":"same-path"}]})));
        assert!(view.selected.is_null());
        assert!(view.diff.is_null());
        assert!(!view.accept_listing(json!({"snapshot":"new","entries":[]})));
    }
    #[test]
    fn background_status_keeps_loaded_pages_until_the_snapshot_changes() {
        let entry = json!({"entryId":"old","path":{"bytesB64":"YQ=="},"unstaged":true});
        let page = json!({"snapshot":"old","entries":[{"entryId":"first"},entry],"nextCursor":"after-second-page"});
        let mut view = Workspace {
            repo: json!({"repoId":"repo"}),
            page: page.clone(),
            selected: entry.clone(),
            diff: json!({"entries":["reviewed lines"]}),
            ..Default::default()
        };
        assert!(!view.accept_listing(json!({"snapshot":"old","entries":[{"entryId":"first"}],"nextCursor":"after-first-page"})));
        assert_eq!(view.page, page);
        assert_eq!(view.selected, entry);
        assert_eq!(view.diff["entries"], json!(["reviewed lines"]));

        let mut current = entry.clone();
        current["entryId"] = json!("current");
        assert!(
            view.accept_listing(json!({"snapshot":"new","entries":[current],"nextCursor":null}))
        );
        assert_eq!(view.selected, current);
        assert!(view.diff.is_null());
        assert!(view.page["nextCursor"].is_null());
        assert!(view.accept_listing(json!({"snapshot":"clean","entries":[]})));
        assert!(view.selected.is_null());
    }
    #[test]
    fn observed_external_changes_retire_suspended_changes_but_keep_current_history() {
        let mut view = Workspace {
            repo: json!({"repoId":"repo"}),
            page: json!({"snapshot":"before","entries":[]}),
            ..Default::default()
        };
        let mut resources = Resources::default();
        view.switch_view(1, &mut resources);
        view.selected = json!({"oid":{"hex":"inspected"}});
        assert!(!view.observe_status_snapshot(&json!({"snapshot":"before"})));
        assert!(view.observe_status_snapshot(&json!({"snapshot":"after"})));
        assert_eq!(view.selected["oid"]["hex"], "inspected");
        assert_eq!(view.switch_view(0, &mut resources), Some(true));
        assert!(view.page.is_null());
    }
    #[test]
    fn index_refresh_follows_the_selected_file_between_groups() {
        let entry = json!({"entryId":"old","path":{"bytesB64":"YQ=="},"untracked":true});
        let mut view = Workspace {
            page: json!({"snapshot":"before","entries":[entry]}),
            selected: entry,
            diff: json!({"snapshot":"old-diff"}),
            ..Default::default()
        };
        let staged = json!({"entryId":"new","path":{"bytesB64":"YQ=="},"staged":true});
        assert!(
            view.accept_listing_with_scope(json!({"snapshot":"staged","entries":[staged]}), true)
        );
        assert_eq!(view.selected, staged);
        assert!(view.diff_staged);
        assert!(view.diff.is_null());
        let unstaged = json!({"entryId":"unstaged","path":{"bytesB64":"YQ=="},"unstaged":true});
        assert!(
            view.accept_listing_with_scope(
                json!({"snapshot":"unstaged","entries":[unstaged]}),
                true
            )
        );
        assert_eq!(view.selected, unstaged);
        assert!(!view.diff_staged);
    }
    #[test]
    fn index_refresh_keeps_partial_or_unrelated_selection_but_retires_old_authority() {
        let entry = json!({"entryId":"old","path":{"bytesB64":"YQ=="},"unstaged":true});
        let old_diff = json!({"snapshot":"old-diff","metadata":{"sourceSnapshot":"before"}});
        let mut view = Workspace {
            page: json!({"snapshot":"before","entries":[entry]}),
            selected: entry,
            diff: old_diff.clone(),
            ..Default::default()
        };
        let partial =
            json!({"entryId":"new","path":{"bytesB64":"YQ=="},"staged":true,"unstaged":true});
        assert!(
            view.accept_listing_with_scope(json!({"snapshot":"after","entries":[partial]}), true)
        );
        assert_eq!(view.selected, partial);
        assert!(!view.diff_staged);
        assert_eq!(view.diff, old_diff);
        assert_ne!(
            view.diff["metadata"]["sourceSnapshot"],
            view.page["snapshot"]
        );
        // A filtered-out or removed file must not select a different sibling.
        assert!(view.accept_listing_with_scope(json!({"snapshot":"filtered","entries":[{"entryId":"sibling","path":{"bytesB64":"Yg=="},"staged":true}]}), true));
        assert!(view.selected.is_null());
        assert!(view.diff.is_null());
    }
    #[test]
    fn diff_recovery_rebinds_to_new_status_and_is_cancelled_when_leaving_changes() {
        use newport_client::git_read_recovery::Decision;
        let entry = json!({"entryId":"old","path":{"bytesB64":"YQ=="},"unstaged":true});
        let mut view = Workspace {
            repo: json!({"repoId":"repo"}),
            page: json!({"snapshot":"old"}),
            selected: entry.clone(),
            ..Default::default()
        };
        assert_eq!(
            view.diff_recovery
                .begin(Some("STALE_SNAPSHOT"), &entry, false),
            Decision::Refresh
        );
        // Stale-error recovery retires the old comparison before rereading;
        // an ordinary background refresh instead retains the selected path.
        view.selected = serde_json::Value::Null;
        view.page = serde_json::Value::Null;
        let mut next = entry.clone();
        next["entryId"] = json!("current");
        view.accept_listing(json!({"snapshot":"current","entries":[next]}));
        assert!(view.selected.is_null());
        assert_eq!(view.diff_recovery.resolve(&view.page), Some((next, false)));

        assert_eq!(
            view.diff_recovery.begin(Some("STALE_ENTRY"), &entry, false),
            Decision::Refresh
        );
        let mut resources = Resources::default();
        let status = resources.begin("git-list").unwrap();
        view.switch_view(1, &mut resources);
        assert!(!resources.complete_detailed("git-list", status, Ok(json!({"entries":[entry]}))));
        view.switch_view(0, &mut resources);
        assert!(!view.diff_recovery.pending());
    }
    #[test]
    fn navigation_preserves_completed_selection_and_does_not_reload_current_view() {
        let mut view = Workspace {
            repo: json!({"repoId":"repo","head":{"name":"old"}}),
            server: "server".into(),
            ..Default::default()
        };
        let selected = json!({"entryId":"file"});
        let page = json!({"entries":[selected],"snapshot":"status"});
        view.page = page.clone();
        view.selected = selected.clone();
        let mut resources = Resources::default();
        let read = resources.begin("git-list").unwrap();
        resources.complete("git-list", read, Ok(page.clone()));
        let log = resources.begin("git-log-events").unwrap();
        assert_eq!(view.switch_view(0, &mut resources), None);
        assert_eq!(view.selected, selected);
        assert_eq!(view.switch_view(1, &mut resources), Some(true));
        assert!(resources.get("git-list").is_none());
        assert!(resources.complete("git-log-events", log, Ok(json!({"cursor":4}))));
        let history = resources.begin("git-list").unwrap();
        resources.complete("git-list", history, Ok(json!({"entries":[]})));
        view.repo["head"] = json!({"name":"fresh"});
        assert_eq!(view.switch_view(0, &mut resources), Some(false));
        assert_eq!(view.page, page);
        assert_eq!(view.selected, selected);
        assert_eq!(resources.get("git-list").unwrap().value, page);
        assert_eq!(view.repo["head"]["name"], "fresh");
        assert_eq!(view.saved_views.len(), 1);
    }
    #[test]
    fn checkout_path_survives_new_cached_and_invalidated_tabs() {
        // The bookmark may be a subdirectory or symlink: repo.root cannot
        // replace the original requested path used by persistence/recovery.
        let path = json!({"display":"/linked/src", "bytesB64":"L2xpbmtlZC9zcmM="});
        let mut view = Workspace {
            repo: json!({"repoId":"repo", "root":{"display":"/linked"}}),
            opening_path: path.clone(),
            ..Default::default()
        };
        let mut resources = Resources::default();
        for tab in [1, 4, 0, 3, 1] {
            assert!(view.switch_view(tab, &mut resources).is_some());
            assert_eq!(view.opening_path, path);
        }
        view.invalidate_saved_views();
        view.switch_view(0, &mut resources);
        assert_eq!(view.opening_path, path);
    }
    #[test]
    fn checkout_summary_and_pending_read_survive_tabs_without_restoring_stale_metadata() {
        let mut view = Workspace {
            repo: json!({"repoId":"repo"}),
            ..Default::default()
        };
        view.checkout_summary
            .accept(&json!({"metadata":{"totalEntries":2}}));
        let mut resources = Resources::default();
        let read = resources
            .begin(crate::git_checkout_picker::SUMMARY)
            .unwrap();
        view.switch_view(1, &mut resources);
        let fresh = json!({"metadata":{"totalEntries":3}});
        assert!(resources.complete(crate::git_checkout_picker::SUMMARY, read, Ok(fresh.clone())));
        view.checkout_summary.accept(&fresh);
        let expected = view.checkout_summary.clone();
        for tab in [0, 4, 1] {
            view.switch_view(tab, &mut resources);
            assert_eq!(view.checkout_summary, expected);
        }
        view.invalidate_saved_views();
        view.switch_view(0, &mut resources);
        assert_eq!(view.checkout_summary, expected);
        // Navigating to a different checkout starts a fresh workspace.
        assert_ne!(Workspace::default().checkout_summary, expected);
    }
    #[test]
    fn returning_to_an_inflight_view_requires_reload_and_rejects_late_replies() {
        let mut view = Workspace {
            repo: json!({"repoId":"repo"}),
            ..Default::default()
        };
        let mut resources = Resources::default();
        let abandoned = resources.begin("git-list").unwrap();
        assert_eq!(view.switch_view(4, &mut resources), Some(true));
        let tags = resources.begin("git-list").unwrap();
        assert!(!resources.complete("git-list", abandoned, Ok(json!({"entries":["wrong"]}))));
        resources.complete("git-list", tags, Ok(json!({"entries":[]})));
        assert_eq!(view.switch_view(0, &mut resources), Some(true));
        assert!(resources.get("git-list").is_none());
        assert!(!resources.complete("git-list", abandoned, Ok(json!({"entries":["wrong"]}))));
    }
    #[test]
    fn cached_view_cannot_replay_an_old_scroll_gesture() {
        let mut view = Workspace {
            repo: json!({"repoId":"repo"}),
            page: json!({"entries":["tag"],"snapshot":"s","nextCursor":"next"}),
            tab: 4,
            ..Default::default()
        };
        view.listing
            .demand
            .arm(crate::git_page_demand::identity(&view.page));
        let mut resources = Resources::default();
        view.switch_view(3, &mut resources);
        view.switch_view(4, &mut resources);
        assert!(!view.listing.demand.take(&view.page, true, true));
        view.listing
            .demand
            .arm(crate::git_page_demand::identity(&view.page));
        assert!(view.listing.demand.take(&view.page, true, true));
        assert!(!view.listing.demand.take(&view.page, true, true));
    }
    #[test]
    fn write_outcomes_invalidate_suspended_reviews_and_snapshots() {
        let mut view = Workspace {
            repo: json!({"repoId":"repo"}),
            selected: json!({"entryId":"previous"}),
            ..Default::default()
        };
        let mut resources = Resources::default();
        let read = resources.begin("git-list").unwrap();
        resources.complete("git-list", read, Ok(json!({"snapshot":"before"})));
        view.switch_view(1, &mut resources);
        view.invalidate_saved_views();
        assert_eq!(view.switch_view(0, &mut resources), Some(true));
        assert!(view.selected.is_null());
        assert!(resources.get("git-list").is_none());
    }
    #[test]
    fn changes_refresh_updates_head_without_inventing_completion() {
        let mut view = Workspace {
            repo: json!({"repoId":"repo","head":{"detached":false,"name":{"display":"refs/heads/old"}}}),
            ..Default::default()
        };
        let head = json!({"detached":true,"oid":{"hex":"1234567890abcdef"}});
        view.accept_listing(json!({"entries":[],"metadata":{"head":head}}));
        assert_eq!(view.repo["head"], head);
        assert_eq!(
            super::head_label(&view.repo["head"]),
            "Detached HEAD · 12345678"
        );
        assert_eq!(listing_feedback(&serde_json::Value::Null, false, false), "");
        assert!(listing_feedback(&view.page, true, false).starts_with("Loading"));
        assert!(listing_feedback(&view.page, false, true).starts_with("Couldn’t"));
        assert!(
            listing_feedback(
                &json!({"entries":[],"metadata":{"truncated":true}}),
                false,
                false
            )
            .contains("incomplete")
        );
    }
    #[test]
    fn worktree_navigation_preserves_bytes_and_rejects_unavailable_rows() {
        let entry = json!({"state":"available","current":false,"kind":"linked","path":{"display":"/�","bytesB64":"L/8="}});
        assert_eq!(worktree_path(&entry), Some(entry["path"].clone()));
        for (field, value) in [
            ("state", json!("missing")),
            ("current", json!(true)),
            ("kind", json!("bare")),
        ] {
            let mut invalid = entry.clone();
            invalid[field] = value;
            assert!(worktree_path(&invalid).is_none());
        }
        let mut invalid = entry;
        invalid["path"]["bytesB64"] = json!("not-base64");
        assert!(worktree_path(&invalid).is_none());
    }
    #[test]
    fn checkout_rejects_remote_current_and_lossy_names() {
        let branch = json!({"name":{"display":"topic","bytesB64":"dG9waWM="},"oid":{"hex":"abcd"},"remote":false,"current":false});
        assert_eq!(
            checkout_target(&branch),
            Some(("topic".into(), "abcd".into()))
        );
        for field in ["remote", "current"] {
            let mut invalid = branch.clone();
            invalid[field] = json!(true);
            assert!(checkout_target(&invalid).is_none());
            if field == "current" {
                assert!(local_branch_target(&invalid).is_some());
            } else {
                assert!(local_branch_target(&invalid).is_none());
            }
        }
        let mut invalid = branch;
        invalid["name"]["bytesB64"] = json!("/w==");
        assert!(checkout_target(&invalid).is_none());
    }
    #[test]
    fn pagination_requires_same_snapshot_and_metadata() {
        let previous = json!({"snapshot":"a","nextCursor":"next","entries":[1],"metadata":{}});
        let page = json!({"snapshot":"a","nextCursor":null,"entries":[2],"metadata":{}});
        assert_eq!(
            merge_page(&previous, page.clone(), "next").unwrap()["entries"],
            json!([1, 2])
        );
        let mut stale = page;
        stale["snapshot"] = json!("b");
        assert!(merge_page(&previous, stale, "next").is_err());
        assert!(merge_page(&previous, previous.clone(), "next").is_err());
    }
}
