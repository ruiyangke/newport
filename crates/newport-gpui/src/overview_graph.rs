//! Geometry belongs to the displayed data and viewport size, not a scroll frame.
use super::*;
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

const MAX_GRAPHS: usize = 128;

#[derive(Clone, Default)]
pub(crate) struct Cache(Rc<RefCell<VecDeque<Entry>>>);

struct Entry {
    key: String,
    graph: Rc<Graph>,
}

#[derive(PartialEq)]
struct Data {
    points: Series,
    second: Series,
    domain: (f64, f64),
    maximum: f64,
    style: GraphStyle,
}
struct Graph {
    data: Data,
    geometry: RefCell<Option<(Layout, Rc<Geometry>)>>,
}
#[derive(Clone, Copy, PartialEq)]
struct Layout {
    size: gpui::Size<Pixels>,
    scale: f32,
}
enum Primitive {
    Path(gpui::Path<Pixels>, Option<usize>, f32),
    Dot(gpui::Point<Pixels>, usize),
}
struct Geometry(Vec<Primitive>);

impl Cache {
    fn get(&self, key: String, data: Data) -> Rc<Graph> {
        let mut cache = self.0.borrow_mut();
        // Keep the bounded working set in recency order. The previous policy
        // pinned the first 128 charts forever, rebuilding every later core on
        // every redraw. Hits and replacements both move to the back.
        if let Some(index) = cache.iter().position(|entry| entry.key == key) {
            let entry = cache.remove(index).unwrap();
            if entry.graph.data == data {
                let graph = entry.graph.clone();
                cache.push_back(entry);
                return graph;
            }
        }
        let graph = Rc::new(Graph {
            data,
            geometry: RefCell::default(),
        });
        if cache.len() == MAX_GRAPHS {
            cache.pop_front();
        }
        cache.push_back(Entry {
            key,
            graph: graph.clone(),
        });
        graph
    }

    pub(super) fn element(
        &self,
        key: impl Into<String>,
        series: (Series, Series),
        domain: (f64, f64),
        maximum: f64,
        style: GraphStyle,
    ) -> AnyElement {
        let graph = self.get(
            key.into(),
            Data {
                points: series.0,
                second: series.1,
                domain,
                maximum,
                style,
            },
        );
        canvas(
            |_, _, _| (),
            move |bounds, _, window, cx| {
                if !visible(bounds, window.content_mask().bounds) {
                    return;
                }
                let geometry = graph.geometry(Layout {
                    size: bounds.size,
                    scale: window.scale_factor(),
                });
                geometry.paint(bounds.origin, window, cx);
            },
        )
        .size_full()
        .into_any_element()
    }
}

fn visible(bounds: Bounds<Pixels>, mask: Bounds<Pixels>) -> bool {
    let width: f32 = bounds.size.width.into();
    let height: f32 = bounds.size.height.into();
    width.is_finite()
        && height.is_finite()
        && width > 0.
        && height > 0.
        && !mask.intersect(&bounds).is_empty()
}

impl Graph {
    fn geometry(&self, layout: Layout) -> Rc<Geometry> {
        let mut cached = self.geometry.borrow_mut();
        if let Some((previous, geometry)) = cached.as_ref()
            && *previous == layout
        {
            return geometry.clone();
        }
        let geometry = Rc::new(Geometry::build(&self.data, layout));
        *cached = Some((layout, geometry.clone()));
        geometry
    }
}

