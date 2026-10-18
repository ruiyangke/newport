//! Native controls capture the exact comparison they were rendered from.
use super::*;
use newport_client::git_selection::Review;
use std::{cell::RefCell, collections::BTreeSet, rc::Rc};

#[path = "git_diff_drag.rs"]
mod drag;
#[path = "git_diff_range.rs"]
mod range;
use range::Gesture;
pub(super) use range::Selection as RangeSelection;
type SelectRange = Rc<dyn Fn(usize, &str, Gesture, &mut App)>;

type Apply = Rc<dyn Fn(&[String], Option<&[String]>, &mut App)>;
type Toggle = Rc<dyn Fn(Option<&[String]>, &mut App)>;
type Discard = Rc<dyn Fn(&[String], Option<&[String]>, &mut Window, &mut App)>;
#[derive(Clone)]
pub(super) struct Actions {
    stage: bool,
    enabled: bool,
    review: Rc<Review>,
    selection: Rc<RefCell<BTreeSet<String>>>,
    run_states: Rc<RefCell<std::collections::HashMap<String, (bool, bool)>>>,
    apply: Apply,
    toggle: Toggle,
    select_range: SelectRange,
    range_selection: Rc<RefCell<RangeSelection>>,
    discard: Option<Discard>,
}
impl Actions {
    fn verb(&self) -> &'static str {
        if self.stage { "Stage" } else { "Unstage" }
    }
    pub(super) fn button(&self, hunk: &str, line: &str) -> AnyElement {
        let enabled = self.enabled && self.review.hunk_complete(hunk);
        let apply = self.apply.clone();
        let hunk = hunk.to_owned();
        let discard_hunk = hunk.clone();
        // An inline action fits the diff's 22px line grid, like the Tauri hunk band.
        let button = Button::new(SharedString::from(format!("apply-diff-hunk-{hunk}")))
            .small()
            .ghost()
            .h(px(22.))
            .min_h(px(22.))
            .max_h(px(22.))
            .label(format!("{} hunk", self.verb()))
            .accessibility_label(format!("{} hunk at line {line}", self.verb()))
            .disabled(!enabled)
            .when(!self.review.hunk_complete(&discard_hunk), |button| {
                button.hover_hint("Load the rest of this hunk to apply it as a whole. Loaded lines can be selected now.")
            })
            .on_click(move |_, _, cx| apply(std::slice::from_ref(&hunk), None, cx));
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(button)
            .when_some(self.discard.clone(), |view, discard| {
                view.child(
                    Button::new(SharedString::from(format!(
                        "discard-diff-hunk-{discard_hunk}"
                    )))
                    .small()
                    .ghost()
                    .h(px(22.))
                    .min_h(px(22.))
                    .max_h(px(22.))
                    .icon(IconName::Undo2)
                    .accessibility_label(format!("Discard hunk at line {line}"))
                    .tooltip("Discard hunk…")
                    .disabled(!enabled)
                    .on_click(move |_, window, cx| {
                        discard(std::slice::from_ref(&discard_hunk), None, window, cx)
                    }),
                )
            })
            .into_any_element()
    }
    pub(super) fn line_control(
        &self,
        row: &git_diff::Row,
        run: Option<&git_diff_runs::Segment>,
        cx: &App,
    ) -> AnyElement {
        self.run_control(row, run, cx)
    }
    fn run_control(
        &self,
        row: &git_diff::Row,
        run: Option<&git_diff_runs::Segment>,
        cx: &App,
    ) -> AnyElement {
        let run = run.filter(|_| {
            row.selection
                .as_ref()
                .is_some_and(|(hunk, line)| self.review.contains_line(hunk, line))
        });
        let mut strip = div()
            .w(px(12.))
            .h(px(22.))
            .flex_shrink_0()
            .px(px(3.))
            .when(!self.enabled, |v| v.opacity(0.5));
        if let Some(run) = run {
            // Actions are rebuilt for each render. Count a visible run once,
            // rather than scanning a large run for each of its visible rows.
            let (full, mixed) = *self
                .run_states
                .borrow_mut()
                .entry(run.lines[0].clone())
                .or_insert_with(|| {
                    let selected = self.selection.borrow();
                    let count = run
                        .lines
                        .iter()
                        .filter(|line| selected.contains(*line))
                        .count();
                    (
                        count == run.lines.len(),
                        count > 0 && count < run.lines.len(),
                    )
                });
            let lines = run.lines.clone();
            let toggle = self.toggle.clone();
            let color = cx.theme().primary;
            let track = div()
                .size_full()
                .bg(color.opacity(if full {
                    1.
                } else if mixed {
                    0.55
                } else {
                    0.18
                }))
                .when(run.first, |v| v.rounded_t(px(2.)))
                .when(run.last, |v| v.rounded_b(px(2.)));
            let label = format!(
                "{} {} changed {} from line {}",
                self.verb(),
                lines.len(),
                if lines.len() == 1 { "line" } else { "lines" },
                if row.new.is_empty() {
                    &row.old
                } else {
                    &row.new
                }
            );
            if run.first {
                // Kit Base supplies mixed-state accessibility, keyboard activation
                // and focus; the narrow strip follows the original diff gutter.
                strip = strip.child(
                    gpui_kit::base::Checkbox::new(SharedString::from(format!(
                        "select-diff-run-{}",
                        lines[0]
                    )))
                    .checked(full)
                    .indeterminate(mixed)
                    .disabled(!self.enabled)
                    .accessibility_label(label)
                    .size_full()
                    .focus_visible(move |s| s.border_1().border_color(color))
                    .on_change(move |_, _, _, cx| toggle(Some(&lines), cx))
                    .child(track),
                );
            } else {
                let enabled = self.enabled;
                strip = strip.child(
                    track
                        .id(SharedString::from(format!(
                            "diff-run-strip-{}",
                            row.selection.as_ref().unwrap().1
                        )))
                        .on_click(move |_, _, cx| {
                            if enabled {
                                toggle(Some(&lines), cx);
                            }
                        }),
                );
            }
        }
        strip.into_any_element()
    }
    pub(super) fn selection_bar(&self, cx: &App) -> Option<AnyElement> {
        let ranges = self.range_selection.borrow();
        let current = self.selection.borrow();
        let selected = ranges.toolbar_selection().unwrap_or(&current);
        let enabled = self.enabled && ranges.toolbar_selection().is_none();
        let lines: Vec<_> = selected.iter().cloned().collect();
        if lines.is_empty() {
            return None;
        }
        let hunks = self.review.hunks_for_lines(&lines).ok()?;
        let noun = if lines.len() == 1 { "line" } else { "lines" };
        let apply = self.apply.clone();
        let toggle = self.toggle.clone();
        let mixed = self.review.retains_deletions(selected);
        let discard_lines = lines.clone();
        let discard_hunks = hunks.clone();
        Some(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(cx.theme().border)
                .child(label(format!("{} {noun} selected", lines.len())))
                .child(
                    crate::controls::compact_button("apply-selected-diff-lines")
                        .label(format!("{} {} {noun}", self.verb(), lines.len()))
                        .disabled(!enabled)
                        .on_click(move |_, _, cx| apply(&hunks, Some(&lines), cx)),
                )
                .child(
                    crate::controls::compact_button("clear-selected-diff-lines")
                        .label("Clear")
                        .disabled(!enabled)
                        .on_click(move |_, _, cx| toggle(None, cx)),
                )
                .when_some(self.discard.clone(), |view, discard| {
                    view.child(
                        crate::controls::compact_button("discard-selected-diff-lines")
                            .danger()
                            .label(format!("Discard {} {noun}…", discard_lines.len()))
                            .disabled(!enabled)
                            .on_click(move |_, window, cx| {
                                discard(&discard_hunks, Some(&discard_lines), window, cx)
                            }),
                    )
                })
                .when(mixed, |view| {
                    view.child(label("Deletions you did not select stay in the file.").text_xs())
                })
                .into_any_element(),
        )
    }
}

