//! Compose Activity presentation; shared client owns jobs, output and cancellation.
use super::*;
use crate::{log_line, selectable_log::SelectableLog};
use gpui_kit::{base::Selectable, component::spinner::Spinner};
use newport_client::compose_activity::ActivityData;
use newport_domain::{
    compose::{self, ComposeJob},
    containers_log::Severity,
};

#[cfg(test)]
#[path = "compose_activity_tests.rs"]
mod tests;

#[derive(Clone, PartialEq)]
struct OutputRow {
    offset: u64,
    message: String,
    tone: log_line::Tone,
}
pub(super) struct Activity {
    data: ActivityData,
    dataset: Option<(String, String)>,
    source_stamp: Option<(u64, usize)>,
    rows: Rc<Vec<OutputRow>>,
    list: ListState,
    focus: FocusHandle,
    selection: SelectableLog,
    unseen: usize,
    history_open: bool,
    output_error: Option<String>,
}
impl Activity {
    pub(super) fn new(window: &mut Window, cx: &mut Context<ContainersPanel>) -> Self {
        // Tail following controls scrolling; short output still starts at the top.
        let list = ListState::new(0, ListAlignment::Top, px(120.));
        list.set_follow_mode(FollowMode::Tail);
        list.set_scroll_handler(cx.listener(|this, event: &ListScrollEvent, _, cx| {
            if event.is_following_tail {
                this.activity.unseen = 0;
            }
            cx.stop_propagation();
            cx.notify();
        }));
        let focus = cx.focus_handle();
        Self {
            data: Default::default(),
            dataset: None,
            source_stamp: None,
            rows: Rc::new(vec![]),
            list,
            selection: SelectableLog::new(focus.clone(), window, cx),
            focus,
            unseen: 0,
            history_open: false,
            output_error: None,
        }
    }
    pub(super) fn sync(&mut self, client: &Containers, cx: &mut App) {
        self.data.sync(client);
        let dataset = self
            .data
            .project()
            .zip(self.data.selected_job())
            .map(|(p, j)| (p.into(), j.into()));
        if self.dataset != dataset {
            if self.dataset.as_ref().map(|d| &d.0) != dataset.as_ref().map(|d| &d.0) {
                self.history_open = false;
            }
            self.dataset = dataset;
            self.source_stamp = None;
            self.rows = Rc::new(vec![]);
            self.unseen = 0;
            self.output_error = None;
            self.selection.clear(cx);
            self.selection.set_rows(vec![], cx);
            self.list.reset(0);
            self.list.set_follow_mode(FollowMode::Tail);
        }
        let Some(output) = self.data.output() else {
            return;
        };
        let stamp = (output.offset, output.output.len());
        if self.source_stamp == Some(stamp) {
            return;
        }
        let rows = match compose::output_lines(&output.output, output.offset) {
            Ok(lines) => lines
                .into_iter()
                .map(|line| OutputRow {
                    offset: line.offset,
                    message: line.message.to_owned(),
                    tone: match line.severity {
                        Severity::Normal => log_line::Tone::Normal,
                        Severity::Warning => log_line::Tone::Warning,
                        Severity::Error => log_line::Tone::Error,
                    },
                })
                .collect::<Vec<_>>(),
            Err(error) => {
                self.output_error = Some(error.into());
                return;
            }
        };
        self.output_error = None;
        let pinned = self.list.is_following_tail();
        if !pinned && let Some((previous_end, _)) = self.source_stamp {
            self.unseen += rows.iter().filter(|r| r.offset >= previous_end).count();
        }
        if self.rows.first().map(|r| r.offset) != rows.first().map(|r| r.offset) {
            self.selection.clear(cx);
        }
        self.source_stamp = Some(stamp);
        let anchor = self.list.logical_scroll_top();
        let preserved = self
            .rows
            .get(anchor.item_ix)
            .and_then(|r| rows.iter().position(|n| n.offset == r.offset));
        let count = self.list.item_count();
        if rows.len() >= count {
            self.list.splice(count..count, rows.len() - count);
        } else {
            self.list.splice(rows.len()..count, 0);
        }
        self.list.remeasure();
        if pinned {
            self.list.set_follow_mode(FollowMode::Tail);
        } else {
            self.list.scroll_to(ListOffset {
                item_ix: preserved.unwrap_or(0),
                offset_in_item: preserved.map_or(px(0.), |_| anchor.offset_in_item),
            });
        }
        self.selection
            .set_rows(rows.iter().map(|r| Some(r.message.clone())).collect(), cx);
        self.rows = Rc::new(rows);
    }
}

