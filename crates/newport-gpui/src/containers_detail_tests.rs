use crate::containers::tests::{click, draw, fixture, select, select_logs, settle, snapshot};
use gpui::{TestAppContext, px, size};
use gpui_kit as gpui;

#[gpui::test]
fn detail_controls_follow_pane_width_and_keep_logs_below_tabs(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    for selection in ["p:stack", "c:stack-web-1"] {
        select_logs(&panel, cx, selection);
        // 1100 leaves the pane just wide enough beside the resizable list,
        // which a fixed 280px list estimate would have stacked.
        for width in [1200., 1100., 820., 600.] {
            cx.simulate_resize(size(px(width), px(1100.)));
            draw(cx);
            let pane = cx.debug_bounds("containers-detail").unwrap();
            let tabs = cx.debug_bounds("container-detail-tabs-wrap").unwrap();
            let tools = cx.debug_bounds("container-detail-log-controls").unwrap();
            let toolbar = cx.debug_bounds("container-log-toolbar").unwrap();
            let logs = cx.debug_bounds("container-log-viewport").unwrap();
            if pane.size.width - px(32.) > px(840.) {
                assert!(tools.left() >= tabs.right() + px(11.));
                assert!(
                    tools.top() < tabs.bottom(),
                    "wide controls share the tab row"
                );
            } else {
                assert!(toolbar.top() >= tabs.bottom() + px(7.));
                assert_eq!(tools.left(), tabs.left());
            }
            assert!(toolbar.left() >= pane.left());
            assert!(toolbar.right() <= pane.right() + px(1.));
            assert!(logs.top() >= tools.bottom());
            assert!(logs.top() >= tabs.bottom());
        }
        click(
            cx,
            if selection.starts_with("p:") {
                "Activity"
            } else {
                "Inspect"
            },
        );
        settle(&panel, cx);
        assert!(cx.debug_bounds("container-detail-log-controls").is_none());
        click(cx, "Logs");
        settle(&panel, cx);
        assert!(cx.debug_bounds("container-detail-log-controls").is_some());
    }
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn project_title_path_and_status_keep_reference_spacing(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    let mut source = snapshot();
    source.containers[0].compose.as_mut().unwrap().working_dir = Some("/srv/stack".into());
    backend.set_snapshot(source);
    panel.update(cx, |panel, _| panel.client.refresh());
    settle(&panel, cx);
    select(&panel, cx, "p:stack");
    let title = cx.debug_bounds("compose-project-title").unwrap();
    let path = cx.debug_bounds("compose-project-path").unwrap();
    let status = cx.debug_bounds("compose-project-status").unwrap();
    assert_eq!(path.top() - title.bottom(), px(2.));
    assert_eq!(status.top() - path.bottom(), px(8.));
}

#[gpui::test]
fn switching_detail_tabs_keeps_tabs_aligned_without_empty_controls_row(cx: &mut TestAppContext) {
    // Keep the RPC peer alive when this layout sweep spans an automatic refresh.
    let (panel, cx, _backend) = fixture(cx);
    for (selection, alternate) in [("c:stack-web-1", "Inspect"), ("p:stack", "Activity")] {
        select(&panel, cx, selection);
        for width in [
            1200., 1033., 1032., 913., 912., 820., 792., 791., 601., 600., 512., 511., 440.,
        ] {
            cx.simulate_resize(size(px(width), px(1100.)));
            draw(cx);
            click(cx, "Logs");
            settle(&panel, cx);
            assert_eq!(panel.read_with(cx, |panel, _| panel.width), width);
            let before = cx.debug_bounds("container-detail-bar").unwrap();
            let tabs = cx.debug_bounds("container-detail-tabs-wrap").unwrap();
            click(cx, alternate);
            settle(&panel, cx);
            let after = cx.debug_bounds("container-detail-bar").unwrap();
            assert_eq!(
                before.origin, after.origin,
                "tab bar moved for {selection} at width {width}"
            );
            assert_eq!(before.size.width, after.size.width);
            assert!(
                after.size.height <= px(36.),
                "empty controls row for {selection} at {width}"
            );
            assert_eq!(
                tabs,
                cx.debug_bounds("container-detail-tabs-wrap").unwrap(),
                "tabs moved at {width}"
            );
            assert!(cx.debug_bounds("container-detail-log-controls").is_none());
        }
    }
}
