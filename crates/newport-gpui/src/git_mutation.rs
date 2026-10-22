//! UI ownership survives navigation; writes are never automatically replayed.
use super::*;
use newport_gpui::git_operation::{Operation, State};
pub(super) struct GitNotice;
enum Reply {
    Rejected(String),
    Write(Result<Operation, newport_gpui::git_operation::WriteError>),
    Outcome(Result<Operation, String>),
}
pub struct Mutation {
    index_only: bool,
    pub(super) project_creation: Option<(uuid::Uuid, String)>,
    pub(super) integration_review: Option<uuid::Uuid>,
    pub(super) remote_refs_write: bool,
    pub(super) settings_draft: Option<(uuid::Uuid, String, String, bool)>,
    pub(super) remote_draft: Option<(uuid::Uuid, String, String)>,
    pub(super) worktree_create_draft: Option<super::git_worktree_create::Draft>,
    pub(super) worktree_repair_draft: Option<(uuid::Uuid, String)>,
    pub(super) worktree_draft: Option<(uuid::Uuid, String)>,
    pub(super) tag_draft: Option<git_tags::Draft>,
    pub(super) stash_draft: Option<(uuid::Uuid, String, bool, bool)>,
    pub(super) sync_draft: Option<uuid::Uuid>,
    pub(super) transfer_draft: Option<(uuid::Uuid, String)>,
    pub(super) branch_form: Option<(uuid::Uuid, String)>,
    pub(super) branch_management: Option<git_branch_manage::Draft>,
    pub(super) commit_draft: Option<(String, String, String)>,
    server: String,
    repo: String,
    id: uuid::Uuid,
    pending: Option<mpsc::Receiver<Reply>>,
    outcome: Option<Operation>,
    error: Option<String>,
    not_started: bool,
    context: String,
    title: String,
    receipt: Option<newport_gpui::git_operation::ReceiptState>,
    recovery_at: Option<Instant>,
    recovery_attempt: u8,
    last_notice: Option<String>,
}
impl Mutation {
    pub(super) fn dialog_error(&self) -> Option<String> {
        if self.pending.is_some() {
            return None;
        }
        failure_notice(
            self.outcome.as_ref().map(|o| &o.state),
            self.error.as_deref().or_else(|| {
                self.outcome
                    .as_ref()
                    .and_then(|o| o.error.as_ref().map(|error| error.message.as_str()))
            }),
            self.not_started,
            self.receipt,
        )
        .map(|(message, _)| message)
    }
    pub(super) fn id(&self) -> uuid::Uuid {
        self.id
    }
}
impl Newport {
    pub(super) fn git_write_pending(&self) -> bool {
        self.git_mutation
            .as_ref()
            .is_some_and(|state| state.pending.is_some())
    }
    pub(super) fn git_mutation_blocks_write(&self) -> bool {
        self.git.switching_path.is_some()
            || self.git.index_refresh.is_some()
            || self.resources.get("git-refresh").is_some_and(|r| r.pending)
            || self.git_mutation.as_ref().is_some_and(|state| {
                state.pending.is_some()
                    || (!state.not_started
                        && !confirmed_write_outcome(
                            state.outcome.as_ref().map(|o| &o.state),
                            state.receipt,
                        ))
            })
    }
    pub(super) fn git_fetch(&mut self, repo: String, remote: String, cx: &mut Context<Self>) {
        self.start_git_mutation(
            repo,
            format!("Fetch {remote}"),
            move |backend, server, repo, id| backend.fetch(server, repo, &remote, id),
            cx,
        );
    }
    pub(super) fn start_git_mutation(
        &mut self,
        repo: String,
        title: String,
        work: impl FnOnce(
            backend::Backend,
            &str,
            &str,
            uuid::Uuid,
        ) -> Result<Operation, newport_gpui::git_operation::WriteError>
        + Send
        + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.git_mutation_blocks_write() {
            return;
        }
        self.git.invalidate_saved_views();
        let server = self.server.clone();
        let id = uuid::Uuid::new_v4();
        let (send, receive) = mpsc::sync_channel(1);
        let mut state = Mutation {
            index_only: false,
            project_creation: None,
            integration_review: None,
            remote_refs_write: false,
            settings_draft: None,
            remote_draft: None,
            worktree_draft: None,
            worktree_repair_draft: None,
            worktree_create_draft: None,
            tag_draft: None,
            stash_draft: None,
            transfer_draft: None,
            sync_draft: None,
            branch_form: None,
            branch_management: None,
            commit_draft: None,
            title,
            receipt: None,
            recovery_at: None,
            recovery_attempt: 0,
            last_notice: None,
            server,
            repo,
            id,
            pending: Some(receive),
            outcome: None,
            error: None,
            not_started: false,
            context: format!(
                "{} · {}",
                self.selected_server()
                    .map(|s| text(s, "name"))
                    .unwrap_or_default(),
                self.selection
                    .as_ref()
                    .map(|s| text(s, "name"))
                    .unwrap_or_default()
            ),
        };
        let (server, repo) = (state.server.clone(), state.repo.clone());
        match std::thread::Builder::new()
            .name("git-write".into())
            .spawn(move || {
                let backend = backend::Backend::discover();
                let result = work(backend.clone(), &server, &repo, id);
                if let Ok(operation) = &result {
                    retire_completed_receipt(&backend, &server, id, &operation.state);
                }
                if let Err(error) = &result
                    && error.dispatched
                    && retire_rejected_receipt(&backend, &server, id)
                {
                    let _ = send.send(Reply::Rejected(error.message.clone()));
                    return;
                }
                let _ = send.send(Reply::Write(result));
            }) {
            Ok(_) => {}
            Err(error) => {
                state.pending = None;
                state.error = Some(error.to_string());
                state.not_started = true;
            }
        }
        self.git_mutation = Some(state);
        self.notify_git_failure(cx);
        cx.notify();
    }
    pub(super) fn start_git_index_mutation(
        &mut self,
        repo: String,
        title: String,
        work: impl FnOnce(
            backend::Backend,
            &str,
            &str,
            uuid::Uuid,
        ) -> Result<Operation, newport_gpui::git_operation::WriteError>
        + Send
        + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.git_mutation_blocks_write() {
            return;
        }
        self.start_git_mutation(repo, title, work, cx);
        if let Some(state) = self.git_mutation.as_mut() {
            state.index_only = true;
        }
    }
    fn check_git_outcome(&mut self, reviewed: bool, cx: &mut Context<Self>) {
        let Some(state) = self.git_mutation.as_mut() else {
            return;
        };
        if state.pending.is_some() {
            return;
        }
        let (send, receive) = mpsc::sync_channel(1);
        state.pending = Some(receive);
        state.recovery_at = None;
        let server = state.server.clone();
        let id = state.id;
        if let Err(error) = std::thread::Builder::new()
            .name("git-outcome".into())
            .spawn(move || {
                let backend = backend::Backend::discover();
                let result = if reviewed {
                    backend
                        .call(
                            "git_review_operation",
                            json!({"serverId":server,"operationId":id}),
                        )
                        .and_then(|_| backend.git_operation(&server, id))
                } else {
                    backend.git_operation(&server, id)
                };
                if let Ok(operation) = &result {
                    retire_completed_receipt(&backend, &server, id, &operation.state);
                }
                // The lookup can prove the command never started (the server
                // has no record of it); the backend then marks it rejected.
                if result.is_err() && retire_rejected_receipt(&backend, &server, id) {
                    let _ = send.send(Reply::Rejected(
                        "The command never started on the server, so nothing changed.".into(),
                    ));
                    return;
                }
                let result = Reply::Outcome(result);
                let _ = send.send(result);
            })
        {
            state.pending = None;
            state.error = Some(error.to_string());
            state.recovery_at = Some(Instant::now() + Duration::from_secs(30));
            self.notify_git_failure(cx);
        }
        cx.notify();
    }
    pub(super) fn poll_git_mutation(&mut self, cx: &mut Context<Self>) -> bool {
        if self.git_mutation.as_ref().is_some_and(|state| {
            state.pending.is_none() && state.recovery_at.is_some_and(|at| Instant::now() >= at)
        }) {
            self.check_git_outcome(false, cx);
        }
        let current_repo = self.git_repository_id();
        let Some(state) = self.git_mutation.as_mut() else {
            return false;
        };
        let Some(receive) = &state.pending else {
            return false;
        };
        let reply = match receive.try_recv() {
            Ok(reply) => reply,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => Reply::Outcome(Err(
                "The connection was interrupted. Checking whether the command completed.".into(),
            )),
        };
        state.pending = None;
        state.error = None;
        // A view may have been suspended while this operation was running.
        // Even failures can leave changed repository state or a stale review;
        // never restore those cached confirmations after an outcome arrives.
        if self.server == state.server && current_repo == state.repo {
            self.git.invalidate_saved_views();
        }
        let mut refresh = false;
        let mut created_bookmark = None;
        let mut created_checkout = None;
        let mut refresh_remote_refs = false;
        match reply {
            Reply::Rejected(error) => {
                state.receipt = Some(newport_gpui::git_operation::ReceiptState::Rejected);
                // The backend durably confirmed rejection before execution. This
                // is not an uncertain write and must not block recovery controls.
                state.error = Some(error);
                refresh = self.server == state.server
                    && self.page == Page::Projects
                    && current_repo == state.repo;
            }
            Reply::Outcome(Ok(outcome)) | Reply::Write(Ok(outcome)) => {
                if outcome.state == State::Succeeded
                    && let Some((id, name)) = state.project_creation.take()
                {
                    if let Some(form) = self.project_create.as_mut()
                        && form.id == id
                    {
                        form.created = true;
                    }
                    if let Some(path) = outcome.result.as_ref().and_then(|r| r.get("path")) {
                        created_bookmark =
                            Some((id, json!({"serverId":state.server,"name":name,"path":path})));
                    } else {
                        state.error=Some("Repository created, but no canonical path was returned. Add the existing repository; do not repeat creation.".into());
                    }
                }
                if matches!(outcome.state, State::Succeeded | State::NeedsResolution)
                    && let Some(id) = state.integration_review
                    && self
                        .git
                        .integration_review
                        .as_ref()
                        .is_some_and(|review| review.id == id)
                {
                    self.git.integration_review = None;
                }
                if state.remote_refs_write
                    && self.server == state.server
                    && self.page == Page::Projects
                    && current_repo == state.repo
                {
                    if outcome.state == State::Succeeded {
                        refresh_remote_refs = true;
                    } else if outcome.state == State::Failed
                        && let Some(review) = self.git.remote_ref_review.as_mut()
                    {
                        review.stale = true;
                    }
                }
                if let Some(draft) = state.settings_draft.as_ref()
                    && let Some(form) = self.git.settings.as_mut()
                    && form.draft(cx).0 == draft.0
                {
                    if outcome.state == State::Failed {
                        form.needs_review = true;
                        refresh = self.server == state.server
                            && self.page == Page::Projects
                            && current_repo == state.repo;
                    } else if outcome.state == State::Succeeded && form.draft(cx) == *draft {
                        self.git.settings = None;
                    }
                }
                if outcome.state == State::Failed
                    && let Some(draft) = state.remote_draft.as_ref()
                    && let Some(form) = self.git.remote_edit.as_mut()
                    && form.draft(cx).0 == draft.0
                {
                    form.needs_review = true;
                    refresh = self.server == state.server
                        && self.page == Page::Projects
                        && current_repo == state.repo;
                }
                if outcome.state == State::Succeeded
                    && let Some(draft) = state.remote_draft.take()
                    && self
                        .git
                        .remote_edit
                        .as_ref()
                        .is_some_and(|form| form.draft(cx) == draft)
                {
                    self.git.remote_edit = None;
                }
                if outcome.state == State::Succeeded
                    && let Some(draft) = state.worktree_repair_draft.take()
                    && self
                        .git
                        .worktree_repair
                        .as_ref()
                        .is_some_and(|form| form.draft(cx) == draft)
                {
                    self.git.worktree_repair = None;
                }
                if outcome.state == State::Succeeded
                    && let Some(draft) = state.worktree_create_draft.take()
                    && self.server == state.server
                    && self.page == Page::Projects
                    && current_repo == state.repo
                    && self
                        .git
                        .worktree_create
                        .as_ref()
                        .is_some_and(|form| form.draft(cx) == draft)
                {
                    created_checkout = draft.open_destination();
                    self.git.worktree_create = None;
                }
                if outcome.state == State::Succeeded
                    && let Some(draft) = state.worktree_draft.take()
                    && self
                        .git
                        .worktree_lock
                        .as_ref()
                        .is_some_and(|form| form.draft(cx) == draft)
                {
                    self.git.worktree_lock = None;
                }
                if outcome.state == State::Succeeded
                    && let Some(draft) = state.tag_draft.take()
                    && self
                        .git
                        .tag_form
                        .as_ref()
                        .is_some_and(|form| form.draft(cx) == draft)
                {
                    self.git.tag_form = None;
                }
                if outcome.state == State::Succeeded
                    && let Some(draft) = state.stash_draft.take()
                    && self
                        .git
                        .stash_save
                        .as_ref()
                        .is_some_and(|form| form.draft(cx) == draft)
                {
                    self.git.stash_save = None;
                }
                if outcome.state == State::Succeeded
                    && let Some(draft) = state.transfer_draft.take()
                    && self
                        .git
                        .pull_form
                        .as_ref()
                        .is_some_and(|form| form.draft(cx) == draft)
                {
                    self.git.pull_form = None;
                }
                if outcome.state == State::Succeeded
                    && let Some(id) = state.sync_draft.take()
                    && self
                        .git
                        .sync_form
                        .as_ref()
                        .is_some_and(|form| form.id == id)
                {
                    self.git.sync_form = None;
                }
                if let Some(draft) = state.branch_management.as_ref()
                    && let Some(form) = self.git.branch_manage.as_mut()
                    && matches!(outcome.state, State::Succeeded | State::Failed)
                    && form.accept_outcome(draft, outcome.state == State::Succeeded)
                {
                    self.git.branch_manage = None;
                }
                if outcome.state == State::Succeeded
                    && let Some((id, name)) = state.branch_form.take()
                    && self.git.branch_form.as_ref().is_some_and(|form| {
                        form.id == id && form.name.read(cx).value().as_str() == name
                    })
                {
                    self.git.branch_form = None;
                }
                if outcome.state == State::Succeeded
                    && let Some((key, summary, description)) = state.commit_draft.take()
                    && let Some(draft) = self.git_drafts.get_mut(&key)
                {
                    draft.clear_if_unchanged = Some((summary, description));
                }
                refresh |= matches!(outcome.state, State::Succeeded | State::NeedsResolution)
                    && self.server == state.server
                    && self.page == Page::Projects
                    && current_repo == state.repo;
                state.outcome = Some(outcome);
            }
            Reply::Outcome(Err(error)) => state.error = Some(error),
            Reply::Write(Err(error)) => {
                state.not_started = !error.dispatched;
                state.error = Some(error.message);
            }
        }
        if let Some(state) = &mut self.git_mutation {
            let result = state.outcome.as_ref().map(|o| &o.state);
            if needs_recovery(state.not_started, result, state.receipt) {
                state.recovery_attempt = state.recovery_attempt.saturating_add(1);
                state.recovery_at = Some(Instant::now() + recovery_delay(state.recovery_attempt));
            } else {
                state.recovery_at = None;
            }
            if result == Some(&State::Succeeded) {
                self.git_notice_clear = true;
            }
        }
        self.notify_git_failure(cx);
        if let Some((id, args)) = created_bookmark {
            self.save_created_project(id, args);
        }
        if let Some(path) = created_checkout {
            self.git_open_worktree_path(path, cx);
        } else if refresh && !current_repo.is_empty() {
            if let Some(editor) = self.code_workspace() {
                editor.update(cx, |e, _| e.navigator.invalidate());
            }
            if self.git.tab == 1
                && self
                    .git_mutation
                    .as_ref()
                    .is_some_and(|state| state.index_only)
            {
                // An index write cannot change an immutable commit document.
                self.resources.invalidate("git-tag-status");
                return true;
            }
            if self.git.tab == 0
                && self.git_mutation.as_ref().is_some_and(|state| {
                    state.index_only
                        && state
                            .outcome
                            .as_ref()
                            .is_some_and(|o| o.state == State::Succeeded)
                })
            {
                self.git_refresh_index();
                return true;
            }
            let form = self.git.pull_form.take();
            self.git_refresh();
            if let Some(mut form) = form {
                form.needs_review = true;
                let remote = form.remote().to_owned();
                self.git.selected = json!({"name":remote});
                self.request("git-remote-detail", "git_request", json!({"serverId":self.server,"request":{"method":"repo.remote","params":{"repoId":self.git_repository_id(),"name":remote}}}));
                self.git.pull_form = Some(form);
            }
        }
        if refresh_remote_refs {
            self.refresh_remote_refs_after_write();
        }
        true
    }

