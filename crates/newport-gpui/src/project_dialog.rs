//! Project operations keep the workspace visible behind a Kit dialog.
use super::*;
use gpui_kit::component::input::InputEvent;

/// Static instructions must remain readable outside the custom paint tree.
/// Keep their full text in accessibility and let long names wrap in the modal.
pub(super) fn description(
    id: &'static str,
    content: impl Into<String>,
) -> impl IntoElement + Styled {
    let content = content.into();
    label(content.clone())
        .id(id)
        .role(accesskit::Role::Label)
        .aria_label(content)
        .test_support()
        .min_w_0()
        .text_size(px(crate::ui::FONT_SIZE))
        .line_height(px(18.))
}

/// Form action row. Callers add Cancel before the primary action
/// so keyboard order follows the same order as the visible controls.
pub(super) fn actions() -> Div {
    div()
        .w_full()
        .flex()
        .flex_wrap()
        .items_center()
        .justify_end()
        .gap(px(8.))
        .flex_shrink_0()
}

/// Form layout: space fields apart, keep each label close to its control.
pub(super) fn form() -> Div {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(16.))
        .px(px(20.))
        .pt(px(16.))
        .pb(px(20.))
}

pub(super) fn field(title: &'static str) -> Div {
    div()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(div().text_size(px(crate::ui::FONT_SIZE)).child(title))
}

/// Client validation belongs to the current form, not the backend resource.
/// Derive its message from current inputs after the first submit attempt so
/// correcting a field never leaves a stale error behind.
pub(super) fn validation_error(message: impl Into<String>, cx: &App) -> impl IntoElement {
    let message = message.into();
    div()
        .id("project-validation-error")
        .role(accesskit::Role::Alert)
        .aria_label(message.clone())
        .min_w_0()
        .text_size(px(crate::ui::FONT_SIZE))
        .line_height(px(18.))
        .text_color(crate::theme::error(cx.theme().is_dark()))
        .child(message)
}

