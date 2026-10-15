use super::*;
pub(super) struct Review {
    mode: backend::StashMode,
    message: String,
    oid: String,
    index: u64,
    token: String,
    snapshot: String,
    reinstate: bool,
}
pub(super) struct SaveForm {
    id: uuid::Uuid,
    message: Entity<InputState>,
    snapshot: String,
    include_untracked: bool,
    keep_index: bool,
    _subscription: Subscription,
}
impl SaveForm {
    pub(super) fn draft(&self, cx: &App) -> (uuid::Uuid, String, bool, bool) {
        (
            self.id,
            self.message.read(cx).value().to_string(),
            self.include_untracked,
            self.keep_index,
        )
    }
}
impl Newport {
    pub(super) fn stash_dialog_title(&self) -> String {
        self.git
            .stash_apply
            .as_ref()
            .map(|review| format!("{} stash", review.mode.label()))
            .unwrap_or_else(|| "Save stash".into())
    }

    fn can_save_stash(&self) -> bool {
        self.git.tab == 3
            && !self.git_mutation_blocks_write()
            && self.git.repo["capabilities"]["readOnly"] == false
            && self.git.repo["capabilities"]["workingTree"] == true
            && self.value("git-stash-status")["snapshot"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
            && self.value("git-stash-status")["metadata"]["integration"].is_null()
            && self
                .resources
                .get("git-stash-status")
                .is_some_and(|r| !r.pending && r.error.is_none())
    }
    fn new_stash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_save_stash() {
            return;
        }
        let message = cx.new(|cx| InputState::new(window, cx).placeholder("Optional description"));
        let subscription =
            Self::project_form_input(&message, project_dialog::Kind::Stash, Self::save_stash, cx);
        self.git.stash_apply = None;
        self.git.detail_open = true;
        self.git.stash_save = Some(SaveForm {
            id: uuid::Uuid::new_v4(),
            message,
            snapshot: text(&self.value("git-stash-status"), "snapshot"),
            include_untracked: false,
            keep_index: false,
            _subscription: subscription,
        });
        cx.notify();
    }
    fn save_stash(&mut self, cx: &mut Context<Self>) {
        if !self.can_save_stash() {
            return;
        }
        let Some(form) = &self.git.stash_save else {
            return;
        };
        let draft = form.draft(cx);
        let options = (draft.1.clone(), draft.2, draft.3);
        let snapshot = form.snapshot.clone();
        self.start_git_mutation(
            self.git_repository_id(),
            "Save stash".into(),
            move |backend, server, repo, id| {
                backend.save_stash(server, repo, &snapshot, options, id)
            },
            cx,
        );
        if let Some(state) = self.git_mutation.as_mut() {
            state.stash_draft = Some(draft);
        }
    }

