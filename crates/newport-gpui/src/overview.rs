use super::*;
#[path = "overview_cores.rs"]
mod core_render;
#[path = "overview_graph.rs"]
mod graph_render;
use gpui_kit::component::{
    Selectable,
    button::{ButtonCustomVariant, ButtonGroup},
    progress::Progress,
};
use newport_protocol::{desktop::metrics, monitor::metrics::Snapshot};

#[derive(Default)]
pub(super) struct OverviewState {
    pub(super) graphs: graph_render::Cache,
    pub(super) cores: core_render::Cache,
    pub cached_reads: Option<newport_client::resources::ResourceGroup>,
    pub preferences: std::collections::BTreeMap<String, Preferences>,
    pub inspection: Option<(String, f64)>,
    pub chart_focus: Option<FocusHandle>,
    pub keyboard_inspection: bool,
    pub(super) charts: std::cell::RefCell<Option<(String, std::rc::Rc<PreparedCharts>)>>,
    pub processes: std::collections::BTreeMap<String, overview_process::State>,
    pub interfaces: std::collections::BTreeMap<String, String>,
    /// The latest reading; read it through [`Newport::overview_reading`].
    pub(super) reading: Option<metrics::Reading>,
    /// Recorded history; read it through [`Newport::overview_history`].
    pub(super) history: Option<metrics::History>,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(default)]
pub(super) struct Preferences {
    pub metric: usize,
    pub minutes: u64,
    pub virtual_open: bool,
    pub sort: usize,
    pub ascending: bool,
    pub process_filter: String,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            metric: 0,
            minutes: 5,
            virtual_open: false,
            sort: 0,
            ascending: false,
            process_filter: String::new(),
        }
    }
}
impl Preferences {
    pub fn normalize(&mut self) {
        if self.metric >= 4 {
            self.metric = 0;
        }
        if !RANGES.iter().any(|(minutes, _)| *minutes == self.minutes) {
            self.minutes = 5;
        }
        if self.sort >= 6 {
            self.sort = 0;
            self.ascending = false;
        }
        if self.process_filter.len() > 4096 || self.process_filter.contains('\0') {
            self.process_filter.clear();
        }
    }
}
impl OverviewState {
    pub fn prefs(&self, server: &str) -> Preferences {
        self.preferences.get(server).cloned().unwrap_or_default()
    }
}
const NAMES: [&str; 4] = ["CPU", "Memory", "Load", "Network"];
const RANGES: [(u64, &str); 5] = [
    (5, "5m"),
    (15, "15m"),
    (60, "1h"),
    (1440, "24h"),
    (10080, "7d"),
];
type Series = std::rc::Rc<Vec<Point>>;

pub(super) struct PreparedCharts {
    plots: Vec<Series>,
    tx: Series,
    sparklines: Vec<Series>,
    spark_tx: Series,
    history_plots: Vec<Series>,
    history_tx: Series,
}

#[derive(Clone, Debug, PartialEq)]
struct Point {
    at: f64,
    value: Option<f64>,
    gap: bool,
}
fn resource_reading(data: Option<&Snapshot>, metric: usize) -> Option<f64> {
    let data = data?;
    let finite = |value: f64| Some(value).filter(|v| v.is_finite() && *v >= 0.);
    match metric {
        0 => finite(data.cpu),
        1 => {
            let total = data.memory_total as f64;
            (total > 0.).then(|| data.memory_used as f64 / total * 100.)
        }
        2 => {
            let cores = f64::from(data.cores);
            (cores > 0.)
                .then(|| finite(data.load[0]).map(|load| load / cores * 100.))
                .flatten()
        }
        _ => None,
    }
}
fn resource_level(value: Option<f64>, metric: usize) -> Option<&'static str> {
    let value = value?;
    let (warning, critical) = match metric {
        0 => (80., 95.),
        1 => (85., 95.),
        2 => (100., 200.),
        4 => (50., 90.),
        _ => return None,
    };
    if value >= critical {
        Some("Critical")
    } else if value >= warning {
        Some("High")
    } else {
        None
    }
}
fn card_level(data: Option<&Snapshot>, metric: usize) -> Option<&'static str> {
    let direct = resource_level(resource_reading(data, metric), metric);
    if metric != 1 {
        return direct;
    }
    let swap = data
        .filter(|data| data.swap_total > 0)
        .map(|data| data.swap_used as f64 / data.swap_total as f64 * 100.);
    let swap = resource_level(swap, 4);
    if direct == Some("Critical") || swap == Some("Critical") {
        Some("Critical")
    } else {
        direct.or(swap)
    }
}
fn interface_rank(name: &str) -> usize {
    if name == "lo"
        || name.starts_with("lo:")
        || [
            "veth", "br-", "docker", "virbr", "vnet", "tap", "cni", "flannel", "cali", "cilium",
            "vxlan", "weave", "kube-", "podman", "lxc", "lxd", "lxcbr", "ovs-", "dummy", "ifb",
            "nlmon", "gre", "sit", "ip6tnl",
        ]
        .iter()
        .any(|prefix| name.starts_with(prefix))
    {
        2
    } else if [
        "tailscale",
        "wg",
        "tun",
        "utun",
        "zt",
        "ppp",
        "ipsec",
        "nebula",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
    {
        1
    } else {
        0
    }
}
/// Named interfaces, ordinary ones first and busiest first within a rank.
/// `traffic` is an interface's bytes received and sent.
fn ordered_interfaces<T>(
    mut interfaces: Vec<T>,
    name: impl Fn(&T) -> &str,
    traffic: impl Fn(&T) -> f64,
) -> Vec<T> {
    interfaces.retain(|value| !name(value).is_empty());
    interfaces.sort_by(|a, b| {
        let (a_name, b_name) = (name(a), name(b));
        interface_rank(a_name)
            .cmp(&interface_rank(b_name))
            .then_with(|| traffic(b).total_cmp(&traffic(a)))
            .then_with(|| a_name.cmp(b_name))
    });
    interfaces
}
/// The interfaces the network chart can draw, by name: the latest history
/// sample's, or the reading's before any history arrives.
fn chart_interfaces<'a>(
    history: &'a [metrics::Sample],
    data: Option<&'a Snapshot>,
) -> Vec<&'a str> {
    match history.last() {
        Some(sample) => ordered_interfaces(
            sample.value.network().iter().collect(),
            |row| row.name.as_str(),
            |row| counter(row.received) + counter(row.sent),
        )
        .into_iter()
        .map(|row| row.name.as_str())
        .collect(),
        None => ordered_interfaces(
            data.map_or(&[][..], |data| &data.network).iter().collect(),
            |row| row.name.as_str(),
            |row| row.received as f64 + row.sent as f64,
        )
        .into_iter()
        .map(|row| row.name.as_str())
        .collect(),
    }
}
fn selected_interface<'a>(interfaces: &[&'a str], preferred: Option<&String>) -> &'a str {
    interfaces
        .iter()
        .find(|name| Some(**name) == preferred.map(String::as_str))
        .or_else(|| interfaces.first())
        .copied()
        .unwrap_or("")
}
/// A recorded counter, or zero when it wasn't recorded.
fn counter(value: Option<u64>) -> f64 {
    value.map_or(0., |value| value as f64)
}
/// A recorded measure, or zero when it wasn't recorded or isn't finite.
fn number(value: Option<f64>) -> f64 {
    value.filter(|v| v.is_finite()).unwrap_or(0.)
}
fn ratio(used: f64, total: f64) -> f64 {
    if total > 0. { used / total * 100. } else { 0. }
}
fn rate(n: f64) -> String {
    if n >= 1048576. {
        format!("{:.1} MiB/s", n / 1048576.)
    } else if n >= 1024. {
        format!("{:.1} KiB/s", n / 1024.)
    } else {
        format!("{n:.0} B/s")
    }
}
fn network_axis(value: f64, peak: f64) -> String {
    let (scale, unit) = if peak >= 1_048_576. {
        (1_048_576., "MiB/s")
    } else if peak >= 1024. {
        (1024., "KiB/s")
    } else {
        (1., "B/s")
    };
    let value = value / scale;
    if value < 10. {
        format!("{value:.1} {unit}")
    } else {
        format!("{value:.0} {unit}")
    }
}
fn series(
    history: &[metrics::Sample],
    metric: usize,
    interface: &str,
    outgoing: bool,
) -> Vec<Point> {
    let resolution =
        |value: &metrics::SampleValue| value.resolution_ms().map_or(0., |ms| ms as f64);
    // Chart and minute buckets carry rates; raw samples carry counters.
    let carries_rates = |value: &metrics::SampleValue| {
        matches!(value, metrics::SampleValue::Chart(_)) || value.resolution_ms() == Some(60_000)
    };
    history
        .iter()
        .enumerate()
        .map(|(i, sample)| {
            let data = &sample.value;
            let measures = data.measures();
            let previous = i.checked_sub(1).map(|j| &history[j]);
            let elapsed = previous
                .map(|p| sample.at as f64 - p.at as f64)
                .unwrap_or(0.);
            let gap = data.gap()
                || previous.is_some_and(|p| {
                    p.value.gap()
                        || elapsed > 30000f64.max(resolution(data)).max(resolution(&p.value))
                });
            let value = match metric {
                0 => measures.cpu,
                1 => (number(measures.memory_total) > 0.)
                    .then(|| ratio(number(measures.memory_used), number(measures.memory_total))),
                2 => measures.load_one,
                _ => {
                    let current = data.network().iter().find(|n| n.name == interface);
                    current.and_then(|n| {
                        if carries_rates(data) {
                            return if outgoing { n.tx } else { n.rx };
                        }
                        let old = &previous?.value;
                        if gap
                            || elapsed <= 0.
                            || elapsed > 30000.
                            || carries_rates(old)
                            || number(data.uptime()) < number(old.uptime())
                        {
                            return None;
                        }
                        let old = old.network().iter().find(|n| n.name == interface)?;
                        let delta = if outgoing {
                            n.sent? as f64 - old.sent? as f64
                        } else {
                            n.received? as f64 - old.received? as f64
                        };
                        (delta >= 0.).then_some(delta * 1000. / elapsed)
                    })
                }
            }
            .filter(|v| v.is_finite());
            Point {
                at: sample.at as f64,
                value,
                gap,
            }
        })
        .collect()
}
// Long windows use duration-weighted averages. Empty buckets remain
// missing, so a disconnected period is never drawn as zero activity.
fn history_start(first: Option<f64>, end: f64, minutes: u64) -> f64 {
    let requested = end - minutes as f64 * 60_000.;
    first
        .filter(|at| at.is_finite())
        .map(|at| requested.max(at.min(end - 60_000.)))
        .unwrap_or(requested)
}

