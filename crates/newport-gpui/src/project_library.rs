//! Local project bookmarks; these controls never mutate a remote repository.
use super::*;
use gpui_kit::component::tab::Tab;
use newport_client::project_bookmark::files_path;

pub(super) struct Edit {
    project: Value,
    remove: bool,
    name: Entity<InputState>,
    _subscription: Subscription,
}
fn rename(project: &Value, server: &str, name: &str) -> Option<Value> {
    let name = name.trim();
    if project["serverId"].as_str() != Some(server)
        || !project["id"].is_string()
        || name.is_empty()
        || name.len() > 256
        || name.chars().any(char::is_control)
    {
        return None;
    }
    let mut saved = project.clone();
    saved["name"] = json!(name);
    Some(saved)
}
pub(super) use newport_client::project_checks::status_key;

#[derive(Debug, PartialEq)]
enum LibraryNotice<'a> {
    Loading,
    Refreshing,
    Error { detail: &'a str, cached: bool },
    Empty(&'static str),
}

/// An unread or failed list is never evidence that there are no bookmarks.
/// Resources retain successful values during refreshes, so the same rows can
/// remain usable while the notice describes the latest read.
fn library_notice<'a>(
    resource: Option<&'a newport_client::resources::Resource>,
    visible_count: usize,
    query: &str,
    favorites_only: bool,
) -> Option<LibraryNotice<'a>> {
    let Some(resource) = resource else {
        return Some(LibraryNotice::Loading);
    };
    let cached = resource.value.is_array();
    if let Some(detail) = resource.error.as_deref() {
        return Some(LibraryNotice::Error { detail, cached });
    }
    if resource.display_pending() || !cached {
        return Some(if cached {
            LibraryNotice::Refreshing
        } else {
            LibraryNotice::Loading
        });
    }
    (visible_count == 0).then_some(LibraryNotice::Empty(if !query.trim().is_empty() {
        "No projects match your search."
    } else if favorites_only {
        "No favorite projects. Use a project’s star button in All to add favorites."
    } else {
        "No saved projects. Add a repository using its absolute path on this server."
    }))
}

fn library_feedback(notice: LibraryNotice<'_>, cx: &App) -> AnyElement {
    if notice == LibraryNotice::Loading {
        return loading::lines("projects", 4);
    }
    if let LibraryNotice::Empty(message) = notice {
        let (title, description, icon) = if message.starts_with("No projects match") {
            (
                "No matching projects",
                "Try another search or clear the search field.",
                IconName::Search,
            )
        } else if message.starts_with("No favorite") {
            (
                "No favorite projects",
                "Choose All, then star a project to keep it here.",
                IconName::Star,
            )
        } else {
            (
                "No saved projects",
                "Use Add project to open, clone, or create a repository on this server.",
                IconName::Folder,
            )
        };
        return empty::pane(icon, title, description, cx).into_any_element();
    }
    let failed = matches!(notice, LibraryNotice::Error { .. });
    let message = match notice {
        LibraryNotice::Loading => unreachable!(),
        LibraryNotice::Refreshing => "Refreshing projects…".to_owned(),
        LibraryNotice::Error { detail, cached } => format!(
            "{} {detail}",
            if cached {
                "Could not refresh projects. Showing the last saved list. Use Refresh to try again."
            } else {
                "Could not load projects. Use Refresh to try again."
            }
        ),
        LibraryNotice::Empty(message) => message.to_owned(),
    };
    div()
        .id("project-library-feedback")
        .role(if failed {
            accesskit::Role::Alert
        } else {
            accesskit::Role::Status
        })
        .aria_label(message.clone())
        .w_full()
        .min_w_0()
        .text_size(px(crate::ui::FONT_SIZE))
        .text_color(cx.theme().muted_foreground)
        .when(failed, |view| view.p_3().bg(cx.theme().muted))
        .child(message)
        .into_any_element()
}