    fn can_apply_stash(&self) -> bool {
        self.git.tab == 3
            && self.git.error.is_none()
            && !self.git_mutation_blocks_write()
            && self.git.repo["capabilities"]["readOnly"] == false
            && self.git.repo["capabilities"]["workingTree"] == true
            && self.git.selected["oid"].is_string()
            && self.git.selected["index"].is_u64()
            && self.git.page["metadata"]["listToken"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
            && self.value("git-stash-status")["snapshot"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
            && self.value("git-stash-status")["metadata"]["integration"].is_null()
            && ["git-list", "git-stash-status"].iter().all(|key| {
                self.resources
                    .get(key)
                    .is_some_and(|r| !r.pending && r.error.is_none())
            })
    }
    fn review_stash(&mut self, mode: backend::StashMode, cx: &mut Context<Self>) {
        if !self.can_apply_stash() {
            return;
        }
        self.git.stash_apply = Some(Review {
            mode,
            message: stash_message(&self.git.selected),
            oid: text(&self.git.selected, "oid"),
            index: self.git.selected["index"].as_u64().unwrap(),
            token: text(&self.git.page["metadata"], "listToken"),
            snapshot: text(&self.value("git-stash-status"), "snapshot"),
            reinstate: false,
        });
        cx.notify();
    }
    fn apply_stash(&mut self, cx: &mut Context<Self>) {
        if !self.can_apply_stash() {
            return;
        }
        let Some(review) = &self.git.stash_apply else {
            return;
        };
        let oid = review.oid.clone();
        let index = review.index;
        let token = review.token.clone();
        let snapshot = review.snapshot.clone();
        let reinstate = review.reinstate;
        let mode = review.mode;
        self.start_git_mutation(
            self.git_repository_id(),
            format!("{} stash@{{{index}}}", mode.label()),
            move |backend, server, repo, id| {
                backend.modify_stash(
                    server,
                    repo,
                    &snapshot,
                    backend::StashTarget {
                        mode,
                        oid: &oid,
                        index,
                        token: &token,
                        reinstate_index: reinstate,
                    },
                    id,
                )
            },
            cx,
        );
    }
    pub(super) fn git_stash_selected_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let actions = div().flex().flex_wrap().gap_2().children(
            [
                backend::StashMode::Apply,
                backend::StashMode::Pop,
                backend::StashMode::Drop,
            ]
            .into_iter()
            .map(|mode| {
                crate::controls::button(format!("stash-{}", mode.label()))
                    .label(format!("{}…", mode.label()))
                    .disabled(
                        !self.can_apply_stash()
                            || self.git.stash_apply.is_some()
                            || self.git.stash_save.is_some(),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.review_stash(mode, cx)))
            }),
        );
        actions.into_any_element()
    }
    pub(super) fn git_stash_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let action = crate::controls::button("new-stash")
            .icon(IconName::Plus)
            .label("Save changes…")
            .disabled(
                !self.can_save_stash()
                    || self.git.stash_save.is_some()
                    || self.git.stash_apply.is_some(),
            )
            .on_click(cx.listener(|this, _, window, cx| this.new_stash(window, cx)));
        crate::git_listing::toolbar(&self.git.page, "stash", "stashes", action, cx)
    }
    pub(super) fn git_stash_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.git_mutation_blocks_write();
        let body = div()
            .flex()
            .flex_col()
            .gap_3()
            .w_full()
            .when_some(self.git.stash_save.as_ref(), |view, form| {
                view.child(
                    div()
                        .w_full()
                        .max_w(px(520.))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(label("Stash message").text_xs())
                        .child(
                            crate::controls::input(&form.message)
                                .aria_label("Stash message")
                                .disabled(busy),
                        ),
                )
                .child(
                    crate::controls::checkbox("stash-include-untracked")
                        .label("Include untracked files")
                        .checked(form.include_untracked)
                        .disabled(busy)
                        .on_click(cx.listener(|this, value: &bool, _, cx| {
                            if !this.git_mutation_blocks_write()
                                && let Some(form) = this.git.stash_save.as_mut()
                            {
                                form.include_untracked = *value;
                                cx.notify();
                            }
                        })),
                )
                .child(
                    crate::controls::checkbox("stash-keep-index")
                        .label("Keep staged changes in the working tree")
                        .checked(form.keep_index)
                        .disabled(busy)
                        .on_click(cx.listener(|this, value: &bool, _, cx| {
                            if !this.git_mutation_blocks_write()
                                && let Some(form) = this.git.stash_save.as_mut()
                            {
                                form.keep_index = *value;
                                cx.notify();
                            }
                        })),
                )
                .child(
                    project_dialog::actions()
                        .child(
                            crate::controls::button("save-stash-cancel")
                                .label("Cancel")
                                .disabled(self.git_write_pending())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if this.git_write_pending() {
                                        return;
                                    }
                                    this.git.stash_save = None;
                                    this.git.detail_open = !this.git.selected.is_null();
                                    cx.notify();
                                })),
                        )
                        .child(
                            crate::controls::button("save-stash-confirm")
                                .loading(self.git_write_pending())
                                .label("Save stash")
                                .disabled(!self.can_save_stash())
                                .on_click(cx.listener(|this, _, _, cx| this.save_stash(cx))),
                        ),
                )
            })
            .when_some(self.git.stash_apply.as_ref(), |view, review| {
                view.child(review_description(review))
                    .when(review.mode != backend::StashMode::Drop, |view| {
                        view.child(
                            crate::controls::checkbox("stash-reinstate-index")
                                .label("Restore staged changes too")
                                .checked(review.reinstate)
                                .disabled(busy)
                                .on_click(cx.listener(|this, value: &bool, _, cx| {
                                    if !this.git_mutation_blocks_write()
                                        && let Some(review) = this.git.stash_apply.as_mut()
                                    {
                                        review.reinstate = *value;
                                        cx.notify();
                                    }
                                })),
                        )
                    })
                    .child(
                        project_dialog::actions()
                            .child(
                                crate::controls::button("apply-stash-cancel")
                                    .label("Cancel")
                                    .disabled(self.git_write_pending())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if this.git_write_pending() {
                                            return;
                                        }
                                        this.git.stash_apply = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                crate::controls::button("apply-stash-confirm")
                                    .loading(self.git_write_pending())
                                    .when(review.mode == backend::StashMode::Drop, |button| {
                                        button.danger()
                                    })
                                    .label(format!("{} stash", review.mode.label()))
                                    .disabled(!self.can_apply_stash())
                                    .on_click(cx.listener(|this, _, _, cx| this.apply_stash(cx))),
                            ),
                    )
            });
        crate::git_listing::editor(
            match self.git.stash_apply.as_ref().map(|review| review.mode) {
                Some(backend::StashMode::Apply) => "Apply stash",
                Some(backend::StashMode::Pop) => "Pop stash",
                Some(backend::StashMode::Drop) => "Drop stash",
                None => "Save stash",
            },
            body,
            cx,
        )
    }
}

