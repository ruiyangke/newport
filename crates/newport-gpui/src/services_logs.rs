use super::*;
use crate::log_line::{LogLine, RowBounds, Tone as LogTone, day_strip};
use crate::selectable_log::SelectableLog;
use gpui_kit::component::input::{EditorState, InputEvent};
use newport_domain::services::JournalEntry;
use std::cell::RefCell;

#[derive(Clone, PartialEq)]
enum LogRow {
    Day { key: String, label: String },
    // Keep the visible timestamp and selectable document in the same locale
    // snapshot. A locale change must never invalidate a text run's byte ranges.
    // The entry's message has its escapes stripped; colours ride alongside.
    Line(JournalEntry, String, crate::log_line::Styles),
}
pub(super) struct Logs {
    pub(super) live: bool,
    pub(super) find: Entity<InputState>,
    pub(super) editor: Entity<EditorState>,
    editor_value: String,
    editor_identity: Option<String>,
    level_value: String,
    list: ListState,
    focus: FocusHandle,
    selection: SelectableLog,
    rows: Rc<Vec<LogRow>>,
    source: Vec<JournalEntry>,
    ansi: crate::log_line::Ansi,
    dataset: Option<(LogTarget, Option<String>)>,
    unseen: usize,
    initialized: bool,
    _subscriptions: Vec<Subscription>,
}
impl Logs {
    #[cfg(test)]
    pub(super) fn is_following_tail(&self) -> bool {
        self.list.is_following_tail()
    }

