//! Creation is dispatched once; bookmarking is a separate recoverable step.
use super::*;
use gpui_kit::component::input::InputEvent;
use newport_client::project_creation::parameters;
pub(super) struct Create {
    pub id: uuid::Uuid,
    clone_repo: bool,
    name: Entity<InputState>,
    path: Entity<InputState>,
    url: Entity<InputState>,
    branch: Entity<InputState>,
    home: String,
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
        let home = account_home(
            self.file_browsers
                .get(&self.server)
                .and_then(|browser| browser.home.as_deref()),
            self.selected_server()
                .and_then(|server| server["sshUser"].as_str())
                .unwrap_or_default(),
        );
        let example = if clone_repo {
            clone_destination(&home, "")
        } else {
            init_destination(&home, "")
        };
        let path = cx.new(|cx| InputState::new(window, cx).placeholder(example));
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("Repository URL"));
        let branch = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(if clone_repo { "" } else { "main" })
                .placeholder("Remote default branch")
        });
        let mut subscriptions: Vec<_> = [&name, &path, &url, &branch]
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
        if !clone_repo {
            let (path, home) = (path.clone(), home.clone());
            subscriptions.push(cx.subscribe_in(
                &name,
                window,
                move |_, name, event, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        let destination = init_destination(&home, &name.read(cx).value());
                        path.update(cx, |path, cx| path.set_placeholder(destination, window, cx));
                    }
                },
            ));
        }
        if clone_repo {
            let (name, path, home) = (name.clone(), path.clone(), home.clone());
            subscriptions.push(
                cx.subscribe_in(&url, window, move |_, url, event, window, cx| {
                    if !matches!(event, InputEvent::Change) {
                        return;
                    }
                    let url = url.read(cx).value().to_string();
                    let repository = repository_name(&url).unwrap_or("Project name");
                    name.update(cx, |name, cx| name.set_placeholder(repository, window, cx));
                    let destination = clone_destination(&home, &url);
                    path.update(cx, |path, cx| path.set_placeholder(destination, window, cx));
                }),
            );
        }
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
            home,
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
        let (name, path) = form.name_and_path(cx);
        let Ok(params) = parameters(
            form.clone_repo,
            &name,
            &path,
            form.url.read(cx).value().as_str(),
            form.branch.read(cx).value().as_str(),
        ) else {
            return;
        };
        let name = name.trim().to_owned();
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
        let (name, path) = form.name_and_path(cx);
        let validation = parameters(
            form.clone_repo,
            &name,
            &path,
            form.url.read(cx).value().as_str(),
            form.branch.read(cx).value().as_str(),
        );
        project_dialog::form()
            .child(project_dialog::field("Project name")
                .child(crate::controls::input(&form.name).aria_label("New project name").disabled(busy||form.created)))
            .when(form.clone_repo,|v|v.child(project_dialog::field("Repository URL")
                .child(crate::controls::input(&form.url).aria_label("Repository URL").disabled(busy||form.created))))
            .child(project_dialog::field(if form.clone_repo {"Destination on server"}else{"Folder on server"})
                .child(div().flex().items_center().gap_2()
                .child(div().flex_1().min_w_0().child(crate::controls::input(&form.path).aria_label("New repository path").disabled(busy||form.created)))
                .child(crate::controls::button("browse-project-destination").label("Browse…").disabled(busy||form.created).on_click(cx.listener(|this,_,window,cx|{if let Some(form)=&this.project_create {this.begin_project_folder(form.path.clone(),form.clone_repo,window,cx);}}))))
                .child(project_dialog::description("project-creation-path-hint", if form.clone_repo {"Leave it empty to clone into your home folder. The destination must not exist yet."}else{"Leave it empty to create it in your home folder. An existing folder keeps its files."})))
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

    /// A clone left without a name or destination takes them from its URL,
    /// as `git clone` does.
    fn name_and_path(&self, cx: &App) -> (String, String) {
        let name = self.name.read(cx).value().to_string();
        let path = self.path.read(cx).value().to_string();
        if !self.clone_repo {
            let path = match folder_name(&name) {
                Some(_) if path.is_empty() => init_destination(&self.home, &name),
                _ => path,
            };
            return (name, path);
        }
        let url = self.url.read(cx).value().to_string();
        let name = match repository_name(&url) {
            Some(repository) if name.trim().is_empty() => repository.to_owned(),
            _ => name,
        };
        let path = match repository_name(&url) {
            Some(_) if path.is_empty() => clone_destination(&self.home, &url),
            _ => path,
        };
        (name, path)
    }
}

/// The account's home folder, as Files found it, or as an SSH account's
/// home usually is.
fn account_home(home: Option<&str>, user: &str) -> String {
    match (home, user) {
        (Some(home), _) => home.trim_end_matches('/').to_owned(),
        (None, "root") => "/root".to_owned(),
        (None, "") => "/home/you".to_owned(),
        (None, user) => format!("/home/{user}"),
    }
}

/// The folder `git clone` would make for `url`: its last path segment
/// without `.git`.
fn repository_name(url: &str) -> Option<&str> {
    let url = url.trim().trim_end_matches('/');
    let last = url.rsplit(['/', ':']).next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    (!name.is_empty() && name != "." && name != ".." && name != url).then_some(name)
}

/// A new repository's folder is named after the project when that makes one
/// folder name.
fn folder_name(name: &str) -> Option<&str> {
    let name = name.trim();
    (!name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\0'])).then_some(name)
}

fn init_destination(home: &str, name: &str) -> String {
    format!("{home}/{}", folder_name(name).unwrap_or("app"))
}

fn clone_destination(home: &str, url: &str) -> String {
    format!("{home}/{}", repository_name(url).unwrap_or("repository"))
}

#[cfg(test)]
mod example_tests {
    use super::{account_home, clone_destination, init_destination, repository_name};

    #[test]
    fn a_new_repository_is_named_after_its_project() {
        assert_eq!(init_destination("/home/u", " notes "), "/home/u/notes");
        assert_eq!(init_destination("/home/u", "My Notes"), "/home/u/My Notes");
        for name in ["", "..", "a/b"] {
            assert_eq!(init_destination("/home/u", name), "/home/u/app", "{name}");
        }
    }

    #[test]
    fn the_suggested_destination_is_under_the_account_home() {
        assert_eq!(account_home(Some("/srv/u/"), "u"), "/srv/u");
        assert_eq!(account_home(None, "newport"), "/home/newport");
        assert_eq!(account_home(None, "root"), "/root");
    }

    #[test]
    fn a_clone_is_named_after_its_repository_like_git_clone() {
        for (url, name) in [
            (
                "https://github.com/octocat/Hello-World.git",
                Some("Hello-World"),
            ),
            (
                "git@github.com:octocat/Spoon-Knife.git",
                Some("Spoon-Knife"),
            ),
            ("ssh://host/srv/repos/tools/ ", Some("tools")),
            ("host:repo", Some("repo")),
            ("/srv/git/site.git/", Some("site")),
            ("https://", None),
            ("plain", None),
            ("", None),
        ] {
            assert_eq!(repository_name(url), name, "{url}");
        }
        assert_eq!(
            clone_destination(
                "/home/newport",
                "https://github.com/octocat/Hello-World.git"
            ),
            "/home/newport/Hello-World"
        );
        assert_eq!(clone_destination("/root", ""), "/root/repository");
    }
}
