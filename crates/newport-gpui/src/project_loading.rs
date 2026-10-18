//! Content-shaped placeholders for Projects. Background reads never add rows.
use super::*;
use gpui_kit::component::skeleton::Skeleton;
fn bar(width: f32, height: f32) -> impl IntoElement {
    Skeleton::new()
        .w(relative(width))
        .h(px(height))
        .rounded(px(3.))
}
fn region(name: &str) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!("loading-{name}")))
        .role(accesskit::Role::Status)
        .aria_label(format!("Loading {name}"))
        .w_full()
        .min_w_0()
        .overflow_hidden()
}
/// Replace a picker’s single metadata line, without adding a separate panel.
pub(super) fn picker_detail(name: &str, content: Option<String>, pending: bool) -> AnyElement {
    div()
        .w_full()
        .min_w_0()
        .h(px(16.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .when(pending, |view| {
            view.child(region(name).child(bar(0.62, 10.)))
        })
        .when(!pending, |view| {
            view.when_some(content, |view, content| {
                view.child(
                    label(content.clone())
                        .id(SharedString::from(format!("{name}-detail")))
                        .role(accesskit::Role::Label)
                        .aria_label(content.clone())
                        .hover_hint(content.clone())
                        .text_xs()
                        .truncate(),
                )
            })
        })
        .into_any_element()
}
pub(super) fn selector(name: &str) -> AnyElement {
    region(name)
        .h(px(crate::controls::CONTROL_HEIGHT))
        .child(bar(1., crate::controls::CONTROL_HEIGHT))
        .into_any_element()
}
/// Loading occupies list slots; retained rows never acquire a second loading block.
pub(super) fn picker_count(loaded: usize, pending: bool) -> usize {
    if pending && loaded == 0 { 4 } else { loaded }
}
pub(super) fn picker_row(name: &str, index: usize) -> AnyElement {
    region(&format!("{name}-{index}"))
        .h(px(48.))
        .flex_shrink_0()
        .px_4()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(bar(if index.is_multiple_of(2) { 0.64 } else { 0.48 }, 13.))
                .child(bar(0.4, 11.)),
        )
        .child(div().w(px(28.)).flex_shrink_0())
        .into_any_element()
}
pub(super) fn listing(name: &str, tab: usize, count: usize) -> AnyElement {
    region(name)
        .flex()
        .flex_col()
        .children((0..count).map(|i| {
            let two = tab != 0 && tab != 6;
            let height = match tab {
                0 => 32.,
                1 => 46.,
                2 => 54.,
                6 => 36.,
                _ => 48.,
            };
            div()
                .h(px(height))
                .flex_shrink_0()
                .px_3()
                .flex()
                .items_center()
                .gap_2()
                .when(tab != 1, |row| {
                    row.child(Skeleton::new().size(px(14.)).rounded(px(3.)))
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(5.))
                        .child(bar(if i % 3 == 0 { 0.76 } else { 0.6 }, 12.))
                        .when(two, |row| {
                            row.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(bar(0.45, 10.))
                                    .child(bar(0.16, 10.)),
                            )
                        }),
                )
        }))
        .into_any_element()
}
pub(super) fn commit_header() -> AnyElement {
    region("commit details")
        .px_4()
        .py_3()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .flex()
                .justify_between()
                .child(bar(0.65, 17.))
                .child(bar(0.16, 28.)),
        )
        .child(bar(0.9, 12.))
        .child(bar(0.72, 12.))
        .child(
            div()
                .flex()
                .gap_3()
                .child(bar(0.19, 11.))
                .child(bar(0.12, 11.))
                .child(bar(0.3, 11.)),
        )
        .into_any_element()
}
pub(super) fn diff() -> AnyElement {
    region("file diff")
        .flex_1()
        .min_h_0()
        .py_2()
        .flex()
        .flex_col()
        .children((0..18).map(|i| {
            div()
                .h(px(22.))
                .px_3()
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_3()
                .child(Skeleton::new().w(px(20.)).h(px(10.)).rounded(px(2.)))
                .child(Skeleton::new().w(px(20.)).h(px(10.)).rounded(px(2.)))
                .child(bar([0.62, 0.44, 0.76, 0.3, 0.54][i % 5], 12.))
        }))
        .into_any_element()
}
pub(super) fn blob() -> AnyElement {
    region("recorded Git content")
        .flex_1()
        .min_h_0()
        .p(px(12.))
        .flex()
        .flex_col()
        .children((0..18).map(|i| {
            div()
                .h(px(20.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_3()
                .child(Skeleton::new().w(px(20.)).h(px(10.)).rounded(px(2.)))
                .child(bar([0.62, 0.44, 0.76, 0.3, 0.54][i % 5], 12.))
        }))
        .into_any_element()
}
pub(super) fn library(width: f32, cx: &App) -> AnyElement {
    region("projects")
        .debug_selector(|| "project-library-loading".into())
        .border_1()
        .border_color(cx.theme().border)
        .rounded(px(8.))
        .child(
            div()
                .h(px(30.))
                .px_3()
                .flex()
                .items_center()
                .bg(cx.theme().sidebar)
                .child(bar(0.12, 11.)),
        )
        .children((0..4).map(|i| {
            div()
                .h(px(56.))
                .px_3()
                .flex()
                .items_center()
                .gap_3()
                .border_t_1()
                .border_color(cx.theme().border)
                .child(Skeleton::new().size(px(20.)).rounded(px(4.)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(bar(if i % 2 == 0 { 0.55 } else { 0.4 }, 14.))
                        .child(bar(0.75, 11.)),
                )
                .when(width >= 650., |row| {
                    row.child(div().w(px(170.)).child(bar(0.5, 12.)))
                })
                .when(width >= 430., |row| {
                    row.child(div().w(px(170.)).child(bar(0.4, 12.)))
                })
        }))
        .into_any_element()
}
pub(super) fn workspace(navigation: impl IntoElement, cx: &App) -> AnyElement {
    region("repository")
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(
            div()
                .h(px(46.))
                .flex_shrink_0()
                .px_3()
                .flex()
                .items_center()
                .gap_3()
                .border_b_1()
                .border_color(cx.theme().border)
                .child(navigation)
                .child(div().flex_1())
                .child(bar(0.13, 28.))
                .child(bar(0.13, 28.)),
        )
        .child(
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .w(px(280.))
                        .max_w(relative(0.4))
                        .flex_shrink_0()
                        .border_r_1()
                        .border_color(cx.theme().border)
                        .child(
                            div()
                                .h(px(46.))
                                .px_3()
                                .flex()
                                .items_center()
                                .child(bar(0.9, 28.)),
                        )
                        .child(listing("changed files", 0, 10)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .h(px(40.))
                                .px_3()
                                .flex()
                                .items_center()
                                .child(bar(0.5, 13.)),
                        )
                        .child(diff()),
                ),
        )
        .into_any_element()
}
pub(super) fn resource(key: &str, tab: usize) -> AnyElement {
    match key {
        "project-folder" => listing("folders", 0, 8),
        "git-history-tag" => commit_header(),
        "git-list" if tab != 0 => listing("repository entries", tab, 8),
        "git-branches" => listing("branches", 3, 6),
        "git-checkouts" => listing("worktrees", 5, 4),
        "git-history-detail" => commit_header(),
        "git-remote-detail" => region("remote details")
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .child(bar(0.2, 10.))
            .child(bar(0.85, 13.))
            .child(bar(0.2, 10.))
            .child(bar(0.85, 13.))
            .into_any_element(),
        "git-repository-settings" => region("repository settings")
            .px_4()
            .py_2()
            .flex()
            .flex_col()
            .gap_2()
            .child(bar(0.4, 18.))
            .child(bar(0.25, 18.))
            .child(div().flex().gap_2().children((0..2).map(|index| {
                div()
                    .debug_selector(move || format!("settings-loading-identity-{index}"))
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(bar(0.3, 18.))
                    .child(bar(1., crate::controls::CONTROL_HEIGHT))
            })))
            .child(
                div()
                    .debug_selector(|| "settings-loading-prune".into())
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(20.))
                    .child(Skeleton::new().size(px(16.)).rounded(px(3.)))
                    .child(bar(0.65, 12.)),
            )
            .child(bar(0.9, 18.))
            .child(
                div()
                    .debug_selector(|| "settings-loading-actions".into())
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Skeleton::new()
                            .w(px(64.))
                            .h(px(crate::controls::CONTROL_HEIGHT)),
                    )
                    .child(
                        Skeleton::new()
                            .w(px(96.))
                            .h(px(crate::controls::CONTROL_HEIGHT)),
                    ),
            )
            .into_any_element(),
        _ => div().into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Context, IntoElement, Render, TestAppContext, Window, div, prelude::*, px, size};
    use gpui_kit as gpui;
    struct SettingsLoading;
    impl Render for SettingsLoading {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            super::resource("git-repository-settings", 6)
        }
    }
    #[gpui::test]
    fn repository_settings_skeleton_places_identity_fields_beside_each_other(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (_, visual) = cx.add_window_view(|_, _| SettingsLoading);
        for width in [520., 280.] {
            visual.simulate_resize(size(px(width), px(400.)));
            visual.update(|window, cx| window.draw(cx).clear(cx));
            let name = visual.debug_bounds("settings-loading-identity-0").unwrap();
            let email = visual.debug_bounds("settings-loading-identity-1").unwrap();
            let prune = visual.debug_bounds("settings-loading-prune").unwrap();
            let actions = visual.debug_bounds("settings-loading-actions").unwrap();
            assert_eq!(name.top(), email.top());
            assert_eq!(name.size, email.size);
            assert!(name.right() < email.left());
            assert!(email.right() <= px(width));
            assert!(prune.top() > email.bottom());
            assert!(actions.top() > prune.bottom());
            assert!(actions.bottom() <= px(400.));
        }
    }
    #[test]
    fn picker_placeholders_only_replace_an_empty_pending_page() {
        assert_eq!(super::picker_count(0, true), 4);
        assert_eq!(super::picker_count(0, false), 0);
        assert_eq!(super::picker_count(50, true), 50);
        assert_eq!(super::picker_count(50, false), 50);
    }
    struct PickerRows {
        pending: bool,
    }
    impl Render for PickerRows {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w_full()
                .flex()
                .flex_col()
                .child(div().h(px(52.)).flex_shrink_0().child("Filter"))
                .children((0..4).map(|index| {
                    div()
                        .debug_selector(move || format!("picker-row-{index}"))
                        .when(self.pending, |row| {
                            row.child(super::picker_row("branches", index))
                        })
                        .when(!self.pending, |row| {
                            row.child(div().h(px(48.)).child("Branch and tracking status"))
                        })
                }))
                .child(div().debug_selector(|| "picker-footer".into()).h(px(36.)))
        }
    }
    #[gpui::test]
    fn picker_loading_rows_match_loaded_height_and_keep_footer_position(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (view, visual) = cx.add_window_view(|_, _| PickerRows { pending: true });
        visual.simulate_resize(size(px(365.), px(400.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let first = visual.debug_bounds("picker-row-0").unwrap();
        let last = visual.debug_bounds("picker-row-3").unwrap();
        let footer = visual.debug_bounds("picker-footer").unwrap();
        assert_eq!(first.top(), px(52.));
        assert_eq!(first.size.height, px(48.));
        assert_eq!(last.bottom(), footer.top());
        view.update(visual, |view, cx| {
            view.pending = false;
            cx.notify();
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(visual.debug_bounds("picker-row-0").unwrap(), first);
        assert_eq!(visual.debug_bounds("picker-row-3").unwrap(), last);
        assert_eq!(visual.debug_bounds("picker-footer").unwrap(), footer);
    }
    struct PickerDetail {
        pending: bool,
    }
    impl Render for PickerDetail {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(div().h(px(28.)).child("Choose branch"))
                .child(super::picker_detail(
                    "branch",
                    Some("Commit 12345678".into()),
                    self.pending,
                ))
                .child(div().debug_selector(|| "next-field".into()).h(px(28.)))
        }
    }
    #[gpui::test]
    fn picker_metadata_replaces_skeleton_without_moving_next_field(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (view, visual) = cx.add_window_view(|_, _| PickerDetail { pending: true });
        visual.simulate_resize(size(px(280.), px(200.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let loading = visual.debug_bounds("next-field").unwrap();
        assert_eq!(loading.top(), px(60.));
        view.update(visual, |view, cx| {
            view.pending = false;
            cx.notify();
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(visual.debug_bounds("next-field").unwrap(), loading);
    }
    struct Loading;
    impl Render for Loading {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(div().h(px(46.)).flex_shrink_0().child("Changes / History"))
                .child(super::resource("git-tag-status", 1))
                .child(div().h(px(36.)).flex_shrink_0().child("Filter"))
                .child(
                    div()
                        .debug_selector(|| "rows".into())
                        .child(super::listing("history", 1, 4)),
                )
                .child(super::resource("git-more", 1))
                .child(div().debug_selector(|| "after-rows".into()).h(px(20.)))
        }
    }
    #[gpui::test]
    fn skeleton_rows_replace_content_below_controls_without_background_read_blocks(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (_, visual) = cx.add_window_view(|_, _| Loading);
        visual.simulate_resize(size(px(280.), px(480.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let rows = visual.debug_bounds("rows").unwrap();
        let after = visual.debug_bounds("after-rows").unwrap();
        assert_eq!(rows.top(), px(82.));
        assert_eq!(rows.size.height, px(4. * 46.));
        assert_eq!(rows.size.width, px(280.));
        assert_eq!(after.top(), rows.bottom());
    }
}
