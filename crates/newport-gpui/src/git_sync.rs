//! One-click toolbar transfers with asynchronous, scope-bound preflight checks.
use super::*;
use gpui_kit::component::button::DropdownButton;
use newport_client::git_sync::{Kind, Target};

pub(super) struct Form {
    pub id: uuid::Uuid,
    kind: Kind,
    server: String,
    repo: String,
    target: Option<Target>,
    action: Option<Value>,
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
    fn accept_remote(&mut self, remote: &Value) {
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
            self.request("git-sync-status", "git_request", json!({"serverId":self.server,"request":{"method":"repo.status","params":{"repoId":self.git_repository_id(),"pageSize":1}}}));
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
        self.request("git-sync-review-status", "git_request", json!({"serverId":self.server,"request":{"method":"repo.status","params":{"repoId":self.git_repository_id(),"pageSize":1}}}));
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
            let remote = self.value(key);
            if let Some(form) = self.git.sync_form.as_mut() {
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
        match Target::from_status(&self.git.repo, &self.value(key)) {
            Ok(target) => {
                self.request("git-sync-remote", "git_request", json!({"serverId":self.server,"request":{"method":"repo.remote","params":{"repoId":self.git_repository_id(),"name":target.remote}}}));
                self.git.sync_form.as_mut().unwrap().target = Some(target);
            }
            Err(error) => self.git.sync_form.as_mut().unwrap().error = Some(error),
        }
    }
    fn git_sync_action(&self) -> Option<Value> {
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
        let target = Target::from_status(&self.git.repo, &self.value("git-sync-status"));
        let kind = target.as_ref().map_or(Kind::Fetch, Target::preferred);
        let remote = target
            .as_ref()
            .map_or("origin", |target| target.remote.as_str());
        let busy = self.git_mutation_blocks_write()
            || self.project_dialog_open
            || self.git.sync_form.is_some();
        let pending = reading.is_none_or(|r| r.pending && r.value.is_null());
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
    use serde_json::json;

    #[gpui_kit::test]
    fn cached_sync_control_stays_usable_during_observation_and_still_requires_review(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
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
                view.git.repo = json!({"repoId":"repo", "bare":false,
                    "capabilities":{"readOnly":false,"workingTree":true}});
                let reference = |name: &str| json!({"display":name,"bytesB64":STANDARD.encode(name)});
                let status = json!({"snapshot":"old", "entries":[], "nextCursor":null, "metadata":{
                    "head":{"detached":false,"unborn":false,"name":reference("refs/heads/main"),
                        "oid":{"hex":"a".repeat(40)}},
                    "upstream":{"remote":reference("origin"),"reference":reference("refs/heads/main")}
                }});
                let generation = view.resources.begin("git-sync-status").unwrap();
                view.resources.complete("git-sync-status", generation, Ok(status));
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
        assert_eq!(request.args["serverId"], "fixture");
        assert_eq!(request.args["request"]["params"]["repoId"], "repo");
        assert_eq!(request.args["request"]["method"], "repo.status");
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
        form.accept_remote(&json!({"name":"origin","token":"reviewed-config"}));
        assert!(form.action.is_some());
        let mut resources = Resources::default();
        let status = resources.begin("git-sync-review-status").unwrap();
        let remote = resources.begin("git-sync-remote").unwrap();
        form.invalidate(&mut resources);
        assert!(!form.matches("server-a", "repo-a"));
        assert!(form.action.is_none());
        assert!(!resources.complete("git-sync-review-status", status, Ok(json!({}))));
        assert!(!resources.complete("git-sync-remote", remote, Ok(json!({}))));
        form.accept_remote(&json!({"name":"origin","token":"late-config"}));
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
        form.accept_remote(&json!({"name":"origin","token":"reviewed-config"}));
        form.accept_remote(&json!({"name":"origin","token":"refreshed-config"}));
        assert_eq!(
            form.action.as_ref().unwrap()["expectedToken"],
            "reviewed-config"
        );
        form.submitted = true;
        assert!(!form.matches("server-a", "repo-a"));
    }

    #[test]
    fn invalid_remote_keeps_review_recoverable_without_a_write() {
        let mut form = review();
        form.accept_remote(&json!({"name":"different-remote","token":"token"}));
        assert!(form.error.is_some());
        assert!(form.action.is_none());
        assert!(!form.submitted);
        let mut retried = review();
        retried.accept_remote(&json!({"name":"origin","token":"fresh-config"}));
        assert!(retried.error.is_none());
        assert_eq!(retried.action.unwrap()["expectedToken"], "fresh-config");
    }
}
