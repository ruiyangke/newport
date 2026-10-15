use crate::page_perf_tests::{PageHost, frame, owner};
use crate::{Newport, Page, theme};
use gpui::{AppContext, Entity, TestAppContext, VisualTestContext, px, size};
use gpui_kit as gpui;
use serde_json::json;

fn fixture(cx: &mut TestAppContext) -> (Entity<Newport>, &mut VisualTestContext, String) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        theme::install(false, cx);
        cx.set_reduce_motion(true);
    });
    let id = uuid::Uuid::new_v4().to_string();
    let mut view = None;
    let (_, visual) = cx.add_window_view(|window,cx| {
        let entity = cx.new(|cx| {
            let mut v=owner(window,cx); v.page=Page::Integration;
            v.snapshot["runtime"]["agentStatus"]["fixture"]=json!({"installed":true,"installedHash":"a".repeat(64)});
            v.snapshot["runtime"]["agentOperations"]["fixture"]=json!({"operations":[],"removals":[],"helperRecoveries":[],"helperHistory":{"supported":true,"history":{"buildSha256":"a".repeat(64),"operations":[{"operationId":id,"fingerprint":"b".repeat(64),"origin":"standalone","state":"applied","fileCount":3,"restoreBlocked":null}]}}});
            v
        }); view=Some(entity.clone()); PageHost(entity)
    });
    visual.simulate_resize(size(px(550.), px(1500.)));
    frame(visual);
    (view.unwrap(), visual, id)
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
fn click_restore(visual: &mut VisualTestContext) {
    let label = visual.update(|window, _| {
        gpui_kit::base::test_support::snapshots(window)
            .iter()
            .find_map(|n| {
                n.label()
                    .filter(|l| l.starts_with("Restore changes for "))
                    .map(str::to_owned)
            })
            .expect("a scoped restore action")
    });
    click(visual, &label);
}
#[gpui::test]
fn confirmation_is_scoped_cancelable_and_preserves_retry_identity(cx: &mut TestAppContext) {
    let (view, visual, id) = fixture(cx);
    let (work, requests) = std::sync::mpsc::sync_channel(16);
    view.update(visual, |v, _| v.work = work.into());
    click_restore(visual);
    assert!(has(visual, "Restore these shell changes?"));
    assert!(requests.try_recv().is_err());
    let cancelled = view.read_with(visual, |v, _| {
        v.needle_helper_ui["fixture"].review.clone().unwrap()
    });
    click(visual, "Cancel");
    click_restore(visual);
    view.update(visual, |v, cx| v.submit_helper_restore(cancelled, cx));
    assert!(
        requests.try_recv().is_err(),
        "cancelled callback stays cancelled"
    );
    click(visual, "Restore shell changes");
    let first = requests.try_recv().unwrap();
    assert_eq!(first.method, "restore_standalone_helpers");
    assert_eq!(first.args["operationId"], id);
    assert_eq!(first.args["build"], "a".repeat(64));
    assert_eq!(first.args["fingerprint"], "b".repeat(64));
    assert!(has(visual, "Restoring shell changes…"));
    let reviewed = view.read_with(visual, |v, _| {
        v.needle_helper_ui["fixture"].review.clone().unwrap()
    });
    view.update(visual, |v, cx| {
        v.submit_helper_restore(reviewed.clone(), cx)
    });
    assert!(
        requests.try_recv().is_err(),
        "no duplicate pending dispatch"
    );
    view.update(visual, |v, cx| {
        v.resources
            .complete(&first.key, first.generation, Err("response lost".into()));
        cx.notify();
    });
    frame(visual);
    click(visual, "Retry restore");
    assert_eq!(requests.try_recv().unwrap().args, first.args);
}
#[gpui::test]
fn stale_evidence_and_changed_connection_disable_confirmation(cx: &mut TestAppContext) {
    for change in ["connection", "hash", "error", "ownership"] {
        let (view, visual, _) = fixture(cx);
        let (work, requests) = std::sync::mpsc::sync_channel(8);
        view.update(visual, |v, _| v.work = work.into());
        click_restore(visual);
        let selected = view.read_with(visual, |v, _| {
            v.needle_helper_ui["fixture"].review.clone().unwrap()
        });
        view.update(visual, |v, cx| {
            match change {
                "connection" => {
                    v.snapshot["config"]["servers"][0]["sshHost"] = json!("another-host")
                }
                "hash" => {
                    v.snapshot["runtime"]["agentStatus"]["fixture"]["installedHash"] =
                        json!("c".repeat(64))
                }
                "error" => {
                    v.snapshot["runtime"]["agentOperations"]["fixture"]["helperHistory"]["error"] =
                        json!("offline")
                }
                _ => {
                    v.snapshot["runtime"]["agentOperations"]["fixture"]["helperHistory"]["history"]
                        ["operations"][0]["fingerprint"] = json!("d".repeat(64))
                }
            }
            v.submit_helper_restore(selected, cx);
            cx.notify();
        });
        frame(visual);
        assert!(has(
            visual,
            "The server or recorded ownership changed. Dismiss this review and inspect the history again."
        ));
        click(visual, "Restore shell changes");
        assert!(requests.try_recv().is_err(), "{change}");
    }
}
#[gpui::test]
fn durable_recovery_has_review_retry_and_completion_without_reinstall(cx: &mut TestAppContext) {
    let (view, visual, id) = fixture(cx);
    let (work, requests) = std::sync::mpsc::sync_channel(16);
    view.update(visual,|v,cx| {
        v.work=work.into();
        v.snapshot["runtime"]["agentOperations"]["fixture"]["helperRecoveries"]=json!([{"operationId":id,"build":"a".repeat(64),"fingerprint":"b".repeat(64),"phase":"restoring","error":"A shell file was edited"}]);cx.notify();
    });
    frame(visual);
    assert!(has(visual, "Shell restoration unfinished"));
    assert!(!has(visual, "Reinstall Needle"));
    assert!(!has(visual, "Remove Needle…"));
    click(visual, "Review files");
    let review = requests.try_recv().unwrap();
    assert_eq!(review.method, "review_agent");
    let path = format!("/home/fixture/{}/.bashrc", "nested/".repeat(45));
    view.update(visual,|v,cx| { v.resources.complete(&review.key,review.generation,Ok(json!({"operationId":id,"files":[{"index":0,"path":path,"status":"conflict","message":"Personal edits are preserved.","fingerprint":"e".repeat(64)}]})));cx.notify(); });
    frame(visual);
    for width in [550., 1000.] {
        visual.simulate_resize(size(px(width), px(1500.)));
        frame(visual);
        visual.update(|window, _| {
            for node in gpui_kit::base::test_support::snapshots(window) {
                if node.label().is_some_and(|s| {
                    [
                        "Retry restore",
                        "Review files",
                        "Keep current file",
                        path.as_str(),
                    ]
                    .contains(&s)
                }) {
                    assert!(
                        node.bounds().left() >= px(0.) && node.bounds().right() <= px(width),
                        "{:?}",
                        node.bounds()
                    );
                }
            }
        });
    }
    click(visual, "Keep current file");
    let keep = requests.try_recv().unwrap();
    assert_eq!(keep.method, "keep_agent_file");
    assert_eq!(keep.args["fingerprint"], "e".repeat(64));
    view.update(visual, |v, cx| {
        v.resources.complete(
            &keep.key,
            keep.generation,
            Ok(json!({"operationId":id,"files":[]})),
        );
        cx.notify();
    });
    frame(visual);
    click(visual, "Retry restore");
    let restore = requests.try_recv().unwrap();
    assert_eq!(restore.args["operationId"], id);
    view.update(visual, |v, cx| {
        v.resources
            .complete(&restore.key, restore.generation, Ok(json!("Restored")));
        v.snapshot["runtime"]["agentOperations"]["fixture"]["helperRecoveries"][0]["phase"] =
            json!("restored");
        cx.notify();
    });
    frame(visual);
    assert!(!has(visual, "Retry restore"));
    assert!(!has(visual, "Review files"));
    assert!(!has(visual, &format!("Restore changes for {id}")));
    assert!(has(visual, "Reinstall Needle"));
}
#[gpui::test]
fn history_paginates_and_refresh_is_read_only_and_scoped(cx: &mut TestAppContext) {
    let (view, visual, _) = fixture(cx);
    let (work, requests) = std::sync::mpsc::sync_channel(8);
    view.update(visual,|v,cx| {v.work=work.into();
        let history=&mut v.snapshot["runtime"]["agentOperations"]["fixture"]["helperHistory"]["history"]["operations"];
        let template=history[0].clone();
        *history=json!((0..16).map(|_|{let mut row=template.clone();row["operationId"]=json!(uuid::Uuid::new_v4());row["origin"]=json!("unattributed");row["restoreBlocked"]=json!("Ownership must be reviewed before restoration.");row}).collect::<Vec<_>>());cx.notify();
    });
    frame(visual);
    assert!(has(visual, "Show older changes"));
    click_restore(visual);
    assert!(requests.try_recv().is_err());
    click(visual, "Show older changes");
    assert_eq!(
        view.read_with(visual, |v, _| v.needle_helper_ui["fixture"].visible),
        15
    );
    view.update(visual, |v, cx| {
        v.needle_helper_ui.get_mut("fixture").unwrap().visible = 5;
        cx.notify();
    });
    frame(visual);
    click(visual, "Refresh history");
    let inspect = requests.try_recv().unwrap();
    assert_eq!(inspect.method, "inspect_agent");
    assert_eq!(inspect.args["expectedConnection"]["id"], "fixture");
}

