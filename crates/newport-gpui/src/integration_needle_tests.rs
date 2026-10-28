use crate::page_perf_tests::{PageHost, frame, owner};
use crate::{Newport, Page, theme};
use gpui::{AppContext, Entity, TestAppContext, VisualTestContext, px, size};
use gpui_kit as gpui;
use serde_json::json;

fn fixture(cx: &mut TestAppContext) -> (Entity<Newport>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        theme::install(false, cx);
        cx.set_reduce_motion(true);
    });
    let mut view = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let entity = cx.new(|cx| {
            let mut v = owner(window, cx);
            v.page = Page::Integration;
            v.snapshot["runtime"]["agentStatus"]["fixture"] =
                json!({"installed":true,"installedHash":"a".repeat(64)});
            v
        });
        view = Some(entity.clone());
        PageHost(entity)
    });
    visual.simulate_resize(size(px(550.), px(1500.)));
    frame(visual);
    (view.unwrap(), visual)
}

fn has(visual: &mut VisualTestContext, label: &str) -> bool {
    visual.update(|window, _| {
        gpui_kit::base::test_support::snapshots(window)
            .iter()
            .any(|n| n.label() == Some(label))
    })
}

fn click(visual: &mut VisualTestContext, label: &str) {
    let bounds = visual.update(|window, _| {
        gpui_kit::base::test_support::snapshots(window)
            .iter()
            .find(|n| n.label() == Some(label))
            .unwrap_or_else(|| panic!("Missing {label}"))
            .bounds()
    });
    visual.simulate_click(bounds.center(), Default::default());
    frame(visual);
}

#[gpui::test]
fn the_install_button_names_what_it_does(cx: &mut TestAppContext) {
    let (view, visual) = fixture(cx);
    assert!(has(visual, "Reinstall Needle"));
    assert!(has(visual, "Uninstall Needle…"));
    view.update(visual, |v, cx| {
        v.snapshot["runtime"]["agentStatus"]["fixture"]["updateAvailable"] = json!(true);
        cx.notify();
    });
    frame(visual);
    assert!(has(visual, "Update Needle"));
    view.update(visual, |v, cx| {
        v.snapshot["runtime"]["agentStatus"]["fixture"] = json!({"installed":false});
        cx.notify();
    });
    frame(visual);
    assert!(
        has(visual, "Set up Needle"),
        "the same action as every page"
    );
    assert!(!has(visual, "Uninstall Needle…"), "nothing to uninstall");
}

#[gpui::test]
fn a_server_that_skipped_setup_says_so_whatever_is_on_it(cx: &mut TestAppContext) {
    let (view, visual) = fixture(cx);
    view.update(visual, |v, cx| {
        let server = &mut v.snapshot["config"]["servers"][0];
        server["setupComplete"] = json!(false);
        server["clipboardEnabled"] = json!(true);
        // Another build left on the server is not this desktop's Needle.
        v.snapshot["runtime"]["agentStatus"]["fixture"]["buildRelation"] = json!("different");
        cx.notify();
    });
    frame(visual);
    assert!(has(visual, "Needle agent: Not set up"));
    assert!(has(visual, "Needle isn’t set up on this server."));
    assert!(has(visual, "Clipboard access: Needs Needle"));
    assert!(
        !has(visual, "Sharing connection: Disconnected"),
        "no connection to wait for"
    );
    assert!(!has(visual, "Needle agent: Installed"));
}

#[gpui::test]
fn uninstalling_asks_first_then_sends_the_reviewed_connection(cx: &mut TestAppContext) {
    let (view, visual) = fixture(cx);
    let (work, requests) = std::sync::mpsc::sync_channel(16);
    view.update(visual, |v, _| v.work = work.into());
    click(visual, "Uninstall Needle…");
    assert!(requests.try_recv().is_err(), "asking sends nothing");
    assert!(has(visual, "Uninstall"));
    click(visual, "Cancel");
    assert!(has(visual, "Uninstall Needle…"));
    assert!(requests.try_recv().is_err());
    click(visual, "Uninstall Needle…");
    click(visual, "Uninstall");
    let request = requests.try_recv().unwrap();
    assert_eq!(request.method, "uninstall_agent");
    assert_eq!(request.args["expectedConnection"]["id"], "fixture");
    assert!(has(visual, "Uninstalling Needle…"));
}

#[gpui::test]
fn a_changed_connection_cancels_the_confirmation(cx: &mut TestAppContext) {
    let (view, visual) = fixture(cx);
    let (work, requests) = std::sync::mpsc::sync_channel(16);
    view.update(visual, |v, _| v.work = work.into());
    click(visual, "Uninstall Needle…");
    view.update(visual, |v, cx| {
        v.snapshot["config"]["servers"][0]["sshPort"] = json!(2222);
        cx.notify();
    });
    frame(visual);
    click(visual, "Uninstall");
    assert!(
        requests.try_recv().is_err(),
        "a stale confirmation sends nothing"
    );
}

#[gpui::test]
fn uninstalling_brings_back_a_setup_finished_in_this_window(cx: &mut TestAppContext) {
    let (view, visual) = fixture(cx);
    let (work, _requests) = std::sync::mpsc::sync_channel(16);
    view.update(visual, |v, _| {
        v.work = work.into();
        v.page = Page::Overview;
        v.snapshot["config"]["servers"][0]["setupComplete"] = json!(false);
        v.poll_setup();
        v.setup_flow.as_mut().unwrap().dismissed = true;
        v.snapshot["config"]["servers"][0]["setupComplete"] = json!(true);
        v.page = Page::Integration;
    });
    frame(visual);
    click(visual, "Uninstall Needle…");
    click(visual, "Uninstall");
    view.update(visual, |v, cx| {
        v.snapshot["config"]["servers"][0]["setupComplete"] = json!(false);
        cx.notify();
    });
    frame(visual);
    view.read_with(visual, |v, _| assert!(v.setup_visible()));
}

#[gpui::test]
fn the_uninstall_confirmation_scrolls_into_view(cx: &mut TestAppContext) {
    let (view, visual) = fixture(cx);
    visual.simulate_resize(size(px(550.), px(420.)));
    frame(visual);
    view.update(visual, |v, cx| {
        v.integration_scroll.scroll_to_bottom();
        cx.notify();
    });
    frame(visual);
    click(visual, "Uninstall Needle…");
    frame(visual);
    let viewport = visual.debug_bounds("integrations-scroll-region").unwrap();
    let confirm = visual.update(|window, _| {
        gpui_kit::base::test_support::snapshots(window)
            .iter()
            .find(|n| n.label() == Some("Uninstall"))
            .unwrap()
            .bounds()
    });
    assert!(
        confirm.bottom() <= viewport.bottom(),
        "{confirm:?} {viewport:?}"
    );
}
