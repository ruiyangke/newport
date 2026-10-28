//! Query-revision projections and viewport layout for the read-only core grid.
use super::*;
use std::{cell::RefCell, rc::Rc};

const GAP: f32 = 6.;

pub(super) struct Core {
    id: String,
    pub(super) usage: f64,
    peak: f64,
    points: Series,
}

#[derive(Default)]
pub(crate) struct Cache {
    projection: RefCell<Option<Projection>>,
}
struct Projection {
    server: String,
    revision: u64,
    cores: Rc<Vec<Core>>,
}
impl Cache {
    pub(super) fn get(&self, server: &str, revision: u64, source: &Value) -> Rc<Vec<Core>> {
        let mut projection = self.projection.borrow_mut();
        if let Some(previous) = projection.as_ref()
            && previous.server == server
            && previous.revision == revision
        {
            return previous.cores.clone();
        }
        let mut source: Vec<_> = source.as_array().into_iter().flatten().collect();
        source.sort_by_key(|cpu| cpu["id"].as_u64().unwrap_or(0));
        let cores = Rc::new(source.into_iter().map(prepare).collect());
        *projection = Some(Projection {
            server: server.into(),
            revision,
            cores: Rc::clone(&cores),
        });
        cores
    }
}

fn prepare(cpu: &Value) -> Core {
    let readings = cpu["history"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let usage = number(&cpu["usage"]);
    let peak = readings.iter().map(number).fold(usage, f64::max);
    let points = if readings.len() > 1 {
        readings
            .iter()
            .enumerate()
            .map(|(i, value)| Point {
                at: 60. - (readings.len() - 1 - i) as f64 * 60. / 59.,
                value: Some(number(value)),
                gap: false,
            })
            .collect()
    } else {
        let value = readings.first().map(number).unwrap_or(usage);
        [0., 60.]
            .into_iter()
            .map(|at| Point {
                at,
                value: Some(value),
                gap: false,
            })
            .collect()
    };
    Core {
        id: text(cpu, "id"),
        usage,
        peak,
        points: Rc::new(points),
    }
}

pub(super) fn grid(
    cores: Rc<Vec<Core>>,
    graphs: graph_render::Cache,
    columns: usize,
    height: f32,
) -> impl IntoElement {
    let columns = columns.max(1);
    let rows = cores.len().div_ceil(columns);
    let total_height = rows as f32 * height + rows.saturating_sub(1) as f32 * GAP;
    // Reserve the exact CSS-grid-equivalent height in the page's single scroll
    // container. No core tile has focus or actions that require keeping it alive.
    canvas(
        move |bounds, window, cx| {
            let clipped = bounds.intersect(&window.content_mask().bounds);
            if clipped.is_empty() {
                return Vec::new();
            }
            let stride = px(height + GAP);
            let first = ((clipped.top() - bounds.top()) / stride).floor() as usize;
            let end = (((clipped.bottom() - bounds.top()) / stride).ceil() as usize).min(rows);
            let width =
                (bounds.size.width - px(columns.saturating_sub(1) as f32 * GAP)) / columns as f32;
            let mut visible = Vec::new();
            for row in first..end {
                let top = bounds.top() + stride * row as f32;
                if top + px(height) <= clipped.top() {
                    continue;
                }
                for column in 0..columns {
                    let Some(core) = cores.get(row * columns + column) else {
                        break;
                    };
                    let mut tile = tile(core, height, &graphs, cx);
                    tile.layout_as_root(
                        size(
                            AvailableSpace::Definite(width),
                            AvailableSpace::Definite(px(height)),
                        ),
                        window,
                        cx,
                    );
                    tile.prepaint_at(
                        point(bounds.left() + (width + px(GAP)) * column as f32, top),
                        window,
                        cx,
                    );
                    visible.push(tile);
                }
            }
            visible
        },
        |_, tiles, window, cx| {
            for mut tile in tiles {
                tile.paint(window, cx);
            }
        },
    )
    .w_full()
    .h(px(total_height))
    .flex_shrink_0()
}

fn tile(core: &Core, height: f32, graphs: &graph_render::Cache, cx: &App) -> AnyElement {
    let accent = crate::theme::chart_primary(cx.theme().is_dark());
    let selector = format!("cpu-core-{}", core.id);
    div()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector.clone())
        .role(accesskit::Role::Image)
        .test_support()
        .aria_label(format!(
            "Core {}: {:.0}% now, peak {:.0}% in the last minute",
            core.id, core.usage, core.peak
        ))
        .relative()
        .w_full()
        .h(px(height))
        .rounded(px(crate::ui::RADIUS))
        .overflow_hidden()
        .border_1()
        .border_color(accent.opacity(0.25))
        .bg(accent.opacity(0.03))
        .child(graphs.element(
            format!("core-{}", core.id),
            (core.points.clone(), Series::default()),
            (0., 60.),
            100.,
            GraphStyle::Core,
        ))
        .child(
            div()
                .absolute()
                .top(px(3.))
                .left(px(4.))
                .px(px(2.))
                .rounded(px(crate::ui::RADIUS))
                .bg(cx.theme().background.opacity(0.75))
                .text_size(px(crate::ui::FONT_SIZE))
                .child(core.id.clone()),
        )
        .child(
            div()
                .absolute()
                .top(px(3.))
                .right(px(4.))
                .px(px(2.))
                .rounded(px(crate::ui::RADIUS))
                .bg(cx.theme().background.opacity(0.75))
                .text_size(px(crate::ui::FONT_SIZE))
                .child(format!("{:.0}%", core.usage)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{Cache, grid};
    use crate::{overview::graph_render, theme};
    use gpui_kit::{
        self as gpui, Context, IntoElement, Render, ScrollHandle, TestAppContext, Window, div,
        point, prelude::*, px, size,
    };
    use serde_json::{Value, json};
    use std::rc::Rc;

    #[test]
    fn projection_tracks_query_revision_and_server_without_rebuilding_on_redraw() {
        let cache = Cache::default();
        let source = json!([
            {"id":2,"usage":17,"history":[22,36]},
            {"id":0,"usage":45},
            {"id":1,"usage":30,"history":[9]}
        ]);
        let first = cache.get("server-a", 1, &source);
        assert_eq!(
            first.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["0", "1", "2"]
        );
        assert_eq!(first[0].points[0].value, Some(45.));
        assert_eq!(first[1].points[0].value, Some(9.));
        assert_eq!(first[2].peak, 36.);
        assert!(Rc::ptr_eq(&first, &cache.get("server-a", 1, &source)));
        let second = cache.get("server-b", 1, &source);
        assert!(!Rc::ptr_eq(&first, &second));
        let updated = cache.get("server-b", 2, &json!([{"id":2,"usage":99}]));
        assert_eq!(updated.len(), 1);
        assert_eq!(updated[0].usage, 99.);
        assert_eq!(updated[0].points[1].at, 60.);
        assert!(cache.get("server-b", 0, &Value::Null).is_empty());
    }

    struct GridFixture {
        cache: Cache,
        graphs: graph_render::Cache,
        data: Value,
        revision: u64,
        columns: usize,
        height: f32,
        scroll: ScrollHandle,
    }
    impl Render for GridFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("core-viewport")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .flex()
                .flex_col()
                .child(div().h(px(20.)).flex_shrink_0())
                .child(grid(
                    self.cache.get("fixture", self.revision, &self.data),
                    self.graphs.clone(),
                    self.columns,
                    self.height,
                ))
                .child(
                    div()
                        .debug_selector(|| "core-footer".into())
                        .h(px(20.))
                        .flex_shrink_0(),
                )
        }
    }

    #[gpui::test]
    #[ignore = "manual core-grid CPU benchmark; includes scrolling beyond chart-cache capacity"]
    fn benchmark_core_grid_after_scrolling(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            theme::install(false, cx);
        });
        let (view, visual) = cx.add_window_view(|_, _| GridFixture {
            cache: Cache::default(),
            graphs: graph_render::Cache::default(),
            data: json!(
                (0..256)
                    .map(|id| json!({
                        "id":id,"usage":id % 100,
                        "history":(0..60).map(|i| (id+i)%100).collect::<Vec<_>>()
                    }))
                    .collect::<Vec<_>>()
            ),
            revision: 1,
            columns: 8,
            height: 80.,
            scroll: ScrollHandle::new(),
        });
        visual.simulate_resize(size(px(1000.), px(400.)));
        crate::page_perf_tests::frame(visual);
        for (position, offset, selector) in [
            ("first", 0., "cpu-core-0"),
            ("last", -10000., "cpu-core-255"),
            ("return", 0., "cpu-core-0"),
        ] {
            if position == "last" {
                // Visit enough distinct charts to exhaust the cache before
                // measuring a stable viewport containing later core IDs.
                for offset in (0..2400).step_by(300) {
                    view.update(visual, |view, cx| {
                        view.scroll.set_offset(point(px(0.), px(-offset as f32)));
                        cx.notify();
                    });
                    crate::page_perf_tests::frame(visual);
                }
            }
            view.update(visual, |view, cx| {
                view.scroll.set_offset(point(px(0.), px(offset)));
                cx.notify();
            });
            crate::page_perf_tests::frame(visual);
            assert!(visual.debug_bounds(selector).is_some());
            let start = std::time::Instant::now();
            for _ in 0..30 {
                view.update(visual, |_, cx| cx.notify());
                crate::page_perf_tests::frame(visual);
            }
            eprintln!(
                "256-core grid viewport={position}: {:.3} ms/CPU frame",
                start.elapsed().as_secs_f64() * 1000. / 30.
            );
        }
    }

    #[gpui::test]
    fn core_grid_preserves_geometry_and_readings_through_scroll_resize_and_refresh(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            theme::install(false, cx);
        });
        let (view, visual) = cx.add_window_view(|_, _| GridFixture {
            cache: Cache::default(),
            graphs: graph_render::Cache::default(),
            data: json!(
                (0..5)
                    .map(|id| json!({"id":id,"usage":id,"history":[0,50]}))
                    .collect::<Vec<_>>()
            ),
            revision: 1,
            columns: 2,
            height: 40.,
            scroll: ScrollHandle::new(),
        });
        visual.simulate_resize(size(px(206.), px(100.)));
        crate::page_perf_tests::frame(visual);
        let a = visual.debug_bounds("cpu-core-0").unwrap();
        let b = visual.debug_bounds("cpu-core-1").unwrap();
        assert_eq!(a.top(), px(20.));
        assert_eq!(a.size, size(px(100.), px(40.)));
        assert_eq!(b.left(), px(106.));
        assert_eq!(visual.debug_bounds("cpu-core-2").unwrap().top(), px(66.));
        assert!(visual.debug_bounds("cpu-core-4").is_none());
        view.update(visual, |view, cx| {
            view.scroll.set_offset(point(px(0.), px(-60.)));
            cx.notify();
        });
        crate::page_perf_tests::frame(visual);
        assert!(visual.debug_bounds("cpu-core-0").is_none());
        assert_eq!(visual.debug_bounds("cpu-core-2").unwrap().top(), px(6.));
        view.update(visual, |view, cx| {
            view.scroll.set_offset(point(px(0.), px(-72.)));
            cx.notify();
        });
        crate::page_perf_tests::frame(visual);
        assert_eq!(visual.debug_bounds("cpu-core-4").unwrap().bottom(), px(80.));
        assert_eq!(
            visual.debug_bounds("core-footer").unwrap().bottom(),
            px(100.)
        );
        view.update(visual, |view, cx| {
            view.columns = 1;
            view.height = 60.;
            view.scroll.set_offset(point(px(0.), px(0.)));
            view.data[0]["usage"] = json!(73);
            view.revision += 1;
            cx.notify();
        });
        visual.simulate_resize(size(px(110.), px(100.)));
        crate::page_perf_tests::frame(visual);
        assert_eq!(
            visual.debug_bounds("cpu-core-0").unwrap().size,
            size(px(110.), px(60.))
        );
        assert!(visual.debug_bounds("cpu-core-1").is_some());
        assert!(visual.debug_bounds("cpu-core-2").is_none());
        assert!(visual.update(|window, _| {
            gpui_kit::base::test_support::snapshots(window)
                .iter()
                .any(|node| node.label() == Some("Core 0: 73% now, peak 73% in the last minute"))
        }));
        view.update(visual, |view, cx| {
            view.scroll.set_offset(point(px(0.), px(-264.)));
            cx.notify();
        });
        crate::page_perf_tests::frame(visual);
        assert_eq!(visual.debug_bounds("cpu-core-4").unwrap().bottom(), px(80.));
        assert_eq!(
            visual.debug_bounds("core-footer").unwrap().bottom(),
            px(100.)
        );
        view.update(visual, |view, cx| {
            view.data = json!([]);
            view.revision += 1;
            cx.notify();
        });
        crate::page_perf_tests::frame(visual);
        assert!(visual.debug_bounds("cpu-core-4").is_none());
        assert_eq!(
            visual.debug_bounds("core-footer").unwrap().bottom(),
            px(40.)
        );
    }
}
