//! Find within the loaded diff only. This never issues Git requests or selects
//! staging IDs; match coordinates are presentation rows and UTF-8 byte ranges.
use super::*;
use gpui_kit::component::{Selectable, input::InputEvent};
use regex::RegexBuilder;
use std::ops::Range;

const MAX_MATCHES: usize = 10_000;
#[derive(Clone, Debug, PartialEq, Eq)]
struct Match {
    side: usize,
    row: usize,
    range: Range<usize>,
}
#[derive(Default)]
pub(super) struct Model {
    pub open: bool,
    query: String,
    case: bool,
    word: bool,
    regex: bool,
    identity: Value,
    rows: Vec<Vec<Option<String>>>,
    matches: Vec<Match>,
    active: usize,
    truncated: bool,
    error: Option<String>,
}
impl Model {
    pub(super) fn set_query(&mut self, query: String) {
        self.query = query;
        self.refresh(false);
    }
    pub(super) fn clear_rows(&mut self) {
        if !self.rows.is_empty() {
            self.set_rows(Value::Null, vec![]);
        }
    }
    pub(super) fn set_rows(&mut self, identity: Value, rows: Vec<Vec<Option<String>>>) {
        let retain = self.identity == identity;
        self.identity = identity;
        self.rows = rows;
        self.refresh(retain);
    }
    fn refresh(&mut self, retain: bool) {
        let previous = retain
            .then(|| self.matches.get(self.active).cloned())
            .flatten();
        self.matches.clear();
        self.active = 0;
        self.error = None;
        self.truncated = false;
        if !self.open || self.query.is_empty() {
            return;
        }
        let pattern = if self.regex {
            self.query.clone()
        } else {
            regex::escape(&self.query)
        };
        let pattern = if self.word {
            format!(r"\b(?:{pattern})\b")
        } else {
            pattern
        };
        let pattern = match RegexBuilder::new(&pattern)
            .case_insensitive(!self.case)
            .size_limit(1 << 20)
            .build()
        {
            Ok(pattern) => pattern,
            Err(_) => {
                self.error = Some("Invalid or overly complex regular expression".into());
                return;
            }
        };
        'sides: for (side, rows) in self.rows.iter().enumerate() {
            for (row, text) in rows.iter().enumerate() {
                let Some(text) = text else {
                    continue;
                };
                for found in pattern.find_iter(text) {
                    if self.matches.len() == MAX_MATCHES {
                        self.truncated = true;
                        break 'sides;
                    }
                    self.matches.push(Match {
                        side,
                        row,
                        range: found.range(),
                    });
                }
            }
        }
        if let Some(previous) = previous {
            self.active = self
                .matches
                .iter()
                .position(|m| *m == previous)
                .unwrap_or(0);
        }
    }
    fn step(&mut self, previous: bool) {
        let count = self.matches.len();
        if count > 0 {
            self.active = (self.active + if previous { count - 1 } else { 1 }) % count;
        }
    }
    pub(super) fn highlights(
        &self,
        side: usize,
        row: usize,
        cx: &App,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        if !self.open {
            return vec![];
        }
        // Matches are ordered by pane, row and offset. Only inspect this row.
        let start = self
            .matches
            .partition_point(|m| (m.side, m.row) < (side, row));
        self.matches[start..]
            .iter()
            .enumerate()
            .take_while(|(_, m)| (m.side, m.row) == (side, row))
            .map(|(i, m)| {
                (
                    m.range.clone(),
                    HighlightStyle {
                        background_color: Some(
                            cx.theme().primary.opacity(if start + i == self.active {
                                0.38
                            } else {
                                0.18
                            }),
                        ),
                        ..Default::default()
                    },
                )
            })
            .collect()
    }
    fn status(&self) -> String {
        if let Some(error) = &self.error {
            return error.clone();
        }
        if self.query.is_empty() {
            return "Loaded preview only".into();
        }
        if self.matches.is_empty() {
            return "No matches in loaded preview".into();
        }
        format!(
            "{} / {}{} · loaded preview",
            self.active + 1,
            self.matches.len(),
            if self.truncated { "+" } else { "" }
        )
    }
}