impl Newport {
    pub(super) fn project_open_files(
        &mut self,
        project: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.server_form.is_some() || project["serverId"].as_str() != Some(&self.server) {
            return;
        }
        let Some(path) = files_path(project, &self.server) else {
            self.notice = Some("This repository’s path cannot be opened in Files. Files requires an absolute UTF-8 path.".into());
            cx.notify();
            return;
        };
        self.save_workspace_selection();
        self.restore_git = None;
        self.suspend_project_library();
        self.page = Page::Files;
        self.workspace_page = Page::Files;
        self.reset();
        self.save_workspace_selection();
        self.prepare_file_browser(window, cx);
        // Request only the selected directory, never the browser’s previous location.
        self.files_navigate(path, window, cx);
    }
    fn project_edit_busy(&self) -> bool {
        self.resources
            .get("project-edit")
            .is_some_and(|r| r.pending)
    }
    pub(super) fn edit_project(
        &mut self,
        project: Value,
        remove: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project_add.is_some()
            || self.project_add_pending()
            || self.project_create.is_some()
            || self.project_edit_busy()
            || project["serverId"].as_str() != Some(&self.server)
        {
            return;
        }
        let name = cx.new(|cx| InputState::new(window, cx).default_value(text(&project, "name")));
        if !remove {
            crate::controls::focus_editor(&name, window, cx);
        }
        let subscription = Self::project_form_input(
            &name,
            project_dialog::Kind::Edit,
            Self::submit_project_edit,
            cx,
        );
        self.project_edit = Some(Edit {
            project,
            remove,
            name,
            _subscription: subscription,
        });
        self.resources.invalidate("project-edit");
        cx.notify();
    }
    fn submit_project_edit(&mut self, cx: &mut Context<Self>) {
        if self.project_edit_busy() {
            return;
        }
        let Some(edit) = &self.project_edit else {
            return;
        };
        let Some(project) = rename(
            &edit.project,
            &self.server,
            edit.name.read(cx).value().as_str(),
        ) else {
            return;
        };
        if edit.remove {
            self.request(
                "project-edit",
                "git_projects_remove",
                json!({"serverId":self.server,"projectId":project["id"]}),
            );
        } else {
            self.request(
                "project-edit",
                "git_projects_save",
                json!({"project":project}),
            );
        }
        cx.notify();
    }
    pub(super) fn project_edit_completed(&mut self) {
        if self
            .resources
            .get("project-edit")
            .is_some_and(|r| r.error.is_none())
        {
            self.project_bookmarks_changed(&self.server.clone());
            self.project_edit = None;
            self.notification = Some(Notification::success("Project list updated."));
            if self.page == Page::Projects && self.selection.is_none() {
                self.refresh();
            }
        }
    }
    pub(super) fn project_edit_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(edit) = self.project_edit.as_ref() else {
            return div().into_any_element();
        };
        let busy = self.project_edit_busy();
        let valid = rename(
            &edit.project,
            &self.server,
            edit.name.read(cx).value().as_str(),
        )
        .is_some();
        div()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .py_2()
                    .child(edit_description(&edit.project, edit.remove))
                    .when(!edit.remove, |v| {
                        v.child(
                            crate::controls::input(&edit.name)
                                .aria_label("Project name")
                                .disabled(busy),
                        )
                    })
                    .child(
                        project_dialog::actions()
                            .child(
                                crate::controls::button("project-edit-cancel")
                                    .label("Cancel")
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.project_edit = None;
                                        this.resources.invalidate("project-edit");
                                        cx.notify();
                                    })),
                            )
                            .child(
                                crate::controls::button("project-edit-submit")
                                    .loading(busy)
                                    .when(edit.remove, |button| button.danger())
                                    .label(if edit.remove {
                                        "Remove from list"
                                    } else {
                                        "Save name"
                                    })
                                    .disabled(busy || !valid)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.submit_project_edit(cx)),
                                    ),
                            ),
                    )
                    .child(self.status("project-edit", cx)),
            )
            .into_any_element()
    }
    pub(super) fn projects(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.selection.is_some() {
            self.sync_git_log_width(window, cx);
            self.sync_git_transfer_status();
            return self.git_screen(window, cx);
        }
        let busy = self.project_add_pending()
            || self.project_add.is_some()
            || self.project_edit_busy()
            || self.project_create.is_some()
            || self
                .resources
                .get("project-created-save")
                .is_some_and(|r| r.pending);
        self.sync_project_library(window, cx);
        let query = self.project_query(cx);
        let library = self.project_libraries.get_mut(&self.server).unwrap();
        let filter = library.input.as_ref().unwrap().clone();
        let preferences = self
            .project_preferences
            .get(&self.server)
            .cloned()
            .unwrap_or_default();
        let projects = library.list.prepare(
            self.resources.get("page"),
            &preferences,
            &query,
            self.content_width,
        );
        let scroll = library.list.state.clone();
        let count = projects.len();
        let columns = project_library_rows::Columns::for_width(
            self.content_width.min(1240.) - crate::controls::PAGE_PADDING * 2.,
        );
        let notice = library_notice(
            self.resources.get("page"),
            count,
            &query,
            preferences.favorites_only,
        );
        let loading = notice == Some(LibraryNotice::Loading);
        // No bookmarks at all, rather than none matching a search or Favorites.
        let empty_library = self.resources.get("page").is_some_and(|page| {
            page.error.is_none()
                && !page.display_pending()
                && page.value.as_array().is_some_and(Vec::is_empty)
        });
        let form_open = self.project_add.is_some()
            || self.project_create.is_some()
            || self.project_edit.is_some();
        div().id("projects-scroll").flex_1().min_h_0().max_w(px(1240.)).p(px(crate::controls::PAGE_PADDING)).flex().flex_col().gap_3()
            .when(!empty_library, |v| v.child(div().flex().flex_wrap().items_center().gap_2().child(div().w(px(260.)).max_w_full().min_w(px(120.)).child(crate::controls::input(&filter).aria_label("Search projects").prefix(Icon::new(IconName::Search).size(px(14.)))))
                .child(crate::controls::tabs("project-library-filter").gap_0().selected_index(usize::from(preferences.favorites_only)).children(["All","Favorites"].into_iter().map(|name|Tab::new().label(name))).on_click(cx.listener(|this,index:&usize,_,cx|{this.project_preferences.entry(this.server.clone()).or_default().favorites_only=*index==1;this.project_preferences_changed();cx.notify();})))
                .child(div().ml_auto().min_w_0().max_w_full().flex().flex_wrap().gap_2()
                .child(crate::controls::button("refresh-project-library").label("Refresh").icon(IconName::RefreshCw)
                    .disabled(self.resources.get("page").is_some_and(|r|r.pending))
                    .on_click(cx.listener(|this,_,_,cx|{this.refresh();cx.notify();})))
                .child(crate::controls::button("project-add-menu").label("Add project").icon(IconName::Plus).dropdown_caret(true)
                    .disabled(busy || form_open).dropdown_menu({
                        let weak=cx.entity().downgrade();move |menu,_,_| {
                            let add=weak.clone();let clone_repo=weak.clone();let create=weak.clone();
                            menu.item(PopupMenuItem::new("Add existing repository…").on_click(move |_,window,cx|{let _=add.update(cx,|this,cx|this.begin_project_add(window,cx));}))
                                .item(PopupMenuItem::new("Clone repository…").on_click(move |_,window,cx|{let _=clone_repo.update(cx,|this,cx|this.begin_project_create(true,window,cx));}))
                                .item(PopupMenuItem::new("New repository…").on_click(move |_,window,cx|{let _=create.update(cx,|this,cx|this.begin_project_create(false,window,cx));}))
                        }
                    }))
                )
            ))
            .when(self.project_create.is_none() && self.project_creation_save_owner.as_ref().is_some_and(|(_,server)| server == &self.server) && self.resources.get("project-created-save").is_some_and(|r|r.error.is_some()), |v|v.child(label("Repository created, but its bookmark could not be saved. Add the existing repository; do not repeat creation.")).child(self.status("project-created-save",cx)))
            .when(empty_library, |view| view.child(self.empty_library(busy || form_open, cx)))
            .when_some(notice.filter(|notice| *notice != LibraryNotice::Loading && !empty_library), |view, notice| view.child(library_feedback(notice,cx)))
            .when(loading,|view|view.child(project_loading::library(self.content_width.min(1240.) - crate::controls::PAGE_PADDING * 2.,cx)))
            .when(!loading && count > 0,|view|view.child(div().flex().flex_col().min_h_0().overflow_hidden()
                .flex_1()
                .child(self.project_library_caption(count,columns,cx))
                .child(list(scroll,cx.processor(move |this,index:usize,_,cx| {
                    this.project_library_row(&projects[index],index,columns,busy || this.project_edit.is_some(),index + 1 == count,cx)
                })).flex_1().min_h_0())))
            .into_any_element()
    }
}

