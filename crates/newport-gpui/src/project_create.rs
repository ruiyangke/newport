//! Creation is dispatched once; bookmarking is a separate recoverable step.
use super::*;
use newport_client::project_creation::parameters;
pub(super) struct Create {
    pub id: uuid::Uuid,
    clone_repo: bool,
    name: Entity<InputState>,
    path: Entity<InputState>,
    url: Entity<InputState>,
    branch: Entity<InputState>,
    attempted: bool,
    pub created: bool,
    pub bookmark: Option<Value>,
    _subscriptions: Vec<Subscription>,
}
impl Newport {
    pub(super) fn begin_project_create(
        &mut self,
        clone_repo: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project_add.is_some()
            || self.project_create.is_some()
            || self.project_edit.is_some()
            || self.project_add_pending()
            || self.git_mutation_blocks_write()
            || self
                .resources
                .get("project-created-save")
                .is_some_and(|r| r.pending)
        {
            return;
        }
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Project name"));
        crate::controls::focus_editor(&name, window, cx);
        let path = cx.new(|cx| InputState::new(window, cx).placeholder("/home/user/projects/app"));
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("Repository URL"));
        let branch = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(if clone_repo { "" } else { "main" })
                .placeholder("Remote default branch")
        });
        let subscriptions = [&name, &path, &url, &branch]
            .into_iter()
            .map(|input| {
                Self::project_form_input(
                    input,
                    project_dialog::Kind::Create,
                    Self::submit_project_create,
                    cx,
                )
            })
            .collect();
        self.resources.invalidate("project-created-save");
        self.project_creation_save_owner = None;
        self.project_checks = None;
        self.project_create = Some(Create {
            id: uuid::Uuid::new_v4(),
            clone_repo,
            name,
            path,
            url,
            branch,
            attempted: false,
            created: false,
            bookmark: None,
            _subscriptions: subscriptions,
        });
        cx.notify();
    }
    fn submit_project_create(&mut self, cx: &mut Context<Self>) {
        if self.git_mutation_blocks_write()
            || self
                .resources
                .get("project-created-save")
                .is_some_and(|r| r.pending)
        {
            return;
        }
        let Some(form) = self.project_create.as_mut() else {
            return;
        };
        if form.created {
            return;
        }
        form.attempted = true;
        cx.notify();
        let Ok(params) = parameters(
            form.clone_repo,
            form.name.read(cx).value().as_str(),
            form.path.read(cx).value().as_str(),
            form.url.read(cx).value().as_str(),
            form.branch.read(cx).value().as_str(),
        ) else {
            return;
        };
        let name = form.name.read(cx).value().trim().to_owned();
        let id = form.id;
        let clone_repo = form.clone_repo;
        let method = if form.clone_repo {
            "repo.clone"
        } else {
            "repo.init"
        };
        self.start_git_mutation(
            String::new(),
            format!("{} {name}", if clone_repo { "Clone" } else { "Create" }),
            move |backend, server, _, operation| {
                backend.bootstrap(server, method, params, operation)
            },
            cx,
        );
        if let Some(mutation) = self.git_mutation.as_mut() {
            mutation.project_creation = Some((id, name));
        }
    }
    pub(super) fn save_created_project(&mut self, id: uuid::Uuid, args: Value) {
        self.project_creation_save_owner = Some((id, text(&args, "serverId")));
        if let Some(form) = self.project_create.as_mut().filter(|form| form.id == id) {
            form.bookmark = Some(args.clone());
        }
        self.request("project-created-save", "native.add_project", args);
    }
    fn retry_project_bookmark(&mut self, cx: &mut Context<Self>) {
        if self
            .resources
            .get("project-created-save")
            .is_some_and(|r| r.pending)
        {
            return;
        }
        let Some(form) = &self.project_create else {
            return;
        };
        let Some(args) = form
            .bookmark
            .clone()
            .filter(|args| args["serverId"].as_str() == Some(&self.server))
        else {
            return;
        };
        self.save_created_project(form.id, args);
        cx.notify();
    }
    pub(super) fn project_create_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.project_create else {
            return div().into_any_element();
        };
        let busy = self.git_mutation_blocks_write()
            || self
                .resources
                .get("project-created-save")
                .is_some_and(|r| r.pending);
        let validation = parameters(
            form.clone_repo,
            form.name.read(cx).value().as_str(),
            form.path.read(cx).value().as_str(),
            form.url.read(cx).value().as_str(),
            form.branch.read(cx).value().as_str(),
        );
        let server = self.snapshot["config"]["servers"]
            .as_array()
            .and_then(|servers| servers.iter().find(|server| server["id"] == self.server))
            .map(server_picker::display_name)
            .unwrap_or_else(|| "the selected server".into());
        project_dialog::form()
            .child(project_dialog::description("project-creation-server", format!("On {server}. The repository will be saved in Projects.")).opacity(1.))
            .child(project_dialog::field("Project name")
                .child(crate::controls::input(&form.name).aria_label("New project name").disabled(busy||form.created)))
            .when(form.clone_repo,|v|v.child(project_dialog::field("Repository URL")
                .child(crate::controls::input(&form.url).aria_label("Repository URL").disabled(busy||form.created))))
            .child(project_dialog::field(if form.clone_repo {"Destination on server"}else{"Existing directory on server"})
                .child(div().flex().items_center().gap_2()
                .child(div().flex_1().min_w_0().child(crate::controls::input(&form.path).aria_label("New repository path").disabled(busy||form.created)))
                .child(crate::controls::button("browse-project-destination").label("Browse…").disabled(busy||form.created).on_click(cx.listener(|this,_,window,cx|{if let Some(form)=&this.project_create {this.begin_project_folder(form.path.clone(),form.clone_repo,window,cx);}}))))
                .child(project_dialog::description("project-creation-path-hint", if form.clone_repo {"The parent directory must exist; the destination must not exist yet."}else{"Choose an existing directory without a Git repository. Its files will be kept."})))
            .child(project_dialog::field(if form.clone_repo {"Branch (optional)"}else{"Initial branch"})
                .child(crate::controls::input(&form.branch).aria_label(if form.clone_repo {"Branch (optional)"}else{"Initial branch"}).disabled(busy||form.created)))
            .when(form.created,|v|v.child(label("Repository created. Saving its project bookmark does not run Git creation again.")))
            .when_some(validation.err().filter(|_| form.attempted && !form.created), |view, error| view.child(project_dialog::validation_error(error, cx)))
            .when(self.resources.get("project-created-save").is_some_and(|r| r.pending || r.error.is_some()), |view| view.child(self.status("project-created-save",cx)))
            .child(project_dialog::actions()
                .child(crate::controls::form_button("create-project-cancel").min_w(px(76.)).label(if form.created {"Close"}else{"Cancel"}).disabled(busy).on_click(cx.listener(|this,_,_,cx|{this.project_create=None;cx.notify();})))
                .when(!form.created, |v|v.child(crate::controls::form_button("create-project-submit").min_w(px(76.)).primary().loading(busy).label(if form.clone_repo {"Clone repository"}else{"Create repository"}).disabled(busy||form.created).on_click(cx.listener(|this,_,_,cx|this.submit_project_create(cx)))))
                .when(form.created && form.bookmark.is_some(), |v|v.child(crate::controls::form_button("retry-created-project-save").min_w(px(76.)).label("Try again").accessibility_label("Try saving the project again").disabled(busy).on_click(cx.listener(|this,_,_,cx|this.retry_project_bookmark(cx))))))
            .into_any_element()
    }
}

impl Create {
    pub(super) fn title(&self) -> &'static str {
        if self.clone_repo {
            "Clone repository"
        } else {
            "Create repository"
        }
    }

    pub(super) fn initial_focus(&self, cx: &App) -> FocusHandle {
        self.name.read(cx).focus_handle(cx)
    }
}