pub(super) struct Controls {
    input: Entity<InputState>,
    _subscription: Subscription,
}
impl Newport {
    pub(super) fn open_git_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page != Page::Projects
            || !(self.git.tab == 0 || self.git_is_inspection())
            || self.git.remote_refs.is_some()
            || !(if self.git_is_inspection() {
                self.git.history.path["bytesB64"].is_string()
            } else {
                self.git.selected["entryId"].is_string()
            })
            || self.git.selected["conflicted"] == true
            || self.diff_page()["snapshot"].is_null()
        {
            return;
        }
        let model = self.git.diff_text.search.clone();
        model.borrow_mut().open = true;
        model.borrow_mut().refresh(true);
        if self.git.diff_find.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find in diff"));
            input.update(cx, |input, cx| {
                input.set_value(model.borrow().query.clone(), window, cx)
            });
            let subscription = cx.subscribe_in(&input, window, |this, input, event, _, cx| {
                match event {
                    InputEvent::Change => {
                        let model = this.git.diff_text.search.clone();
                        let mut model = model.borrow_mut();
                        model.set_query(input.read(cx).value().to_string());
                        drop(model);
                        this.reveal_git_match(cx);
                    }
                    InputEvent::PressEnter { shift, .. } => this.next_git_match(*shift, cx),
                    _ => {}
                }
                cx.notify();
            });
            self.git.diff_find = Some(Controls {
                input,
                _subscription: subscription,
            });
        }
        self.git
            .diff_find
            .as_ref()
            .unwrap()
            .input
            .update(cx, |input, cx| {
                input.select_all(window, cx);
                input.focus_handle(cx).focus(window, cx);
            });
        cx.notify();
    }
    fn close_git_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.git.diff_find = None;
        self.git.diff_text.search.borrow_mut().open = false;
        self.focus.focus(window, cx);
        cx.notify();
    }
    fn next_git_match(&mut self, previous: bool, cx: &mut Context<Self>) {
        self.git.diff_text.search.borrow_mut().step(previous);
        self.reveal_git_match(cx);
        cx.notify();
    }
    fn reveal_git_match(&mut self, cx: &App) {
        // A previous wheel gesture must not turn a programmatic search jump
        // into demand for another remote page.
        self.git.diff_demand = git_diff::ScrollDemand::default();
        let model = self.git.diff_text.search.borrow();
        let Some(found) = model.matches.get(model.active) else {
            return;
        };
        self.git
            .diff_scroll
            .scroll_to_item(found.row, ScrollStrategy::Center);
        let text = model.rows[found.side][found.row].as_ref().unwrap();
        let prefix: SharedString = text[..found.range.start].to_owned().into();
        let width = WindowTextSystem::new(cx.text_system().clone())
            .shape_line(
                prefix.clone(),
                px(crate::ui::FONT_SIZE),
                &[TextRun {
                    len: prefix.len(),
                    font: font(cx.theme().mono_font_family.clone()),
                    ..Default::default()
                }],
                None,
            )
            .width;
        let handle = self.git.diff_scroll.0.borrow();
        let offset = handle.base_handle.offset();
        // Keep the match near the leading edge, leaving the gutter visible for
        // short prefixes. Both split panes use the same horizontal coordinate.
        handle.base_handle.set_offset(point(
            -width.min(handle.base_handle.max_offset().x),
            offset.y,
        ));
    }
    pub(super) fn git_find_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(controls) = &self.git.diff_find else {
            return div().into_any_element();
        };
        let model = self.git.diff_text.search.borrow();
        let disabled = model.matches.is_empty();
        let status = model.status();
        div()
            .id("git-diff-find")
            .role(Role::Search)
            .aria_label("Find in diff")
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .py(px(8.))
            .border_b_1()
            .border_color(cx.theme().border)
            .on_action(
                cx.listener(|this, _: &gpui_kit::component::input::Escape, window, cx| {
                    this.close_git_find(window, cx)
                }),
            )
            .child(
                crate::controls::search_input(&controls.input, cx)
                    .w(px(crate::controls::SEARCH_WIDTH))
                    .max_w_full()
                    .aria_label("Find in diff"),
            )
            .children(
                [
                    (0usize, "Aa", "Match case", model.case),
                    (1, "Word", "Whole word", model.word),
                    (2, ".*", "Regular expression", model.regex),
                ]
                .into_iter()
                .map(|(index, title, name, selected)| {
                    crate::controls::compact_button(("diff-find-option", index))
                        .label(title)
                        .ghost()
                        .selected(selected)
                        .toggled(selected)
                        .accessibility_label(name)
                        .hover_hint(name)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let mut model = this.git.diff_text.search.borrow_mut();
                            match index {
                                0 => model.case = !model.case,
                                1 => model.word = !model.word,
                                _ => model.regex = !model.regex,
                            }
                            model.refresh(false);
                            drop(model);
                            this.reveal_git_match(cx);
                            cx.notify();
                        }))
                }),
            )
            .child(
                div()
                    .id("diff-find-status")
                    .role(Role::Status)
                    .aria_label(status.clone())
                    .flex_1()
                    .text_size(px(crate::ui::FONT_SIZE))
                    .child(status),
            )
            .children(
                [
                    (true, IconName::ChevronUp, "Previous match"),
                    (false, IconName::ChevronDown, "Next match"),
                ]
                .into_iter()
                .map(|(previous, icon, name)| {
                    crate::controls::compact_icon_button(if previous {
                        "diff-find-prev"
                    } else {
                        "diff-find-next"
                    })
                    .icon(icon)
                    .ghost()
                    .accessibility_label(name)
                    .hover_hint(name)
                    .disabled(disabled)
                    .on_click(cx.listener(move |this, _, _, cx| this.next_git_match(previous, cx)))
                }),
            )
            .child(
                crate::controls::compact_icon_button("diff-find-close")
                    .icon(IconName::X)
                    .ghost()
                    .accessibility_label("Close diff search")
                    .tooltip("Close (Esc)")
                    .on_click(cx.listener(|this, _, window, cx| this.close_git_find(window, cx))),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_MATCHES, Model};
    use serde_json::json;
    fn model(rows: &[Option<&str>]) -> Model {
        let mut m = Model {
            open: true,
            ..Default::default()
        };
        m.set_rows(
            json!("first"),
            vec![rows.iter().map(|r| r.map(str::to_owned)).collect()],
        );
        m
    }
    #[test]
    fn unicode_literal_case_word_and_regex_are_utf8_ranges() {
        let mut m = model(&[None, Some("Été été étéx 雪 🦀 a.*")]);
        m.set_query("été".into());
        assert_eq!(m.matches.len(), 3);
        for found in &m.matches {
            assert!(
                m.rows[found.side][found.row]
                    .as_ref()
                    .unwrap()
                    .get(found.range.clone())
                    .is_some()
            );
        }
        m.word = true;
        m.refresh(false);
        assert_eq!(m.matches.len(), 2);
        m.case = true;
        m.refresh(false);
        assert_eq!(m.matches.len(), 1);
        m.word = false;
        m.set_query("a.*".into());
        assert_eq!(
            &m.rows[0][1].as_ref().unwrap()[m.matches[0].range.clone()],
            "a.*"
        );
        m.regex = true;
        m.set_query("雪|🦀".into());
        assert_eq!(m.matches.len(), 2);
        m.set_query("[".into());
        assert!(m.error.is_some());
        assert!(m.matches.is_empty());
        m.set_query("^".into());
        assert!(m.error.is_none());
        assert_eq!(m.matches[0].range, 0..0);
    }
    #[test]
    fn navigation_wraps_and_append_keeps_current_match_but_replacement_resets() {
        let mut m = model(&[Some("needle needle")]);
        m.set_query("needle".into());
        m.step(true);
        assert_eq!(m.active, 1);
        m.step(false);
        assert_eq!(m.active, 0);
        m.step(false);
        assert_eq!(m.active, 1);
        m.set_rows(
            json!("first"),
            vec![vec![Some("needle needle".into()), Some("needle".into())]],
        );
        assert_eq!(m.active, 1);
        assert_eq!(m.matches.len(), 3);
        m.set_rows(json!("second"), vec![vec![Some("needle needle".into())]]);
        assert_eq!(m.active, 0);
        m.clear_rows();
        assert!(m.matches.is_empty());
    }
    #[test]
    fn split_coordinates_ignore_decorative_rows_and_never_cross_spacers() {
        let mut m = Model {
            open: true,
            ..Default::default()
        };
        m.set_rows(
            json!("split"),
            vec![
                vec![None, Some("old".into()), None],
                vec![None, None, Some("new".into())],
            ],
        );
        m.regex = true;
        m.set_query("old|new".into());
        assert_eq!(
            m.matches
                .iter()
                .map(|m| (m.side, m.row))
                .collect::<Vec<_>>(),
            vec![(0, 1), (1, 2)]
        );
        m.set_query("old.*new".into());
        assert!(m.matches.is_empty());
    }
    #[test]
    fn matches_are_bounded_and_closed_search_does_no_matching() {
        let text = "x ".repeat(MAX_MATCHES + 1);
        let mut m = model(&[Some(&text)]);
        m.set_query("x".into());
        assert_eq!(m.matches.len(), MAX_MATCHES);
        assert!(m.truncated);
        m.open = false;
        m.refresh(false);
        assert!(m.matches.is_empty());
        m.open = true;
        m.set_query("".into());
        assert!(m.matches.is_empty());
    }
}