    pub(super) fn reshow_git_failure(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = &mut self.git_mutation {
            state.last_notice = None;
        }
        self.notify_git_failure(cx);
    }

    fn notify_git_failure(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &mut self.git_mutation else {
            return;
        };
        if state.pending.is_some() {
            return;
        }
        let result = state.outcome.as_ref().map(|o| &o.state);
        let error = state.error.as_deref().or_else(|| {
            state
                .outcome
                .as_ref()
                .and_then(|o| o.error.as_ref().map(|e| e.message.as_str()))
        });
        let Some((message, action)) =
            failure_notice(result, error, state.not_started, state.receipt)
        else {
            return;
        };
        let key = format!("{message}:{action:?}");
        if state.last_notice.as_ref() == Some(&key) {
            return;
        }
        state.last_notice = Some(key);
        let mut notification = Notification::error(message)
            .id::<GitNotice>()
            .title(format!("{} · {}", state.title, state.context));
        if let Some(reviewed) = action {
            let id = state.id;
            let owner = cx.weak_entity();
            notification = notification.action(move |_, _, cx| {
                let owner = owner.clone();
                crate::controls::button("recover-git-command")
                    .label(if reviewed {
                        "I’ve inspected the repository"
                    } else {
                        "Try again"
                    })
                    .when(!reviewed, |button| {
                        button.accessibility_label("Try the connection again")
                    })
                    .on_click(cx.listener(move |notice, _, window, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            if this.git_mutation.as_ref().is_some_and(|state| {
                                state.id == id
                                    && state.pending.is_none()
                                    && (!reviewed
                                        || state
                                            .outcome
                                            .as_ref()
                                            .is_some_and(|o| o.state == State::OutcomeUnknown))
                            }) {
                                if let Some(state) = &mut this.git_mutation {
                                    state.last_notice = None;
                                }
                                this.check_git_outcome(reviewed, cx);
                            }
                        });
                        notice.dismiss(window, cx);
                    }))
            });
        }
        self.notification = Some(notification);
    }
}

