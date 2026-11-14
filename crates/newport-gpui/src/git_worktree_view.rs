//! Shared worktree picker labels and action-review visibility.
use super::*;
use newport_protocol::git::Worktree;

pub(super) fn row_labels(entry: &Worktree) -> (String, String) {
    let title = entry
        .name
        .as_ref()
        .map(|name| name.display.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(if entry.kind == "bare" {
            "Bare repository"
        } else {
            "Main worktree"
        })
        .to_owned();
    let branch = entry
        .head
        .name
        .as_ref()
        .map(|name| name.display.as_str())
        .filter(|_| !entry.head.detached)
        .filter(|s| !s.is_empty())
        .map(|name| name.strip_prefix("refs/heads/").unwrap_or(name))
        .unwrap_or(if entry.head.detached {
            "Detached HEAD"
        } else {
            "Branch unavailable"
        });
    let mut labels = vec![branch.to_owned()];
    if entry.current {
        labels.push("Current".into());
    }
    if entry.locked {
        labels.push("Locked".into());
    }
    match entry.state.as_str() {
        "missing" => labels.push("Missing".into()),
        "available" => {}
        _ => labels.push("Details unavailable".into()),
    }
    (title, labels.join(" · "))
}

/// Keep long remote checkout paths within the picker instead of expanding a
/// plain-text tooltip across the window.
pub(super) fn hover(entry: &Worktree, window: &mut Window, cx: &mut App) -> AnyView {
    let path = entry.path.display.clone();
    let width = crate::hover::width(window);
    let height = crate::hover::height(window);
    crate::hover::card((12., 12.), move |_, cx| {
        div()
            .id("worktree-hover-card")
            .debug_selector(|| "worktree-hover-card".into())
            .w(px(width - 24.))
            .max_h(px(height))
            .overflow_y_scroll()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(4.))
            .text_color(cx.theme().foreground)
            .line_height(relative(1.45))
            .when(!path.is_empty(), |card| {
                card.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .text_size(px(crate::ui::FONT_SIZE))
                        .child(crate::label("Path").text_color(cx.theme().muted_foreground))
                        .child(
                            div()
                                .w_full()
                                .min_w_0()
                                .font_family(cx.theme().mono_font_family.clone())
                                .whitespace_normal()
                                .child(path.clone()),
                        ),
                )
            })
    })
    .w(px(width))
    .build(window, cx)
}
impl Newport {
    pub(super) fn git_worktree_review_open(&self) -> bool {
        self.git.worktree_create.is_some()
            || self.git.worktree_lock.is_some()
            || self.git.worktree_repair.is_some()
            || self.git.worktree_remove.is_some()
    }
}
#[cfg(test)]
mod tests {
    use super::row_labels;
    use crate::git_fixture::worktree;
    use newport_protocol::git::Path;

    #[gpui_kit::test]
    fn worktree_hover_wraps_long_paths_within_the_window(cx: &mut gpui_kit::TestAppContext) {
        use super::*;
        struct Host(Worktree);
        impl Render for Host {
            fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div().child(hover(&self.0, window, cx))
            }
        }
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
        });
        let mut entry = worktree(
            Some("custom-app-demo"),
            &format!(
                "/tmp/build-fixture/home-developer-projects-example-api-service/{}/scratchpad/custom-app-demo",
                "a".repeat(100)
            ),
        );
        entry.head.detached = true;
        let (_, visual) = cx.add_window_view(|_, _| Host(entry));
        for dark in [false, true] {
            visual.update(|_, cx| crate::theme::install(dark, cx));
            for width in [800., 320.] {
                visual.simulate_resize(size(px(width), px(600.)));
                crate::page_perf_tests::frame(visual);
                let bounds = visual.debug_bounds("worktree-hover-card").unwrap();
                assert!(bounds.size.width <= px(356.) && bounds.right() <= px(width));
                assert!(
                    bounds.size.height > px(60.),
                    "the full path should wrap: {bounds:?}"
                );
                assert!(
                    bounds.size.height < px(300.),
                    "the hover should remain compact: {bounds:?}"
                );
            }
        }
    }
    #[test]
    fn rows_distinguish_main_detached_locked_and_missing_checkouts() {
        let mut main = worktree(None, "/repo");
        main.kind = "main".into();
        main.current = true;
        main.head.name = Some(Path::new(b"refs/heads/main"));
        assert_eq!(
            row_labels(&main),
            ("Main worktree".into(), "main · Current".into())
        );
        let mut feature = worktree(Some("feature"), "/repo-feature");
        feature.state = "missing".into();
        feature.locked = true;
        feature.head.detached = true;
        feature.head.name = Some(Path::new(b"HEAD"));
        assert_eq!(
            row_labels(&feature),
            ("feature".into(), "Detached HEAD · Locked · Missing".into())
        );
        let mut bare = worktree(None, "/repo.git");
        bare.kind = "bare".into();
        assert_eq!(row_labels(&bare).0, "Bare repository");
        let mut unknown = worktree(Some(""), "/repo");
        unknown.state = "unknown".into();
        let (title, details) = row_labels(&unknown);
        assert_eq!(title, "Main worktree");
        assert_eq!(details, "Branch unavailable · Details unavailable");
    }
}
