use super::*;
use newport_client::git_worktree::{
    lock_target, removal_target, repair_path_valid, review_matches,
};

pub(super) struct LockForm {
    id: uuid::Uuid,
    name: String,
    path: String,
    snapshot: String,
    unlock: bool,
    reason: Entity<InputState>,
    _subscription: Subscription,
}
impl LockForm {
    pub(super) fn title(&self) -> &'static str {
        if self.unlock {
            "Unlock worktree"
        } else {
            "Lock worktree"
        }
    }

    pub(super) fn draft(&self, cx: &App) -> (uuid::Uuid, String) {
        (self.id, self.reason.read(cx).value().to_string())
    }
}

impl Newport {
    pub(super) fn checkout_worktree_review_open(&self) -> bool {
        self.git.worktree_lock.is_some()
            || self.git.worktree_repair.is_some()
            || self.git.worktree_remove.is_some()
    }
    pub(super) fn git_worktree_review_editor(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (title, controls) = if let Some(form) = &self.git.worktree_lock {
            (
                if form.unlock {
                    "Unlock worktree"
                } else {
                    "Lock worktree"
                },
                self.git_worktree_lock_controls(cx),
            )
        } else if self.git.worktree_repair.is_some() {
            (
                "Locate moved worktree",
                self.git_worktree_repair_controls(cx),
            )
        } else if let Some(review) = &self.git.worktree_remove {
            (
                if review.missing {
                    "Remove missing registration"
                } else {
                    "Remove worktree"
                },
                self.git_worktree_remove_controls(cx),
            )
        } else {
            return None;
        };
        let body = div().flex().flex_col().gap_3()
            .when(self.git_worktree_review_stale(), |view| view.child(crate::label("Repository state changed. Cancel this review and select the worktree again.")
                .id("worktree-review-stale").role(accesskit::Role::Label).aria_label("Repository state changed. Cancel this review and select the worktree again.")))
            .child(controls);
        Some(git_listing::editor(title, body, cx))
    }
    pub(super) fn can_lock_worktree(&self) -> bool {
        self.git.checkouts.open
            && self.git.repo["capabilities"]["readOnly"] == false
            && !self.git_mutation_blocks_write()
            && self.git.checkouts.contains(&self.git.checkouts.selected)
            && lock_target(&self.git.checkouts.selected).is_some()
            && self.git.checkouts.page["entries"]
                .as_array()
                .is_some_and(|rows| rows.contains(&self.git.checkouts.selected))
            && self.git.checkouts.page["snapshot"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
            && self
                .resources
                .get("git-checkouts")
                .is_some_and(|r| !r.pending && r.error.is_none())
    }
    pub(super) fn review_worktree_lock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_lock_worktree() || self.git_worktree_review_open() {
            return;
        }
        let Some((name, unlock)) = lock_target(&self.git.checkouts.selected) else {
            return;
        };
        let reason = cx.new(|cx| InputState::new(window, cx).placeholder("Optional lock reason"));
        if !unlock {
            crate::controls::focus_editor(&reason, window, cx);
        }
        let subscription = Self::project_form_input(
            &reason,
            project_dialog::Kind::Worktree,
            Self::change_worktree_lock,
            cx,
        );
        self.git.worktree_lock = Some(LockForm {
            id: uuid::Uuid::new_v4(),
            name,
            path: text(&self.git.checkouts.selected["path"], "display"),
            snapshot: text(&self.git.checkouts.page, "snapshot"),
            unlock,
            reason,
            _subscription: subscription,
        });
        cx.notify();
    }
    fn change_worktree_lock(&mut self, cx: &mut Context<Self>) {
        if !self.can_lock_worktree() {
            return;
        }
        let Some(form) = &self.git.worktree_lock else {
            return;
        };
        if !self.worktree_review_current(&form.name, &form.snapshot) {
            return;
        }
        let draft = form.draft(cx);
        let reason = draft.1.clone();
        let name = form.name.clone();
        let snapshot = form.snapshot.clone();
        let unlock = form.unlock;
        self.start_git_mutation(
            self.git_repository_id(),
            format!("{} worktree {name}", if unlock { "Unlock" } else { "Lock" }),
            move |backend, server, repo, id| {
                backend.lock_worktree(server, repo, &snapshot, (&name, &reason, unlock), id)
            },
            cx,
        );
        if let Some(state) = self.git_mutation.as_mut() {
            state.worktree_draft = Some(draft);
        }
    }
    pub(super) fn git_worktree_lock_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.git.worktree_lock.is_none() {
            return div().into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .items_start()
            .gap_2()
            .py_2()
            .when_some(self.git.worktree_lock.as_ref(), |view, form| {
                view.child(
                    crate::label(format!(
                        "{} {}",
                        if form.unlock {
                            "Allow removal and pruning of"
                        } else {
                            "Prevent removal and pruning of"
                        },
                        form.path
                    ))
                    .text_size(px(crate::ui::FONT_SIZE)),
                )
                .when(!form.unlock, |view| {
                    view.child(
                        div().w_full().max_w(px(520.)).child(
                            crate::controls::input(&form.reason)
                                .aria_label("Worktree lock reason")
                                .disabled(self.git_mutation_blocks_write()),
                        ),
                    )
                })
                .when_some(
                    newport_client::git_worktree::lock_action(
                        &form.name,
                        &form.reason.read(cx).value(),
                        form.unlock,
                    )
                    .err(),
                    |view, error| {
                        view.child(
                            crate::label(error.clone())
                                .id("worktree-lock-input-error")
                                .role(accesskit::Role::Label)
                                .aria_label(error)
                                .text_color(theme::error(cx.theme().is_dark()))
                                .text_size(px(crate::ui::FONT_SIZE)),
                        )
                    },
                )
                .child(
                    project_dialog::actions()
                        .child(
                            crate::controls::button("cancel-worktree-lock")
                                .label("Cancel")
                                .disabled(self.git_mutation_blocks_write())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.git.worktree_lock = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            crate::controls::button("confirm-worktree-lock")
                                .primary()
                                .label(if form.unlock {
                                    "Unlock worktree"
                                } else {
                                    "Lock worktree"
                                })
                                .disabled(
                                    !self.can_lock_worktree()
                                        || newport_client::git_worktree::lock_action(
                                            &form.name,
                                            &form.reason.read(cx).value(),
                                            form.unlock,
                                        )
                                        .is_err()
                                        || !self
                                            .worktree_review_current(&form.name, &form.snapshot),
                                )
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.change_worktree_lock(cx)),
                                ),
                        ),
                )
            })
            .into_any_element()
    }
}