fn stash_message(selected: &Value) -> String {
    let message = selected["message"]
        .as_str()
        .filter(|message| !message.is_empty())
        .unwrap_or("Unnamed stash");
    format!(
        "{message}{}",
        if selected["messageTruncated"] == true {
            "…"
        } else {
            ""
        }
    )
}

fn review_description(review: &Review) -> Div {
    div().flex().flex_col().min_w_0().gap_2()
        .child(project_dialog::description("stash-review-identity", format!("{} · stash@{{{}}} · {}", review.message, review.index, review.oid.chars().take(12).collect::<String>())).font_weight(FontWeight::SEMIBOLD))
        .child(project_dialog::description("stash-review-description", match review.mode {
            backend::StashMode::Apply => "Apply changes to this working tree. The saved stash will be kept.",
            backend::StashMode::Pop => "Apply changes and remove this saved entry on success. Conflicts keep the stash.",
            backend::StashMode::Drop => "Delete this saved stash without applying it. This cannot be undone in Newport.",
        }))
}

impl SaveForm {
    pub(super) fn initial_focus(&self, cx: &App) -> FocusHandle {
        self.message.read(cx).focus_handle(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::{Review, review_description, stash_message};
    use crate::backend;
    use gpui_kit::base::test_support::snapshots;
    use gpui_kit::{self as gpui, Context, IntoElement, Render, Window, div, prelude::*, px};
    use serde_json::json;

    struct ReviewHost(Review);
    impl Render for ReviewHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w_full()
                .flex()
                .flex_col()
                .child(review_description(&self.0))
                .child(
                    div()
                        .debug_selector(|| "review-footer".into())
                        .h(px(28.))
                        .child("Cancel"),
                )
        }
    }
    #[gpui::test]
    fn stash_review_exposes_identity_and_action_consequences_without_clipping(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui::init(cx);
            crate::theme::install(false, cx);
        });
        let message =
            "On feature/开发: saved changes before updating the application and its dependencies";
        let (view, visual) = cx.add_window_view(|_, _| {
            ReviewHost(Review {
                mode: backend::StashMode::Apply,
                message: message.into(),
                oid: "123456789abcdef0123456789abcdef0123456789a".into(),
                index: 12,
                token: "list".into(),
                snapshot: "status".into(),
                reinstate: false,
            })
        });
        for (mode, consequence) in [
            (backend::StashMode::Apply, "The saved stash will be kept."),
            (backend::StashMode::Pop, "Conflicts keep the stash."),
            (
                backend::StashMode::Drop,
                "This cannot be undone in Newport.",
            ),
        ] {
            view.update(visual, |view, cx| {
                view.0.mode = mode;
                cx.notify();
            });
            for width in [520., 280.] {
                visual.simulate_resize(gpui::size(px(width), px(400.)));
                visual.update(|window, cx| window.draw(cx).clear(cx));
                let (identity, description) = visual.update(|window, _| {
                    let elements = snapshots(window);
                    let expected = format!("{message} · stash@{{12}} · 123456789abc");
                    let identity = elements
                        .iter()
                        .find(|node| node.label() == Some(expected.as_str()))
                        .unwrap();
                    let description = elements
                        .iter()
                        .find(|node| node.label().is_some_and(|text| text.ends_with(consequence)))
                        .unwrap();
                    assert_eq!(identity.role(), Some(gpui::Role::Label));
                    assert_eq!(description.role(), Some(gpui::Role::Label));
                    (identity.bounds(), description.bounds())
                });
                assert!(identity.size.height > px(18.));
                assert!(identity.right() <= px(width));
                assert!(description.top() >= identity.bottom());
                assert!(description.right() <= px(width));
                assert!(
                    visual.debug_bounds("review-footer").unwrap().top() >= description.bottom()
                );
            }
        }
    }

    #[test]
    fn stash_review_marks_truncated_messages_and_names_empty_ones() {
        assert_eq!(stash_message(&json!({"message":""})), "Unnamed stash");
        assert_eq!(
            stash_message(&json!({"message":"保存", "messageTruncated":true})),
            "保存…"
        );
        assert_eq!(stash_message(&json!({"message":"saved"})), "saved");
    }
}