fn history_points(
    points: &[Point],
    history: &[metrics::Sample],
    domain: (f64, f64),
    minutes: u64,
) -> Vec<Point> {
    if history
        .iter()
        .any(|sample| matches!(sample.value, metrics::SampleValue::Chart(_)))
    {
        return chart_points(points, 400);
    }
    let interval = match minutes {
        1440 => 300_000.,
        10080 => 1_800_000.,
        _ => return chart_points(points, 400),
    };
    if points.is_empty() || domain.1 <= domain.0 {
        return vec![];
    }
    let count = ((domain.1 - domain.0) / interval).ceil() as usize;
    let mut sums = vec![(0., 0.); count];
    for (point, sample) in points.iter().zip(history) {
        if !point.at.is_finite() || point.at < domain.0 || point.at > domain.1 {
            continue;
        }
        let Some(value) = point.value.filter(|value| value.is_finite()) else {
            continue;
        };
        let weight = sample
            .value
            .resolution_ms()
            .filter(|weight| *weight > 0)
            .map_or(10_000., |weight| weight as f64);
        let index = (((point.at - domain.0) / interval).floor() as usize).min(count - 1);
        sums[index].0 += value * weight;
        sums[index].1 += weight;
    }
    sums.into_iter()
        .enumerate()
        .map(|(index, (sum, weight))| {
            let start = domain.0 + index as f64 * interval;
            Point {
                at: start + interval.min(domain.1 - start) / 2.,
                value: (weight > 0.).then_some(sum / weight),
                gap: false,
            }
        })
        .collect()
}

// Match the reference chartPoints budget: extrema and gap boundaries survive.
// This is display-only; latest readings and historical values remain untouched.
fn chart_points(points: &[Point], buckets: usize) -> Vec<Point> {
    let buckets = buckets.max(1);
    if points.len() <= buckets.saturating_mul(2) {
        return points.to_vec();
    }
    let size = points.len().div_ceil(buckets);
    let mut keep = vec![false; points.len()];
    for start in (0..points.len()).step_by(size) {
        let end = (start + size).min(points.len());
        keep[start] = true;
        keep[end - 1] = true;
        let (mut low, mut high) = (start, start);
        for i in start..end {
            if points[i].gap && i > 0 {
                keep[i - 1] = true;
                keep[i] = true;
            }
            if let Some(value) = points[i].value {
                if points[low].value.is_none_or(|previous| value < previous) {
                    low = i;
                }
                if points[high].value.is_none_or(|previous| value > previous) {
                    high = i;
                }
            } else {
                if i > 0 && points[i - 1].value.is_some() {
                    keep[i - 1] = true;
                    keep[i] = true;
                }
                if i + 1 < points.len() && points[i + 1].value.is_some() {
                    keep[i] = true;
                    keep[i + 1] = true;
                }
            }
        }
        keep[low] = true;
        keep[high] = true;
    }
    points
        .iter()
        .zip(keep)
        .filter_map(|(point, keep)| keep.then_some(point.clone()))
        .collect()
}

fn graph_runs(points: &[Point], domain: (f64, f64)) -> Vec<Vec<(f64, f64)>> {
    let mut runs = Vec::new();
    let mut run = Vec::new();
    for p in points {
        let value = p.value.filter(|v| v.is_finite());
        let visible = p.at.is_finite() && p.at >= domain.0 && p.at <= domain.1 && value.is_some();
        if (!visible || p.gap) && !run.is_empty() {
            runs.push(std::mem::take(&mut run));
        }
        if visible {
            run.push((p.at, value.unwrap()));
        }
    }
    if !run.is_empty() {
        runs.push(run);
    }
    runs
}
// Preserve the first/last sample and both extrema in each screen column.
// Reduce each continuous run separately so outages never become connecting lines.
fn pixel_envelope(run: Vec<(f64, f64)>, domain: (f64, f64), columns: usize) -> Vec<(f64, f64)> {
    let columns = columns.max(1);
    if run.len() <= columns.saturating_mul(4) || domain.1 <= domain.0 {
        return run;
    }
    let bucket = |at: f64| {
        (((at - domain.0) / (domain.1 - domain.0) * columns as f64).floor() as usize)
            .min(columns - 1)
    };
    let mut reduced = Vec::with_capacity(columns.saturating_mul(4).min(run.len()));
    let mut start = 0;
    while start < run.len() {
        let column = bucket(run[start].0);
        let mut end = start + 1;
        let (mut low, mut high) = (start, start);
        while end < run.len() && bucket(run[end].0) == column {
            if run[end].1 < run[low].1 {
                low = end;
            }
            if run[end].1 > run[high].1 {
                high = end;
            }
            end += 1;
        }
        let mut indices = [start, low, high, end - 1];
        indices.sort_unstable();
        let mut previous = None;
        for index in indices {
            if previous != Some(index) {
                reduced.push(run[index]);
            }
            previous = Some(index);
        }
        start = end;
    }
    reduced
}
fn chart_dot(
    window: &mut Window,
    position: gpui::Point<Pixels>,
    radius: f32,
    color: Hsla,
    border: Hsla,
) {
    window.paint_quad(gpui::quad(
        Bounds::new(
            position - point(px(radius), px(radius)),
            gpui::size(px(radius * 2.), px(radius * 2.)),
        ),
        px(radius),
        color,
        px(if radius >= 4. { 2. } else { 0. }),
        border,
        gpui::BorderStyle::Solid,
    ));
}
// Vector paths retain every discontinuity and use actual sample times, not an
// evenly spaced category axis.
#[derive(Clone, Copy, PartialEq)]
enum GraphStyle {
    Sparkline,
    History,
    Core,
}

// Charts contain straight segments. Emit their dashes directly instead of
// asking Lyon's path sampler to split a path across discontinuous subpaths
// (which can panic at an End event). Phase continues around corners, but each
// data run starts separately so an outage never becomes a connecting stroke.
fn dashed_run(
    positions: &[gpui::Point<Pixels>],
    pattern: (f64, f64),
    mut emit: impl FnMut(bool, gpui::Point<Pixels>),
) {
    debug_assert!(pattern.0 > 0. && pattern.1 > 0.);
    let mut drawing = true;
    let mut remaining = pattern.0;
    let mut pen_down = false;
    for pair in positions.windows(2) {
        let from = pair[0];
        let to = pair[1];
        let x: f32 = (to.x - from.x).into();
        let y: f32 = (to.y - from.y).into();
        let length = f64::from(x).hypot(f64::from(y));
        if !length.is_finite() || length <= f64::EPSILON {
            continue;
        }
        let at = |distance: f64| {
            let fraction = (distance / length) as f32;
            point(from.x + px(x * fraction), from.y + px(y * fraction))
        };
        let mut offset = 0.;
        while offset < length {
            let next = (offset + remaining).min(length);
            if next <= offset {
                break;
            }
            if drawing {
                if !pen_down {
                    emit(true, at(offset));
                }
                emit(false, at(next));
                pen_down = true;
            }
            remaining -= next - offset;
            offset = next;
            if remaining <= 1e-9 {
                drawing = !drawing;
                remaining = if drawing { pattern.0 } else { pattern.1 };
                if !drawing {
                    pen_down = false;
                }
            }
        }
    }
}

