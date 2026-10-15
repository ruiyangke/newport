//! Searchable, sortable monitoring rows; process data remains read-only.
use super::*;
use gpui_kit::component::input::InputEvent;
const PAGE_SIZE: usize = 20;
const ROW_HEIGHT: f32 = 46.;
const SORT_NAMES: [&str; 6] = ["CPU", "Memory", "Process", "PID", "User", "Time"];
fn page_range(count: usize, requested: usize) -> std::ops::Range<usize> {
    let start = requested.min(count.saturating_sub(1) / PAGE_SIZE) * PAGE_SIZE;
    start..(start + PAGE_SIZE).min(count)
}
#[derive(Default)]
pub(super) struct State {
    input: Option<Entity<InputState>>,
    page: usize,
    subscription: Option<Subscription>,
    ordered: std::cell::RefCell<ProcessCache>,
}
#[derive(Default)]
struct ProcessCache {
    key: Option<(u64, String, usize, bool)>,
    rows: std::rc::Rc<Vec<Value>>,
}
impl ProcessCache {
    fn get(
        &mut self,
        revision: u64,
        source: &[Value],
        query: &str,
        sort: usize,
        ascending: bool,
    ) -> std::rc::Rc<Vec<Value>> {
        let key = (
            revision,
            query.trim().to_lowercase(),
            sort.min(5),
            ascending,
        );
        if self.key.as_ref() != Some(&key) {
            self.rows = std::rc::Rc::new(ordered(source.to_vec(), &key.1, key.2, ascending));
            self.key = Some(key);
        }
        self.rows.clone()
    }
}
fn ordered(mut rows: Vec<Value>, query: &str, sort: usize, ascending: bool) -> Vec<Value> {
    let query = query.trim().to_lowercase();
    rows.retain(|row| {
        ["name", "command", "user", "pid"]
            .iter()
            .any(|key| text(row, key).to_lowercase().contains(&query))
    });
    rows.sort_by(|a, b| {
        let key = ["cpu", "memory", "name", "pid", "user", "elapsed"][sort.min(5)];
        let order = if matches!(key, "name" | "user") {
            text(a, key)
                .to_lowercase()
                .cmp(&text(b, key).to_lowercase())
        } else {
            a[key]
                .as_f64()
                .unwrap_or(-1.)
                .total_cmp(&b[key].as_f64().unwrap_or(-1.))
        };
        (if ascending { order } else { order.reverse() })
            .then_with(|| a["pid"].as_u64().cmp(&b["pid"].as_u64()))
    });
    rows
}
fn elapsed(value: &Value) -> String {
    let Some(seconds) = value.as_u64() else {
        return "—".into();
    };
    if seconds >= 86400 {
        format!("{}d {}h", seconds / 86400, seconds % 86400 / 3600)
    } else if seconds >= 3600 {
        format!("{}h {}m", seconds / 3600, seconds % 3600 / 60)
    } else {
        format!("{}m {}s", seconds / 60, seconds % 60)
    }
}
fn magnitude(value: String, share: f64, width: f32, bars: bool, cx: &App) -> AnyElement {
    let color: Hsla = rgb(if cx.theme().is_dark() {
        0x3987e5
    } else {
        0x3478d4
    })
    .into();
    div()
        .w(px(width))
        .pr(px(8.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_end()
        .gap(px(8.))
        .when(bars, |view| {
            view.child(
                div()
                    .w(px(44.))
                    .flex_shrink_0()
                    .h(px(4.))
                    .rounded_full()
                    .bg(color.opacity(0.15))
                    .child(
                        div()
                            .h_full()
                            .w(px(44. * (share.clamp(0., 100.) / 100.) as f32))
                            .rounded_full()
                            .bg(color),
                    ),
            )
        })
        .child(div().flex_1().min_w_0().text_right().child(value))
        .into_any_element()
}
// The page owns the only scroll viewport. Reserve the complete page height,
// but construct/layout only intersecting read-only rows during prepaint. These
// rows contain no focusable controls; sort, filter and pagination remain in flow.
fn process_rows(
    processes: std::rc::Rc<Vec<Value>>,
    range: std::ops::Range<usize>,
    wide: bool,
    memory_total: f64,
) -> impl IntoElement {
    let count = range.len();
    canvas(
        move |bounds, window, cx| {
            let clipped = bounds.intersect(&window.content_mask().bounds);
            if clipped.is_empty() {
                return Vec::new();
            }
            let first = ((clipped.top() - bounds.top()) / px(ROW_HEIGHT)).floor() as usize;
            let end =
                (((clipped.bottom() - bounds.top()) / px(ROW_HEIGHT)).ceil() as usize).min(count);
            (first..end)
                .map(|index| {
                    let mut row =
                        process_row(&processes[range.start + index], wide, memory_total, cx);
                    row.layout_as_root(
                        size(
                            AvailableSpace::Definite(bounds.size.width),
                            AvailableSpace::Definite(px(ROW_HEIGHT)),
                        ),
                        window,
                        cx,
                    );
                    row.prepaint_at(
                        point(bounds.left(), bounds.top() + px(index as f32 * ROW_HEIGHT)),
                        window,
                        cx,
                    );
                    row
                })
                .collect::<Vec<_>>()
        },
        |_, rows, window, cx| {
            for mut row in rows {
                row.paint(window, cx);
            }
        },
    )
    .w_full()
    .h(px(count as f32 * ROW_HEIGHT))
    .flex_shrink_0()
}

fn process_row(p: &Value, wide: bool, memory_total: f64, cx: &App) -> AnyElement {
    let selector = format!("overview-process-{}", text(p, "pid"));
    let state = match p["state"].as_str().unwrap_or("") {
        "D" => " · Waiting on I/O",
        "Z" => " · Zombie",
        "T" => " · Stopped",
        "t" => " · Traced",
        _ => "",
    };
    div()
        .debug_selector(move || selector.clone())
        .h(px(ROW_HEIGHT))
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .flex()
        .items_center()
        .border_b_1()
        .border_color(cx.theme().border)
        .text_size(px(12.))
        .hover(|s| s.bg(theme::row_hover(cx)))
        .child(
            div()
                .flex_1()
                .w_0()
                .px(px(8.))
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .truncate()
                        .font_weight(FontWeight::MEDIUM)
                        .child(format!("{}{state}", text(p, "name"))),
                )
                .child(
                    label(
                        p["command"]
                            .as_str()
                            .unwrap_or(if p["memory"].as_u64() == Some(0) {
                                "Kernel thread"
                            } else {
                                ""
                            })
                            .to_owned(),
                    )
                    .text_size(px(11.))
                    .truncate(),
                ),
        )
        .child(
            cell(text(p, "pid"), 72.)
                .px(px(8.))
                .text_right()
                .text_color(cx.theme().muted_foreground),
        )
        .when(wide, |v| {
            v.child(
                cell(text(p, "user"), 90.)
                    .px(px(8.))
                    .truncate()
                    .text_color(cx.theme().muted_foreground),
            )
        })
        .child(magnitude(
            format!("{:.1}%", p["cpu"].as_f64().unwrap_or(0.)),
            p["cpu"].as_f64().unwrap_or(0.),
            if wide { 124. } else { 65. },
            wide,
            cx,
        ))
        .child(magnitude(
            bytes(&p["memory"]),
            if memory_total > 0. {
                p["memory"].as_f64().unwrap_or(0.) / memory_total * 100.
            } else {
                0.
            },
            if wide { 142. } else { 80. },
            wide,
            cx,
        ))
        .when(wide, |v| {
            v.child(
                cell(elapsed(&p["elapsed"]), 85.)
                    .px(px(8.))
                    .text_right()
                    .text_color(cx.theme().muted_foreground),
            )
        })
        .into_any_element()
}

impl Newport {
    fn sort_overview_processes(&mut self, index: usize, cx: &mut Context<Self>) {
        let state = self
            .overview_state
            .preferences
            .entry(self.server.clone())
            .or_default();
        state.ascending = if state.sort == index {
            !state.ascending
        } else {
            matches!(index, 2..=4)
        };
        state.sort = index;
        if let Some(state) = self.overview_state.processes.get_mut(&self.server) {
            state.page = 0;
        }
        self.save_workspace_selection();
        cx.notify();
    }
    pub(super) fn sync_overview_processes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page != Page::Overview || self.server.is_empty() {
            return;
        }
        let preferences = self.overview_state.prefs(&self.server);
        let server = self.server.clone();
        let state = self
            .overview_state
            .processes
            .entry(self.server.clone())
            .or_default();
        if state.input.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Filter processes…")
                    .default_value(preferences.process_filter)
            });
            state.subscription = Some(cx.subscribe(&input, move |this, input, event, cx| {
                if !matches!(event, InputEvent::Change) {
                    return;
                }
                this.overview_state
                    .preferences
                    .entry(server.clone())
                    .or_default()
                    .process_filter = input.read(cx).value().to_string();
                if let Some(state) = this.overview_state.processes.get_mut(&server) {
                    state.page = 0;
                }
                cx.notify();
            }));
            state.input = Some(input);
        }
    }
    pub(super) fn overview_processes(&self, data: &Value, cx: &mut Context<Self>) -> AnyElement {
        let Some(state) = self.overview_state.processes.get(&self.server) else {
            return div().into_any_element();
        };
        let Some(input) = state.input.as_ref() else {
            return div().into_any_element();
        };
        let requested_page = state.page;
        let preferences = self.overview_state.prefs(&self.server);
        let source = data["processes"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let total = source.len();
        let filtering = !input.read(cx).value().trim().is_empty();
        let revision = self
            .resources
            .get("page")
            .map_or(0, |resource| resource.data_revision);
        let processes = state.ordered.borrow_mut().get(
            revision,
            source,
            input.read(cx).value().as_str(),
            preferences.sort,
            preferences.ascending,
        );
        let state = preferences;
        let count = processes.len();
        let range = page_range(count, requested_page);
        let page = range.start / PAGE_SIZE;
        let wide = self.content_width >= 720.;
        let memory_total = data["memoryTotal"].as_f64().unwrap_or(0.);
        let columns = if wide {
            vec![
                (2, "Process", 0.),
                (3, "PID", 72.),
                (4, "User", 90.),
                (0, "CPU", 124.),
                (1, "Memory", 142.),
                (5, "Time", 85.),
            ]
        } else {
            vec![
                (2, "Process", 0.),
                (3, "PID", 72.),
                (0, "CPU", 65.),
                (1, "Memory", 80.),
            ]
        };
        div()
            .mt(px(24.))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("Processes · {}", text(data, "processCount"))),
                    )
                    .child(
                        div()
                            .w(px(220.))
                            .child(crate::controls::input(input).aria_label("Filter processes")),
                    )
                    .when(!wide, |view| {
                        let weak = cx.entity().downgrade();
                        let server = self.server.clone();
                        let sort = state.sort.min(5);
                        let ascending = state.ascending;
                        view.child(crate::controls::button("process-sort-menu")
                            .label(format!("{} {}", SORT_NAMES[sort], if ascending { "↑" } else { "↓" }))
                            .accessibility_label(format!("Sort processes by {}, {}", SORT_NAMES[sort], if ascending { "ascending" } else { "descending" }))
                            .dropdown_menu(move |mut menu, _, _| {
                                for (index, name) in SORT_NAMES.iter().enumerate() {
                                    let weak = weak.clone();
                                    let server = server.clone();
                                    menu = menu.item(PopupMenuItem::new(*name).checked(index == sort)
                                        .on_click(move |_, _, cx| {
                                            let _ = weak.update(cx, |this, cx| {
                                                if this.server == server { this.sort_overview_processes(index, cx); }
                                            });
                                        }));
                                }
                                menu
                            }))
                    }),
            )
            .child(label(if filtering {
                format!("Showing {count} of {total} collected processes")
            } else {
                format!("The busiest and largest {total} of {} · CPU over the last second, as a share of one core", text(data, "processCount"))
            }).text_size(px(11.)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .children(columns.into_iter().map(|(index, name, width)| {
                        let ascending = state.sort == index && state.ascending;
                        crate::controls::compact_button(("process-sort", index))
                            .child(div().w_full().flex().items_center().gap_1().text_size(px(11.))
                                .when(matches!(index, 0 | 1 | 3 | 5), |v| v.justify_end())
                                .when(matches!(index, 2 | 4), |v| v.justify_start())
                                .child(format!(
                                "{name}{}",
                                if state.sort == index {
                                    if ascending { " ↑" } else { " ↓" }
                                } else {
                                    ""
                                }
                            )))
                            .accessibility_label(format!("Sort by {name}{}", if state.sort == index { if ascending { ", ascending" } else { ", descending" } } else { "" }))
                            .px(px(8.))
                            .ghost()
                            .when(width == 0., |v| v.flex_1().min_w_0())
                            .when(width > 0., |v| v.w(px(width)).flex_shrink_0())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.sort_overview_processes(index, cx);
                            }))
                    })),
            )
            .when(count == 0, |v| {
                v.child(empty::compact(if total == 0 {
                    "No processes reported."
                } else {
                    "No processes match your search."
                }, cx))
            })
            .child(process_rows(processes, range.clone(), wide, memory_total))
            .when(count > 0, |view| view.child(
                div().flex().flex_wrap().items_center().justify_between().gap_2().py_2()
                    .child(label(format!("{}–{} of {count}{}", range.start + 1, range.end, if filtering { " matching" } else { " collected" })).text_size(px(11.)))
                    .when(count > PAGE_SIZE, |view| view.child(
                        crate::controls::pagination("process-pages").current_page(page + 1).total_pages(count.div_ceil(PAGE_SIZE))
                            .on_click(cx.listener(|this, page: &usize, _, cx| {
                                if let Some(state) = this.overview_state.processes.get_mut(&this.server) { state.page = page.saturating_sub(1); }
                                cx.notify();
                            }))
                    ))
            ))
            .into_any_element()
    }
}
#[cfg(test)]
mod tests {
    use super::{ProcessCache, elapsed, ordered, page_range};
    use crate::theme;
    use gpui_kit::{
        self as gpui, Context, IntoElement, Render, ScrollHandle, TestAppContext, Window, div,
        point, prelude::*, px, size,
    };
    use serde_json::Value;
    use serde_json::json;

