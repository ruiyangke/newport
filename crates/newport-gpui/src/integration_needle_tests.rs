use crate::page_perf_tests::{PageHost, frame};
use crate::project_library_state::typed_fixture::*;
use crate::project_library_state::typed_test_owner as owner;
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
            v.snapshot["runtime"]["agentStatus"][TYPED_SERVER] =
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
    if matches!(label, "Reinstall" | "Uninstall…" | "Check again") && !has(visual, label) {
        click(visual, "More Needle actions");
    }
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
fn the_active_bundled_runtime_outweighs_a_failed_default_agent_check(cx: &mut TestAppContext) {
    let (view, visual) = fixture(cx);
    view.update(visual, |v, cx| {
        v.snapshot["runtime"]["agentStatus"][TYPED_SERVER] = json!({
            "installed":true,"installedHash":"a".repeat(64),
            "bundledHash":"b".repeat(64),"buildRelation":"older",
            "updateAvailable":true,"error":"The default product cannot be verified."
        });
        v.snapshot["runtime"]["agentRuntime"][TYPED_SERVER] = json!({
            "status":"ready","hello":{"buildSha256":"b".repeat(64)}
        });
        cx.notify();
    });
    frame(visual);
    assert!(has(visual, "Needle: Ready"));
    assert!(!has(visual, "Update Needle"));
    assert!(!has(visual, "Update"));
    assert!(!has(visual, "Build aaaaaaa"));
    assert!(has(visual, "Details"));
}

#[gpui::test]
fn maintenance_is_disclosed_and_the_primary_action_follows_the_observed_state(
    cx: &mut TestAppContext,
) {
    let (view, visual) = fixture(cx);
    assert!(has(visual, "More Needle actions"));
    assert!(!has(visual, "Reinstall"));
    assert!(!has(visual, "Uninstall…"));
    view.update(visual, |v, cx| {
        let status = &mut v.snapshot["runtime"]["agentStatus"][TYPED_SERVER];
        status["updateAvailable"] = json!(true);
        status["buildRelation"] = json!("older");
        cx.notify();
    });
    frame(visual);
    assert!(has(visual, "Update now"));
    view.update(visual, |v, cx| {
        v.snapshot["runtime"]["agentStatus"][TYPED_SERVER] = json!({"installed":false});
        cx.notify();
    });
    frame(visual);
    assert!(has(visual, "Set up Needle"));
    click(visual, "More Needle actions");
    assert!(!has(visual, "Uninstall…"), "nothing to uninstall");
    assert!(!has(visual, "Reinstall"));
}

#[gpui::test]
fn a_server_that_skipped_setup_says_so_whatever_is_on_it(cx: &mut TestAppContext) {
    let (view, visual) = fixture(cx);
    view.update(visual, |v, cx| {
        let server = &mut v.snapshot["config"]["servers"][0];
        server["setupComplete"] = json!(false);
        server["clipboardEnabled"] = json!(true);
        // Another build left on the server is not this desktop's Needle.
        v.snapshot["runtime"]["agentStatus"][TYPED_SERVER]["buildRelation"] = json!("different");
        cx.notify();
    });
    frame(visual);
    assert!(has(visual, "Needle: Not set up"));
    assert!(has(visual, "Clipboard access: Needs Needle"));
    assert!(
        !has(visual, "Sharing connection: Disconnected"),
        "no connection to wait for"
    );
    assert!(!has(visual, "Needle: Installed"));
    assert!(
        !has(visual, "Build aaaaaaa"),
        "no version for a server without setup"
    );
}