impl ContainersPanel {
    /// A notification may outlive both the job and its project. Revalidate before
    /// changing the current selection; the host separately checks connection identity.
    pub(crate) fn open_compose_activity(
        &mut self,
        project: &str,
        job: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.retired
            || !self.client.has_project_job(project, job)
            || !self.client.snapshot().is_some_and(|snapshot| {
                snapshot.containers.iter().any(|container| {
                    container
                        .compose
                        .as_ref()
                        .is_some_and(|labels| labels.project == project)
                })
            })
        {
            return false;
        }
        self.select(domain::project_key(project), window, cx);
        if !self.client.select_project_job(project, job.into()) {
            return false;
        }
        self.tab = 1;
        self.sync_selection(cx);
        self.detail_scroll.set_offset(point(px(0.), px(0.)));
        self.activity.focus.focus(window, cx);
        cx.notify();
        true
    }
    pub(crate) fn running_compose_job(&self) -> Option<&ComposeJob> {
        let project = self.client.selected_project()?;
        compose::running_job(self.activity.data.jobs(), project).filter(|job| {
            self.activity
                .data
                .output()
                .is_none_or(|output| output.job != job.job || output.state.is_running())
        })
    }
    pub(crate) fn show_compose_output(
        &mut self,
        project: &str,
        job: String,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.client.selected_project() != Some(project)
            || self.active_selection.as_deref() != Some(domain::project_key(project).as_str())
            || self
                .running_compose_job()
                .is_none_or(|running| running.job != job)
            || !self.client.select_project_job(project, job)
        {
            return false;
        }
        self.tab = 1;
        self.sync_selection(cx);
        cx.notify();
        true
    }
    pub(super) fn sync_activity(&mut self, cx: &mut Context<Self>) {
        let enabled = self.tab == 1
            && self
                .active_selection
                .as_deref()
                .is_some_and(|key| key.starts_with("p:"));
        self.client.set_activity_active(enabled);
        if enabled && let Some(project) = self.client.selected_project().map(str::to_owned) {
            let preferred = self.client.preferred_job(&project).map(str::to_owned);
            if preferred.as_deref() != self.client.selected_job() {
                if let Some(job) = preferred {
                    self.client.select_project_job(&project, job);
                } else {
                    self.client.clear_job_selection();
                }
            }
        }
        self.activity.sync(&self.client, cx);
    }
    fn activity_keys(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.modifiers.modified() {
            return;
        }
        let list = &self.activity.list;
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
            self.activity.unseen = 0;
        }
        cx.stop_propagation();
        cx.notify();
    }
    pub(super) fn compose_activity_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let data = &self.activity.data;
        let current = data.current();
        let running = current.is_some_and(|j| j.state.is_running());
        let pending = (data.output_pending() && data.output().is_none())
            || (current.is_none() && data.jobs_pending());
        let has_job = current.is_some() || data.selected_job().is_some();
        if !pending && !has_job && data.jobs_error().is_none() {
            return crate::empty::compact(
                "Compose commands you run here — Up, Pull, Rebuild, Down, Scale — show their output in this tab.",
                cx,
            )
            .id("compose-activity-empty")
            .debug_selector(|| "compose-activity-empty".into())
            .role(accesskit::Role::Status)
            .into_any_element();
        }
        let pinned = self.activity.list.is_following_tail();
        let project = self.client.selected_project().unwrap_or("").to_owned();
        let rows = self.activity.rows.clone();
        let count = rows.len();
        let now = chrono::Local::now().timestamp_millis();
        let footer = if pending {
            "Starting…".into()
        } else if !has_job {
            "No commands yet".into()
        } else {
            format!(
                "{count} lines · {}",
                if !running {
                    "finished"
                } else if pinned {
                    "following new lines"
                } else {
                    "scrolled up; new lines wait below"
                }
            )
        };
        let output_error = data
            .output_error()
            .or(self.activity.output_error.as_deref());
        div().id("compose-activity").debug_selector(|| "compose-activity".into())
            .flex().flex_col().flex_1().min_h(px(260.)).gap(px(8.))
            .when_some(data.jobs_error(), |v,error| v.child(div().flex().items_center().gap(px(8.))
                .child(alert("compose-activity-jobs-error",error.to_owned(),cx).flex_1())
                .child(crate::controls::button("compose-jobs-retry").label("Try again").accessibility_label("Try reading history again").disabled(data.jobs_pending())
                    .on_click(cx.listener(|this,_,_,cx| { this.client.retry_jobs(); cx.notify(); })))))
            .when_some(current, |v,job| v.child(self.activity_header(job, now, cx)))
            .when_some(output_error, |v,error| v.child(div().flex().items_center().gap(px(8.))
                .child(alert("compose-activity-output-error",error.to_owned(),cx).flex_1())
                .child(crate::controls::button("compose-output-retry").label("Try again").accessibility_label("Try reading output again").disabled(data.output_pending())
                    .on_click(cx.listener(|this,_,_,cx| { this.client.retry_job_output(); cx.notify(); })))))
            .when(self.client.resource("compose-cancel").is_some_and(|r|r.error.is_some()), |v| v.child(self.status("compose-cancel",cx)))
            .child(div().id("compose-output-viewport").debug_selector(|| "compose-output-viewport".into())
                .role(accesskit::Role::Log).aria_label(format!("{project} activity output"))
                .a11y_synthetic_children(move |tree| { if pending { tree.parent_node().set_busy(); } tree.parent_node().set_live(if running {accesskit::Live::Polite} else {accesskit::Live::Off}); })
                .track_focus(&self.activity.focus).on_key_down(cx.listener(Self::activity_keys))
                .relative().flex_1().min_h(px(200.)).border_1().border_color(cx.theme().border)
                .focus_visible(|s| crate::controls::inset_focus_ring(s, cx)).rounded(px(crate::ui::RADIUS))
                .bg(cx.theme().muted.opacity(0.35)).overflow_hidden()
                .when(pending && rows.is_empty(), |v|v.child(loading::lines("compose-activity",12)))
                .when(!pending && rows.is_empty() && data.jobs_error().is_none() && output_error.is_none(), |v|v.child(crate::empty::compact(if !has_job {
                    "Compose commands you run here — Up, Pull, Rebuild, Down, Scale — show their output in this tab."
                } else if running { "Waiting for output…" } else { "No output." },cx)))
                .when(!rows.is_empty(), |v| v.child(self.activity.selection.viewport(
                    list(self.activity.list.clone(),cx.processor(move |this,index:usize,_,cx| {
                        let row=&rows[index];
                        log_line::render("compose-output",index, log_line::LogLine { at:None,time:None,source:"",message:&row.message,styles:None,tone:row.tone },"",&this.activity.selection,cx)
                    })).size_full().px(px(10.)).py(px(8.)))))
                .when(self.activity.unseen>0&&!pinned, |v|v.child(div().absolute().bottom(px(12.)).left_0().right_0().flex().justify_center().child(
                    super::logs::new_lines_button("compose-output-latest",self.activity.unseen as u64,cx)
                        .on_click(cx.listener(|this,_,_,cx| {this.activity.list.set_follow_mode(FollowMode::Tail);this.activity.unseen=0;cx.notify();})))))
                .on_scroll_wheel(|_,_,cx| cx.stop_propagation()))
            .child(div().id("compose-output-footer").debug_selector(|| "compose-output-footer".into()).flex_shrink_0().child(crate::controls::pane_status_bar(cx)
                .left(quiet(footer.clone(),cx).text_size(px(crate::ui::FONT_SIZE)).id("compose-output-status").role(accesskit::Role::Status).aria_label(footer))
                .right(crate::controls::compact_icon_button("compose-output-copy").ghost().icon(IconName::Copy).accessibility_label("Copy command output").disabled(count==0)
                    .on_click(cx.listener(|this,_,_,cx| {
                        if let Some(job)=this.activity.data.output() { cx.write_to_clipboard(ClipboardItem::new_string(job.output.clone())); }
                    })))))
            .when(data.jobs().len()>1, |v|v.child(self.activity_history(now,cx)))
            .into_any_element()
    }
    fn activity_header(&self, job: &ComposeJob, now: i64, cx: &mut Context<Self>) -> AnyElement {
        let project = job.project.clone();
        let id = job.job.clone();
        let cancelling = self.client.compose_cancelling(&id);
        div()
            .flex_shrink_0()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(12.))
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.))
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(px(crate::ui::FONT_SIZE))
                    .child(job.command.clone()),
            )
            .child(activity_job_state(
                job,
                now,
                "compose-current-status".into(),
                cx,
            ))
            .when(job.state.is_running(), |v| {
                v.child(
                    crate::controls::compact_button("compose-activity-cancel")
                        .label(if cancelling {
                            "Cancelling…"
                        } else {
                            "Cancel"
                        })
                        .icon(IconName::Square)
                        .loading(cancelling)
                        .disabled(cancelling || self.client.compose_cancel_pending())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.client.selected_project() == Some(project.as_str())
                                && this.client.selected_job() == Some(id.as_str())
                            {
                                this.client.compose_cancel(id.clone());
                            }
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }
    fn activity_history(&self, now: i64, cx: &mut Context<Self>) -> AnyElement {
        let jobs = self.activity.data.jobs();
        let selected = self.client.selected_job();
        div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                crate::controls::compact_button("compose-history-toggle")
                    .ghost()
                    .icon(if self.activity.history_open {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .label(format!("Earlier commands ({})", jobs.len() - 1))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.activity.history_open = !this.activity.history_open;
                        cx.notify();
                    })),
            )
            .when(self.activity.history_open, |v| {
                v.child(
                    div()
                        .id("compose-history")
                        .max_h(px(180.))
                        .overflow_y_scroll()
                        .children(jobs.iter().map(|job| {
                            let project = job.project.clone();
                            let id = job.job.clone();
                            let status = compose::job_status(job, now);
                            crate::controls::compact_button(SharedString::from(format!(
                                "compose-history-{}",
                                job.job
                            )))
                            .ghost()
                            .w_full()
                            .justify_start()
                            .selected(selected == Some(job.job.as_str()))
                            .accessibility_label(format!(
                                "{} {} · {}",
                                if selected == Some(job.job.as_str()) {
                                    "Current command:"
                                } else {
                                    "View"
                                },
                                job.command,
                                status.label
                            ))
                            .child(
                                div()
                                    .flex()
                                    .w_full()
                                    .min_w_0()
                                    .items_center()
                                    .gap(px(10.))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .text_size(px(crate::ui::FONT_SIZE))
                                            .child(job.command.clone()),
                                    )
                                    .child(activity_job_state(
                                        job,
                                        now,
                                        format!("compose-history-{}-status", job.job),
                                        cx,
                                    )),
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if this.client.select_project_job(&project, id.clone()) {
                                        this.activity.sync(&this.client, cx);
                                    }
                                    cx.notify();
                                },
                            ))
                        }))
                        .on_scroll_wheel(|_, _, cx| cx.stop_propagation()),
                )
            })
            .into_any_element()
    }
}

fn activity_job_state(job: &ComposeJob, now: i64, id: String, cx: &App) -> AnyElement {
    let status = compose::job_status(job, now);
    let icon_id = format!("{id}-icon");
    div()
        .debug_selector(move || id.clone())
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(5.))
        .text_size(px(crate::ui::FONT_SIZE))
        .line_height(relative(1.5))
        .font_weight(FontWeight::NORMAL)
        .text_color(tone_color(status.tone, cx))
        .child(
            div()
                .debug_selector(move || icon_id.clone())
                .size(px(12.))
                .flex_shrink_0()
                .child(if job.state.is_running() {
                    Spinner::new()
                        .with_size(px(12.))
                        .color(tone_color(status.tone, cx))
                        .into_any_element()
                } else {
                    Icon::new(if job.state == compose::JobState::Succeeded {
                        IconName::CircleCheck
                    } else {
                        IconName::CircleX
                    })
                    .size(px(12.))
                    .into_any_element()
                }),
        )
        .child(quiet(status.label, cx).text_color(tone_color(status.tone, cx)))
        .into_any_element()
}
