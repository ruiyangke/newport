//! Presentation of the agent's diff order, never a second diff algorithm.
use super::*;
use crate::git_diff::Row;
use std::rc::Rc;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Layout {
    #[default]
    Unified,
    Split,
}

#[derive(Debug, PartialEq, Eq)]
struct Pair {
    old: Option<usize>,
    new: Option<usize>,
}
pub(super) struct Model {
    pairs: Vec<Pair>,
    width: Pixels,
    runs: [Vec<Option<git_diff_runs::Segment>>; 2],
}
impl Model {
    pub(super) fn source_row(&self, row: usize) -> Option<usize> {
        self.pairs
            .get(row)
            .or_else(|| self.pairs.last())
            .and_then(|pair| pair.old.or(pair.new))
    }
    pub(super) fn row_count(&self) -> usize {
        self.pairs.len()
    }
}

fn both(row: &Row) -> bool {
    row.heading || matches!(row.origin.as_str(), " " | "=" | "")
}

/// Pair each deletion with the corresponding addition within a change run.
/// Context and headings end a run; missing lines become blank spacer rows.
fn align(rows: &[Row]) -> Vec<Pair> {
    let mut result = Vec::new();
    let mut index = 0;
    while index < rows.len() {
        if both(&rows[index]) {
            result.push(Pair {
                old: Some(index),
                new: Some(index),
            });
            index += 1;
            continue;
        }
        let mut old = Vec::new();
        let mut new = Vec::new();
        while index < rows.len() && !both(&rows[index]) {
            if matches!(rows[index].origin.as_str(), "-" | "<") {
                old.push(index);
            } else {
                new.push(index);
            }
            index += 1;
        }
        for i in 0..old.len().max(new.len()) {
            result.push(Pair {
                old: old.get(i).copied(),
                new: new.get(i).copied(),
            });
        }
    }
    result
}

impl Newport {
    pub(super) fn diff_layout(&self) -> Layout {
        self.git_diff_layouts
            .get(&self.server)
            .copied()
            .unwrap_or_default()
    }

    pub(super) fn split_diff(
        &self,
        presentation: Rc<git_diff::Presentation>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let actions = self.diff_actions(&presentation, cx);
        let model = model(&presentation, cx);
        let documents = self.git.diff_text.prepare(
            git_diff_text::key(self.diff_page(), true, &self.diff_title()),
            &presentation,
            || {
                [false, true]
                    .map(|new| {
                        model
                            .pairs
                            .iter()
                            .map(|pair| {
                                git_diff_text::row(
                                    (if new { pair.new } else { pair.old })
                                        .map(|i| &presentation.lines[i]),
                                )
                            })
                            .collect()
                    })
                    .into()
            },
            &self.focus,
            window,
            cx,
        );
        render(
            presentation,
            &self.git.diff_scroll,
            actions,
            Some(documents),
            cx,
        )
    }
}

fn model<'a>(presentation: &'a git_diff::Presentation, cx: &App) -> &'a Model {
    // Equal content extents let both panes safely share the same scroll
    // offset in both axes. Measure glyphs (including Unicode), not bytes.
    presentation.split.get_or_init(|| {
        let width = presentation.text_width(cx) + px(260.);
        let pairs = align(&presentation.lines);
        let runs = [false, true].map(|new| {
            git_diff_runs::group(pairs.iter().map(|pair| {
                (if new { pair.new } else { pair.old }).map(|index| &presentation.lines[index])
            }))
        });
        Model { pairs, width, runs }
    })
}