/// All callbacks revalidate the comparison and write gate at activation time.
pub(super) fn current(
    this: &Newport,
    server: &str,
    repo: &str,
    diff: &str,
    review: &Review,
) -> bool {
    this.server == server
        && this.git_repository_id() == repo
        && text(this.diff_page(), "snapshot") == diff
        && !this.git_is_inspection()
        && this.git_can_change_index(!review.staged())
        && review.matches(
            &text(&this.git.page, "snapshot"),
            &text(&this.git.selected, "entryId"),
            this.git.diff_staged,
            this.git.diff_context.value(),
        )
        && ["git-diff", "git-diff-more"].iter().all(|key| {
            this.resources
                .get(key)
                .is_none_or(|r| (*key == "git-diff-more" || !r.pending) && r.error.is_none())
        })
}
impl Newport {
    pub(super) fn diff_actions(
        &self,
        presentation: &git_diff::Presentation,
        cx: &Context<Self>,
    ) -> Option<Actions> {
        if self.git_is_inspection()
            || self.git.selected["conflicted"] == true
            || presentation.error.is_some()
        {
            return None;
        }
        let review = presentation.review.as_ref().ok()?.clone();
        let stage = !review.staged();
        let repo = self.git_repository_id();
        let server = self.server.clone();
        let diff_snapshot = text(self.diff_page(), "snapshot");
        let enabled = current(self, &server, &repo, &diff_snapshot, &review);
        let guard_review = review.clone();
        let guard = Rc::new(move |this: &Newport| {
            current(this, &server, &repo, &diff_snapshot, &guard_review)
        });
        let selection = presentation.selection.clone();
        let range_selection = presentation.range_selection.clone();
        {
            let mut ranges = range_selection.borrow_mut();
            if !enabled
                || ranges.active_lane().is_some_and(|lane| {
                    (lane == 0) != (self.diff_layout() == git_diff_split::Layout::Unified)
                })
            {
                ranges.reset();
            }
        }
        range_selection
            .borrow_mut()
            .prepare(presentation.lines.iter().filter_map(|row| {
                row.selection
                    .as_ref()
                    .filter(|(hunk, line)| review.contains_line(hunk, line))
                    .map(|(_, line)| (line.clone(), row.origin == "+"))
            }));
        let select_range: SelectRange = {
            let layout = self.diff_layout();
            let guard = guard.clone();
            let selection = selection.clone();
            let range_selection = range_selection.clone();
            let weak = cx.entity().downgrade();
            Rc::new(move |lane, line, gesture, cx| {
                let _ = weak.update(cx, |this, cx| {
                    if !guard(this)
                        || this.page != Page::Projects
                        || this.diff_layout() != layout
                        || this.git.tab != 0
                        || this.project_dialog_open
                        || this.git.remote_refs.is_some()
                        || (git_layout::compact(this.content_width, this.git.tab)
                            && !this.git.detail_open)
                    {
                        range_selection.borrow_mut().reset();
                        return;
                    }
                    range_selection.borrow_mut().apply(
                        lane,
                        line,
                        gesture,
                        &mut selection.borrow_mut(),
                    );
                    cx.notify();
                });
            })
        };
        let discard: Option<Discard> = (stage && self.git_can_discard()).then(|| {
            let guard = guard.clone();
            let review = review.clone();
            let weak = cx.entity().downgrade();
            Rc::new(
                move |hunks: &[String],
                      lines: Option<&[String]>,
                      window: &mut Window,
                      cx: &mut App| {
                    let _ = weak.update(cx, |this, cx| {
                        if guard(this) {
                            this.prepare_git_discard(
                                Some((
                                    review.clone(),
                                    hunks.to_vec(),
                                    lines.map(<[String]>::to_vec),
                                )),
                                window,
                                cx,
                            );
                        }
                    });
                },
            ) as Discard
        });
        let toggle = {
            let guard = guard.clone();
            let review = review.clone();
            let selection = selection.clone();
            let range_selection = range_selection.clone();
            let weak = cx.entity().downgrade();
            Rc::new(move |lines: Option<&[String]>, cx: &mut App| {
                let _ = weak.update(cx, |this, cx| {
                    if !guard(this) {
                        return;
                    }
                    range_selection.borrow_mut().reset();
                    let mut selected = selection.borrow_mut();
                    if let Some(lines) = lines {
                        if review.toggle_lines(&mut selected, lines).is_err() {
                            return;
                        }
                    } else {
                        selected.clear();
                    }
                    cx.notify();
                });
            })
        };
        let apply = {
            let weak = cx.entity().downgrade();
            let review = review.clone();
            Rc::new(
                move |hunks: &[String], lines: Option<&[String]>, cx: &mut App| {
                    let _ = weak.update(cx, |this, cx| {
                        if !guard(this) || review.action(hunks, lines).is_err() {
                            return;
                        }
                        let review = review.as_ref().clone();
                        let hunks = hunks.to_vec();
                        let lines = lines.map(<[String]>::to_vec);
                        let description = match &lines {
                            Some(lines) => format!(
                                "{} {} selected {} in {}",
                                if stage { "Stage" } else { "Unstage" },
                                lines.len(),
                                if lines.len() == 1 { "line" } else { "lines" },
                                this.diff_title()
                            ),
                            None => format!(
                                "{} hunk in {}",
                                if stage { "Stage" } else { "Unstage" },
                                this.diff_title()
                            ),
                        };
                        this.start_git_index_mutation(
                            this.git_repository_id(),
                            description,
                            move |backend, server, repo, id| {
                                backend.change_index_selection(
                                    server,
                                    repo,
                                    &review,
                                    &hunks,
                                    lines.as_deref(),
                                    id,
                                )
                            },
                            cx,
                        );
                    });
                },
            )
        };
        Some(Actions {
            stage,
            enabled,
            review,
            selection,
            run_states: Rc::default(),
            apply,
            toggle,
            select_range,
            range_selection,
            discard,
        })
    }
}

