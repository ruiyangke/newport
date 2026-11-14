//! Explicit remote transfers, with reviewed configuration and status guards.
use super::*;
use base64::Engine as _;
use newport_protocol::git::{Head, Remote, Reply, Request as GitRequest};
pub(super) struct Form {
    pub id: uuid::Uuid,
    pub needs_review: bool,
    branch: Entity<InputState>,
    remote: String,
    token: String,
    snapshot: String,
    push: Option<(String, String)>,
    _subscription: Subscription,
}
impl Form {
    pub(super) fn title(&self) -> &'static str {
        if self.push.is_some() {
            "Push branch"
        } else {
            "Pull branch"
        }
    }
    pub(super) fn initial_focus(&self, cx: &App) -> FocusHandle {
        self.branch.read(cx).focus_handle(cx)
    }

    pub fn draft(&self, cx: &Context<Newport>) -> (uuid::Uuid, String) {
        (self.id, self.branch.read(cx).value().to_string())
    }
    pub fn remote(&self) -> &str {
        &self.remote
    }
}
/// The local branch and commit a push sends: a named, attached HEAD whose
/// name is lossless.
pub(super) fn push_source(head: Option<&Head>) -> Option<(String, String)> {
    let head = head.filter(|head| !head.detached)?;
    let name = head.name.as_ref()?;
    if name.bytes_b64 != base64::engine::general_purpose::STANDARD.encode(&name.display) {
        return None;
    }
    let branch = name.display.strip_prefix("refs/heads/")?;
    let oid = &head.oid.as_ref()?.hex;
    (!branch.is_empty() && !oid.is_empty()).then(|| (branch.to_owned(), oid.to_owned()))
}
impl Newport {
    pub(super) fn git_select_remote(&mut self) {
        self.resources.invalidate("git-remote-detail");
        self.git.pull_form = None;
        let Some(name) = self.git.remote_name().map(str::to_owned) else {
            return;
        };
        self.request_git_typed(
            "git-remote-detail",
            GitRequest::Remote {
                repo_id: self.git_repository_id(),
                name,
            },
        );
    }
    /// The remote configuration the last detail read returned.
    pub(super) fn git_remote_detail(&self) -> Option<&Remote> {
        match self.git_reply("git-remote-detail") {
            Some(Reply::Remote(remote)) => Some(remote),
            _ => None,
        }
    }
    fn can_prepare_pull(&self) -> bool {
        let status = self.git_status("git-remote-status");
        let remote = self.git_remote_detail();
        self.git.tab == 6
            && self.git.settings.is_none()
            && !self.git.settings_open
            && self.git.remote_edit.is_none()
            && !self.git_mutation_blocks_write()
            && self.git.writable()
            && self.git.has_working_tree()
            && self.git.bare() == Some(false)
            && ["git-remote-status", "git-remote-detail"]
                .iter()
                .all(|key| {
                    self.resources
                        .get(key)
                        .is_some_and(|r| !r.pending && r.error.is_none())
                })
            && status.is_some_and(|status| {
                !status.snapshot.is_empty()
                    && status.metadata.integration.is_none()
                    && !status.metadata.head.detached
            })
            && remote.is_some_and(|remote| {
                Some(remote.name.as_str()) == self.git.remote_name() && !remote.token.is_empty()
            })
    }
    fn prepare_pull(&mut self, push: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_prepare_pull() {
            return;
        }
        let (Some(status), Some(remote)) = (
            self.git_status("git-remote-status").cloned(),
            self.git_remote_detail().cloned(),
        ) else {
            return;
        };
        let source = push_source(Some(&status.metadata.head));
        if push && source.is_none() {
            return;
        }
        let name = status
            .metadata
            .head
            .name
            .as_ref()
            .map_or_else(|| "—".into(), |name| name.display.clone());
        let branch = cx.new(|cx| InputState::new(window, cx).placeholder("Remote branch"));
        branch.update(cx, |input, cx| {
            input.set_value(
                name.strip_prefix("refs/heads/").unwrap_or(&name).to_owned(),
                window,
                cx,
            )
        });
        let subscription = Self::project_form_input(
            &branch,
            project_dialog::Kind::Transfer,
            Self::pull_fast_forward,
            cx,
        );
        self.git.pull_form = Some(Form {
            id: uuid::Uuid::new_v4(),
            needs_review: false,
            branch,
            remote: remote.name,
            token: remote.token,
            snapshot: status.snapshot,
            push: if push { source } else { None },
            _subscription: subscription,
        });
        cx.notify();
    }
    fn pull_fast_forward(&mut self, cx: &mut Context<Self>) {
        if !self.can_prepare_pull() {
            return;
        }
        let Some(form) = &self.git.pull_form else {
            return;
        };
        if form.needs_review {
            return;
        }
        let draft = form.draft(cx);
        let branch = form.branch.read(cx).value().trim().to_owned();
        if branch.is_empty() {
            return;
        }
        let remote = form.remote.clone();
        let token = form.token.clone();
        let snapshot = form.snapshot.clone();
        let source = form.push.clone();
        self.start_git_mutation(
            self.git_repository_id(),
            format!(
                "{} {remote}/{branch}",
                if source.is_some() { "Push" } else { "Pull" }
            ),
            move |backend, server, repo, id| {
                if let Some((local, oid)) = source {
                    backend.push(
                        server,
                        repo,
                        &snapshot,
                        backend::PushTarget {
                            remote: &remote,
                            token: &token,
                            branch: &local,
                            oid: &oid,
                            destination: &branch,
                        },
                        id,
                    )
                } else {
                    backend.pull_fast_forward(
                        server,
                        repo,
                        &snapshot,
                        (&remote, &token, &branch),
                        id,
                    )
                }
            },
            cx,
        );
        if let Some(mutation) = self.git_mutation.as_mut() {
            mutation.transfer_draft = Some(draft);
        }
    }
    fn review_transfer(&mut self, cx: &mut Context<Self>) {
        if !self.can_prepare_pull() {
            return;
        }
        let (Some(status), Some(remote)) = (
            self.git_status("git-remote-status").cloned(),
            self.git_remote_detail().cloned(),
        ) else {
            return;
        };
        let Some(form) = self.git.pull_form.as_mut() else {
            return;
        };
        if form.push.is_some() {
            let Some(source) = push_source(Some(&status.metadata.head)) else {
                return;
            };
            form.push = Some(source);
        }
        form.snapshot = status.snapshot;
        form.token = remote.token;
        form.needs_review = false;
        cx.notify();
    }
    pub(super) fn git_transfer_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let reviewing = self.git.remote_edit.is_some()
            || self.git.pull_form.is_some()
            || self.git.settings.is_some()
            || self.git.settings_open;
        div()
            .flex()
            .flex_wrap()
            .gap_2()
            .child(
                crate::controls::button("git-fetch-remote")
                    .label("Fetch")
                    .disabled(
                        reviewing
                            || self.git_selected_remote().is_none()
                            || self.git_mutation_blocks_write()
                            || !self.git.writable(),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(remote) = this.git.remote_name().map(str::to_owned) {
                            this.git_fetch(this.git_repository_id(), remote, cx);
                        }
                    })),
            )
            .child(
                crate::controls::button("prepare-pull")
                    .label("Pull…")
                    .disabled(!self.can_prepare_pull() || reviewing)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.prepare_pull(false, window, cx)),
                    ),
            )
            .child(
                crate::controls::button("prepare-push")
                    .label("Push…")
                    .disabled(
                        !self.can_prepare_pull()
                            || reviewing
                            || push_source(
                                self.git_status("git-remote-status")
                                    .map(|status| &status.metadata.head),
                            )
                            .is_none(),
                    )
                    .on_click(
                        cx.listener(|this, _, window, cx| this.prepare_pull(true, window, cx)),
                    ),
            )
            .into_any_element()
    }
    pub(super) fn git_pull_view(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .px_4()
            .when(self.git.pull_form.is_some(), |view|view.py_2())
            .child(self.status("git-remote-status", cx))

            .when_some(self.git.pull_form.as_ref(), |view, form| {
                view.when(form.needs_review,|view|view.child(crate::label("Your draft was kept. Review the updated repository state before submitting again.").text_size(px(crate::ui::FONT_SIZE)))
                    .child(crate::controls::button("review-transfer").label("Review updated state").disabled(!self.can_prepare_pull()).on_click(cx.listener(|this,_,_,cx|this.review_transfer(cx)))))
                .child(div().font_weight(FontWeight::SEMIBOLD).child(form.title()))
                .child(
                    crate::label(if let Some((branch,oid)) = &form.push { format!("Push {branch} · {} to {}. Remote history will not be overwritten.",oid.chars().take(8).collect::<String>(),form.remote) } else {format!(
                        "Fast-forward current branch from {}. Diverged history will be refused.",
                        form.remote
                    )})
                    .text_size(px(crate::ui::FONT_SIZE)),
                )
                .child(crate::label(if form.push.is_some() {"Destination branch"} else {"Remote branch"}))
                .child(
                            div().w_full().min_w_0().child(
                                crate::controls::input(&form.branch)
                                    .aria_label(if form.push.is_some() {"Destination branch to push"} else {"Remote branch to pull"})
                                    .disabled(self.git_mutation_blocks_write()),
                            ),
                        )
    .child(project_dialog::actions()
    .child(crate::controls::button("cancel-pull").label("Cancel").disabled(self.git_write_pending()).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.git.pull_form = None;
                                cx.notify();
                            }),
                        ))
    .child(
                            crate::controls::button("pull-fast-forward").primary().loading(self.git_write_pending())
                                .label(if form.push.is_some() {"Push branch"} else {"Pull fast-forward"})
                                .disabled(
                                    !self.can_prepare_pull()
                                        || form.needs_review
                                        || form.branch.read(cx).value().trim().is_empty(),
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.pull_fast_forward(cx))),
                        ))
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::push_source;
    #[test]
    fn push_requires_attached_byte_safe_branch_and_tip() {
        let mut head = crate::git_fixture::repository("repo").head;
        head.oid = Some(crate::git_fixture::object("abc"));
        assert_eq!(
            push_source(Some(&head)),
            Some(("main".into(), "abc".into()))
        );
        head.detached = true;
        assert!(push_source(Some(&head)).is_none());
        head.detached = false;
        head.oid = None;
        assert!(push_source(Some(&head)).is_none());
        head.oid = Some(crate::git_fixture::object("abc"));
        head.name.as_mut().unwrap().bytes_b64 = "/w==".into();
        assert!(push_source(Some(&head)).is_none());
        assert!(push_source(None).is_none());
    }
}
