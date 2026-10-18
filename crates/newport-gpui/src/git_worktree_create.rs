use super::*;
use gpui_kit::component::tab::Tab;

/// Pending and failed validation share the field's hint slot. Long errors wrap
/// rather than adding a second row below an empty skeleton placeholder.
fn review_hint(
    name: &'static str,
    review: &newport_client::git_worktree::NameReview,
    value: &str,
    retry: impl IntoElement,
    cx: &App,
) -> AnyElement {
    let pending = review.pending(value);
    div()
        .w_full()
        .min_w_0()
        .min_h(px(24.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_2()
        .when(pending, |view| {
            view.child(project_loading::picker_detail(name, None, true))
        })
        .when_some(review.error().filter(|_| !pending), |view, error| {
            view.child(
                crate::project_ui::label(error.to_owned())
                    .id(SharedString::from(format!("{name}-error")))
                    .role(accesskit::Role::Alert)
                    .aria_label(error.to_owned())
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(crate::project_ui::FONT_SIZE))
                    .opacity(1.)
                    .text_color(cx.theme().danger),
            )
            .child(retry)
        })
        .into_any_element()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FormOrigin {
    BranchPicker,
    CheckoutPicker,
}
impl FormOrigin {
    fn resource_key(self) -> &'static str {
        match self {
            Self::BranchPicker => "git-worktree-list",
            Self::CheckoutPicker => "git-checkouts",
        }
    }
    fn active(self, _tab: usize, picker_open: bool) -> bool {
        match self {
            Self::BranchPicker | Self::CheckoutPicker => picker_open,
        }
    }
    fn review_matches(self, tab: usize, picker_open: bool, snapshot: &str, page: &Value) -> bool {
        self.active(tab, picker_open)
            && !snapshot.is_empty()
            && page["snapshot"].as_str() == Some(snapshot)
    }
}
pub(super) struct Form {
    id: uuid::Uuid,
    origin: FormOrigin,
    path: Entity<InputState>,
    name: Entity<InputState>,
    branch: Entity<InputState>,
    picker: Option<Entity<super::git_worktree_picker::WorktreeBranchPicker>>,
    base_picker: Option<Entity<super::git_worktree_picker::WorktreeBranchPicker>>,
    new_branch: bool,
    locked: bool,
    open_after: bool,
    oid: String,
    existing_branch: Option<String>,
    snapshot: String,
    main_root: String,
    suggested_name: newport_client::git_worktree::SuggestedField,
    suggested_path: newport_client::git_worktree::SuggestedField,
    name_review: newport_client::git_worktree::NameReview,
    name_check_task: Option<Task<()>>,
    branch_review: newport_client::git_worktree::NameReview,
    branch_check_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Draft {
    id: uuid::Uuid,
    name: String,
    path: String,
    branch: String,
    oid: String,
    new_branch: bool,
    locked: bool,
    open_after: bool,
}
impl Draft {
    pub(super) fn open_destination(&self) -> Option<Value> {
        use base64::Engine as _;
        self.open_after.then(|| json!({"display":self.path,"bytesB64":base64::engine::general_purpose::STANDARD.encode(&self.path)}))
    }
    fn target(&self) -> newport_client::git_worktree::CreateTarget<'_> {
        newport_client::git_worktree::CreateTarget {
            name: &self.name,
            path: &self.path,
            branch: &self.branch,
            oid: &self.oid,
            new_branch: self.new_branch,
            locked: self.locked,
        }
    }
}
impl Form {
    fn names_ready(&self, draft: &Draft) -> bool {
        self.name_review.ready(&draft.name)
            && (!draft.new_branch || self.branch_review.ready(&draft.branch))
    }
    pub(super) fn is_checkout_picker(&self) -> bool {
        self.origin == FormOrigin::CheckoutPicker
    }
    pub(super) fn is_branch_picker(&self) -> bool {
        self.origin == FormOrigin::BranchPicker
    }
    pub(super) fn draft(&self, cx: &App) -> Draft {
        let (branch, oid) = if let Some(branch) = &self.existing_branch {
            (branch.clone(), self.oid.clone())
        } else if self.new_branch {
            (
                self.branch.read(cx).value().trim().to_string(),
                self.base_picker
                    .as_ref()
                    .and_then(|picker| picker.read(cx).branch())
                    .map(|branch| branch.oid.clone())
                    .unwrap_or_else(|| self.oid.clone()),
            )
        } else {
            self.picker
                .as_ref()
                .and_then(|picker| picker.read(cx).branch())
                .map(|b| (b.name.clone(), b.oid.clone()))
                .unwrap_or_default()
        };
        let name = self.suggested_name.value(
            &self.name.read(cx).value(),
            newport_client::git_worktree::suggested_name(&branch),
        );
        let path = self.suggested_path.value(
            &self.path.read(cx).value(),
            newport_client::git_worktree::suggested_path(&self.main_root, &name),
        );
        Draft {
            id: self.id,
            name,
            path,
            branch,
            oid,
            new_branch: self.existing_branch.is_none() && self.new_branch,
            locked: self.locked,
            open_after: self.open_after,
        }
    }
}

fn owner_path(branch: &Value, page: &Value) -> Option<Value> {
    use base64::Engine as _;
    let (name, _) = super::git::local_branch_target(branch)?;
    let owner = page["entries"].as_array()?.first()?;
    let expected = base64::engine::general_purpose::STANDARD.encode(format!("refs/heads/{name}"));
    if owner["head"]["name"]["bytesB64"] != expected {
        return None;
    }
    super::git::worktree_path(owner)
}
impl Newport {
    pub(super) fn git_branch_worktree_unoccupied(&self) -> bool {
        self.resources
            .get("git-branch-worktree-owner")
            .is_some_and(|r| !r.pending && r.error.is_none())
            && self.value("git-branch-worktree-owner")["entries"]
                .as_array()
                .is_some_and(Vec::is_empty)
    }
    pub(super) fn branch_owner_path(&self) -> Option<Value> {
        if !self.git.branches.open
            || self.git.branches.error.is_some()
            || !self
                .resources
                .get("git-branch-worktree-owner")
                .is_some_and(|r| !r.pending && r.error.is_none())
        {
            return None;
        }
        owner_path(
            &self.git.branches.selected,
            &self.value("git-branch-worktree-owner"),
        )
    }
    pub(super) fn open_branch_worktree(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.branch_owner_path() {
            self.git_open_worktree_path(path, cx);
        }
    }

    pub(super) fn git_branch_worktree_owner(&mut self) {
        self.resources.invalidate("git-branch-worktree-owner");
        if !self.git.branches.open {
            return;
        }
        let Some((branch, _)) = super::git::local_branch_target(&self.git.branches.selected) else {
            return;
        };
        self.request("git-branch-worktree-owner","git_request",json!({"serverId":self.server,"request":{"method":"repo.worktrees","params":{"repoId":self.git.repo["repoId"],"pageSize":1,"branch":format!("refs/heads/{branch}")}}}));
    }
    pub(super) fn git_branch_worktree_hint(&self, cx: &mut Context<Self>) -> AnyElement {
        let page = self.value("git-branch-worktree-owner");
        let owner = page["entries"].as_array().and_then(|rows| rows.first());
        div()
            .flex()
            .flex_col()
            .items_start()
            .gap_2()
            .px_4()
            .child(self.status("git-branch-worktree-owner", cx))
            .when_some(owner, |view, owner| {
                view.child(
                    crate::project_ui::label(format!(
                        "Already checked out in {}",
                        text(&owner["path"], "display")
                    ))
                    .text_size(px(crate::project_ui::FONT_SIZE)),
                )
                .child(
                    crate::project_ui::button("open-branch-worktree")
                        .label("Open existing worktree")
                        .disabled(self.branch_owner_path().is_none())
                        .on_click(cx.listener(|this, _, _, cx| this.open_branch_worktree(cx))),
                )
            })
            .into_any_element()
    }

    fn worktree_creation_page(&self, origin: FormOrigin) -> Value {
        match origin {
            FormOrigin::BranchPicker => self.value(origin.resource_key()),
            FormOrigin::CheckoutPicker => self.git.checkouts.page.clone(),
        }
    }
    pub(super) fn can_create_branch_worktree(&self) -> bool {
        self.git.branches.open
            && !self.git_branch_review_open()
            && self.can_create_worktree_from(FormOrigin::BranchPicker)
            && !self.git_worktree_review_open()
            && self.git_branch_worktree_unoccupied()
            && self.git.branches.selected["current"] == false
            && super::git::local_branch_target(&self.git.branches.selected).is_some()
            && self
                .resources
                .get("git-branches")
                .is_some_and(|r| !r.pending && r.error.is_none())
    }
    pub(super) fn new_branch_worktree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_create_branch_worktree() {
            return;
        }
        let Some((branch, oid)) = super::git::local_branch_target(&self.git.branches.selected)
        else {
            return;
        };
        self.new_worktree_from(FormOrigin::BranchPicker, window, cx);
        if let Some(form) = self.git.worktree_create.as_mut() {
            form.oid = oid;
            form.existing_branch = Some(branch);
        }
    }
    fn worktree_origin_open(&self, origin: FormOrigin) -> bool {
        if origin == FormOrigin::CheckoutPicker {
            self.git.checkouts.open
        } else {
            self.git.branches.open
        }
    }
    pub(super) fn can_create_picker_worktree(&self) -> bool {
        self.can_create_worktree_from(FormOrigin::CheckoutPicker)
    }
    pub(super) fn new_picker_worktree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_worktree_review_open() {
            return;
        }
        self.new_worktree_from(FormOrigin::CheckoutPicker, window, cx);
    }
    fn can_create_worktree_from(&self, origin: FormOrigin) -> bool {
        origin.active(self.git.tab, self.worktree_origin_open(origin))
            && !self.git_mutation_blocks_write()
            && self.git.repo["capabilities"]["readOnly"] == false
            && if origin == FormOrigin::BranchPicker {
                self.git.branches.error.is_none()
            } else {
                self.git.error.is_none()
            }
            && self
                .resources
                .get(origin.resource_key())
                .is_some_and(|r| !r.pending && r.error.is_none())
            && self.worktree_creation_page(origin)["snapshot"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
    }
    fn new_worktree_from(
        &mut self,
        origin: FormOrigin,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.git_worktree_review_open() || !self.can_create_worktree_from(origin) {
            return;
        }
        let page = self.worktree_creation_page(origin);
        let path = cx.new(|cx| InputState::new(window, cx).placeholder("Absolute remote path"));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Worktree name"));
        crate::controls::focus_editor(&name, window, cx);
        let branch = cx.new(|cx| InputState::new(window, cx).placeholder("New branch name"));
        let picker = (origin != FormOrigin::BranchPicker).then(|| {
            cx.new(|cx| {
                super::git_worktree_picker::WorktreeBranchPicker::new(
                    self.server.clone(),
                    self.git_repository_id(),
                    self.git.repo["bare"] == true,
                    newport_client::git_branch::WorktreeBranchUse::Existing,
                    window,
                    cx,
                )
            })
        });
        let base_picker = (origin != FormOrigin::BranchPicker).then(|| {
            cx.new(|cx| {
                super::git_worktree_picker::WorktreeBranchPicker::new(
                    self.server.clone(),
                    self.git_repository_id(),
                    self.git.repo["bare"] == true,
                    newport_client::git_branch::WorktreeBranchUse::StartingPoint,
                    window,
                    cx,
                )
            })
        });
        let mut subscriptions = vec![
            Self::project_form_input(
                &path,
                project_dialog::Kind::WorktreeCreate,
                Self::create_worktree,
                cx,
            ),
            Self::project_form_input(
                &name,
                project_dialog::Kind::WorktreeCreate,
                Self::create_worktree,
                cx,
            ),
            Self::project_form_input(
                &branch,
                project_dialog::Kind::WorktreeCreate,
                Self::create_worktree,
                cx,
            ),
        ];
        if let Some(picker) = &picker {
            subscriptions.push(cx.observe(picker, |_, _, cx| cx.notify()));
        }
        if let Some(picker) = &base_picker {
            subscriptions.push(cx.observe(picker, |_, _, cx| cx.notify()));
        }
        self.git.worktree_create = Some(Form {
            id: uuid::Uuid::new_v4(),
            origin,
            path,
            name,
            branch,
            picker,
            base_picker,
            new_branch: true,
            locked: false,
            open_after: true,
            oid: text(&page["metadata"]["current"]["head"]["oid"], "hex"),
            existing_branch: None,
            snapshot: text(&page, "snapshot"),
            main_root: page["metadata"]["main"]["path"]["display"]
                .as_str()
                .unwrap_or_else(|| {
                    self.git.repo["root"]["display"]
                        .as_str()
                        .unwrap_or_default()
                })
                .to_owned(),
            suggested_name: Default::default(),
            suggested_path: Default::default(),
            name_review: Default::default(),
            name_check_task: None,
            branch_review: Default::default(),
            branch_check_task: None,
            _subscriptions: subscriptions,
        });
        cx.notify();
    }
    fn create_worktree(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &self.git.worktree_create else {
            return;
        };
        if !self.can_submit_worktree(form) {
            return;
        }
        let draft = form.draft(cx);
        if draft.target().action().is_err() || !form.names_ready(&draft) {
            return;
        }
        let snapshot = form.snapshot.clone();
        let submitted = draft.clone();
        self.start_git_mutation(
            self.git_repository_id(),
            format!("Create worktree {}", draft.name),
            move |backend, server, repo, id| {
                backend.create_worktree(server, repo, &snapshot, submitted.target(), id)
            },
            cx,
        );
        if let Some(state) = self.git_mutation.as_mut() {
            state.worktree_create_draft = Some(draft);
        }
    }
    fn can_submit_worktree(&self, form: &Form) -> bool {
        self.can_create_worktree_from(form.origin)
            && form.origin.review_matches(
                self.git.tab,
                self.worktree_origin_open(form.origin),
                &form.snapshot,
                &self.worktree_creation_page(form.origin),
            )
    }
    pub(super) fn sync_worktree_creation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.git_mutation_blocks_write()
            && let Some(form) = &mut self.git.worktree_create
        {
            let draft = form.draft(cx);
            if let Some(name) = form.suggested_name.update(
                &form.name.read(cx).value(),
                newport_client::git_worktree::suggested_name(&draft.branch),
            ) {
                form.name
                    .update(cx, |input, cx| input.set_value(name, window, cx));
            }
            let name = form.name.read(cx).value().to_string();
            if let Some(path) = form.suggested_path.update(
                &form.path.read(cx).value(),
                newport_client::git_worktree::suggested_path(&form.main_root, &name),
            ) {
                form.path
                    .update(cx, |input, cx| input.set_value(path, window, cx));
            }
            let checked_branch = if draft.new_branch { &draft.branch } else { "" };
            if let Some(generation) = form.branch_review.request(checked_branch) {
                let branch = checked_branch.to_owned();
                let id = form.id;
                let server = self.server.clone();
                let repo = text(&self.git.repo, "repoId");
                let executor = cx.background_executor().clone();
                form.branch_check_task = Some(cx.spawn(async move |owner, cx| {
                    executor.timer(Duration::from_millis(200)).await;
                    let checked = branch.clone();
                    let result = executor
                        .spawn(async move {
                            backend::Backend::discover()
                                .check_worktree_branch(&server, &repo, &checked)
                        })
                        .await;
                    let _ = owner.update(cx, |this, cx| {
                        if let Some(form) = &mut this.git.worktree_create
                            && form.id == id
                            && form.branch_review.complete(generation, &branch, result)
                        {
                            cx.notify();
                        }
                    });
                }));
            } else if checked_branch.is_empty() {
                form.branch_check_task = None;
            }
            if let Some(generation) = form.name_review.request(&draft.name) {
                let name = draft.name.clone();
                let id = form.id;
                let server = self.server.clone();
                let repo = text(&self.git.repo, "repoId");
                let executor = cx.background_executor().clone();
                form.name_check_task = Some(cx.spawn(async move |owner, cx| {
                    executor.timer(Duration::from_millis(200)).await;
                    let checked = name.clone();
                    let result = executor
                        .spawn(async move {
                            backend::Backend::discover()
                                .check_worktree_name(&server, &repo, &checked)
                        })
                        .await;
                    let _ = owner.update(cx, |this, cx| {
                        if let Some(form) = &mut this.git.worktree_create
                            && form.id == id
                            && form.name_review.complete(generation, &name, result)
                        {
                            cx.notify();
                        }
                    });
                }));
            } else if draft.name.is_empty() {
                form.name_check_task = None;
            }
        }
        if let Some(form) = &self.git.worktree_create {
            let enabled = self.can_submit_worktree(form)
                && form.existing_branch.is_none()
                && !form.new_branch;
            if let Some(picker) = &form.picker {
                picker.update(cx, |picker, cx| picker.set_enabled(enabled, cx));
            }
            if let Some(picker) = &form.base_picker {
                let enabled = self.can_submit_worktree(form) && form.new_branch;
                picker.update(cx, |picker, cx| picker.set_enabled(enabled, cx));
            }
        }
    }
    pub(super) fn git_worktree_create_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = self.git.worktree_create.as_ref() else {
            return div().into_any_element();
        };
        if !form
            .origin
            .active(self.git.tab, self.worktree_origin_open(form.origin))
        {
            return div().into_any_element();
        }
        let draft = form.draft(cx);
        let busy = self.git_mutation_blocks_write();
        let stale = !form.origin.review_matches(
            self.git.tab,
            self.worktree_origin_open(form.origin),
            &form.snapshot,
            &self.worktree_creation_page(form.origin),
        );
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .w_full()
            .min_w_0()
            .when(!self.project_dialog_rendering, |view| {
                view.max_w(px(520.))
                    .when(form.origin == FormOrigin::BranchPicker, |view| view.px_4())
            })
            .py_2()
            .when_some(form.existing_branch.as_ref(), |view, branch| {
                view.child(crate::project_ui::label(format!(
                    "Check out {branch} · {}",
                    form.oid.chars().take(8).collect::<String>()
                )))
            })
            .when(form.existing_branch.is_none(), |view| {
                view.child(
                    crate::project_ui::tabs("worktree-branch-source")
                        .segmented()
                        .selected_index(usize::from(!form.new_branch))
                        .child(Tab::new().label("New branch").disabled(busy))
                        .child(Tab::new().label("Existing branch").disabled(busy))
                        .on_click(cx.listener(|this, index: &usize, _, cx| {
                            if *index > 1 || this.git_mutation_blocks_write() {
                                return;
                            }
                            if let Some(form) = this.git.worktree_create.as_mut() {
                                form.new_branch = *index == 0;
                            }
                            cx.notify();
                        })),
                )
                .when(!form.new_branch, |view| {
                    view.when_some(form.picker.as_ref(), |view, picker| {
                        view.child(picker.clone())
                    })
                })
                .when(form.new_branch, |view| {
                    view.child(
                        project_dialog::field("New branch name")
                            .child(
                                crate::project_ui::input(&form.branch)
                                    .aria_label("New worktree branch")
                                    .disabled(busy),
                            )
                            .child(review_hint(
                                "worktree branch name",
                                &form.branch_review,
                                &draft.branch,
                                crate::project_ui::compact_button("retry-worktree-branch")
                                    .label("Check again")
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if !this.git_mutation_blocks_write()
                                            && let Some(form) = &mut this.git.worktree_create
                                        {
                                            form.branch_review.retry();
                                            cx.notify();
                                        }
                                    })),
                                cx,
                            )),
                    )
                    .child(
                        project_dialog::field("Start from")
                            .when_some(form.base_picker.as_ref(), |view, picker| {
                                view.child(picker.clone())
                            }),
                    )
                })
            })
            .child(
                project_dialog::field("Name")
                    .child(
                        crate::project_ui::input(&form.name)
                            .aria_label("New worktree name")
                            .disabled(busy),
                    )
                    .child(review_hint(
                        "worktree name",
                        &form.name_review,
                        &draft.name,
                        crate::project_ui::compact_button("retry-worktree-name")
                            .label("Check again")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this.git_mutation_blocks_write()
                                    && let Some(form) = &mut this.git.worktree_create
                                {
                                    form.name_review.retry();
                                    cx.notify();
                                }
                            })),
                        cx,
                    )),
            )
            .child(
                project_dialog::field("Location on server")
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div().flex_1().min_w_0().child(
                                    crate::project_ui::input(&form.path)
                                        .aria_label("New worktree path")
                                        .disabled(busy),
                                ),
                            )
                            .child(
                                crate::project_ui::button("browse-worktree-destination")
                                    .label("Browse…")
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        if !this.git_mutation_blocks_write()
                                            && let Some(form) = &this.git.worktree_create
                                        {
                                            this.begin_project_folder(
                                                form.path.clone(),
                                                true,
                                                window,
                                                cx,
                                            );
                                        }
                                    })),
                            ),
                    )
                    .child(
                        crate::project_ui::label("Use a new directory under an existing parent.")
                            .text_size(px(crate::project_ui::FONT_SIZE)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        crate::project_ui::checkbox("open-new-worktree")
                            .label("Open worktree after creation")
                            .checked(form.open_after)
                            .disabled(busy)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                if !this.git_mutation_blocks_write()
                                    && let Some(form) = &mut this.git.worktree_create
                                {
                                    form.open_after = *checked;
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        crate::project_ui::checkbox("lock-new-worktree")
                            .label("Lock worktree to protect it from removal")
                            .checked(form.locked)
                            .disabled(busy)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                if this.git_mutation_blocks_write() {
                                    return;
                                }
                                if let Some(form) = this.git.worktree_create.as_mut() {
                                    form.locked = *checked;
                                }
                                cx.notify();
                            })),
                    ),
            )
            .when(stale, |view| {
                view.child(crate::project_ui::label(
                    "Repository state changed. Cancel and review again.",
                ))
            })
            .child(
                project_dialog::actions()
                    .child(
                        crate::project_ui::button("cancel-create-worktree")
                            .label("Cancel")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this.git_mutation_blocks_write() {
                                    this.git.worktree_create = None;
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        crate::project_ui::button("create-worktree")
                            .primary()
                            .loading(self.git_write_pending())
                            .label("Create worktree")
                            .disabled(
                                !self.can_submit_worktree(form)
                                    || draft.target().action().is_err()
                                    || !form.names_ready(&draft),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.create_worktree(cx))),
                    ),
            )
            .into_any_element()
    }
}

