//! Text selection is independent from the IDs selected for staging.
use super::*;
use crate::selectable_log::SelectableLog;
use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};

pub(super) type Document = Rc<RefCell<DiffText>>;
pub(super) struct DiffText {
    pub selection: SelectableLog,
    side: usize,
    search: Rc<RefCell<git_diff_search::Model>>,
    syntax: git_diff_syntax::SharedStyles,
}

/// Reserve a 12px rail below the list so the horizontal thumb never covers code.
/// Keep one scroll container; split panes share offsets but have separate bounds.
pub(super) fn scroll_viewport(
    id: &'static str,
    content: impl IntoElement,
    scroll: &UniformListScrollHandle,
) -> AnyElement {
    use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
    div()
        .relative()
        .flex()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .pb(px(12.))
        .child(content)
        .child(
            Scrollbar::horizontal(scroll)
                .id(id)
                .mode(ScrollbarMode::Always)
                .styles(|s| s.track(|t| t.width(px(12.))))
                .viewport_from_layout(),
        )
        .into_any_element()
}
#[derive(Default)]
pub(super) struct Cache {
    state: RefCell<Option<State>>,
    pub search: Rc<RefCell<git_diff_search::Model>>,
    syntax: RefCell<git_diff_syntax::Cache>,
}
struct State {
    key: Value,
    presentation: Weak<git_diff::Presentation>,
    documents: Vec<Document>,
}
impl Cache {
    pub(super) fn clear(&self, cx: &mut App) {
        if let Some(state) = self.state.borrow_mut().take() {
            for document in state.documents {
                document.borrow_mut().selection.clear(cx);
            }
        }
        self.search.borrow_mut().clear_rows();
        self.syntax.borrow_mut().clear();
    }
    pub(super) fn prepare(
        &self,
        key: Value,
        presentation: &Rc<git_diff::Presentation>,
        rows: impl FnOnce() -> Vec<Vec<Option<String>>>,
        focus: &FocusHandle,
        window: &Window,
        cx: &mut App,
    ) -> Vec<Document> {
        let mut cached = self.state.borrow_mut();
        if let Some(state) = cached.as_ref()
            && state.key == key
            && state
                .presentation
                .upgrade()
                .is_some_and(|old| Rc::ptr_eq(&old, presentation))
        {
            self.syntax.borrow_mut().refresh(window, cx);
            return state.documents.clone();
        }
        let rows = rows();
        self.syntax.borrow_mut().set_source(
            key[3].as_str().unwrap_or_default(),
            git_diff_syntax::project(presentation, &rows),
            window,
            cx,
        );
        self.search.borrow_mut().set_rows(key.clone(), rows.clone());
        if cached
            .as_ref()
            .is_some_and(|state| state.key != key || state.documents.len() != rows.len())
        {
            for document in &cached.as_ref().unwrap().documents {
                document.borrow_mut().selection.clear(cx);
            }
            *cached = None;
        }
        let state = cached.get_or_insert_with(|| State {
            key,
            presentation: Weak::new(),
            documents: rows
                .iter()
                .enumerate()
                .map(|(side, _)| {
                    Rc::new(RefCell::new(DiffText {
                        selection: SelectableLog::new(focus.clone(), window, cx),
                        side,
                        search: self.search.clone(),
                        syntax: self.syntax.borrow().styles.clone(),
                    }))
                })
                .collect(),
        });
        for (document, rows) in state.documents.iter().zip(rows) {
            document.borrow_mut().selection.set_rows(rows, cx);
        }
        state.presentation = Rc::downgrade(presentation);
        state.documents.clone()
    }
}

