//! Repository navigation uses saved bookmarks, never discovered worktrees.
use super::*;
use desktop::preferences::Project;

/// Resolve at activation time: a menu can outlive a rename, removal or server
/// change. Keep byte paths intact and never dispatch its captured stale row.
fn destination(projects: &[Project], server: &str, captured: &Project) -> Option<Project> {
    if captured.server.to_string() != server {
        return None;
    }
    projects
        .iter()
        .find(|project| {
            project.id == captured.id
                && project.server == captured.server
                && project.path.bytes_b64 == captured.path.bytes_b64
        })
        .cloned()
}

/// Bookmarks belong to the server; every other read belongs to the checkout.
/// Removing the old generations also rejects late open/diff/preflight replies.
pub(super) fn clear_checkout(resources: &mut newport_client::resources::Resources) {
    let _ = resources.take_prefix_except("", &["projects"]);
}

impl Newport {
    pub(super) fn open_saved_project(&mut self, captured: &Project, cx: &mut Context<Self>) {
        if self.page != Page::Projects || self.git_write_pending() || self.project_dialog_open {
            return;
        }
        let Some(project) = destination(
            self.library_projects().unwrap_or_default(),
            &self.server,
            captured,
        ) else {
            return;
        };
        if self
            .selection
            .as_ref()
            .is_some_and(|current| current.id == project.id)
        {
            return;
        }
        self.git_release();
        clear_checkout(&mut self.resources);
        self.restore_git = None;
        self.git = git::Workspace::default();
        let path = project.path.clone();
        self.selection = Some(project);
        self.git_request_open(path);
        self.save_workspace_selection();
        cx.notify();
    }

    pub(super) fn git_project_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let name = self
            .selection
            .as_ref()
            .map(|project| project.name.clone())
            .unwrap_or_default();
        let busy = self.git_write_pending() || self.project_dialog_open;
        let owner = cx.weak_entity();
        git_layout::picker("git-project-picker", IconName::FolderGit2, name.clone())
            .icon(Icon::new(IconName::FolderGit2).text_color(cx.theme().muted_foreground))
            .accessibility_label(format!("Current repository: {name}"))
            .disabled(busy)
            .dropdown_menu(move |mut menu, _, cx| {
                let Some(entity) = owner.upgrade() else {
                    return menu;
                };
                let this = entity.read(cx);
                let server = this.server.clone();
                let busy = this.git_write_pending() || this.project_dialog_open;
                let current = this.selection.as_ref().map(|project| project.id);
                let projects = this.library_projects().unwrap_or_default().to_vec();
                let back = owner.clone();
                let back_server = server.clone();
                menu = menu
                    .min_w(px(git_layout::PICKER_WIDTH))
                    .max_w(px(git_layout::PICKER_WIDTH))
                    .max_h(px(git_layout::PICKER_MAX_HEIGHT))
                    .scrollable(true)
                    .item(
                        PopupMenuItem::new("All projects")
                            .icon(IconName::List)
                            .disabled(busy)
                            .on_click(move |_, _, cx| {
                                let _ = back.update(cx, |this, cx| {
                                    if this.server == back_server
                                        && this.page == Page::Projects
                                        && !this.git_write_pending()
                                        && !this.project_dialog_open
                                    {
                                        this.git_all_projects(cx);
                                    }
                                });
                            }),
                    )
                    .separator();
                for project in projects
                    .into_iter()
                    .filter(|project| project.server.to_string() == server)
                {
                    let open = owner.clone();
                    let selected = current == Some(project.id);
                    menu = menu.item(
                        PopupMenuItem::new(project.name.clone())
                            .icon(IconName::FolderGit2)
                            .checked(selected)
                            .disabled(busy || selected)
                            .on_click(move |_, _, cx| {
                                let _ = open
                                    .update(cx, |this, cx| this.open_saved_project(&project, cx));
                            }),
                    );
                }
                menu
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{clear_checkout, destination};
    use crate::project_library_state::typed_fixture::{TYPED_SECOND, TYPED_SERVER, bookmark};
    use newport_client::resources::Resources;

    #[test]
    fn stale_picker_cannot_open_removed_moved_or_other_server_bookmarks() {
        let captured = bookmark(1, TYPED_SERVER, "Old", "/repo");
        let mut renamed = captured.clone();
        renamed.name = "New".into();
        assert_eq!(
            destination(std::slice::from_ref(&renamed), TYPED_SERVER, &captured),
            Some(renamed)
        );
        assert!(destination(std::slice::from_ref(&captured), TYPED_SECOND, &captured).is_none());
        assert!(destination(&[], TYPED_SERVER, &captured).is_none());
        let mut moved = captured.clone();
        moved.path = newport_protocol::git::Path::new(b"/other");
        assert!(destination(&[moved], TYPED_SERVER, &captured).is_none());
        let mut foreign = captured.clone();
        foreign.server = uuid::Uuid::parse_str(TYPED_SECOND).unwrap();
        assert!(destination(std::slice::from_ref(&captured), TYPED_SERVER, &foreign).is_none());
    }

    #[test]
    fn switching_preserves_bookmarks_and_rejects_late_checkout_responses() {
        let mut resources = Resources::default();
        let bookmarks = resources.begin("projects").unwrap();
        resources.complete_owned("projects", bookmarks, Ok(true));
        let old_open = resources.begin("repository").unwrap();
        let old_review = resources.begin("git-sync-review-status").unwrap();
        let old_diff = resources.begin("git-diff").unwrap();
        clear_checkout(&mut resources);
        assert_eq!(resources.get("projects").unwrap().data_revision, bookmarks);
        let new_open = resources.begin("repository").unwrap();
        assert!(!resources.complete_owned("repository", old_open, Ok(true)));
        assert!(!resources.complete_owned("git-sync-review-status", old_review, Ok(true)));
        assert!(!resources.complete_owned("git-diff", old_diff, Ok(true)));
        assert!(resources.complete_owned("repository", new_open, Ok(true)));
    }
}