fn needs_recovery(
    not_started: bool,
    state: Option<&State>,
    receipt: Option<newport_gpui::git_operation::ReceiptState>,
) -> bool {
    !not_started
        && !confirmed_write_outcome(state, receipt)
        && !matches!(state, Some(State::OutcomeUnknown))
}

fn recovery_delay(attempt: u8) -> Duration {
    Duration::from_secs((1_u64 << attempt.min(5)).min(30))
}

fn retire_completed_receipt(
    backend: &backend::Backend,
    server: &str,
    id: uuid::Uuid,
    state: &State,
) {
    if matches!(
        state,
        State::Succeeded | State::Failed | State::ReviewedUnknown
    ) {
        // The backend validates dismissibility. Cleanup failure must never turn
        // a completed command into a failed command or cause it to be replayed.
        let _ = backend.call(
            "git_acknowledge_operation",
            json!({"serverId":server,"operationId":id}),
        );
    }
}

/// Whether the backend recorded this command as rejected before it ran; such
/// a receipt is retired here, as nothing about it needs recovery.
fn retire_rejected_receipt(backend: &backend::Backend, server: &str, id: uuid::Uuid) -> bool {
    let rejected = backend
        .call("git_pending_operations", json!({"serverId":server}))
        .and_then(|value| newport_gpui::git_operation::receipts(value, server))
        .is_ok_and(|receipts| {
            receipts.iter().any(|r| {
                r.operation_id == id
                    && r.state == newport_gpui::git_operation::ReceiptState::Rejected
            })
        });
    if rejected {
        retire_completed_receipt(backend, server, id, &State::Failed);
    }
    rejected
}

