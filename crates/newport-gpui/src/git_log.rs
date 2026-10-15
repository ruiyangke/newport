//! Bounded session log consumption; only events for the exact repository are displayed.
use super::*;
use newport_gpui::git_log_text::{LogLine, LogRecord, plain_text, render_styled_at};
use std::sync::Arc;
#[derive(Default)]
pub(super) struct Log {
    scope: String,
    cursor: u64,
    pub open: bool,
    follow: bool,
    last_poll: Option<Instant>,
    records: std::collections::VecDeque<Arc<LogRecord>>,
    rows: std::sync::Arc<Vec<LogLine>>,
    scroll: UniformListScrollHandle,
    gap: bool,
    columns: u16,
    rendered_columns: u16,
    revision: u64,
    reflow: Option<Task<()>>,
    reflow_error: Option<String>,
    pub view: git_log_view::Cache,
}
impl Log {
    fn columns(&self) -> u16 {
        if self.columns == 0 { 160 } else { self.columns }
    }
    fn invalidate_reflow(&mut self) {
        self.revision += 1;
        self.reflow = None;
        self.reflow_error = None;
    }
    fn accepts_reflow(&self, scope: &str, revision: u64, columns: u16) -> bool {
        self.scope == scope && self.revision == revision && self.columns() == columns
    }
}
impl Newport {
    pub(super) fn sync_git_log(&mut self, cx: &mut Context<Self>) {
        if self.page != Page::Projects || self.git_repository_id().is_empty() {
            return;
        }
        let scope = json!([
            self.server,
            self.git_repository_id(),
            self.snapshot["instanceId"]
        ])
        .to_string();
        if self.git_log.scope != scope {
            self.resources.invalidate("git-log-events");
            self.git_log.view.clear(cx);
            self.git_log = Log {
                scope,
                follow: true,
                ..Default::default()
            };
        }
        if self
            .git_log
            .last_poll
            .is_none_or(|t| t.elapsed() >= Duration::from_millis(500))
            && !self
                .resources
                .get("git-log-events")
                .is_some_and(|r| r.pending)
        {
            self.git_log.last_poll = Some(Instant::now());
            self.request(
                "git-log-events",
                "native.git_logs",
                json!({"after": self.git_log.cursor,"serverId":self.server,"repoId":self.git_repository_id(),"columns":self.git_log.columns()}),
            );
        }
    }
    pub(super) fn git_log_completed(&mut self, cx: &mut Context<Self>) {
        if self
            .resources
            .get("git-log-events")
            .is_some_and(|r| r.error.is_some())
        {
            return;
        }
        let value = self.value("git-log-events");
        let Some(cursor) = value["cursor"].as_u64() else {
            return;
        };
        let Some(event_count) = value["eventCount"].as_u64() else {
            return;
        };
        if cursor < self.git_log.cursor {
            self.git_log.cursor = 0;
            return;
        }
        if cursor == self.git_log.cursor {
            return;
        }
        self.git_log.gap |=
            cursor.saturating_sub(self.git_log.cursor) > event_count || value["omitted"] == true;
        self.git_log.cursor = cursor;
        if value["records"].as_array().is_none_or(Vec::is_empty) {
            return;
        }
        let was_empty = self.git_log.records.is_empty();
        if let Some(records) = value["records"].as_array() {
            for record in records {
                if let Ok(record) = serde_json::from_value::<LogRecord>(record.clone()) {
                    self.git_log.records.push_back(Arc::new(record));
                } else {
                    self.git_log.gap = true;
                }
            }
        }
        let columns = self.git_log.columns();
        if value["columns"].as_u64() != Some(u64::from(columns))
            || (!was_empty && self.git_log.rendered_columns != columns)
        {
            self.git_log_reflow(cx);
        } else {
            self.git_log.invalidate_reflow();
            self.git_log.rendered_columns = columns;
            self.git_log_rebuild_rows();
        }
    }
    fn git_log_rebuild_rows(&mut self) {
        while self.git_log.records.len() > 100
            || self
                .git_log
                .records
                .iter()
                .map(|record| record.lines.len())
                .sum::<usize>()
                > 20_000
        {
            self.git_log.records.pop_front();
        }
        self.git_log.rows = std::sync::Arc::new(
            self.git_log
                .records
                .iter()
                .flat_map(|record| {
                    record
                        .lines
                        .iter()
                        .cloned()
                        .chain(std::iter::once(LogLine::default()))
                })
                .collect(),
        );
        self.git_log_follow_tail();
    }
    fn git_log_reflow(&mut self, cx: &mut Context<Self>) {
        self.git_log.invalidate_reflow();
        // Bound raw storage even while a resize worker is still running.
        while self.git_log.records.len() > 100 {
            self.git_log.records.pop_front();
        }
        let records = self.git_log.records.clone();
        let scope = self.git_log.scope.clone();
        let revision = self.git_log.revision;
        let columns = self.git_log.columns();
        let executor = cx.background_executor().clone();
        self.git_log.reflow = Some(cx.spawn(async move |owner, cx| {
            executor.timer(Duration::from_millis(100)).await;
            let result = executor
                .spawn(async move {
                    records
                        .into_iter()
                        .map(|record| {
                            let lines = render_styled_at(&record.source, columns)?;
                            Ok(Arc::new(LogRecord {
                                source: record.source.clone(),
                                lines,
                            }))
                        })
                        .collect::<Result<std::collections::VecDeque<_>, String>>()
                })
                .await;
            let _ = owner.update(cx, |this, cx| {
                if !this.git_log.accepts_reflow(&scope, revision, columns) {
                    return;
                }
                this.git_log.reflow = None;
                match result {
                    Ok(records) => {
                        this.git_log.records = records;
                        this.git_log.rendered_columns = columns;
                        this.git_log_rebuild_rows();
                    }
                    Err(error) => this.git_log.reflow_error = Some(error),
                }
                cx.notify();
            });
        }));
    }
    pub(super) fn git_log_follow_tail(&self) {
        if self.git_log.follow && self.git_log.open {
            self.git_log.scroll.scroll_to_item(
                self.git_log.rows.len().saturating_sub(1),
                ScrollStrategy::Bottom,
            );
        }
    }
    pub(super) fn sync_git_log_width(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.git_log.open {
            return;
        }
        let columns = git_log_view::columns(self.content_width, window, cx);
        if self.git_log.columns != columns {
            self.git_log.columns = columns;
            self.git_log_reflow(cx);
        }
    }
    pub(super) fn git_log_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.git_log.open = false;
        self.git_log.view.clear(cx);
        self.git_log.view.trigger(cx).focus(window, cx);
        cx.notify();
    }
    pub(super) fn git_log_panel(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if !self.git_log.open {
            return div().into_any_element();
        }
        let rows = self.git_log.rows.clone();
        let document = self.git_log.view.prepare(&rows, window, cx);
        let focus = document.borrow().focus.clone();
        let lines = document.clone();
        div()
            .id("git-output-panel")
            .role(accesskit::Role::Log)
            .aria_label("Git command log")
            .track_focus(&focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.git_log_close(window, cx);
                }
            }))
            .h(px(320.).min(window.viewport_size().height * 0.45))
            .min_h_0()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .h(px(36.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .flex_shrink_0()
                            .child("Git output"),
                    )
                    .child(
                        label("Latest 100 commands · this session")
                            .text_size(px(11.))
                            .min_w_0()
                            .flex_1()
                            .truncate(),
                    )
                    .child(
                        crate::controls::compact_button("git-log-follow")
                            .label(if self.git_log.follow {
                                "Auto-scroll on"
                            } else {
                                "Auto-scroll off"
                            })
                            .toggled(self.git_log.follow)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.git_log.follow = !this.git_log.follow;
                                this.git_log_follow_tail();
                                cx.notify();
                            })),
                    )
                    .child(
                        crate::controls::compact_icon_button("git-log-copy")
                            .icon(IconName::Copy)
                            .ghost()
                            .accessibility_label("Copy Git log")
                            .tooltip("Copy log")
                            .disabled(rows.is_empty())
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(plain_text(
                                    &this.git_log.rows,
                                )));
                                this.notification = Some(Notification::success("Git log copied."));
                                cx.notify();
                            })),
                    )
                    .child(
                        crate::controls::compact_icon_button("git-log-clear")
                            .icon(IconName::Trash)
                            .ghost()
                            .accessibility_label("Clear Git log")
                            .tooltip("Clear log")
                            .disabled(rows.is_empty())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.git_log.view.clear(cx);
                                this.git_log.records.clear();
                                this.git_log.rows = Default::default();
                                this.git_log.invalidate_reflow();
                                this.git_log.gap = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        crate::controls::compact_icon_button("git-log-close")
                            .icon(IconName::X)
                            .ghost()
                            .accessibility_label("Close Git log")
                            .tooltip("Close log")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.git_log_close(window, cx)),
                            ),
                    ),
            )
            .child(self.status("git-log-events", cx))
            .when_some(self.git_log.reflow_error.clone(), |view, error| {
                view.child(label(error))
            })
            .when(self.git_log.gap, |view| {
                view.child(label("Some older output was omitted from the bounded log."))
            })
            .when(rows.is_empty(), |view| {
                view.child(
                    label("Refresh this repository or run a Git action to see output.").p_3(),
                )
            })
            .child(
                document.borrow().selection.viewport(
                    uniform_list(
                        "git-log-lines",
                        rows.len(),
                        cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                            range.map(|i| git_log_view::line(&lines, i, cx)).collect()
                        }),
                    )
                    .track_scroll(&self.git_log.scroll)
                    .flex_1()
                    .min_h_0()
                    .px_3(),
                ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::Log;
    #[test]
    fn reflow_cannot_publish_after_resize_clear_append_or_scope_change() {
        let mut log = Log {
            scope: "server/repo/instance".into(),
            columns: 80,
            ..Default::default()
        };
        assert!(log.accepts_reflow("server/repo/instance", 0, 80));
        assert!(!log.accepts_reflow("other/repo/instance", 0, 80));
        log.columns = 60;
        assert!(!log.accepts_reflow("server/repo/instance", 0, 80));
        log.invalidate_reflow();
        assert!(!log.accepts_reflow("server/repo/instance", 0, 60));
        assert!(log.accepts_reflow("server/repo/instance", 1, 60));
    }
}