fn subscribe_form_input<T: 'static>(
    input: &Entity<InputState>,
    ready: impl Fn(&T) -> bool + 'static,
    submit: fn(&mut T, &mut Context<T>),
    cx: &mut Context<T>,
) -> Subscription {
    cx.subscribe(input, move |this, _, event, cx| {
        if matches!(event, InputEvent::Change) {
            cx.notify();
        } else if matches!(
            event,
            InputEvent::PressEnter {
                secondary: false,
                shift: false
            }
        ) && ready(this)
        {
            submit(this, cx);
        }
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Folder,
    Add,
    Create,
    Edit,
    Branch,
    BranchManage,
    WorktreeCreate,
    Worktree,
    Tag,
    TagPush,
    Stash,
    Remote,
    RemoteReference,
    Settings,
    Transfer,
    Integration,
}
impl Kind {
    fn library_form(self) -> bool {
        matches!(self, Self::Add | Self::Create)
    }
    fn uses_git(self) -> bool {
        !matches!(self, Self::Folder | Self::Add | Self::Edit)
    }
    fn local_save_key(self) -> Option<&'static str> {
        match self {
            Self::Add => Some("project-save"),
            Self::Edit => Some("project-edit"),
            Self::Create => Some("project-created-save"),
            _ => None,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Folder => "Choose project folder",
            Self::Add => "Add repository",
            Self::Create => "Create project",
            Self::Edit => "Edit project",
            Self::Branch => "New branch",
            Self::BranchManage => "Manage branch",
            Self::WorktreeCreate => "New worktree",
            Self::Worktree => "Manage worktree",
            Self::Tag => "Manage tag",
            Self::TagPush => "Push tag",
            Self::Stash => "Manage stash",
            Self::Remote => "Edit remote",
            Self::RemoteReference => "Review remote reference",
            Self::Settings => "Repository settings",
            Self::Transfer => "Transfer changes",
            Self::Integration => "Review integration",
        }
    }
}
/// The operation present before this dialog opened belongs to the workspace,
/// not to the new draft. Keep blockers reachable, without relabelling an old
/// completed/failed operation as the result of the new form.
#[derive(Default)]
pub(super) struct FeedbackScope {
    previous_operation: Option<uuid::Uuid>,
}
impl FeedbackScope {
    fn new(previous_operation: Option<uuid::Uuid>) -> Self {
        Self { previous_operation }
    }
    fn owns(&self, current: Option<uuid::Uuid>) -> bool {
        current.is_some() && current != self.previous_operation
    }
    fn busy(&self, kind: Kind, current: Option<uuid::Uuid>, pending: bool, saving: bool) -> bool {
        saving || (kind.uses_git() && self.owns(current) && pending)
    }
}
pub(super) fn scroll_body(body: AnyElement, viewport_height: Pixels) -> AnyElement {
    div()
        .id("project-dialog-scroll")
        .max_h((viewport_height - px(140.)).max(px(120.)))
        .overflow_y_scroll()
        // Let focus rings paint beyond full-width fields without moving the
        // form's alignment or disabling vertical scrolling in short windows.
        .mx(-px(4.))
        .px(px(4.))
        .child(body)
        .into_any_element()
}
impl Newport {
    fn project_dialog_title(&self) -> String {
        let Some(kind) = self.project_dialog_kind() else {
            return "Project operation".into();
        };
        let title = match kind {
            Kind::Create => self
                .project_create
                .as_ref()
                .map(project_create::Create::title),
            Kind::Edit => self.project_edit.as_ref().map(project_library::Edit::title),
            Kind::Branch => self.git.branch_form.as_ref().map(git::BranchForm::title),
            Kind::BranchManage => self
                .git
                .branch_manage
                .as_ref()
                .map(git_branch_manage::Form::title),
            Kind::Worktree => self
                .git
                .worktree_lock
                .as_ref()
                .map(git_worktrees::LockForm::title)
                .or(Some(if self.git.worktree_repair.is_some() {
                    "Locate moved worktree"
                } else {
                    "Remove worktree"
                })),
            Kind::Tag => Some(if self.git.tag_delete.is_some() {
                "Delete local tag"
            } else {
                "New tag"
            }),
            Kind::Stash => return self.stash_dialog_title(),
            Kind::RemoteReference => return self.remote_ref_dialog_title(),
            Kind::Remote => self
                .git
                .remote_edit
                .as_ref()
                .map(git_remote_edit::Form::title),
            Kind::Transfer => self.git.pull_form.as_ref().map(git_pull::Form::title),
            Kind::Integration => {
                return self
                    .git
                    .integration_review
                    .as_ref()
                    .map(|r| r.title.clone())
                    .unwrap_or_else(|| kind.label().into());
            }
            _ => None,
        };
        title.unwrap_or(kind.label()).into()
    }
    /// Single-line fields submit the form on Enter. Multiline editors and
    /// destructive confirmation buttons deliberately use their own handlers.
    pub(super) fn project_form_input(
        input: &Entity<InputState>,
        kind: Kind,
        submit: fn(&mut Self, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> Subscription {
        subscribe_form_input(
            input,
            move |this: &Self| {
                this.project_dialog_open
                    && this.project_dialog_kind() == Some(kind)
                    && !this.project_dialog_busy()
            },
            submit,
            cx,
        )
    }

    fn cancel_project_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.project_dialog_busy() {
            return false;
        }
        if self.project_folder.is_some() {
            self.return_from_project_folder(window, cx);
            return false;
        }
        true
    }
    fn project_dialog_kind(&self) -> Option<Kind> {
        if self.page != Page::Projects {
            return None;
        }
        if self.project_folder.is_some() {
            Some(Kind::Folder)
        } else if self.project_add.is_some() {
            Some(Kind::Add)
        } else if self.project_create.is_some() {
            Some(Kind::Create)
        } else if self.project_edit.is_some() {
            Some(Kind::Edit)
        } else if self.git.branch_form.is_some() {
            Some(Kind::Branch)
        } else if self.git.branch_manage.is_some() {
            Some(Kind::BranchManage)
        } else if self.git.worktree_create.is_some() {
            Some(Kind::WorktreeCreate)
        } else if self.git.worktree_lock.is_some()
            || self.git.worktree_repair.is_some()
            || self.git.worktree_remove.is_some()
        {
            Some(Kind::Worktree)
        } else if self.git.tag_form.is_some() || self.git.tag_delete.is_some() {
            Some(Kind::Tag)
        } else if self.git.tag_push.is_some() {
            Some(Kind::TagPush)
        } else if self.git.stash_save.is_some() || self.git.stash_apply.is_some() {
            Some(Kind::Stash)
        } else if self.git.remote_edit.is_some() {
            Some(Kind::Remote)
        } else if self.git.remote_ref_review.is_some() {
            Some(Kind::RemoteReference)
        } else if self.git.settings_open || self.git.settings.is_some() {
            Some(Kind::Settings)
        } else if self.git.pull_form.is_some() {
            Some(Kind::Transfer)
        } else if self.git.integration_review.is_some() {
            Some(Kind::Integration)
        } else {
            None
        }
    }
    fn project_dialog_busy(&self) -> bool {
        let Some(kind) = self.project_dialog_kind() else {
            return false;
        };
        let saving = kind
            .local_save_key()
            .is_some_and(|key| self.resources.get(key).is_some_and(|r| r.pending));
        self.project_dialog_feedback.busy(
            kind,
            self.git_mutation.as_ref().map(git_mutation::Mutation::id),
            self.git_write_pending(),
            saving,
        )
    }
    fn dismiss_project_dialog(&mut self, cx: &mut Context<Self>) {
        self.close_project_folder();
        self.close_checkout_picker();
        self.git_branch_picker_close();
        self.project_add = None;
        self.project_create = None;
        self.project_edit = None;
        self.git.branch_form = None;
        self.git.branch_manage = None;
        self.git.worktree_create = None;
        self.git.worktree_lock = None;
        self.git.worktree_repair = None;
        self.git.worktree_remove = None;
        self.git.tag_form = None;
        self.git.tag_delete = None;
        self.git.tag_push = None;
        self.git.stash_save = None;
        self.git.stash_apply = None;
        self.git.remote_edit = None;
        self.git.remote_ref_review = None;
        self.git.settings = None;
        self.git.settings_open = false;
        self.git.pull_form = None;
        self.git.sync_form = None;
        self.git.integration_review = None;
        self.project_dialog_open = false;
        cx.notify();
    }
    pub(super) fn sync_project_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project_dialog_open == self.project_dialog_kind().is_some() {
            return;
        }
        let owner = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            let _ = owner.update(cx, |this, cx| {
                if this.project_dialog_kind().is_none() {
                    if this.project_dialog_open {
                        this.project_dialog_open = false;
                        window.close_dialog(cx);
                    }
                    return;
                }
                if this.project_dialog_open || window.has_active_dialog(cx) {
                    return;
                }
                this.project_dialog_open = true;
                this.project_dialog_feedback =
                    FeedbackScope::new(this.git_mutation.as_ref().map(git_mutation::Mutation::id));
                let owner = cx.weak_entity();
                let focus_owner = owner.clone();
                window.open_dialog(cx, move |dialog, window, cx| {
                    let cancel = owner.clone();
                    let close = owner.clone();
                    let (busy, folder, library_form) = owner
                        .update(cx, |this, _| {
                            let folder = this.project_dialog_kind() == Some(Kind::Folder);
                            (
                                this.project_dialog_busy(),
                                folder,
                                this.project_dialog_kind().is_some_and(Kind::library_form),
                            )
                        })
                        .unwrap_or((false, false, false));
                    dialog
                        .w(px(if folder {
                            680.
                        } else if library_form {
                            480.
                        } else {
                            580.
                        }))
                        .max_w((window.viewport_size().width - px(32.)).max(px(240.)))
                        .centered(true)
                        .rounded(px(crate::ui::RADIUS))
                        .border_0()
                        .p_4()
                        .when(library_form, |dialog| dialog.p_0())
                        .overlay_closable(false)
                        .keyboard(!busy)
                        .close_button(!busy && !library_form)
                        .on_cancel(move |_, window, cx| {
                            cancel
                                .update(cx, |this, cx| this.cancel_project_dialog(window, cx))
                                .unwrap_or(true)
                        })
                        .on_close(move |_, _, cx| {
                            let _ = close.update(cx, |this, cx| this.dismiss_project_dialog(cx));
                        })
                        .on_ok(|_, _, _| false)
                        .content({
                            let owner = owner.clone();
                            move |content, window, cx| {
                                let body = owner
                                    .update(cx, |this, cx| {
                                        this.project_dialog_body(window.viewport_size(), cx)
                                    })
                                    .unwrap_or_else(|_| div().into_any_element());
                                content.child(scroll_body(body, window.viewport_size().height))
                            }
                        })
                });
                window.on_next_frame(move |window, cx| {
                    let _ = focus_owner.update(cx, |this, cx| {
                        if this.project_dialog_open
                            && let Some(focus) = this.project_initial_focus(cx)
                        {
                            focus.focus(window, cx);
                        }
                    });
                });
                cx.notify();
            });
        });
    }
    fn project_initial_focus(&self, cx: &App) -> Option<FocusHandle> {
        match self.project_dialog_kind()? {
            Kind::Add => self.project_add.as_ref().map(|f| f.initial_focus(cx)),
            Kind::Create => self.project_create.as_ref().map(|f| f.initial_focus(cx)),
            Kind::Edit => self.project_edit.as_ref().and_then(|f| f.initial_focus(cx)),
            Kind::Branch => self
                .git
                .branch_form
                .as_ref()
                .map(|f| f.name.read(cx).focus_handle(cx)),
            Kind::WorktreeCreate => self
                .git
                .worktree_create
                .as_ref()
                .map(|f| f.initial_focus(cx)),
            Kind::Worktree => self
                .git
                .worktree_lock
                .as_ref()
                .and_then(|f| f.initial_focus(cx))
                .or_else(|| {
                    self.git
                        .worktree_repair
                        .as_ref()
                        .map(|f| f.initial_focus(cx))
                }),
            Kind::Tag => self.git.tag_form.as_ref().map(|f| f.initial_focus(cx)),
            Kind::Stash => self.git.stash_save.as_ref().map(|f| f.initial_focus(cx)),
            Kind::Remote => self
                .git
                .remote_edit
                .as_ref()
                .and_then(|f| f.initial_focus(cx)),
            Kind::Settings => self.git.settings.as_ref().map(|f| f.initial_focus(cx)),
            Kind::Transfer => self.git.pull_form.as_ref().map(|f| f.initial_focus(cx)),
            Kind::Integration => self
                .git
                .integration_review
                .as_ref()
                .and_then(|f| f.message.as_ref())
                .map(|f| f.read(cx).focus_handle(cx)),
            _ => None,
        }
    }
    fn project_dialog_body(
        &mut self,
        viewport: Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(kind) = self.project_dialog_kind() else {
            return div().into_any_element();
        };
        self.project_dialog_rendering = true;
        let body = match kind {
            Kind::Folder => div()
                .h(px(420.))
                .flex()
                .flex_col()
                .child(self.project_folder_view(cx))
                .into_any_element(),
            Kind::Add => self.project_add_view(cx),
            Kind::Create => self.project_create_view(cx),
            Kind::Edit => self.project_edit_view(cx),
            Kind::Branch => self.git_branch_form(cx),
            Kind::BranchManage => self.branch_manage_view(cx),
            Kind::WorktreeCreate => {
                git_listing::modal_editor("New worktree", self.git_worktree_create_controls(cx), cx)
            }
            Kind::Worktree => self
                .git_worktree_review_editor(cx)
                .unwrap_or_else(|| div().into_any_element()),
            Kind::Tag => self.git_tag_editor(cx),
            Kind::TagPush => self.git_tag_push_editor(cx),
            Kind::Stash => self.git_stash_editor(cx),
            Kind::Remote => self.git_remote_edit_view(cx),
            Kind::RemoteReference => self.remote_ref_actions_view(cx),
            Kind::Settings => self.repository_settings_dialog(cx),
            Kind::Transfer => self.git_pull_view(cx),
            Kind::Integration => {
                if self
                    .git
                    .integration_review
                    .as_ref()
                    .is_some_and(|r| r.is_branch_picker())
                {
                    self.git_branch_integration_view(cx)
                } else {
                    self.git_integration_view(cx)
                }
            }
        };
        self.project_dialog_rendering = false;
        div()
            .flex()
            .flex_col()
            .min_w_0()
            .gap_2()
            .when(kind.library_form(), |view| {
                let compact = viewport.width <= px(540.) || viewport.height <= px(560.);
                view.gap_0().child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.))
                        .px(px(if compact { 18. } else { 24. }))
                        .pt(px(if compact { 14. } else { 20. }))
                        .pb(px(if compact { 0. } else { 4. }))
                        .child(
                            crate::controls::dialog_title(self.project_dialog_title())
                                .id("project-form-heading")
                                .role(accesskit::Role::Heading)
                                .aria_label(self.project_dialog_title())
                                .min_w_0()
                                .flex_1(),
                        )
                        .child(
                            crate::controls::icon_button("project-form-close")
                                .ghost()
                                .icon(IconName::Close)
                                .tooltip("Close dialog")
                                .accessibility_label("Close dialog")
                                .disabled(self.project_dialog_busy())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    if !this.project_dialog_busy() {
                                        this.dismiss_project_dialog(cx);
                                        window.close_dialog(cx);
                                    }
                                })),
                        ),
                )
            })
            .when_some(
                self.git_mutation
                    .as_ref()
                    .filter(|mutation| {
                        kind.uses_git() && self.project_dialog_feedback.owns(Some(mutation.id()))
                    })
                    .and_then(git_mutation::Mutation::dialog_error),
                |view, message| {
                    view.child(
                        div()
                            .id("project-form-error")
                            .role(accesskit::Role::Alert)
                            .aria_label(message.clone())
                            .text_color(crate::theme::error(cx.theme().is_dark()))
                            .when(kind.library_form(), |error| error.px(px(20.)))
                            .child(message),
                    )
                },
            )
            .child(body)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{FeedbackScope, Kind, subscribe_form_input};
    use gpui_kit::component::button::ButtonVariants;
    use gpui_kit::component::input::InputState;
    use gpui_kit::{
        self as gpui, AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _,
        IntoElement, ParentElement as _, Render, Styled as _, Subscription, TestAppContext, Window,
        div,
    };
    use uuid::Uuid;
    struct LibraryForm;
    impl Render for LibraryForm {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            super::form()
                .child(super::field("Project name").child(
                    div().debug_selector(|| "name-input".into()).h(gpui::px(28.)),
                ))
                .child(super::field("Repository path on server")
                    .child(div().debug_selector(|| "path-input".into()).h(gpui::px(28.)))
                    .child(super::description("path-hint", "Choose an existing directory without a Git repository. Its files will be kept.")))
                .child(super::validation_error("Enter an absolute repository path on this server.", cx))
                .child(super::actions().debug_selector(|| "form-actions".into())
                    .child(crate::controls::form_button("cancel").label("Cancel").min_w(gpui::px(76.)))
                    .child(crate::controls::form_button("submit").primary().label("Add repository").min_w(gpui::px(76.))))
        }
    }
    #[gpui::test]
    fn library_form_keeps_controls_aligned_and_wraps_feedback_at_compact_width(
        cx: &mut TestAppContext,
    ) {
        use gpui::{px, size};
        cx.update(|cx| {
            gpui::init(cx);
            crate::theme::install(false, cx);
        });
        let (_, visual) = cx.add_window_view(|_, _| LibraryForm);
        let mut wide_footer = None;
        for width in [480., 300.] {
            visual.simulate_resize(size(px(width), px(600.)));
            visual.update(|window, cx| window.draw(cx).clear(cx));
            let name = visual.debug_bounds("name-input").unwrap();
            let path = visual.debug_bounds("path-input").unwrap();
            let actions = visual.debug_bounds("form-actions").unwrap();
            assert_eq!(name.left(), px(20.));
            assert_eq!(name.right(), px(width - 20.));
            assert_eq!(path.left(), name.left());
            assert_eq!(path.right(), name.right());
            assert_eq!(name.size.height, px(28.));
            assert_eq!(path.size.height, px(28.));
            assert!(path.top() > name.bottom());
            assert!(actions.top() > path.bottom());
            assert_eq!(actions.right(), path.right());
            if let Some(top) = wide_footer {
                assert!(actions.top() > top);
            } else {
                wide_footer = Some(actions.top());
            }
        }
    }
    struct Footer {
        cancel: gpui::FocusHandle,
        submit: gpui::FocusHandle,
    }
    impl Render for Footer {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            super::actions()
                .child(
                    div()
                        .debug_selector(|| "cancel-action".into())
                        .flex_shrink_0()
                        .child(
                            crate::controls::button("cancel")
                                .label("Cancel")
                                .track_focus(&self.cancel),
                        ),
                )
                .child(
                    div()
                        .debug_selector(|| "submit-action".into())
                        .flex_shrink_0()
                        .child(
                            crate::controls::button("submit")
                                .label("Create worktree")
                                .track_focus(&self.submit),
                        ),
                )
        }
    }
    #[gpui::test]
    fn modal_actions_align_right_wrap_without_overflow_and_follow_visual_focus_order(
        cx: &mut TestAppContext,
    ) {
        use gpui::{px, size};
        cx.update(|cx| {
            gpui::init(cx);
            crate::theme::install(false, cx);
        });
        let (view, visual) = cx.add_window_view(|_, cx| Footer {
            cancel: cx.focus_handle(),
            submit: cx.focus_handle(),
        });
        visual.simulate_resize(size(px(500.), px(160.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let cancel = visual.debug_bounds("cancel-action").unwrap();
        let submit = visual.debug_bounds("submit-action").unwrap();
        assert_eq!(cancel.top(), submit.top());
        assert_eq!(cancel.right() + px(8.), submit.left());
        assert_eq!(submit.right(), px(500.));
        assert_eq!(submit.size.height, px(crate::controls::CONTROL_HEIGHT));
        visual.update(|window, cx| {
            window.focus_next(cx);
            assert!(view.read(cx).cancel.is_focused(window));
            window.focus_next(cx);
            assert!(view.read(cx).submit.is_focused(window));
            window.focus_prev(cx);
            assert!(view.read(cx).cancel.is_focused(window));
        });
        visual.simulate_resize(size(px(150.), px(160.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let cancel = visual.debug_bounds("cancel-action").unwrap();
        let submit = visual.debug_bounds("submit-action").unwrap();
        assert!(submit.top() >= cancel.bottom() + px(8.));
        assert_eq!(cancel.right(), px(150.));
        assert_eq!(submit.right(), px(150.));
        assert!(cancel.left() >= px(0.) && submit.left() >= px(0.));
    }
    #[test]
    fn new_dialog_does_not_inherit_previous_completion_but_keeps_its_own_failure() {
        let old = Some(Uuid::new_v4());
        let submitted = Some(Uuid::new_v4());
        let scope = FeedbackScope::new(old);
        assert!(!scope.owns(old));
        assert!(!scope.owns(None));
        assert!(scope.owns(submitted));
        for kind in [
            Kind::Create,
            Kind::Branch,
            Kind::Worktree,
            Kind::Tag,
            Kind::Stash,
            Kind::Remote,
            Kind::Settings,
            Kind::Transfer,
        ] {
            assert!(scope.busy(kind, submitted, true, false));
            assert!(!scope.busy(kind, submitted, false, false));
            // Reopening after completion captures a new boundary.
        }
    }
    #[test]
    fn unrelated_operations_do_not_trap_a_dialog() {
        let existing = Some(Uuid::new_v4());
        let scope = FeedbackScope::new(existing);
        assert!(!scope.busy(Kind::Worktree, existing, true, false));
        for kind in [Kind::Folder, Kind::Add, Kind::Edit] {
            assert!(!scope.busy(kind, existing, true, false));
        }
        assert!(scope.busy(Kind::Add, existing, true, true));
        assert_eq!(Kind::Add.local_save_key(), Some("project-save"));
        assert_eq!(Kind::Edit.local_save_key(), Some("project-edit"));
        assert_eq!(Kind::Create.local_save_key(), Some("project-created-save"));
        assert_eq!(Kind::Folder.local_save_key(), None);
    }

    struct FormHost {
        input: Entity<InputState>,
        submissions: usize,
        active: bool,
        pending: bool,
        _subscription: Subscription,
    }
    impl Render for FormHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().p_4().child(crate::controls::input(&self.input))
        }
    }
    #[gpui::test]
    fn single_line_enter_submits_once_and_ignores_hidden_or_busy_forms(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui::init(cx);
            crate::theme::install(false, cx);
        });
        let mut host = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                let input = cx.new(|cx| InputState::new(window, cx));
                let subscription = subscribe_form_input(
                    &input,
                    |this: &FormHost| this.active && !this.pending,
                    |this, cx| {
                        this.submissions += 1;
                        this.pending = true;
                        cx.notify();
                    },
                    cx,
                );
                input.read(cx).focus_handle(cx).focus(window, cx);
                FormHost {
                    input,
                    submissions: 0,
                    active: true,
                    pending: false,
                    _subscription: subscription,
                }
            });
            host = Some(view.clone());
            gpui::component::Root::new(view, window, cx)
        });
        let host = host.unwrap();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_keystrokes("shift-enter");
        cx.run_until_parked();
        host.read_with(cx, |this, _| assert_eq!(this.submissions, 0));
        cx.simulate_keystrokes("enter enter");
        cx.run_until_parked();
        host.read_with(cx, |this, _| assert_eq!(this.submissions, 1));
        host.update(cx, |this, _| {
            this.pending = false;
            this.active = false;
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        host.read_with(cx, |this, _| assert_eq!(this.submissions, 1));
    }
}
