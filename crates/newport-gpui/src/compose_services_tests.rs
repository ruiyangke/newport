use super::ContainersPanel;
use crate::containers::tests::{
    FakeBackend, click, compose_project, draw, element, fixture, select, settle, snapshot,
};
use gpui::{
    Entity, KeyDownEvent, KeyUpEvent, Keystroke, TestAppContext, VisualTestContext, px, size,
};
use gpui_kit as gpui;
use gpui_kit::base::test_support::{ElementSnapshot, snapshots};
use gpui_kit::component::WindowExt;
use newport_domain::containers::{ContainerPort, ContainerState, PortProtocol};
use serde_json::{Value, json};
use std::{thread, time::Duration};

fn configure_table(
    panel: &Entity<ContainersPanel>,
    cx: &mut VisualTestContext,
    backend: &FakeBackend,
) {
    let mut source = snapshot();
    let web = &mut source.containers[0];
    web.compose.as_mut().unwrap().number = Some(1);
    web.cpu = Some(12.5);
    web.memory = Some(64 * 1024 * 1024);
    web.ports = vec![ContainerPort {
        ip: Some("0.0.0.0".into()),
        host_port: Some(8080),
        container_port: 80,
        protocol: PortProtocol::Tcp,
    }];
    let mut replica = web.clone();
    replica.id = "e".repeat(64);
    replica.name = "stack-web-2".into();
    replica.state = ContainerState::Paused;
    replica.compose.as_mut().unwrap().number = Some(2);
    replica.cpu = None;
    replica.memory = None;
    replica.ports.clear();
    let mut orphan = replica.clone();
    orphan.id = "f".repeat(64);
    orphan.name = "stack-legacy-1".into();
    orphan.state = ContainerState::Created;
    orphan.compose.as_mut().unwrap().service = "legacy".into();
    orphan.compose.as_mut().unwrap().number = Some(1);
    let mut other = replica.clone();
    other.id = "8".repeat(64);
    other.name = "other-web-1".into();
    other.compose.as_mut().unwrap().project = "other".into();
    source.containers.extend([replica, orphan, other]);
    backend.set_snapshot(source);
    let mut detail = compose_project();
    detail["services"] = json!([
        {"name":"worker","image":"fixture/worker:1","build":true,"profiles":[]},
        {"name":"queue","image":"fixture/queue:1","build":false,"profiles":["background","dev"]},
        {"name":"web","image":"fixture/web:1","build":false,"profiles":[]}
    ]);
    backend.set_compose_project(Ok(detail));
    select(panel, cx, "p:stack");
    panel.update(cx, |p, _| {
        p.client.refresh();
        p.client.retry_compose();
    });
    settle(panel, cx);
}

