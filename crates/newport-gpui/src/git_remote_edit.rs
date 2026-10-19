//! Remote editors capture configuration identity; selection cannot retarget a write.
use super::*;
pub(super) use newport_client::git_configuration::RemoteEdit as Kind;
use newport_client::git_configuration::{remote_action as action, reviewed_remote};
fn title(kind: Kind) -> &'static str {
    match kind {
        Kind::Add => "Add remote",
        Kind::Rename => "Rename remote",
        Kind::Url => "Change remote URL",
        Kind::Remove => "Remove remote",
    }
}
pub(super) struct Form {
    pub(super) needs_review: bool,
    id: uuid::Uuid,
    kind: Kind,
    remote: Value,
    snapshot: String,
    name: Entity<InputState>,
    url: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}
impl Form {
    pub(super) fn title(&self) -> &'static str {
        title(self.kind)
    }

    pub(super) fn draft(&self, cx: &App) -> (uuid::Uuid, String, String) {
        (
            self.id,
            self.name.read(cx).value().to_string(),
            self.url.read(cx).value().to_string(),
        )
    }
}
fn review_description(kind: Kind, remote: &Value) -> Div {
    div().min_w_0().flex().flex_col().gap_2()
        .child(div().id("remote-editor-title")
            .role(accesskit::Role::Heading).aria_label(title(kind))
            .font_weight(FontWeight::SEMIBOLD).child(title(kind)))
        .when(kind != Kind::Add, |view| view.child(project_dialog::description(
            "remote-editor-identity", format!("{} · {}", text(remote, "name"), text(remote, "url")))))
        .when(kind == Kind::Remove, |view| view.child(project_dialog::description(
            "remote-remove-description", "Removes this remote’s configuration and remote-tracking references. The remote repository itself is kept.")))
}
impl Newport {
    pub(super) fn can_edit_remote(&self) -> bool {
        self.git.tab == 6
            && self.git.settings.is_none()
            && !self.git.settings_open
            && !self.git_mutation_blocks_write()
            && self.git.repo["capabilities"]["readOnly"] == false
            && self.git.error.is_none()
            && ["git-list", "git-remote-status"].iter().all(|key| {
                self.resources
                    .get(key)
                    .is_some_and(|r| !r.pending && r.error.is_none())
            })
            && self.value("git-remote-status")["snapshot"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
    }
    pub(super) fn prepare_remote_edit(
        &mut self,
        kind: Kind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_edit_remote() || self.git.remote_edit.is_some() {
            return;
        }
        let remote = self.git_selected_remote().unwrap_or(Value::Null);
        if kind != Kind::Add && (!remote["name"].is_string() || !remote["token"].is_string()) {
            return;
        }
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Remote name"));
        let url =
            cx.new(|cx| InputState::new(window, cx).placeholder("Repository URL or server path"));
        match kind {
            Kind::Add | Kind::Rename => crate::controls::focus_editor(&name, window, cx),
            Kind::Url => crate::controls::focus_editor(&url, window, cx),
            Kind::Remove => {}
        }
        if kind == Kind::Rename {
            name.update(cx, |input, cx| {
                input.set_value(text(&remote, "name"), window, cx)
            });
        }
        // Display URLs may be redacted. Never submit the displayed URL as config.
        let subscriptions = vec![
            Self::project_form_input(
                &name,
                project_dialog::Kind::Remote,
                Self::submit_remote_edit,
                cx,
            ),
            Self::project_form_input(
                &url,
                project_dialog::Kind::Remote,
                Self::submit_remote_edit,
                cx,
            ),
        ];
        self.git.pull_form = None;
        self.git.remote_edit = Some(Form {
            needs_review: false,
            id: uuid::Uuid::new_v4(),
            kind,
            remote,
            snapshot: text(&self.value("git-remote-status"), "snapshot"),
            name,
            url,
            _subscriptions: subscriptions,
        });
        self.git_load_remote_review();
        self.git.detail_open = true;
        cx.notify();
    }
    fn submit_remote_edit(&mut self, cx: &mut Context<Self>) {
        if !self.can_edit_remote() {
            return;
        }
        let Some(form) = &self.git.remote_edit else {
            return;
        };
        if form.needs_review {
            return;
        }
        let draft = form.draft(cx);
        let Some(action) = action(form.kind, &form.remote, &draft.1, &draft.2) else {
            return;
        };
        let snapshot = form.snapshot.clone();
        let title = format!("{} {}", title(form.kind), text(&action, "name"));
        self.start_git_mutation(
            self.git_repository_id(),
            title,
            move |backend, server, repo, id| {
                backend.apply_reviewed_action(server, repo, &snapshot, action, id)
            },
            cx,
        );
        if let Some(state) = self.git_mutation.as_mut() {
            state.remote_draft = Some(draft);
        }
    }
    fn review_remote_edit(&mut self, cx: &mut Context<Self>) {
        if !self.can_edit_remote() {
            return;
        }
        let snapshot = text(&self.value("git-remote-status"), "snapshot");
        let detail = self.git_reviewed_remote();
        let Some(form) = self.git.remote_edit.as_mut() else {
            return;
        };
        let Some(remote) = reviewed_remote(form.kind, &form.remote, &detail) else {
            return;
        };
        form.remote = remote;
        form.snapshot = snapshot;
        form.needs_review = false;
        cx.notify();
    }
    pub(super) fn git_remote_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let enabled = self.can_edit_remote()
            && self.git.remote_edit.is_none()
            && self.git.pull_form.is_none();
        let selected = enabled && self.git_selected_remote().is_some();
        let actions = crate::controls::button("remote-config-actions")
            .label("Remote actions")
            .disabled(!selected)
            .dropdown_menu({
                let weak = cx.entity().downgrade();
                move |mut menu, _, _| {
                    for kind in [Kind::Rename, Kind::Url, Kind::Remove] {
                        let owner = weak.clone();
                        menu = menu.item(PopupMenuItem::new(format!("{}…", title(kind))).on_click(
                            move |_, window, cx| {
                                let _ = owner.update(cx, |this, cx| {
                                    this.prepare_remote_edit(kind, window, cx)
                                });
                            },
                        ));
                    }
                    menu
                }
            });
        div()
            .flex()
            .flex_wrap()
            .gap_2()
            .child(self.git_transfer_controls(cx))
            .child(actions)
            .child(self.remote_refs_button(cx))
            .into_any_element()
    }
    pub(super) fn git_remote_edit_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut view = div().flex().flex_col().gap_2().px_4().py_2();
        if let Some(form) = &self.git.remote_edit {
            let draft = form.draft(cx);
            let busy = self.git_mutation_blocks_write();
            let latest = reviewed_remote(form.kind, &form.remote, &self.git_reviewed_remote());
            view = view.child(review_description(form.kind, &form.remote))
                .when(form.needs_review, |view| view.child(project_dialog::description("remote-review-required", "Your draft was kept. Review the latest configuration before submitting again."))
                    .when(self.can_edit_remote(), |view| view.child(project_dialog::description("remote-review-current", match &latest {
                        Some(remote) if form.kind != Kind::Add => format!("Current URL: {}", text(remote,"url")),
                        Some(_) => "The repository state has been refreshed.".into(),
                        None => "Current configuration is not available. Refresh to check again.".into(),
                    })))
                    .child(div().flex().flex_wrap().gap_2()
                        .child(crate::controls::button("refresh-remote-review").label("Refresh configuration").disabled(busy).on_click(cx.listener(|this,_,_,cx|{this.git_refresh();cx.notify();})))
                        .child(crate::controls::button("accept-remote-review").label("Use reviewed configuration").disabled(!self.can_edit_remote() || latest.is_none()).on_click(cx.listener(|this,_,_,cx|this.review_remote_edit(cx))))))
                .when(matches!(form.kind,Kind::Add|Kind::Rename),|view|view.child(crate::label("Remote name")).child(crate::controls::input(&form.name).aria_label("Remote name").disabled(busy)))
                .when(matches!(form.kind,Kind::Add|Kind::Url),|view|view.child(crate::label("Remote URL")).child(crate::controls::input(&form.url).aria_label("Repository URL or server path").disabled(busy)))
                .child(project_dialog::actions()
                    .child(crate::controls::button("cancel-git-remote").label("Cancel").disabled(busy).on_click(cx.listener(|this,_,_,cx|{this.git.remote_edit=None;cx.notify();})))
                    .child(crate::controls::button("save-git-remote").loading(self.git_write_pending()).when(form.kind == Kind::Remove, |button| button.danger()).label(title(form.kind)).disabled(form.needs_review || !self.can_edit_remote() || action(form.kind,&form.remote,&draft.1,&draft.2).is_none()).on_click(cx.listener(|this,_,_,cx|this.submit_remote_edit(cx)))));
        }
        view.into_any_element()
    }
}