pub(super) fn render<T: 'static>(
    presentation: Rc<git_diff::Presentation>,
    scroll: &UniformListScrollHandle,
    actions: Option<git_diff_actions::Actions>,
    documents: Option<Vec<git_diff_text::Document>>,
    cx: &mut Context<T>,
) -> AnyElement {
    let model = model(&presentation, cx);
    let width = model.width;
    let count = model.pairs.len();
    div()
        .flex()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .children([false, true].into_iter().map(|new| {
            let presentation = presentation.clone();
            let actions = actions.clone();
            let drag_actions = actions.clone();
            let drag_presentation = presentation.clone();
            let document = documents
                .as_ref()
                .map(|docs| docs[usize::from(new)].clone());
            let row_document = document.clone();
            div()
                .debug_selector(move || {
                    if new {
                        "diff-new-pane"
                    } else {
                        "diff-old-pane"
                    }
                    .into()
                })
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .when(new, |view| {
                    view.border_l_1().border_color(cx.theme().border)
                })
                .child(
                    label(if new { "New version" } else { "Old version" })
                        .px_3()
                        .py_1()
                        .text_xs()
                        .border_b_1()
                        .border_color(cx.theme().border),
                )
                .child(
                    uniform_list(
                        if new {
                            "diff-new-version"
                        } else {
                            "diff-old-version"
                        },
                        count,
                        cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                            range
                                .map(|i| {
                                    let pair = &presentation
                                        .split
                                        .get()
                                        .expect("initialized split model")
                                        .pairs[i];
                                    let row = if new { pair.new } else { pair.old }
                                        .map(|index| &presentation.lines[index]);
                                    div()
                                        .h(px(22.))
                                        .w(width)
                                        .min_w_full()
                                        .flex()
                                        .items_center()
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .text_size(px(12.))
                                        .bg(match row {
                                            Some(row) if row.heading => {
                                                theme::diff_hunk(cx.theme().is_dark())
                                            }
                                            Some(row) if row.origin == "+" => {
                                                theme::diff_add(cx.theme().is_dark())
                                            }
                                            Some(row) if row.origin == "-" => {
                                                theme::diff_remove(cx.theme().is_dark())
                                            }
                                            Some(_) => cx.theme().background,
                                            None => cx.theme().muted,
                                        })
                                        .when_some(row, |view, row| {
                                            view.when_some(actions.as_ref(), |view, actions| {
                                                view.child(
                                                    actions.line_control(
                                                        row,
                                                        presentation.split.get().unwrap().runs
                                                            [usize::from(new)][i]
                                                            .as_ref(),
                                                        cx,
                                                    ),
                                                )
                                            })
                                            .child(git_diff_actions::line_number(
                                                row,
                                                new,
                                                true,
                                                actions.as_ref(),
                                                cx,
                                            ))
                                            .child(cell(
                                                if row.heading {
                                                    String::new()
                                                } else {
                                                    row.origin.clone()
                                                },
                                                18.,
                                            ))
                                            .child(
                                                git_diff_text::code_container(row, i, Some(new))
                                                    .flex_shrink_0()
                                                    .whitespace_nowrap()
                                                    .text_color(if row.heading {
                                                        cx.theme().muted_foreground
                                                    } else if row.origin == "+" {
                                                        theme::diff_add_ink(cx.theme().is_dark())
                                                    } else if row.origin == "-" {
                                                        theme::diff_remove_ink(cx.theme().is_dark())
                                                    } else {
                                                        cx.theme().foreground
                                                    })
                                                    .child(
                                                        if let Some(document) = row_document
                                                            .as_ref()
                                                            .filter(|_| !row.heading)
                                                        {
                                                            git_diff_text::text(document, i, cx)
                                                        } else {
                                                            div()
                                                                .child(row.text())
                                                                .into_any_element()
                                                        },
                                                    ),
                                            )
                                            .when_some(
                                                actions
                                                    .as_ref()
                                                    .zip(row.hunk.as_deref())
                                                    .filter(|_| new),
                                                |view, (actions, hunk)| {
                                                    view.child(
                                                        div()
                                                            .ml_3()
                                                            .child(actions.button(hunk, &row.new)),
                                                    )
                                                },
                                            )
                                        })
                                })
                                .collect()
                        }),
                    )
                    .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                    .track_scroll(scroll)
                    .flex_1()
                    .map(|list| match document {
                        Some(document) => document
                            .borrow()
                            .selection
                            .viewport(list)
                            .into_any_element(),
                        None => list.into_any_element(),
                    })
                    .map(|list| match drag_actions {
                        Some(actions) => actions.viewport(
                            if new { 2 } else { 1 },
                            || {
                                drag_presentation
                                    .split
                                    .get()
                                    .unwrap()
                                    .pairs
                                    .iter()
                                    .enumerate()
                                    .filter_map(|(i, pair)| {
                                        let index = if new { pair.new } else { pair.old }?;
                                        drag_presentation.lines[index]
                                            .selection
                                            .as_ref()
                                            .map(|(_, line)| (i, line.clone()))
                                    })
                                    .collect()
                            },
                            scroll,
                            list,
                        ),
                        None => list,
                    })
                    .map(|list| {
                        git_diff_text::scroll_viewport(
                            if new {
                                "new-diff-scrollbar"
                            } else {
                                "old-diff-scrollbar"
                            },
                            list,
                            scroll,
                        )
                    }),
                )
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{Pair, Row, align};
    fn row(origin: &str) -> Row {
        Row {
            origin: origin.into(),
            ..Default::default()
        }
    }
    #[test]
    fn visible_split_rows_resolve_to_the_correct_source_file() {
        let rows = [
            Row {
                heading: true,
                file_path: Some("docs/a.md".into()),
                ..Default::default()
            },
            row("-"),
            row("+"),
            row("+"),
            Row {
                heading: true,
                file_path: Some("docs/b.md".into()),
                ..Default::default()
            },
            row("+"),
        ];
        let model = super::Model {
            pairs: align(&rows),
            width: super::px(0.),
            runs: [vec![], vec![]],
        };
        assert_eq!(model.source_row(1), Some(1));
        assert_eq!(model.source_row(2), Some(3));
        assert_eq!(model.source_row(3), Some(4));
        assert_eq!(model.source_row(4), Some(5));
    }
    #[test]
    fn replacements_pair_and_pad_before_context_resumes() {
        let rows = [row(" "), row("-"), row("-"), row("+"), row(" ")];
        assert_eq!(
            align(&rows),
            vec![
                Pair {
                    old: Some(0),
                    new: Some(0)
                },
                Pair {
                    old: Some(1),
                    new: Some(3)
                },
                Pair {
                    old: Some(2),
                    new: None
                },
                Pair {
                    old: Some(4),
                    new: Some(4)
                }
            ]
        );
    }
    #[test]
    fn boundaries_prevent_pairing_unrelated_hunks() {
        let rows = [
            row("-"),
            Row {
                heading: true,
                ..Default::default()
            },
            row("+"),
        ];
        assert_eq!(
            align(&rows),
            vec![
                Pair {
                    old: Some(0),
                    new: None
                },
                Pair {
                    old: Some(1),
                    new: Some(1)
                },
                Pair {
                    old: None,
                    new: Some(2)
                }
            ]
        );
    }
    #[test]
    fn empty_and_one_sided_comparisons_have_equal_visual_row_counts() {
        assert!(align(&[]).is_empty());
        for origin in ["+", "-"] {
            let pairs = align(&[row(origin), row(origin)]);
            assert_eq!(pairs.len(), 2);
            assert!(pairs.iter().all(|pair| if origin == "+" {
                pair.old.is_none() && pair.new.is_some()
            } else {
                pair.old.is_some() && pair.new.is_none()
            }));
        }
    }
    #[test]
    fn eof_markers_stay_on_their_own_sides() {
        let rows = [row("-"), row("<"), row("+"), row(">"), row("=")];
        assert_eq!(
            align(&rows),
            vec![
                Pair {
                    old: Some(0),
                    new: Some(2)
                },
                Pair {
                    old: Some(1),
                    new: Some(3)
                },
                Pair {
                    old: Some(4),
                    new: Some(4)
                }
            ]
        );
    }
}

#[cfg(test)]
mod scroll_tests {
    use super::render;
    use crate::git_diff::{Presentation, Row};
    use gpui_kit::{
        self as gpui, Context, IntoElement, Render, ScrollDelta, ScrollWheelEvent, TestAppContext,
        UniformListScrollHandle, VisualTestContext, Window, div, point, prelude::*, px, size,
    };
    use std::{cell::OnceCell, rc::Rc};

    struct View {
        presentation: Rc<Presentation>,
        scroll: UniformListScrollHandle,
    }
    impl Render for View {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().flex().child(render(
                self.presentation.clone(),
                &self.scroll,
                None,
                None,
                cx,
            ))
        }
    }
    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    #[gpui::test]
    fn split_panes_scroll_long_lines_and_spacers_together(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (view, cx) = cx.add_window_view(|_, _| View {
            presentation: Rc::new(Presentation {
                selection: Rc::default(),
                range_selection: Rc::default(),
                lines: (0..300)
                    .map(|i| Row {
                        origin: "+".into(),
                        new: i.to_string(),
                        bytes: if i == 100 {
                            "x".repeat(3000).into_bytes()
                        } else {
                            b"short".to_vec()
                        },
                        ..Default::default()
                    })
                    .collect(),
                review: Err("test".into()),
                error: None,
                split: OnceCell::new(),
                runs: OnceCell::new(),
                width: OnceCell::new(),
            }),
            scroll: UniformListScrollHandle::new(),
        });
        cx.simulate_resize(size(px(900.), px(600.)));
        draw(cx);
        for (step, pane) in ["diff-new-pane", "diff-old-pane"].into_iter().enumerate() {
            let bounds = cx.debug_bounds(pane).unwrap();
            cx.simulate_event(ScrollWheelEvent {
                position: bounds.center(),
                delta: ScrollDelta::Pixels(point(px(-300.), px(0.))),
                ..Default::default()
            });
            draw(cx);
            cx.simulate_event(ScrollWheelEvent {
                position: bounds.center(),
                delta: ScrollDelta::Pixels(point(px(0.), px(-200.))),
                ..Default::default()
            });
            draw(cx);
            view.read_with(cx, |v, _| {
                let offset = v.scroll.0.borrow().base_handle.offset();
                assert_eq!(offset.x, px(-300. * (step + 1) as f32));
                assert_eq!(offset.y, px(-200. * (step + 1) as f32));
            });
        }
    }
}
