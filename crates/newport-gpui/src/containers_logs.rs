//! Container log presentation: native controls, virtual wrapped rows and selection.
//! Domain severity/search and bounded log sequencing live in the shared crates.
use super::*;
use crate::{log_line, selectable_log::SelectableLog};
use newport_client::containers::LogWindow;
use newport_domain::{
    containers::ContainerLogEntry,
    containers_log::{Filter, Level, Severity},
};

pub(super) fn new_lines_button(id: &'static str, count: u64, cx: &App) -> Button {
    let mut content = div().text_size(px(crate::ui::FONT_SIZE)).gap(px(5.));
    crate::controls::button(id)
        .label(format!(
            "{count} new {}",
            if count == 1 { "line" } else { "lines" }
        ))
        .icon(IconName::ArrowDown)
        .content_style(
            content.style().clone(),
            gpui_kit::component::Size::Size(px(12.)),
        )
        .rounded(px(crate::ui::RADIUS))
        .px(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .bg(cx.theme().background)
        .border_color(cx.theme().border)
        .shadow_md()
}

#[derive(Clone, PartialEq)]
enum Row {
    Day {
        key: String,
        label: String,
    },
    Line {
        sequence: u64,
        entry: ContainerLogEntry,
        time: String,
        source: String,
        styles: log_line::Styles,
        tone: log_line::Tone,
    },
}
impl Row {
    fn same_anchor(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Day { key: a, .. }, Self::Day { key: b, .. }) => a == b,
            (Self::Line { sequence: a, .. }, Self::Line { sequence: b, .. }) => a == b,
            _ => false,
        }
    }
    fn text(&self) -> Option<String> {
        match self {
            Self::Day { .. } => None,
            Self::Line {
                entry,
                time,
                source,
                ..
            } => Some(format!("{time} {source}{}", entry.message)),
        }
    }
}

pub(super) struct Logs {
    pub(super) level: Level,
    pub(super) find: Entity<InputState>,
    list: ListState,
    focus: FocusHandle,
    selection: SelectableLog,
    rows: Rc<Vec<Row>>,
    /// Retained entries with escapes stripped; `styles` holds their colours.
    source: Vec<ContainerLogEntry>,
    styles: Vec<log_line::Styles>,
    ansi: log_line::Ansi,
    epoch: Option<u64>,
    first: u64,
    end: u64,
    unseen: u64,
    _subscription: Subscription,
}
impl Logs {
    pub(super) fn new(window: &mut Window, cx: &mut Context<ContainersPanel>) -> Self {
        let find = cx.new(|cx| InputState::new(window, cx).placeholder("Search these lines"));
        let subscription = cx.subscribe(&find, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.logs.rebuild(cx, true);
                cx.notify();
            }
        });
        // Tail following controls scrolling; short output still starts at the top.
        let list = ListState::new(0, ListAlignment::Top, px(120.));
        list.set_follow_mode(FollowMode::Tail);
        list.set_scroll_handler(cx.listener(|this, event: &ListScrollEvent, _, cx| {
            // ListState is already borrowed by its scroll handler.
            if event.is_following_tail {
                this.logs.unseen = 0;
            }
            cx.stop_propagation();
            cx.notify();
        }));
        let focus = cx.focus_handle();
        Self {
            selection: SelectableLog::new(focus.clone(), window, cx),
            find,
            list,
            focus,
            level: Level::All,
            rows: Rc::new(vec![]),
            source: vec![],
            styles: vec![],
            ansi: log_line::Ansi::default(),
            epoch: None,
            first: 0,
            end: 0,
            unseen: 0,
            _subscription: subscription,
        }
    }

    pub(super) fn sync(&mut self, window: Option<LogWindow<'_>>, cx: &mut App) {
        let epoch = window.as_ref().map(|w| w.epoch);
        if self.epoch != epoch {
            self.epoch = epoch;
            self.first = 0;
            self.end = 0;
            self.unseen = 0;
            self.source.clear();
            self.styles.clear();
            self.rows = Rc::new(vec![]);
            self.selection.clear(cx);
            self.selection.set_rows(vec![], cx);
            self.list.reset(0);
            self.list.set_follow_mode(FollowMode::Tail);
        }
        let Some(window) = window else {
            return;
        };
        if (self.first, self.end) == (window.first, window.end) {
            return;
        }
        // Sequence metadata counts arrivals even when the bounded cache evicts
        // equally many rows, or two separate entries have identical text/time.
        if self.end > 0 && !self.list.is_following_tail() {
            self.unseen += window.end.saturating_sub(self.end);
        }
        let evicted = window.first != self.first;
        self.first = window.first;
        self.end = window.end;
        self.source = window.entries.to_vec();
        let ansi = &mut self.ansi;
        self.styles = self
            .source
            .iter_mut()
            .map(|entry| ansi.strip(&mut entry.message))
            .collect();
        if evicted {
            self.selection.clear(cx);
        }
        self.rebuild(cx, false);
    }

    pub(super) fn rebuild(&mut self, cx: &mut App, filter_changed: bool) {
        let filter = Filter::new(self.level, self.find.read(cx).value().as_ref());
        let rows = rows(&self.source, &self.styles, self.first, &filter);
        if filter_changed {
            self.selection.clear(cx);
        }
        if rows == *self.rows {
            return;
        }
        let pinned = self.list.is_following_tail();
        let anchor = self.list.logical_scroll_top();
        let preserved = self
            .rows
            .get(anchor.item_ix)
            .and_then(|old| rows.iter().position(|row| row.same_anchor(old)));
        let count = self.list.item_count();
        if rows.len() >= count {
            self.list.splice(count..count, rows.len() - count);
        } else {
            self.list.splice(rows.len()..count, 0);
        }
        self.list.remeasure();
        if pinned {
            self.list.set_follow_mode(FollowMode::Tail);
        } else if let Some(item_ix) = preserved {
            self.list.scroll_to(ListOffset {
                item_ix,
                offset_in_item: anchor.offset_in_item,
            });
        } else {
            // A filtered/evicted anchor falls back to the first retained line,
            // never jumping a reader who scrolled up to the live tail.
            self.list.scroll_to(ListOffset {
                item_ix: 0,
                offset_in_item: px(0.),
            });
        }
        self.selection
            .set_rows(rows.iter().map(Row::text).collect(), cx);
        self.rows = Rc::new(rows);
    }
}

