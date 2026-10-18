//! GitChangesSplit's dimensions and compact list-to-detail navigation.
use super::*;

pub(super) const LIST_WIDTH: f32 = 290.;
pub(super) const MIN_LIST_WIDTH: f32 = 240.;
pub(super) const MAX_LIST_WIDTH: f32 = 560.;

/// Keep the identity beside its icon and caret; only the label yields space.
pub(super) fn picker(id: impl Into<ElementId>, icon: IconName, title: String) -> Button {
    let mut content = div()
        .text_size(px(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .gap(px(6.));
    crate::controls::button(id)
        .ghost()
        .icon(icon)
        .label(title)
        .dropdown_caret(true)
        .px(px(8.))
        .min_w(px(68.))
        .max_w(px(280.))
        .flex_shrink(1.)
        .content_style(
            content.style().clone(),
            gpui_kit::component::Size::Size(px(15.)),
        )
}

/// The row owns selection and hover; nested choice buttons must stay transparent.
pub(super) fn picker_row(
    id: impl Into<ElementId>,
    current: bool,
    highlighted: bool,
    cx: &App,
) -> Stateful<Div> {
    let background = if current {
        theme::git_selection(cx.theme().is_dark())
    } else if highlighted {
        theme::row_hover(cx)
    } else {
        cx.theme().transparent
    };
    div()
        .id(id)
        .h(px(48.))
        .w_full()
        .min_w_0()
        .flex()
        .items_center()
        .px_1()
        .gap_1()
        .rounded_sm()
        .border_1()
        .border_color(if highlighted {
            cx.theme().ring
        } else {
            cx.theme().transparent
        })
        .bg(background)
        .hover(|row| {
            row.bg(if current {
                theme::git_selection(cx.theme().is_dark())
            } else {
                theme::row_hover(cx)
            })
        })
}

pub(super) fn picker_choice(id: impl Into<ElementId>, cx: &App) -> Button {
    crate::controls::button(id)
        .custom(gpui_kit::component::button::ButtonCustomVariant::new(cx))
        .h_full()
        .min_h_0()
        .max_h_full()
        .flex_1()
        .min_w_0()
}

pub(super) fn picker_container() -> StyleRefinement {
    div()
        .min_w(px(68.))
        .max_w(px(280.))
        .flex_shrink(1.)
        .style()
        .clone()
}

pub(super) fn toolbar(
    navigation: [AnyElement; 3],
    actions: [AnyElement; 3],
    width: f32,
    cx: &App,
) -> Div {
    let compact = width < 700.;
    let mut identity = div()
        .flex()
        .min_w_0()
        .items_center()
        .gap(px(4.))
        .when(compact, |row| row.w_full());
    for (index, picker) in navigation.into_iter().enumerate() {
        if index > 0 {
            identity = identity.child(
                div()
                    .w(px(1.))
                    .h(px(16.))
                    .mx(px(2.))
                    .flex_shrink_0()
                    .bg(cx.theme().border),
            );
        }
        identity = identity.child(picker);
    }
    div()
        .flex()
        .flex_shrink_0()
        .min_w_0()
        .min_h(px(46.))
        .px(px(12.))
        .py(px(8.))
        .gap(px(4.))
        .items_center()
        .border_b_1()
        .border_color(cx.theme().border)
        .when(compact, |bar| bar.flex_col().gap(px(6.)))
        .child(identity)
        .child(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(6.))
                .ml_auto()
                .pl(px(8.))
                .children(actions),
        )
}

pub(super) fn compact(content_width: f32, tab: usize) -> bool {
    content_width < 700. && matches!(tab, 0 | 1 | 3 | 4 | 6)
}

impl git::Workspace {
    fn detail_title(&self) -> Option<String> {
        if !self.selected.is_object() {
            return None;
        }
        Some(match self.tab {
            0 => self.selected["path"]["display"]
                .as_str()
                .or_else(|| self.selected["oldPath"]["display"].as_str())
                .unwrap_or("Selected file")
                .to_owned(),
            1 => git_commit_header::history_subject(&self.selected),
            3 => self.selected["message"]
                .as_str()
                .filter(|message| !message.is_empty())
                .unwrap_or("Unnamed stash")
                .lines()
                .next()
                .unwrap_or("Unnamed stash")
                .to_owned(),
            4 => text(&self.selected["name"], "display"),
            6 => text(&self.selected, "name"),
            _ => return None,
        })
    }

    fn resume_detail(&mut self) {
        if self.detail_title().is_some() {
            self.detail_open = true;
        }
    }
}

impl Newport {
    pub(super) fn git_resume_detail(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(title) = self.git.detail_title() else {
            return div().into_any_element();
        };
        resume_bar(
            title,
            crate::controls::button("git-resume-detail")
                .label(if matches!(self.git.tab, 0 | 1 | 3) {
                    "Show diff"
                } else {
                    "Show details"
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    // Returning to an inspection must retain its comparison,
                    // loaded pages, scroll and selection. No read is started.
                    this.git.resume_detail();
                    cx.notify();
                })),
            cx,
        )
        .into_any_element()
    }
}

