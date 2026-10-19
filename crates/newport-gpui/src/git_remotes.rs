//! Remote names are paged independently from the selected configuration and
//! captured editors. Searching never retargets a pending write or fetches URLs.
use super::*;

fn remote_url(id: &'static str, title: &'static str, url: Option<&str>) -> impl IntoElement {
    let value = url.unwrap_or("Not configured");
    div()
        .id(id)
        .role(accesskit::Role::Label)
        .aria_label(format!("{title}: {value}"))
        .test_support()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_3()
        .child(crate::label(title).text_size(px(crate::ui::FONT_SIZE)))
        .child(
            div()
                .min_w_0()
                .text_size(px(crate::ui::FONT_SIZE))
                .child(value.to_owned()),
        )
}

pub(super) fn selected_detail(selected: &Value, detail: &Value) -> Option<Value> {
    (selected["name"].as_str().is_some_and(|s| !s.is_empty())
        && selected["name"] == detail["name"]
        && detail["token"].as_str().is_some_and(|s| !s.is_empty()))
    .then(|| detail.clone())
}
impl Newport {
    pub(super) fn git_selected_remote(&self) -> Option<Value> {
        let resource = self.resources.get("git-remote-detail")?;
        if resource.pending || resource.error.is_some() {
            return None;
        }
        selected_detail(&self.git.selected, &resource.value)
    }
    pub(super) fn git_remote_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        git_listing::toolbar(
            &self.git.page,
            "remote",
            "remotes",
            crate::controls::compact_button("add-git-remote")
                .label("Add remote")
                .disabled(
                    !self.can_edit_remote()
                        || self.git.remote_edit.is_some()
                        || self.git.pull_form.is_some(),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.prepare_remote_edit(git_remote_edit::Kind::Add, window, cx)
                })),
            cx,
        )
    }
    pub(super) fn git_remote_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let remote = self.git_selected_remote();
        div()
            .id("git-remote-inspector")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(
                                        self.git.selected["name"]
                                            .as_str()
                                            .unwrap_or("Remotes")
                                            .to_owned(),
                                    ),
                            )
                            .child(self.repository_settings_button(cx)),
                    )
                    .when(
                        self.git.selected.is_null()
                            && self.git.remote_edit.is_none()
                            && self.git.settings.is_none()
                            && !self.git.settings_open,
                        |view| {
                            view.child(empty::pane(IconName::Network, "Choose a remote", "Select a remote to view its URLs and manage transfers.", cx))
                        },
                    )
                    .child(self.status("git-remote-detail", cx))
                    .child(self.status("git-remote-status", cx))
                    .when(
                        self.resources
                            .get("git-remote-status")
                            .is_some_and(|resource| resource.error.is_some()),
                        |view| {
                            view.child(
                                crate::controls::button("retry-remote-status")
                                    .label("Retry repository status")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.resources.invalidate("git-remote-status");
                                        this.request(
                                            "git-remote-status",
                                            "git_request",
                                            json!({"serverId":this.server,"request":{"method":"repo.status","params":{"repoId":this.git_repository_id(),"pageSize":1}}}),
                                        );
                                        cx.notify();
                                    })),
                            )
                        },
                    )
                    .when(
                        self.resources
                            .get("git-remote-detail")
                            .is_some_and(|r| r.error.is_some()),
                        |view| {
                            view.child(
                                crate::controls::button("retry-remote-detail")
                                    .label("Retry remote")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.git_select_remote();
                                        cx.notify();
                                    })),
                            )
                        },
                    )
                    .when_some(remote, |view, remote| {
                        view.child(remote_url("remote-fetch-url", "Fetch URL", remote["url"].as_str()))
                            .child(remote_url("remote-push-url", "Push URL", remote["pushUrl"].as_str().or(remote["url"].as_str())))
                            .child(self.git_remote_actions(cx))
                    })
                    .when(
                        self.git.settings_open && self.project_dialog_rendering,
                        |view| {
                            view.child(self.status("git-repository-settings", cx))
                                .when(
                                    self.resources
                                        .get("git-repository-settings")
                                        .is_some_and(|r| r.error.is_some()),
                                    |view| {
                                        view.child(
                                            crate::controls::button("retry-repository-settings")
                                                .label("Retry settings")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.resources
                                                        .invalidate("git-repository-settings");
                                                    this.git_load_repository_settings();
                                                    cx.notify();
                                                })),
                                        )
                                    },
                                )
                                .child(
                                    crate::controls::button("cancel-settings-loading")
                                        .label("Cancel")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.git.settings_open = false;
                                            this.resources.invalidate("git-repository-settings");
                                            cx.notify();
                                        })),
                                )
                        },
                    ),
            )
            .when(self.project_dialog_rendering, |view| {
                view.child(self.git_remote_edit_view(cx))
            })
            .child(self.status("git-remote-review", cx))
            .when(
                self.git.settings.is_some() && self.project_dialog_rendering,
                |view| view.child(self.status("git-repository-settings", cx)),
            )
            .child(
                div()
                    .max_w(px(720.))
                    .when(self.project_dialog_rendering, |view| {
                        view.child(self.repository_settings_view(cx))
                    }),
            )
            .when(self.project_dialog_rendering, |view| {
                view.child(self.git_pull_view(cx))
            })
            .into_any_element()
    }
}
#[cfg(test)]
mod tests {
    use super::{remote_url, selected_detail};
    use gpui_kit::base::test_support::snapshots;
    use gpui_kit::{self as gpui, Context, IntoElement, Render, Window, div, prelude::*};
    use serde_json::{Value, json};
    struct UrlHost;
    impl Render for UrlHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex()
                .flex_col()
                .child(remote_url(
                    "fetch-url",
                    "Fetch URL",
                    Some("https://host.test/团队/repo.git"),
                ))
                .child(remote_url("push-url", "Push URL", None))
        }
    }
    #[gpui::test]
    fn remote_urls_expose_labeled_values_and_missing_configuration(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui::init(cx);
            crate::theme::install(false, cx);
        });
        let (_, visual) = cx.add_window_view(|_, _| UrlHost);
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            let nodes = snapshots(window);
            for expected in [
                "Fetch URL: https://host.test/团队/repo.git",
                "Push URL: Not configured",
            ] {
                let node = nodes
                    .iter()
                    .find(|node| node.label() == Some(expected))
                    .unwrap();
                assert_eq!(node.role(), Some(gpui::Role::Label));
            }
        });
    }
    #[test]
    fn details_require_the_selected_name_and_reviewable_configuration() {
        let selected = json!({"name":"backup"});
        assert!(selected_detail(&selected, &json!({"name":"origin","token":"t"})).is_none());
        assert!(selected_detail(&selected, &json!({"name":"backup"})).is_none());
        assert!(selected_detail(&Value::Null, &Value::Null).is_none());
        assert!(selected_detail(&selected, &json!({"name":"backup","token":"t"})).is_some());
    }
}
