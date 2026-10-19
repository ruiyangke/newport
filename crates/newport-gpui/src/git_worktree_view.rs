//! Shared worktree picker labels and action-review visibility.
use super::*;

pub(super) fn row_labels(entry: &Value) -> (String, String) {
    let title = entry["name"]["display"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(if entry["kind"] == "bare" {
            "Bare repository"
        } else {
            "Main worktree"
        })
        .to_owned();
    let branch = entry["head"]["name"]["display"]
        .as_str()
        .filter(|_| entry["head"]["detached"] != true)
        .filter(|s| !s.is_empty())
        .map(|name| name.strip_prefix("refs/heads/").unwrap_or(name))
        .unwrap_or(if entry["head"]["detached"] == true {
            "Detached HEAD"
        } else {
            "Branch unavailable"
        });
    let mut labels = vec![branch.to_owned()];
    if entry["current"] == true {
        labels.push("Current".into());
    }
    if entry["locked"] == true {
        labels.push("Locked".into());
    }
    match entry["state"].as_str() {
        Some("missing") => labels.push("Missing".into()),
        Some("available") => {}
        _ => labels.push("Details unavailable".into()),
    }
    (title, labels.join(" · "))
}

/// Keep long remote checkout paths within the picker instead of expanding a
/// plain-text tooltip across the window.
pub(super) fn hover(entry: &Value, window: &mut Window, cx: &mut App) -> AnyView {
    let path = text(&entry["path"], "display");
    let width = crate::hover::width(window);
    let height = crate::hover::height(window);
    crate::hover::surface(gpui_kit::component::tooltip::Tooltip::element(
        move |_, cx| {
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
        },
    ))
    .w(px(width))
    .p(px(12.))
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
    use serde_json::json;

    #[gpui_kit::test]
    fn worktree_hover_wraps_long_paths_within_the_window(cx: &mut gpui_kit::TestAppContext) {
        use super::*;
        struct Host(Value);
        impl Render for Host {
            fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div().child(hover(&self.0, window, cx))
            }
        }
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
        });
        let entry = json!({
            "name":{"display":"custom-app-demo"},
            "head":{"detached":true},
            "state":"available",
            "path":{"display":format!("/tmp/claude-1000/-home-user-Projects-marketplace-service/{}/scratchpad/custom-app-demo", "a".repeat(100))}
        });
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
        assert_eq!(
            row_labels(
                &json!({"kind":"main","state":"available","current":true,"head":{"name":{"display":"refs/heads/main"}}})
            ),
            ("Main worktree".into(), "main · Current".into())
        );
        assert_eq!(
            row_labels(
                &json!({"kind":"linked","name":{"display":"feature"},"state":"missing","locked":true,"head":{"detached":true,"name":{"display":"HEAD"}}})
            ),
            ("feature".into(), "Detached HEAD · Locked · Missing".into())
        );
        assert_eq!(row_labels(&json!({"kind":"bare"})).0, "Bare repository");
        assert!(row_labels(&json!({})).1.contains("Branch unavailable"));
    }
}