fn menu_item(cx: &mut VisualTestContext, label: &str) -> ElementSnapshot {
    cx.update(|window, _| {
        snapshots(window)
            .into_iter()
            .find(|node| {
                node.role() == Some(gpui::accesskit::Role::MenuItem) && node.label() == Some(label)
            })
            .unwrap_or_else(|| panic!("missing menu item {label}"))
    })
}
fn has_menu_item(cx: &mut VisualTestContext, label: &str) -> bool {
    cx.update(|window, _| {
        snapshots(window).into_iter().any(|node| {
            node.role() == Some(gpui::accesskit::Role::MenuItem) && node.label() == Some(label)
        })
    })
}
fn click_menu(cx: &mut VisualTestContext, label: &str) {
    let at = menu_item(cx, label).bounds().center();
    cx.simulate_click(at, Default::default());
    draw(cx);
}
fn by_id(cx: &mut VisualTestContext, id: &str) -> ElementSnapshot {
    cx.update(|window, _| {
        snapshots(window)
            .into_iter()
            .find(|node| {
                node.path()
                    .last()
                    .is_some_and(|part| part.to_string() == id)
            })
            .unwrap_or_else(|| panic!("missing element {id}"))
    })
}
fn click_button_id(cx: &mut VisualTestContext, id: &str) {
    let at = by_id(cx, id).bounds().center();
    cx.simulate_click(at, Default::default());
    draw(cx);
}
fn key(cx: &mut VisualTestContext, key: &str) {
    let keystroke = Keystroke::parse(key).unwrap();
    cx.simulate_event(KeyDownEvent {
        physical_key: None,
        key_metadata: None,
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    cx.simulate_event(KeyUpEvent {
        physical_key: None,
        key_metadata: None,
        keystroke,
    });
    draw(cx);
}
fn wait_held(backend: &FakeBackend) {
    for _ in 0..300 {
        if backend.held_actions() == 1 {
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!("Compose action was not dispatched");
}
fn last_action(backend: &FakeBackend) -> Value {
    backend.requests("compose_run").last().unwrap()["action"].clone()
}

#[gpui::test]
fn table_renders_definition_order_missing_profiles_replicas_and_orphans(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    configure_table(&panel, cx, &backend);
    let table = cx.debug_bounds("compose-services-table").unwrap();
    let detail = cx.debug_bounds("containers-detail").unwrap();
    assert!(table.size.width >= gpui::px(460.));
    assert!(
        table.left() >= detail.left() && table.right() <= detail.right(),
        "normal detail should fit the table without horizontal overflow"
    );
    let mut last_bottom = table.top();
    for selector in [
        "compose-service-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "compose-service-missing-queue",
        "compose-service-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "compose-service-eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        "compose-service-ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    ] {
        let row = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("missing {selector}"));
        assert!(
            row.top() >= last_bottom,
            "rows must follow definition and replica order"
        );
        last_bottom = row.bottom();
    }
    assert!(
        cx.debug_bounds(
            "compose-service-cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        )
        .is_none(),
        "one-off jobs are not service rows"
    );
    for (suffix, expected) in [
        ("state", "Running"),
        ("cpu", "13%"),
        ("memory", "64.0 MiB"),
        ("ports", "8080→80"),
    ] {
        assert_eq!(
            by_id(cx, &format!("compose-service-{}-{suffix}", "a".repeat(64))).label(),
            Some(expected)
        );
    }
    assert_eq!(
        element(cx, "Not created · profile background, dev").role(),
        Some(gpui::accesskit::Role::Cell)
    );
    assert_eq!(
        element(cx, "web #2").role(),
        Some(gpui::accesskit::Role::Cell)
    );
    assert!(
        element(
            cx,
            "legacy · stack-legacy-1 · Not in the current Compose file"
        )
        .visible()
    );
    assert_eq!(
        by_id(cx, &format!("compose-service-{}-state", "e".repeat(64))).label(),
        Some("Paused")
    );
    assert_eq!(
        by_id(cx, &format!("compose-service-{}-state", "f".repeat(64))).label(),
        Some("Created")
    );
    assert_ne!(element(cx, "Up queue").disabled(), Some(true));
    let up = element(cx, "Up queue").bounds();
    assert_eq!(up.size.height, px(24.));
    let more = element(cx, "More actions for web").bounds();
    assert_eq!(more.size, size(px(28.), px(28.)));
    let web_row = by_id(cx, &format!("compose-service-{}", "a".repeat(64))).bounds();
    assert_eq!(web_row.size.height, px(35.));
    let action_cell = by_id(cx, "compose-service-missing-queue-actions").bounds();
    assert!(up.left() >= action_cell.left() && up.right() <= action_cell.right());
    let up_label_width = cx.update(|window, cx| {
        use gpui_kit::component::ActiveTheme;
        window
            .text_system()
            .shape_line(
                "Up".into(),
                px(11.5),
                &[gpui::TextRun {
                    len: 2,
                    font: gpui::font(cx.theme().font_family.clone()),
                    ..Default::default()
                }],
                None,
            )
            .width
    });
    assert!(
        (up.size.width - up_label_width - px(18.)).abs() <= px(1.),
        "Up must retain its 11.5px label, 8px side padding and 1px outline"
    );
    for name in [
        "stack-worker-1",
        "stack-web-1",
        "stack-web-2",
        "stack-legacy-1",
    ] {
        assert!(element(cx, &format!("Open container {name}")).visible());
    }
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn natural_columns_preserve_long_names_replicas_alignment_and_narrow_actions(
    cx: &mut TestAppContext,
) {
    let (panel, cx, backend) = fixture(cx);
    configure_table(&panel, cx, &backend);
    let short_width = by_id(cx, "compose-head-service").bounds().size.width;

    let service = "web-with-an-extraordinarily-long-service-name-that-must-stay-readable";
    let missing = "background-worker-with-a-long-name-that-is-not-created";
    let mut source = snapshot();
    let web = &mut source.containers[0];
    web.compose.as_mut().unwrap().service = service.into();
    web.cpu = Some(987.6);
    web.memory = Some(64 * 1024 * 1024);
    let mut replica = web.clone();
    replica.id = "e".repeat(64);
    replica.name = "stack-web-12".into();
    replica.compose.as_mut().unwrap().number = Some(12);
    replica.cpu = Some(1.);
    replica.memory = Some(512);
    source.containers.push(replica);
    backend.set_snapshot(source);
    let mut detail = compose_project();
    detail["services"] = json!([
        {"name":service,"image":"fixture/web:1","build":false,"profiles":[]},
        {"name":"worker","image":"fixture/worker:1","build":true,"profiles":[]},
        {"name":missing,"image":"fixture/background:1","build":false,"profiles":["background","development"]}
    ]);
    backend.set_compose_project(Ok(detail));
    panel.update(cx, |p, _| {
        p.client.refresh();
        p.client.retry_compose();
    });
    settle(&panel, cx);
    assert!(by_id(cx, "compose-head-service").bounds().size.width > short_width);

    for width in [1600., 540., 340.] {
        cx.simulate_resize(size(px(width), px(1200.)));
        draw(cx);
        let scroll = cx.debug_bounds("compose-services-scroll").unwrap();
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: scroll.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(10_000.), px(0.))),
            ..Default::default()
        });
        draw(cx);
        let table = cx.debug_bounds("compose-services-table").unwrap();
        if width <= 540. {
            assert!(
                table.size.width > scroll.size.width,
                "long content must be horizontally scrollable"
            );
        } else {
            assert!(
                table.right() <= scroll.right(),
                "the natural table {table:?} should fit a wide detail pane {scroll:?}"
            );
        }

        for id in ["a".repeat(64), "e".repeat(64)] {
            let row = format!("compose-service-{id}");
            for (column, suffix) in [
                ("service", "name"),
                ("state", "state"),
                ("cpu", "cpu"),
                ("memory", "memory"),
                ("ports", "ports"),
                ("actions", "actions"),
            ] {
                let header = by_id(cx, &format!("compose-head-{column}")).bounds();
                let cell = by_id(cx, &format!("{row}-{suffix}")).bounds();
                assert!((header.left() - cell.left()).abs() <= px(1.));
                assert!((header.right() - cell.right()).abs() <= px(1.));
            }
            for suffix in ["cpu", "memory"] {
                let cell = by_id(cx, &format!("{row}-{suffix}")).bounds();
                let value = by_id(cx, &format!("{row}-{suffix}-value")).bounds();
                assert!(value.left() >= cell.left());
                assert!(
                    (value.right() - cell.right()).abs() <= px(1.),
                    "{suffix} values must align at the column's right edge"
                );
            }
            let name = by_id(cx, &format!("{row}-label")).bounds();
            let button = by_id(cx, &format!("{row}-open")).bounds();
            assert!(
                name.size.width > short_width,
                "the full long name should retain its natural width"
            );
            assert!(name.left() >= button.left() && name.right() <= button.right());
        }
        let replica = by_id(cx, &format!("compose-service-{}-replica", "e".repeat(64))).bounds();
        let name = by_id(cx, &format!("compose-service-{}-label", "e".repeat(64))).bounds();
        let button = element(cx, "Open container stack-web-12").bounds();
        assert!(replica.left() > name.right() && replica.right() <= button.right());
        let missing_name = by_id(cx, &format!("compose-service-missing-{missing}-label")).bounds();
        let missing_cell = by_id(cx, &format!("compose-service-missing-{missing}-name")).bounds();
        assert!(missing_name.right() <= missing_cell.right());
        let span = by_id(cx, &format!("compose-service-missing-{missing}-state")).bounds();
        assert!((span.left() - by_id(cx, "compose-head-state").bounds().left()).abs() <= px(1.));
        assert!((span.right() - by_id(cx, "compose-head-ports").bounds().right()).abs() <= px(1.));

        cx.simulate_event(gpui::ScrollWheelEvent {
            position: scroll.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(-10_000.), px(0.))),
            ..Default::default()
        });
        draw(cx);
        for label in [
            format!("More actions for {service}"),
            format!("Up {missing}"),
        ] {
            let control = element(cx, &label).bounds();
            assert!(
                control.left() >= scroll.left() && control.right() <= scroll.right(),
                "{label} must remain reachable by scrolling at {width}px"
            );
        }
    }
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn service_navigation_targets_the_clicked_replica_and_supports_keyboard(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    configure_table(&panel, cx, &backend);
    // Focus the rendered container button through the normal tab order.
    click_button_id(cx, &format!("compose-service-{}-more", "a".repeat(64)));
    key(cx, "escape");
    cx.update(|window, cx| window.focus_prev(cx));
    draw(cx);
    assert_eq!(
        element(cx, "Open container stack-web-1").focused(),
        Some(true)
    );
    key(cx, "enter");
    settle(&panel, cx);
    assert_eq!(
        panel.read_with(cx, |p, _| p.client.selected_container().unwrap().id.clone()),
        "a".repeat(64)
    );
    select(&panel, cx, "p:stack");
    click(cx, "Open container stack-web-2");
    settle(&panel, cx);
    assert_eq!(
        panel.read_with(cx, |p, _| p.client.selected_container().unwrap().id.clone()),
        "e".repeat(64)
    );
    assert_eq!(
        backend.requests("container_inspect").last().unwrap()["container"],
        "e".repeat(64)
    );
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn service_menu_scale_opens_current_replicas_and_cancel_never_writes(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    configure_table(&panel, cx, &backend);
    let mut source = snapshot();
    source.containers[0].compose.as_mut().unwrap().number = Some(1);
    let mut replica = source.containers[0].clone();
    replica.id = "e".repeat(64);
    replica.name = "stack-web-2".into();
    replica.compose.as_mut().unwrap().number = Some(2);
    let mut stopped = replica.clone();
    stopped.id = "f".repeat(64);
    stopped.name = "stack-web-3".into();
    stopped.compose.as_mut().unwrap().number = Some(3);
    stopped.state = ContainerState::Exited;
    stopped.exit_code = Some(0);
    source.containers.extend([replica, stopped]);
    backend.set_snapshot(source);
    panel.update(cx, |panel, _| panel.client.refresh());
    settle(&panel, cx);

    click_button_id(cx, &format!("compose-service-{}-more", "e".repeat(64)));
    assert_ne!(menu_item(cx, "Scale…").disabled(), Some(true));
    click_menu(cx, "Scale…");
    cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
    assert!(element(cx, "Scale web").visible());
    assert_eq!(element(cx, "Containers").value(), Some("2"));
    assert!(element(cx, "Run 2").visible());
    assert!(!has_menu_item(cx, "Scale…"));
    assert_eq!(backend.writes(), 0, "opening Scale only prepares a draft");

    click(cx, "Cancel");
    cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    assert!(cx.debug_bounds("compose-review").is_none());
    assert_eq!(
        panel.read_with(cx, |panel, _| panel
            .client
            .selected_project()
            .map(str::to_owned)),
        Some("stack".into())
    );
    assert_eq!(backend.writes(), 0, "cancelling Scale must not run Compose");
}

#[gpui::test]
fn service_menu_sends_exact_options_and_missing_up_is_scoped(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    configure_table(&panel, cx, &backend);
    backend.hold_actions();
    for (label, build, pull, force) in [
        ("Recreate", false, false, true),
        ("Pull & recreate", false, true, true),
        ("Rebuild", true, false, true),
    ] {
        click(cx, "More actions for worker");
        assert!(has_menu_item(cx, "Rebuild"));
        click_menu(cx, label);
        wait_held(&backend);
        assert_eq!(
            backend.requests("compose_run").last().unwrap()["project"],
            "stack"
        );
        assert_eq!(
            last_action(&backend),
            json!({"kind":"up","services":["worker"],"build":build,"pull":pull,"forceRecreate":force})
        );
        assert!(!panel.read_with(cx, |p, _| p.client.can_compose_run("stack")));
        let requests = backend.count("compose_run");
        click(cx, "Up queue");
        assert_eq!(
            backend.count("compose_run"),
            requests,
            "pending service command blocks another write"
        );
        backend.fail_action("fixture command did not start");
        settle(&panel, cx);
    }
    click(cx, "More actions for web");
    assert!(!has_menu_item(cx, "Rebuild"));
    assert!(has_menu_item(cx, "Scale…"));
    key(cx, "escape");
    click(cx, "Up queue");
    wait_held(&backend);
    assert_eq!(
        last_action(&backend),
        json!({"kind":"up","services":["queue"],"build":false,"pull":false,"forceRecreate":false})
    );
    backend.fail_action("fixture missing service did not start");
    settle(&panel, cx);
    assert_eq!(backend.writes(), 4);
}

#[gpui::test]
fn already_open_service_menu_rejects_project_and_same_project_container_navigation(
    cx: &mut TestAppContext,
) {
    let (panel, cx, backend) = fixture(cx);
    configure_table(&panel, cx, &backend);
    for selection in ["c:stack-web-1", "p:other"] {
        select(&panel, cx, "p:stack");
        click(cx, "More actions for worker");
        let stale_at = menu_item(cx, "Recreate").bounds().center();
        // Keep the painted menu alive while navigation updates the shared
        // selection, reproducing a queued click from the previous render.
        panel.update(cx, |p, cx| {
            p.selected = Some(selection.into());
            p.sync_selection(cx);
        });
        cx.simulate_click(stale_at, Default::default());
        settle(&panel, cx);
        assert_eq!(
            backend.writes(),
            0,
            "old service callback cannot act after {selection}"
        );
        assert_eq!(
            panel.read_with(cx, |p, _| p.selected.clone()),
            Some(selection.into())
        );
    }
}

#[gpui::test]
fn service_pages_share_detail_scroll_and_bound_rendering_across_refreshes(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    let mut source = snapshot();
    let template = source.containers[0].clone();
    source.containers = (0..100)
        .map(|i| {
            let mut container = template.clone();
            container.id = format!("{i:064x}");
            container.name = format!("worker-{i:03}");
            container.compose.as_mut().unwrap().service = format!("worker-{i:03}");
            container
        })
        .collect();
    let mut detail = compose_project();
    detail["services"] = json!([]);
    backend.set_compose_project(Ok(detail));
    backend.set_snapshot(source.clone());
    select(&panel, cx, "p:stack");
    panel.update(cx, |p, _| {
        p.client.refresh();
        p.client.retry_compose();
    });
    settle(&panel, cx);
    let mounted = |cx: &mut VisualTestContext| {
        cx.update(|window, _| {
            snapshots(window)
                .into_iter()
                .filter(|node| {
                    node.label()
                        .is_some_and(|label| label.starts_with("Open container worker-"))
                })
                .count()
        })
    };
    assert_eq!(mounted(cx), 40);
    let rows = panel.read_with(cx, |p, _| p.service_table.borrow().rows.clone());
    assert_eq!(rows.len(), 100);
    draw(cx);
    assert!(panel.read_with(cx, |p, _| std::rc::Rc::ptr_eq(
        &rows,
        &p.service_table.borrow().rows
    )));
    let focus = panel.read_with(cx, |p, _| p.service_table.borrow().focus[0].clone());
    cx.update(|window, cx| focus.focus(window, cx));
    // A wheel gesture on the service table must move the page, not a nested list.
    let row = element(cx, "Open container worker-000").bounds();
    let before = panel.read_with(cx, |p, _| p.detail_scroll.offset());
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: row.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-100.))),
        ..Default::default()
    });
    draw(cx);
    let after = panel.read_with(cx, |p, _| p.detail_scroll.offset());
    assert!(
        after.y < before.y,
        "scrolling over a row must scroll the detail page"
    );
    let moved = element(cx, "Open container worker-000").bounds();
    assert!((moved.top() - row.top() - (after.y - before.y)).abs() <= px(1.));
    assert!(cx.update(|window, _| focus.is_focused(window)));
    panel.update(cx, |p, cx| {
        p.detail_scroll.set_offset(gpui::point(px(0.), px(0.)));
        cx.notify();
    });
    draw(cx);
    click(cx, "3");
    draw(cx);
    assert_eq!(panel.read_with(cx, |p, _| p.service_table.borrow().page), 2);
    assert_eq!(mounted(cx), 20);
    cx.update(|window, _| {
        assert!(
            snapshots(window)
                .iter()
                .any(|node| node.label() == Some("Open container worker-099"))
        );
        assert!(
            !snapshots(window)
                .iter()
                .any(|node| node.label() == Some("Open container worker-000"))
        );
    });
    // Refreshing the selected page keeps its anchor and mounted controls.
    panel.update(cx, |p, cx| p.refresh(cx));
    settle(&panel, cx);
    assert_eq!(panel.read_with(cx, |p, _| p.service_table.borrow().page), 2);
    source.containers.truncate(2);
    backend.set_snapshot(source);
    panel.update(cx, |p, cx| p.refresh(cx));
    settle(&panel, cx);
    assert_eq!(
        panel.read_with(cx, |p, _| p.service_table.borrow().rows.len()),
        2
    );
    assert_eq!(mounted(cx), 2);
}