impl Form {
    pub(super) fn initial_focus(&self, cx: &App) -> FocusHandle {
        if self.existing_branch.is_none() && self.new_branch {
            self.branch.read(cx).focus_handle(cx)
        } else {
            self.name.read(cx).focus_handle(cx)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FormOrigin, owner_path};
    use base64::Engine as _;
    use gpui::{
        Context, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _,
        TestAppContext, Window, div, px, size,
    };
    use gpui_kit as gpui;
    use gpui_kit::base::test_support::snapshots;
    use serde_json::json;

    struct HintHost {
        review: newport_client::git_worktree::NameReview,
    }
    impl Render for HintHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .w_full()
                .flex()
                .flex_col()
                .child(super::review_hint(
                    "worktree branch name",
                    &self.review,
                    "main",
                    crate::project_ui::compact_button("retry-hint").label("Check again"),
                    cx,
                ))
                .child(
                    div()
                        .debug_selector(|| "next-field".into())
                        .h(px(28.))
                        .child("Start from"),
                )
        }
    }
    #[gpui::test]
    fn validation_feedback_replaces_loading_and_exposes_wrapped_errors(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui::init(cx);
            crate::theme::install(false, cx);
        });
        let (view, visual) = cx.add_window_view(|_, _| HintHost {
            review: Default::default(),
        });
        visual.simulate_resize(size(px(520.), px(240.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let pending_top = visual.debug_bounds("next-field").unwrap().top();
        view.update(visual, |view, cx| {
            let generation = view.review.request("main").unwrap();
            view.review.complete(generation, "main", Ok(()));
            cx.notify();
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let ready_top = visual.debug_bounds("next-field").unwrap().top();
        assert_eq!(
            ready_top, pending_top,
            "validation preserves the feedback slot"
        );
        let message = "That branch already exists. Choose another name or use Existing branch.";
        view.update(visual, |view, cx| {
            view.review.retry();
            let generation = view.review.request("main").unwrap();
            view.review
                .complete(generation, "main", Err(message.into()));
            cx.notify();
        });
        for width in [520., 280.] {
            visual.simulate_resize(size(px(width), px(240.)));
            visual.update(|window, cx| window.draw(cx).clear(cx));
            let (error, retry) = visual.update(|window, _| {
                let elements = snapshots(window);
                let error = elements
                    .iter()
                    .find(|e| e.label() == Some(message))
                    .unwrap();
                assert_eq!(error.role(), Some(gpui::Role::Alert));
                assert!(
                    !elements
                        .iter()
                        .any(|e| e.label() == Some("Loading worktree branch name"))
                );
                let retry = elements
                    .iter()
                    .find(|e| e.label() == Some("Check again"))
                    .unwrap();
                (error.bounds(), retry.bounds())
            });
            assert!(error.left() >= px(0.) && retry.right() <= px(width));
            assert!(error.right() <= retry.left());
            assert!(
                visual.debug_bounds("next-field").unwrap().top()
                    >= error.bottom().max(retry.bottom())
            );
            if width == 280. {
                assert!(error.size.height > px(24.));
            }
        }
    }
    #[test]
    fn open_after_creation_uses_the_submitted_byte_exact_destination_and_can_be_disabled() {
        let mut draft = super::Draft {
            id: uuid::Uuid::new_v4(),
            name: "review".into(),
            path: "/srv/开发 checkout".into(),
            branch: "topic".into(),
            oid: "a".repeat(40),
            new_branch: true,
            locked: false,
            open_after: true,
        };
        let action = draft.target().action().unwrap();
        assert_eq!(draft.open_destination(), Some(action["path"].clone()));
        draft.open_after = false;
        assert!(draft.open_destination().is_none());
        assert_eq!(draft.target().action().unwrap(), action);
    }
    #[test]
    fn owner_navigation_requires_matching_branch_and_available_checkout() {
        let encode = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
        let branch = json!({"name":{"display":"topic","bytesB64":encode("topic")},"oid":{"hex":"abc"},"remote":false});
        let mut page = json!({"entries":[{"kind":"linked","current":false,"state":"available","path":{"display":"/checkout","bytesB64":encode("/checkout")},"head":{"name":{"bytesB64":encode("refs/heads/topic")}}}]});
        assert_eq!(
            owner_path(&branch, &page),
            Some(page["entries"][0]["path"].clone())
        );
        page["entries"][0]["head"]["name"]["bytesB64"] = json!(encode("refs/heads/other"));
        assert!(owner_path(&branch, &page).is_none());
        page["entries"][0]["head"]["name"]["bytesB64"] = json!(encode("refs/heads/topic"));
        page["entries"][0]["state"] = json!("missing");
        assert!(owner_path(&branch, &page).is_none());
    }
    #[test]
    fn worktree_review_keeps_origin_and_rejects_changed_snapshot() {
        let page = json!({"snapshot":"worktrees-before"});
        let branches = json!({"snapshot":"branches-before"});
        assert_eq!(FormOrigin::CheckoutPicker.resource_key(), "git-checkouts");
        for tab in [0, 1, 3, 4, 5, 6] {
            assert!(FormOrigin::CheckoutPicker.review_matches(
                tab,
                true,
                "worktrees-before",
                &page
            ));
            assert!(!FormOrigin::CheckoutPicker.review_matches(
                tab,
                false,
                "worktrees-before",
                &page
            ));
            assert!(!FormOrigin::CheckoutPicker.review_matches(tab, true, "changed", &page));
        }
        assert_eq!(FormOrigin::BranchPicker.resource_key(), "git-worktree-list");
        assert!(FormOrigin::BranchPicker.review_matches(1, true, "branches-before", &branches));
        assert!(!FormOrigin::BranchPicker.review_matches(1, false, "branches-before", &branches));
        assert!(!FormOrigin::BranchPicker.review_matches(1, true, "branches-before", &page));
        assert!(!FormOrigin::BranchPicker.review_matches(1, true, "", &json!({"snapshot":""})));
    }
}