// Successful/running commands remain silent. Recovery actions only look up the
// existing command or acknowledge an explicit review; they never replay it.
fn failure_notice(
    state: Option<&State>,
    error: Option<&str>,
    not_started: bool,
    receipt: Option<newport_gpui::git_operation::ReceiptState>,
) -> Option<(String, Option<bool>)> {
    if state == Some(&State::OutcomeUnknown) {
        return Some(("The command was interrupted. Inspect the repository before continuing; it may have made changes.".into(), Some(true)));
    }
    if state == Some(&State::NeedsResolution) {
        return Some((
            error
                .unwrap_or("Resolve the repository conflicts to continue.")
                .into(),
            None,
        ));
    }
    error.map(|message| {
        (
            message.to_owned(),
            needs_recovery(not_started, state, receipt).then_some(false),
        )
    })
}

fn confirmed_write_outcome(
    state: Option<&State>,
    receipt: Option<newport_gpui::git_operation::ReceiptState>,
) -> bool {
    receipt == Some(newport_gpui::git_operation::ReceiptState::Rejected)
        || matches!(
            state,
            Some(
                State::Succeeded | State::Failed | State::ReviewedUnknown | State::NeedsResolution
            )
        )
}
#[cfg(test)]
mod mutation_tests {
    use super::{State, confirmed_write_outcome, failure_notice, needs_recovery, recovery_delay};
    #[test]
    fn routine_commands_are_silent_and_failures_have_no_outcome_controls() {
        for state in [
            None,
            Some(&State::Running),
            Some(&State::Succeeded),
            Some(&State::ReviewedUnknown),
        ] {
            assert_eq!(failure_notice(state, None, false, None), None);
        }
        assert_eq!(
            failure_notice(
                Some(&State::Failed),
                Some("Authentication failed"),
                false,
                None
            ),
            Some(("Authentication failed".into(), None))
        );
        assert_eq!(
            failure_notice(None, Some("Could not start"), true, None),
            Some(("Could not start".into(), None))
        );
        assert_eq!(
            failure_notice(None, Some("Disconnected"), false, None),
            Some(("Disconnected".into(), Some(false)))
        );
        assert_eq!(
            failure_notice(Some(&State::OutcomeUnknown), None, false, None)
                .unwrap()
                .1,
            Some(true)
        );
        assert_eq!(
            failure_notice(Some(&State::NeedsResolution), None, false, None)
                .unwrap()
                .1,
            None
        );
    }
    #[test]
    fn recovery_only_rechecks_unconfirmed_commands_with_bounded_backoff() {
        assert!(needs_recovery(false, None, None));
        assert!(needs_recovery(false, Some(&State::Running), None));
        assert!(!needs_recovery(true, None, None));
        assert!(!needs_recovery(
            false,
            None,
            Some(newport_gpui::git_operation::ReceiptState::Rejected)
        ));
        for state in [
            State::Succeeded,
            State::Failed,
            State::NeedsResolution,
            State::OutcomeUnknown,
            State::ReviewedUnknown,
        ] {
            assert!(!needs_recovery(false, Some(&state), None));
        }
        assert_eq!(recovery_delay(1).as_secs(), 2);
        assert_eq!(recovery_delay(2).as_secs(), 4);
        assert_eq!(recovery_delay(u8::MAX).as_secs(), 30);
    }
    #[test]
    fn rejection_is_confirmed_but_missing_or_running_outcome_is_not() {
        use newport_gpui::git_operation::ReceiptState;
        assert!(confirmed_write_outcome(None, Some(ReceiptState::Rejected)));
        assert!(!confirmed_write_outcome(None, None));
        assert!(!confirmed_write_outcome(Some(&State::Running), None));
        for state in [
            State::Succeeded,
            State::Failed,
            State::ReviewedUnknown,
            State::NeedsResolution,
        ] {
            assert!(confirmed_write_outcome(Some(&state), None));
        }
    }
}