#[gpui::test]
fn history_status_and_targets_remain_truthful_at_narrow_width(cx: &mut TestAppContext) {
    let (view, visual, id) = fixture(cx);
    let host = format!("{}.example.test", "long-server-name".repeat(24));
    view.update(visual, |v, cx| {
        v.snapshot["config"]["servers"][0]["sshHost"] = json!(host);
        cx.notify();
    });
    frame(visual);
    click_restore(visual);
    visual.update(|window, _| {
        let nodes = gpui_kit::base::test_support::snapshots(window);
        let target = nodes
            .iter()
            .find(|n| n.label().is_some_and(|s| s.contains(&host)))
            .unwrap();
        assert!(target.bounds().left() >= px(0.) && target.bounds().right() <= px(550.));
        assert!(target.label().unwrap().contains(&id[..8]));
    });
    click(visual, "Cancel");
    view.update(visual, |v, cx| {
        v.snapshot["runtime"]["agentStatus"]["fixture"]["installed"] = json!(false);
        v.snapshot["runtime"]["agentOperations"]["fixture"]["helperHistory"]["supported"] =
            json!(false);
        cx.notify();
    });
    frame(visual);
    assert!(has(
        visual,
        "Shell history is unavailable until a compatible Needle installation can be inspected."
    ));
    click_restore(visual);
    assert!(!has(visual, "Restore these shell changes?"));
}

#[gpui::test]
fn refresh_rejects_a_callback_from_a_replaced_connection(cx: &mut TestAppContext) {
    let (view, visual, _) = fixture(cx);
    let (work, requests) = std::sync::mpsc::sync_channel(8);
    view.update(visual, |v, cx| {
        v.work = work.into();
        let server = v.selected_server().unwrap();
        let scope = crate::terminal_scope::connection(&server, &v.snapshot);
        v.snapshot["config"]["servers"][0]["sshHost"] = json!("replacement");
        v.inspect_helper_history(server, scope, cx);
    });
    assert!(requests.try_recv().is_err());
}