    pub(super) fn new(window: &mut Window, cx: &mut Context<ServicesPanel>) -> Self {
        let find = cx.new(|cx| InputState::new(window, cx).placeholder("Search these lines"));
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .line_number(true)
                .searchable(true)
                .soft_wrap(true)
        });
        let find_sub = cx.subscribe(&find, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.logs.rebuild(cx, true);
                cx.notify();
            }
        });
        // Tail following controls scrolling; short output still starts at the top.
        let list = ListState::new(0, ListAlignment::Top, px(120.));
        list.set_follow_mode(FollowMode::Tail);
        list.set_scroll_handler(cx.listener(|this, event: &ListScrollEvent, _, cx| {
            // This callback runs while the list owns its state: use the event
            // instead of re-borrowing ListState in the scroll handler.
            if event.is_following_tail {
                this.logs.unseen = 0;
            }
            // The journal owns wheel gestures inside its viewport, including
            // at either boundary. Letting the same gesture bubble also scrolls
            // the surrounding service detail and hides its heading.
            cx.stop_propagation();
            cx.notify();
        }));
        let focus = cx.focus_handle();
        let selection = SelectableLog::new(focus.clone(), window, cx);
        Self {
            live: true,
            find,
            editor,
            editor_value: String::new(),
            editor_identity: None,
            level_value: "all".into(),
            list,
            focus,
            selection,
            rows: Rc::new(vec![]),
            source: vec![],
            ansi: Default::default(),
            dataset: None,
            unseen: 0,
            initialized: false,
            _subscriptions: vec![find_sub],
        }
    }
    pub(super) fn priority(&self) -> Option<String> {
        (self.level_value != "all").then(|| self.level_value.clone())
    }
    pub(super) fn restore_preferences(
        &mut self,
        level: String,
        live: bool,
        query: String,
        window: &mut Window,
        cx: &mut Context<ServicesPanel>,
    ) {
        self.level_value = match level.as_str() {
            "warning" | "err" => level,
            _ => "all".into(),
        };
        self.live = live;
        self.find
            .update(cx, |find, cx| find.set_value(query, window, cx));
    }
    pub(super) fn sync(&mut self, client: &Services, cx: &mut Context<ServicesPanel>) {
        let source = client
            .logs()
            .map(|p| p.entries.as_slice())
            .unwrap_or_default();
        if source == self.source {
            return;
        }
        let pinned = self.list.is_following_tail();
        let added = self
            .source
            .last()
            .and_then(|last| source.iter().rposition(|row| row == last))
            .map(|at| source.len() - at - 1)
            .unwrap_or(0);
        if self.initialized && !pinned {
            self.unseen += added;
        }
        self.source = source.to_vec();
        self.initialized = true;
        self.rebuild(cx, false);
    }
    pub(super) fn change_dataset(
        &mut self,
        target: Option<(LogTarget, Option<String>)>,
        cx: &mut App,
    ) {
        if self.dataset == target {
            return;
        }
        let different_target = self.dataset.as_ref().map(|p| &p.0) != target.as_ref().map(|p| &p.0);
        self.dataset = target;
        self.selection.clear(cx);
        self.unseen = 0;
        self.initialized = false;
        if different_target {
            self.source.clear();
            self.rows = Rc::new(vec![]);
            self.selection.set_rows(vec![], cx);
            self.list.reset(0);
            self.list.set_follow_mode(FollowMode::Tail);
        }
    }
    fn rebuild(&mut self, cx: &mut App, filter_changed: bool) {
        let rows = styled_rows(
            &self.source,
            self.find.read(cx).value().as_ref(),
            &mut self.ansi,
        );
        if rows == *self.rows {
            return;
        }
        // Keep the logical row anchor when the bounded journal drops its prefix.
        // Filtering is an explicit new view; appended entries preserve manual scroll.
        if filter_changed {
            self.list.reset(rows.len());
        } else {
            let anchor = self.list.logical_scroll_top();
            let pinned = self.list.is_following_tail();
            let preserved = self
                .rows
                .get(anchor.item_ix)
                .and_then(|row| rows.iter().position(|next| next == row));
            let count = self.list.item_count();
            if rows.len() >= count {
                self.list.splice(count..count, rows.len() - count);
            } else {
                self.list.splice(rows.len()..count, 0);
            }
            self.list.remeasure();
            if !pinned && let Some(item_ix) = preserved {
                self.list.scroll_to(ListOffset {
                    item_ix,
                    offset_in_item: anchor.offset_in_item,
                });
            }
        }
        self.selection.set_rows(
            rows.iter()
                .map(|row| match row {
                    LogRow::Day { .. } => None,
                    LogRow::Line(line, time, _) => {
                        Some(format!("{} {}{}", time, source(line), line.message))
                    }
                })
                .collect(),
            cx,
        );
        self.rows = Rc::new(rows);
    }
    pub(super) fn sync_editor(
        &mut self,
        client: &Services,
        identity: Option<String>,
        window: &mut Window,
        cx: &mut Context<ServicesPanel>,
    ) {
        let text = client
            .unit_file()
            .map(|file| file.text.as_str())
            .unwrap_or_default();
        if self.editor_identity != identity {
            self.editor_identity = identity;
            self.editor_value = text.into();
            self.editor = cx.new(|cx| {
                EditorState::new(window, cx)
                    .line_number(true)
                    .searchable(true)
                    .soft_wrap(true)
                    .default_value(text.to_owned())
            });
        } else if self.editor_value != text {
            self.editor_value = text.into();
            self.editor.update(cx, |editor, cx| {
                editor.set_value(text.to_owned(), window, cx)
            });
        }
    }
}
impl ServicesPanel {
    fn navigate_logs(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.modifiers.modified() {
            return;
        }
        let list = &self.logs.list;
        let page = (list.viewport_bounds().size.height - px(24.)).max(px(24.));
        match event.keystroke.key.as_str() {
            "home" => list.scroll_to(ListOffset {
                item_ix: 0,
                offset_in_item: px(0.),
            }),
            "end" => list.set_follow_mode(FollowMode::Tail),
            key @ ("up" | "down" | "pageup" | "pagedown") => {
                let distance = match key {
                    "up" => px(24.),
                    "down" => px(-24.),
                    "pageup" => page,
                    _ => -page,
                };
                // Tail mode uses an end sentinel rather than a viewport-top
                // offset. Clamp it before applying keyboard scroll distance.
                let current = list
                    .scroll_px_offset_for_scrollbar()
                    .y
                    .max(-list.max_offset_for_scrollbar().y);
                list.set_offset_from_scrollbar(point(px(0.), current + distance));
            }
            _ => return,
        }
        if list.is_following_tail() {
            self.logs.unseen = 0;
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn log_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::connections_selector::{Choice, Selector};
        let owner = cx.weak_entity();
        div()
            .flex()
            .flex_wrap()
            .min_w_0()
            .justify_end()
            .items_center()
            .gap(px(8.))
            .child(
                crate::controls::switch("services-live")
                    .label("Live")
                    .accessibility_label("Follow new lines")
                    .checked(self.logs.live)
                    .on_click(cx.listener(|this, live, _, cx| {
                        this.logs.live = *live;
                        this.sync_selection(cx);
                        cx.notify();
                    })),
            )
            .child(Selector::new(
                "services-log-level",
                "Log level",
                match self.logs.level_value.as_str() {
                    "warning" => 1,
                    "err" => 2,
                    _ => 0,
                },
                vec![
                    Choice::text(0, "All"),
                    Choice::text(1, "Warnings"),
                    Choice::text(2, "Errors"),
                ],
                move |index, _, cx| {
                    let _ = owner.update(cx, |this, cx| {
                        this.logs.level_value = ["all", "warning", "err"][index].into();
                        this.sync_selection(cx);
                        cx.notify();
                    });
                },
            ))
            .child(
                div()
                    .w(px(170.))
                    .flex_shrink_0()
                    .h(px(crate::controls::CONTROL_HEIGHT))
                    .child(
                        crate::controls::input(&self.logs.find)
                            .prefix(
                                Icon::new(IconName::Search).size(px(crate::controls::ICON_SIZE)),
                            )
                            .aria_label("Search logs"),
                    ),
            )
            .into_any_element()
    }
    pub(super) fn log_view(&self, label: &str, cx: &mut Context<Self>) -> AnyElement {
        let resource = self.client.resource("logs");
        let pending = self.client.logs().is_none() && resource.is_none_or(|r| r.error.is_none());
        let placeholder = resource.is_some_and(|r| r.value.is_null() && r.pending)
            && self.client.logs().is_some();
        let rows = self.logs.rows.clone();
        let query = self.logs.find.read(cx).value().to_string();
        let count = rows
            .iter()
            .filter(|r| matches!(r, LogRow::Line(..)))
            .count();
        let total = self.logs.source.len();
        let pinned = self.logs.list.is_following_tail();
        let live = self.logs.live;
        let days = Rc::new(
            rows.iter()
                .enumerate()
                .filter_map(|(index, row)| match row {
                    LogRow::Day { label, .. } => Some((index, label.clone())),
                    _ => None,
                })
                .collect(),
        );
        // Row bounds come from this frame's virtual-list prepaint, including
        // follow-tail layout where ListState's logical offset is the end sentinel.
        let row_bounds = Rc::new(RefCell::new(Vec::new()));
        let sticky_bounds = row_bounds.clone();
        let footer = if pending {
            "Reading the journal…".into()
        } else if placeholder {
            "Filtering…".into()
        } else {
            format!(
                "{}{}",
                if query.trim().is_empty() {
                    format!("{total} lines")
                } else {
                    format!("{count} of {total} lines")
                },
                if !self.logs.live {
                    " · paused"
                } else if pinned {
                    " · following new lines"
                } else {
                    " · scrolled up; new lines wait below"
                }
            )
        };
        let empty = if !query.trim().is_empty() {
            "No lines match."
        } else {
            match self.logs.level_value.as_str() {
                "err" => "No errors in the latest lines.",
                "warning" => "No warnings or errors in the latest lines.",
                _ => "No accessible journal entries.",
            }
        };
        div()
            .id("services-logs")
            .debug_selector(|| "services-logs".into())
            .flex_1()
            .min_h(px(300.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .when_some(self.error("logs"), |v, error| {
                v.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(alert("services-logs-error", error, cx).flex_1())
                        .child(
                            crate::controls::button("services-logs-retry")
                                .label("Retry logs")
                                .disabled(self.pending("logs"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.client.retry_logs();
                                    cx.notify();
                                })),
                        ),
                )
            })
            .child(
                div()
                    .id("services-log-region")
                    .debug_selector(|| "services-log-region".into())
                    .role(accesskit::Role::Log)
                    .aria_label(format!("{label} output"))
                    .a11y_synthetic_children(move |tree| {
                        if pending || placeholder {
                            tree.parent_node().set_busy();
                        }
                        tree.parent_node().set_live(if live {
                            accesskit::Live::Polite
                        } else {
                            accesskit::Live::Off
                        });
                    })
                    .track_focus(&self.logs.focus)
                    .on_key_down(cx.listener(Self::navigate_logs))
                    .relative()
                    .min_h(px(260.))
                    .flex_1()
                    .border_1()
                    .border_color(cx.theme().border)
                    .focus_visible(|style| style.border_color(cx.theme().ring))
                    .rounded(px(crate::ui::RADIUS))
                    .bg(cx.theme().muted.opacity(0.35))
                    .overflow_hidden()
                    .when(pending, |v| v.child(loading::lines("journal", 12)))
                    .when(
                        !pending && rows.is_empty() && self.error("logs").is_none(),
                        |v| v.child(crate::empty::compact(empty, cx)),
                    )
                    .when(!pending && !rows.is_empty(), |v| {
                        v.child(
                            self.logs.selection.viewport(
                                list(
                                    self.logs.list.clone(),
                                    cx.processor(move |this, index: usize, _, cx| {
                                        log_line(
                                            index,
                                            &rows[index],
                                            &query,
                                            row_bounds.clone(),
                                            &this.logs.selection,
                                            cx,
                                        )
                                    }),
                                )
                                .size_full()
                                .px(px(10.))
                                .py(px(8.))
                                .when(placeholder, |v| v.opacity(0.55)),
                            ),
                        )
                    })
                    .when(!pending, |v| {
                        v.child(crate::log_line::sticky_day_context(
                            "services-sticky-log-day",
                            days,
                            sticky_bounds,
                            placeholder,
                        ))
                    })
                    .when(self.logs.unseen > 0 && !pinned, |v| {
                        v.child(
                            div().absolute().bottom(px(12.)).left(px(12.)).child(
                                crate::controls::button("services-log-latest")
                                    .label(format!("{} new lines", self.logs.unseen))
                                    .icon(IconName::ArrowDown)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.logs.list.set_follow_mode(FollowMode::Tail);
                                        this.logs.unseen = 0;
                                        cx.notify();
                                    })),
                            ),
                        )
                    })
                    .on_scroll_wheel(|_, _, cx| cx.stop_propagation()),
            )
            .child(
                div()
                    .id("services-log-footer")
                    .debug_selector(|| "services-log-footer".into())
                    .flex_shrink_0()
                    .child(
                        crate::controls::pane_status_bar(cx)
                            .left(
                                quiet(footer.clone(), cx)
                                    .text_size(px(crate::ui::FONT_SIZE))
                                    .id("services-log-status")
                                    .aria_label(footer)
                                    .role(accesskit::Role::Status),
                            )
                            .right(
                                crate::controls::compact_icon_button("services-copy-logs")
                                    .icon(IconName::Copy)
                                    .ghost()
                                    .accessibility_label("Copy visible logs")
                                    .disabled(count == 0 || placeholder)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let text = this
                                            .logs
                                            .rows
                                            .iter()
                                            .map(|row| match row {
                                                LogRow::Day { label, .. } => label.clone(),
                                                LogRow::Line(line, time, _) => {
                                                    format!(
                                                        "{} {}{}",
                                                        time,
                                                        source(line),
                                                        line.message
                                                    )
                                                }
                                            })
                                            .collect::<Vec<_>>()
                                            .join("\n");
                                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}
#[cfg(test)]
fn log_rows(lines: &[JournalEntry], query: &str) -> Vec<LogRow> {
    styled_rows(lines, query, &mut Default::default())
}
fn styled_rows(
    lines: &[JournalEntry],
    query: &str,
    ansi: &mut crate::log_line::Ansi,
) -> Vec<LogRow> {
    let needle = query.trim().to_lowercase();
    let stamped: Vec<_> = lines
        .iter()
        .filter_map(|line| line.at.and_then(crate::localized_time::journal_day_key))
        .collect();
    let spans_days = stamped
        .first()
        .zip(stamped.last())
        .is_some_and(|(a, b)| a != b);
    let mut previous = None;
    let mut rows = vec![];
    for (line, styles) in lines
        .iter()
        .map(|line| {
            let mut line = line.clone();
            let styles = ansi.strip(&mut line.message);
            (line, styles)
        })
        .filter(|(line, _)| {
            format!("{} {}", source(line), line.message)
                .to_lowercase()
                .contains(&needle)
        })
    {
        if spans_days
            && let Some(at) = line.at
            && let Some(key) = crate::localized_time::journal_day_key(at)
            && previous.as_ref() != Some(&key)
        {
            rows.push(LogRow::Day {
                key: key.clone(),
                label: day(at),
            });
            previous = Some(key);
        }
        let time = timestamp(line.at);
        rows.push(LogRow::Line(line, time, styles));
    }
    rows
}
fn day(at: i64) -> String {
    crate::localized_time::journal_day(at)
}
fn timestamp(at: Option<i64>) -> String {
    at.map(crate::localized_time::journal_time)
        .unwrap_or("—".into())
}
fn source(line: &JournalEntry) -> String {
    line.identifier
        .as_ref()
        .map(|id| {
            format!(
                "{id}{}: ",
                line.pid.map(|pid| format!("[{pid}]")).unwrap_or_default()
            )
        })
        .unwrap_or_default()
}
fn log_line(
    index: usize,
    row: &LogRow,
    query: &str,
    geometry: RowBounds,
    selection: &SelectableLog,
    cx: &App,
) -> AnyElement {
    let content = match row {
        LogRow::Day { label, .. } => day_strip(label, cx)
            .id(("services-log-day", index))
            .debug_selector(move || format!("services-log-day-{index}"))
            .role(accesskit::Role::Heading)
            .aria_level(3)
            .aria_label(label.clone())
            .mx(px(-10.))
            .my(px(4.))
            .when(index == 0, |v| v.mt_0())
            .into_any_element(),
        LogRow::Line(line, time, styles) => {
            let prefix = source(line);
            crate::log_line::render(
                "services-log",
                index,
                LogLine {
                    at: line.at,
                    time: Some(time),
                    source: &prefix,
                    message: &line.message,
                    styles: styles.as_deref(),
                    tone: match domain::priority_tone(line.priority) {
                        Tone::Danger => LogTone::Error,
                        Tone::Warning => LogTone::Warning,
                        _ => LogTone::Normal,
                    },
                },
                query,
                selection,
                cx,
            )
        }
    };
    crate::log_line::track_row(index, content, geometry)
}

#[cfg(test)]
mod tests {
    use super::{LogRow, log_rows};
    use gpui::{ListOffset, ScrollDelta, ScrollWheelEvent, TestAppContext, point, px};
    use gpui_kit as gpui;
    use newport_domain::services::JournalEntry;
    fn lines() -> Vec<JournalEntry> {
        (0..24)
            .map(|i| JournalEntry {
                at: Some(1_700_000_000_000 + i / 12 * 86_400_000),
                priority: Some(6),
                pid: Some(31),
                identifier: Some("api".into()),
                message: format!("entry {i}"),
            })
            .collect()
    }
    #[test]
    fn journal_search_includes_source_pid_and_day_boundaries() {
        let lines = lines();
        let rows = log_rows(&lines, "");
        assert_eq!(
            rows.iter()
                .filter(|r| matches!(r, LogRow::Day { .. }))
                .count(),
            2
        );
        assert_eq!(
            log_rows(&lines, "api[31]")
                .iter()
                .filter(|r| matches!(r, LogRow::Line(..)))
                .count(),
            24
        );
        assert_eq!(
            log_rows(&lines, "entry 23")
                .iter()
                .filter(|r| matches!(r, LogRow::Line(..)))
                .count(),
            1
        );
        assert!(log_rows(&lines, "missing").is_empty());
        assert!(
            !log_rows(&lines[..12], "")
                .iter()
                .any(|row| matches!(row, LogRow::Day { .. })),
            "one-day journals need no date dividers"
        );
        let undated = vec![
            JournalEntry {
                at: None,
                ..Default::default()
            },
            JournalEntry {
                at: Some(i64::MAX),
                ..Default::default()
            },
        ];
        assert!(
            !log_rows(&undated, "")
                .iter()
                .any(|row| matches!(row, LogRow::Day { .. })),
            "unknown or invalid timestamps must not invent a date"
        );
    }
    #[gpui::test]
    fn short_journal_starts_at_top_while_following(cx: &mut TestAppContext) {
        use crate::services::tests::{draw, fixture};
        let (panel, cx, _backend) = fixture(cx);
        panel.update(cx, |panel, cx| {
            panel.logs.source = lines()[..3].to_vec();
            panel.logs.rebuild(cx, true);
            cx.notify();
        });
        for height in [1000., 1200.] {
            cx.simulate_resize(gpui::size(px(980.), px(height)));
            draw(cx);
            let viewport = cx.debug_bounds("services-log-region").unwrap();
            let first = panel.read_with(cx, |p, _| p.logs.selection.text_position(0, 0).unwrap());
            assert!((first.y - viewport.top() - px(9.)).abs() < px(3.));
            assert!(panel.read_with(cx, |p, _| p.logs.list.is_following_tail()));
        }
    }

    #[gpui::test]
    fn journal_keyboard_scroll_is_contained_and_restores_following(cx: &mut TestAppContext) {
        use crate::services::tests::{draw, fixture};
        use gpui::{KeyDownEvent, Keystroke};
        let (panel, cx, _backend) = fixture(cx);
        panel.update_in(cx, |panel, window, cx| {
            panel.logs.source = (0..240)
                .map(|index| JournalEntry {
                    message: format!("entry {index}"),
                    ..Default::default()
                })
                .collect();
            panel.logs.rebuild(cx, true);
            panel.logs.list.set_follow_mode(gpui::FollowMode::Tail);
            panel.logs.focus.focus(window, cx);
            cx.notify();
        });
        draw(cx);
        let outer = panel.read_with(cx, |panel, _| panel.detail_scroll.offset());
        let key = |key: &str, cx: &mut gpui::VisualTestContext| {
            cx.simulate_event(KeyDownEvent {
                physical_key: None,
                key_metadata: None,
                keystroke: Keystroke::parse(key).unwrap(),
                is_held: false,
                prefer_character_input: false,
            });
            draw(cx);
        };
        let offset = |cx: &gpui::VisualTestContext| {
            panel.read_with(cx, |panel, _| panel.logs.list.logical_scroll_top())
        };
        let moved_up = |before: ListOffset, after: ListOffset| {
            after.item_ix < before.item_ix
                || (after.item_ix == before.item_ix && after.offset_in_item < before.offset_in_item)
        };
        key("up", cx);
        assert!(panel.read_with(cx, |panel, _| !panel.logs.list.is_following_tail()));
        let first = offset(cx);
        key("pageup", cx);
        let paged = offset(cx);
        assert!(moved_up(first, paged));
        key("up", cx);
        assert!(moved_up(paged, offset(cx)));
        key("down", cx);
        key("pagedown", cx);
        assert!(moved_up(offset(cx), paged));
        key("home", cx);
        assert_eq!(offset(cx).item_ix, 0);
        assert_eq!(offset(cx).offset_in_item, px(0.));
        panel.update(cx, |panel, cx| {
            panel.logs.unseen = 8;
            cx.notify();
        });
        key("end", cx);
        assert!(panel.read_with(cx, |panel, _| panel.logs.list.is_following_tail()));
        assert_eq!(panel.read_with(cx, |panel, _| panel.logs.unseen), 0);
        assert_eq!(
            panel.read_with(cx, |panel, _| panel.detail_scroll.offset()),
            outer
        );

        // The log handler must leave shortcuts and the search field's caret
        // navigation alone, even though both controls share the Services view.
        let tail = offset(cx);
        key("shift-home", cx);
        assert_eq!(offset(cx).item_ix, tail.item_ix);
        assert_eq!(offset(cx).offset_in_item, tail.offset_in_item);
        panel.update_in(cx, |panel, window, cx| {
            panel
                .logs
                .find
                .update(cx, |find, cx| find.focus(window, cx));
        });
        key("home", cx);
        assert_eq!(offset(cx).item_ix, tail.item_ix);
        assert_eq!(offset(cx).offset_in_item, tail.offset_in_item);
    }

    #[gpui::test]
    fn bounded_log_eviction_preserves_scrolled_line_when_day_header_survives(
        cx: &mut TestAppContext,
    ) {
        let (panel, cx, _backend) = crate::services::tests::fixture(cx);
        panel.update(cx, |panel, cx| {
            panel.logs.source = lines();
            panel.logs.rebuild(cx, true);
            panel.logs.list.scroll_to(ListOffset {
                item_ix: 8,
                offset_in_item: px(3.),
            });
            let before = panel.logs.rows[8].clone();
            panel.logs.source.drain(0..5);
            panel.logs.rebuild(cx, false);
            let offset = panel.logs.list.logical_scroll_top();
            assert!(panel.logs.rows[offset.item_ix] == before);
            assert_eq!(offset.offset_in_item, px(3.));
            assert!(!panel.logs.list.is_following_tail());
        });
    }

    #[gpui::test]
    fn sticky_days_follow_scroll_tail_filters_and_undated_lines(cx: &mut TestAppContext) {
        use crate::services::tests::{draw, fixture};
        let (panel, cx, _backend) = fixture(cx);
        let mut entries = lines();
        entries.extend((24..160).map(|i| JournalEntry {
            at: Some(1_700_000_000_000 + i / 80 * 86_400_000),
            message: format!("entry {i}"),
            ..Default::default()
        }));
        // Two chronological dates, followed by an undated continuation.
        for (i, entry) in entries.iter_mut().enumerate() {
            entry.at = if !(5..150).contains(&i) {
                None
            } else {
                Some(1_700_000_000_000 + i as i64 / 80 * 86_400_000)
            };
        }
        let first = super::day(entries[5].at.unwrap());
        let second = super::day(entries[80].at.unwrap());
        // GPUI's test selector API requires static names.
        let first_selector: &'static str =
            Box::leak(format!("services-sticky-log-day-{first}").into_boxed_str());
        let second_selector: &'static str =
            Box::leak(format!("services-sticky-log-day-{second}").into_boxed_str());
        panel.update(cx, |panel, cx| {
            panel.logs.source = entries;
            panel.logs.rebuild(cx, true);
            panel.logs.list.scroll_to(ListOffset {
                item_ix: 0,
                offset_in_item: px(0.),
            });
            cx.notify();
        });
        draw(cx);
        assert!(
            cx.debug_bounds("services-sticky-log-day").is_none(),
            "undated leading lines must not borrow a future date"
        );

        panel.update(cx, |panel, cx| {
            panel.logs.list.scroll_to(ListOffset {
                item_ix: 30,
                offset_in_item: px(0.),
            });
            cx.notify();
        });
        draw(cx);
        let region = cx.debug_bounds("services-log-region").unwrap();
        let sticky = cx.debug_bounds(first_selector).unwrap();
        assert!((sticky.top() - region.top()).abs() <= px(1.));
        assert!(sticky.size.height < px(30.));
        let before = panel.read_with(cx, |panel, _| panel.logs.list.logical_scroll_top());
        // Scroll directly over the repeated date. It is decorative and must
        // leave wheel events with the existing virtual list.
        cx.simulate_event(ScrollWheelEvent {
            position: sticky.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(-80.))),
            ..Default::default()
        });
        draw(cx);
        let after = panel.read_with(cx, |panel, _| panel.logs.list.logical_scroll_top());
        assert!(after.item_ix > before.item_ix || after.offset_in_item > before.offset_in_item);

        panel.update(cx, |panel, cx| {
            panel.logs.list.scroll_to(ListOffset {
                item_ix: 80,
                offset_in_item: px(10.),
            });
            cx.notify();
        });
        draw(cx);
        let previous = cx.debug_bounds(first_selector).unwrap();
        let next = cx.debug_bounds("services-log-day-81").unwrap();
        assert!(
            previous.bottom() <= next.top(),
            "the next date must displace, not overlap, the sticky context: previous={previous:?}, next={next:?}"
        );

        panel.update(cx, |panel, cx| {
            panel.logs.list.set_follow_mode(gpui::FollowMode::Tail);
            cx.notify();
        });
        draw(cx);
        assert!(cx.debug_bounds(second_selector).is_some());
        assert!(cx.debug_bounds(first_selector).is_none());

        panel.update_in(cx, |panel, window, cx| {
            panel
                .logs
                .find
                .update(cx, |find, cx| find.set_value("entry 4", window, cx));
            panel.logs.rebuild(cx, true);
            cx.notify();
        });
        draw(cx);
        assert!(
            cx.debug_bounds(second_selector).is_none(),
            "a local filter must not retain the previous date context"
        );
        panel.update_in(cx, |panel, window, cx| {
            panel
                .logs
                .find
                .update(cx, |find, cx| find.set_value("missing", window, cx));
            panel.logs.rebuild(cx, true);
            cx.notify();
        });
        draw(cx);
        assert!(cx.debug_bounds("services-sticky-log-day").is_none());
    }

    #[gpui::test]
    fn services_log_drag_copies_literal_columns_and_filter_clears_selection(
        cx: &mut TestAppContext,
    ) {
        use crate::services::tests::{draw, fixture};
        use gpui::{Modifiers, MouseButton};
        let (panel, cx, _backend) = fixture(cx);
        let entries = vec![
            JournalEntry {
                at: Some(1_700_000_000_000),
                identifier: Some("api".into()),
                pid: Some(31),
                message: "Café **literal** <tag> 🦀".into(),
                ..Default::default()
            },
            JournalEntry {
                at: Some(1_700_000_001_000),
                identifier: Some("服务".into()),
                pid: Some(2),
                message: "Café second e\u{301} tail".into(),
                ..Default::default()
            },
        ];
        let literal: Vec<_> = entries
            .iter()
            .map(|line| {
                format!(
                    "{} {}{}",
                    super::timestamp(line.at),
                    super::source(line),
                    line.message
                )
            })
            .collect();
        let start = literal[0].find("Café").unwrap();
        let end = literal[1].find(" tail").unwrap();
        panel.update_in(cx, |panel, window, cx| {
            panel.logs.source = entries;
            panel
                .logs
                .find
                .update(cx, |find, cx| find.set_value("Café", window, cx));
            panel.logs.rebuild(cx, true);
            cx.notify();
        });
        draw(cx);
        let (from, to) = panel.read_with(cx, |panel, _| {
            (
                panel.logs.selection.text_position(0, start).unwrap() + point(px(0.1), px(5.)),
                panel.logs.selection.text_position(1, end).unwrap() + point(px(0.1), px(5.)),
            )
        });
        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
        cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        draw(cx);
        let expected = format!("{}\n{}", &literal[0][start..], &literal[1][..end]);
        assert_eq!(
            cx.update(gpui_kit::base::TextSelection::selected_text),
            expected
        );
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-c"
        } else {
            "ctrl-c"
        });
        assert_eq!(
            cx.update(|_, cx| cx
                .read_from_clipboard()
                .and_then(|item| item.text())
                .unwrap_or_default()),
            expected
        );
        panel.update_in(cx, |panel, window, cx| {
            panel
                .logs
                .find
                .update(cx, |find, cx| find.set_value("missing", window, cx));
            panel.logs.rebuild(cx, true);
            cx.notify();
        });
        draw(cx);
        assert!(
            cx.update(gpui_kit::base::TextSelection::selected_text)
                .is_empty()
        );
    }
}