fn resume_bar(title: String, button: Button, cx: &App) -> Div {
    div()
        .flex()
        .items_center()
        .flex_shrink_0()
        .min_w_0()
        .gap(px(8.))
        .px(px(10.))
        .py(px(8.))
        .border_t_1()
        .border_color(cx.theme().border)
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .child(title),
        )
        .child(button.flex_shrink_0())
}

#[cfg(test)]
mod tests {
    use super::{compact, resume_bar};
    use crate::git;
    use gpui_kit::component::button::ButtonVariants;
    use gpui_kit::{
        self as gpui, Context, IntoElement, Render, TestAppContext, Window, div, prelude::*, px,
        size,
    };
    use serde_json::json;

    struct PickerRow {
        current: bool,
        highlighted: bool,
    }
    impl Render for PickerRow {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().p(px(20.)).child(
                super::picker_row("choice-row", self.current, self.highlighted, cx)
                    .debug_selector(|| "choice-row".into())
                    .child(
                        super::picker_choice("choice", cx)
                            .label("main")
                            .debug_selector(|| "choice".into()),
                    )
                    .child(
                        crate::controls::compact_icon_button("actions")
                            .ghost()
                            .icon(gpui::assets::IconName::Ellipsis)
                            .debug_selector(|| "actions".into()),
                    ),
            )
        }
    }

    #[gpui::test]
    fn picker_rows_share_hover_and_preserve_current_selection(cx: &mut TestAppContext) {
        cx.update(gpui::component::init);
        let (view, visual) = cx.add_window_view(|_, _| PickerRow {
            current: false,
            highlighted: false,
        });
        visual.simulate_resize(size(px(360.), px(180.)));
        for dark in [false, true] {
            visual.update(|_, cx| crate::theme::install(dark, cx));
            for current in [false, true] {
                view.update(visual, |view, cx| {
                    view.current = current;
                    view.highlighted = false;
                    cx.notify();
                });
                visual.update(|window, cx| window.draw(cx).clear(cx));
                let row = visual.debug_bounds("choice-row").unwrap();
                // Hover the choice, then the separate action: both retain the row tint.
                for marker in ["choice", "actions"] {
                    let target = visual.debug_bounds(marker).unwrap().center();
                    visual.simulate_mouse_move(target, None, Default::default());
                    visual.update(|window, cx| {
                        window.draw(cx).clear(cx);
                        let color = if current {
                            crate::theme::git_selection(dark)
                        } else {
                            crate::theme::row_hover(cx)
                        };
                        assert!(
                            window
                                .painted_quads()
                                .iter()
                                .any(|q| q.background == color.into())
                        );
                    });
                    assert_eq!(visual.debug_bounds("choice-row").unwrap(), row);
                }
                visual.simulate_mouse_move(
                    gpui::point(px(350.), px(170.)),
                    None,
                    Default::default(),
                );
                view.update(visual, |view, cx| {
                    view.highlighted = true;
                    cx.notify();
                });
                visual.update(|window, cx| {
                    window.draw(cx).clear(cx);
                    let color = if current {
                        crate::theme::git_selection(dark)
                    } else {
                        crate::theme::row_hover(cx)
                    };
                    assert!(
                        window
                            .painted_quads()
                            .iter()
                            .any(|q| q.background == color.into())
                    );
                });
                assert_eq!(visual.debug_bounds("choice-row").unwrap(), row);
            }
        }
    }

    struct Toolbar;
    impl Render for Toolbar {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            use gpui_kit::{assets::IconName, component::popover::Popover};
            let name = "a-long-repository-worktree-or-branch-name-that-must-truncate";
            let repository = super::picker("repo", IconName::FolderGit2, name.into())
                .debug_selector(|| "repo-picker".into())
                .into_any_element();
            let popup = |id, marker: &'static str| {
                Popover::new(id)
                    .trigger_style(super::picker_container())
                    .trigger(
                        super::picker(id, IconName::GitBranch, name.into())
                            .w_full()
                            .debug_selector(move || marker.into()),
                    )
                    .into_any_element()
            };
            div().size_full().child(
                super::toolbar(
                    [
                        repository,
                        popup("worktree", "worktree-picker"),
                        popup("branch", "branch-picker"),
                    ],
                    [
                        div()
                            .flex()
                            .gap(px(4.))
                            .child(
                                crate::controls::button("transfer")
                                    .max_w(px(200.))
                                    .label(format!("Fetch {name}"))
                                    .debug_selector(|| "transfer".into()),
                            )
                            .child(
                                crate::controls::icon_button("options").icon(IconName::ChevronDown),
                            )
                            .into_any_element(),
                        crate::controls::icon_button("refresh")
                            .icon(IconName::RefreshCw)
                            .debug_selector(|| "refresh".into())
                            .into_any_element(),
                        crate::controls::icon_button("tools")
                            .icon(IconName::Ellipsis)
                            .debug_selector(|| "tools".into())
                            .into_any_element(),
                    ],
                    window.viewport_size().width.into(),
                    cx,
                )
                .debug_selector(|| "toolbar".into()),
            )
        }
    }
    #[gpui::test]
    fn long_toolbar_labels_keep_navigation_and_actions_reachable(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (_, visual) = cx.add_window_view(|_, _| Toolbar);
        for width in [340., 464., 699., 700., 1000., 1500.] {
            visual.simulate_resize(size(px(width), px(200.)));
            visual.update(|window, cx| window.draw(cx).clear(cx));
            let repo = visual.debug_bounds("repo-picker").unwrap();
            let worktree = visual.debug_bounds("worktree-picker").unwrap();
            let branch = visual.debug_bounds("branch-picker").unwrap();
            let transfer = visual.debug_bounds("transfer").unwrap();
            assert_eq!(repo.top(), worktree.top());
            assert_eq!(repo.top(), branch.top());
            assert!(repo.right() <= worktree.left());
            assert!(worktree.right() <= branch.left());
            for marker in [
                "repo-picker",
                "worktree-picker",
                "branch-picker",
                "transfer",
                "refresh",
                "tools",
            ] {
                let bounds = visual.debug_bounds(marker).unwrap();
                assert!(
                    bounds.left() >= px(0.) && bounds.right() <= px(width),
                    "{width}: {marker} {bounds:?}"
                );
                assert_eq!(bounds.size.height, px(28.));
            }
            if width < 700. {
                assert!(transfer.top() > branch.bottom());
            } else {
                assert_eq!(transfer.top(), branch.top());
                assert!(branch.right() <= transfer.left());
                assert_eq!(visual.debug_bounds("toolbar").unwrap().size.height, px(46.));
            }
        }
    }

    #[test]
    fn compact_navigation_preserves_the_review_and_requires_a_selection() {
        let mut view = git::Workspace::default();
        view.resume_detail();
        assert!(!view.detail_open);
        view.selected = json!({"path":{"display":"src/main.rs"},"entryId":"entry"});
        view.diff = json!({"snapshot":"review", "nextCursor":"next", "entries":[1,2]});
        view.diff_staged = true;
        let diff = view.diff.clone();
        view.resume_detail();
        assert!(view.detail_open);
        assert_eq!(view.detail_title().as_deref(), Some("src/main.rs"));
        assert_eq!(view.diff, diff);
        assert!(view.diff_staged);
        assert_eq!(view.selected["entryId"], "entry");
        assert!(compact(699., 0));
        assert!(compact(699., 1));
        assert!(!compact(700., 0));
        assert!(!compact(700., 1));
        assert!(!compact(500., 2));
    }

    struct Preview;
    impl Render for Preview {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .flex_col()
                .justify_end()
                .child(resume_bar(
                    "a/very/long/directory/and/a-long-selected-file-name.rs".into(),
                    crate::controls::button("show-diff-test")
                        .label("Show diff")
                        .debug_selector(|| "resume-button".into()),
                    cx,
                ))
        }
    }

    #[gpui::test]
    fn compact_resume_keeps_the_action_inside_the_pane(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (_, visual) = cx.add_window_view(|_, _| Preview);
        for width in [240., 400., 699.] {
            visual.simulate_resize(size(px(width), px(160.)));
            visual.update(|window, cx| window.draw(cx).clear(cx));
            let button = visual.debug_bounds("resume-button").unwrap();
            assert!(button.left() >= px(0.) && button.right() <= px(width));
            assert!(button.top() >= px(0.) && button.bottom() <= px(160.));
            assert!(button.size.width > px(60.));
        }
    }
}
