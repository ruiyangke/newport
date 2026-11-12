use super::*;
use crate::connections::fixtures::{Listener, observation};
use gpui_kit::{Context, FocusHandle, Render, TestAppContext, VisualTestContext, size};

fn issue(kind: p::IssueKind, count: u64) -> p::Issue {
    p::Issue {
        kind,
        count,
        message: "SECRET-REMOTE-ERROR /private/path --token=secret".into(),
    }
}

fn evidence() -> newport_protocol::desktop::connection::discovery::Discovery {
    let mut value = observation([Listener::new(8080).process(
        "worker",
        "worker",
        &["worker", "SECRET-ARGUMENT"],
        None,
    )])
    .data
    .unwrap();
    let snapshot = &mut value.current;
    snapshot.elapsed_ms = 25;
    snapshot.diagnostics.ipv4_source = Some(p::SocketSource::Proc);
    snapshot.diagnostics.ipv6_source = Some(p::SocketSource::Diagnostic);
    snapshot
        .coverage
        .ipv4
        .issues
        .push(issue(p::IssueKind::Unsupported, 1));
    for (report, kind) in [
        (
            &mut snapshot.coverage.processes,
            p::IssueKind::PermissionDenied,
        ),
        (&mut snapshot.coverage.cgroups, p::IssueKind::Unsupported),
        (&mut snapshot.coverage.services, p::IssueKind::TimedOut),
        (
            &mut snapshot.coverage.elevation,
            p::IssueKind::PermissionDenied,
        ),
        (&mut snapshot.coverage.details, p::IssueKind::BudgetExceeded),
    ] {
        *report = p::Report {
            outcome: p::Outcome::Partial,
            issues: vec![issue(kind, 3)],
        };
    }
    snapshot.coverage.elevation = p::Report {
        outcome: p::Outcome::Unavailable,
        issues: vec![issue(p::IssueKind::PermissionDenied, 1)],
    };
    value
}

#[test]
fn typed_diagnostics_cover_every_stage_and_exclude_remote_text() {
    let view = View::new(evidence(), false).unwrap();
    let summary = Summary::new(Some(&view));
    assert_eq!(summary.rows.len(), 8);
    assert_eq!(summary.freshness, "Latest scan");
    assert_eq!(
        summary.counts.as_deref(),
        Some("25 ms · listeners: 1 · publications: 0")
    );
    for (label, part) in [
        ("IPv4 ports", "not supported · proc fallback"),
        ("IPv6 ports", "Complete · kernel sockets"),
        ("Processes", "permission denied"),
        ("Cgroups", "not supported"),
        ("Services", "timed out"),
        ("Containers", "Not applicable"),
        ("Elevation", "sudo needs a password or isn't allowed"),
        ("Details", "scan limit reached"),
    ] {
        assert!(
            summary
                .rows
                .iter()
                .any(|(name, text)| *name == label && text.contains(part)),
            "{label}: {summary:?}"
        );
    }
    assert!(!format!("{summary:?}").contains("SECRET"));
}

#[test]
fn missing_stale_and_empty_scans_do_not_claim_a_current_complete_inventory() {
    let missing = Summary::new(None);
    assert!(missing.freshness.contains("No scan details"));
    assert!(missing.rows.is_empty() && missing.counts.is_none());
    let value = observation([]).data.unwrap();
    let fresh = View::new(value.clone(), false).unwrap();
    let stale = View::new(value, true).unwrap();
    assert_eq!(Summary::new(Some(&fresh)).freshness, "Latest scan");
    assert!(
        Summary::new(Some(&fresh))
            .counts
            .unwrap()
            .contains("listeners: 0")
    );
    assert!(
        Summary::new(Some(&stale))
            .freshness
            .contains("Last available scan")
    );
    assert_eq!(Summary::new(Some(&stale)).rows.len(), 8);
}

#[test]
fn failed_scan_details_do_not_claim_a_successful_empty_reading() {
    let mut value = observation([]).data.unwrap();
    value.current.coverage.ipv4 = p::Report {
        outcome: p::Outcome::Unavailable,
        issues: vec![issue(p::IssueKind::TimedOut, 1)],
    };
    value.current.coverage.ipv6 = p::Report {
        outcome: p::Outcome::Unavailable,
        issues: vec![issue(p::IssueKind::PermissionDenied, 1)],
    };
    let view = View::new(value, true).unwrap();
    let summary = Summary::new(Some(&view));
    assert_eq!(summary.freshness, "Failed scan");
    assert!(
        summary
            .rows
            .iter()
            .any(|(name, text)| { *name == "IPv4 ports" && text == "Unavailable · timed out" })
    );
    assert!(!format!("{summary:?}").contains("SECRET"));
}

#[test]
fn issue_counts_saturate_and_repeated_messages_do_not_expand_the_view() {
    let report = p::Report {
        outcome: p::Outcome::Unavailable,
        issues: vec![
            issue(p::IssueKind::PermissionDenied, u64::MAX),
            issue(p::IssueKind::PermissionDenied, 10),
        ],
    };
    let text = super::report(&report);
    assert_eq!(
        text,
        format!("Unavailable · permission denied ({} reports)", u64::MAX)
    );
    assert!(!text.contains("SECRET"));
}

#[test]
fn successful_stages_stay_out_of_row_limitations_and_refused_elevation_is_explicit() {
    let mut value = observation([]).data.unwrap().current;
    assert!(limitations(&value).is_empty());
    value.coverage.elevation = p::Report::complete();
    assert!(limitations(&value).is_empty());
    value.coverage.elevation = p::Report::not_applicable();
    assert!(limitations(&value).is_empty());
    value.coverage.elevation = p::Report {
        outcome: p::Outcome::Unavailable,
        issues: vec![issue(p::IssueKind::PermissionDenied, 1)],
    };
    assert_eq!(
        limitations(&value),
        vec![(
            "Elevation",
            "Unavailable · sudo needs a password or isn't allowed".into()
        )]
    );
}

