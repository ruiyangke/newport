//! Modal editor for an existing repository bookmark; no remote Git write.
use super::*;
use newport_client::project_bookmark::add_arguments;
pub(super) struct Add {
    name: Entity<InputState>,
    pub path: Entity<InputState>,
    attempted: bool,
    _subscriptions: Vec<Subscription>,
}
impl Newport {
    pub(super) fn project_add_pending(&self) -> bool {
        self.resources
            .get("project-save")
            .is_some_and(|resource| resource.pending)
    }
    pub(super) fn begin_project_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project_add.is_some()
            || self.project_create.is_some()
            || self.project_edit.is_some()
            || self.project_add_pending()
        {
            return;
        }
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Project name"));
        crate::controls::focus_editor(&name, window, cx);
        let path = cx.new(|cx| InputState::new(window, cx).placeholder("Absolute repository path"));
        let subscriptions = [&name, &path]
            .into_iter()
            .map(|input| {
                Self::project_form_input(
                    input,
                    project_dialog::Kind::Add,
                    Self::add_existing_project,
                    cx,
                )
            })
            .collect();
        self.resources.invalidate("project-save");
        self.project_checks = None;
        self.project_add = Some(Add {
            name,
            path,
            attempted: false,
            _subscriptions: subscriptions,
        });
        cx.notify();
    }
    fn add_existing_project(&mut self, cx: &mut Context<Self>) {
        if self.project_add_pending() {
            return;
        }
        let Some(form) = self.project_add.as_mut() else {
            return;
        };
        form.attempted = true;
        cx.notify();
        let Ok(args) = add_arguments(
            &self.server,
            &form.name.read(cx).value(),
            &form.path.read(cx).value(),
        ) else {
            return;
        };
        self.request("project-save", "native.add_project", args);
        cx.notify();
    }
    pub(super) fn project_add_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.project_add else {
            return div().into_any_element();
        };
        let pending = self.project_add_pending();
        let validation = add_arguments(
            &self.server,
            &form.name.read(cx).value(),
            &form.path.read(cx).value(),
        );
        project_dialog::form()
            .child(
                project_dialog::field("Project name").child(
                    crate::controls::input(&form.name)
                        .aria_label("Project name")
                        .disabled(pending),
                ),
            )
            .child(
                project_dialog::field("Repository path on server").child(
                    div()
                        .flex()
                        .gap_2()
                        .items_center()
                        .child(
                            div().flex_1().min_w_0().child(
                                crate::controls::input(&form.path)
                                    .aria_label("Repository path")
                                    .disabled(pending),
                            ),
                        )
                        .child(
                            crate::controls::button("browse-existing-project")
                                .label("Browse…")
                                .disabled(pending)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    if !this.project_add_pending()
                                        && let Some(form) = &this.project_add
                                    {
                                        this.begin_project_folder(
                                            form.path.clone(),
                                            false,
                                            window,
                                            cx,
                                        );
                                    }
                                })),
                        ),
                ),
            )
            .when_some(
                validation.err().filter(|_| form.attempted),
                |view, error| view.child(project_dialog::validation_error(error, cx)),
            )
            .when(
                self.resources
                    .get("project-save")
                    .is_some_and(|r| r.pending || r.error.is_some()),
                |view| view.child(self.status("project-save", cx)),
            )
            .child(
                project_dialog::actions()
                    .child(
                        crate::controls::form_button("cancel-add-project")
                            .label("Cancel")
                            .min_w(px(76.))
                            .disabled(pending)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this.project_add_pending() {
                                    this.project_add = None;
                                    this.resources.invalidate("project-save");
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        crate::controls::form_button("add-project")
                            .primary()
                            .min_w(px(76.))
                            .loading(pending)
                            .label("Add repository")
                            .disabled(pending)
                            .on_click(cx.listener(|this, _, _, cx| this.add_existing_project(cx))),
                    ),
            )
            .into_any_element()
    }
}

impl Add {
    pub(super) fn initial_focus(&self, cx: &App) -> FocusHandle {
        self.name.read(cx).focus_handle(cx)
    }
}
