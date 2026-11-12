//! Read-only scan diagnostics built from enums and counts, never remote messages.
use gpui_kit::{
    Anchor, App, Div, Focusable, FontWeight, ScrollHandle, SharedString, Stateful, Window,
    accesskit,
    assets::IconName,
    base::{ObservedElement, TestSupportExt},
    component::{ActiveTheme, button::ButtonVariants, popover::Popover},
    div,
    prelude::*,
    px, relative,
};
use newport_client::discovery::View;
use newport_protocol::monitor::ports::discovery as p;
use std::sync::Arc;

const CAUSES: [(p::IssueKind, &str); 8] = [
    (p::IssueKind::PermissionDenied, "permission denied"),
    (p::IssueKind::Unsupported, "not supported"),
    (p::IssueKind::TimedOut, "timed out"),
    (p::IssueKind::Interrupted, "interrupted"),
    (p::IssueKind::BudgetExceeded, "scan limit reached"),
    (p::IssueKind::Io, "read failed"),
    (p::IssueKind::InvalidResponse, "invalid response"),
    (p::IssueKind::Unavailable, "source unavailable"),
];

pub(crate) fn report(value: &p::Report) -> String {
    let status = match value.outcome {
        p::Outcome::Complete => "Complete",
        p::Outcome::Partial => "Limited",
        p::Outcome::Unavailable => "Unavailable",
        p::Outcome::NotAttempted => "Not checked",
        p::Outcome::NotApplicable => "Not applicable",
    };
    let mut parts = vec![status.to_owned()];
    for (kind, label) in CAUSES {
        let count = value
            .issues
            .iter()
            .filter(|issue| issue.kind == kind)
            .fold(0u64, |total, issue| total.saturating_add(issue.count));
        match count {
            0 => {}
            1 => parts.push(label.into()),
            _ => parts.push(format!("{label} ({count} reports)")),
        }
    }
    parts.join(" · ")
}

fn inventory(value: &p::Report, source: Option<p::SocketSource>) -> String {
    let status = report(value);
    match source {
        Some(p::SocketSource::Diagnostic) => format!("{status} · kernel sockets"),
        Some(p::SocketSource::Proc) => format!("{status} · proc fallback"),
        None => status,
    }
}

/// Whether Needle sees other accounts' processes through sudo for this
/// connection, or why not.
fn elevation(value: &p::Report) -> String {
    let refused = value
        .issues
        .iter()
        .any(|issue| issue.kind == p::IssueKind::PermissionDenied);
    match value.outcome {
        p::Outcome::Complete => "Active".into(),
        p::Outcome::NotApplicable => "Not needed".into(),
        p::Outcome::Unavailable if refused => {
            "Unavailable · sudo needs a password or isn't allowed".into()
        }
        _ => report(value),
    }
}

#[derive(Clone, Copy)]
enum Stage {
    Ipv4,
    Ipv6,
    Processes,
    Cgroups,
    Services,
    Containers,
    Elevation,
    Details,
}

impl Stage {
    fn label(self) -> &'static str {
        match self {
            Self::Ipv4 => "IPv4 ports",
            Self::Ipv6 => "IPv6 ports",
            Self::Processes => "Processes",
            Self::Cgroups => "Cgroups",
            Self::Services => "Services",
            Self::Containers => "Containers",
            Self::Elevation => "Elevation",
            Self::Details => "Details",
        }
    }
}

fn reports(snapshot: &p::Snapshot) -> [(Stage, &p::Report); 8] {
    let coverage = &snapshot.coverage;
    [
        (Stage::Ipv4, &coverage.ipv4),
        (Stage::Ipv6, &coverage.ipv6),
        (Stage::Processes, &coverage.processes),
        (Stage::Cgroups, &coverage.cgroups),
        (Stage::Services, &coverage.services),
        (Stage::Containers, &coverage.containers),
        (Stage::Elevation, &coverage.elevation),
        (Stage::Details, &coverage.details),
    ]
}

fn rows(snapshot: &p::Snapshot) -> [(Stage, String); 8] {
    reports(snapshot).map(|(stage, value)| {
        let text = match stage {
            Stage::Ipv4 => inventory(value, snapshot.diagnostics.ipv4_source),
            Stage::Ipv6 => inventory(value, snapshot.diagnostics.ipv6_source),
            Stage::Elevation => elevation(value),
            _ => report(value),
        };
        (stage, text)
    })
}

pub(crate) fn limited(value: &p::Report) -> bool {
    matches!(value.outcome, p::Outcome::Partial | p::Outcome::Unavailable)
        || !value.issues.is_empty()
}

