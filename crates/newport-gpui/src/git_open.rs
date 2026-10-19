//! An unavailable checkout is recoverable without exposing inert Git controls.
use super::*;
impl Newport {
    pub(super) fn git_recover_missing_checkout(&mut self, key: &str) -> bool {
        let code = self
            .resources
            .get(key)
            .and_then(|row| row.error_code.as_deref());
        if !matches!(key, "git-list" | "git-sync-status")
            || self.page != Page::Projects
            || !self.git.repo["repoId"].is_string()
            || self.git_write_pending()
            || self.git.switching_path.is_some()
            || self.project_dialog_open
            || !newport_client::git_checkout_recovery::unavailable(code)
        {
            return false;
        }
        let Some(project) = self.selection.as_ref() else {
            return false;
        };
        let Some(root) = newport_client::git_checkout_recovery::destination(
            code,
            &self.server,
            project,
            &self.git.opening_path,
        ) else {
            self.git.error = Some(format!(
                "This project’s folder is unavailable on the server: {}",
                text(&self.git.repo["root"], "display")
            ));
            return true;
        };
        let missing = text(&self.git.repo["root"], "display");
        self.git_release();
        // Retire every old checkout generation, including duplicate missing
        // replies. A failed open of the bookmark stops on the normal Retry UI.
        git_project_picker::clear_checkout(&mut self.resources);
        self.restore_git = None;
        self.git = git::Workspace::default();
        self.git.returned_from_missing_checkout = Some(missing);
        self.git_request_open(root);
        true
    }
    pub(super) fn git_open_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page != Page::Projects || !self.git.repo["repoId"].is_string() {
            return;
        }
        // repo.open resolves subdirectories and symlinks. Use the returned
        // checkout root, not the bookmark or the originally entered path.
        let destination = json!({"serverId":self.server,"path":self.git.repo["root"]});
        self.project_open_files(&destination, window, cx);
    }
    pub(super) fn git_request_open(&mut self, path: Value) {
        self.git_open_with_retry(path, false);
    }
    fn git_open_with_retry(&mut self, path: Value, retry: bool) {
        self.project_checks = None;
        if let Some(project) = self.selection.as_ref() {
            self.resources
                .invalidate(&project_library::status_key(project));
        }
        self.git.opening_path = path.clone();
        self.resources.invalidate("repository");
        self.request(
            "repository",
            "git_request",
            json!({"serverId":self.server,"retryConnection":retry,"request":{"method":"repo.open","params":{"path":path}}}),
        );
    }
    pub(super) fn git_all_projects(&mut self, cx: &mut Context<Self>) {
        self.git_release();
        let library = self.resources.take_cached(&["page"]);
        self.resources.clear();
        self.resources.restore(library);
        self.selection = None;
        self.git = git::Workspace::default();
        self.restore_git = None;
        self.save_workspace_selection();
        self.refresh();
        cx.notify();
    }
    pub(super) fn git_open_screen(&self, cx: &mut Context<Self>) -> AnyElement {
        let resource = self.resources.get("repository");
        let pending = resource.is_some_and(|r| r.pending);
        let error = resource.and_then(|r| r.error.clone());
        if pending {
            return project_loading::workspace(
                crate::controls::button("open-back-projects")
                    .label("All projects")
                    .on_click(cx.listener(|this, _, _, cx| this.git_all_projects(cx))),
                cx,
            );
        }
        let root = self
            .selection
            .as_ref()
            .map(|p| p["path"].clone())
            .unwrap_or_default();
        let different_root =
            root["bytesB64"].is_string() && root["bytesB64"] != self.git.opening_path["bytesB64"];
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .items_start()
            .gap_3()
            .p_5()
            .child(
                crate::controls::button("open-back-projects")
                    .label("All projects")
                    .on_click(cx.listener(|this, _, _, cx| this.git_all_projects(cx))),
            )
            .when(!pending, |view| {
                view.child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Could not open this checkout"),
                )
            })
            .child(crate::label(text(&self.git.opening_path, "display")))
            .when_some(error, |view, error| {
                view.child(div().text_color(cx.theme().danger).child(error))
            })
            .when(!pending, |view| {
                view.child(crate::label(
                    "Retry the connection, or open the saved project to locate a moved worktree.",
                ))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            crate::controls::button("retry-open-repository")
                                .label("Retry")
                                .disabled(!self.git.opening_path["bytesB64"].is_string())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.git_open_with_retry(this.git.opening_path.clone(), true);
                                    cx.notify();
                                })),
                        )
                        .when(different_root, |view| {
                            view.child(
                                crate::controls::button("open-saved-project")
                                    .label("Open saved project")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.git_open_with_retry(root.clone(), true);
                                        cx.notify();
                                    })),
                            )
                        }),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit as gpui;
    use serde_json::json;

    #[gpui::test]
    fn checkout_retry_requests_transport_recovery_but_ordinary_opens_do_not(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (view, cx) = cx.add_window_view(crate::page_perf_tests::owner);
        view.update(cx, |this, _| {
            let (work, requests) = std::sync::mpsc::sync_channel(8);
            this.work = work.into();
            let path = json!({"bytesB64":"L3Nydi9hcHA=","display":"/srv/app"});
            this.git_request_open(path.clone());
            let initial = requests.try_recv().unwrap();
            assert_eq!(initial.args["retryConnection"], false);
            this.resources.complete(
                "repository",
                initial.generation,
                Err("SSH agent unavailable: early eof".into()),
            );
            this.git_open_with_retry(path.clone(), true);
            let retry = requests.try_recv().unwrap();
            assert_eq!(retry.method, "git_request");
            assert_eq!(retry.args["retryConnection"], true);
            assert_eq!(retry.args["request"]["params"]["path"], path);
            assert!(retry.generation > initial.generation);
            assert!(this.resources.get("repository").unwrap().pending);
            assert!(requests.try_recv().is_err());
        });
    }
}
