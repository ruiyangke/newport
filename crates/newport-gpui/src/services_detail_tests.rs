use super::{ActiveData, Selection, ServicesPanel, tests};
use gpui_kit::base::test_support::{ElementSnapshot, snapshots};
use gpui_kit::{self as gpui, Entity, KeyDownEvent, Keystroke, TestAppContext, VisualTestContext};
use newport_domain::services::{Scope, TimerUnit};

fn element(cx: &mut VisualTestContext, label: &str) -> ElementSnapshot {
    cx.update(|window, _| {
        snapshots(window)
            .into_iter()
            .find(|element| element.label() == Some(label))
            .unwrap_or_else(|| panic!("missing {label}"))
    })
}
fn click(cx: &mut VisualTestContext, label: &str) {
    let point = element(cx, label).bounds().center();
    cx.simulate_click(point, Default::default());
    tests::draw(cx);
}
fn key(cx: &mut VisualTestContext, key: &'static str) {
    cx.simulate_event(KeyDownEvent {
        physical_key: None,
        key_metadata: None,
        keystroke: Keystroke::parse(key).unwrap(),
        is_held: false,
        prefer_character_input: false,
    });
    tests::draw(cx);
}
fn assert_tab(panel: &Entity<ServicesPanel>, cx: &mut VisualTestContext, expected: usize) {
    cx.update(|window, cx| {
        let panel = panel.read(cx);
        assert_eq!(panel.tab, expected);
        assert!(panel.detail_tab_focus[expected].is_focused(window));
    });
}

#[gpui::test]
fn detail_tabs_have_one_tab_stop_and_activate_with_navigation_keys(cx: &mut TestAppContext) {
    let (panel, cx, backend) = tests::fixture(cx);
    click(cx, "Logs");
    assert_tab(&panel, cx, 0);
    // The selected tab is the only tab stop, in both directions.
    cx.update(|window, cx| window.focus_next(cx));
    cx.update(|window, cx| {
        assert!(!panel.read(cx).detail_tab_focus[1].is_focused(window));
        window.focus_prev(cx);
    });
    assert_tab(&panel, cx, 0);

    key(cx, "right");
    assert_tab(&panel, cx, 1);
    tests::settle(&panel, cx);
    assert!(panel.read_with(cx, |panel, _| matches!(
        panel.active_data,
        ActiveData::File(..)
    )));
    let file_reads = backend.count("service_unit_file");
    for key_name in ["enter", "space", "cmd-right"] {
        key(cx, key_name);
        assert_tab(&panel, cx, 1);
    }
    tests::settle(&panel, cx);
    assert_eq!(backend.count("service_unit_file"), file_reads);

    key(cx, "right");
    assert_tab(&panel, cx, 0);
    key(cx, "left");
    assert_tab(&panel, cx, 1);
    key(cx, "home");
    assert_tab(&panel, cx, 0);
    key(cx, "end");
    assert_tab(&panel, cx, 1);
    cx.update(|window, cx| {
        window.focus_prev(cx);
        assert!(!panel.read(cx).detail_tab_focus[0].is_focused(window));
        window.focus_next(cx);
    });
    assert_tab(&panel, cx, 1);
    assert_eq!(backend.count("service_action"), 0);
}

#[gpui::test]
fn timer_without_target_has_one_operable_unit_file_tab(cx: &mut TestAppContext) {
    let (panel, cx, backend) = tests::fixture(cx);
    let mut snapshot = tests::snapshot();
    snapshot.timers.push(TimerUnit {
        name: "orphan.timer".into(),
        scope: Scope::User,
        ..Default::default()
    });
    backend.set_snapshot(snapshot);
    panel.update(cx, |panel, cx| panel.refresh(cx));
    tests::settle(&panel, cx);
    panel.update(cx, |panel, cx| {
        panel.select(Selection::Unit("user:orphan.timer".into()), cx);
    });
    tests::draw(cx);
    click(cx, "Unit file");
    for key_name in ["left", "right", "home", "end", "enter", "space"] {
        key(cx, key_name);
        assert_tab(&panel, cx, 1);
    }
    assert!(panel.read_with(cx, |panel, _| {
        matches!(&panel.active_data, ActiveData::File(name, Scope::User) if name == "orphan.timer")
    }));
    let labels = cx.update(|window, _| snapshots(window));
    assert!(!labels.iter().any(|element| element.label() == Some("Logs")));
    assert_eq!(backend.count("service_action"), 0);
}

#[gpui::test]
fn timer_service_link_keeps_scope_and_natural_width(cx: &mut TestAppContext) {
    let (panel, cx, backend) = tests::fixture(cx);
    let mut snapshot = tests::snapshot();
    let mut same_name = snapshot.services[0].clone();
    same_name.scope = Scope::User;
    snapshot.services.push(same_name);
    snapshot.timers.push(TimerUnit {
        name: "app.timer".into(),
        scope: Scope::User,
        unit: Some("app.service".into()),
        ..Default::default()
    });
    backend.set_snapshot(snapshot);
    panel.update(cx, |panel, cx| panel.refresh(cx));
    tests::settle(&panel, cx);
    panel.update(cx, |panel, cx| {
        panel.select(Selection::Unit("user:app.timer".into()), cx);
    });
    tests::draw(cx);
    let link = element(cx, "app.service · running");
    let pane = cx.debug_bounds("services-detail").unwrap();
    assert!(link.bounds().size.width < pane.size.width);
    assert_eq!(link.role(), Some(gpui::Role::Button));
    click(cx, "app.service · running");
    assert_eq!(
        panel.read_with(cx, |panel, _| panel.selected.clone()),
        Some(Selection::Unit("user:app.service".into()))
    );
    assert!(panel.read_with(cx, |panel, _| {
        matches!(&panel.active_data, ActiveData::Logs(target, _) if target.scope == Scope::User && target.unit.as_deref() == Some("app.service"))
    }));
    assert_eq!(backend.count("service_action"), 0);
}

