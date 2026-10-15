//! Reviewed branch integration and recovery, using the agent's managed-operation flags.
use super::*;
use base64::Engine as _;
use gpui_kit::component::input::TextareaState;
use newport_client::git_conflict::{recovery_allowed, recovery_supported};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReviewOrigin {
    Changes,
    History,
    BranchPicker,
}
impl ReviewOrigin {
    fn workspace(tab: usize) -> Option<Self> {
        match tab {
            0 => Some(Self::Changes),
            1 => Some(Self::History),
            _ => None,
        }
    }
    fn resource_key(self) -> &'static str {
        match self {
            Self::Changes => "git-list",
            Self::History => "git-tag-status",
            Self::BranchPicker => "git-branch-status",
        }
    }
    fn active(self, tab: usize, picker_open: bool) -> bool {
        match self {
            Self::BranchPicker => picker_open,
            _ => Self::workspace(tab) == Some(self),
        }
    }
}
pub(super) struct Review {
    pub(super) id: uuid::Uuid,
    pub(super) origin: ReviewOrigin,
    pub(super) action: Value,
    pub(super) snapshot: String,
    pub(super) title: String,
    pub(super) detail: String,
    pub(super) reset_confirmed: bool,
    pub(super) mainline: Option<newport_client::git_history::MainlineReview>,
    pub(super) message: Option<Entity<TextareaState>>,
    pub(super) _subscription: Option<Subscription>,
}
impl Review {
    fn select_mainline(&mut self, id: uuid::Uuid, snapshot: &str, index: usize) -> bool {
        self.id == id
            && self.snapshot == snapshot
            && self
                .mainline
                .as_ref()
                .is_some_and(|review| review.select_parent(&mut self.action, index))
    }
    pub(super) fn is_branch_picker(&self) -> bool {
        self.origin == ReviewOrigin::BranchPicker
    }
}
fn branch_target(entry: &Value) -> Option<(String, String)> {
    let name = entry["name"]["display"].as_str()?;
    let oid = entry["oid"]["hex"].as_str()?;
    (entry["current"] == false
        && !name.is_empty()
        && !oid.is_empty()
        && entry["name"]["bytesB64"] == base64::engine::general_purpose::STANDARD.encode(name))
    .then(|| (name.to_owned(), oid.to_owned()))
}
impl Newport {
    fn integration_status(&self, origin: ReviewOrigin) -> Value {
        if origin == ReviewOrigin::Changes {
            self.git.page.clone()
        } else {
            self.value(origin.resource_key())
        }
    }
    pub(super) fn integration_ready(&self) -> bool {
        ReviewOrigin::workspace(self.git.tab)
            .is_some_and(|origin| self.integration_ready_from(origin))
    }
    fn integration_ready_from(&self, origin: ReviewOrigin) -> bool {
        origin.active(self.git.tab, self.git.branches.open)
            && !self.git_mutation_blocks_write()
            && self.git.repo["capabilities"]["readOnly"] == false
            && self.git.repo["capabilities"]["workingTree"] == true
            && if origin == ReviewOrigin::BranchPicker {
                self.git.branches.error.is_none()
            } else {
                self.git.error.is_none()
            }
            && self
                .resources
                .get(origin.resource_key())
                .is_some_and(|r| !r.pending && r.error.is_none())
            && self.integration_status(origin)["snapshot"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
    }
    pub(super) fn can_integrate_branch(&self) -> bool {
        self.git.branches.open
            && !self.git_branch_review_open()
            && self.git.integration_review.is_none()
            && self.integration_ready_from(ReviewOrigin::BranchPicker)
            && self.value("git-branch-status")["metadata"]["integration"].is_null()
            && self.value("git-branch-status")["metadata"]["head"]["detached"] == false
            && self.value("git-branch-status")["metadata"]["head"]["oid"]["hex"].is_string()
            && self
                .resources
                .get("git-branches")
                .is_some_and(|r| !r.pending && r.error.is_none())
            && branch_target(&self.git.branches.selected).is_some()
    }
    pub(super) fn prepare_branch_integration(&mut self, kind: &str, cx: &mut Context<Self>) {
        if !self.can_integrate_branch() {
            return;
        }
        let Some((name, oid)) = branch_target(&self.git.branches.selected) else {
            return;
        };
        let status = self.integration_status(ReviewOrigin::BranchPicker);
        let current = text(&status["metadata"]["head"]["name"], "display");
        let (title, detail, action) = match kind {
            "merge" => (
                format!("Merge {name} into {current}"),
                "Git may stop for conflict resolution. Review changes before continuing a merge.",
                json!({"kind":"merge","targetOid":oid}),
            ),
            "merge.fast_forward" => (
                format!("Fast-forward {current} to {name}"),
                "Only moves the current branch forward. Diverged history is refused.",
                json!({"kind":"merge.fast_forward","targetOid":oid}),
            ),
            "rebase" => (
                format!("Rebase {current} onto {name}"),
                "Replays the current branch’s commits and changes their identities. Git may stop for conflicts.",
                json!({"kind":"rebase","upstreamOid":oid}),
            ),
            _ => return,
        };
        self.git.integration_review = Some(Review {
            id: uuid::Uuid::new_v4(),
            origin: ReviewOrigin::BranchPicker,
            action,
            snapshot: text(&status, "snapshot"),
            title,
            detail: format!("{detail} Reviewed target: {oid}"),
            reset_confirmed: false,
            mainline: None,
            message: None,
            _subscription: None,
        });
        cx.notify();
    }
    fn prepare_integration_recovery(
        &mut self,
        kind: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(origin) = ReviewOrigin::workspace(self.git.tab) else {
            return;
        };
        let status = self.integration_status(origin);
        if !self.integration_ready() || !recovery_allowed(&status, kind) {
            return;
        }
        let (title, detail) = match kind {
            "integration.continue" => (
                "Continue operation",
                "Continue with staged resolutions. Leave the message empty to use Git’s default.",
            ),
            "integration.abort" => (
                "Abort operation",
                "Return to the state before the operation. Conflict resolutions and edits made during it may be discarded.",
            ),
            "integration.skip" => (
                "Skip commit",
                "Omit the current commit from this rebase and discard its pending resolution. Later commits are still processed.",
            ),
            _ => return,
        };
        let message = (kind == "integration.continue").then(|| {
            cx.new(|cx| TextareaState::new(window, cx).placeholder("Commit message (optional)"))
        });
        let subscription = message
            .as_ref()
            .map(|input| cx.observe(input, |_, _, cx| cx.notify()));
        self.git.integration_review = Some(Review {
            id: uuid::Uuid::new_v4(),
            origin,
            action: json!({"kind":kind}),
            snapshot: text(&status, "snapshot"),
            title: title.into(),
            detail: detail.into(),
            reset_confirmed: false,
            mainline: None,
            message,
            _subscription: subscription,
        });
        cx.notify();
    }
    fn submit_integration(&mut self, cx: &mut Context<Self>) {
        let Some(review) = &self.git.integration_review else {
            return;
        };
        let status = self.integration_status(review.origin);
        if !self.integration_ready_from(review.origin)
            || review.snapshot != text(&status, "snapshot")
        {
            return;
        }
        let mut action = review.action.clone();
        if review
            .mainline
            .as_ref()
            .is_some_and(|mainline| mainline.selected_index(&action).is_none())
        {
            return;
        }
        if action["kind"] == "reset"
            && (!git_reset::ready(&action, review.reset_confirmed)
                || action["expectedOid"] != status["metadata"]["head"]["oid"]["hex"])
        {
            return;
        }
        if action["kind"] == "commit.amend"
            && (action["expectedOid"] != status["metadata"]["head"]["oid"]["hex"]
                || review
                    .message
                    .as_ref()
                    .is_none_or(|message| message.read(cx).value().trim().is_empty()))
        {
            return;
        }
        if let Some(message) = &review.message {
            let value = message.read(cx).value().to_string();
            if !value.trim().is_empty() {
                action["message"] = json!(value);
            }
        }
        let snapshot = review.snapshot.clone();
        let id = review.id;
        self.start_git_mutation(
            self.git_repository_id(),
            review.title.clone(),
            move |backend, server, repo, id| {
                backend.apply_reviewed_action(server, repo, &snapshot, action, id)
            },
            cx,
        );
        if let Some(state) = self.git_mutation.as_mut() {
            state.integration_review = Some(id);
        }
    }
    pub(super) fn git_integration_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(origin) = ReviewOrigin::workspace(self.git.tab) else {
            return div().into_any_element();
        };
        self.integration_view_from(origin, cx)
    }
    pub(super) fn git_branch_integration_view(&self, cx: &mut Context<Self>) -> AnyElement {
        self.integration_view_from(ReviewOrigin::BranchPicker, cx)
    }
    fn mainline_review_controls(
        &self,
        review: &Review,
        stale: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(mainline) = review.mainline.as_ref() else {
            return div().into_any_element();
        };
        let id = review.id;
        let weak = cx.entity().downgrade();
        mainline_control(
            mainline,
            &review.action,
            stale || !self.integration_ready_from(review.origin),
            move |index, _, cx| {
                let _ = weak.update(cx, |this, cx| {
                    let Some(review) = this.git.integration_review.as_ref() else {
                        return;
                    };
                    if !this.integration_ready_from(review.origin) {
                        return;
                    }
                    let snapshot = text(&this.integration_status(review.origin), "snapshot");
                    if let Some(review) = this.git.integration_review.as_mut()
                        && review.select_mainline(id, &snapshot, index)
                    {
                        cx.notify();
                    }
                });
            },
        )
    }
    fn integration_view_from(&self, origin: ReviewOrigin, cx: &mut Context<Self>) -> AnyElement {
        if !origin.active(self.git.tab, self.git.branches.open) {
            return div().into_any_element();
        }
        let status = self.integration_status(origin);
        let state = &status["metadata"]["integration"];
        let review = self
            .git
            .integration_review
            .as_ref()
            .filter(|review| review.origin == origin);
        let show_recovery = origin != ReviewOrigin::BranchPicker && state.is_object();
        if review.is_none() && !show_recovery {
            return div().into_any_element();
        }
        let mut view = div().flex().flex_col().gap_2().px_4().py_2();
        if show_recovery {
            let weak = cx.entity().downgrade();
            let snapshot = text(&status, "snapshot");
            let server = self.server.clone();
            let repo = self.git_repository_id();
            view = view.child(recovery_banner(
                &status,
                !self.integration_ready(),
                self.git.integration_review.is_none(),
                move |kind, window, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        if this.server == server
                            && this.git_repository_id() == repo
                            && origin.active(this.git.tab, this.git.branches.open)
                            && text(&this.integration_status(origin), "snapshot") == snapshot
                        {
                            this.prepare_integration_recovery(kind, window, cx);
                        }
                    });
                },
            ));
        }
        if let Some(review) = review.filter(|_| self.project_dialog_rendering) {
            let stale = review.snapshot != text(&status, "snapshot");
            view = view
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(review.title.clone()),
                )
                .child(label(review.detail.clone()))
                .when(review.mainline.is_some(), |view| {
                    view.child(self.mainline_review_controls(review, stale, cx))
                })
                .when(review.action["kind"] == "reset", |view| {
                    view.child(self.reset_review_controls(review, cx))
                })
                .when(stale, |view| {
                    view.child(label("Repository state changed. Cancel and review again."))
                })
                .when_some(review.message.as_ref(), |view, message| {
                    view.child(
                        crate::controls::textarea(message)
                            .h(px(88.))
                            .aria_label(if review.action["kind"] == "commit.amend" {
                                "Replacement commit message"
                            } else {
                                "Integration commit message"
                            })
                            .disabled(self.git_mutation_blocks_write()),
                    )
                })
                .child(
                    project_dialog::actions()
                        .child(
                            crate::controls::button("cancel-integration")
                                .label("Cancel")
                                .disabled(self.git_mutation_blocks_write())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.git.integration_review = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            crate::controls::button("confirm-integration")
                                .loading(self.git_write_pending())
                                .label(match review.action["kind"].as_str() {
                                    Some("merge") => "Merge",
                                    Some("cherry_pick") => "Cherry-pick",
                                    Some("checkout") => "Check out commit",
                                    Some("commit.amend") => "Amend latest commit",
                                    Some("reset") => "Reset to commit",
                                    Some("conflict.resolve") => {
                                        if review.action["expectedOid"].is_null() {
                                            "Delete file"
                                        } else {
                                            "Use selected side"
                                        }
                                    }
                                    Some("revert") => "Revert commit",
                                    Some("merge.fast_forward") => "Fast-forward",
                                    Some("rebase") => "Rebase",
                                    Some("integration.continue") => "Continue operation",
                                    Some("integration.abort") => "Abort operation",
                                    Some("integration.skip") => "Skip commit",
                                    _ => "Confirm",
                                })
                                .when(
                                    matches!(
                                        review.action["kind"].as_str(),
                                        Some("integration.abort" | "integration.skip")
                                    ) || (review.action["kind"] == "reset"
                                        && review.action["mode"] == "hard"),
                                    |button| button.text_color(cx.theme().danger),
                                )
                                .disabled(
                                    !self.integration_ready_from(review.origin)
                                        || stale
                                        || review.mainline.as_ref().is_some_and(|mainline| {
                                            mainline.selected_index(&review.action).is_none()
                                        })
                                        || (review.action["kind"] == "reset"
                                            && !git_reset::ready(
                                                &review.action,
                                                review.reset_confirmed,
                                            ))
                                        || (review.action["kind"] == "commit.amend"
                                            && review.message.as_ref().is_none_or(|message| {
                                                message.read(cx).value().trim().is_empty()
                                            })),
                                )
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.submit_integration(cx)),
                                ),
                        ),
                );
        }
        view.into_any_element()
    }
}
fn recovery_name(status: &Value) -> &'static str {
    match status["metadata"]["integration"]["kind"].as_str() {
        Some("merge") => "Merge",
        Some("cherry_pick") => "Cherry-pick",
        Some("revert") => "Revert",
        Some("rebase") => "Rebase",
        _ => "Git operation",
    }
}
fn recovery_heading(status: &Value) -> String {
    let state = &status["metadata"]["integration"];
    let mut title = format!("{} in progress", recovery_name(status));
    if let (Some(position), Some(total)) = (state["position"].as_u64(), state["total"].as_u64())
        && total > 0
        && position <= total
    {
        title.push_str(&format!(" · {position} of {total}"));
    }
    title
}
fn recovery_banner(
    status: &Value,
    disabled: bool,
    show_controls: bool,
    on_action: impl Fn(&'static str, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let managed = status["metadata"]["integration"]["managed"] == true;
    let conflicts = rows(&status["entries"])
        .iter()
        .any(|entry| entry["conflicted"] == true);
    let detail = if !managed {
        "This operation cannot be managed by Newport. Finish it with the tool that started it, then refresh."
    } else if conflicts {
        "Resolve the conflicting files, then stage each resolution before continuing."
    } else {
        "Review the staged changes before continuing. Git checks for remaining conflicts."
    };
    let mut view = div().flex().flex_wrap().items_center().gap_4().child(
        div()
            .flex_1()
            .min_w(px(220.))
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(recovery_heading(status)),
            )
            .child(label(detail).text_xs()),
    );
    if show_controls && managed {
        let on_action = std::rc::Rc::new(on_action);
        let mut controls = div().flex().flex_wrap().gap_2();
        for (kind, title) in [
            (
                "integration.continue",
                format!("Continue {}", recovery_name(status).to_lowercase()),
            ),
            ("integration.skip", "Skip commit…".into()),
            ("integration.abort", "Abort…".into()),
        ] {
            if recovery_supported(status, kind) {
                let on_action = on_action.clone();
                controls = controls.child(
                    crate::controls::button(kind)
                        .label(title)
                        .disabled(disabled || !recovery_allowed(status, kind))
                        .on_click(move |_, window, cx| on_action(kind, window, cx)),
                );
            }
        }
        view = view.child(controls);
    }
    view.into_any_element()
}
fn mainline_control(
    mainline: &newport_client::git_history::MainlineReview,
    action: &Value,
    disabled: bool,
    on_select: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let selected = mainline.selected_index(action);
    let parents = mainline.parents().to_vec();
    let title = selected
        .and_then(|index| {
            parents
                .get(index)
                .map(|oid| format!("Parent {} · {}", index + 1, &oid[..8]))
        })
        .unwrap_or_else(|| "Choose mainline parent".into());
    let on_select = std::rc::Rc::new(on_select);
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(label("Mainline parent"))
        .child(
            crate::controls::button("git-replay-parent")
                .accessibility_label(format!("Mainline parent: {title}"))
                .label(title)
                .disabled(disabled)
                .dropdown_menu(move |mut menu, _, _| {
                    for (index, oid) in parents.iter().enumerate() {
                        let on_select = on_select.clone();
                        menu = menu.item(
                            PopupMenuItem::new(format!("Parent {} · {}", index + 1, &oid[..8]))
                                .checked(selected == Some(index))
                                .on_click(move |_, window, cx| on_select(index, window, cx)),
                        );
                    }
                    menu
                }),
        )
        .child(label("Use the changes relative to this parent of the merge commit.").text_xs())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use gpui_kit::{
        self as gpui, AppContext as _, Context, IntoElement, ParentElement as _, Render,
        Styled as _, TestAppContext, Window, div, px,
    };

    struct RecoveryHost {
        status: serde_json::Value,
        actions: Vec<&'static str>,
    }
    impl Render for RecoveryHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let weak = cx.entity().downgrade();
            div().size_full().p_4().child(super::recovery_banner(
                &self.status,
                false,
                true,
                move |kind, _, cx| {
                    let _ = weak.update(cx, |view, _| view.actions.push(kind));
                },
            ))
        }
    }

    #[gpui::test]
    fn recovery_controls_follow_capabilities_and_conflict_readiness(cx: &mut TestAppContext) {
        use gpui::base::test_support::snapshots;
        cx.update(|cx| {
            gpui::init(cx);
            crate::theme::install(false, cx);
            cx.set_reduce_motion(true);
        });
        let mut host = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| RecoveryHost {
                status: serde_json::json!({"metadata":{"integration":{"kind":"merge","managed":true,"canContinue":true,"canAbort":true,"canSkip":false}},"entries":[{"conflicted":true}]}),
                actions: Vec::new(),
            });
            host = Some(view.clone());
            gpui::component::Root::new(view, window, cx)
        });
        let host = host.unwrap();
        for width in [360., 900.] {
            cx.update(|window, cx| {
                window.resize(gpui::size(px(width), px(300.)));
                window.draw(cx).clear(cx);
                let mut buttons: Vec<_> = snapshots(window)
                    .into_iter()
                    .filter(|node| node.role() == Some(gpui::accesskit::Role::Button))
                    .filter_map(|node| node.label().map(str::to_owned))
                    .collect();
                buttons.sort();
                assert_eq!(buttons, ["Abort…", "Continue merge"]);
            });
        }
        for (label, expected) in [
            ("Continue merge", vec![]),
            ("Abort…", vec!["integration.abort"]),
        ] {
            let position = cx.update(|window, _| {
                snapshots(window)
                    .into_iter()
                    .find(|node| node.label() == Some(label))
                    .unwrap()
                    .bounds()
                    .center()
            });
            cx.simulate_click(position, Default::default());
            cx.run_until_parked();
            host.read_with(cx, |view, _| assert_eq!(view.actions, expected));
        }
        host.update(cx, |view, cx| {
            view.status["entries"] = serde_json::json!([{"conflicted":false}]);
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let position = cx.update(|window, _| {
            snapshots(window)
                .into_iter()
                .find(|node| node.label() == Some("Continue merge"))
                .unwrap()
                .bounds()
                .center()
        });
        cx.simulate_click(position, Default::default());
        cx.run_until_parked();
        host.read_with(cx, |view, _| {
            assert_eq!(view.actions, ["integration.abort", "integration.continue"])
        });
        host.update(cx, |view, cx| {
            view.status["metadata"]["integration"]["managed"] = serde_json::json!(false);
            cx.notify();
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            assert!(
                !snapshots(window)
                    .iter()
                    .any(|node| node.role() == Some(gpui::accesskit::Role::Button))
            );
        });
    }

    #[test]
    fn recovery_title_uses_readable_names_and_valid_progress() {
        let mut status = serde_json::json!({"metadata":{"integration":{"kind":"rebase","position":2,"total":5}}});
        assert_eq!(
            super::recovery_heading(&status),
            "Rebase in progress · 2 of 5"
        );
        status["metadata"]["integration"]["position"] = serde_json::json!(6);
        assert_eq!(super::recovery_heading(&status), "Rebase in progress");
        status["metadata"]["integration"]["kind"] = serde_json::json!("cherry_pick");
        assert_eq!(super::recovery_heading(&status), "Cherry-pick in progress");
        status["metadata"]["integration"]["kind"] = serde_json::json!("unknown");
        assert_eq!(
            super::recovery_heading(&status),
            "Git operation in progress"
        );
    }

    struct ReplayHost {
        mainline: newport_client::git_history::MainlineReview,
        action: serde_json::Value,
        disabled: bool,
    }
    impl Render for ReplayHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let weak = cx.entity().downgrade();
            div().size_full().p_4().child(super::mainline_control(
                &self.mainline,
                &self.action,
                self.disabled,
                move |index, _, cx| {
                    let _ = weak.update(cx, |view, cx| {
                        if !view.disabled && view.mainline.select_parent(&mut view.action, index) {
                            cx.notify();
                        }
                    });
                },
            ))
        }
    }

    #[gpui::test]
    fn mainline_menu_updates_the_review_and_announces_its_selected_parent(cx: &mut TestAppContext) {
        use gpui::base::test_support::snapshots;
        cx.update(|cx| {
            gpui::init(cx);
            crate::theme::install(false, cx);
            cx.set_reduce_motion(true);
        });
        let oid = "a".repeat(40);
        let detail = serde_json::json!({"oid":{"hex":oid},"parents":[{"hex":"b".repeat(40)},{"hex":"c".repeat(40)}]});
        let mut host = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| ReplayHost {
                mainline: newport_client::git_history::MainlineReview::from_commit(&detail, &oid)
                    .unwrap(),
                action: newport_client::git_history::commit_action("revert", &detail, &oid, 0)
                    .unwrap(),
                disabled: false,
            });
            host = Some(view.clone());
            gpui::component::Root::new(view, window, cx)
        });
        let host = host.unwrap();
        cx.update(|window, cx| {
            window.resize(gpui::size(px(640.), px(480.)));
            window.draw(cx).clear(cx);
        });
        for label in [
            "Mainline parent: Parent 1 · bbbbbbbb",
            "Parent 2 · cccccccc",
            "Mainline parent: Parent 2 · cccccccc",
        ] {
            let position = cx.update(|window, _| {
                snapshots(window)
                    .into_iter()
                    .find(|node| {
                        node.label() == Some(label)
                            && matches!(
                                node.role(),
                                Some(
                                    gpui::accesskit::Role::Button | gpui::accesskit::Role::MenuItem
                                )
                            )
                    })
                    .unwrap_or_else(|| panic!("Missing {label}"))
                    .bounds()
                    .center()
            });
            cx.simulate_click(position, Default::default());
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
        }
        assert_eq!(
            host.read_with(cx, |host, _| host.action["mainline"].clone()),
            serde_json::json!(2)
        );
        assert_eq!(
            host.read_with(cx, |host, _| host.action["targetOid"].clone()),
            serde_json::json!(oid)
        );
        cx.update(|window, _| {
            let selected = snapshots(window)
                .into_iter()
                .find(|node| node.label() == Some("Mainline parent: Parent 2 · cccccccc"))
                .unwrap();
            assert_eq!(selected.role(), Some(gpui::accesskit::Role::Button));
        });
        cx.simulate_keystrokes("escape");
    }
    #[test]
    fn parent_menu_cannot_edit_a_replaced_or_stale_review() {
        let oid = "a".repeat(40);
        let detail = serde_json::json!({"oid":{"hex":oid},"parents":[{"hex":"b".repeat(40)},{"hex":"c".repeat(40)}]});
        let id = uuid::Uuid::new_v4();
        let mut review = super::Review {
            id,
            origin: super::ReviewOrigin::History,
            action: newport_client::git_history::commit_action("cherry_pick", &detail, &oid, 0)
                .unwrap(),
            snapshot: "snapshot".into(),
            title: "Cherry-pick".into(),
            detail: String::new(),
            reset_confirmed: false,
            mainline: newport_client::git_history::MainlineReview::from_commit(&detail, &oid),
            message: None,
            _subscription: None,
        };
        let original = review.action.clone();
        assert!(!review.select_mainline(uuid::Uuid::new_v4(), "snapshot", 1));
        assert!(!review.select_mainline(id, "changed", 1));
        assert!(!review.select_mainline(id, "snapshot", 2));
        assert_eq!(review.action, original);
        assert!(review.select_mainline(id, "snapshot", 1));
        assert_eq!(review.action["mainline"], 2);
        assert_eq!(review.action["targetOid"], oid);
    }
    use super::{ReviewOrigin, branch_target, recovery_allowed};
    use serde_json::json;
    #[test]
    fn recovery_obeys_agent_flags_and_conflicts() {
        let mut status = json!({"metadata":{"integration":{"managed":true,"canContinue":true,"canAbort":true,"canSkip":false}},"entries":[]});
        assert!(recovery_allowed(&status, "integration.continue"));
        assert!(recovery_allowed(&status, "integration.abort"));
        assert!(!recovery_allowed(&status, "integration.skip"));
        status["entries"] = json!([{"conflicted":true}]);
        assert!(!recovery_allowed(&status, "integration.continue"));
        status["metadata"]["integration"]["managed"] = json!(false);
        assert!(!recovery_allowed(&status, "integration.abort"));
        assert!(branch_target(&json!({"name":{"display":"topic","bytesB64":"/w=="},"oid":{"hex":"abc"},"current":false})).is_none());
    }
    #[test]
    fn opening_picker_does_not_retarget_workspace_review_status() {
        for (tab, origin, key) in [
            (0, ReviewOrigin::Changes, "git-list"),
            (1, ReviewOrigin::History, "git-tag-status"),
        ] {
            assert_eq!(ReviewOrigin::workspace(tab), Some(origin));
            assert!(origin.active(tab, false));
            assert!(origin.active(tab, true));
            assert_eq!(origin.resource_key(), key);
            assert_ne!(
                origin.resource_key(),
                ReviewOrigin::BranchPicker.resource_key()
            );
        }
        assert!(!ReviewOrigin::Changes.active(1, true));
        assert!(!ReviewOrigin::History.active(0, true));
        assert!(!ReviewOrigin::BranchPicker.active(0, false));
        assert!(ReviewOrigin::BranchPicker.active(1, true));
        assert_eq!(ReviewOrigin::workspace(2), None);
    }
}