fn nearest_sample(points: &[Point], domain: (f64, f64), fraction: f64) -> Option<&Point> {
    if !fraction.is_finite() || domain.1 <= domain.0 {
        return None;
    }
    let at = domain.0 + fraction.clamp(0., 1.) * (domain.1 - domain.0);
    points
        .iter()
        .filter(|p| p.at >= domain.0 && p.at <= domain.1)
        .min_by(|a, b| (a.at - at).abs().total_cmp(&(b.at - at).abs()))
}
fn step_sample(
    points: &[Point],
    domain: (f64, f64),
    current: Option<f64>,
    key: &str,
) -> Option<f64> {
    let times = points
        .iter()
        .map(|p| p.at)
        .filter(|at| at.is_finite() && *at >= domain.0 && *at <= domain.1)
        .collect::<Vec<_>>();
    let first = *times.first()?;
    let last = *times.last()?;
    match key {
        "home" => Some(first),
        "end" => Some(last),
        "left" => Some(
            current
                .and_then(|at| times.iter().rev().find(|t| **t < at).copied())
                .unwrap_or(if current.is_some() { first } else { last }),
        ),
        "right" => Some(
            current
                .and_then(|at| times.iter().find(|t| **t > at).copied())
                .unwrap_or(last),
        ),
        _ => None,
    }
}
impl Newport {
    /// The latest metrics reading, while its read is shown.
    pub(super) fn overview_reading(&self) -> Option<&metrics::Reading> {
        self.owned("overview", &self.overview_state.reading)
    }
    /// Recorded metrics history, oldest first, while its read is shown.
    pub(super) fn overview_history(&self) -> &[metrics::Sample] {
        self.owned("history", &self.overview_state.history)
            .map_or(&[], |history| &history.samples)
    }
    /// Settles a metrics reading or history read.
    pub(super) fn complete_metrics(
        &mut self,
        key: &str,
        generation: u64,
        result: Result<desktop::Reply, newport_client::request_error::RequestError>,
    ) -> bool {
        let reply = result.and_then(|reply| match reply {
            desktop::Reply::Metrics(reply) => Ok(*reply),
            _ => Err("The backend answered a metrics read with another reply.".into()),
        });
        match key {
            "overview" => {
                let reading = reply.and_then(|reply| match reply {
                    metrics::Reply::Collect(reading) => Ok(*reading),
                    _ => Err("The backend answered a metrics read with another reply.".into()),
                });
                let reading = reading.map(|reading| {
                    let changed = self.overview_state.reading.as_ref() != Some(&reading);
                    (reading, changed)
                });
                self.complete_owned(key, generation, reading, |this| {
                    &mut this.overview_state.reading
                })
            }
            _ => {
                let history = reply.and_then(|reply| match reply {
                    metrics::Reply::History(history) => Ok(*history),
                    _ => Err("The backend answered a history read with another reply.".into()),
                });
                let history = history.map(|history| {
                    let changed = self.overview_state.history.as_ref() != Some(&history);
                    (history, changed)
                });
                self.complete_owned(key, generation, history, |this| {
                    &mut this.overview_state.history
                })
            }
        }
    }
    fn inspectable_history(
        &self,
        drawing: (Series, Series),
        inspection: (Series, Series),
        domain: (f64, f64),
        maximum: f64,
        key: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (points, second) = inspection;
        let metric = self.overview_state.prefs(&self.server).metric;
        let core_capacity = self
            .overview_reading()
            .map(|reading| f64::from(reading.sample.cores))
            .filter(|cores| metric == 2 && *cores > 0.);
        let selected = self
            .overview_state
            .inspection
            .as_ref()
            .filter(|(k, _)| k == &key)
            .and_then(|(_, at)| {
                points
                    .iter()
                    .find(|p| p.at == *at && p.at >= domain.0 && p.at <= domain.1)
            });
        let fraction =
            selected.map(|p| ((p.at - domain.0) / (domain.1 - domain.0).max(1.)).clamp(0., 1.));
        let markers = selected
            .map(|p| {
                [
                    p.value,
                    second
                        .iter()
                        .find(|other| other.at == p.at)
                        .and_then(|other| other.value),
                ]
            })
            .unwrap_or([None, None]);
        let format_value = |value: Option<f64>| {
            value
                .map(|v| {
                    if metric == 3 {
                        rate(v)
                    } else if metric < 2 {
                        format!("{v:.1}%")
                    } else {
                        format!("{v:.2}")
                    }
                })
                .unwrap_or_else(|| "Unavailable".into())
        };
        let selection_description = selected
            .map(|p| {
                let timestamp = chrono::DateTime::from_timestamp_millis(p.at as i64)
                    .map(|d| {
                        d.with_timezone(&chrono::Local)
                            .format("%b %e, %H:%M:%S")
                            .to_string()
                    })
                    .unwrap_or_default();
                if metric == 3 {
                    format!(
                        " Selected {timestamp}: receive {}, send {}.",
                        format_value(markers[0]),
                        format_value(markers[1])
                    )
                } else {
                    format!(" Selected {timestamp}: {}.", format_value(p.value))
                }
            })
            .unwrap_or_default();
        let tooltip = selected.map(|p| {
            let timestamp = chrono::DateTime::from_timestamp_millis(p.at as i64)
                .map(|date| {
                    date.with_timezone(&chrono::Local)
                        .format("%b %e, %H:%M:%S")
                        .to_string()
                })
                .unwrap_or_else(|| "Unknown time".into());
            div()
                .absolute()
                .top(px(8.))
                .when(fraction.unwrap_or(0.) > 0.5, |v| v.left(px(8.)))
                .when(fraction.unwrap_or(0.) <= 0.5, |v| v.right(px(8.)))
                .min_w(px(150.))
                .p(px(10.))
                .rounded(px(crate::ui::RADIUS))
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().background)
                .shadow_sm()
                .child(label(timestamp).text_size(px(crate::ui::FONT_SIZE)))
                .child(
                    div()
                        .mt_1()
                        .text_size(px(crate::ui::FONT_SIZE))
                        .child(format!(
                            "{} · {}",
                            if metric == 3 {
                                "Receive"
                            } else {
                                NAMES[metric]
                            },
                            format_value(p.value)
                        )),
                )
                .when(metric == 3, |v| {
                    v.child(
                        div()
                            .mt_1()
                            .text_size(px(crate::ui::FONT_SIZE))
                            .child(format!(
                                "Send · {}",
                                format_value(
                                    second
                                        .iter()
                                        .find(|other| other.at == p.at)
                                        .and_then(|other| other.value)
                                )
                            )),
                    )
                })
        });
        let bounds = std::rc::Rc::new(std::cell::Cell::new(None::<Bounds<Pixels>>));
        let measured = bounds.clone();
        let move_key = key.clone();
        let hit_points = points.clone();
        let key_points = points.clone();
        let key_identity = key.clone();
        div()
            .id("overview-history-inspection")
            .role(accesskit::Role::Group)
            .aria_label(format!(
                "{} history. Left and Right inspect samples; Home and End jump; Escape dismisses.{selection_description}",
                NAMES[metric]
            ))
            .when_some(self.overview_state.chart_focus.as_ref(), |view, focus| {
                view.track_focus(focus).tab_stop(true)
            })
            .focus_visible(|view| {
                view.shadow(vec![gpui::BoxShadow {
                    color: cx.theme().primary.opacity(0.6),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(2.),
                    inset: false,
                }])
            })
            .on_click(cx.listener(|this, _, window, cx| {
                if let Some(focus) = &this.overview_state.chart_focus {
                    focus.focus(window, cx);
                }
            }))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.modifiers.platform
                    || event.keystroke.modifiers.control
                    || event.keystroke.modifiers.alt
                {
                    return;
                }
                let name = event.keystroke.key.as_str();
                if !matches!(name, "left" | "right" | "home" | "end" | "escape") {
                    return;
                }
                let current = this
                    .overview_state
                    .inspection
                    .as_ref()
                    .filter(|(k, _)| k == &key_identity)
                    .map(|(_, at)| *at);
                this.overview_state.inspection = if name == "escape" {
                    None
                } else {
                    step_sample(&key_points, domain, current, name)
                        .map(|at| (key_identity.clone(), at))
                };
                this.overview_state.keyboard_inspection = name != "escape";
                window.prevent_default();
                cx.stop_propagation();
                cx.notify();
            }))
            .relative()
            .bg(cx.theme().background)
            .size_full()
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                this.overview_state.keyboard_inspection = false;
                let Some(bounds) = bounds.get() else {
                    return;
                };
                let width: f32 = bounds.size.width.into();
                if width <= 0. {
                    return;
                }
                let x: f32 = (event.position.x - bounds.left()).into();
                let next = nearest_sample(&hit_points, domain, (x / width) as f64)
                    .map(|p| (move_key.clone(), p.at));
                if this.overview_state.inspection != next {
                    this.overview_state.inspection = next;
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if !*hovered
                    && !this.overview_state.keyboard_inspection
                    && this
                        .overview_state
                        .inspection
                        .as_ref()
                        .is_some_and(|(k, _)| k == &key)
                {
                    this.overview_state.inspection = None;
                    cx.notify();
                }
            }))
            .child(self.overview_state.graphs.element("history", drawing, domain, maximum, GraphStyle::History))
            .when_some(core_capacity, |view, cores| {
                view.child(
                    div().absolute().left_0().right_0()
                        .top(relative((1. - cores / maximum.max(1.)).clamp(0., 1.) as f32))
                        .border_t_1().border_color(cx.theme().muted_foreground.opacity(0.5))
                        .flex().justify_end()
                        .child(label(format!("{cores:.0} {}", if cores == 1. { "core" } else { "cores" }))
                            .text_size(px(crate::ui::FONT_SIZE)).px_1().pt_1()),
                )
            })
            .child(
                canvas(
                    move |bounds, _, _| {
                        measured.set(Some(bounds));
                    },
                    move |bounds, _, window, cx| {
                        if let Some(fraction) = fraction {
                            let x = bounds.left() + bounds.size.width * fraction as f32;
                            let mut path = PathBuilder::stroke(px(1.));
                            path.move_to(point(x, bounds.top()));
                            path.line_to(point(x, bounds.bottom()));
                            if let Ok(path) = path.build() {
                                window.paint_path(path, cx.theme().muted_foreground.opacity(0.6));
                            }
                            for (index, value) in markers.iter().enumerate() {
                                if let Some(value) = value.filter(|v| v.is_finite()) {
                                    let y = bounds.top()
                                        + bounds.size.height
                                            * (1. - (value / maximum.max(1.)).clamp(0., 1.)) as f32;
                                    let color = if index == 0 {
                                        crate::theme::chart_primary(cx.theme().is_dark())
                                    } else {
                                        crate::theme::chart_secondary(cx.theme().is_dark())
                                    };
                                    chart_dot(
                                        window,
                                        point(x, y),
                                        4.,
                                        color,
                                        cx.theme().background,
                                    );
                                }
                            }
                        }
                    },
                )
                .absolute()
                .size_full()
                .top_0()
                .left_0(),
            )
            .when_some(tooltip, |view, tip| view.child(tip))
            .into_any_element()
    }
    fn overview_storage(&self, data: Option<&Snapshot>, cx: &mut Context<Self>) -> AnyElement {
        let usage = |disk: &newport_protocol::monitor::metrics::Disk| {
            ratio(disk.used as f64, disk.total as f64)
        };
        let mut disks: Vec<_> = data.map_or(&[][..], |data| &data.disks).iter().collect();
        disks.sort_by(|a, b| usage(b).total_cmp(&usage(a)));
        div()
            .mt(px(24.))
            .flex()
            .flex_col()
            .gap_3()
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Storage"))
            .when(disks.is_empty(), |view| {
                view.child(label("Storage information unavailable."))
            })
            .child(
                div()
                    .grid()
                    .grid_cols(((self.content_width - 16.) / 328.).floor().max(1.) as u16)
                    .gap_x(px(28.))
                    .gap_y(px(12.))
                    .children(disks.into_iter().enumerate().map(|(index, disk)| {
                        let used = usage(disk);
                        let known = disk.total > 0;
                        let color = if used >= 95. {
                            theme::error(cx.theme().is_dark())
                        } else if used >= 85. {
                            theme::warning(cx.theme().is_dark())
                        } else {
                            crate::theme::chart_primary(cx.theme().is_dark())
                        };
                        let mount = disk.mount.clone();
                        div()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_size(px(crate::ui::FONT_SIZE))
                                            .child(mount.clone()),
                                    )
                                    .child(
                                        label(if known {
                                            format!(
                                                "{used:.0}% · {} free of {}",
                                                byte_size(disk.available as f64),
                                                byte_size(disk.total as f64)
                                            )
                                        } else {
                                            "Usage unavailable".into()
                                        })
                                        .text_size(px(crate::ui::FONT_SIZE)),
                                    ),
                            )
                            .when(known && used >= 85., |view| {
                                view.child(
                                    label(if used >= 95. {
                                        "Critical · storage nearly full"
                                    } else {
                                        "High storage usage"
                                    })
                                    .text_size(px(crate::ui::FONT_SIZE))
                                    .text_color(color),
                                )
                            })
                            .when(known, |view| {
                                view.child(
                                    Progress::new(("storage-usage", index))
                                        .small()
                                        .value(used as f32)
                                        .color(color)
                                        .accessibility_label(format!("Storage {mount} usage")),
                                )
                            })
                    })),
            )
            .into_any_element()
    }
    fn overview_network_totals(
        &self,
        data: Option<&Snapshot>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let interfaces = ordered_interfaces(
            data.map_or(&[][..], |data| &data.network).iter().collect(),
            |n| n.name.as_str(),
            |n| n.received as f64 + n.sent as f64,
        );
        let (primary, folded): (Vec<_>, Vec<_>) = interfaces
            .iter()
            .copied()
            .partition(|n| interface_rank(&n.name) < 2);
        let open = self.overview_state.prefs(&self.server).virtual_open;
        let total = |count: fn(&newport_protocol::monitor::metrics::Network) -> u64| {
            byte_size(
                folded
                    .iter()
                    .map(|n| count(n))
                    .fold(0u64, u64::saturating_add) as f64,
            )
        };
        let counter = |value: String| div().w(px(120.)).flex_shrink_0().text_right().child(value);
        div()
            .mt(px(24.))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Network totals"),
                    )
                    .child(
                        label("Since each interface started").text_size(px(crate::ui::FONT_SIZE)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .pb_2()
                    .child(div().flex_1().child(label("Interface")))
                    .child(counter("Received".into()))
                    .child(counter("Sent".into())),
            )
            .when(interfaces.is_empty(), |v| {
                v.child(empty::compact("No network interfaces reported.", cx))
            })
            .children(primary.into_iter().map(|n| {
                div()
                    .flex()
                    .gap_2()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(div().flex_1().min_w_0().truncate().child(n.name.clone()))
                    .child(counter(byte_size(n.received as f64)))
                    .child(counter(byte_size(n.sent as f64)))
            }))
            .when(!folded.is_empty(), |v| {
                v.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div().flex_1().min_w_0().flex().child(
                                crate::controls::compact_button("virtual-interfaces")
                                    .toggled(open)
                                    .icon(if open {
                                        IconName::ChevronDown
                                    } else {
                                        IconName::ChevronRight
                                    })
                                    .label(format!("{} virtual and loopback", folded.len()))
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let pref = this
                                            .overview_state
                                            .preferences
                                            .entry(this.server.clone())
                                            .or_default();
                                        pref.virtual_open = !pref.virtual_open;
                                        this.save_workspace_selection();
                                        cx.notify();
                                    })),
                            ),
                        )
                        .child(counter(total(|n| n.received)))
                        .child(counter(total(|n| n.sent))),
                )
            })
            .when(open, |v| {
                v.child(
                    div()
                        .grid()
                        .grid_cols(
                            ((self.content_width - crate::controls::PAGE_PADDING * 2.) / 280.)
                                .floor()
                                .max(1.) as u16,
                        )
                        .gap_x(px(24.))
                        .children(folded.into_iter().map(|n| {
                            div()
                                .flex()
                                .min_w_0()
                                .items_center()
                                .gap_2()
                                .py_2()
                                .border_b_1()
                                .border_color(cx.theme().border)
                                .text_size(px(crate::ui::FONT_SIZE))
                                .child(div().flex_1().min_w_0().truncate().child(n.name.clone()))
                                .child(label(format!("↓ {}", byte_size(n.received as f64))))
                                .child(label(format!("↑ {}", byte_size(n.sent as f64))))
                        })),
                )
            })
            .into_any_element()
    }
    pub(super) fn overview(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.setup_visible() {
            return self.render_server_setup(cx);
        }
        if self.needle_missing() {
            return self.needle_required("Monitoring", cx);
        }
        let reading = self.overview_reading();
        let data = reading.map(|reading| &reading.sample);
        let collection_error = self
            .resources
            .get("overview")
            .and_then(|r| r.error.as_deref())
            .or_else(|| reading.and_then(|reading| reading.collection_error.as_deref()))
            .filter(|error| !error.is_empty());
        // While the connection is down its failures are expected.
        let offline = self.connection_down(cx);
        let collection_error = collection_error.filter(|error| {
            offline.is_none()
                && *error != "Waiting for the background sampler’s first reading. Refresh shortly."
        });
        let history_error = reading
            .and_then(|reading| reading.history_error.as_deref())
            .filter(|error| !error.is_empty())
            .or_else(|| {
                self.resources
                    .get("history")
                    .and_then(|r| r.error.as_deref())
            })
            .filter(|_| offline.is_none());
        let offline = offline
            .filter(|_| data.is_some())
            .map(crate::connection_wait::last_reading);
        let history = self.overview_history();
        let end = history.last().map(|p| p.at as f64).unwrap_or(0.);
        let minutes = self.overview_state.prefs(&self.server).minutes;
        let domain = (
            history_start(history.first().map(|p| p.at as f64), end, minutes),
            end,
        );
        let interfaces = chart_interfaces(history, data);
        let interface = selected_interface(
            &interfaces,
            self.overview_state.interfaces.get(&self.server),
        );
        let cores = data.map_or(0., |data| f64::from(data.cores));
        let load = number(data.map(|data| data.load[0]));
        let revision = self
            .resources
            .get("history")
            .map(|r| r.data_revision)
            .unwrap_or(0);
        let key = format!("{}:{revision}:{minutes}:{interface}", self.server);
        let charts = {
            let mut cache = self.overview_state.charts.borrow_mut();
            if cache.as_ref().is_none_or(|(previous, _)| previous != &key) {
                let plots: Vec<_> = (0..4)
                    .map(|metric| std::rc::Rc::new(series(history, metric, interface, false)))
                    .collect();
                let tx = std::rc::Rc::new(series(history, 3, interface, true));
                let sparklines = plots
                    .iter()
                    .map(|points| std::rc::Rc::new(chart_points(points, 120)))
                    .collect();
                let spark_tx = std::rc::Rc::new(chart_points(&tx, 120));
                let history_plots = plots
                    .iter()
                    .map(|points| {
                        std::rc::Rc::new(history_points(points, history, domain, minutes))
                    })
                    .collect();
                let history_tx = std::rc::Rc::new(history_points(&tx, history, domain, minutes));
                *cache = Some((
                    key,
                    std::rc::Rc::new(PreparedCharts {
                        plots,
                        tx,
                        sparklines,
                        spark_tx,
                        history_plots,
                        history_tx,
                    }),
                ));
            }
            cache.as_ref().unwrap().1.clone()
        };
        let PreparedCharts {
            plots,
            tx,
            sparklines,
            spark_tx,
            history_plots,
            history_tx,
        } = charts.as_ref();
        let latest = |points: &[Point]| {
            points
                .last()
                .and_then(|p| p.value)
                .map(rate)
                .unwrap_or_else(|| "—".into())
        };
        let readings = [
            resource_reading(data, 0),
            resource_reading(data, 1),
            resource_reading(data, 2),
        ];
        let percent = |value: Option<f64>| {
            value
                .map(|v| format!("{v:.0}%"))
                .unwrap_or_else(|| "—".into())
        };
        let values = [
            percent(readings[0]),
            percent(readings[1]),
            percent(readings[2]),
            latest(&plots[3]),
        ];
        let contexts = [
            if data.is_some_and(|data| data.cores > 0) {
                format!("{cores:.0} cores")
            } else {
                "Core count unavailable".into()
            },
            if let Some(data) = data.filter(|_| readings[1].is_some()) {
                format!(
                    "{} of {}",
                    byte_size(data.memory_used as f64),
                    byte_size(data.memory_total as f64)
                )
            } else {
                "Memory unavailable".into()
            },
            if readings[2].is_some() {
                format!("of {cores:.0} cores · load {load:.2}")
            } else {
                "Load unavailable".into()
            },
            format!("↓ in · ↑ out {}", latest(tx)),
        ];
        let maximum = |metric: usize| {
            if metric < 2 {
                100.
            } else if metric == 2 {
                (cores.max(plots[2].iter().filter_map(|p| p.value).fold(0., f64::max)) * 1.15)
                    .max(1.)
            } else {
                plots[3]
                    .iter()
                    .chain(tx.iter())
                    .filter_map(|p| p.value)
                    .fold(1., f64::max)
                    * 1.15
            }
        };
        let metric = self.overview_state.prefs(&self.server).metric;
        let chart_peak = history_plots[metric]
            .iter()
            .chain(if metric == 3 {
                history_tx.iter()
            } else {
                [].iter()
            })
            .filter_map(|point| point.value)
            .fold(0., f64::max);
        let chart_maximum = match metric {
            0 | 1 => 100.,
            2 => (cores.max(chart_peak) * 1.15).max(1.),
            _ => chart_peak.max(1.) * 1.15,
        };

        let cpus = self.overview_state.cores.get(
            &self.server,
            self.resources
                .get("overview")
                .map_or(0, |r| r.data_revision),
            data.map_or(&[], |data| &data.cpus),
        );
        let grid_width = (self.content_width - crate::controls::PAGE_PADDING * 2. - 30.).max(112.);
        let min_width = if cpus.len() > 32 { 76. } else { 112. };
        let columns = ((grid_width + 6.) / (min_width + 6.)).floor().max(1.) as u16;
        let core_height = (grid_width - (columns - 1) as f32 * 6.) / columns as f32 * 2. / 3.;
        let busiest = cpus.iter().map(|cpu| cpu.usage).fold(0., f64::max);

        div()
            .id("overview-scroll")
            .debug_selector(|| "overview-scroll".into())
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .p(px(crate::controls::PAGE_PADDING))
            .flex()
            .flex_col()
            .child(
                div()
                    .mb(px(12.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Resources"))
                    .child(ButtonGroup::new("overview-history-range").children(
                        RANGES.into_iter().map(|(minutes, name)| {
                            crate::controls::button(("range", minutes))
                                .label(name)
                                .selected(
                                    self.overview_state.prefs(&self.server).minutes == minutes,
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.overview_state
                                        .preferences
                                        .entry(this.server.clone())
                                        .or_default()
                                        .minutes = minutes;
                                    this.save_workspace_selection();
                                    this.resources.fence("history");
                                    this.refresh();
                                    cx.notify();
                                }))
                        }),
                    )),
            )
            .when_some(offline, |view, notice| {
                view.child(
                    div()
                        .id("overview-offline")
                        .debug_selector(|| "overview-offline".into())
                        .role(accesskit::Role::Status)
                        .text_size(px(crate::ui::FONT_SIZE))
                        .text_color(cx.theme().muted_foreground)
                        .mb_2()
                        .child(notice),
                )
            })
            .when_some(collection_error, |view, error| {
                view.child(
                    div()
                        .id("overview-error-0")
                        .debug_selector(|| "overview-error-0".into())
                        .role(accesskit::Role::Alert)
                        .text_size(px(crate::ui::FONT_SIZE))
                        .text_color(theme::error(cx.theme().is_dark()))
                        .mb_2()
                        .child(format!(
                            "{error}{}",
                            if data.is_some() {
                                " Showing the last available reading."
                            } else {
                                ""
                            }
                        )),
                )
            })
            .when_some(history_error, |view, error| {
                view.child(
                    div()
                        .id("overview-error-1")
                        .role(accesskit::Role::Alert)
                        .text_size(px(crate::ui::FONT_SIZE))
                        .text_color(theme::error(cx.theme().is_dark()))
                        .mb_2()
                        .child(error.to_owned()),
                )
            })
            .child(
                div()
                    .grid()
                    .grid_cols(loading::overview_columns(self.content_width))
                    .gap(px(10.))
                    .children((0..4).map(|index| {
                        let level = card_level(data, index);
                        let hint = data.and_then(|data| match index {
                            1 if data.swap_total > 0 => Some(format!(
                                "Swap {} of {}",
                                byte_size(data.swap_used as f64),
                                byte_size(data.swap_total as f64)
                            )),
                            2 => Some(format!(
                                "Load average {} over 1 / 5 / 15 minutes",
                                data.load
                                    .iter()
                                    .map(|v| format!("{v:.2}"))
                                    .collect::<Vec<_>>()
                                    .join(" / ")
                            )),
                            _ => None,
                        });
                        let accent = crate::theme::chart_primary(cx.theme().is_dark());
                        div()
                            .min_w_0()
                            .rounded(px(crate::ui::RADIUS))
                            .border_1()
                            .border_color(if metric == index {
                                accent.opacity(0.6)
                            } else {
                                cx.theme().border
                            })
                            .when(metric == index, |card| {
                                card.shadow(vec![gpui::BoxShadow {
                                    color: accent.opacity(0.12),
                                    offset: gpui::point(px(0.), px(0.)),
                                    blur_radius: px(0.),
                                    spread_radius: px(3.),
                                    inset: false,
                                }])
                            })
                            .child(
                                Button::new(("resource", index))
                                    .when_some(hint, |button, hint| button.hover_hint(hint))
                                    .accessibility_label(format!(
                                        "{}: {}{}",
                                        NAMES[index],
                                        values[index],
                                        level.map(|level| format!(", {level}")).unwrap_or_default()
                                    ))
                                    .custom(
                                        ButtonCustomVariant::new(cx)
                                            .color(cx.theme().background)
                                            .hover(cx.theme().background)
                                            .active(cx.theme().background),
                                    )
                                    .selected(metric == index)
                                    .toggled(metric == index)
                                    .w_full()
                                    .h(px(132.))
                                    .p_0()
                                    .rounded(px(crate::ui::RADIUS))
                                    .border_0()
                                    .child(
                                        div()
                                            .size_full()
                                            .px(px(13.))
                                            .pt(px(11.))
                                            .pb(px(8.))
                                            .flex()
                                            .flex_col()
                                            .items_start()
                                            .child(
                                                div()
                                                    .w_full()
                                                    .flex()
                                                    .items_center()
                                                    .justify_between()
                                                    .child(
                                                        label(NAMES[index])
                                                            .text_size(px(crate::ui::FONT_SIZE))
                                                            .font_weight(FontWeight::MEDIUM),
                                                    )
                                                    .when_some(level, |view, level| {
                                                        view.child(
                                                            label(level)
                                                                .text_size(px(crate::ui::FONT_SIZE))
                                                                .text_color(
                                                                    if level == "Critical" {
                                                                        theme::error(
                                                                            cx.theme().is_dark(),
                                                                        )
                                                                    } else {
                                                                        theme::warning(
                                                                            cx.theme().is_dark(),
                                                                        )
                                                                    },
                                                                ),
                                                        )
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_baseline()
                                                    .gap(px(1.))
                                                    .text_size(px(26.))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .child(
                                                        values[index]
                                                            .trim_end_matches('%')
                                                            .split(' ')
                                                            .next()
                                                            .unwrap_or("—")
                                                            .to_owned(),
                                                    )
                                                    .child(
                                                        label(
                                                            if index < 3 && values[index] != "—" {
                                                                "%".to_owned()
                                                            } else {
                                                                values[index]
                                                                    .split_once(' ')
                                                                    .map(|(_, unit)| {
                                                                        unit.to_owned()
                                                                    })
                                                                    .unwrap_or_default()
                                                            },
                                                        )
                                                        .text_size(px(14.)),
                                                    ),
                                            )
                                            .child(
                                                label(contexts[index].clone())
                                                    .text_size(px(crate::ui::FONT_SIZE))
                                                    .truncate(),
                                            )
                                            .child(div().flex_1())
                                            .child(div().w_full().h(px(34.)).child(
                                                self.overview_state.graphs.element(
                                                    format!("spark-{index}"),
                                                    (
                                                        sparklines[index].clone(),
                                                        if index == 3 {
                                                            spark_tx.clone()
                                                        } else {
                                                            Series::default()
                                                        },
                                                    ),
                                                    domain,
                                                    maximum(index),
                                                    GraphStyle::Sparkline,
                                                ),
                                            )),
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.overview_state
                                            .preferences
                                            .entry(this.server.clone())
                                            .or_default()
                                            .metric = index;
                                        this.save_workspace_selection();
                                        cx.notify();
                                    })),
                            )
                    })),
            )
            .when(!cpus.is_empty(), |view| {
                view.child(
                    div()
                        .mt(px(10.))
                        .rounded(px(crate::ui::RADIUS))
                        .border_1()
                        .border_color(cx.theme().border)
                        .px(px(14.))
                        .pt(px(9.))
                        .pb(px(12.))
                        .child(
                            div()
                                .mb(px(8.))
                                .flex()
                                .justify_between()
                                .child(label("Cores"))
                                .child(
                                    label(format!(
                                        "{} online · busiest {busiest:.0}% · last minute",
                                        cpus.len()
                                    ))
                                    .text_size(px(crate::ui::FONT_SIZE)),
                                ),
                        )
                        .child(core_render::grid(
                            cpus.clone(),
                            self.overview_state.graphs.clone(),
                            columns as usize,
                            core_height,
                        )),
                )
            })
            .child(
                div()
                    .mt(px(12.))
                    .rounded(px(crate::ui::RADIUS))
                    .border_1()
                    .border_color(cx.theme().border)
                    .px(px(14.))
                    .pt(px(12.))
                    .pb(px(10.))
                    .flex_shrink_0()
                    .child(
                        div()
                            .mb(px(6.))
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(140.))
                                    .text_size(px(crate::ui::FONT_SIZE))
                                    .child(format!(
                                        "{} · last {}",
                                        NAMES[metric],
                                        RANGES.iter().find(|r| r.0 == minutes).unwrap().1
                                    )),
                            )
                            .when(metric == 3 && interfaces.len() > 1, |view| {
                                let names = interfaces
                                    .iter()
                                    .map(|name| (*name).to_owned())
                                    .collect::<Vec<_>>();
                                let server = self.server.clone();
                                let weak = cx.entity().downgrade();
                                view.child(
                                    crate::controls::button("overview-interface")
                                        .label(interface.to_owned())
                                        .accessibility_label("Graph network interface")
                                        .dropdown_menu(move |mut menu, _, _| {
                                            for name in &names {
                                                let selected = name.clone();
                                                let server = server.clone();
                                                let weak = weak.clone();
                                                let title = if interface_rank(name) == 2 {
                                                    format!("{name} · virtual / loopback")
                                                } else {
                                                    name.clone()
                                                };
                                                menu =
                                                    menu.item(PopupMenuItem::new(title).on_click(
                                                        move |_, _, cx| {
                                                            let _ = weak.update(cx, |this, cx| {
                                                                if this.server != server {
                                                                    return;
                                                                }
                                                                this.overview_state
                                                                    .interfaces
                                                                    .insert(
                                                                        server.clone(),
                                                                        selected.clone(),
                                                                    );
                                                                this.save_workspace_selection();
                                                                cx.notify();
                                                            });
                                                        },
                                                    ));
                                            }
                                            menu
                                        }),
                                )
                            })
                            .when(!self.initial_loading("history"), |view| {
                                view.child(
                                    label(if history.is_empty() {
                                        "No readings yet".into()
                                    } else {
                                        chrono::DateTime::from_timestamp_millis(end as i64)
                                            .map(|date| {
                                                format!(
                                                    "Last reading {}",
                                                    date.with_timezone(&chrono::Local)
                                                        .format("%H:%M:%S")
                                                )
                                            })
                                            .unwrap_or_else(|| "Reading time unavailable".into())
                                    })
                                    .text_size(px(crate::ui::FONT_SIZE))
                                    .font_weight(FontWeight::NORMAL),
                                )
                            }),
                    )
                    .when(self.initial_loading("history"), |view| {
                        view.child(loading::history(metric))
                    })
                    .when(metric == 3 && !history.is_empty(), |view| {
                        view.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_3()
                                .justify_end()
                                .text_size(px(crate::ui::FONT_SIZE))
                                .mb_2()
                                .child(
                                    label(format!("Receive · {}", latest(&plots[3]))).text_color(
                                        crate::theme::chart_primary(cx.theme().is_dark()),
                                    ),
                                )
                                .child(label(format!("Send · {}", latest(tx))).text_color(
                                    crate::theme::chart_secondary(cx.theme().is_dark()),
                                )),
                        )
                    })
                    .when(!history.is_empty(), |view| {
                        view.child(
                            div()
                                .flex()
                                .h(px(208.))
                                .flex_shrink_0()
                                .child(
                                    div()
                                        .w(px(if metric == 3 { 64. } else { 40. }))
                                        .flex_shrink_0()
                                        .flex()
                                        .flex_col()
                                        .justify_between()
                                        .children(
                                            [chart_maximum, chart_maximum / 2., 0.]
                                                .into_iter()
                                                .map(|value| {
                                                    label(if metric == 3 {
                                                        network_axis(value, chart_peak)
                                                    } else if metric < 2 {
                                                        format!("{value:.0}%")
                                                    } else {
                                                        format!("{value:.1}")
                                                    })
                                                    .text_size(px(crate::ui::FONT_SIZE))
                                                }),
                                        ),
                                )
                                .child(div().flex_1().min_w_0().child(self.inspectable_history(
                                    (
                                        history_plots[metric].clone(),
                                        if metric == 3 {
                                            history_tx.clone()
                                        } else {
                                            Series::default()
                                        },
                                    ),
                                    if matches!(minutes, 1440 | 10080) {
                                        (
                                            history_plots[metric].clone(),
                                            if metric == 3 {
                                                history_tx.clone()
                                            } else {
                                                Series::default()
                                            },
                                        )
                                    } else {
                                        (
                                            plots[metric].clone(),
                                            if metric == 3 {
                                                tx.clone()
                                            } else {
                                                Series::default()
                                            },
                                        )
                                    },
                                    domain,
                                    chart_maximum,
                                    format!("{}:{metric}:{minutes}:{interface}", self.server),
                                    cx,
                                ))),
                        )
                    })
                    .when(!history.is_empty(), |view| {
                        view.child(
                            div()
                                .ml(px(if metric == 3 { 64. } else { 40. }))
                                .mt_1()
                                .h(px(14.))
                                .flex_shrink_0()
                                .flex()
                                .justify_between()
                                .children(
                                    [domain.0, (domain.0 + domain.1) / 2., domain.1]
                                        .into_iter()
                                        .map(|at| {
                                            let date =
                                                chrono::DateTime::from_timestamp_millis(at as i64)
                                                    .map(|date| date.with_timezone(&chrono::Local));
                                            let start = chrono::DateTime::from_timestamp_millis(
                                                domain.0 as i64,
                                            )
                                            .map(|date| {
                                                date.with_timezone(&chrono::Local).date_naive()
                                            });
                                            let end = chrono::DateTime::from_timestamp_millis(
                                                domain.1 as i64,
                                            )
                                            .map(|date| {
                                                date.with_timezone(&chrono::Local).date_naive()
                                            });
                                            label(
                                                date.map(|date| {
                                                    date.format(if start != end {
                                                        "%b %-d %H:%M"
                                                    } else {
                                                        "%H:%M"
                                                    })
                                                    .to_string()
                                                })
                                                .unwrap_or_else(|| "—".into()),
                                            )
                                            .text_size(px(crate::ui::FONT_SIZE))
                                        }),
                                ),
                        )
                    })
                    .when(
                        history.is_empty() && !self.initial_loading("history"),
                        |view| {
                            view.child(
                                div()
                                    .h(px(120.))
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(empty::compact(
                                        "No history yet. Readings will appear here as they arrive.",
                                        cx,
                                    )),
                            )
                        },
                    ),
            )
            .child(self.overview_storage(data, cx))
            .child(self.overview_network_totals(data, cx))
            .child(self.overview_processes(data, cx))
            .into_any_element()
    }
}