struct Host {
    before: FocusHandle,
    view: Option<Arc<View>>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p(px(12.))
            .flex()
            .gap(px(8.))
            .child(
                crate::controls::button("before-diagnostics")
                    .track_focus(&self.before)
                    .label("Before"),
            )
            .child(control("fixture", self.view.clone()))
    }
}

fn frame(cx: &mut VisualTestContext) {
    crate::page_perf_tests::frame(cx);
    crate::page_perf_tests::frame(cx);
}

#[gpui_kit::test]
fn diagnostic_popover_exposes_scan_status_and_redacted_coverage(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (host, cx) = cx.add_window_view(|_, cx| Host {
        before: cx.focus_handle(),
        view: None,
    });
    cx.simulate_resize(size(px(1000.), px(900.)));
    for (view, freshness) in [
        (Some(View::new(evidence(), false).unwrap()), "Latest scan"),
        (
            Some(View::new(evidence(), true).unwrap()),
            "Last available scan · refresh to verify",
        ),
        (None, "No scan details available. Refresh to check again."),
    ] {
        let has_scan = view.is_some();
        host.update(cx, |host, cx| {
            host.view = view.map(Arc::new);
            cx.notify();
        });
        frame(cx);
        cx.update(|window, cx| {
            window.activate_window();
            host.read(cx).before.clone().focus(window, cx);
            window.focus_next(cx);
        });
        cx.simulate_keystrokes("enter");
        frame(cx);
        cx.update(|window, _| {
            let nodes = gpui_kit::base::test_support::snapshots(window);
            let labels: Vec<_> = nodes
                .iter()
                .filter(|node| node.role() == Some(accesskit::Role::Label))
                .filter_map(|node| node.label())
                .collect();
            assert!(
                labels.iter().any(|label| label.starts_with(freshness)),
                "scan status must be accessible: {labels:?}"
            );
            assert!(labels.iter().all(|label| !label.contains("SECRET")));
            if has_scan {
                assert!(
                    labels.iter().any(|label| {
                        label.contains("25 ms · listeners: 1 · publications: 0")
                    })
                );
                for stage in [
                    "IPv4 ports",
                    "IPv6 ports",
                    "Processes",
                    "Cgroups",
                    "Services",
                    "Containers",
                    "Elevation",
                    "Details",
                ] {
                    assert!(
                        labels
                            .iter()
                            .any(|label| label.starts_with(&format!("{stage}: "))),
                        "coverage must be accessible for {stage}: {labels:?}"
                    );
                }
                assert!(labels.contains(&"Processes: Limited · permission denied (3 reports)"));
                assert!(
                    labels.contains(
                        &"Elevation: Unavailable · sudo needs a password or isn't allowed"
                    )
                );
            } else {
                assert_eq!(labels, [freshness]);
            }
        });
        cx.simulate_keystrokes("escape");
        frame(cx);
    }
}

#[gpui_kit::test]
fn diagnostic_popover_opens_by_keyboard_and_stays_bounded_in_both_themes(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (host, cx) = cx.add_window_view(|_, cx| Host {
        before: cx.focus_handle(),
        view: Some(Arc::new(View::new(evidence(), false).unwrap())),
    });
    for (dark, width, height) in [(false, 800., 600.), (true, 320., 280.)] {
        cx.update(|_, cx| crate::theme::install(dark, cx));
        cx.simulate_resize(size(px(width), px(height)));
        frame(cx);
        cx.update(|window, cx| {
            window.activate_window();
            host.read(cx).before.clone().focus(window, cx);
            window.focus_next(cx);
        });
        frame(cx);
        cx.update(|window, _| {
            let nodes = gpui_kit::base::test_support::snapshots(window);
            let trigger = nodes
                .iter()
                .find(|node| {
                    node.label() == Some("Discovery details")
                        && node.role() == Some(accesskit::Role::Button)
                })
                .unwrap();
            assert_eq!(trigger.focused(), Some(true));
        });
        cx.simulate_keystrokes("enter");
        frame(cx);
        let bounds = cx
            .debug_bounds("discovery-diagnostics")
            .expect("keyboard opened diagnostics");
        assert!(bounds.left() >= px(0.) && bounds.right() <= px(width));
        assert!(bounds.top() >= px(0.) && bounds.bottom() <= px(height));
        let limit = cx.update(|window, _| crate::hover::height(window));
        assert!(bounds.size.height <= px(limit));
        cx.simulate_keystrokes("end");
        frame(cx);
        let last = cx.debug_bounds("discovery-diagnostic-Details").unwrap();
        assert!(
            last.top() >= bounds.top() && last.bottom() <= bounds.bottom() + px(1.),
            "keyboard scrolling reaches the final stage: {last:?} within {bounds:?}"
        );
        cx.simulate_keystrokes("home");
        frame(cx);
        assert!(
            cx.debug_bounds("discovery-diagnostic-IPv4 ports")
                .unwrap()
                .top()
                >= bounds.top()
        );
        cx.simulate_keystrokes("escape");
        frame(cx);
        assert!(cx.debug_bounds("discovery-diagnostics").is_none());
    }
    host.update(cx, |host, cx| {
        host.view = None;
        cx.notify();
    });
    frame(cx);
    cx.update(|window, cx| {
        host.read(cx).before.clone().focus(window, cx);
        window.focus_next(cx);
    });
    cx.simulate_keystrokes("enter");
    frame(cx);
    assert!(
        cx.debug_bounds("discovery-diagnostics").is_some(),
        "diagnostics remain available without rows"
    );
}