    struct RowsFixture {
        rows: std::rc::Rc<Vec<Value>>,
        range: std::ops::Range<usize>,
        scroll: ScrollHandle,
        wide: bool,
    }
    impl Render for RowsFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("rows-viewport")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .flex()
                .flex_col()
                .child(div().h(px(20.)).flex_shrink_0())
                .child(super::process_rows(
                    self.rows.clone(),
                    self.range.clone(),
                    self.wide,
                    8192.,
                ))
                .child(
                    div()
                        .debug_selector(|| "rows-footer".into())
                        .h(px(20.))
                        .flex_shrink_0(),
                )
        }
    }

    #[gpui::test]
    fn process_rows_mount_only_visible_rows_and_keep_page_geometry(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            theme::install(false, cx);
        });
        let (view, visual) = cx.add_window_view(|_, _| RowsFixture {
            rows: std::rc::Rc::new((0..40).map(|pid| json!({"pid":pid,"name":format!("worker-{pid}"),"cpu":pid,"memory":1024})).collect()),
            range: 0..20,
            scroll: ScrollHandle::new(),
            wide: true,
        });
        visual.simulate_resize(size(px(900.), px(100.)));
        crate::page_perf_tests::frame(visual);
        assert_eq!(
            visual.debug_bounds("overview-process-0").unwrap().top(),
            px(20.)
        );
        assert_eq!(
            visual.debug_bounds("overview-process-1").unwrap().bottom(),
            px(112.)
        );
        assert!(visual.debug_bounds("overview-process-2").is_none());
        assert!(visual.debug_bounds("overview-process-19").is_none());
        // A partially visible first row must remain mounted, then disappear at
        // the exact boundary. No nested scrolling or approximate-height spacer.
        view.update(visual, |view, cx| {
            view.scroll.set_offset(point(px(0.), px(-65.)));
            cx.notify();
        });
        crate::page_perf_tests::frame(visual);
        assert_eq!(
            visual.debug_bounds("overview-process-0").unwrap().bottom(),
            px(1.)
        );
        assert!(visual.debug_bounds("overview-process-3").is_some());
        view.update(visual, |view, cx| {
            view.scroll.set_offset(point(px(0.), px(-66.)));
            cx.notify();
        });
        crate::page_perf_tests::frame(visual);
        assert!(visual.debug_bounds("overview-process-0").is_none());
        assert_eq!(
            visual.debug_bounds("overview-process-1").unwrap().top(),
            px(0.)
        );
        view.update(visual, |view, cx| {
            view.scroll.set_offset(point(px(0.), px(-860.)));
            cx.notify();
        });
        crate::page_perf_tests::frame(visual);
        assert!(visual.debug_bounds("overview-process-0").is_none());
        assert_eq!(
            visual.debug_bounds("overview-process-19").unwrap().bottom(),
            px(80.)
        );
        assert_eq!(
            visual.debug_bounds("rows-footer").unwrap().bottom(),
            px(100.)
        );
        // Paging changes the data at the same positions; a narrow resize keeps
        // the reserved height and materializes the new page, never old rows.
        view.update(visual, |view, cx| {
            view.range = 20..40;
            view.wide = false;
            cx.notify();
        });
        visual.simulate_resize(size(px(400.), px(100.)));
        crate::page_perf_tests::frame(visual);
        assert!(visual.debug_bounds("overview-process-19").is_none());
        let last = visual.debug_bounds("overview-process-39").unwrap();
        assert_eq!(last.size.width, px(400.));
        assert_eq!(last.bottom(), px(80.));
        view.update(visual, |view, cx| {
            view.range = 0..0;
            cx.notify();
        });
        crate::page_perf_tests::frame(visual);
        assert!(visual.debug_bounds("overview-process-39").is_none());
        assert_eq!(
            visual.debug_bounds("rows-footer").unwrap().bottom(),
            px(40.)
        );
    }

    #[test]
    #[ignore = "manual process preparation benchmark; excludes GPUI layout/paint"]
    fn benchmark_process_preparation() {
        use std::{hint::black_box, time::Instant};
        let rows: Vec<_> = (0..5000)
            .map(|pid| {
                json!({
                    "pid": pid, "name": format!("worker-{}", 5000 - pid),
                    "command": format!("/usr/bin/worker --project /srv/project-{pid} --listen"),
                    "user": "service", "cpu": pid % 100, "memory": pid * 4096,
                    "elapsed": pid * 10, "state": "S"
                })
            })
            .collect();
        let start = Instant::now();
        for _ in 0..60 {
            black_box(ordered(black_box(rows.clone()), "", 0, false));
        }
        eprintln!(
            "5000 processes, repeated preparation: {:.3} ms/redraw",
            start.elapsed().as_secs_f64() * 1000. / 60.
        );
        let mut cache = ProcessCache::default();
        black_box(cache.get(1, &rows, "", 0, false));
        let start = Instant::now();
        for _ in 0..60 {
            black_box(cache.get(1, black_box(&rows), "", 0, false));
        }
        eprintln!(
            "5000 processes, cached preparation: {:.3} ms/redraw",
            start.elapsed().as_secs_f64() * 1000. / 60.
        );
    }
    #[test]
    fn process_cache_reuses_redraws_and_invalidates_on_data_filter_and_sort() {
        let source = vec![
            json!({"pid":1,"cpu":1,"name":"node"}),
            json!({"pid":2,"cpu":9,"name":"worker"}),
        ];
        let mut cache = ProcessCache::default();
        let first = cache.get(1, &source, "", 0, false);
        assert_eq!(first[0]["pid"], 2);
        assert!(std::rc::Rc::ptr_eq(
            &first,
            &cache.get(1, &source, "", 0, false)
        ));
        let filtered = cache.get(1, &source, " NODE ", 0, false);
        assert_eq!(filtered.len(), 1);
        assert!(std::rc::Rc::ptr_eq(
            &filtered,
            &cache.get(1, &source, "node", 0, false)
        ));
        assert_eq!(cache.get(1, &source, "", 0, true)[0]["pid"], 1);
        assert_eq!(cache.get(1, &source, "", 3, false)[0]["pid"], 2);
        let updated = vec![json!({"pid":3,"cpu":20})];
        assert_eq!(cache.get(2, &updated, "", 0, false)[0]["pid"], 3);
        assert!(cache.get(0, &[], "", 0, false).is_empty());
    }
    #[test]
    fn pagination_handles_empty_exact_and_shrinking_results() {
        assert_eq!(page_range(0, 9), 0..0);
        assert_eq!(page_range(40, 1), 20..40);
        assert_eq!(page_range(41, 2), 40..41);
        assert_eq!(page_range(4, 2), 0..4);
        assert_eq!(page_range(41, usize::MAX), 40..41);
    }
    #[test]
    fn numeric_sort_and_ties_are_stable_before_pagination() {
        let rows = vec![
            json!({"pid":10,"cpu":2}),
            json!({"pid":2,"cpu":10}),
            json!({"pid":1,"cpu":10}),
        ];
        let sorted = ordered(rows.clone(), "", 0, false);
        assert_eq!(
            sorted
                .iter()
                .map(|p| p["pid"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![1, 2, 10]
        );
        assert_eq!(ordered(rows, "", 3, true)[2]["pid"], 10);
    }
    #[test]
    fn process_search_sort_and_missing_elapsed() {
        let rows = vec![
            json!({"pid":2,"name":"node","command":"node /project/api","cpu":3}),
            json!({"pid":1,"name":"worker","user":"service","cpu":20}),
        ];
        assert_eq!(ordered(rows.clone(), "", 0, false)[0]["pid"], 1);
        assert_eq!(ordered(rows.clone(), "API", 0, false)[0]["pid"], 2);
        assert_eq!(ordered(rows, "service", 0, false).len(), 1);
        assert_eq!(elapsed(&json!(3661)), "1h 1m");
        assert_eq!(elapsed(&json!(null)), "—");
    }
}
