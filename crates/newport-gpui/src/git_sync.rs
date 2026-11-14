//! One-click toolbar transfers with asynchronous, scope-bound preflight checks.
use super::*;
use gpui_kit::component::button::DropdownButton;
use newport_client::git_sync::{Kind, Target};
use newport_protocol::git::{Action, Request};

pub(super) struct Form {
    pub id: uuid::Uuid,
    kind: Kind,
    server: String,
    repo: String,
    target: Option<Target>,
    action: Option<Action>,
    error: Option<String>,
    submitted: bool,
    needs_review: bool,
}

impl Form {
    fn new(kind: Kind, server: String, repo: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            kind,
            server,
            repo,
            target: None,
            action: None,
            error: None,
            submitted: false,
            needs_review: false,
        }
    }
    fn matches(&self, server: &str, repo: &str) -> bool {
        self.server == server && self.repo == repo && !self.needs_review && !self.submitted
    }
    pub(super) fn invalidate(&mut self, resources: &mut Resources) {
        self.needs_review = true;
        self.action = None;
        // Removing these generations also rejects responses already in flight.
        resources.invalidate("git-sync-review-status");
        resources.invalidate("git-sync-remote");
    }
    fn accept_remote(&mut self, remote: &newport_protocol::git::Remote) {
        if self.needs_review || self.submitted || self.action.is_some() {
            return;
        }
        if let Some(target) = &self.target {
            match target.action(self.kind, remote) {
                Ok(action) => {
                    self.action = Some(action);
                    self.error = None;
                }
                Err(error) => self.error = Some(error),
            }
        }
    }
}
impl Newport {
    pub(super) fn sync_git_transfer_status(&mut self) {
        if !self.git_repository_id().is_empty() && self.resources.get("git-sync-status").is_none() {
            self.request_git_typed(
                "git-sync-status",
                Request::Status {
                    repo_id: self.git_repository_id(),
                    page_size: 1,
                    filter: None,
                    cursor: None,
                },
            );
        }
    }
    fn begin_git_sync(&mut self, kind: Kind, cx: &mut Context<Self>) {
        if self.git_mutation_blocks_write()
            || self.git.sync_form.is_some()
            || self.page != Page::Projects
            || self.git_repository_id().is_empty()
        {
            return;
        }
        for key in ["git-sync-review-status", "git-sync-remote"] {
            self.resources.invalidate(key);
        }
        self.git.sync_form = Some(Form::new(
            kind,
            self.server.clone(),
            self.git_repository_id(),
        ));
        self.request_git_typed(
            "git-sync-review-status",
            Request::Status {
                repo_id: self.git_repository_id(),
                page_size: 1,
                filter: None,
                cursor: None,
            },
        );
        cx.notify();
    }
    pub(super) fn git_sync_completed(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.page != Page::Projects
            || !self
                .git
                .sync_form
                .as_ref()
                .is_some_and(|form| form.matches(&self.server, &self.git_repository_id()))
            || self
                .resources
                .get(key)
                .is_none_or(|r| r.pending || r.error.is_some())
        {
            return;
        }
        if key == "git-sync-remote" {
            if let Some(newport_protocol::git::Reply::Remote(remote)) = self.git_reply(key).cloned()
                && let Some(form) = self.git.sync_form.as_mut()
            {
                form.accept_remote(&remote);
            }
            if self.git_mutation_blocks_write() {
                self.git.sync_form.as_mut().unwrap().error = Some(
                    "Another Git operation is in progress. Try again when it finishes.".into(),
                );
            } else {
                self.submit_git_sync(cx);
            }
            return;
        }
        if key != "git-sync-review-status"
            || self
                .git
                .sync_form
                .as_ref()
                .is_some_and(|form| form.target.is_some())
        {
            return;
        }
        let Some(status) = self.git_status(key) else {
            return;
        };
        match Target::from_status(self.git.repo.as_ref(), status) {
            Ok(target) => {
                self.request_git_typed(
                    "git-sync-remote",
                    Request::Remote {
                        repo_id: self.git_repository_id(),
                        name: target.remote.clone(),
                    },
                );
                self.git.sync_form.as_mut().unwrap().target = Some(target);
            }
            Err(error) => self.git.sync_form.as_mut().unwrap().error = Some(error),
        }
    }
    fn git_sync_action(&self) -> Option<Action> {
        let form = self.git.sync_form.as_ref()?;
        if self.page != Page::Projects
            || !form.matches(&self.server, &self.git_repository_id())
            || self.git_mutation_blocks_write()
            || ["git-sync-review-status", "git-sync-remote"]
                .iter()
                .any(|key| {
                    self.resources
                        .get(key)
                        .is_none_or(|r| r.pending || r.error.is_some())
                })
        {
            return None;
        }
        form.action.clone()
    }
    fn submit_git_sync(&mut self, cx: &mut Context<Self>) {
        let Some(action) = self.git_sync_action() else {
            return;
        };
        let form = self.git.sync_form.as_ref().unwrap();
        let id = form.id;
        let target = form.target.as_ref().unwrap().clone();
        let title = format!("{} {}", form.kind.label(), target.remote);
        self.start_git_mutation(
            self.git_repository_id(),
            title,
            move |backend, server, repo, operation| {
                backend.apply_reviewed_action(server, repo, &target.snapshot, action, operation)
            },
            cx,
        );
        self.git.sync_form.as_mut().unwrap().submitted = true;
        if let Some(mutation) = self.git_mutation.as_mut() {
            mutation.sync_draft = Some(id);
        }
        // The operation owns recovery from here; never replay a dispatched write.
        self.git.sync_form = None;
    }
    pub(super) fn git_sync_control(&self, cx: &mut Context<Self>) -> AnyElement {
        let reading = self.resources.get("git-sync-status");
        let target = match self.git_status("git-sync-status") {
            Some(status) => Target::from_status(self.git.repo.as_ref(), status),
            None => Err("Refresh repository status first.".into()),
        };
        let kind = target.as_ref().map_or(Kind::Fetch, Target::preferred);
        let remote = target
            .as_ref()
            .map_or("origin", |target| target.remote.as_str());
        let busy = self.git_mutation_blocks_write()
            || self.project_dialog_open
            || self.git.sync_form.is_some();
        let pending = reading.is_none_or(|r| r.pending && !r.has_value());
        let error = reading.and_then(|r| r.error.clone());
        // Starting a transfer obtains a fresh review status and remote config.
        // A background observation need not disable the cached entry point.
        let disabled = busy
            || reading.is_none_or(|r| r.display_pending())
            || error.is_some()
            || target.is_err();
        let tooltip = error
            .or_else(|| target.as_ref().err().cloned())
            .unwrap_or_else(|| {
                let target = target.as_ref().unwrap();
                format!(
                    "Compared with {}/{} as last fetched; the remote has not been contacted.",
                    target.remote, target.destination
                )
            });
        let count = target.as_ref().map_or(0, |target| {
            if kind == Kind::Pull {
                target.behind
            } else if kind == Kind::Push {
                target.ahead
            } else {
                0
            }
        });
        if pending {
            return div()
                .w(px(148.))
                .flex_shrink_0()
                .child(project_loading::selector("upstream transfer"))
                .into_any_element();
        }
        DropdownButton::new("git-sync-control")
            .ghost()
            .flex_shrink_0()
            .disabled(busy)
            .button(
                crate::controls::button("git-sync-primary")
                    .max_w(px(200.))
                    .label(format!("{} {remote}", kind.label()))
                    .icon(match kind {
                        Kind::Fetch => IconName::Download,
                        Kind::Pull => IconName::ArrowDown,
                        Kind::Push => IconName::ArrowUp,
                    })
                    .hover_hint(tooltip)
                    .loading(
                        self.git_write_pending()
                            || self.git.sync_form.as_ref().is_some_and(|form| {
                                form.error.is_none()
                                    && !form.needs_review
                                    && !["git-sync-review-status", "git-sync-remote"].iter().any(
                                        |key| {
                                            self.resources
                                                .get(key)
                                                .is_some_and(|r| r.error.is_some())
                                        },
                                    )
                            }),
                    )
                    .disabled(disabled)
                    .when(count > 0, |button| {
                        button.child(
                            crate::label(count.to_string()).text_size(px(crate::ui::FONT_SIZE)),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.begin_git_sync(kind, cx))),
            )
            .menu_trigger(
                crate::controls::icon_button("git-sync-options")
                    .icon(IconName::ChevronDown)
                    .accessibility_label("Transfer options")
                    .tooltip("Transfer options")
                    .disabled(busy),
            )
            .dropdown_menu({
                let owner = cx.weak_entity();
                move |mut menu, _, _| {
                    for kind in [Kind::Fetch, Kind::Pull, Kind::Push] {
                        let owner = owner.clone();
                        menu = menu.item(
                            PopupMenuItem::new(format!("{} upstream", kind.label()))
                                .disabled(disabled)
                                .on_click(move |_, _, cx| {
                                    let _ =
                                        owner.update(cx, |this, cx| this.begin_git_sync(kind, cx));
                                }),
                        );
                    }
                    let refresh = owner.clone();
                    let remotes = owner.clone();
                    menu.separator()
                        .item(PopupMenuItem::new("Refresh sync status").on_click(
                            move |_, _, cx| {
                                let _ = refresh.update(cx, |this, cx| {
                                    this.resources.invalidate("git-sync-status");
                                    this.sync_git_transfer_status();
                                    cx.notify();
                                });
                            },
                        ))
                        .item(
                            PopupMenuItem::new("Manage remotes").on_click(move |_, _, cx| {
                                let _ = remotes.update(cx, |this, cx| this.git_navigate(6, cx));
                            }),
                        )
                }
            })
            .into_any_element()
    }
    pub(super) fn git_sync_feedback(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.git.sync_form else {
            return div().into_any_element();
        };
        let error = form
            .error
            .clone()
            .or_else(|| {
                ["git-sync-review-status", "git-sync-remote"]
                    .iter()
                    .find_map(|key| self.resources.get(key).and_then(|r| r.error.clone()))
            })
            .or_else(|| {
                form.needs_review
                    .then(|| "Repository changed before the transfer started. Try again.".into())
            });
        let Some(error) = error else {
            return div().into_any_element();
        };
        let kind = form.kind;
        div()
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .child(crate::label(error).flex_1())
            .child(
                crate::controls::button("retry-git-sync")
                    .label("Try again")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.git.sync_form = None;
                        this.begin_git_sync(kind, cx);
                    })),
            )
            .child(
                crate::controls::icon_button("dismiss-git-sync")
                    .icon(IconName::X)
                    .accessibility_label("Dismiss transfer error")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.git.sync_form = None;
                        for key in ["git-sync-review-status", "git-sync-remote"] {
                            this.resources.invalidate(key);
                        }
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Form, Kind, Target};
    use newport_client::resources::Resources;
    use newport_protocol::git::{Remote, Reply};

    fn remote(name: &str, token: &str) -> Remote {
        Remote {
            name: name.into(),
            url: None,
            push_url: None,
            token: token.into(),
        }
    }

    #[gpui_kit::test]
    fn cached_sync_control_stays_usable_during_observation_and_still_requires_review(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::{AppContext, Context, Entity, IntoElement, Render, Window};
        struct Host(Entity<crate::Newport>);
        impl Render for Host {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                self.0.update(cx, |view, cx| view.git_sync_control(cx))
            }
        }
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (work, requests) = std::sync::mpsc::sync_channel(8);
        let mut owner = None;
        let (_, visual) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                let mut view = crate::page_perf_tests::owner(window, cx);
                view.work = work.into();
                view.page = crate::Page::Projects;
                view.server = "00000000-0000-4000-8000-000000000001".into();
                view.git.repo = Some(crate::git_fixture::repository("repo"));
                let mut status = crate::git_fixture::status_page("old", vec![], None);
                status.metadata.upstream = Some(newport_protocol::git::Upstream {
                    remote: newport_protocol::git::Path::new(b"origin"),
                    reference: newport_protocol::git::Path::new(b"refs/heads/main"),
                });
                let generation = view.resources.begin("git-sync-status").unwrap();
                view.resources.complete_git(
                    "git-sync-status",
                    generation,
                    Ok(Reply::Status(Box::new(status))),
                );
                view.resources.begin_refresh("git-sync-status", false);
                view
            });
            owner = Some(view.clone());
            Host(view)
        });
        let view = owner.unwrap();
        crate::page_perf_tests::frame(visual);
        let button = visual.update(|window, _| {
            gpui_kit::base::test_support::snapshots(window)
                .into_iter()
                .find(|node| node.label() == Some("Fetch origin"))
                .unwrap()
        });
        assert_ne!(button.disabled(), Some(true));
        visual.simulate_click(button.bounds().center(), Default::default());
        crate::page_perf_tests::frame(visual);
        let request = requests.try_recv().expect("fresh status review");
        assert_eq!(request.key, "git-sync-review-status");
        let crate::request_dispatch::Request::Git(args) = request.request else {
            panic!("expected a Git request");
        };
        assert_eq!(
            args.server.to_string(),
            "00000000-0000-4000-8000-000000000001"
        );
        assert!(
            matches!(args.request, newport_protocol::git::Request::Status {
            ref repo_id, page_size: 1, filter: None, cursor: None,
        } if repo_id == "repo")
        );
        assert!(
            requests.try_recv().is_err(),
            "cached status cannot authorize a write"
        );
        assert!(view.read_with(visual, |view, _| {
            view.git.sync_form.as_ref().unwrap().target.is_none()
        }));
    }

    fn review() -> Form {
        let mut form = Form::new(Kind::Push, "server-a".into(), "repo-a".into());
        form.target = Some(Target {
            snapshot: "reviewed-snapshot".into(),
            remote: "origin".into(),
            destination: "main".into(),
            branch: "topic".into(),
            oid: "a".repeat(40),
            ahead: 1,
            behind: 0,
        });
        form
    }

    #[test]
    fn refresh_revokes_confirmation_and_rejects_inflight_review_reads() {
        let mut form = review();
        form.accept_remote(&remote("origin", "reviewed-config"));
        assert!(form.action.is_some());
        let mut resources = Resources::default();
        let status = resources.begin("git-sync-review-status").unwrap();
        let config = resources.begin("git-sync-remote").unwrap();
        form.invalidate(&mut resources);
        assert!(!form.matches("server-a", "repo-a"));
        assert!(form.action.is_none());
        let page = crate::git_fixture::status_page("late", vec![], None);
        assert!(!resources.complete_git(
            "git-sync-review-status",
            status,
            Ok(Reply::Status(Box::new(page)))
        ));
        assert!(!resources.complete_git(
            "git-sync-remote",
            config,
            Ok(Reply::Remote(Box::new(remote("origin", "late-config"))))
        ));
        form.accept_remote(&remote("origin", "late-config"));
        assert!(form.action.is_none());
        // Reopening gets a new identity; an old operation cannot close it.
        assert_ne!(form.id, review().id);
    }

    #[test]
    fn reviewed_transfer_pins_config_and_cannot_cross_scope_or_replay() {
        let mut form = review();
        assert!(form.matches("server-a", "repo-a"));
        assert!(!form.matches("server-b", "repo-a"));
        assert!(!form.matches("server-a", "repo-b"));
        form.accept_remote(&remote("origin", "reviewed-config"));
        form.accept_remote(&remote("origin", "refreshed-config"));
        assert_eq!(
            serde_json::to_value(form.action.as_ref().unwrap()).unwrap()["expectedToken"],
            "reviewed-config"
        );
        form.submitted = true;
        assert!(!form.matches("server-a", "repo-a"));
    }

    #[test]
    fn invalid_remote_keeps_review_recoverable_without_a_write() {
        let mut form = review();
        form.accept_remote(&remote("different-remote", "token"));
        assert!(form.error.is_some());
        assert!(form.action.is_none());
        assert!(!form.submitted);
        let mut retried = review();
        retried.accept_remote(&remote("origin", "fresh-config"));
        assert!(retried.error.is_none());
        assert_eq!(
            serde_json::to_value(retried.action.unwrap()).unwrap()["expectedToken"],
            "fresh-config"
        );
    }
}
