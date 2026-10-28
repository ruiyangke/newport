use super::{Point, positions};
use crate::containers::tests::{draw, fixture, select, settle, snapshot};
use gpui::{Bounds, TestAppContext, point, px, size};
use gpui_kit as gpui;
use newport_domain::containers::ContainerState;
use std::time::{Duration, Instant};

#[test]
fn chart_preserves_actual_time_and_cpu_above_one_core() {
    let now = Instant::now();
    let values = [
        Point {
            at: now,
            sequence: 1,
            value: 0.,
        },
        Point {
            at: now + Duration::from_secs(5),
            sequence: 2,
            value: 100.,
        },
        Point {
            at: now + Duration::from_secs(15),
            sequence: 3,
            value: 200.,
        },
    ];
    let bounds = Bounds::new(point(px(10.), px(20.)), size(px(84.), px(18.)));
    let runs = positions(&values, 100., bounds);
    assert_eq!(runs.len(), 1);
    assert_eq!(
        runs[0],
        vec![
            point(px(10.), px(37.)),
            point(px(38.), px(29.5)),
            point(px(94.), px(22.))
        ]
    );
    // An absent limit leaves headroom above the largest value.
    let unbounded = positions(&values, 0., bounds);
    assert_eq!(unbounded[0][2].y, px(25.));
}

#[test]
fn chart_does_not_bridge_unknown_readings_or_suspended_polling() {
    let now = Instant::now();
    let values = [
        Point {
            at: now,
            sequence: 1,
            value: 1.,
        },
        Point {
            at: now + Duration::from_secs(5),
            sequence: 3,
            value: 2.,
        },
        Point {
            at: now + Duration::from_secs(10),
            sequence: 4,
            value: 3.,
        },
        Point {
            at: now + Duration::from_secs(40),
            sequence: 5,
            value: 4.,
        },
    ];
    let bounds = Bounds::new(point(px(0.), px(0.)), size(px(84.), px(18.)));
    let runs = positions(&values, 100., bounds);
    assert_eq!(runs.iter().map(Vec::len).collect::<Vec<_>>(), vec![1, 2, 1]);
    for point in runs.iter().flatten() {
        assert!(point.x >= px(0.) && point.x <= px(84.));
        assert!(point.y >= px(2.) && point.y <= px(17.));
    }
}

#[gpui::test]
fn resource_sparks_follow_real_snapshots_keep_small_geometry_and_clear_for_recreated_ids(
    cx: &mut TestAppContext,
) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "c:stack-web-1");
    assert!(cx.debug_bounds("container-cpu-spark").is_none());
    let mut data = snapshot();
    data.containers[0].cpu = Some(100.);
    data.containers[0].memory = Some(1000);
    data.containers[0].memory_limit = Some(4000);
    for cpu in [100., 180.] {
        data.containers[0].cpu = Some(cpu);
        backend.set_snapshot(data.clone());
        panel.update(cx, |p, _| p.client.refresh());
        settle(&panel, cx);
    }
    for dark in [false, true] {
        cx.update(|_, cx| crate::theme::install(dark, cx));
        draw(cx);
        for id in ["container-cpu-spark", "container-memory-spark"] {
            let bounds = cx.debug_bounds(id).unwrap();
            assert_eq!(bounds.size, size(px(84.), px(18.)));
        }
        cx.update(|window, _| {
            let charts: Vec<_> = gpui::base::test_support::snapshots(window)
                .into_iter()
                .filter(|node| node.role() == Some(gpui::accesskit::Role::Image))
                .collect();
            assert!(charts.iter().any(|node| {
                node.label()
                    .is_some_and(|label| label.starts_with("CPU over the last "))
            }));
            assert!(charts.iter().any(|node| {
                node.label()
                    .is_some_and(|label| label.starts_with("Memory over the last "))
            }));
        });
    }
    let reads = backend.count("containers_snapshot");
    for _ in 0..4 {
        draw(cx);
    }
    assert_eq!(
        backend.count("containers_snapshot"),
        reads,
        "painting does not sample or fetch"
    );
    assert_eq!(
        panel.read_with(cx, |p, _| p
            .client
            .stats_history(&"a".repeat(64))
            .unwrap()
            .cpu()
            .len()),
        2
    );
    data.containers[0].memory = None;
    backend.set_snapshot(data.clone());
    panel.update(cx, |p, _| p.client.refresh());
    settle(&panel, cx);
    cx.update(|window, _| {
        let node = gpui::base::test_support::snapshots(window)
            .into_iter()
            .find(|node| {
                node.path()
                    .iter()
                    .any(|part| part.to_string() == "container-fact-memory-value")
            })
            .unwrap();
        assert_eq!(
            node.label(),
            Some("— of 3.9 KiB"),
            "unknown memory must not become zero"
        );
    });
    assert_eq!(
        panel.read_with(cx, |p, _| p
            .client
            .stats_history(&"a".repeat(64))
            .unwrap()
            .memory()
            .len()),
        2
    );
    data.containers[0].state = ContainerState::Paused;
    backend.set_snapshot(data.clone());
    panel.update(cx, |p, _| p.client.refresh());
    settle(&panel, cx);
    assert!(
        cx.debug_bounds("container-cpu-spark").is_none(),
        "stats show only for running containers"
    );
    data.containers[0].state = ContainerState::Running;
    data.containers[0].id = "f".repeat(64);
    backend.set_snapshot(data);
    panel.update(cx, |p, _| p.client.refresh());
    settle(&panel, cx);
    assert!(
        cx.debug_bounds("container-cpu-spark").is_none(),
        "a recreated name must not inherit its predecessor's history"
    );
    assert_eq!(backend.writes(), 0);
}
