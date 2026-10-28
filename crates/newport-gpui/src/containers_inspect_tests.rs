use super::{InspectAction, health_command};
use crate::containers::{
    ContainersPanel,
    tests::{FakeBackend, click, draw, element, fixture, select, settle, snapshot},
};
use gpui_kit as gpui;
use gpui_kit::{ClipboardItem, Entity, TestAppContext, VisualTestContext, point, px, size};
use newport_domain::containers::{
    ContainerInspect, EnvVar, HealthResult, Healthcheck, Mount, Network,
};
use std::{thread, time::Duration};

fn inspection() -> ContainerInspect {
    ContainerInspect {
        id: snapshot().containers[0].id.clone(),
        name: "stack-web-1".into(),
        entrypoint: Some(vec!["/usr/bin/app".into()]),
        cmd: Some(vec!["serve".into()]),
        working_dir: Some("/srv/app".into()),
        user: Some("fixture".into()),
        hostname: Some("web".into()),
        restart_policy: Some("unless-stopped".into()),
        env: vec![
            EnvVar {
                name: "MODE".into(),
                value: "public-mode".into(),
            },
            EnvVar {
                name: "API_TOKEN".into(),
                value: "private-token".into(),
            },
        ],
        mounts: vec![Mount {
            kind: "volume".into(),
            destination: "/data".into(),
            name: Some("stack-data".into()),
            rw: true,
            ..Default::default()
        }],
        networks: vec![Network {
            name: "stack_default".into(),
            ip_address: Some("172.20.0.2".into()),
            aliases: vec!["web".into()],
        }],
        healthcheck: Some(Healthcheck {
            test: vec!["CMD".into(), "echo".into(), "CMD-SHELL".into()],
            interval: Some(30_000),
        }),
        last_health: vec![
            HealthResult {
                at: 1_700_000_000_000,
                exit_code: 0,
                output: " \n".into(),
            },
            HealthResult {
                at: 1_700_000_030_000,
                exit_code: 2,
                output: "  connection refused\n".into(),
            },
        ],
        labels: [("com.example.owner".into(), "fixture".into())].into(),
        ..Default::default()
    }
}

fn open(
    panel: &Entity<ContainersPanel>,
    cx: &mut VisualTestContext,
    backend: &FakeBackend,
    data: ContainerInspect,
) {
    backend.set_inspect(Ok(data));
    select(panel, cx, "c:stack-web-1");
    click(cx, "Inspect");
    panel.update(cx, |panel, _| panel.client.retry_inspect());
    settle(panel, cx);
}
fn has_label(cx: &mut VisualTestContext, label: &str) -> bool {
    cx.update(|window, _| {
        gpui_kit::base::test_support::snapshots(window)
            .iter()
            .any(|node| node.label() == Some(label))
    })
}
fn scroll_top(panel: &Entity<ContainersPanel>, cx: &mut VisualTestContext) {
    panel.update(cx, |panel, cx| {
        panel.detail_scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    });
    draw(cx);
}