/// A reading of `sample` with no collection or history errors.
#[cfg(test)]
pub(crate) fn reading(sample: Snapshot) -> metrics::Reading {
    metrics::Reading {
        sample,
        sampled_at: None,
        collection_error: None,
        history_error: None,
    }
}
/// A raw history sample at `at` with only its CPU recorded.
#[cfg(test)]
pub(crate) fn cpu_sample(at: i64, cpu: f64) -> metrics::Sample {
    metrics::Sample {
        at,
        value: metrics::SampleValue::Raw(Box::new(metrics::HistoricalRaw {
            measures: metrics::NumericMeasures {
                cpu: Some(cpu),
                memory_used: None,
                memory_total: None,
                swap_used: None,
                swap_total: None,
                load_one: None,
                load_five: None,
                load_fifteen: None,
            },
            uptime: None,
            network: Vec::new(),
            disks: None,
        })),
    }
}
#[cfg(test)]
impl Newport {
    /// Settles a metrics read with `reply`, as the backend answers it.
    pub(crate) fn put_metrics(&mut self, reply: metrics::Reply) {
        let key = match reply {
            metrics::Reply::Collect(_) => "overview",
            _ => "history",
        };
        let generation = self.resources.begin_refresh(key, false).unwrap();
        assert!(self.complete_metrics(
            key,
            generation,
            Ok(desktop::Reply::Metrics(Box::new(reply)))
        ));
    }
    pub(crate) fn put_reading(&mut self, sample: Snapshot) {
        self.put_metrics(metrics::Reply::Collect(Box::new(reading(sample))));
    }
    pub(crate) fn put_history(&mut self, samples: Vec<metrics::Sample>) {
        self.put_metrics(metrics::Reply::History(Box::new(metrics::History {
            samples,
        })));
    }
}
#[cfg(test)]
#[path = "overview_offline_tests.rs"]
mod offline_tests;
#[cfg(test)]
mod tests {
    use super::{
        Point, card_level, chart_points, graph_runs, history_points, history_start, nearest_sample,
        ordered_interfaces, pixel_envelope, resource_level, resource_reading, selected_interface,
        series, step_sample,
    };
    use newport_protocol::{
        desktop::metrics::{
            ChartBucket, HistoricalRaw, MinuteRollup, NetworkHistory, NumericMeasures, Sample,
            SampleValue,
        },
        monitor::metrics::Snapshot,
    };