fn edit_description(project: &Value, remove: bool) -> Div {
    let title = format!(
        "{} {}",
        if remove {
            "Remove from list:"
        } else {
            "Rename:"
        },
        text(project, "name")
    );
    div()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .id("project-edit-title")
                .role(accesskit::Role::Heading)
                .aria_label(title.clone())
                .test_support()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(project_dialog::description(
            "project-edit-path",
            text(&project["path"], "display"),
        ))
        .when(remove, |view| {
            view.child(project_dialog::description(
                "project-edit-consequence",
                "Only the saved bookmark is removed. The repository and its files stay on the server.",
            ))
        })
}

impl Edit {
    pub(super) fn title(&self) -> &'static str {
        if self.remove {
            "Remove project bookmark"
        } else {
            "Rename project"
        }
    }

    pub(super) fn initial_focus(&self, cx: &App) -> Option<FocusHandle> {
        (!self.remove).then(|| self.name.read(cx).focus_handle(cx))
    }
}

impl Newport {
    /// An empty library offers the Add project choices in place, instead of
    /// pointing at the menu in the corner.
    fn empty_library(&self, disabled: bool, cx: &mut Context<Self>) -> AnyElement {
        let server = self
            .selected_server()
            .and_then(|server| server["name"].as_str())
            .unwrap_or("this server")
            .to_owned();
        let narrow = self.content_width < 640.;
        let card =
            |id: &'static str,
             icon: IconName,
             title: &'static str,
             text: String,
             action: fn(&mut Newport, &mut Window, &mut Context<Newport>)| {
                div()
                    .id(id)
                    .debug_selector(move || id.into())
                    .role(accesskit::Role::Button)
                    .aria_label(title)
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .p(px(14.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(px(crate::ui::RADIUS))
                    .text_size(px(crate::ui::FONT_SIZE))
                    .when(!disabled, |card| {
                        card.cursor_pointer()
                            .hover(|card| card.bg(theme::row_hover(cx)))
                            .on_click(
                                cx.listener(move |this, _, window, cx| action(this, window, cx)),
                            )
                    })
                    .when(disabled, |card| card.opacity(0.5))
                    .child(
                        Icon::new(icon)
                            .size(px(18.))
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(div().font_weight(FontWeight::MEDIUM).child(title))
                    .child(
                        div()
                            .line_height(relative(1.5))
                            .text_color(cx.theme().muted_foreground)
                            .child(text),
                    )
            };
        div()
            .id("projects-empty")
            .debug_selector(|| "projects-empty".into())
            .w_full()
            .max_w(px(640.))
            .mx_auto()
            .pt(px(32.))
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(
                div()
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("Bring a project to {server}")),
                    )
                    .child(
                        div()
                            .mt(px(4.))
                            .text_size(px(crate::ui::FONT_SIZE))
                            .line_height(relative(1.6))
                            .text_color(cx.theme().muted_foreground)
                            .child("Projects are Git repositories on this server. Edit files, run commands, and commit without leaving Newport."),
                    ),
            )
            .child(
                div()
                    .flex()
                    .when(narrow, |v| v.flex_col())
                    .gap(px(12.))
                    .child(card(
                        "projects-empty-add",
                        IconName::Folder,
                        "Add a repository",
                        format!("Pick a Git folder that’s already on {server}."),
                        |this, window, cx| this.begin_project_add(window, cx),
                    ))
                    .child(card(
                        "projects-empty-clone",
                        IconName::Download,
                        "Clone a repository",
                        "From GitHub, GitLab, or any Git URL.".into(),
                        |this, window, cx| this.begin_project_create(true, window, cx),
                    ))
                    .child(card(
                        "projects-empty-create",
                        IconName::Plus,
                        "Create a repository",
                        "Start a new one in a folder you choose.".into(),
                        |this, window, cx| this.begin_project_create(false, window, cx),
                    )),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{LibraryNotice, library_notice, rename};
    use newport_client::resources::Resources;
    use serde_json::json;

    #[gpui_kit::test]
    fn input_presentation_does_not_reset_project_scroll_or_invalidate_other_pages(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::{AppContext, IntoElement, ListOffset, Render, px};
        use std::{cell::Cell, rc::Rc};
        struct IdleHost;
        impl Render for IdleHost {
            fn render(
                &mut self,
                _: &mut gpui::Window,
                _: &mut gpui::Context<Self>,
            ) -> impl IntoElement {
                gpui::div()
            }
        }
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let mut view = None;
        let (_, visual) = cx.add_window_view(|window, cx| {
            view = Some(cx.new(|cx| crate::page_perf_tests::owner(window, cx)));
            IdleHost
        });
        let view = view.unwrap();
        let notices = Rc::new(Cell::new(0));
        let observed = notices.clone();
        let _subscription =
            visual.update(|_, cx| cx.observe(&view, move |_, _| observed.set(observed.get() + 1)));
        let projects: Vec<_> = (0..100)
            .map(|i| {
                json!({
                    "id":format!("p{i}"),"serverId":"fixture","name":format!("Project {i}"),
                    "path":{"display":format!("/work/{i}"),"bytesB64":i.to_string()}
                })
            })
            .collect();
        for page in [crate::Page::Projects, crate::Page::Overview] {
            let filter = view.update(visual, |view, _| {
                view.page = page;
                view.project_library_state().list.sync(&projects, 900.);
                view.project_library_state()
                    .list
                    .state
                    .scroll_to(ListOffset {
                        item_ix: 25,
                        offset_in_item: px(8.),
                    });
                view.project_libraries[&view.server]
                    .input
                    .as_ref()
                    .unwrap()
                    .clone()
            });
            visual.run_until_parked();
            notices.set(0);
            for _ in 0..3 {
                filter.update(visual, |_, cx| cx.notify());
                visual.run_until_parked();
            }
            assert_eq!(
                notices.get(),
                0,
                "input presentation should not notify {}",
                page.title()
            );
            assert_eq!(
                view.read_with(visual, |view, _| view.project_libraries[&view.server]
                    .list
                    .state
                    .logical_scroll_top()
                    .item_ix),
                25
            );
        }
        view.update(visual, |view, _| view.page = crate::Page::Projects);
        let filter = view.read_with(visual, |view, _| {
            view.project_libraries[&view.server]
                .input
                .as_ref()
                .unwrap()
                .clone()
        });
        visual.update(|window, cx| {
            filter.update(cx, |filter, cx| filter.replace_all("Project 9", window, cx))
        });
        visual.run_until_parked();
        assert_eq!(
            notices.get(),
            1,
            "an actual edit must update the visible list"
        );
        assert_eq!(
            view.read_with(visual, |view, _| view.project_libraries[&view.server]
                .list
                .state
                .logical_scroll_top()
                .item_ix),
            0
        );
    }

    #[gpui_kit::test]
    fn bookmark_review_exposes_identity_path_and_removal_consequence(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::base::test_support::snapshots;
        use gpui_kit::{IntoElement, Render, px, size};

        struct ReviewHost(bool);
        impl Render for ReviewHost {
            fn render(
                &mut self,
                _: &mut gpui::Window,
                _: &mut gpui::Context<Self>,
            ) -> impl IntoElement {
                super::edit_description(
                    &json!({"name":"日本語 project", "path":{"display":"/home/fixture/日本語 project"}}),
                    self.0,
                )
            }
        }
        cx.update(|cx| {
            gpui::init(cx);
            crate::theme::install(false, cx);
        });
        let (view, visual) = cx.add_window_view(|_, _| ReviewHost(true));
        for width in [280., 520.] {
            visual.simulate_resize(size(px(width), px(500.)));
            for remove in [true, false] {
                view.update(visual, |view, cx| {
                    view.0 = remove;
                    cx.notify();
                });
                visual.update(|window, cx| {
                    window.draw(cx).clear(cx);
                    let nodes = snapshots(window);
                    let title = if remove {
                        "Remove from list: 日本語 project"
                    } else {
                        "Rename: 日本語 project"
                    };
                    assert!(
                        nodes
                            .iter()
                            .any(|node| node.role() == Some(gpui::Role::Heading)
                                && node.label() == Some(title))
                    );
                    assert!(
                        nodes
                            .iter()
                            .any(|node| node.label() == Some("/home/fixture/日本語 project"))
                    );
                    assert_eq!(
                        nodes.iter().any(|node| node
                            .label()
                            .is_some_and(|label| label.starts_with("Only the saved bookmark"))),
                        remove
                    );
                });
            }
        }
    }
    #[test]
    fn status_cache_is_scoped_to_server_bookmark_and_exact_path() {
        let project = json!({"id":"p","serverId":"a","path":{"bytesB64":"same"}});
        let key = super::status_key(&project);
        for field in ["id", "serverId"] {
            let mut other = project.clone();
            other[field] = json!("other");
            assert_ne!(key, super::status_key(&other));
        }
        let mut other = project;
        other["path"]["bytesB64"] = json!("another");
        assert_ne!(key, super::status_key(&other));
    }
    #[test]
    fn library_empty_states_require_a_successful_list_read() {
        let mut resources = Resources::default();
        assert_eq!(
            library_notice(resources.get("page"), 0, "", false),
            Some(LibraryNotice::Loading)
        );
        let read = resources.begin("page").unwrap();
        assert_eq!(
            library_notice(resources.get("page"), 0, "missing", true),
            Some(LibraryNotice::Loading)
        );
        resources.complete("page", read, Err("Connection closed".into()));
        assert_eq!(
            library_notice(resources.get("page"), 0, "", false),
            Some(LibraryNotice::Error {
                detail: "Connection closed",
                cached: false,
            })
        );
        let retry = resources.begin("page").unwrap();
        assert_eq!(
            library_notice(resources.get("page"), 0, "", false),
            Some(LibraryNotice::Loading)
        );
        resources.complete("page", retry, Ok(json!([])));
        assert!(matches!(
            library_notice(resources.get("page"), 0, "  ", false),
            Some(LibraryNotice::Empty(message)) if message.starts_with("No saved projects.")
        ));
        assert!(matches!(
            library_notice(resources.get("page"), 0, "", true),
            Some(LibraryNotice::Empty(message)) if message.starts_with("No favorite projects.")
        ));
        assert_eq!(
            library_notice(resources.get("page"), 0, "missing", true),
            Some(LibraryNotice::Empty("No projects match your search."))
        );
    }
    #[test]
    fn library_refresh_keeps_bookmarks_and_never_misreports_a_failed_search_as_empty() {
        let mut resources = Resources::default();
        let bookmarks =
            json!([{"id":"one","name":"日本語 project","path":{"display":"/projects/日本語"}}]);
        let read = resources.begin("page").unwrap();
        resources.complete("page", read, Ok(bookmarks.clone()));
        assert_eq!(library_notice(resources.get("page"), 1, "", false), None);

        let background = resources.begin_refresh("page", false).unwrap();
        assert_eq!(library_notice(resources.get("page"), 1, "", false), None);
        resources.complete("page", background, Ok(bookmarks.clone()));
        let refresh = resources.begin_manual("page").unwrap();
        assert_eq!(resources.get("page").unwrap().value, bookmarks);
        assert_eq!(
            library_notice(resources.get("page"), 0, "no match", false),
            Some(LibraryNotice::Refreshing)
        );
        resources.complete("page", refresh, Err("Server unavailable".into()));
        assert_eq!(resources.get("page").unwrap().value, bookmarks);
        assert_eq!(
            library_notice(resources.get("page"), 0, "no match", false),
            Some(LibraryNotice::Error {
                detail: "Server unavailable",
                cached: true,
            })
        );
        let retry = resources.begin("page").unwrap();
        assert_eq!(resources.get("page").unwrap().value, bookmarks);
        resources.complete("page", retry, Ok(bookmarks));
        assert_eq!(
            library_notice(resources.get("page"), 0, "no match", false),
            Some(LibraryNotice::Empty("No projects match your search."))
        );
    }
    #[test]
    fn rename_preserves_identity_and_byte_path_and_rejects_invalid_names() {
        let p = json!({"id":"project","serverId":"server","name":"Old","path":{"display":"/repo�","bytesB64":"L3JlcG//"}});
        let renamed = rename(&p, "server", " New ").unwrap();
        assert_eq!(renamed["name"], "New");
        assert_eq!(renamed["path"], p["path"]);
        assert_eq!(renamed["id"], p["id"]);
        assert!(rename(&p, "other", "Name").is_none());
        for name in ["", "  ", "bad\nname", &"é".repeat(129)] {
            assert!(rename(&p, "server", name).is_none());
        }
        assert!(rename(&p, "server", &"é".repeat(128)).is_some());
    }
}