impl Newport {
    pub(super) fn git_load_remote_review(&mut self) {
        let Some(form) = &self.git.remote_edit else {
            return;
        };
        if form.kind == Kind::Add {
            return;
        }
        self.request("git-remote-review", "git_request", json!({"serverId":self.server,"request":{"method":"repo.remote","params":{"repoId":self.git_repository_id(),"name":form.remote["name"]}}}));
    }
    fn git_reviewed_remote(&self) -> Value {
        self.resources
            .get("git-remote-review")
            .filter(|r| !r.pending && r.error.is_none())
            .map(|r| r.value.clone())
            .unwrap_or(Value::Null)
    }
}

impl Form {
    pub(super) fn initial_focus(&self, cx: &App) -> Option<FocusHandle> {
        match self.kind {
            Kind::Add | Kind::Rename => Some(self.name.read(cx).focus_handle(cx)),
            Kind::Url => Some(self.url.read(cx).focus_handle(cx)),
            Kind::Remove => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Kind, review_description};
    use gpui_kit::base::test_support::snapshots;
    use gpui_kit::{self as gpui, Context, IntoElement, Render, Window, div, prelude::*, px};
    use serde_json::{Value, json};

    struct ReviewHost(Value);
    impl Render for ReviewHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w_full()
                .flex()
                .flex_col()
                .child(review_description(Kind::Remove, &self.0))
                .child(
                    div()
                        .debug_selector(|| "remote-review-footer".into())
                        .h(px(28.))
                        .child("Cancel"),
                )
        }
    }
    #[gpui::test]
    fn remote_removal_exposes_target_and_consequence_at_narrow_widths(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui::init(cx);
            crate::theme::install(false, cx);
        });
        let url = format!(
            "https://git.example.test/{}/repository.git",
            "a".repeat(180)
        );
        let name = "upstream-开发";
        let (_, visual) = cx.add_window_view(|_, _| ReviewHost(json!({"name":name,"url":url})));
        for width in [520., 280.] {
            visual.simulate_resize(gpui::size(px(width), px(600.)));
            visual.update(|window, cx| window.draw(cx).clear(cx));
            let (identity, consequence) = visual.update(|window, _| {
                let nodes = snapshots(window);
                let expected = format!("{name} · {url}");
                let identity = nodes
                    .iter()
                    .find(|n| n.label() == Some(expected.as_str()))
                    .unwrap();
                let consequence = nodes
                    .iter()
                    .find(|n| {
                        n.label()
                            .is_some_and(|s| s.ends_with("The remote repository itself is kept."))
                    })
                    .unwrap();
                assert_eq!(identity.role(), Some(gpui::Role::Label));
                assert_eq!(consequence.role(), Some(gpui::Role::Label));
                (identity.bounds(), consequence.bounds())
            });
            assert!(identity.size.height > px(18.));
            assert!(identity.right() <= px(width));
            assert!(consequence.top() >= identity.bottom());
            assert!(consequence.right() <= px(width));
            assert!(
                visual.debug_bounds("remote-review-footer").unwrap().top() >= consequence.bottom()
            );
        }
    }
}
