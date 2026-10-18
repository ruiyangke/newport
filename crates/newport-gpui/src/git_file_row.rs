//! One status vocabulary and filename-first path presentation for Git lists.
use super::*;

/// Selection must remain distinct from hover throughout the Git workspace.
/// Set the button variant itself: a background on the outer row cannot
/// override the component's hover/pressed surfaces.
/// Pair with `selected(selected)`: Kit fades a custom variant's normal color,
/// whereas selected styling uses the exact active color.
pub(super) fn selection_style(
    selected: bool,
    cx: &App,
) -> gpui_kit::component::button::ButtonCustomVariant {
    let tint = theme::git_selection(cx.theme().is_dark());
    gpui_kit::component::button::ButtonCustomVariant::new(cx)
        .color(if selected {
            tint
        } else {
            cx.theme().transparent
        })
        .hover(if selected { tint } else { theme::row_hover(cx) })
        .active(tint)
}

pub(super) fn mark(status: &str) -> &'static str {
    match status {
        "Added" => "A",
        "Deleted" => "D",
        "Renamed" => "R",
        "Copied" => "C",
        "Typechange" => "T",
        "Untracked" => "?",
        "Conflicted" => "!",
        _ => "M",
    }
}
pub(super) fn name(mark: &str) -> &'static str {
    match mark {
        "A" => "Added",
        "D" => "Deleted",
        "R" => "Renamed",
        "C" => "Copied",
        "T" => "Type changed",
        "?" => "Untracked",
        "!" => "Conflicted",
        _ => "Modified",
    }
}
pub(super) fn badge(mark: &'static str, cx: &App) -> Div {
    let dark = cx.theme().is_dark();
    let color = match mark {
        "A" | "?" => theme::success(dark),
        "M" => theme::warning(dark),
        "D" | "!" => theme::error(dark),
        _ => cx.theme().primary,
    };
    div()
        .size(px(18.))
        .line_height(px(crate::project_ui::FONT_SIZE))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .font_family(cx.theme().mono_font_family.clone())
        .font_weight(FontWeight::BOLD)
        .text_size(px(crate::project_ui::FONT_SIZE))
        .bg(if mark == "!" {
            color
        } else {
            color.opacity(if mark == "M" { 0.16 } else { 0.14 })
        })
        .text_color(if mark == "!" {
            rgb(0xffffff).into()
        } else {
            color
        })
        .child(mark)
}
fn split_path(path: &str) -> (&str, &str) {
    path.rfind('/')
        .map(|at| path.split_at(at + 1))
        .unwrap_or(("", path))
}
pub(super) fn path(path: String, cx: &App) -> Div {
    let (dir, base) = split_path(&path);
    div()
        .text_size(px(crate::project_ui::FONT_SIZE))
        .flex()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .when(!dir.is_empty(), |view| {
            view.child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(cx.theme().muted_foreground)
                    .child(dir.to_owned()),
            )
        })
        .child(
            div()
                .min_w_0()
                .max_w_full()
                .truncate()
                .flex_shrink(0.0001)
                .child(base.to_owned()),
        )
}
/// Changes navigator: identify the filename first; let the parent path yield
/// space before truncating the name. Preserve a trailing slash for directory
/// entries returned by older agents.
pub(super) fn filename_first(path: &str, cx: &App) -> Div {
    let directory = path.ends_with('/');
    let (parent, name) = split_path(path.trim_end_matches('/'));
    let name = format!("{name}{}", if directory { "/" } else { "" });
    div()
        .flex()
        .items_center()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .gap(px(6.))
        .whitespace_nowrap()
        .child(
            div()
                .min_w_0()
                .max_w_full()
                .truncate()
                .flex_shrink(0.0001)
                .child(name),
        )
        .when(!parent.is_empty(), |row| {
            row.child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(crate::project_ui::FONT_SIZE))
                    .text_color(cx.theme().muted_foreground)
                    .child(parent.trim_end_matches('/').to_owned()),
            )
        })
}

