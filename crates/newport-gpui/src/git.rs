//! The native Git workspace consumes the same bounded, snapshot-based pages.
use super::*;
use base64::Engine as _;
use git_listing::{Entry, Listing, StatusPage};
use gpui_kit::base::Selectable;
use gpui_kit::component::resizable::resizable_panel;
use newport_client::resources::{ResourceGroup, Resources};
use newport_protocol::git::{
    Branch, Head, Path as GitPath, Repository, Request as GitRequest, Side, StatusEntry,
};
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
    pub(super) opening_path: Option<GitPath>,
    // The displayed checkout remains authoritative until repo.open succeeds.
    pub(super) switching_path: Option<GitPath>,
    switching_repo: Option<Repository>,
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
    pub(super) repo: Option<Repository>,
    pub(super) restore_document: bool,
    pub(super) tab: usize,
    pub(super) history: git_history::History,
    pub(super) page: Option<Listing>,
    pub selected: Option<Entry>,
    pub(super) detail_open: bool,
    pub diff_staged: bool,
    pub(super) diff_context: git_diff::ContextLines,
    pub(super) diff_presentation: git_diff::Cache,
    pub(super) diff_text: git_diff_text::Cache,
    pub(super) diff_find: Option<git_diff_search::Controls>,
    pub(super) diff: Option<newport_client::git_diff_page::DiffPage>,
    pub(super) diff_demand: git_diff::ScrollDemand,
    pub(super) diff_recovery: newport_client::git_read_recovery::Recovery,
    pub(super) index_refresh: Option<git_index_refresh::Refresh>,
    diff_cursor: Option<String>,
    pub diff_scroll: UniformListScrollHandle,
    pub(super) continuation: Option<String>,
    /// The repository each repository refresh read reported, by request key.
    pub(super) refreshed: std::collections::HashMap<String, String>,
    pub scroll: UniformListScrollHandle,
    pub error: Option<String>,
}
impl Workspace {
    /// An open repository with no switch under way: it can be set aside
    /// whole while another page shows.
    pub(super) fn settled(&self) -> bool {
        self.repo.is_some() && self.switching_path.is_none() && self.switching_repo.is_none()
    }
    /// Releases the repositories it holds open on the server.
    pub(super) fn release(&mut self) {
        if let Some(pending) = self.switching_repo.take()
            && Some(pending.repo_id.as_str()) != self.repo_id()
        {
            release_repository(self.server.clone(), pending.repo_id);
        }
        if let Some(repo) = self.repo.take() {
            release_repository(self.server.clone(), repo.repo_id);
        }
    }
    /// The open repository's ID.
    pub(super) fn repo_id(&self) -> Option<&str> {
        self.repo.as_ref().map(|repo| repo.repo_id.as_str())
    }
    /// Whether the open repository accepts writes.
    pub(super) fn writable(&self) -> bool {
        self.repo
            .as_ref()
            .is_some_and(|repo| !repo.capabilities.read_only)
    }
    /// Whether the open repository has a working tree.
    pub(super) fn has_working_tree(&self) -> bool {
        self.repo
            .as_ref()
            .is_some_and(|repo| repo.capabilities.working_tree)
    }
    /// Whether the open repository is bare, once one is open.
    pub(super) fn bare(&self) -> Option<bool> {
        self.repo.as_ref().map(|repo| repo.bare)
    }
    /// The open repository's checkout root.
    pub(super) fn root(&self) -> Option<&GitPath> {
        self.repo.as_ref().map(|repo| &repo.root)
    }
    /// The open repository's root as text, when its bytes are UTF-8.
    pub(super) fn root_path(&self) -> Option<String> {
        project_editor::root_path(self.root()?)
    }
    /// Shows a newer HEAD reported by a status read. Returns whether it
    /// differed from the one shown.
    pub(super) fn observe_head(&mut self, head: &Head) -> bool {
        if let Some(repo) = self.repo.as_mut()
            && repo.head != *head
        {
            repo.head = head.clone();
            return true;
        }
        false
    }
    /// The status page the Changes tab shows.
    pub(super) fn status_page(&self) -> Option<&StatusPage> {
        self.page.as_ref().and_then(Listing::status)
    }
    /// The snapshot of the page shown, or nothing before one loads.
    pub(super) fn snapshot(&self) -> &str {
        self.page
            .as_ref()
            .map(Listing::snapshot)
            .unwrap_or_default()
    }
    /// The selected changed file.
    pub(super) fn entry(&self) -> Option<&StatusEntry> {
        match &self.selected {
            Some(Entry::Change(entry)) => Some(entry),
            _ => None,
        }
    }
    /// The selected remote's name.
    pub(super) fn remote_name(&self) -> Option<&str> {
        match &self.selected {
            Some(Entry::Remote(remote)) => Some(&remote.name),
            _ => None,
        }
    }
}
/// Closes a repository handle on the server, off the UI thread.
pub(super) fn release_repository(server: String, repo_id: String) {
    std::thread::spawn(move || backend::Backend::discover().release_repository(&server, &repo_id));
}
struct SavedView {
    workspace: Workspace,
    resources: ResourceGroup,
}
impl Workspace {
    fn read_request(&self, cursor: Option<String>) -> Option<GitRequest> {
        let repo_id = self.repo_id()?.to_owned();
        Some(match self.tab {
            0 => GitRequest::Status {
                repo_id,
                filter: Some(self.changes.filter()),
                page_size: 100,
                cursor,
            },
            1 => GitRequest::History {
                repo_id,
                page_size: 100,
                cursor,
                revision: "HEAD".into(),
                message_bytes: Some(512),
            },
            2 => GitRequest::Branches {
                repo_id,
                page_size: 100,
                cursor,
                filter: String::new(),
                branch_kind: None,
            },
            3 => GitRequest::Stashes {
                repo_id,
                page_size: 100,
                cursor,
            },
            4 => GitRequest::Tags {
                repo_id,
                page_size: 100,
                cursor,
                message_bytes: Some(512),
            },
            6 => GitRequest::RemoteNames {
                repo_id,
                page_size: 20,
                cursor,
                filter: self.listing.remote_query().into(),
            },
            _ => return None,
        })
    }

    fn begin_checkout_switch(&mut self, path: GitPath) -> bool {
        if self.switching_path.is_some()
            || self.repo.is_none()
            || Some(&path.bytes_b64) == self.opening_path.as_ref().map(|path| &path.bytes_b64)
        {
            return false;
        }
        self.switching_path = Some(path);
        true
    }

    /// Preserve presentation, never checkout-specific selections or snapshots.
    fn replace_checkout(&mut self, path: GitPath) {
        *self = Self {
            tab: self.tab,
            detail_open: self.detail_open,
            diff_context: self.diff_context,
            opening_path: Some(path),
            ..Default::default()
        };
    }

    fn install_checkout(&mut self, path: GitPath, repo: Repository, page: Listing) {
        let tab = if repo.bare && self.tab == 0 {
            1
        } else {
            self.tab
        };
        self.replace_checkout(path);
        self.tab = tab;
        self.repo = Some(repo);
        self.accept_listing(page);
    }