#[gpui::test]
fn unit_file_content_follows_tabs_without_an_empty_toolbar(cx: &mut TestAppContext) {
    let (panel, cx, backend) = tests::fixture(cx);
    let mut snapshot = tests::snapshot();
    snapshot.timers.push(TimerUnit {
        name: "app.timer".into(),
        scope: Scope::System,
        unit: Some("app.service".into()),
        ..Default::default()
    });
    backend.set_snapshot(snapshot);
    panel.update(cx, |panel, cx| panel.refresh(cx));
    tests::settle(&panel, cx);
    for name in ["app.service", "app.timer"] {
        panel.update(cx, |panel, cx| {
            panel.select(Selection::Unit(format!("system:{name}")), cx)
        });
        for width in [1200., 820., 600., 440.] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(1100.)));
            tests::draw(cx);
            click(cx, "Unit file");
            tests::settle(&panel, cx);
            let tab = element(cx, "Unit file").bounds();
            let editor = element(cx, &format!("{name} unit file")).bounds();
            assert!(
                editor.top() - tab.bottom() <= gpui::px(12.),
                "empty toolbar at {width} for {name}: {tab:?} {editor:?}"
            );
        }
    }
}

#[gpui::test]
fn journal_error_wraps_and_keeps_retry_inside_the_pane(cx: &mut TestAppContext) {
    let (panel, cx, backend) = tests::fixture(cx);
    panel.update(cx, |panel, cx| {
        panel.select(panel.effective_selection(cx), cx)
    });
    backend.hold_reads();
    panel.update(cx, |panel, cx| {
        panel.client.retry_logs();
        cx.notify();
    });
    let message = "This account cannot read that journal: the system journal needs root or the systemd-journal group. Check the server account permissions and try again.";
    let mut failed = false;
    for _ in 0..300 {
        panel.update(cx, |panel, cx| panel.poll(true, cx));
        tests::draw(cx);
        if backend.fail_read("service_logs", message) {
            failed = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(failed);
    tests::settle(&panel, cx);
    for width in [1200., 820., 600., 440.] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(1100.)));
        tests::draw(cx);
        let pane = cx.debug_bounds("services-detail").unwrap();
        let error = cx.debug_bounds("services-logs-error").unwrap();
        let retry = element(cx, "Try reading logs again").bounds();
        assert!(
            error.left() >= pane.left() && error.right() <= pane.right(),
            "error overflow at {width}: {error:?} {pane:?}"
        );
        assert!(
            error.size.height > gpui::px(21.),
            "error must wrap at {width}: {error:?}"
        );
        assert!(
            retry.left() >= pane.left() && retry.right() <= pane.right(),
            "retry overflow at {width}: {retry:?}"
        );
    }
}

#[gpui::test]
fn unit_file_editor_keeps_a_readable_height(cx: &mut TestAppContext) {
    let (panel, cx, _backend) = tests::fixture(cx);
    panel.update(cx, |panel, cx| {
        panel.select(panel.effective_selection(cx), cx)
    });
    for width in [1200., 820., 600.] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(900.)));
        tests::draw(cx);
        click(cx, "Unit file");
        tests::settle(&panel, cx);
        let editor = element(cx, "app.service unit file").bounds();
        assert!(
            editor.size.height >= gpui::px(280.),
            "unit file editor collapses at width {width}: {editor:?}"
        );
    }
}

#[gpui::test]
fn detail_bar_right_aligns_controls_without_reserving_empty_space(cx: &mut TestAppContext) {
    // Keep the RPC peer alive when this layout sweep spans an automatic refresh.
    let (panel, cx, _backend) = tests::fixture(cx);
    panel.update(cx, |panel, cx| {
        panel.select(panel.effective_selection(cx), cx)
    });
    for width in [
        1200., 913., 912., 820., 792., 791., 601., 600., 512., 511., 440.,
    ] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(1100.)));
        tests::draw(cx);
        click(cx, "Logs");
        tests::settle(&panel, cx);
        assert_eq!(panel.read_with(cx, |panel, _| panel.width), width);
        let before = cx.debug_bounds("service-detail-bar").unwrap();
        let controls = cx.debug_bounds("service-detail-controls").unwrap();
        let search = element(cx, "Search logs");
        let search = cx.update(|window, _| crate::field_frame(window, &search));
        assert!(
            (search.right() - controls.right()).abs() <= gpui::px(1.),
            "controls not aligned at {width}: {search:?} {controls:?}"
        );
        click(cx, "Unit file");
        tests::settle(&panel, cx);
        let after = cx.debug_bounds("service-detail-bar").unwrap();
        assert_eq!(before.top(), after.top(), "tabs moved at width {width}");
        assert!(after.size.height <= before.size.height);
        assert!(cx.debug_bounds("service-detail-controls").is_none());
        cx.update(|window, _| {
            assert!(
                !snapshots(window)
                    .iter()
                    .any(|node| node.label() == Some("Search logs"))
            );
        });
    }
}