pub(crate) fn limitations(snapshot: &p::Snapshot) -> Vec<(&'static str, String)> {
    rows(snapshot)
        .into_iter()
        .zip(reports(snapshot))
        .filter(|((stage, _), (_, value))| {
            limited(value)
                || match stage {
                    Stage::Ipv4 => snapshot.diagnostics.ipv4_source == Some(p::SocketSource::Proc),
                    Stage::Ipv6 => snapshot.diagnostics.ipv6_source == Some(p::SocketSource::Proc),
                    _ => false,
                }
        })
        .map(|((stage, text), _)| (stage.label(), text))
        .collect()
}

#[derive(Debug)]
struct Summary {
    freshness: &'static str,
    counts: Option<String>,
    rows: Vec<(&'static str, String)>,
}

impl Summary {
    fn new(view: Option<&View>) -> Self {
        let Some(view) = view else {
            return Self {
                freshness: "No scan details available. Refresh to check again.",
                counts: None,
                rows: Vec::new(),
            };
        };
        let snapshot = &view.evidence().current;
        let mut rows: Vec<_> = rows(snapshot)
            .map(|(stage, text)| (stage.label(), text))
            .into();
        if limited(&view.evidence().retention) {
            rows.push(("Retained rows", report(&view.evidence().retention)));
        }
        Self {
            freshness: if !view.evidence().has_current_inventory() {
                "Failed scan"
            } else if view.inventory().fresh {
                "Latest scan"
            } else {
                "Last available scan · refresh to verify"
            },
            counts: Some(format!(
                "{} ms · listeners: {} · publications: {}",
                snapshot.elapsed_ms,
                snapshot.sockets.len(),
                snapshot.publications.len()
            )),
            rows,
        }
    }
}

pub(crate) fn control(server: &str, view: Option<Arc<View>>) -> Popover {
    Popover::new(SharedString::from(format!("discovery-details:{server}")))
        .anchor(Anchor::TopRight)
        .p(px(crate::hover::PADDING))
        .offset(px(4.))
        .trigger(
            crate::controls::icon_button("discovery-details")
                .ghost()
                .icon(IconName::Info)
                .accessibility_label("Discovery details")
                .tooltip("Discovery details"),
        )
        .content(move |state, window, cx| {
            let scroll = window
                .use_keyed_state("discovery-diagnostics-scroll", cx, |_, _| {
                    ScrollHandle::new()
                })
                .read(cx)
                .clone();
            panel(view.as_deref(), &scroll, window, cx)
                .track_focus(&crate::hover::scroll_focus(
                    &state.focus_handle(cx),
                    window,
                    cx,
                ))
                .on_key_down(move |event, window, cx| {
                    crate::hover::scroll_on_key(event, &scroll, window, cx);
                })
        })
}

fn panel(
    view: Option<&View>,
    scroll: &ScrollHandle,
    window: &Window,
    cx: &App,
) -> ObservedElement<Stateful<Div>> {
    let summary = Summary::new(view);
    div()
        .id("discovery-diagnostics")
        .debug_selector(|| "discovery-diagnostics".into())
        .test_support()
        .role(accesskit::Role::Group)
        .aria_label("Discovery details")
        .w(px(crate::hover::width(window) - 2. * crate::hover::PADDING))
        .max_h(px(crate::hover::height(window)))
        .overflow_y_scroll()
        .track_scroll(scroll)
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(10.))
        .text_size(px(crate::ui::FONT_SIZE))
        .line_height(relative(1.45))
        .text_color(cx.theme().popover_foreground)
        .child(
            div()
                .id("discovery-diagnostics-heading")
                .test_support()
                .role(accesskit::Role::Heading)
                .aria_label("Discovery details")
                .flex_shrink_0()
                .font_weight(FontWeight::SEMIBOLD)
                .child("Discovery details"),
        )
        .child(
            div()
                .id("discovery-scan-summary")
                .test_support()
                .role(accesskit::Role::Label)
                .aria_label(match summary.counts.as_deref() {
                    Some(counts) => format!("{} · {counts}", summary.freshness),
                    None => summary.freshness.to_owned(),
                })
                .flex_shrink_0()
                .text_color(cx.theme().muted_foreground)
                .whitespace_normal()
                .child(summary.freshness)
                .when_some(summary.counts, |view, counts| {
                    view.child(div().child(counts))
                }),
        )
        .children(summary.rows.into_iter().map(|(label, value)| {
            div()
                .id(SharedString::from(format!("discovery-coverage:{label}")))
                .test_support()
                .role(accesskit::Role::Label)
                .aria_label(format!("{label}: {value}"))
                .debug_selector(move || format!("discovery-diagnostic-{label}"))
                .flex_shrink_0()
                .flex()
                .items_start()
                .gap(px(12.))
                .min_w_0()
                .child(
                    div()
                        .w(px(88.))
                        .flex_shrink_0()
                        .whitespace_normal()
                        .text_color(cx.theme().muted_foreground)
                        .child(label),
                )
                .child(div().flex_1().min_w_0().whitespace_normal().child(value))
        }))
}

#[cfg(test)]
mod tests;