pub(super) fn row(row: Option<&git_diff::Row>) -> Option<String> {
    row.filter(|r| !r.heading).map(git_diff::Row::text)
}
/// Expose the visible code independently from staging controls. The custom
/// selectable-text painter does not emit accessibility text of its own.
pub(super) fn code_container(
    row: &git_diff::Row,
    index: usize,
    side: Option<bool>,
) -> impl IntoElement + ParentElement + Styled {
    let id = match side {
        None => "diff-code-unified",
        Some(false) => "diff-code-old",
        Some(true) => "diff-code-new",
    };
    div()
        .id((id, index))
        .role(accesskit::Role::Label)
        .aria_label(code_label(row, side))
        .test_support()
}
fn code_label(row: &git_diff::Row, side: Option<bool>) -> String {
    let content = row.text();
    if row.heading || (row.old.is_empty() && row.new.is_empty()) {
        return content;
    }
    let location = match side {
        Some(false) => format!("Old line {}", row.old),
        Some(true) => format!("New line {}", row.new),
        None if row.origin == "+" => format!("Line {}", row.new),
        None if row.origin == "-" => format!("Line {}", row.old),
        None => format!("Old line {}, new line {}", row.old, row.new),
    };
    let change = match row.origin.as_str() {
        "+" => ", added",
        "-" => ", deleted",
        _ => "",
    };
    format!("{location}{change}: {content}")
}
pub(super) fn key(page: &Value, split: bool, path: &str) -> Value {
    // Cursor is intentionally excluded: continuation is an append, not a new
    // comparison. Context, side, path, source snapshot and layout are identity.
    json!([page["snapshot"], page["metadata"], split, path])
}
pub(super) fn text(document: &Document, index: usize, cx: &App) -> AnyElement {
    let doc = document.borrow();
    doc.selection
        .full_row(index)
        .with_tabs(4)
        .with_highlights(git_diff_syntax::overlay(
            doc.syntax.borrow().row(doc.side, index),
            &doc.search.borrow().highlights(doc.side, index, cx),
        ))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{Cache, Document};
    use crate::{
        git_diff::{Presentation, Row},
        git_diff_split,
    };
    use gpui::{
        AppContext, Context, FocusHandle, InteractiveElement, IntoElement, MouseButton,
        ParentElement, Render, Styled, TestAppContext, UniformListScrollHandle, VisualTestContext,
        Window, div, point, px, size,
    };
    use gpui_kit as gpui;
    use serde_json::{Value, json};
    use std::{cell::OnceCell, rc::Rc};
    struct View {
        presentation: Rc<Presentation>,
        cache: Cache,
        documents: Vec<Document>,
        focus: FocusHandle,
        scroll: UniformListScrollHandle,
        split: bool,
        snapshot: String,
    }
    fn presentation(extra: bool) -> Rc<Presentation> {
        let mut lines = vec![
            Row {
                heading: true,
                bytes: b"@@ header @@".to_vec(),
                ..Default::default()
            },
            Row {
                origin: "-".into(),
                old: "1".into(),
                bytes: "old Café 🦀\n".as_bytes().to_vec(),
                ..Default::default()
            },
            Row {
                origin: "+".into(),
                new: "1".into(),
                bytes: "new 雪 🦀\n".as_bytes().to_vec(),
                ..Default::default()
            },
            Row {
                origin: " ".into(),
                old: "2".into(),
                new: "2".into(),
                bytes: b"context\n".to_vec(),
                ..Default::default()
            },
        ];
        if extra {
            lines.push(Row {
                origin: " ".into(),
                old: "3".into(),
                new: "3".into(),
                bytes: b"appended\n".to_vec(),
                ..Default::default()
            });
        }
        Rc::new(Presentation {
            lines,
            selection: Rc::default(),
            range_selection: Rc::default(),
            review: Err("read only".into()),
            error: None,
            split: OnceCell::new(),
            runs: OnceCell::new(),
            width: OnceCell::new(),
        })
    }
    impl Render for View {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let p = &self.presentation;
            self.documents = self.cache.prepare(
                json!([self.snapshot, null, self.split, "preview.rs"]),
                p,
                || {
                    if self.split {
                        [1, 2]
                            .map(|index| {
                                let mut rows = vec![None, super::row(Some(&p.lines[index]))];
                                rows.extend(p.lines[3..].iter().map(|r| super::row(Some(r))));
                                rows
                            })
                            .into()
                    } else {
                        vec![p.lines.iter().map(|r| super::row(Some(r))).collect()]
                    }
                },
                &self.focus,
                window,
                cx,
            );
            let content = if self.split {
                git_diff_split::render(
                    p.clone(),
                    &self.scroll,
                    None,
                    Some(self.documents.clone()),
                    cx,
                )
            } else {
                crate::git_diff::render_unified(
                    p.clone(),
                    &self.scroll,
                    None,
                    self.documents[0].clone(),
                    cx,
                )
            };
            div()
                .size_full()
                .flex()
                .track_focus(&self.focus)
                .child(content)
        }
    }
    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|w, cx| w.draw(cx).clear(cx));
    }
    fn selected(cx: &mut VisualTestContext) -> String {
        cx.update(gpui::base::TextSelection::selected_text)
    }
    fn copy(cx: &mut VisualTestContext) -> String {
        crate::test_shortcuts(cx, "cmd-c");
        cx.update(|_, cx| cx.read_from_clipboard().unwrap().text().unwrap())
    }
    fn drag(cx: &mut VisualTestContext, doc: &Document, from: (usize, usize), to: (usize, usize)) {
        let at =
            |(r, b)| doc.borrow().selection.text_position(r, b).unwrap() + point(px(0.1), px(5.));
        let (from, to) = (at(from), at(to));
        cx.simulate_mouse_down(from, MouseButton::Left, Default::default());
        cx.simulate_mouse_move(to, Some(MouseButton::Left), Default::default());
        cx.simulate_mouse_up(to, MouseButton::Left, Default::default());
        draw(cx);
    }
    #[gpui::test]
    fn visible_diff_code_is_accessible_in_both_layouts_without_expanding_the_list(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::base::test_support::snapshots;
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let mut view = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let mut data = presentation(false);
            Rc::get_mut(&mut data)
                .unwrap()
                .lines
                .extend((3..1003).map(|i| Row {
                    origin: " ".into(),
                    old: i.to_string(),
                    new: i.to_string(),
                    bytes: format!("context row {i}\n").into_bytes(),
                    ..Default::default()
                }));
            let entity = cx.new(|cx| View {
                presentation: data,
                cache: Cache::default(),
                documents: vec![],
                focus: cx.focus_handle(),
                scroll: UniformListScrollHandle::new(),
                split: false,
                snapshot: "accessible".into(),
            });
            view = Some(entity.clone());
            gpui::base::Root::new(entity, window, cx)
        });
        let view = view.unwrap();
        cx.simulate_resize(size(px(900.), px(400.)));
        for split in [false, true] {
            view.update(cx, |v, cx| {
                v.split = split;
                v.scroll
                    .0
                    .borrow()
                    .base_handle
                    .set_offset(point(px(0.), px(0.)));
                cx.notify();
            });
            draw(cx);
            let labels = cx.update(|window, _| {
                snapshots(window)
                    .into_iter()
                    .filter(|e| e.role() == Some(gpui::Role::Label))
                    .filter_map(|e| e.label().map(str::to_owned))
                    .collect::<Vec<_>>()
            });
            for expected in if split {
                [
                    "Old line 1, deleted: old Café 🦀",
                    "New line 1, added: new 雪 🦀",
                    "Old line 2: context",
                ]
            } else {
                [
                    "Line 1, deleted: old Café 🦀",
                    "Line 1, added: new 雪 🦀",
                    "Old line 2, new line 2: context",
                ]
            } {
                assert!(
                    labels.iter().any(|label| label == expected),
                    "missing {expected}"
                );
            }
            assert!(labels.iter().any(|label| label == "@@ header @@"));
            assert!(
                labels.len() < 60,
                "only virtualized visible rows are exposed"
            );
            assert!(!labels.iter().any(|label| label.contains("context row 900")));
            view.update(cx, |v, cx| {
                v.scroll
                    .0
                    .borrow()
                    .base_handle
                    .set_offset(point(px(0.), px(-19800.)));
                cx.notify();
            });
            draw(cx);
            cx.update(|window, _| {
                let elements = snapshots(window);
                let labels = elements
                    .iter()
                    .filter_map(|e| e.label())
                    .collect::<Vec<_>>();
                assert!(labels.iter().any(|label| label.contains("context row 905")));
                assert!(!labels.iter().any(|label| label.contains("old Café")));
                assert!(labels.len() < 60);
            });
        }
    }
    #[gpui::test]
    #[ignore = "manual Git diff CPU frame benchmark; excludes GPU presentation and repository I/O"]
    fn benchmark_git_diff_frames(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        for count in [50, 5000] {
            let mut view = None;
            let (_, visual) = cx.add_window_view(|window, cx| {
                let mut data = presentation(false);
                Rc::get_mut(&mut data)
                    .unwrap()
                    .lines
                    .extend((0..count).map(|i| Row {
                        origin: " ".into(),
                        old: (i + 3).to_string(),
                        new: (i + 3).to_string(),
                        bytes: format!("context row {i}: literal source content\n").into_bytes(),
                        ..Default::default()
                    }));
                let entity = cx.new(|cx| View {
                    presentation: data,
                    cache: Cache::default(),
                    documents: vec![],
                    focus: cx.focus_handle(),
                    scroll: UniformListScrollHandle::new(),
                    split: false,
                    snapshot: format!("benchmark-{count}"),
                });
                view = Some(entity.clone());
                gpui::base::Root::new(entity, window, cx)
            });
            let view = view.unwrap();
            visual.simulate_resize(size(px(1100.), px(650.)));
            for split in [false, true] {
                view.update(visual, |v, cx| {
                    v.split = split;
                    cx.notify();
                });
                draw(visual);
                let start = std::time::Instant::now();
                for _ in 0..30 {
                    view.update(visual, |_, cx| cx.notify());
                    draw(visual);
                }
                eprintln!(
                    "Git diff {count} lines split={split}: {:.3} ms/CPU frame",
                    start.elapsed().as_secs_f64() * 1000. / 30.
                );
            }
        }
    }

    #[gpui::test]
    fn tabbed_diffs_keep_visual_columns_and_literal_copy_in_both_layouts(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let mut view = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let mut p = presentation(false);
            Rc::get_mut(&mut p).unwrap().lines[1].bytes = "a\t雪\tCafé\n".as_bytes().to_vec();
            let entity = cx.new(|cx| View {
                presentation: p,
                cache: Cache::default(),
                documents: vec![],
                focus: cx.focus_handle(),
                scroll: UniformListScrollHandle::new(),
                split: false,
                snapshot: "tabs".into(),
            });
            view = Some(entity.clone());
            gpui::base::Root::new(entity, window, cx)
        });
        let view = view.unwrap();
        cx.simulate_resize(size(px(900.), px(400.)));
        for split in [false, true] {
            view.update(cx, |v, cx| {
                v.split = split;
                cx.notify();
            });
            draw(cx);
            let doc = view.read_with(cx, |v, _| v.documents[0].clone());
            let position = |byte| doc.borrow().selection.text_position(1, byte).unwrap().x;
            let cell = position(1) - position(0);
            assert!((position(2) - position(0) - cell * 4.).abs() <= px(1.));
            drag(cx, &doc, (1, 0), (1, "a\t雪\tCafé".len()));
            assert_eq!(selected(cx), "a\t雪\tCafé");
            assert_eq!(copy(cx), "a\t雪\tCafé");
            drag(cx, &doc, (1, 2), (1, 1));
            assert_eq!(copy(cx), "\t", "a rendered tab copies as one source tab");
            drag(cx, &doc, (1, 6), (1, "a\t雪\tCafé".len()));
            assert_eq!(copy(cx), "Café", "source offsets following tabs stay exact");
        }
    }
    #[gpui::test]
    fn diff_copy_excludes_gutters_preserves_append_and_resets_comparisons(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let mut view = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let entity = cx.new(|cx| View {
                presentation: presentation(false),
                cache: Cache::default(),
                documents: vec![],
                focus: cx.focus_handle(),
                scroll: UniformListScrollHandle::new(),
                split: false,
                snapshot: "a".into(),
            });
            view = Some(entity.clone());
            gpui::base::Root::new(entity, window, cx)
        });
        let view = view.unwrap();
        cx.simulate_resize(size(px(900.), px(400.)));
        draw(cx);
        let doc = view.read_with(cx, |v, _| v.documents[0].clone());
        drag(cx, &doc, (1, 4), (3, 7));
        assert_eq!(copy(cx), "Café 🦀\nnew 雪 🦀\ncontext");
        assert!(view.read_with(cx, |v, _| v.presentation.selection.borrow().is_empty()));
        view.update(cx, |v, cx| {
            let mut search = v.cache.search.borrow_mut();
            search.open = true;
            search.set_query("雪".into());
            cx.notify();
        });
        draw(cx);
        view.read_with(cx, |v, cx| {
            let highlights = v.cache.search.borrow().highlights(0, 2, cx);
            assert_eq!(highlights[0].0, 4..7);
        });
        assert_eq!(copy(cx), "Café 🦀\nnew 雪 🦀\ncontext");
        view.update(cx, |v, cx| {
            v.presentation = presentation(true);
            cx.notify();
        });
        draw(cx);
        assert_eq!(selected(cx), "Café 🦀\nnew 雪 🦀\ncontext");
        assert!(view.read_with(cx, |v, _| Rc::ptr_eq(&doc, &v.documents[0])));
        view.update(cx, |v, cx| {
            v.snapshot = "b".into();
            cx.notify();
        });
        draw(cx);
        assert_eq!(selected(cx), "");
        view.update(cx, |v, cx| {
            v.split = true;
            cx.notify();
        });
        draw(cx);
        let docs = view.read_with(cx, |v, _| v.documents.clone());
        view.read_with(cx, |v, cx| {
            assert!(v.cache.search.borrow().highlights(0, 1, cx).is_empty());
            assert_eq!(v.cache.search.borrow().highlights(1, 1, cx)[0].0, 4..7);
        });
        drag(cx, &docs[0], (1, 0), (2, 7));
        assert_eq!(copy(cx), "old Café 🦀\ncontext");
        drag(cx, &docs[1], (1, 0), (2, 7));
        assert_eq!(copy(cx), "new 雪 🦀\ncontext");
    }
    #[gpui::test]
    fn horizontal_wheel_and_scrollbar_move_actual_unified_and_split_text(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let mut view = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let entity = cx.new(|cx| {
                let mut p = presentation(false);
                Rc::get_mut(&mut p).unwrap().lines[2].bytes = "long line ".repeat(300).into_bytes();
                View {
                    presentation: p,
                    cache: Cache::default(),
                    documents: vec![],
                    focus: cx.focus_handle(),
                    scroll: UniformListScrollHandle::new(),
                    split: false,
                    snapshot: "wide".into(),
                }
            });
            view = Some(entity.clone());
            gpui::base::Root::new(entity, window, cx)
        });
        let view = view.unwrap();
        cx.simulate_resize(size(px(900.), px(400.)));
        draw(cx);
        for split in [false, true] {
            view.update(cx, |v, cx| {
                v.split = split;
                v.scroll = UniformListScrollHandle::new();
                cx.notify();
            });
            draw(cx);
            let doc = view.read_with(cx, |v, _| v.documents[usize::from(split)].clone());
            let row = if split { 1 } else { 2 };
            let before = doc.borrow().selection.text_position(row, 0).unwrap();
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: point(px(700.), px(200.)),
                delta: gpui::ScrollDelta::Pixels(point(px(-300.), px(0.))),
                ..Default::default()
            });
            draw(cx);
            let after = doc.borrow().selection.text_position(row, 0).unwrap();
            assert_eq!(after.x, before.x - px(300.), "split={split}");
            assert_eq!(
                view.read_with(cx, |v, _| v.scroll.0.borrow().base_handle.offset().x),
                px(-300.)
            );
            // Exercise the actual Kit thumb, including the selectable viewport
            // above it. Both split-pane thumbs must update the shared offset.
            for left in if split { vec![450., 0.] } else { vec![0.] } {
                view.update(cx, |v, cx| {
                    v.scroll
                        .0
                        .borrow()
                        .base_handle
                        .set_offset(point(px(0.), px(0.)));
                    cx.notify();
                });
                draw(cx);
                let start = doc.borrow().selection.text_position(row, 0).unwrap();
                let from = point(px(left + 12.), px(395.));
                let to = from + point(px(100.), px(0.));
                cx.simulate_mouse_down(from, MouseButton::Left, Default::default());
                cx.simulate_mouse_move(to, Some(MouseButton::Left), Default::default());
                cx.simulate_mouse_up(to, MouseButton::Left, Default::default());
                draw(cx);
                let offset = view.read_with(cx, |v, _| v.scroll.0.borrow().base_handle.offset().x);
                assert!(
                    offset < px(-300.),
                    "split={split}, pane={left}, offset={offset:?}"
                );
                // Glyph positions are snapped to physical pixels.
                assert!(
                    (doc.borrow().selection.text_position(row, 0).unwrap().x - start.x - offset)
                        .abs()
                        <= px(0.5)
                );
                assert_eq!(selected(cx), "", "scrollbar dragging must not select code");
            }
        }
    }
    #[gpui::test]
    fn last_diff_line_stays_above_scrollbar_in_both_layouts(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut p = presentation(false);
            for i in 3..80 {
                Rc::get_mut(&mut p)
                    .unwrap()
                    .lines
                    .push(crate::git_diff::Row {
                        origin: " ".into(),
                        old: i.to_string(),
                        new: i.to_string(),
                        bytes: "long line ".repeat(300).into_bytes(),
                        ..Default::default()
                    });
            }
            View {
                presentation: p,
                cache: Cache::default(),
                documents: vec![],
                focus: cx.focus_handle(),
                scroll: UniformListScrollHandle::new(),
                split: false,
                snapshot: "last-line".into(),
            }
        });
        cx.simulate_resize(size(px(900.), px(400.)));
        for split in [false, true] {
            view.update(cx, |v, cx| {
                v.split = split;
                v.scroll = UniformListScrollHandle::new();
                cx.notify();
            });
            draw(cx);
            let last = view.read_with(cx, |v, _| {
                v.presentation.lines.len() - 1 - usize::from(split)
            });
            view.update(cx, |v, cx| {
                v.scroll.scroll_to_item(last, gpui::ScrollStrategy::Bottom);
                cx.notify();
            });
            draw(cx);
            view.read_with(cx, |v, _| {
                let bounds = v.scroll.0.borrow().base_handle.bounds();
                assert!(
                    bounds.bottom() <= px(388.),
                    "list must end above the 12px rail: {bounds:?}"
                );
                for doc in &v.documents {
                    let position = doc
                        .borrow()
                        .selection
                        .text_position(last, 0)
                        .expect("last line is visible");
                    assert!(
                        position.y < px(388.),
                        "last text line overlaps rail: {position:?}"
                    );
                }
            });
        }
    }
    #[test]
    fn cursor_is_not_comparison_identity_but_metadata_and_layout_are() {
        let mut page = json!({"snapshot":"a","metadata":{"side":"old"},"nextCursor":"next"});
        let first = super::key(&page, false, "demo.rs");
        page["nextCursor"] = Value::Null;
        assert_eq!(first, super::key(&page, false, "demo.rs"));
        assert_ne!(first, super::key(&page, true, "demo.rs"));
        assert_ne!(first, super::key(&page, false, "demo.js"));
        page["metadata"]["side"] = json!("new");
        assert_ne!(first, super::key(&page, false, "demo.rs"));
    }
}