impl ContainersPanel {
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
            .id("container-log-toolbar")
            .debug_selector(|| "container-log-toolbar".into())
            .justify_end()
            .min_w_0()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .child(
                crate::controls::switch("container-live-logs")
                    .label("Live")
                    .accessibility_label("Follow new lines")
                    .checked(self.live)
                    .on_click(cx.listener(|this, value: &bool, _, cx| {
                        this.live = *value;
                        this.sync_selection(cx);
                        cx.notify();
                    })),
            )
            .child(Selector::new(
                "container-log-level",
                "Log level",
                match self.logs.level {
                    Level::All => 0,
                    Level::Warnings => 1,
                    Level::Errors => 2,
                },
                vec![
                    Choice::text(0, "All"),
                    Choice::text(1, "Warnings"),
                    Choice::text(2, "Errors"),
                ],
                move |index, _, cx| {
                    let _ = owner.update(cx, |this, cx| {
                        this.logs.level = [Level::All, Level::Warnings, Level::Errors][index];
                        this.logs.rebuild(cx, true);
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
                        crate::controls::search_input(&self.logs.find, cx)
                            .aria_label("Search logs"),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn logs_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let pending = self.client.logs().is_none() && self.error("container-logs").is_none();
        let rows = self.logs.rows.clone();
        let geometry = log_line::RowBounds::default();
        let row_geometry = geometry.clone();
        let days = Rc::new(
            rows.iter()
                .enumerate()
                .filter_map(|(index, row)| {
                    if let Row::Day { label, .. } = row {
                        Some((index, label.clone()))
                    } else {
                        None
                    }
                })
                .collect(),
        );
        let query = self.logs.find.read(cx).value().to_string();
        let filter = Filter::new(self.logs.level, &query);
        let count = rows
            .iter()
            .filter(|row| matches!(row, Row::Line { .. }))
            .count();
        let total = self.logs.source.len();
        let pinned = self.logs.list.is_following_tail();
        let live = self.live;
        let label = self
            .client
            .selected_container()
            .map(|c| format!("{} logs", domain::display_name(c)))
            .or_else(|| self.client.selected_project().map(|p| format!("{p} logs")))
            .unwrap_or_else(|| "Container log output".into());
        let footer = if pending {
            "Reading logs…".to_owned()
        } else {
            format!(
                "{} · {}",
                if filter.has_query() || self.logs.level != Level::All {
                    format!("{count} of {total} lines")
                } else {
                    format!("{total} lines")
                },
                if !live {
                    "paused"
                } else if pinned {
                    "following new lines"
                } else {
                    "scrolled up; new lines wait below"
                }
            )
        };
        let empty = if filter.has_query() {
            "No lines match."
        } else if total == 0 {
            "No log output yet."
        } else {
            match self.logs.level {
                Level::Errors => "No error lines in the latest output.",
                _ => "No warnings or errors in the latest output.",
            }
        };
        div()
            .id("container-logs")
            .debug_selector(|| "container-logs".into())
            .flex()
            .flex_col()
            .gap(px(8.))
            .min_h(px(260.))
            .flex_1()
            .when(self.error("container-logs").is_some(), |v| {
                v.child(self.status("container-logs", cx))
            })
            .child(
                div()
                    .id("container-log-viewport")
                    .debug_selector(|| "container-log-viewport".into())
                    .role(accesskit::Role::Log)
                    .aria_label(label)
                    .a11y_synthetic_children(move |tree| {
                        if pending {
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
                    .flex_1()
                    .min_h(px(200.))
                    .border_1()
                    .border_color(cx.theme().border)
                    .focus_visible(|style| crate::controls::inset_focus_ring(style, cx))
                    .rounded(px(crate::ui::RADIUS))
                    .bg(cx.theme().muted.opacity(0.35))
                    .overflow_hidden()
                    .when(pending, |v| v.child(loading::lines("container-logs", 12)))
                    .when(
                        !pending && rows.is_empty() && self.error("container-logs").is_none(),
                        |v| v.child(crate::empty::compact(empty, cx)),
                    )
                    .when(!pending && !rows.is_empty(), |v| {
                        v.child(
                            self.logs.selection.viewport(
                                list(
                                    self.logs.list.clone(),
                                    cx.processor(move |this, index: usize, _, cx| {
                                        let content = match &rows[index] {
                                            Row::Day { label, .. } => {
                                                log_line::day_strip(label, cx)
                                                    .id(("container-log-day", index))
                                                    .role(accesskit::Role::Heading)
                                                    .aria_level(3)
                                                    .aria_label(label.clone())
                                                    .mx(px(-10.))
                                                    .my(px(4.))
                                                    .when(index == 0, |v| v.mt_0())
                                                    .into_any_element()
                                            }
                                            Row::Line {
                                                entry,
                                                time,
                                                source,
                                                styles,
                                                tone,
                                                ..
                                            } => log_line::render(
                                                "container-log",
                                                index,
                                                log_line::LogLine {
                                                    at: entry.at,
                                                    time: Some(time),
                                                    source,
                                                    message: &entry.message,
                                                    styles: styles.as_deref(),
                                                    tone: *tone,
                                                },
                                                &query,
                                                &this.logs.selection,
                                                cx,
                                            ),
                                        };
                                        log_line::track_row(index, content, row_geometry.clone())
                                    }),
                                )
                                .size_full()
                                .px(px(10.))
                                .py(px(8.)),
                            ),
                        )
                    })
                    .when(!pending, |v| {
                        v.child(log_line::sticky_day_context(
                            "container-sticky-log-day",
                            days,
                            geometry,
                            false,
                        ))
                    })
                    .when(self.logs.unseen > 0 && !pinned, |v| {
                        v.child(
                            div()
                                .absolute()
                                .bottom(px(12.))
                                .left_0()
                                .right_0()
                                .flex()
                                .justify_center()
                                .child(
                                    new_lines_button("container-log-latest", self.logs.unseen, cx)
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
                    .id("container-log-footer")
                    .debug_selector(|| "container-log-footer".into())
                    .flex_shrink_0()
                    .child(
                        crate::controls::pane_status_bar(cx)
                            .left(
                                quiet(footer.clone(), cx)
                                    .text_size(px(crate::ui::FONT_SIZE))
                                    .id("container-log-status")
                                    .role(accesskit::Role::Status)
                                    .aria_label(footer),
                            )
                            .right(
                                crate::controls::compact_icon_button("container-log-copy")
                                    .ghost()
                                    .icon(IconName::Copy)
                                    .accessibility_label("Copy visible logs")
                                    .disabled(count == 0 || pending)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let text = this
                                            .logs
                                            .rows
                                            .iter()
                                            .filter_map(Row::text)
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

fn rows(
    source: &[ContainerLogEntry],
    styles: &[log_line::Styles],
    first: u64,
    filter: &Filter,
) -> Vec<Row> {
    let mut stamped = source
        .iter()
        .filter_map(|entry| entry.at.and_then(crate::localized_time::journal_day_key));
    let first_day = stamped.next();
    let spans_days = stamped
        .next_back()
        .zip(first_day)
        .is_some_and(|(last, first)| last != first);
    let mut previous = None;
    let mut result = vec![];
    for (index, entry) in source
        .iter()
        .enumerate()
        .filter(|(_, entry)| filter.matches(entry))
    {
        if spans_days
            && let Some(at) = entry.at
            && let Some(key) = crate::localized_time::journal_day_key(at)
            && previous.as_ref() != Some(&key)
        {
            result.push(Row::Day {
                key: key.clone(),
                label: crate::localized_time::journal_day(at),
            });
            previous = Some(key);
        }
        let tone = match newport_domain::containers_log::severity(&entry.message) {
            Severity::Normal => log_line::Tone::Normal,
            Severity::Warning => log_line::Tone::Warning,
            Severity::Error => log_line::Tone::Error,
        };
        result.push(Row::Line {
            sequence: first + index as u64,
            entry: entry.clone(),
            time: entry
                .at
                .map(crate::localized_time::journal_time)
                .unwrap_or_else(|| "—".into()),
            source: entry
                .container
                .as_ref()
                .map(|s| format!("{s}: "))
                .unwrap_or_default(),
            styles: styles.get(index).cloned().flatten(),
            tone,
        });
    }
    result
}

#[cfg(test)]
#[path = "containers_logs_tests.rs"]
mod tests;