/// Use the line number itself as the selection target, matching the React diff.
/// Kit's unstyled checkbox retains keyboard and accessibility semantics without
/// adding a square checkbox to every code row.
pub(super) fn line_number(
    row: &git_diff::Row,
    new: bool,
    split: bool,
    actions: Option<&Actions>,
    cx: &App,
) -> AnyElement {
    let number = if row.heading {
        ""
    } else if new {
        &row.new
    } else {
        &row.old
    };
    let plain = || {
        cell(number.to_owned(), 45.)
            .h(px(22.))
            .text_color(cx.theme().muted_foreground)
            .text_right()
            .pr_2()
            .into_any_element()
    };
    let Some((actions, line)) = actions.and_then(|actions| {
        row.selection
            .as_ref()
            .filter(|(hunk, line)| actions.review.contains_line(hunk, line))
            .map(|(_, line)| (actions, line))
    }) else {
        return plain();
    };
    if (row.origin == "+") != new {
        if split {
            return plain();
        }
        // A vertical drag keeps its original x coordinate when crossing from
        // deletions to additions. The empty opposite-number cell must track it.
        let extend = actions.select_range.clone();
        let end = actions.select_range.clone();
        let line = line.clone();
        let move_line = line.clone();
        let enabled = actions.enabled;
        return div()
            .id(SharedString::from(format!("diff-range-secondary-{line}")))
            .child(plain())
            .on_mouse_move(move |event, _, cx| {
                if enabled && event.pressed_button == Some(MouseButton::Left) {
                    extend(0, &move_line, Gesture::Extend, cx);
                }
            })
            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                end(0, &line, Gesture::End, cx)
            })
            .into_any_element();
    }
    let selected = actions.selection.borrow().contains(line);
    let line = line.clone();
    let lane = if split { if new { 2 } else { 1 } } else { 0 };
    let begin = actions.select_range.clone();
    let extend = actions.select_range.clone();
    let end = actions.select_range.clone();
    let end_out = actions.select_range.clone();
    let activate = actions.select_range.clone();
    let down_line = line.clone();
    let move_line = line.clone();
    let up_line = line.clone();
    let out_line = line.clone();
    let enabled = actions.enabled;
    let color = cx.theme().primary;
    gpui_kit::base::Checkbox::new(SharedString::from(format!("select-diff-line-{line}")))
        .checked(selected)
        .disabled(!actions.enabled)
        .accessibility_label(format!(
            "{} {} line {number}",
            actions.verb(),
            if new { "added" } else { "removed" }
        ))
        .w(px(45.))
        .h(px(22.))
        .flex_shrink_0()
        .text_right()
        .pr_2()
        .text_color(if selected {
            color
        } else {
            cx.theme().muted_foreground
        })
        .when(selected, |v| {
            v.bg(theme::git_selection(cx.theme().is_dark()))
        })
        .when(actions.enabled, |v| {
            v.cursor_pointer().hover(move |s| s.bg(color.opacity(0.15)))
        })
        .focus_visible(move |s| s.border_1().border_color(color))
        .on_mouse_down(MouseButton::Left, move |event, _, cx| {
            if enabled {
                begin(
                    lane,
                    &down_line,
                    Gesture::Begin {
                        shift: event.modifiers.shift,
                    },
                    cx,
                );
            }
        })
        .on_mouse_move(move |event, _, cx| {
            if enabled && event.pressed_button == Some(MouseButton::Left) {
                extend(lane, &move_line, Gesture::Extend, cx);
            }
        })
        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
            end(lane, &up_line, Gesture::End, cx)
        })
        .on_mouse_up_out(MouseButton::Left, move |_, _, cx| {
            end_out(lane, &out_line, Gesture::End, cx)
        })
        .on_change(move |_, event, _, cx| {
            if !matches!(event, ClickEvent::Mouse(_)) {
                activate(
                    lane,
                    &line,
                    Gesture::Activate {
                        shift: event.modifiers().shift,
                    },
                    cx,
                );
            }
        })
        .child(number.to_owned())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::Actions;
    use crate::{git_diff::Row, git_diff_runs};
    use gpui::base::test_support::{ElementSnapshot, snapshots};
    use gpui::{
        App, AppContext, Context, IntoElement, ParentElement, Render, Styled, TestAppContext,
        VisualTestContext, Window, div, px,
    };
    use gpui_kit as gpui;
    use newport_client::git_selection::Review;
    use serde_json::json;
    use std::{cell::RefCell, collections::BTreeSet, rc::Rc};

    struct View {
        selection: Rc<RefCell<BTreeSet<String>>>,
        enabled: bool,
        ranges: Rc<RefCell<super::RangeSelection>>,
        text: Option<crate::selectable_log::SelectableLog>,
    }
    impl Render for View {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let review = Rc::new(Review::from_page(&json!({
                "nextCursor":"more","metadata":{"readOnly":false,"totalFiles":1,"sourceSnapshot":"status","entryId":"entry","side":"index_to_worktree","contextLines":3},
                "entries":[{"fileIndex":0,"status":"Modified","oldMode":33188,"newMode":33188,"oldPath":{"bytesB64":"Zg=="},"newPath":{"bytesB64":"Zg=="},"hunks":[{"id":"h","totalLines":3,"lines":[{"lineIndex":0,"id":"a","origin":"-","lineComplete":true},{"lineIndex":1,"id":"b","origin":"+","lineComplete":true}]}]}]
            })).unwrap());
            let weak = cx.entity().downgrade();
            let toggle_review = review.clone();
            let actions = Actions {
                stage: true,
                enabled: self.enabled,
                range_selection: self.ranges.clone(),
                select_range: {
                    self.ranges
                        .borrow_mut()
                        .prepare([("a".into(), false), ("b".into(), true)].into_iter());
                    let state = self.ranges.clone();
                    let weak = weak.clone();
                    Rc::new(move |lane, id, gesture, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            state.borrow_mut().apply(
                                lane,
                                id,
                                gesture,
                                &mut this.selection.borrow_mut(),
                            );
                            cx.notify();
                        });
                    })
                },
                review,
                selection: self.selection.clone(),
                run_states: Rc::default(),
                apply: Rc::new(|_, _, _| panic!("Incomplete hunk must not dispatch an action")),
                discard: None,
                toggle: Rc::new(move |lines, cx: &mut App| {
                    let _ = weak.update(cx, |this, cx| {
                        toggle_review
                            .toggle_lines(&mut this.selection.borrow_mut(), lines.unwrap())
                            .unwrap();
                        cx.notify();
                    });
                }),
            };
            let rows = [
                Row {
                    selection: Some(("h".into(), "a".into())),
                    old: "2".into(),
                    origin: "-".into(),
                    ..Default::default()
                },
                Row {
                    selection: Some(("h".into(), "b".into())),
                    new: "2".into(),
                    origin: "+".into(),
                    ..Default::default()
                },
            ];
            let groups = git_diff_runs::group(rows.iter().map(Some));
            let text = self.text.get_or_insert_with(|| {
                let mut text =
                    crate::selectable_log::SelectableLog::new(cx.focus_handle(), window, cx);
                text.set_rows(vec![Some("old text".into()), Some("new text".into())], cx);
                text
            });
            div()
                .size_full()
                .child(text.viewport(div().flex().flex_col().w(px(300.)).children(
                    rows.iter().enumerate().map(|(i, row)| {
                        div()
                            .flex()
                            .child(actions.line_control(row, groups[i].as_ref(), cx))
                            .child(super::line_number(
                                row,
                                row.origin == "+",
                                false,
                                Some(&actions),
                                cx,
                            ))
                            .child(text.full_row(i))
                    }),
                )))
                .child(actions.button("h", "2"))
        }
    }
    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    fn run(cx: &mut VisualTestContext) -> ElementSnapshot {
        cx.update(|window, _| {
            snapshots(window)
                .into_iter()
                .find(|n| n.label() == Some("Stage 2 changed lines from line 2"))
                .expect("run checkbox")
        })
    }
    #[gpui::test]
    fn run_control_exposes_mixed_state_keyboard_toggle_and_disabled_guard(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (view, cx) = cx.add_window_view(|_, _| View {
            selection: Rc::new(RefCell::new(BTreeSet::from(["a".into()]))),
            enabled: true,
            ranges: Rc::default(),
            text: None,
        });
        draw(cx);
        assert_eq!(run(cx).indeterminate(), Some(true));
        let at = run(cx).bounds().center();
        cx.simulate_click(at, Default::default());
        draw(cx);
        assert_eq!(view.read_with(cx, |v, _| v.selection.borrow().len()), 2);
        assert_eq!(run(cx).checked(), Some(true));
        let keystroke = gpui::Keystroke::parse("space").unwrap();
        cx.simulate_event(gpui::KeyDownEvent {
            physical_key: None,
            key_metadata: None,
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        cx.simulate_event(gpui::KeyUpEvent {
            physical_key: None,
            key_metadata: None,
            keystroke,
        });
        draw(cx);
        assert!(view.read_with(cx, |v, _| v.selection.borrow().is_empty()));
        view.update(cx, |v, cx| {
            v.enabled = false;
            cx.notify();
        });
        draw(cx);
        let node = run(cx);
        cx.simulate_click(node.bounds().center(), Default::default());
        draw(cx);
        assert!(view.read_with(cx, |v, _| v.selection.borrow().is_empty()));
    }
    #[gpui::test]
    fn line_numbers_select_independently_with_keyboard_and_disabled_guard(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (view, cx) = cx.add_window_view(|_, _| View {
            selection: Rc::default(),
            enabled: true,
            ranges: Rc::default(),
            text: None,
        });
        draw(cx);
        let number = |cx: &mut VisualTestContext| {
            cx.update(|window, _| {
                snapshots(window)
                    .into_iter()
                    .find(|n| n.label() == Some("Stage added line 2"))
                    .expect("line number checkbox")
            })
        };
        let hunk = cx.update(|window, _| {
            snapshots(window)
                .into_iter()
                .find(|n| n.label() == Some("Stage hunk at line 2"))
                .expect("hunk action")
        });
        cx.simulate_click(hunk.bounds().center(), Default::default());
        draw(cx);
        assert!(view.read_with(cx, |v, _| v.selection.borrow().is_empty()));
        assert_ne!(number(cx).disabled(), Some(true));
        assert_eq!(number(cx).checked(), Some(false));
        let at = number(cx).bounds().center();
        cx.simulate_click(at, Default::default());
        draw(cx);
        assert_eq!(
            view.read_with(cx, |v, _| v.selection.borrow().clone()),
            BTreeSet::from(["b".into()])
        );
        assert_eq!(number(cx).checked(), Some(true));
        let keystroke = gpui::Keystroke::parse("space").unwrap();
        cx.simulate_event(gpui::KeyDownEvent {
            physical_key: None,
            key_metadata: None,
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        cx.simulate_event(gpui::KeyUpEvent {
            physical_key: None,
            key_metadata: None,
            keystroke,
        });
        draw(cx);
        assert!(view.read_with(cx, |v, _| v.selection.borrow().is_empty()));
        view.update(cx, |v, cx| {
            v.enabled = false;
            cx.notify();
        });
        draw(cx);
        let at = number(cx).bounds().center();
        cx.simulate_click(at, Default::default());
        draw(cx);
        assert!(view.read_with(cx, |v, _| v.selection.borrow().is_empty()));
    }
    #[gpui::test]
    fn gutter_pointer_drag_and_shift_click_extend_the_displayed_range(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let mut view = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let entity = cx.new(|_| View {
                selection: Rc::default(),
                enabled: true,
                ranges: Rc::default(),
                text: None,
            });
            view = Some(entity.clone());
            gpui::base::Root::new(entity, window, cx)
        });
        let view = view.unwrap();
        draw(cx);
        let position = |cx: &mut VisualTestContext, label: &str| {
            cx.update(|window, _| {
                snapshots(window)
                    .into_iter()
                    .find(|n| n.label() == Some(label))
                    .unwrap()
                    .bounds()
                    .center()
            })
        };
        let a = position(cx, "Stage removed line 2");
        let b = position(cx, "Stage added line 2");
        cx.simulate_mouse_down(a, gpui::MouseButton::Left, Default::default());
        draw(cx);
        cx.simulate_mouse_move(
            gpui::point(a.x, b.y),
            Some(gpui::MouseButton::Left),
            Default::default(),
        );
        draw(cx);
        assert_eq!(
            view.read_with(cx, |v, _| v.selection.borrow().clone()),
            BTreeSet::from(["a".into(), "b".into()])
        );
        cx.simulate_mouse_up(b, gpui::MouseButton::Left, Default::default());
        draw(cx);
        // Starting on a selected line deselects; retreating restores the rest.
        cx.simulate_mouse_down(b, gpui::MouseButton::Left, Default::default());
        cx.simulate_mouse_move(a, Some(gpui::MouseButton::Left), Default::default());
        draw(cx);
        assert!(view.read_with(cx, |v, _| v.selection.borrow().is_empty()));
        cx.simulate_mouse_up(a, gpui::MouseButton::Left, Default::default());
        draw(cx);
        cx.simulate_click(a, Default::default());
        draw(cx);
        cx.simulate_click(
            b,
            gpui::Modifiers {
                shift: true,
                ..Default::default()
            },
        );
        draw(cx);
        assert_eq!(
            view.read_with(cx, |v, _| v.selection.borrow().clone()),
            BTreeSet::from(["a".into(), "b".into()])
        );
        assert_eq!(cx.update(gpui::base::TextSelection::selected_text), "");
        let (from, to) = view.read_with(cx, |v, _| {
            let text = v.text.as_ref().unwrap();
            (
                text.text_position(0, 0).unwrap(),
                text.text_position(1, 8).unwrap(),
            )
        });
        let offset = gpui::point(px(0.1), px(5.));
        cx.simulate_mouse_down(from + offset, gpui::MouseButton::Left, Default::default());
        cx.simulate_mouse_move(
            to + offset,
            Some(gpui::MouseButton::Left),
            Default::default(),
        );
        cx.simulate_mouse_up(to + offset, gpui::MouseButton::Left, Default::default());
        draw(cx);
        assert_eq!(
            cx.update(gpui::base::TextSelection::selected_text),
            "old text\nnew text"
        );
        assert_eq!(
            view.read_with(cx, |v, _| v.selection.borrow().clone()),
            BTreeSet::from(["a".into(), "b".into()])
        );
    }
}
