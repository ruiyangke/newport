//! Presentation of a verified clean checkout, with one bounded commit read.
use super::*;
use gpui_kit::component::skeleton::Skeleton;
use newport_client::git_clean::{self as clean, WorkingTree};

const COMMIT: &str = "git-clean-commit";

impl Newport {
    pub(super) fn git_is_clean(&self) -> bool {
        self.git.tab == 0
            && !self.git.changes.filtering()
            && !self.git.changes.searching()
            && WorkingTree::for_page(
                &self.git.page,
                self.resources.get("git-list"),
                self.git.error.is_some(),
            ) == WorkingTree::Clean
    }

    pub(super) fn load_clean_commit(&mut self) {
        if !self.git_is_clean() {
            self.resources.invalidate(COMMIT);
            return;
        }
        let Some(oid) = clean::head_oid(&self.git.page) else {
            self.resources.invalidate(COMMIT);
            return;
        };
        if self.resources.get(COMMIT).is_some() {
            return;
        }
        self.request(
            COMMIT,
            "git_request",
            json!({"serverId":self.server,"request":{
                "method":"repo.commit", "params":{"repoId":self.git_repository_id(),"commitOid":oid}
            }}),
        );
    }

    pub(super) fn git_clean_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let head = &self.git.page["metadata"]["head"];
        let oid = clean::head_oid(&self.git.page);
        let resource = self.resources.get(COMMIT);
        let commit = resource
            .filter(|r| !r.pending && r.error.is_none())
            .and_then(|r| clean::matching_commit(&self.git.page, &r.value));
        let pending = oid.is_some() && resource.is_none_or(|r| r.pending);
        let failed = oid.is_some() && resource.is_some_and(|r| !r.pending) && commit.is_none();
        empty::content(
            IconName::CircleCheck,
            "No local changes",
            "Your working tree is clean.",
            cx,
        )
        .when(head["unborn"] == true, |view| {
            view.child(
                crate::label("No commits yet")
                    .text_size(px(crate::ui::FONT_SIZE))
                    .text_center(),
            )
        })
        .when_some(oid, |view, oid| {
            view.child(
                div()
                    .mt(px(8.))
                    .w_full()
                    .max_w(px(420.))
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .rounded(px(crate::ui::RADIUS))
                    .border_1()
                    .border_color(cx.theme().border)
                    .px(px(10.))
                    .py(px(6.))
                    .child(
                        crate::label(oid.chars().take(7).collect::<String>())
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(crate::ui::FONT_SIZE))
                            .flex_shrink_0(),
                    )
                    .when(pending, |view| {
                        view.child(
                            Skeleton::new()
                                .h(px(12.))
                                .flex_1()
                                .rounded(px(crate::ui::RADIUS)),
                        )
                    })
                    .when_some(commit, |view, commit| {
                        view.child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .truncate()
                                .text_size(px(crate::ui::FONT_SIZE))
                                .child(
                                    text(&commit["message"], "display")
                                        .lines()
                                        .next()
                                        .unwrap_or("Empty commit message")
                                        .to_owned(),
                                ),
                        )
                        .child(
                            crate::label(git_commit_header::relative_time(
                                &commit["time"],
                                chrono::Utc::now().timestamp(),
                            ))
                            .text_size(px(crate::ui::FONT_SIZE))
                            .flex_shrink_0(),
                        )
                    })
                    .when(failed, |view| {
                        view.child(
                            crate::label("Commit details unavailable")
                                .text_size(px(crate::ui::FONT_SIZE)),
                        )
                        .child(
                            crate::controls::compact_button("clean-commit-retry")
                                .label("Retry")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.resources.invalidate(COMMIT);
                                    this.load_clean_commit();
                                    cx.notify();
                                })),
                        )
                    }),
            )
        })
        .child(
            div()
                .mt(px(12.))
                .flex()
                .flex_wrap()
                .justify_center()
                .gap(px(8.))
                .child(
                    crate::controls::button("clean-view-history")
                        .label("View history")
                        .disabled(self.git_write_pending())
                        .on_click(cx.listener(|this, _, _, cx| this.git_navigate(1, cx))),
                )
                .child(
                    crate::controls::button("clean-open-files")
                        .label("Open in Files")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.git_open_files(window, cx);
                        })),
                ),
        )
        .into_any_element()
    }
}