    fn measures(cpu: Option<f64>) -> NumericMeasures {
        NumericMeasures {
            cpu,
            memory_used: Some(25.),
            memory_total: Some(100.),
            swap_used: None,
            swap_total: None,
            load_one: Some(2.),
            load_five: None,
            load_fifteen: None,
        }
    }
    fn interface(name: &str, received: u64, sent: u64) -> NetworkHistory {
        NetworkHistory {
            name: name.into(),
            received: Some(received),
            sent: Some(sent),
            rx: None,
            tx: None,
            rx_peak: None,
            tx_peak: None,
        }
    }
    fn raw(
        at: i64,
        measures: NumericMeasures,
        uptime: f64,
        network: Vec<NetworkHistory>,
    ) -> Sample {
        Sample {
            at,
            value: SampleValue::Raw(Box::new(HistoricalRaw {
                measures,
                uptime: Some(uptime),
                network,
                disks: None,
            })),
        }
    }
    /// A summarized bucket whose eth0 interface carries `rx` bytes a second.
    fn bucket(resolution_ms: u64, gap: bool, rx: f64) -> ChartBucket {
        ChartBucket {
            resolution_ms,
            sample_count: 1,
            gap,
            means: measures(Some(12.)),
            peaks: measures(Some(12.)),
            network: vec![NetworkHistory {
                rx: Some(rx),
                ..interface("eth0", 0, 0)
            }],
        }
    }
    fn minute(at: i64, summary: ChartBucket) -> Sample {
        Sample {
            at,
            value: SampleValue::Minute(Box::new(MinuteRollup {
                summary,
                uptime: None,
                disks: None,
            })),
        }
    }
    #[test]
    fn dashed_graph_keeps_phase_around_corners_and_skips_repeated_points() {
        use crate::gpui::{point, px};
        let positions = [
            point(px(0.), px(0.)),
            point(px(4.), px(0.)),
            point(px(4.), px(0.)),
            point(px(4.), px(8.)),
        ];
        let mut commands = vec![];
        super::dashed_run(&positions, (5., 4.), |begin, position| {
            commands.push((begin, position))
        });
        assert_eq!(
            commands,
            vec![
                (true, point(px(0.), px(0.))),
                (false, point(px(4.), px(0.))),
                (false, point(px(4.), px(1.))),
                (true, point(px(4.), px(5.))),
                (false, point(px(4.), px(8.))),
            ]
        );
    }

