//! Tiny resource charts beneath the container's current readings.
//! Sampling and retention belong to the shared client; this module only paints.
use super::*;
use newport_client::containers_stats::{StatsSample, history_duration_label};
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
struct Point {
    at: Instant,
    sequence: u64,
    value: f64,
}

pub(super) fn cpu(samples: &[StatsSample<f64>], cx: &App) -> Option<AnyElement> {
    chart(samples, |value| value, 100., "CPU", cx)
}

pub(super) fn memory(
    samples: &[StatsSample<u64>],
    limit: Option<u64>,
    cx: &App,
) -> Option<AnyElement> {
    chart(
        samples,
        |value| value as f64,
        limit.unwrap_or(0) as f64,
        "Memory",
        cx,
    )
}

fn chart<T: Copy>(
    samples: &[StatsSample<T>],
    value: impl Fn(T) -> f64,
    limit: f64,
    name: &str,
    cx: &App,
) -> Option<AnyElement> {
    if samples.len() < 2 {
        return None;
    }
    let values: Vec<_> = samples
        .iter()
        .map(|sample| Point {
            at: sample.received_at,
            sequence: sample.sequence,
            value: value(sample.value),
        })
        .collect();
    let label = format!("{name} over the last {}", history_duration_label(samples));
    let selector = format!("container-{}-spark", name.to_lowercase());
    let color: Hsla = theme::chart_primary(cx.theme().is_dark());
    Some(
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .role(accesskit::Role::Image)
            .aria_label(label)
            .w(px(84.))
            .h(px(18.))
            .mt(px(3.))
            .flex_shrink_0()
            .test_support()
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        for points in positions(&values, limit, bounds) {
                            let Some(first) = points.first().copied() else {
                                continue;
                            };
                            if points.len() == 1 {
                                // An isolated reading is a dot, never an area spanning
                                // unavailable samples or time spent away from the page.
                                window.paint_quad(fill(
                                    Bounds::new(
                                        first - point(px(0.75), px(0.75)),
                                        size(px(1.5), px(1.5)),
                                    ),
                                    color,
                                ));
                                continue;
                            }
                            let mut area = PathBuilder::fill();
                            area.move_to(point(first.x, bounds.bottom()));
                            let mut line = PathBuilder::stroke(px(1.25));
                            line.move_to(first);
                            for position in &points {
                                area.line_to(*position);
                            }
                            for position in points.iter().skip(1) {
                                line.line_to(*position);
                            }
                            area.line_to(point(points.last().unwrap().x, bounds.bottom()));
                            area.close();
                            if let Ok(area) = area.build() {
                                window.paint_path(area, color.opacity(0.22));
                            }
                            if let Ok(line) = line.build() {
                                window.paint_path(line, color);
                            }
                        }
                    },
                )
                .size_full(),
            )
            .into_any_element(),
    )
}

fn positions(
    values: &[Point],
    limit: f64,
    bounds: Bounds<Pixels>,
) -> Vec<Vec<gpui_kit::Point<Pixels>>> {
    let Some(first) = values.first() else {
        return vec![];
    };
    let elapsed = values
        .last()
        .unwrap()
        .at
        .saturating_duration_since(first.at)
        .as_secs_f64();
    let peak = values.iter().map(|point| point.value).fold(0., f64::max);
    // Docker CPU may exceed100% on multiple cores. Keep that visible, and use
    // the memory limit when present; unbounded memory has25% headroom like Tauri.
    let top = if limit > 0. {
        limit.max(peak)
    } else {
        peak.max(1e-9) * 1.25
    };
    let mut runs: Vec<Vec<_>> = vec![];
    for (index, value) in values.iter().enumerate() {
        let previous = index.checked_sub(1).map(|at| values[at]);
        if previous.is_none_or(|previous| {
            value.sequence != previous.sequence.saturating_add(1)
                || value.at.saturating_duration_since(previous.at) > Duration::from_secs(15)
        }) {
            runs.push(vec![]);
        }
        let x = if elapsed > 0. {
            value.at.saturating_duration_since(first.at).as_secs_f64() / elapsed
        } else {
            index as f64 / values.len().saturating_sub(1).max(1) as f64
        };
        let y = (17. - (value.value / top).clamp(0., 1.) * 15.) / 18.;
        runs.last_mut().unwrap().push(point(
            bounds.left() + bounds.size.width * x as f32,
            bounds.top() + bounds.size.height * y as f32,
        ));
    }
    runs
}

#[cfg(test)]
#[path = "container_spark_tests.rs"]
mod tests;