pub(super) struct RepairForm {
    id: uuid::Uuid,
    name: String,
    original_path: String,
    snapshot: String,
    path: Entity<InputState>,
    _subscription: Subscription,
}
impl RepairForm {
    pub(super) fn draft(&self, cx: &App) -> (uuid::Uuid, String) {
        (self.id, self.path.read(cx).value().to_string())
    }
}

impl Newport {
    pub(super) fn can_repair_worktree(&self) -> bool {
        self.can_lock_worktree()
            && self.git.checkouts.selected["current"] == false
            && self.git.checkouts.selected["state"] == "missing"
    }
    pub(super) fn review_worktree_repair(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_repair_worktree() || self.git_worktree_review_open() {
            return;
        }
        let Some((name, _)) = lock_target(&self.git.checkouts.selected) else {
            return;
        };
        let path = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Current absolute path of the moved checkout")
        });
        crate::controls::focus_editor(&path, window, cx);
        let subscription = Self::project_form_input(
            &path,
            project_dialog::Kind::Worktree,
            Self::repair_worktree,
            cx,
        );
        self.git.worktree_repair = Some(RepairForm {
            id: uuid::Uuid::new_v4(),
            name,
            original_path: text(&self.git.checkouts.selected["path"], "display"),
            snapshot: text(&self.git.checkouts.page, "snapshot"),
            path,
            _subscription: subscription,
        });
        cx.notify();
    }
    fn repair_worktree(&mut self, cx: &mut Context<Self>) {
        if !self.can_repair_worktree() {
            return;
        }
        let Some(form) = &self.git.worktree_repair else {
            return;
        };
        if !self.worktree_review_current(&form.name, &form.snapshot) {
            return;
        }
        let draft = form.draft(cx);
        if !repair_path_valid(&draft.1) {
            return;
        }
        let name = form.name.clone();
        let snapshot = form.snapshot.clone();
        let path = draft.1.clone();
        self.start_git_mutation(
            self.git_repository_id(),
            format!("Repair worktree {name}"),
            move |backend, server, repo, id| {
                backend.repair_worktree(server, repo, &snapshot, (&name, &path), id)
            },
            cx,
        );
        if let Some(state) = self.git_mutation.as_mut() {
            state.worktree_repair_draft = Some(draft);
        }
    }
    pub(super) fn git_worktree_repair_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.git.worktree_repair.is_none() {
            return div().into_any_element();
        }
        div().flex().flex_col().items_start().gap_2().py_2()
            .when_some(self.git.worktree_repair.as_ref(),|view,form| {
                view.child(crate::label(format!("Reconnect {}",form.original_path)).text_size(px(crate::ui::FONT_SIZE)))
                    .child(div().w_full().max_w(px(520.)).child(crate::controls::input(&form.path).aria_label("Moved worktree path").disabled(self.git_mutation_blocks_write())))
                    .child(crate::label("Select the directory this checkout was moved to. Its files will stay in place.").text_size(px(crate::ui::FONT_SIZE)))
                    .child(project_dialog::actions()
                        .child(crate::controls::button("cancel-worktree-repair").label("Cancel").disabled(self.git_mutation_blocks_write()).on_click(cx.listener(|this,_,_,cx|{this.git.worktree_repair=None;cx.notify();})))
                        .child(crate::controls::button("confirm-worktree-repair").primary().loading(self.git_write_pending()).label("Repair worktree")
                            .disabled(!self.can_repair_worktree() || !self.worktree_review_current(&form.name, &form.snapshot) || !repair_path_valid(&form.draft(cx).1))
                            .on_click(cx.listener(|this,_,_,cx|this.repair_worktree(cx)))))
            }).into_any_element()
    }
}