#[gpui::test]
fn health_interval_output_and_sections_match_inspection_contract(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    open(&panel, cx, &backend, inspection());
    for title in [
        "Command",
        "Environment",
        "Mounts",
        "Networks",
        "Health check",
        "Labels",
    ] {
        assert!(has_label(cx, title), "missing section {title}");
    }
    assert!(has_label(cx, "Every 30s"));
    assert!(
        has_label(cx, "echo CMD-SHELL"),
        "only the first execution sentinel is metadata"
    );
    assert!(has_label(cx, "—"));
    assert!(has_label(cx, "connection refused"));
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn reveal_controls_expose_pressed_state_and_command_copy_is_exact(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    open(&panel, cx, &backend, inspection());
    assert_eq!(element(cx, "Show values").checked(), Some(false));
    click(cx, "Show values");
    assert_eq!(element(cx, "Hide values").checked(), Some(true));
    assert!(!has_label(cx, "Show MODE"));
    assert!(has_label(cx, "Show API_TOKEN"));
    click(cx, "Show API_TOKEN");
    click(cx, "Hide values");
    assert!(has_label(cx, "Show MODE"));
    assert!(has_label(cx, "Show API_TOKEN"));
    click(cx, "Copy container command");
    assert_eq!(
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
        Some("/usr/bin/app serve".into())
    );
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn empty_environment_has_no_reveal_control_and_zero_interval_is_omitted(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    let mut data = inspection();
    data.env.clear();
    data.entrypoint = None;
    data.cmd = None;
    data.healthcheck.as_mut().unwrap().interval = Some(0);
    open(&panel, cx, &backend, data);
    assert!(!has_label(cx, "Show values"));
    assert!(!has_label(cx, "Every 0s"));
    // The component's outer accessibility node carries disabled state; the
    // inner observed button does not. Verify the user's actual copy outcome.
    cx.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into())));
    click(cx, "Copy container command");
    assert_eq!(
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
        Some("unchanged".into())
    );
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn long_unbroken_command_and_env_stay_inside_compact_inspector(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    cx.simulate_resize(size(px(540.), px(1000.)));
    let mut data = inspection();
    data.cmd = Some(vec!["界".repeat(160)]);
    data.env = vec![EnvVar {
        name: "LONG_ENVIRONMENT_VARIABLE_NAME_".repeat(4),
        value: "value".repeat(80),
    }];
    data.mounts.clear();
    data.networks.clear();
    data.healthcheck = None;
    data.last_health.clear();
    open(&panel, cx, &backend, data);
    click(cx, "Show values");
    scroll_top(&panel, cx);
    let inspector = cx.debug_bounds("container-inspect").unwrap();
    for selector in [
        "inspect-command",
        "inspect-env-name-0",
        "inspect-env-value-0",
    ] {
        let bounds = cx.debug_bounds(selector).unwrap();
        assert!(
            bounds.left() >= inspector.left() && bounds.right() <= inspector.right() + px(1.),
            "{selector} escaped inspector: {bounds:?} vs {inspector:?}"
        );
        assert!(
            bounds.size.height > px(30.),
            "{selector} must wrap long unbroken content: {bounds:?}"
        );
    }
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn queued_inspection_actions_are_rejected_after_tab_navigation_recreation_and_retirement(
    cx: &mut TestAppContext,
) {
    let (panel, cx, backend) = fixture(cx);
    let id = inspection().id;
    open(&panel, cx, &backend, inspection());
    cx.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string("untouched".into())));
    click(cx, "Logs");
    panel.update(cx, |panel, cx| {
        assert!(!panel.inspect_action(&id, InspectAction::ToggleValues, cx));
        assert!(!panel.inspect_action(&id, InspectAction::CopyCommand("old command".into()), cx));
    });
    click(cx, "Inspect");
    backend.hold_inspects(true);
    panel.update(cx, |panel, _| panel.client.retry_inspect());
    for _ in 0..300 {
        panel.update(cx, |panel, cx| panel.poll(true, cx));
        draw(cx);
        if backend.held_inspects() == 1 {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(backend.held_inspects(), 1);
    let mut fresh = snapshot();
    fresh.containers[0].id = "e".repeat(64);
    backend.set_snapshot(fresh);
    panel.update(cx, |panel, cx| panel.refresh(cx));
    for _ in 0..300 {
        panel.update(cx, |panel, cx| panel.poll(true, cx));
        draw(cx);
        if panel.read_with(cx, |panel, _| {
            panel.inspected_id.as_deref() == Some(&"e".repeat(64))
        }) {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panel.update(cx, |panel, cx| {
        assert_eq!(panel.inspected_id, Some("e".repeat(64)));
        for action in [
            InspectAction::Retry,
            InspectAction::ToggleValues,
            InspectAction::Reveal("API_TOKEN".into()),
            InspectAction::ToggleLabels,
            InspectAction::CopyCommand("old command".into()),
        ] {
            assert!(!panel.inspect_action(&id, action, cx));
        }
        assert!(!panel.reveal_values && panel.revealed.is_empty() && !panel.labels_open);
    });
    backend.clear_inspect();
    backend.release_inspects();
    settle(&panel, cx);
    assert!(has_label(cx, "Show API_TOKEN"));
    assert_eq!(
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
        Some("untouched".into())
    );
    select(&panel, cx, "c:cache");
    panel.update(cx, |panel, cx| {
        assert!(!panel.inspect_action(&"e".repeat(64), InspectAction::ToggleValues, cx));
        let current = panel.inspected_id.clone().unwrap();
        panel.retire(cx);
        assert!(!panel.inspect_action(&current, InspectAction::ToggleValues, cx));
        assert!(!panel.inspect_action(
            &current,
            InspectAction::CopyCommand("retired command".into()),
            cx
        ));
    });
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn inspection_error_retries_current_id_without_a_write(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    backend.set_inspect(Err("inspection denied".into()));
    select(&panel, cx, "c:stack-web-1");
    click(cx, "Inspect");
    settle(&panel, cx);
    assert_eq!(
        panel.read_with(cx, |panel, _| panel.error("container-inspect")),
        Some("inspection denied".into())
    );
    assert!(has_label(cx, "Try inspecting again"));
    let reads = backend.count("container_inspect");
    backend.set_inspect(Ok(inspection()));
    click(cx, "Try inspecting again");
    settle(&panel, cx);
    assert!(!has_label(cx, "Try inspecting again"));
    assert!(has_label(cx, "Every 30s"));
    assert_eq!(backend.count("container_inspect"), reads + 1);
    assert_eq!(
        backend.requests("container_inspect").last().unwrap()["container"],
        inspection().id
    );
    assert_eq!(backend.writes(), 0);
}

#[test]
fn health_command_keeps_literal_sentinel_arguments() {
    assert_eq!(
        health_command(&[
            "CMD".into(),
            "printf".into(),
            "CMD".into(),
            "CMD-SHELL".into()
        ]),
        "printf CMD CMD-SHELL"
    );
    assert_eq!(
        health_command(&["CMD-SHELL".into(), "test -f /ready".into()]),
        "test -f /ready"
    );
}