#[gpui::test]
fn details_distinguish_the_running_and_default_build_and_reset_for_a_changed_connection(
    cx: &mut TestAppContext,
) {
    let (view, visual) = fixture(cx);
    view.update(visual, |v, cx| {
        let status = &mut v.snapshot["runtime"]["agentStatus"][TYPED_SERVER];
        status["bundledHash"] = json!("b".repeat(64));
        status["installedVersion"] = json!("0.7.0");
        status["bundledVersion"] = json!("0.6.4");
        v.snapshot["runtime"]["agentRuntime"][TYPED_SERVER] =
            json!({"status":"ready","hello":{"buildSha256":"b".repeat(64)}});
        cx.notify();
    });
    frame(visual);
    assert!(!has(visual, "Server default: 0.7.0"));
    click(visual, "Details");
    assert!(has(visual, "Server default: 0.7.0"));
    assert!(has(visual, "Used by this connection: 0.6.4"));
    assert!(has(visual, "Included with Newport: 0.6.4"));
    assert!(has(
        visual,
        &format!("Newport: {}", crate::about::version())
    ));
    view.update(visual, |v, cx| {
        v.snapshot["config"]["servers"][0]["sshPort"] = json!(2222);
        cx.notify();
    });
    frame(visual);
    assert!(!has(visual, "Server default: 0.7.0"));
    assert!(has(visual, "Details"));
}

#[gpui::test]
fn checking_uses_the_reviewed_connection_without_installing(cx: &mut TestAppContext) {
    let (view, visual) = fixture(cx);
    let (work, requests) = std::sync::mpsc::sync_channel(16);
    view.update(visual, |v, _| v.work = work.into());
    click(visual, "Check again");
    let request = requests.try_recv().unwrap();
    assert_eq!(kind(&request), Some(Kind::ConnectionInspectAgent));
    assert!(requests.try_recv().is_err());
    assert!(has(visual, "Needle: Checking…"));
}

#[gpui::test]
fn an_open_maintenance_menu_cannot_act_on_a_replaced_connection(cx: &mut TestAppContext) {
    let (view, visual) = fixture(cx);
    let (work, requests) = std::sync::mpsc::sync_channel(16);
    view.update(visual, |v, _| v.work = work.into());
    click(visual, "More Needle actions");
    view.update(visual, |v, cx| {
        v.snapshot["config"]["servers"][0]["sshPort"] = json!(2222);
        cx.notify();
    });
    frame(visual);
    click(visual, "Reinstall");
    assert!(requests.try_recv().is_err());
}

#[gpui::test]
fn uninstalling_asks_first_then_sends_the_reviewed_connection(cx: &mut TestAppContext) {
    let (view, visual) = fixture(cx);
    let (work, requests) = std::sync::mpsc::sync_channel(16);
    view.update(visual, |v, _| v.work = work.into());
    click(visual, "Uninstall…");
    assert!(requests.try_recv().is_err(), "asking sends nothing");
    assert!(has(visual, "Uninstall"));
    click(visual, "Cancel");
    assert!(has(visual, "More Needle actions"));
    assert!(requests.try_recv().is_err());
    click(visual, "Uninstall…");
    click(visual, "Uninstall");
    let request = requests.try_recv().unwrap();
    assert_eq!(kind(&request), Some(Kind::ConnectionUninstallAgent));
    let newport_protocol::desktop::Operation::Connection(operation) = operation(&request) else {
        panic!("Expected uninstall");
    };
    let newport_protocol::desktop::connection::Operation::UninstallAgent(args) = operation.as_ref()
    else {
        panic!("Expected uninstall");
    };
    assert_eq!(args.expected_server.id.to_string(), TYPED_SERVER);
    assert_eq!(args.expected_server.ssh_port, 22);
    assert_eq!(
        args.expected_server.auth_method,
        newport_protocol::desktop::configuration::AuthMethod::PublicKey
    );
    assert!(has(visual, "Needle: Uninstalling…"));
}

#[gpui::test]
fn a_changed_connection_cancels_the_confirmation(cx: &mut TestAppContext) {
    let (view, visual) = fixture(cx);
    let (work, requests) = std::sync::mpsc::sync_channel(16);
    view.update(visual, |v, _| v.work = work.into());
    click(visual, "Uninstall…");
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
    click(visual, "Uninstall…");
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
    click(visual, "Uninstall…");
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