#[derive(Clone)]
pub(super) struct RemoveReview {
    name: String,
    path: String,
    snapshot: String,
    missing: bool,
}

impl Newport {
    pub(super) fn can_remove_worktree(&self) -> bool {
        self.can_lock_worktree() && removal_target(&self.git.checkouts.selected).is_some()
    }
    pub(super) fn review_worktree_removal(&mut self, cx: &mut Context<Self>) {
        if !self.can_remove_worktree() || self.git_worktree_review_open() {
            return;
        }
        let Some((name, missing)) = removal_target(&self.git.checkouts.selected) else {
            return;
        };
        self.git.worktree_remove = Some(RemoveReview {
            name,
            missing,
            path: text(&self.git.checkouts.selected["path"], "display"),
            snapshot: text(&self.git.checkouts.page, "snapshot"),
        });
        cx.notify();
    }
    fn remove_worktree(&mut self, cx: &mut Context<Self>) {
        if !self.can_remove_worktree() {
            return;
        }
        let Some(review) = self.git.worktree_remove.clone() else {
            return;
        };
        if !self.worktree_review_current(&review.name, &review.snapshot) {
            return;
        }
        self.start_git_mutation(
            self.git_repository_id(),
            format!(
                "Remove {} {}",
                if review.missing {
                    "missing registration"
                } else {
                    "worktree"
                },
                review.name
            ),
            move |backend, server, repo, id| {
                backend.remove_worktree(
                    server,
                    repo,
                    &review.snapshot,
                    (&review.name, review.missing),
                    id,
                )
            },
            cx,
        );
    }
    pub(super) fn git_worktree_remove_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(review) = &self.git.worktree_remove else {
            return div().into_any_element();
        };
        div().flex().flex_col().items_start().gap_2().py_2()
            .child(crate::label(format!("Remove {}?",review.path)))
            .child(crate::label(if review.missing {"Remove only this missing worktree’s registration. Its branch is kept. Locate it instead if the directory was moved."}else{"Delete this checkout directory. Its branch is kept. Git will refuse if the checkout contains local changes or is locked. This cannot be undone in Newport."}).text_size(px(crate::ui::FONT_SIZE)))
            .child(project_dialog::actions()
                .child(crate::controls::button("cancel-worktree-removal").label("Cancel").disabled(self.git_mutation_blocks_write()).on_click(cx.listener(|this,_,_,cx|{this.git.worktree_remove=None;cx.notify();})))
                .child(crate::controls::button("confirm-worktree-removal").loading(self.git_write_pending()).when(!review.missing, |button|button.danger()).when(review.missing, |button|button.primary()).label(if review.missing {"Remove registration"}else{"Remove worktree"}).disabled(!self.can_remove_worktree() || !self.worktree_review_current(&review.name, &review.snapshot)).on_click(cx.listener(|this,_,_,cx|this.remove_worktree(cx)))))
            .into_any_element()
    }
}

impl Newport {
    pub(super) fn worktree_review_current(&self, name: &str, snapshot: &str) -> bool {
        self.git.checkouts.open
            && review_matches(
                name,
                snapshot,
                &self.git.checkouts.selected,
                &self.git.checkouts.page,
            )
    }
    pub(super) fn git_worktree_review_stale(&self) -> bool {
        self.git
            .worktree_lock
            .as_ref()
            .is_some_and(|form| !self.worktree_review_current(&form.name, &form.snapshot))
            || self
                .git
                .worktree_repair
                .as_ref()
                .is_some_and(|form| !self.worktree_review_current(&form.name, &form.snapshot))
            || self
                .git
                .worktree_remove
                .as_ref()
                .is_some_and(|form| !self.worktree_review_current(&form.name, &form.snapshot))
    }
}

impl LockForm {
    pub(super) fn initial_focus(&self, cx: &App) -> Option<FocusHandle> {
        (!self.unlock).then(|| self.reason.read(cx).focus_handle(cx))
    }
}
impl RepairForm {
    pub(super) fn initial_focus(&self, cx: &App) -> FocusHandle {
        self.path.read(cx).focus_handle(cx)
    }
}