    pub(super) fn invalidate_saved_views(&mut self) {
        self.saved_views.clear();
    }
    pub(super) fn observe_status_snapshot(&mut self, status: &StatusPage) -> bool {
        let previous = if self.tab == 0 {
            self.page.as_ref()
        } else {
            self.saved_views
                .get(&0)
                .and_then(|view| view.workspace.page.as_ref())
        };
        let changed = previous.is_some_and(|previous| previous.snapshot() != status.snapshot);
        if changed {
            self.invalidate_saved_views();
        }
        changed
    }
    fn accept_listing(&mut self, page: Listing) -> bool {
        self.accept_listing_with_scope(page, false)
    }
    pub(super) fn accept_listing_with_scope(&mut self, page: Listing, index_only: bool) -> bool {
        let changed_comparison =
            self.tab == 0 && self.page.as_ref().map(Listing::snapshot) != Some(page.snapshot());
        if changed_comparison {
            let path = self.entry().map(|entry| entry.path.clone());
            let find = |staged: bool| {
                page.status().zip(path.as_ref()).and_then(|(page, path)| {
                    newport_client::git_read_recovery::matching_entry(page, path, staged)
                })
            };
            let selected = find(self.diff_staged);
            // A fully staged/unstaged file moves groups. Follow that same byte
            // path, while partial staging stays on the current comparison side.
            let other = (index_only && selected.is_none())
                .then(|| find(!self.diff_staged))
                .flatten();
            let moved = other.is_some();
            self.selected = selected.or(other).map(Entry::Change);
            if moved {
                self.diff_staged = !self.diff_staged;
            }
            // Keep the previous comparison visible during a background read.
            // Its snapshot no longer matches, so all mutation controls reject it.
            if !index_only || moved || self.selected.is_none() {
                self.diff = None;
            }
            self.diff_cursor = None;
        }
        if self.tab == 0
            && let Some(status) = page.status()
        {
            let head = status.metadata.head.clone();
            self.observe_head(&head);
        }
        // A background first-page read must not collapse the pages the user
        // already loaded, reset their cursor, or disturb an unchanged diff.
        if self.tab != 0 || changed_comparison || self.page.is_none() {
            self.page = Some(page);
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
            || (tab == 0 && self.bare() == Some(true))
            || self.repo.is_none()
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
/// The checkout path of another available, non-bare worktree, when its
/// bytes form an absolute path.
pub(super) fn worktree_destination(entry: &newport_protocol::git::Worktree) -> Option<GitPath> {
    if entry.state != "available" || entry.current || entry.kind == "bare" {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&entry.path.bytes_b64)
        .ok()?;
    (bytes.starts_with(b"/") && !bytes.contains(&0)).then(|| entry.path.clone())
}
pub(super) fn head_label(head: &Head) -> String {
    if head.detached {
        format!(
            "Detached HEAD · {}",
            head.oid
                .as_ref()
                .map(|oid| oid.hex.chars().take(8).collect::<String>())
                .unwrap_or_default()
        )
    } else {
        let name = head
            .name
            .as_ref()
            .map(|name| name.display.as_str())
            .unwrap_or_default();
        name.strip_prefix("refs/heads/").unwrap_or(name).to_owned()
    }
}
fn listing_feedback(page: Option<&Listing>, pending: bool, failed: bool) -> String {
    if pending {
        return "Loading repository…".into();
    }
    if failed {
        return "Couldn’t refresh this view".into();
    }
    let Some(page) = page else {
        return String::new();
    };
    if page.truncated() {
        return format!("{} entries shown · Listing incomplete", page.len());
    }
    if page.next_cursor().is_some() {
        return format!("{} entries shown · More available", page.len());
    }
    format!(
        "{} {}",
        page.len(),
        if page.len() == 1 { "entry" } else { "entries" }
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
fn change_mark(entry: &StatusEntry) -> &'static str {
    if entry.conflicted {
        return "!";
    }
    if entry.untracked {
        return "?";
    }
    let flags = entry.flags;
    if entry.staged {
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
fn checkout_target(branch: Option<&Branch>) -> Option<(String, String)> {
    local_branch_target(branch.filter(|branch| !branch.current)?)
}
/// The name and tip of a local branch whose name is lossless.
pub(super) fn local_branch_target(branch: &Branch) -> Option<(String, String)> {
    let name = branch.name.display.as_str();
    let oid = branch.oid.hex.as_str();
    (!branch.remote
        && !name.is_empty()
        && !oid.is_empty()
        && branch.name.bytes_b64 == base64::engine::general_purpose::STANDARD.encode(name))
    .then(|| (name.to_owned(), oid.to_owned()))
}
/// A listing row's title and the detail line beneath it.
fn row_text(entry: &Entry) -> (String, String) {
    match entry {
        Entry::Change(e) => (
            e.path.display.clone(),
            if e.conflicted {
                "Conflicted"
            } else if e.staged && e.unstaged {
                "Staged and unstaged"
            } else if e.staged {
                "Staged"
            } else if e.untracked {
                "Untracked"
            } else {
                "Modified"
            }
            .into(),
        ),
        Entry::Commit(e) => (
            git_commit_header::history_subject(e),
            git_commit_header::history_subtitle(e),
        ),
        Entry::Branch(e) => (
            e.name.display.clone(),
            [
                if e.current {
                    "Current branch".into()
                } else {
                    String::new()
                },
                e.upstream
                    .as_ref()
                    .map(|upstream| {
                        let name = upstream.display.as_str();
                        format!(
                            "Tracks {}",
                            name.strip_prefix("refs/remotes/")
                                .or_else(|| name.strip_prefix("refs/heads/"))
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
        Entry::Stash(e) => (
            format!(
                "{}{}",
                if e.message.is_empty() {
                    "Unnamed stash"
                } else {
                    &e.message
                },
                if e.message_truncated { "…" } else { "" }
            ),
            format!(
                "stash@{{{}}} · {}",
                e.index,
                git_commit_header::relative_time(e.time, chrono::Utc::now().timestamp())
            ),
        ),
        Entry::Tag(e) => (
            e.name
                .as_ref()
                .map_or_else(|| "—".into(), |name| name.display.clone()),
            format!(
                "{} · {}",
                if e.annotated {
                    "Annotated tag"
                } else {
                    "Lightweight tag"
                },
                e.oid.hex.chars().take(7).collect::<String>()
            ),
        ),
        Entry::Remote(e) => (e.name.clone(), String::new()),
    }
}
impl Newport {
    pub(super) fn git_open_worktree_path(&mut self, path: GitPath, cx: &mut Context<Self>) {
        if self.git_write_pending() || !self.git.begin_checkout_switch(path.clone()) {
            return;
        }
        self.close_checkout_picker();
        self.git_branch_picker_close();
        self.project_checks = None;
        self.resources.invalidate("repository");
        self.request_git_typed("repository", GitRequest::Open { path });
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
            && self.git.writable()
            && self.git.has_working_tree()
            && self.git.branches.error.is_none()
            && ["git-branches", "git-branch-status"].iter().all(|key| {
                self.resources
                    .get(key)
                    .is_some_and(|r| !r.pending && r.error.is_none())
            })
            && self.git_status("git-branch-status").is_some_and(|status| {
                !status.snapshot.is_empty() && status.metadata.integration.is_none()
            })
    }
    /// The snapshot of the branch picker's status read.
    pub(super) fn branch_status_snapshot(&self) -> String {
        self.git_status("git-branch-status")
            .map_or_else(|| "—".into(), |status| status.snapshot.clone())
    }
    pub(super) fn git_can_checkout(&self) -> bool {
        self.git_can_write_branch()
            && !self.git_branch_review_open()
            && checkout_target(self.git.branches.selected.as_ref()).is_some()
            && self.git_branch_worktree_unoccupied()
    }
    pub(super) fn git_new_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.git_can_write_branch() || self.git_branch_review_open() {
            return;
        }
        let Some(oid) = self
            .git_status("git-branch-status")
            .and_then(|status| status.metadata.head.oid.as_ref())
            .map(|oid| oid.hex.clone())
            .filter(|oid| !oid.is_empty())
        else {
            return;
        };
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
            snapshot: self.branch_status_snapshot(),
            oid,
            source: self
                .git_status("git-branch-status")
                .and_then(|status| status.metadata.head.name.as_ref())
                .map_or_else(|| "—".into(), |name| name.display.clone()),
            rename: false,
            _subscription: subscription,
        });
        cx.notify();
    }
    pub(super) fn git_rename_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.git_can_write_branch() || self.git_branch_review_open() {
            return;
        }
        let Some((source, oid)) = self
            .git
            .branches
            .selected
            .as_ref()
            .and_then(local_branch_target)
        else {
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
            snapshot: self.branch_status_snapshot(),
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
        let Some((branch, oid)) = checkout_target(self.git.branches.selected.as_ref()) else {
            return;
        };
        let snapshot = self.branch_status_snapshot();
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
        self.git
            .entry()
            .is_some_and(|entry| self.git_can_change_entry(stage, entry))
    }
    pub(super) fn git_can_change_entry(&self, stage: bool, entry: &StatusEntry) -> bool {
        let Some(page) = self.git.status_page() else {
            return false;
        };
        self.git.tab == 0
            && !self.git_mutation_blocks_write()
            && self.git.writable()
            && self.git.has_working_tree()
            && self.git.bare() == Some(false)
            && !page.snapshot.is_empty()
            && !entry.entry_id.is_empty()
            && self.git.error.is_none()
            && !self
                .resources
                .get("git-list")
                .is_some_and(|r| r.pending || r.error.is_some())
            && if stage {
                entry.unstaged || entry.untracked || entry.conflicted
            } else {
                entry.staged && page.metadata.integration.is_none()
            }
    }
    pub(super) fn git_change_index(&mut self, stage: bool, cx: &mut Context<Self>) {
        if !self.git_can_change_index(stage) {
            return;
        }
        let snapshot = self.git.snapshot().to_owned();
        let Some(entry) = self.git.entry().map(|entry| entry.entry_id.clone()) else {
            return;
        };
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
        self.git.repo_id().unwrap_or_default().to_owned()
    }
    pub(super) fn diff_page(&self) -> Option<&newport_client::git_diff_page::DiffPage> {
        if self.git_is_inspection() {
            return self.git.history.diff.as_ref();
        }
        self.git.diff.as_ref()
    }
    pub(super) fn diff_title(&self) -> String {
        if self.git_is_inspection() {
            return self
                .git
                .history
                .path
                .as_ref()
                .map_or_else(|| "—".into(), |path| path.display.clone());
        }
        self.git
            .entry()
            .map_or_else(|| "—".into(), |entry| entry.path.display.clone())
    }
    pub(super) fn git_release(&mut self) {
        self.git.release();
    }
    pub(super) fn git_refresh(&mut self) {
        if self.git.switching_path.is_some() || self.git.repo.is_none() {
            return;
        }
        self.git_cancel_index_refresh();
        // Local resource invalidation alone can return the backend's still-fresh
        // cache. Wait for scoped invalidation before dispatching replacement reads.
        let Ok(server) = uuid::Uuid::parse_str(&self.server) else {
            return;
        };
        self.request_repository_refresh("git-refresh", server);
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
            || self.git.refreshed.get("git-refresh").map(String::as_str) != self.git.repo_id()
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
        self.git.page = None;
        self.git.continuation = None;
        self.git.error = None;
        let remote = self.git.remote_name().map(str::to_owned);
        self.git.selected = None;
        if self.git.tab == 6
            && let Some(name) = remote
        {
            self.git.selected = Some(Entry::Remote(newport_protocol::git::RemoteName { name }));
        }
        self.git.pull_form = None;
        self.git.stash_apply = None;
        self.git.tag_delete = None;
        self.git.tag_push = None;
        self.git.worktree_remove = None;
        self.git.diff = None;
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
        self.git.page = None;
        self.git.selected = None;
        self.git.diff = None;
        self.git.continuation = None;
        self.git.diff_cursor = None;
        self.git.error = None;
        self.git.scroll = UniformListScrollHandle::default();
        self.git.listing.demand = git_page_demand::Demand::default();
    }
    fn git_cancel_checkout_switch(&mut self, error: String) {
        let path = self.git.switching_path.take();
        if let Some(repo) = self.git.switching_repo.take()
            && Some(repo.repo_id.as_str()) != self.git.repo_id()
        {
            release_repository(self.server.clone(), repo.repo_id);
        }
        self.resources.invalidate("repository");
        self.resources.invalidate("git-checkout-initial");
        self.notification = Some(Notification::error(format!(
            "Couldn’t open {}: {error}",
            path.map(|path| path.display).unwrap_or_default()
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
                let Some(repo) = self.git_opened_repository() else {
                    self.git_cancel_checkout_switch("Invalid repository response.".into());
                    return;
                };
                // Prepare the new checkout behind the current view. Its first
                // page uses the same defaults as the fresh workspace below.
                let next = Workspace {
                    tab: if repo.bare && self.git.tab == 0 {
                        1
                    } else {
                        self.git.tab
                    },
                    repo: Some(repo.clone()),
                    ..Default::default()
                };
                let Some(request) = next.read_request(None) else {
                    return;
                };
                self.git.switching_repo = Some(repo);
                self.resources.invalidate("git-checkout-initial");
                self.request_git_typed("git-checkout-initial", request);
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
            let Some(first_page) = self.git_listing(key) else {
                self.git_cancel_checkout_switch("Invalid repository listing.".into());
                return;
            };
            if let Listing::Remotes(page) = &first_page
                && let Err(error) = newport_client::git_remotes::Names::validate_first(page)
            {
                self.git_cancel_checkout_switch(error);
                return;
            }
            let Some(repo) = self.git.switching_repo.take() else {
                return;
            };
            let path = self.git.switching_path.take().unwrap();
            if Some(repo.repo_id.as_str()) != self.git.repo_id() {
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
                .complete_git("git-list", generation, Ok(first_page.into_reply()));
            self.git_repository_opened(repo);
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
                if let Some(head) = self
                    .git_status(key)
                    .map(|status| status.metadata.head.clone())
                {
                    self.git.observe_head(&head);
                }
            }
            "repository" => {
                if let Some(repo) = self.git_opened_repository() {
                    self.git_repository_opened(repo);
                }
            }
            "git-list" => {
                if let Some(Listing::Remotes(page)) = self.git_listing(key)
                    && let Err(error) = newport_client::git_remotes::Names::validate_first(&page)
                {
                    self.git.error = Some(error);
                    return;
                }
                self.git_listing_completed(false);
            }
            "git-more" => {
                if let Some(cursor) = self.git.continuation.take() {
                    let merged = match (&self.git.page, self.git_listing(key)) {
                        (Some(page), Some(incoming)) => page.merge(incoming, &cursor),
                        _ => Err("Invalid repository page. Refresh to retry.".into()),
                    };
                    match merged {
                        Ok(p) => self.git.page = Some(p),
                        Err(e) => self.git.error = Some(e),
                    }
                }
            }
            "git-diff" | "git-diff-more" => {
                let result = match self.git_reply(key) {
                    Some(newport_protocol::git::Reply::DiffPage(page))
                        if page.metadata.source_snapshot.as_deref()
                            == self.git.page.as_ref().map(Listing::snapshot)
                            && page.metadata.entry_id.as_deref()
                                == self.git.entry().map(|entry| entry.entry_id.as_str())
                            && self
                                .git
                                .diff_context
                                .matches(&page.metadata, self.git.diff_staged) =>
                    {
                        (**page).clone()
                    }
                    _ => {
                        self.git.error =
                            Some("The file changed. Refresh before reviewing its diff.".into());
                        return;
                    }
                };
                if key == "git-diff" {
                    self.git.diff = Some(result);
                    self.git.diff_recovery.reset();
                } else if let Some(cursor) = self.git.diff_cursor.take() {
                    let merged = match &self.git.diff {
                        Some(diff) => {
                            newport_client::git_diff_page::merge(diff, result, &cursor, true)
                        }
                        None => Err("Invalid repository page. Refresh to retry.".into()),
                    };
                    match merged {
                        Ok(page) => self.git.diff = Some(page),
                        Err(error) => self.git.error = Some(error),
                    }
                }
            }
            _ => {}
        }
    }
    /// The listing `key`'s read last returned.
    pub(super) fn git_listing(&self, key: &str) -> Option<Listing> {
        self.git_reply(key).and_then(Listing::from_reply)
    }
    /// The status page `key`'s read last returned.
    pub(super) fn git_status(&self, key: &str) -> Option<&StatusPage> {
        match self.git_reply(key) {
            Some(newport_protocol::git::Reply::Status(page)) => Some(page),
            _ => None,
        }
    }
    pub(super) fn git_listing_completed(&mut self, index_only: bool) {
        let Some(page) = self.git_listing("git-list") else {
            return;
        };
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
        let page = self.git.page.as_ref().and_then(Listing::status);
        if let Some((entry, staged)) = self.git.diff_recovery.resolve(page) {
            self.git.selected = Some(Entry::Change(entry));
            self.git.diff_staged = staged;
            self.git_diff_request(false);
        } else if comparison_changed && self.git.entry().is_some() {
            self.git_diff_request(false);
        }
    }
    pub(super) fn git_status_request(&self) -> GitRequest {
        GitRequest::Status {
            repo_id: self.git_repository_id(),
            filter: Some(self.git.changes.filter()),
            page_size: 100,
            cursor: None,
        }
    }
    pub(super) fn git_listing_request(&self, cursor: Option<String>) -> Option<GitRequest> {
        self.git.read_request(cursor)
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
        let entry = self.git.entry().cloned();
        match self
            .git
            .diff_recovery
            .begin(code, entry.as_ref(), self.git.diff_staged)
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
        self.git.page = None;
        self.git.selected = None;
        self.git.diff = None;
        self.git.continuation = None;
        self.git.diff_cursor = None;
        self.git.diff_demand = git_diff::ScrollDemand::default();
        self.git.diff_scroll = UniformListScrollHandle::default();
        self.git.error = None;
        self.git_load();
    }
    fn git_diff_request(&mut self, more: bool) {
        let Some(entry_id) = self.git.entry().map(|entry| entry.entry_id.clone()) else {
            return;
        };
        if self.git.index_refresh.is_some() || self.git.error.is_some() {
            return;
        }
        let side = if self.git.diff_staged {
            Side::HeadToIndex
        } else {
            Side::IndexToWorktree
        };
        let mut request_cursor = None;
        if more {
            if self.git.diff_cursor.is_some()
                || self
                    .resources
                    .get("git-diff-more")
                    .is_some_and(|r| r.error.is_some())
            {
                return;
            }
            let Some(cursor) = self
                .git
                .diff
                .as_ref()
                .and_then(|diff| diff.next_cursor.clone())
            else {
                return;
            };
            request_cursor = Some(cursor.clone());
            self.git.diff_cursor = Some(cursor);
        }
        self.request_git_typed(
            if more { "git-diff-more" } else { "git-diff" },
            GitRequest::DiffPage {
                repo_id: self.git_repository_id(),
                snapshot: self.git.snapshot().to_owned(),
                entry_id,
                side,
                page_size: 200,
                max_bytes: Some(65536),
                context_lines: u32::from(self.git.diff_context.value()),
                cursor: request_cursor,
                line_encoding: None,
            },
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
        self.git.diff = None;
        self.git.diff_cursor = None;
        self.git.diff_scroll = UniformListScrollHandle::default();
        self.git.error = None;
        if self.git.entry().is_some_and(|entry| entry.conflicted) {
            self.select_conflict_side(0);
        } else {
            self.git_diff_request(false);
        }
    }
    /// The repository the last open read returned.
    fn git_opened_repository(&self) -> Option<Repository> {
        match self.git_reply("repository") {
            Some(newport_protocol::git::Reply::Open(repo)) => Some((**repo).clone()),
            _ => None,
        }
    }
    fn git_repository_opened(&mut self, repo: Repository) {
        self.git.server = self.server.clone();
        self.git.repo = Some(repo);
        self.git.restore_document = true;
        self.load_checkout_summary();
        if let Some(path) = self.git.returned_from_missing_checkout.take() {
            self.notification = Some(Notification::info(format!(
                "The worktree {path} is no longer available. Opened the saved project."
            )));
        }
        if self.git.bare() == Some(true) && self.git.tab == 0 {
            self.git.tab = 1;
        }
        self.save_workspace_selection();
        if self.git.page.is_some() {
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
        let Some(_) = tab_info(self.git.tab) else {
            return;
        };
        if self.git.repo.is_none() {
            return;
        }
        if self.git.tab == 0 {
            self.git.changes.dispatched();
            self.resources.invalidate("git-clean-commit");
        }
        let Some(request) = self.git_listing_request(None) else {
            return;
        };
        self.request_git_typed("git-list", request);
        self.git_load_related();
    }
    fn git_load_related(&mut self) {
        if self.git.tab == 6 {
            self.git_select_remote();
            self.git_load_remote_review();
            if self.git.settings_open || self.git.settings.is_some() {
                self.git_load_repository_settings();
            }
            self.request_git_typed(
                "git-remote-status",
                GitRequest::Status {
                    repo_id: self.git_repository_id(),
                    page_size: 1,
                    filter: None,
                    cursor: None,
                },
            );
        }
        if matches!(self.git.tab, 1 | 4) {
            self.request_git_typed(
                "git-tag-status",
                GitRequest::Status {
                    repo_id: self.git_repository_id(),
                    page_size: 1,
                    filter: None,
                    cursor: None,
                },
            );
        }
        if self.git.tab == 3 {
            self.request_git_typed(
                "git-stash-status",
                GitRequest::Status {
                    repo_id: self.git_repository_id(),
                    page_size: 1,
                    filter: None,
                    cursor: None,
                },
            );
        }
    }
    pub(super) fn git_listing_scroll(&mut self) {
        let ready = self.page == Page::Projects
            && self.git.remote_refs.is_none()
            && !self.project_dialog_open
            && (!git_layout::compact(self.content_width, self.git.tab) || !self.git.detail_open)
            && self.git.error.is_none()
            && self.git.continuation.is_none()
            && self
                .git
                .page
                .as_ref()
                .and_then(Listing::next_cursor)
                .is_some()
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
        let identity = self.git.page.as_ref().map(Listing::identity);
        if self.git.listing.demand.take(&identity, ready, at_end) || (ready && scrolled) {
            self.git_more();
        }
    }
    pub(super) fn git_more(&mut self) {
        if self.git.index_refresh.is_some() {
            return;
        }
        let Some(_) = tab_info(self.git.tab) else {
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
        let Some(cursor) = self
            .git
            .page
            .as_ref()
            .and_then(Listing::next_cursor)
            .map(str::to_owned)
        else {
            return;
        };
        let Some(request) = self.git_listing_request(Some(cursor.clone())) else {
            return;
        };
        self.git.continuation = Some(cursor);
        self.request_git_typed("git-more", request);
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
            return format!("Opening {}…", path.display);
        }
        let resource = self.resources.get("git-list");
        let pending = resource.is_some_and(|read| read.pending);
        let failed = resource.is_some_and(|read| read.error.is_some()) || self.git.error.is_some();
        // Keep incomplete/error/filter feedback visible instead of presenting
        // a successful read as if it covered the entire listing.
        let page = self.git.page.as_ref();
        if pending || failed || page.is_some_and(Listing::truncated) {
            return listing_feedback(page, pending, failed);
        }
        if let Some(filtered) = self.git_filtered_feedback() {
            return filtered;
        }
        let age = read_feedback(
            resource
                .and_then(|read| read.updated_at)
                .map(|at| at.elapsed()),
        );
        if page.and_then(Listing::next_cursor).is_some() {
            format!("{age} · {}", listing_feedback(page, false, false))
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
                let repo = self.git.repo_id().map(str::to_owned);
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
                                        && this.git.repo_id() == repo.as_deref()
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
                                        && this.git.repo_id() == repo.as_deref()
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
        let scroll_key = self.git.page.as_ref().map(Listing::identity);
        let empty = self.git.page.as_ref().is_some_and(Listing::is_empty)
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
                                    let (title, subtitle) = row_text(e);
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
                                                this.git.selected.as_ref() == Some(e),
                                                cx,
                                            ))
                                            .selected(this.git.selected.as_ref() == Some(e))
                                            .w_full()
                                            .h_full()
                                            .min_w_0()
                                            .toggled(this.git.selected.as_ref() == Some(e))
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
                                                    .when_some(
                                                        match e {
                                                            Entry::Change(entry) => Some(entry),
                                                            _ => None,
                                                        },
                                                        |row, entry| {
                                                            row.child(
                                                                div()
                                                                    .text_size(px(
                                                                        crate::ui::FONT_SIZE,
                                                                    ))
                                                                    .text_color(
                                                                        cx.theme().muted_foreground,
                                                                    )
                                                                    .w(px(16.))
                                                                    .flex_shrink_0()
                                                                    .child(change_mark(entry)),
                                                            )
                                                        },
                                                    )
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
                                                    .when_some(
                                                        match e {
                                                            Entry::Commit(commit) => Some(commit),
                                                            _ => None,
                                                        },
                                                        |row, commit| {
                                                            row.child(
                                                                git_commit_header::history_row_identity(
                                                                    commit, cx,
                                                                ),
                                                            )
                                                        },
                                                    )
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
                                                    this.git.selected = Some(entry.clone());
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
                                                    if let Entry::Change(change) = &entry {
                                                        this.git_select_side(change.staged);
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
                    .on_scroll(cx.listener(move |this, event: &ScrollEvent, _, _| {
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
        if self.git.repo.is_none() {
            return self.git_open_screen(cx);
        }
        if self.git.bare() != Some(true) && self.code_workspace().is_some() {
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
                                .label(if self.git.bare() == Some(true) {
                                    "History"
                                } else {
                                    "Changes"
                                })
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.git_navigate(
                                        usize::from(this.git.bare() == Some(true)),
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
                        self.git
                            .page
                            .as_ref()
                            .and_then(Listing::next_cursor)
                            .is_some()
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
        Entry, GitPath, Listing, Workspace, checkout_target, listing_feedback, local_branch_target,
        worktree_destination,
    };
    use crate::git_fixture::{
        commit, diff_page, history_page, read_only_repository, status_entry, status_page, tag,
        working_metadata,
    };
    use newport_client::resources::Resources;
    use newport_protocol::git::{Path, StatusEntry};

    fn status(snapshot: &str, entries: Vec<StatusEntry>, next: Option<&str>) -> Listing {
        Listing::Status(Box::new(status_page(snapshot, entries, next)))
    }
    /// A change to the file named `a`, in the given groups.
    fn change(id: &str, staged: bool, unstaged: bool, untracked: bool) -> StatusEntry {
        let mut entry = status_entry(id, "a");
        entry.staged = staged;
        entry.unstaged = unstaged;
        entry.untracked = untracked;
        entry
    }
    fn selected(entry: &StatusEntry) -> Option<Entry> {
        Some(Entry::Change(entry.clone()))
    }
    fn old_diff() -> Option<newport_client::git_diff_page::DiffPage> {
        Some(diff_page(
            "old diff",
            working_metadata("before", "entry"),
            vec![],
            None,
        ))
    }

    #[test]
    fn worktree_open_retains_the_visible_checkout_until_success() {
        let old_path = GitPath::new(b"/old");
        let next = GitPath::new(b"/new");
        let old = commit("old", "Old");
        let mut view = Workspace {
            repo: Some(read_only_repository("old")),
            opening_path: Some(old_path.clone()),
            tab: 1,
            page: Some(history_page("old", vec![old.clone()], None)),
            selected: Some(Entry::Commit(old.clone())),
            diff: Some(diff_page(
                "visible diff",
                working_metadata("before", "entry"),
                vec![],
                None,
            )),
            detail_open: true,
            ..Default::default()
        };
        let page = view.page.clone();
        assert!(!view.begin_checkout_switch(old_path.clone()));
        assert!(view.begin_checkout_switch(next.clone()));
        assert_eq!(view.repo_id(), Some("old"));
        assert_eq!(view.opening_path, Some(old_path.clone()));
        assert_eq!(view.page, page);
        assert_eq!(view.selected, Some(Entry::Commit(old)));
        assert_eq!(view.diff.as_ref().unwrap().snapshot, "visible diff");
        assert!(!view.begin_checkout_switch(GitPath::new(b"/third")));
        assert_eq!(view.switch_view(0, &mut Resources::default()), None);
        assert_eq!(view.switching_path, Some(next));
        // Failed opens only retire the pending destination, not the old view.
        view.switching_path.take();
        assert_eq!(view.page, page);
        assert_eq!(view.repo_id(), Some("old"));
        assert_eq!(view.opening_path, Some(old_path.clone()));
    }

    #[test]
    fn successful_worktree_switch_preserves_layout_but_discards_checkout_state() {
        for tab in [0, 1, 2, 3, 4, 6] {
            let old = status_entry("old-file", "old");
            let mut view = Workspace {
                repo: Some(read_only_repository("old")),
                tab,
                detail_open: true,
                page: Some(status("old", vec![old.clone()], None)),
                selected: selected(&old),
                diff: old_diff(),
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
            let path = GitPath::new(b"/new");
            assert!(view.begin_checkout_switch(path.clone()));
            view.replace_checkout(path.clone());
            assert_eq!(view.tab, tab);
            assert!(view.detail_open);
            assert_eq!(view.opening_path.as_ref(), Some(&path));
            assert!(view.repo.is_none());
            assert!(view.page.is_none());
            assert!(view.selected.is_none());
            assert!(view.diff.is_none());
            assert!(view.error.is_none());
            assert!(view.switching_path.is_none());
            assert!(view.saved_views.is_empty());
        }
    }

    #[test]
    fn prepared_checkout_installs_its_first_page_without_an_empty_intermediate_view() {
        for tab in [0, 1, 2, 3, 4, 6] {
            let old = status_entry("old-file", "old");
            let mut view = Workspace {
                repo: Some(read_only_repository("old")),
                tab,
                detail_open: true,
                page: Some(status("old", vec![old.clone()], None)),
                selected: selected(&old),
                diff: old_diff(),
                ..Default::default()
            };
            let path = GitPath::new(b"/new");
            assert!(view.begin_checkout_switch(path.clone()));
            // repo.open alone must not clear or relabel the current contents.
            let repo = read_only_repository("new");
            view.switching_repo = Some(repo.clone());
            assert_eq!(view.repo_id(), Some("old"));
            assert_eq!(view.entry().unwrap().entry_id, "old-file");
            let mut status =
                status_page("new", vec![status_entry("new-file", "new")], Some("next"));
            status.metadata.head = repo.head.clone();
            let page = Listing::Status(Box::new(status));
            view.install_checkout(path.clone(), repo.clone(), page.clone());
            assert_eq!(view.repo, Some(repo));
            assert_eq!(view.page, Some(page));
            assert_eq!(view.opening_path.as_ref(), Some(&path));
            assert_eq!(view.tab, tab);
            assert!(view.detail_open);
            assert!(view.selected.is_none());
            assert!(view.diff.is_none());
            assert!(view.switching_path.is_none());
            assert!(view.switching_repo.is_none());
        }
    }

    #[test]
    fn prepared_checkout_keeps_empty_results_and_bare_repository_history() {
        let mut view = Workspace::default();
        let page = history_page("empty", vec![], None);
        let mut bare = read_only_repository("bare");
        bare.bare = true;
        view.install_checkout(GitPath::new(b"/new"), bare, page.clone());
        assert_eq!(view.tab, 1);
        assert_eq!(view.page, Some(page));
        assert_eq!(
            serde_json::to_value(view.read_request(None).unwrap()).unwrap()["params"]["messageBytes"],
            512
        );
        assert_eq!(
            serde_json::to_value(view.read_request(None).unwrap()).unwrap()["params"]["repoId"],
            "bare"
        );
    }

    #[test]
    fn removed_worktree_page_cannot_replace_the_active_workspace() {
        let mut view = Workspace {
            repo: Some(read_only_repository("repo")),
            page: Some(status("changes", vec![], None)),
            ..Default::default()
        };
        let mut resources = Resources::default();
        assert!(super::tab_info(5).is_none());
        assert_eq!(view.switch_view(5, &mut resources), None);
        assert_eq!(view.tab, 0);
        assert_eq!(view.snapshot(), "changes");
        assert!(view.saved_views.is_empty());
        assert_eq!(view.switch_view(6, &mut resources), Some(true));
        assert_eq!(
            super::tab_info(view.tab),
            Some(("Remotes", "repo.remote_names"))
        );
    }
    #[test]
    fn bare_repository_cannot_navigate_to_changes() {
        let mut bare = read_only_repository("bare");
        bare.bare = true;
        let mut view = Workspace {
            repo: Some(bare),
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
        let same = change("same-path", false, true, false);
        let mut moved = same.clone();
        moved.path = Path::new(b"elsewhere");
        let mut view = Workspace {
            page: Some(status("old", vec![], None)),
            selected: selected(&moved),
            diff: Some(diff_page(
                "old contents",
                working_metadata("before", "entry"),
                vec![],
                None,
            )),
            ..Default::default()
        };
        assert!(view.accept_listing(status("new", vec![same], None)));
        assert!(view.selected.is_none());
        assert!(view.diff.is_none());
        assert!(!view.accept_listing(status("new", vec![], None)));
    }
    #[test]
    fn background_status_keeps_loaded_pages_until_the_snapshot_changes() {
        let entry = change("old", false, true, false);
        let mut first = status_entry("first", "first");
        first.unstaged = false;
        let page = status(
            "old",
            vec![first.clone(), entry.clone()],
            Some("after-second-page"),
        );
        let mut view = Workspace {
            repo: Some(read_only_repository("repo")),
            page: Some(page.clone()),
            selected: selected(&entry),
            diff: Some(diff_page(
                "reviewed lines",
                working_metadata("before", "entry"),
                vec![],
                None,
            )),
            ..Default::default()
        };
        assert!(!view.accept_listing(status("old", vec![first], Some("after-first-page"))));
        assert_eq!(view.page, Some(page));
        assert_eq!(view.selected, selected(&entry));
        assert_eq!(view.diff.as_ref().unwrap().snapshot, "reviewed lines");

        let mut current = entry.clone();
        current.entry_id = "current".into();
        assert!(view.accept_listing(status("new", vec![current.clone()], None)));
        assert_eq!(view.selected, selected(&current));
        assert!(view.diff.is_none());
        assert!(view.page.as_ref().unwrap().next_cursor().is_none());
        assert!(view.accept_listing(status("clean", vec![], None)));
        assert!(view.selected.is_none());
    }
    #[test]
    fn observed_external_changes_retire_suspended_changes_but_keep_current_history() {
        let mut view = Workspace {
            repo: Some(read_only_repository("repo")),
            page: Some(status("before", vec![], None)),
            ..Default::default()
        };
        let mut resources = Resources::default();
        view.switch_view(1, &mut resources);
        let inspected = Entry::Commit(commit("inspected", "Inspected"));
        view.selected = Some(inspected.clone());
        assert!(!view.observe_status_snapshot(&status_page("before", vec![], None)));
        assert!(view.observe_status_snapshot(&status_page("after", vec![], None)));
        assert_eq!(view.selected, Some(inspected));
        assert_eq!(view.switch_view(0, &mut resources), Some(true));
        assert!(view.page.is_none());
    }
    #[test]
    fn index_refresh_follows_the_selected_file_between_groups() {
        let entry = change("old", false, false, true);
        let mut view = Workspace {
            page: Some(status("before", vec![entry.clone()], None)),
            selected: selected(&entry),
            diff: Some(diff_page(
                "old-diff",
                working_metadata("before", "entry"),
                vec![],
                None,
            )),
            ..Default::default()
        };
        let staged = change("new", true, false, false);
        assert!(view.accept_listing_with_scope(status("staged", vec![staged.clone()], None), true));
        assert_eq!(view.selected, selected(&staged));
        assert!(view.diff_staged);
        assert!(view.diff.is_none());
        let unstaged = change("unstaged", false, true, false);
        assert!(
            view.accept_listing_with_scope(status("unstaged", vec![unstaged.clone()], None), true)
        );
        assert_eq!(view.selected, selected(&unstaged));
        assert!(!view.diff_staged);
    }
    #[test]
    fn index_refresh_keeps_partial_or_unrelated_selection_but_retires_old_authority() {
        let entry = change("old", false, true, false);
        let old_diff = Some(diff_page(
            "old-diff",
            working_metadata("before", "old"),
            vec![],
            None,
        ));
        let mut view = Workspace {
            page: Some(status("before", vec![entry.clone()], None)),
            selected: selected(&entry),
            diff: old_diff.clone(),
            ..Default::default()
        };
        let partial = change("new", true, true, false);
        assert!(view.accept_listing_with_scope(status("after", vec![partial.clone()], None), true));
        assert_eq!(view.selected, selected(&partial));
        assert!(!view.diff_staged);
        assert_eq!(view.diff, old_diff);
        assert_ne!(
            view.diff
                .as_ref()
                .unwrap()
                .metadata
                .source_snapshot
                .as_deref(),
            Some(view.snapshot())
        );
        // A filtered-out or removed file must not select a different sibling.
        let mut sibling = change("sibling", true, false, false);
        sibling.path = Path::new(b"b");
        assert!(view.accept_listing_with_scope(status("filtered", vec![sibling], None), true));
        assert!(view.selected.is_none());
        assert!(view.diff.is_none());
    }
    #[test]
    fn diff_recovery_rebinds_to_new_status_and_is_cancelled_when_leaving_changes() {
        use newport_client::git_read_recovery::Decision;
        let entry = change("old", false, true, false);
        let mut view = Workspace {
            repo: Some(read_only_repository("repo")),
            page: Some(status("old", vec![], None)),
            selected: selected(&entry),
            ..Default::default()
        };
        assert_eq!(
            view.diff_recovery
                .begin(Some("STALE_SNAPSHOT"), Some(&entry), false),
            Decision::Refresh
        );
        // Stale-error recovery retires the old comparison before rereading;
        // an ordinary background refresh instead retains the selected path.
        view.selected = None;
        view.page = None;
        let mut next = entry.clone();
        next.entry_id = "current".into();
        view.accept_listing(status("current", vec![next.clone()], None));
        assert!(view.selected.is_none());
        let page = view.status_page().cloned();
        assert_eq!(
            view.diff_recovery.resolve(page.as_ref()),
            Some((next, false))
        );

        assert_eq!(
            view.diff_recovery
                .begin(Some("STALE_ENTRY"), Some(&entry), false),
            Decision::Refresh
        );
        let mut resources = Resources::default();
        let read = resources.begin("git-list").unwrap();
        view.switch_view(1, &mut resources);
        assert!(!resources.complete_git(
            "git-list",
            read,
            Ok(status("late", vec![entry], None).into_reply())
        ));
        view.switch_view(0, &mut resources);
        assert!(!view.diff_recovery.pending());
    }
    #[test]
    fn navigation_preserves_completed_selection_and_does_not_reload_current_view() {
        let mut view = Workspace {
            repo: Some(read_only_repository("repo")),
            server: "server".into(),
            ..Default::default()
        };
        let file = status_entry("file", "file");
        let page = status("status", vec![file.clone()], None);
        view.page = Some(page.clone());
        view.selected = selected(&file);
        let mut resources = Resources::default();
        let read = resources.begin("git-list").unwrap();
        resources.complete_git("git-list", read, Ok(page.clone().into_reply()));
        let log = resources.begin("git-log-events").unwrap();
        assert_eq!(view.switch_view(0, &mut resources), None);
        assert_eq!(view.selected, selected(&file));
        assert_eq!(view.switch_view(1, &mut resources), Some(true));
        assert!(resources.get("git-list").is_none());
        assert!(resources.complete_query("git-log-events", log, &Default::default(), true, true));
        let history = resources.begin("git-list").unwrap();
        resources.complete_git(
            "git-list",
            history,
            Ok(history_page("history", vec![], None).into_reply()),
        );
        let fresh = crate::git_fixture::repository("repo").head;
        view.repo.as_mut().unwrap().head = fresh.clone();
        assert_eq!(view.switch_view(0, &mut resources), Some(false));
        assert_eq!(view.page, Some(page.clone()));
        assert_eq!(view.selected, selected(&file));
        assert_eq!(
            resources.get("git-list").unwrap().git(),
            Some(&page.into_reply())
        );
        assert_eq!(view.repo.as_ref().unwrap().head, fresh);
        assert_eq!(view.saved_views.len(), 1);
    }
    #[test]
    fn checkout_path_survives_new_cached_and_invalidated_tabs() {
        // The bookmark may be a subdirectory or symlink: repo.root cannot
        // replace the original requested path used by persistence/recovery.
        let path = GitPath::new(b"/linked/src");
        let mut repo = read_only_repository("repo");
        repo.root = GitPath::new(b"/linked");
        let mut view = Workspace {
            repo: Some(repo),
            opening_path: Some(path.clone()),
            ..Default::default()
        };
        let mut resources = Resources::default();
        for tab in [1, 4, 0, 3, 1] {
            assert!(view.switch_view(tab, &mut resources).is_some());
            assert_eq!(view.opening_path.as_ref(), Some(&path));
        }
        view.invalidate_saved_views();
        view.switch_view(0, &mut resources);
        assert_eq!(view.opening_path.as_ref(), Some(&path));
    }
    #[test]
    fn checkout_summary_and_pending_read_survive_tabs_without_restoring_stale_metadata() {
        let mut view = Workspace {
            repo: Some(read_only_repository("repo")),
            ..Default::default()
        };
        let listing = |total| {
            let mut page = crate::git_fixture::worktree_page("s", vec![], None);
            page.metadata.total_entries = total;
            page
        };
        view.checkout_summary.accept(&listing(2));
        let mut resources = Resources::default();
        let read = resources
            .begin(crate::git_checkout_picker::SUMMARY)
            .unwrap();
        view.switch_view(1, &mut resources);
        let fresh = listing(3);
        assert!(resources.complete_git(
            crate::git_checkout_picker::SUMMARY,
            read,
            Ok(newport_protocol::git::Reply::Worktrees(Box::new(
                fresh.clone()
            )))
        ));
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
            repo: Some(read_only_repository("repo")),
            ..Default::default()
        };
        let wrong = || {
            Ok(Listing::Tags(newport_protocol::git::Page {
                snapshot: "s".into(),
                entries: vec![tag("wrong", "a")],
                next_cursor: None,
                metadata: newport_protocol::git::TotalEntries { total_entries: 1 },
            })
            .into_reply())
        };
        let mut resources = Resources::default();
        let abandoned = resources.begin("git-list").unwrap();
        assert_eq!(view.switch_view(4, &mut resources), Some(true));
        let tags = resources.begin("git-list").unwrap();
        assert!(!resources.complete_git("git-list", abandoned, wrong()));
        resources.complete_git("git-list", tags, Ok(status("s", vec![], None).into_reply()));
        assert_eq!(view.switch_view(0, &mut resources), Some(true));
        assert!(resources.get("git-list").is_none());
        assert!(!resources.complete_git("git-list", abandoned, wrong()));
    }
    #[test]
    fn cached_view_cannot_replay_an_old_scroll_gesture() {
        let page = Listing::Tags(newport_protocol::git::Page {
            snapshot: "s".into(),
            entries: vec![tag("tag", "a")],
            next_cursor: Some("next".into()),
            metadata: newport_protocol::git::TotalEntries { total_entries: 2 },
        });
        let mut view = Workspace {
            repo: Some(read_only_repository("repo")),
            page: Some(page.clone()),
            tab: 4,
            ..Default::default()
        };
        let identity = Some(page.identity());
        view.listing.demand.arm(identity.clone());
        let mut resources = Resources::default();
        view.switch_view(3, &mut resources);
        view.switch_view(4, &mut resources);
        assert!(!view.listing.demand.take(&identity, true, true));
        view.listing.demand.arm(identity.clone());
        assert!(view.listing.demand.take(&identity, true, true));
        assert!(!view.listing.demand.take(&identity, true, true));
    }
    #[test]
    fn write_outcomes_invalidate_suspended_reviews_and_snapshots() {
        let previous = status_entry("previous", "previous");
        let mut view = Workspace {
            repo: Some(read_only_repository("repo")),
            selected: selected(&previous),
            ..Default::default()
        };
        let mut resources = Resources::default();
        let read = resources.begin("git-list").unwrap();
        resources.complete_git(
            "git-list",
            read,
            Ok(status("before", vec![], None).into_reply()),
        );
        view.switch_view(1, &mut resources);
        view.invalidate_saved_views();
        assert_eq!(view.switch_view(0, &mut resources), Some(true));
        assert!(view.selected.is_none());
        assert!(resources.get("git-list").is_none());
    }
    #[test]
    fn changes_refresh_updates_head_without_inventing_completion() {
        let mut view = Workspace {
            repo: Some(crate::git_fixture::repository("repo")),
            ..Default::default()
        };
        let mut page = status_page("s", vec![], None);
        page.metadata.head = newport_protocol::git::Head {
            oid: Some(crate::git_fixture::object("1234567890abcdef")),
            name: None,
            detached: true,
            unborn: false,
        };
        view.accept_listing(Listing::Status(Box::new(page.clone())));
        let head = &view.repo.as_ref().unwrap().head;
        assert!(head.detached);
        assert_eq!(super::head_label(head), "Detached HEAD · 12345678");
        assert_eq!(listing_feedback(None, false, false), "");
        assert!(listing_feedback(view.page.as_ref(), true, false).starts_with("Loading"));
        assert!(listing_feedback(view.page.as_ref(), false, true).starts_with("Couldn’t"));
        page.metadata.truncated = true;
        assert!(
            listing_feedback(Some(&Listing::Status(Box::new(page))), false, false)
                .contains("incomplete")
        );
    }
    #[test]
    fn worktree_navigation_preserves_bytes_and_rejects_unavailable_rows() {
        let mut entry = crate::git_fixture::worktree(None, "/");
        entry.path = GitPath::new(b"/\xff");
        assert_eq!(
            worktree_destination(&entry),
            Some(GitPath {
                bytes_b64: "L/8=".into(),
                display: entry.path.display.clone(),
            })
        );
        let mut missing = entry.clone();
        missing.state = "missing".into();
        let mut current = entry.clone();
        current.current = true;
        let mut bare = entry.clone();
        bare.kind = "bare".into();
        let mut undecodable = entry.clone();
        undecodable.path.bytes_b64 = "not-base64".into();
        let mut relative = entry.clone();
        relative.path = GitPath::new(b"relative/checkout");
        let mut nul = entry;
        nul.path = GitPath::new(b"/checkout\0name");
        for invalid in [missing, current, bare, undecodable, relative, nul] {
            assert!(worktree_destination(&invalid).is_none());
        }
    }
    #[test]
    fn checkout_rejects_remote_current_and_lossy_names() {
        let branch = crate::git_fixture::branch("topic", "abcd");
        assert_eq!(
            checkout_target(Some(&branch)),
            Some(("topic".into(), "abcd".into()))
        );
        let mut remote = branch.clone();
        remote.remote = true;
        assert!(checkout_target(Some(&remote)).is_none());
        assert!(local_branch_target(&remote).is_none());
        let mut current = branch.clone();
        current.current = true;
        assert!(checkout_target(Some(&current)).is_none());
        assert!(local_branch_target(&current).is_some());
        let mut invalid = branch;
        invalid.name.bytes_b64 = "/w==".into();
        assert!(checkout_target(Some(&invalid)).is_none());
        assert!(checkout_target(None).is_none());
    }
    #[test]
    fn pagination_requires_same_snapshot_and_metadata() {
        let first = commit("first", "First");
        let second = commit("second", "Second");
        let previous = history_page("a", vec![first.clone()], Some("next"));
        let page = history_page("a", vec![second.clone()], None);
        assert_eq!(
            previous.merge(page.clone(), "next").unwrap(),
            history_page("a", vec![first, second.clone()], None)
        );
        let stale = history_page("b", vec![second], None);
        assert!(previous.merge(stale, "next").is_err());
        assert!(previous.merge(previous.clone(), "next").is_err());
        assert!(previous.merge(status("a", vec![], None), "next").is_err());
    }
}