    #[test]
    fn dashed_graph_handles_gaps_isolated_and_zero_length_runs() {
        use crate::gpui::{PathBuilder, point, px};
        let p = |x, y| point(px(x), px(y));
        let runs = [
            vec![p(0., 0.)],
            vec![p(1., 1.), p(1., 1.)],
            vec![p(2., 0.), p(8., 0.)],
            vec![p(20., 0.), p(30., 0.)],
        ];
        let mut path = PathBuilder::stroke(px(1.4));
        let mut starts = vec![];
        for positions in runs {
            super::dashed_run(&positions, (3., 2.), |begin, position| {
                if begin {
                    starts.push(position);
                    path.move_to(position);
                } else {
                    path.line_to(position);
                }
            });
        }
        assert_eq!(starts, vec![p(2., 0.), p(7., 0.), p(20., 0.), p(25., 0.)]);
        assert!(path.build().is_ok());
    }

    #[test]
    fn reference_sparkline_budget_preserves_extrema_and_gaps() {
        let mut points: Vec<_> = (0..60_480)
            .map(|i| Point {
                at: i as f64,
                value: Some(20.),
                gap: false,
            })
            .collect();
        points[42].value = Some(100.);
        points[43].value = Some(0.);
        points[600].value = None;
        points[900].gap = true;
        let reduced = chart_points(&points, 120);
        assert!(reduced.len() <= 488, "{} points", reduced.len());
        for timestamp in [0., 42., 43., 599., 600., 601., 899., 900., 60_479.] {
            assert!(reduced.iter().any(|point| point.at == timestamp));
        }
        assert_eq!(graph_runs(&reduced, (0., 60_479.)).len(), 3);
        assert!(chart_points(&points, 400).len() <= 1608);
    }
    #[test]
    fn pixel_envelope_preserves_spikes_endpoints_and_time_order() {
        let mut run: Vec<_> = (0..10_000).map(|i| (i as f64, 20.)).collect();
        run[42].1 = 100.;
        run[43].1 = 0.;
        let reduced = pixel_envelope(run.clone(), (0., 9999.), 100);
        assert!(reduced.len() <= 400);
        assert_eq!(reduced.first(), run.first());
        assert_eq!(reduced.last(), run.last());
        assert!(reduced.contains(&(42., 100.)));
        assert!(reduced.contains(&(43., 0.)));
        assert!(reduced.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert_eq!(
            pixel_envelope(run[..3].to_vec(), (0., 9999.), 100),
            run[..3]
        );
    }
    #[test]
    fn pixel_envelope_keeps_disconnected_runs_separate() {
        let points: Vec<_> = (0..100)
            .map(|i| Point {
                at: i as f64,
                value: (i != 50).then_some(i as f64),
                gap: false,
            })
            .collect();
        let runs: Vec<_> = graph_runs(&points, (0., 99.))
            .into_iter()
            .map(|run| pixel_envelope(run, (0., 99.), 1))
            .collect();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].last().unwrap().0, 49.);
        assert_eq!(runs[1].first().unwrap().0, 51.);
    }
    #[test]
    #[ignore = "manual chart preparation benchmark; excludes GPUI layout and paint"]
    fn benchmark_overview_history_render_work() {
        use std::{hint::black_box, time::Instant};
        let history: Vec<Sample> = (0..3600)
            .map(|i: u64| {
                raw(
                    i as i64 * 10_000,
                    NumericMeasures {
                        memory_used: Some(20.),
                        load_one: Some(1.2),
                        ..measures(Some(17.))
                    },
                    i as f64 * 10.,
                    (0..40)
                        .map(|n| interface(&format!("eth{n}"), i * 1000, i * 500))
                        .collect(),
                )
            })
            .collect();
        let prepare = |history: &[Sample]| {
            let plots: Vec<_> = (0..4)
                .map(|metric| series(history, metric, "eth0", false))
                .collect();
            let tx = series(history, 3, "eth0", true);
            let averaged: Vec<_> = plots
                .iter()
                .map(|points| history_points(points, history, (0., 35_990_000.), 10080))
                .collect();
            let averaged_tx = history_points(&tx, history, (0., 35_990_000.), 10080);
            (plots, tx, averaged, averaged_tx)
        };
        let start = Instant::now();
        for _ in 0..10 {
            let value = black_box(history.clone());
            let rows = black_box(value.clone());
            black_box(prepare(&rows));
        }
        let before = start.elapsed();
        let cached = prepare(&history);
        let start = Instant::now();
        for _ in 0..10 {
            black_box(&history);
            // Retain the vector copies currently passed into canvas closures.
            black_box(cached.clone());
        }
        eprintln!(
            "3600 samples / 40 interfaces, 10 repeated renders: previous history preparation {before:?}; cached vector reuse {:?}. Excludes layout/paint and initial cache fill.",
            start.elapsed()
        );
    }
    #[test]
    fn history_window_fits_available_samples_with_one_minute_minimum() {
        let end = 604_800_000.;
        assert_eq!(
            history_start(Some(end - 172_800_000.), end, 10080),
            end - 172_800_000.
        );
        assert_eq!(history_start(Some(0.), end, 60), end - 3_600_000.);
        assert_eq!(history_start(Some(end), end, 10080), end - 60_000.);
        assert_eq!(history_start(None, end, 10080), 0.);
    }
    #[test]
    fn long_history_weights_resolutions_and_preserves_empty_buckets() {
        let points = vec![
            Point {
                at: 0.,
                value: Some(100.),
                gap: false,
            },
            Point {
                at: 10_000.,
                value: Some(30.),
                gap: false,
            },
            Point {
                at: 3_600_000.,
                value: Some(20.),
                gap: true,
            },
        ];
        let history = vec![
            minute(0, bucket(10_000, false, 0.)),
            minute(10_000, bucket(60_000, false, 0.)),
            raw(3_600_000, measures(None), 0., Vec::new()),
        ];
        let week = history_points(&points, &history, (0., 604_800_000.), 10080);
        assert_eq!(week.len(), 336);
        assert_eq!(week[0].value, Some(40.));
        assert_eq!(week[1].value, None);
        assert_eq!(week[2].value, Some(20.));
        assert_eq!(week[0].at, 900_000.);
        assert_eq!(
            history_points(&points, &history, (0., 86_400_000.), 1440).len(),
            288
        );
        assert_eq!(
            history_points(&points, &history, (0., 300_000.), 5).len(),
            3
        );
    }
    #[test]
    fn memory_card_includes_swap_pressure_without_flagging_absent_swap() {
        let mut data = Snapshot {
            memory_used: 20,
            memory_total: 100,
            swap_used: 49,
            swap_total: 100,
            ..Default::default()
        };
        assert_eq!(card_level(Some(&data), 1), None);
        data.swap_used = 50;
        assert_eq!(card_level(Some(&data), 1), Some("High"));
        data.swap_used = 90;
        assert_eq!(card_level(Some(&data), 1), Some("Critical"));
        data.swap_total = 0;
        assert_eq!(card_level(Some(&data), 1), None);
        data.memory_used = 96;
        assert_eq!(card_level(Some(&data), 1), Some("Critical"));
        assert_eq!(card_level(None, 1), None);
    }
    #[test]
    fn resource_health_distinguishes_zero_from_unavailable_and_normalizes_load() {
        let reading = |data: Snapshot, metric| resource_reading(Some(&data), metric);
        for metric in 0..3 {
            assert_eq!(resource_reading(None, metric), None);
        }
        assert_eq!(reading(Snapshot::default(), 0), Some(0.));
        let cpu = |cpu| Snapshot {
            cpu,
            ..Default::default()
        };
        assert_eq!(reading(cpu(-1.), 0), None);
        assert_eq!(reading(cpu(f64::NAN), 0), None);
        let memory = |memory_used, memory_total| Snapshot {
            memory_used,
            memory_total,
            ..Default::default()
        };
        assert_eq!(reading(memory(0, 100), 1), Some(0.));
        assert_eq!(reading(memory(20, 0), 1), None);
        let load = |cores, load| Snapshot {
            cores,
            load: [load, 0., 0.],
            ..Default::default()
        };
        assert_eq!(reading(load(32, 8.), 2), Some(25.));
        assert_eq!(reading(load(32, f64::NAN), 2), None);
        assert_eq!(reading(load(0, 8.), 2), None);
        assert_eq!(resource_level(None, 0), None);
        assert_eq!(resource_level(Some(79.9), 0), None);
        assert_eq!(resource_level(Some(80.), 0), Some("High"));
        assert_eq!(resource_level(Some(95.), 0), Some("Critical"));
        assert_eq!(resource_level(Some(84.9), 1), None);
        assert_eq!(resource_level(Some(85.), 1), Some("High"));
        assert_eq!(resource_level(Some(100.), 2), Some("High"));
        assert_eq!(resource_level(Some(200.), 2), Some("Critical"));
    }
    #[test]
    fn keyboard_inspection_respects_visible_window_and_includes_missing_samples() {
        let points = [10., 20., 50., 90.].map(|at| Point {
            at,
            value: None,
            gap: true,
        });
        assert_eq!(step_sample(&points, (20., 90.), None, "home"), Some(20.));
        assert_eq!(step_sample(&points, (20., 90.), None, "left"), Some(90.));
        assert_eq!(
            step_sample(&points, (20., 90.), Some(50.), "left"),
            Some(20.)
        );
        assert_eq!(
            step_sample(&points, (20., 90.), Some(20.), "left"),
            Some(20.)
        );
        assert_eq!(
            step_sample(&points, (20., 90.), Some(20.), "right"),
            Some(50.)
        );
        assert_eq!(
            step_sample(&points, (20., 90.), Some(90.), "right"),
            Some(90.)
        );
        assert_eq!(
            step_sample(&points, (20., 90.), Some(50.), "end"),
            Some(90.)
        );
        assert_eq!(step_sample(&points, (100., 200.), None, "home"), None);
    }
    #[test]
    fn chart_areas_and_lines_never_bridge_missing_or_discontinuous_samples() {
        let points = vec![
            Point {
                at: 0.,
                value: Some(1.),
                gap: false,
            },
            Point {
                at: 10.,
                value: Some(2.),
                gap: false,
            },
            Point {
                at: 20.,
                value: None,
                gap: false,
            },
            Point {
                at: 30.,
                value: Some(3.),
                gap: false,
            },
            Point {
                at: 40.,
                value: Some(4.),
                gap: true,
            },
            Point {
                at: 50.,
                value: Some(f64::NAN),
                gap: false,
            },
            Point {
                at: 60.,
                value: Some(6.),
                gap: false,
            },
        ];
        assert_eq!(
            graph_runs(&points, (0., 60.)),
            vec![
                vec![(0., 1.), (10., 2.)],
                vec![(30., 3.)],
                vec![(40., 4.)],
                vec![(60., 6.)]
            ]
        );
        assert_eq!(
            graph_runs(&points, (10., 40.)),
            vec![vec![(10., 2.)], vec![(30., 3.)], vec![(40., 4.)]]
        );
        assert!(graph_runs(&points, (70., 80.)).is_empty());
    }
    #[test]
    fn inspection_uses_sample_time_and_does_not_replace_missing_values() {
        let points = vec![
            Point {
                at: 0.,
                value: Some(8.),
                gap: false,
            },
            Point {
                at: 10.,
                value: Some(2.),
                gap: false,
            },
            Point {
                at: 40.,
                value: None,
                gap: true,
            },
            Point {
                at: 90.,
                value: Some(6.),
                gap: true,
            },
        ];
        assert_eq!(nearest_sample(&points, (10., 90.), 0.4).unwrap().at, 40.);
        assert_eq!(
            nearest_sample(&points, (10., 90.), 0.4).unwrap().value,
            None
        );
        assert_eq!(nearest_sample(&points, (10., 90.), -1.).unwrap().at, 10.);
        assert_eq!(nearest_sample(&points, (10., 90.), 2.).unwrap().at, 90.);
        assert!(nearest_sample(&points, (100., 200.), 0.5).is_none());
        assert!(nearest_sample(&points, (10., 10.), 0.5).is_none());
        assert!(nearest_sample(&points, (10., 90.), f64::NAN).is_none());
    }
    #[test]
    fn interface_choice_prioritizes_uplinks_and_preserves_explicit_virtual_choice() {
        let interfaces: Vec<&str> = ordered_interfaces(
            vec![
                ("lo", 9000.),
                ("wg0", 8000.),
                ("", 1e9),
                ("eth1", 20.),
                ("eth0", 100.),
                ("cilium0", 7000.),
            ],
            |(name, _)| name,
            |(_, traffic)| *traffic,
        )
        .into_iter()
        .map(|(name, _)| name)
        .collect();
        assert_eq!(interfaces, vec!["eth0", "eth1", "wg0", "lo", "cilium0"]);
        assert_eq!(selected_interface(&interfaces, None), "eth0");
        assert_eq!(selected_interface(&interfaces, Some(&"lo".into())), "lo");
        assert_eq!(
            selected_interface(&interfaces, Some(&"removed".into())),
            "eth0"
        );
        assert_eq!(selected_interface(&[], Some(&"eth0".into())), "");
    }
    fn sample(at: i64, received: u64, uptime: u64) -> Sample {
        raw(
            at,
            measures(Some(12.)),
            uptime as f64,
            vec![interface("eth0", received, 0)],
        )
    }
    #[test]
    fn rate_requires_continuous_samples_without_counter_reset_or_reboot() {
        let readings = vec![
            sample(10000, 100, 10),
            sample(20000, 300, 20),
            sample(30000, 50, 30),
            sample(40000, 100, 2),
            sample(80000, 500, 42),
        ];
        let result = series(&readings, 3, "eth0", false);
        assert_eq!(
            result.iter().map(|p| p.value).collect::<Vec<_>>(),
            vec![None, Some(20.), None, None, None]
        );
        assert!(result[4].gap);
    }
    #[test]
    fn summarized_rates_and_resolution_do_not_create_false_gaps() {
        let a = minute(60000, bucket(60000, false, 3.));
        let b = minute(120000, bucket(60000, false, 3.));
        let result = series(&[a, b], 3, "eth0", false);
        assert_eq!(result[1].value, Some(3.));
        assert!(!result[1].gap);
    }
    #[test]
    fn chart_buckets_use_explicit_rates_and_keep_the_live_tail() {
        let history = vec![
            Sample {
                at: 1_800_000,
                value: SampleValue::Chart(Box::new(bucket(1_800_000, false, 40.))),
            },
            sample(1_803_000, 200, 1803),
            sample(1_806_000, 260, 1806),
        ];
        let points = series(&history, 3, "eth0", false);
        assert_eq!(
            points.iter().map(|p| p.value).collect::<Vec<_>>(),
            vec![Some(40.), None, Some(20.)]
        );
        let chart = history_points(&points, &history, (0., 1_806_000.), 10080);
        assert_eq!(chart.len(), 3);
        assert_eq!(chart[2].value, Some(20.));
    }

    #[test]
    fn missing_values_and_explicit_gaps_are_preserved() {
        let a = minute(10000, bucket(60000, true, 0.));
        let b = raw(20000, measures(None), 20., vec![interface("eth0", 200, 0)]);
        let result = series(&[a, b], 0, "eth0", false);
        assert!(result[0].gap && result[1].gap);
        assert_eq!(result[1].value, None);
    }
}
