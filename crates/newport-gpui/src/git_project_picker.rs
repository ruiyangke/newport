//! Repository navigation uses saved bookmarks, never discovered worktrees.
use super::*;

/// Resolve at activation time: a menu can outlive a rename, removal or server
/// change. Keep byte paths intact and never dispatch its captured stale row.
fn destination(projects: &Value, server: &str, captured: &Value) -> Option<Value> {
    let id = captured["id"].as_str()?;
    let path = captured["path"]["bytesB64"].as_str()?;
    if captured["serverId"].as_str() != Some(server) {
        return None;
    }
    projects
        .as_array()?
        .iter()
        .find(|project| {
            project["id"].as_str() == Some(id)
                && project["serverId"].as_str() == Some(server)
                && project["path"]["bytesB64"].as_str() == Some(path)
        })
        .cloned()
}

/// Bookmarks belong to the server; every other read belongs to the checkout.
/// Removing the old generations also rejects late open/diff/preflight replies.
pub(super) fn clear_checkout(resources: &mut newport_client::resources::Resources) {
    let _ = resources.take_prefix_except("", &["page"]);
}

impl Newport {
    pub(super) fn open_saved_project(&mut self, captured: &Value, cx: &mut Context<Self>) {
        if self.page != Page::Projects || self.git_write_pending() || self.project_dialog_open {
            return;
        }
        let Some(project) = destination(&self.value("page"), &self.server, captured) else {
            return;
        };
        if self
            .selection
            .as_ref()
            .is_some_and(|current| current["id"] == project["id"])
        {
            return;
        }
        self.git_release();
        clear_checkout(&mut self.resources);
        self.restore_git = None;
        self.git = git::Workspace::default();
        self.selection = Some(project.clone());
        self.git_request_open(project["path"].clone());
        self.save_workspace_selection();
        cx.notify();
    }

    pub(super) fn git_project_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let name = self
            .selection
            .as_ref()
            .map(|project| text(project, "name"))
            .unwrap_or_default();
        let busy = self.git_write_pending() || self.project_dialog_open;
        let owner = cx.weak_entity();
        git_layout::picker("git-project-picker", IconName::FolderGit2, name.clone())
            .accessibility_label(format!("Current repository: {name}"))
            .disabled(busy)
            .dropdown_menu(move |mut menu, _, cx| {
                let Some(entity) = owner.upgrade() else {
                    return menu;
                };
                let this = entity.read(cx);
                let server = this.server.clone();
                let busy = this.git_write_pending() || this.project_dialog_open;
                let current = this.selection.as_ref().map(|project| project["id"].clone());
                let projects = rows(&this.value("page"));
                let back = owner.clone();
                let back_server = server.clone();
                menu = menu
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
                    .filter(|project| project["serverId"].as_str() == Some(&server))
                {
                    let open = owner.clone();
                    let selected = current.as_ref() == Some(&project["id"]);
                    menu = menu.item(
                        PopupMenuItem::new(text(&project, "name"))
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
    use newport_client::resources::Resources;
    use serde_json::{Value, json};

    #[test]
    fn stale_picker_cannot_open_removed_moved_or_other_server_bookmarks() {
        let captured =
            json!({"id":"project", "serverId":"a", "path":{"bytesB64":"L3JlcG8="}, "name":"Old"});
        let mut renamed = captured.clone();
        renamed["name"] = json!("New");
        assert_eq!(
            destination(&json!([renamed]), "a", &captured),
            Some(renamed)
        );
        assert!(destination(&json!([captured]), "b", &captured).is_none());
        assert!(destination(&json!([]), "a", &captured).is_none());
        let mut moved = captured.clone();
        moved["path"]["bytesB64"] = json!("L290aGVy");
        assert!(destination(&json!([moved]), "a", &captured).is_none());
        assert!(destination(&json!([]), "a", &Value::Null).is_none());
    }

    #[test]
    fn switching_preserves_bookmarks_and_rejects_late_checkout_responses() {
        let mut resources = Resources::default();
        let bookmarks = resources.begin("page").unwrap();
        let rows = json!([{"id":"project", "serverId":"a"}]);
        resources.complete("page", bookmarks, Ok(rows.clone()));
        let old_open = resources.begin("repository").unwrap();
        let old_review = resources.begin("git-sync-review-status").unwrap();
        let old_diff = resources.begin("git-diff").unwrap();
        clear_checkout(&mut resources);
        assert_eq!(resources.get("page").unwrap().value, rows);
        let new_open = resources.begin("repository").unwrap();
        assert!(!resources.complete("repository", old_open, Ok(json!({"repoId":"old"}))));
        assert!(!resources.complete("git-sync-review-status", old_review, Ok(json!({}))));
        assert!(!resources.complete("git-diff", old_diff, Ok(json!({}))));
        assert!(resources.complete("repository", new_open, Ok(json!({"repoId":"new"}))));
    }
}