pub(super) fn description(entry: &Value, title: &str, status: &str) -> String {
    if let Some(origin) = origin(entry) {
        format!("{title} · {status}\nfrom {origin}")
    } else {
        format!("{title} · {status}")
    }
}
/// Compare protocol path identities, not lossy display strings. Two distinct
/// non-UTF-8 paths can have the same display label.
pub(super) fn origin(entry: &Value) -> Option<&str> {
    let old = &entry["oldPath"];
    let new = entry
        .get("newPath")
        .or_else(|| entry.get("path"))
        .unwrap_or(&Value::Null);
    let old_id = old["bytesB64"].as_str()?;
    let new_id = new["bytesB64"].as_str()?;
    (!old_id.is_empty() && !new_id.is_empty() && old_id != new_id)
        .then(|| old["display"].as_str())
        .flatten()
}
#[cfg(test)]
mod tests {
    use super::{description, mark, name, origin, split_path};
    use gpui_kit::{
        self as gpui, Bounds, Context, IntoElement, Pixels, Render, Window, div, prelude::*, px,
    };
    use serde_json::{Value, json};
    use std::{cell::RefCell, rc::Rc};
    struct PathView {
        bounds: Rc<RefCell<Vec<Bounds<Pixels>>>>,
        width: f32,
    }
    impl Render for PathView {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let bounds = self.bounds.clone();
            div().flex().w(px(self.width)).child(
                super::path(
                    "a/very/long/directory/that/should/shrink/first/filename.rs".into(),
                    cx,
                )
                .on_children_prepainted(move |next, _, _| *bounds.borrow_mut() = next),
            )
        }
    }
    #[gpui::test]
    fn filename_keeps_its_width_before_directory_and_both_fit_narrow_rows(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let bounds = Rc::new(RefCell::new(Vec::new()));
        let (view, cx) = cx.add_window_view(|_, _| PathView {
            bounds: bounds.clone(),
            width: 220.,
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let wide = bounds.borrow().clone();
        assert_eq!(wide.len(), 2);
        assert!(wide[1].size.width > px(60.));
        view.update(cx, |v, cx| {
            v.width = 140.;
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let narrow = bounds.borrow().clone();
        assert!((wide[1].size.width - narrow[1].size.width).abs() < px(1.));
        assert!(narrow[0].size.width < wide[0].size.width);
        assert!(narrow[1].right() <= px(141.));
        view.update(cx, |v, cx| {
            v.width = 50.;
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(bounds.borrow()[1].right() <= px(51.));
    }
    #[test]
    fn rename_origin_uses_path_identity_and_ignores_deleted_or_incomplete_paths() {
        let mut entry = json!({
            "oldPath":{"bytesB64":"b2xk","display":"same display"},
            "path":{"bytesB64":"bmV3","display":"same display"}
        });
        assert_eq!(origin(&entry), Some("same display"));
        entry["path"]["bytesB64"] = json!("b2xk");
        assert_eq!(origin(&entry), None);
        entry["path"] = Value::Null;
        assert_eq!(origin(&entry), None);
        entry["newPath"] = json!({"display":"missing identity"});
        assert_eq!(origin(&entry), None);
    }
    #[test]
    fn paths_preserve_bytes_and_statuses_are_explicit() {
        for path in ["src/deep/file.rs", "file.rs", "dir/", "雪/été.rs", "a//b"] {
            let (dir, base) = split_path(path);
            assert_eq!(format!("{dir}{base}"), path);
        }
        for (status, letter, title) in [
            ("Added", "A", "Added"),
            ("Typechange", "T", "Type changed"),
            ("Renamed", "R", "Renamed"),
            ("Untracked", "?", "Untracked"),
            ("Conflicted", "!", "Conflicted"),
        ] {
            assert_eq!(mark(status), letter);
            assert_eq!(name(letter), title);
        }
        assert_eq!(mark("Modified"), "M");
        assert!(
            description(
                &json!({"oldPath":{"bytesB64":"a","display":"old"},"newPath":{"bytesB64":"b"}}),
                "new",
                "Renamed"
            )
            .ends_with("from old")
        );
    }
}