impl Geometry {
    fn build(data: &Data, layout: Layout) -> Self {
        let Data {
            points,
            second,
            domain,
            maximum,
            style,
        } = data;
        let style = *style;
        let domain = *domain;
        let width: f32 = layout.size.width.into();
        let height: f32 = layout.size.height.into();
        let mut primitives = Vec::new();
        let grid: &[f32] = match style {
            GraphStyle::Sparkline => &[],
            GraphStyle::History => &[0., 0.5, 1.],
            GraphStyle::Core => &[0.5],
        };
        for &fraction in grid {
            let mut path =
                PathBuilder::stroke(px(if style == GraphStyle::Core { 1. } else { 0.5 }));
            path.move_to(point(px(0.), px(height * fraction)));
            path.line_to(point(px(width), px(height * fraction)));
            if let Ok(path) = path.build() {
                primitives.push(Primitive::Path(path, None, 1.));
            }
        }
        for (index, points) in [points, second].into_iter().enumerate() {
            let mut path = PathBuilder::stroke(px(match style {
                GraphStyle::History => 2.,
                GraphStyle::Sparkline => 1.4,
                GraphStyle::Core => 1.,
            }));
            for run in graph_runs(points, domain) {
                let run = pixel_envelope(run, domain, (width * layout.scale).ceil() as usize);
                let positions: Vec<_> = run
                    .iter()
                    .map(|(at, value)| {
                        let x = ((at - domain.0) / (domain.1 - domain.0).max(1.)).clamp(0., 1.)
                            as f32
                            * width;
                        let y = (1. - (value / maximum.max(1.)).clamp(0., 1.)) as f32 * height;
                        point(px(x), px(y))
                    })
                    .collect();
                let first = positions[0];
                if second.is_empty() && style != GraphStyle::Sparkline && positions.len() > 1 {
                    let mut area = PathBuilder::fill();
                    area.move_to(point(first.x, px(height)));
                    for position in &positions {
                        area.line_to(*position);
                    }
                    area.line_to(point(positions.last().unwrap().x, px(height)));
                    area.close();
                    if let Ok(area) = area.build() {
                        primitives.push(Primitive::Path(
                            area,
                            Some(index),
                            if style == GraphStyle::Core { 0.32 } else { 0.1 },
                        ));
                    }
                }
                if positions.len() == 1 {
                    primitives.push(Primitive::Dot(first, index));
                    continue;
                }
                if index == 1 {
                    let pattern = if style == GraphStyle::History {
                        (5., 4.)
                    } else {
                        (3., 2.)
                    };
                    dashed_run(&positions, pattern, |begin, position| {
                        if begin {
                            path.move_to(position);
                        } else {
                            path.line_to(position);
                        }
                    });
                } else {
                    path.move_to(first);
                    for position in positions.into_iter().skip(1) {
                        path.line_to(position);
                    }
                }
            }
            if let Ok(path) = path.build() {
                primitives.push(Primitive::Path(path, Some(index), 1.));
            }
        }
        Self(primitives)
    }

    fn paint(&self, origin: gpui::Point<Pixels>, window: &mut Window, cx: &App) {
        let dark = cx.theme().is_dark();
        let colors: [Hsla; 2] = [
            crate::theme::chart_primary(dark),
            crate::theme::chart_secondary(dark),
        ];
        for primitive in &self.0 {
            match primitive {
                Primitive::Path(path, color, opacity) => {
                    let color = color
                        .map(|index| colors[index])
                        .unwrap_or_else(|| rgba(0x85858c44).into());
                    window.paint_path(translated(path, origin), color.opacity(*opacity));
                }
                Primitive::Dot(position, color) => {
                    chart_dot(
                        window,
                        *position + origin,
                        2.,
                        colors[*color],
                        cx.theme().background,
                    );
                }
            }
        }
    }
}

// Only scene geometry is translated; the cached tessellation stays local to the
// canvas. GPUI applies the current clipping mask, display scale and opacity.
fn translated(path: &gpui::Path<Pixels>, origin: gpui::Point<Pixels>) -> gpui::Path<Pixels> {
    let mut path = path.clone();
    path.bounds.origin += origin;
    for vertex in &mut path.vertices {
        vertex.xy_position += origin;
    }
    path
}

#[cfg(test)]
mod tests {
    use super::{
        Cache, Data, Geometry, GraphStyle, Layout, MAX_GRAPHS, Point, Primitive, translated,
        visible,
    };
    use crate::gpui::{Bounds, point, px, size};
    use std::rc::Rc;
    fn data() -> Data {
        Data {
            points: Rc::new(vec![
                Point {
                    at: 0.,
                    value: Some(20.),
                    gap: false,
                },
                Point {
                    at: 60.,
                    value: Some(80.),
                    gap: false,
                },
            ]),
            second: Default::default(),
            domain: (0., 60.),
            maximum: 100.,
            style: GraphStyle::Core,
        }
    }
    fn layout() -> Layout {
        Layout {
            size: size(px(120.), px(40.)),
            scale: 2.,
        }
    }

    #[test]
    fn graphs_retain_shared_series_and_replacements_leave_old_frames_valid() {
        let cache = Cache::default();
        let input = data();
        let points = input.points.clone();
        let first = cache.get("history".into(), input);
        let geometry = first.geometry(layout());
        assert!(Rc::ptr_eq(&points, &first.data.points));
        let mut next = data();
        next.points = points.clone();
        let retained = cache.get("history".into(), next);
        assert!(Rc::ptr_eq(&first, &retained));
        assert!(Rc::ptr_eq(&points, &retained.data.points));
        let mut changed = data();
        changed.points = points.clone();
        Rc::make_mut(&mut changed.points)[0].value = Some(99.);
        let replacement = cache.get("history".into(), changed);
        assert!(!Rc::ptr_eq(&first, &replacement));
        assert_eq!(points[0].value, Some(20.));
        assert!(Rc::ptr_eq(&geometry, &first.geometry(layout())));
        assert!(!Rc::ptr_eq(&geometry, &replacement.geometry(layout())));
    }

    #[test]
    fn scrolling_reuses_geometry_but_data_dimensions_and_scale_invalidate_it() {
        let cache = Cache::default();
        let first = cache.get("core-0".into(), data());
        let mesh = first.geometry(layout());
        for _ in 0..100 {
            let same = cache.get("core-0".into(), data());
            assert!(Rc::ptr_eq(&first, &same));
            assert!(Rc::ptr_eq(&mesh, &same.geometry(layout())));
        }
        let resized = first.geometry(Layout {
            size: size(px(121.), px(40.)),
            ..layout()
        });
        assert!(!Rc::ptr_eq(&mesh, &resized));
        let scaled = first.geometry(Layout {
            scale: 1.,
            ..layout()
        });
        assert!(!Rc::ptr_eq(&resized, &scaled));
        let mut changed = data();
        Rc::make_mut(&mut changed.points)[0].value = None;
        let updated = cache.get("core-0".into(), changed);
        assert!(!Rc::ptr_eq(&first, &updated));
        assert!(!Rc::ptr_eq(&mesh, &updated.geometry(layout())));
        assert_eq!(cache.0.borrow().len(), 1);
    }

    #[test]
    fn chart_masks_and_translations_follow_scroll_without_mutating_cached_vertices() {
        let viewport = Bounds::new(point(px(0.), px(0.)), size(px(400.), px(300.)));
        assert!(!visible(
            Bounds::new(point(px(10.), px(301.)), layout().size),
            viewport
        ));
        assert!(visible(
            Bounds::new(point(px(10.), px(290.)), layout().size),
            viewport
        ));
        let geometry = Geometry::build(&data(), layout());
        let Primitive::Path(path, _, _) = &geometry.0[0] else {
            panic!("grid path expected");
        };
        let origin = point(px(25.), px(-10.));
        let moved = translated(path, origin);
        assert_eq!(moved.bounds.origin, path.bounds.origin + origin);
        assert_eq!(moved.bounds.size, path.bounds.size);
        for (before, after) in path.vertices.iter().zip(&moved.vertices) {
            assert_eq!(after.xy_position, before.xy_position + origin);
            assert_eq!(after.st_position, before.st_position);
        }
    }

    #[test]
    fn geometry_retention_is_bounded() {
        let cache = Cache::default();
        for core in 0..MAX_GRAPHS * 2 {
            cache.get(format!("core-{core}"), data());
        }
        assert_eq!(cache.0.borrow().len(), MAX_GRAPHS);
    }

    #[test]
    fn scrolling_past_cache_capacity_reuses_the_current_viewport_geometry() {
        let cache = Cache::default();
        for start in (0..MAX_GRAPHS * 3).step_by(32) {
            let visible: Vec<_> = (start..start + 32)
                .map(|core| {
                    let key = format!("core-{core}");
                    let graph = cache.get(key.clone(), data());
                    let geometry = graph.geometry(layout());
                    (key, graph, geometry)
                })
                .collect();
            for _ in 0..3 {
                for (key, graph, geometry) in &visible {
                    let retained = cache.get(key.clone(), data());
                    assert!(Rc::ptr_eq(graph, &retained), "uncached visible chart {key}");
                    assert!(Rc::ptr_eq(geometry, &retained.geometry(layout())));
                }
            }
        }
    }

    #[test]
    fn recently_visible_charts_survive_eviction_and_old_elements_remain_valid() {
        let cache = Cache::default();
        let oldest = cache.get("core-0".into(), data());
        let oldest_geometry = oldest.geometry(layout());
        for core in 1..MAX_GRAPHS {
            cache.get(format!("core-{core}"), data());
        }
        let hot = cache.get("core-1".into(), data());
        let hot_geometry = hot.geometry(layout());
        cache.get("new-core".into(), data());
        assert_eq!(cache.0.borrow().len(), MAX_GRAPHS);
        assert!(Rc::ptr_eq(&hot, &cache.get("core-1".into(), data())));
        assert!(Rc::ptr_eq(&hot_geometry, &hot.geometry(layout())));
        assert!(!Rc::ptr_eq(&oldest, &cache.get("core-0".into(), data())));
        // A canvas owns its graph for the current frame, even after eviction.
        assert!(Rc::ptr_eq(&oldest_geometry, &oldest.geometry(layout())));
        let mut changed = data();
        Rc::make_mut(&mut changed.points)[0].value = Some(99.);
        let updated = cache.get("core-1".into(), changed);
        assert!(!Rc::ptr_eq(&hot, &updated));
        assert!(!Rc::ptr_eq(&hot_geometry, &updated.geometry(layout())));
        assert_eq!(cache.0.borrow().len(), MAX_GRAPHS);
    }
}
