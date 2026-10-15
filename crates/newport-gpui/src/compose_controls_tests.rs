use super::{ComposeReview, ContainersPanel};
use crate::containers::tests::{
    FakeBackend, click, compose_project, draw, element, fixture, select, settle, snapshot,
};
use gpui::base::test_support::snapshots;
use gpui::{
    Entity, Focusable, KeyDownEvent, Keystroke, TestAppContext, VisualTestContext, px, size,
};
use gpui_kit as gpui;
use gpui_kit::component::WindowExt;
use newport_domain::containers::ContainerState;
use serde_json::{Value, json};
use std::{thread, time::Duration};

fn has(cx: &mut VisualTestContext, label: &str) -> bool {
    cx.update(|window, _| snapshots(window).iter().any(|e| e.label() == Some(label)))
}
fn key(cx: &mut VisualTestContext, name: &str) {
    cx.simulate_event(KeyDownEvent {
        physical_key: None,
        key_metadata: None,
        keystroke: Keystroke::parse(name).unwrap(),
        is_held: false,
        prefer_character_input: false,
    });
    draw(cx);
}
fn wait_action(panel: &Entity<ContainersPanel>, cx: &mut VisualTestContext, backend: &FakeBackend) {
    for _ in 0..300 {
        panel.update(cx, |p, cx| p.poll(true, cx));
        draw(cx);
        if backend.held_actions() > 0 {
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!("action not dispatched");
}
fn detail(
    panel: &Entity<ContainersPanel>,
    cx: &mut VisualTestContext,
    backend: &FakeBackend,
    data: Value,
) {
    backend.set_compose_project(Ok(data));
    panel.update(cx, |p, _| p.client.retry_compose());
    settle(panel, cx);
}
fn down(cx: &mut VisualTestContext) {
    click(cx, "More");
    click(cx, "Down…");
}
fn field(panel: &Entity<ContainersPanel>, cx: &mut VisualTestContext, value: &str, replicas: bool) {
    panel.update_in(cx, |p, window, cx| {
        let field = if replicas {
            &p.compose_state.replicas
        } else {
            &p.compose_state.typed
        };
        field.update(cx, |f, cx| f.set_value(value, window, cx));
        cx.notify();
    });
    draw(cx);
}

#[gpui::test]
fn compose_direct_commands_match_reference_and_inspection_errors_do_not_block(
    cx: &mut TestAppContext,
) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "p:stack");
    let mut data = compose_project();
    data["configError"] = json!("Cannot decode optional inspection output");
    detail(&panel, cx, &backend, data);
    backend.hold_actions();
    click(cx, "Stop all");
    assert!(panel.read_with(cx, |p, _| p.review.is_some()));
    key(cx, "escape");
    for (label, expected) in [
        (
            "Up",
            json!({"kind":"up","services":[],"build":false,"pull":false,"forceRecreate":false}),
        ),
        (
            "Pull images & up",
            json!({"kind":"up","services":[],"build":false,"pull":true,"forceRecreate":false}),
        ),
        (
            "Rebuild & up",
            json!({"kind":"up","services":[],"build":true,"pull":false,"forceRecreate":false}),
        ),
        ("Pull images only", json!({"kind":"pull","services":[]})),
        (
            "Build images only",
            json!({"kind":"build","services":[],"noCache":false}),
        ),
    ] {
        let before = backend.count("compose_run");
        if label != "Up" {
            click(cx, "More");
        }
        click(cx, label);
        wait_action(&panel, cx, &backend);
        assert_eq!(backend.count("compose_run"), before + 1);
        assert_eq!(
            backend.requests("compose_run").last().unwrap(),
            &json!({"id":"fixture","project":"stack","action":expected})
        );
        assert!(panel.read_with(cx, |p, _| p.compose_state.proposed.is_none()));
        assert!(panel.read_with(cx, |p, _| p.review.is_none()));
        click(cx, "Up");
        assert_eq!(backend.count("compose_run"), before + 1);
        backend.fail_action("fixture preflight rejection");
        settle(&panel, cx);
        assert!(has(cx, "Up"));
    }
    let mut data = compose_project();
    data["services"][1]["build"] = json!(false);
    detail(&panel, cx, &backend, data);
    click(cx, "More");
    assert!(!has(cx, "Build images only"));
    assert!(!has(cx, "Rebuild & up"));
    key(cx, "escape");
}

#[gpui::test]
fn down_options_require_exact_volume_confirmation_and_reset_on_reopen(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "p:stack");
    backend.hold_actions();
    down(cx);
    cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
    assert_eq!(
        element(cx, "The project’s containers and networks are removed. Images stay, and Up creates everything again from the compose files.").role(),
        Some(gpui::Role::Label)
    );
    assert!(panel.read_with(cx, |p, _| matches!(
        p.compose_state.proposed,
        Some(ComposeReview::Down {
            volumes: false,
            orphans: false,
            ..
        })
    )));
    click(cx, "Delete the project’s volumes");
    click(
        cx,
        "Also remove containers for services no longer in the files",
    );
    assert_eq!(backend.writes(), 0, "option changes only update the draft");
    for value in ["", "Stack", "stack ", "other"] {
        field(&panel, cx, value, false);
        click(cx, "Down & delete volumes");
        assert_eq!(backend.writes(), 0);
    }
    field(&panel, cx, "stack", false);
    panel.update_in(cx, |p, window, cx| {
        p.compose_state
            .typed
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
    });
    key(cx, "enter");
    assert_eq!(
        backend.writes(),
        0,
        "typing the name does not implicitly approve deletion"
    );
    click(cx, "Down & delete volumes");
    wait_action(&panel, cx, &backend);
    assert_eq!(
        backend.requests("compose_run")[0]["action"],
        json!({"kind":"down","volumes":true,"removeOrphans":true})
    );
    assert!(panel.read_with(cx, |p, _| p.compose_state.proposed.is_none()));
    cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    backend.fail_action("fixture failed to start");
    settle(&panel, cx);
    down(cx);
    assert!(panel.read_with(cx, |p, cx| {
        p.compose_state.typed.read(cx).value().is_empty()
    }));
    assert!(panel.read_with(cx, |p, _| matches!(
        p.compose_state.proposed,
        Some(ComposeReview::Down {
            volumes: false,
            orphans: false,
            ..
        })
    )));
    key(cx, "escape");
    assert!(panel.read_with(cx, |p, _| p.compose_state.proposed.is_none()));
    cx.update(|window, cx| assert!(panel.read(cx).detail_focus.is_focused(window)));
    assert_eq!(backend.writes(), 1);
}

#[gpui::test]
fn scale_initializes_from_replicas_validates_boundaries_and_keeps_service_identity(
    cx: &mut TestAppContext,
) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "p:stack");
    backend.hold_actions();
    click(cx, "More actions for worker");
    click(cx, "Scale…");
    // The worker fixture is exited: the description counts running replicas.
    // The input below still defaults to one so scaling can start the service.
    assert_eq!(
        element(cx, "How many worker containers should run? It runs 0 now. Services that publish a fixed host port can only run one.").role(),
        Some(gpui::Role::Label)
    );
    cx.update(|window, cx| {
        assert!(window.has_active_dialog(cx));
        assert!(
            panel
                .read(cx)
                .compose_state
                .replicas
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        );
    });
    key(cx, "enter");
    assert_eq!(
        backend.writes(),
        0,
        "Enter in the replica field does not submit"
    );
    assert_eq!(
        panel.read_with(cx, |p, cx| p
            .compose_state
            .replicas
            .read(cx)
            .value()
            .to_string()),
        "1"
    );
    let bounds = cx.debug_bounds("compose-scale-input").unwrap();
    assert_eq!(bounds.size, size(px(96.), px(34.)));
    let increment = element(cx, "Increase containers").bounds();
    let decrement = element(cx, "Decrease containers").bounds();
    assert!(increment.right() <= bounds.right() && increment.top() >= bounds.top());
    assert!(decrement.bottom() <= bounds.bottom() && decrement.top() >= increment.bottom());
    let replicas = |cx: &mut VisualTestContext| {
        panel.read_with(cx, |p, cx| {
            p.compose_state.replicas.read(cx).value().to_string()
        })
    };
    click(cx, "Increase containers");
    assert_eq!(replicas(cx), "2");
    click(cx, "Decrease containers");
    assert_eq!(replicas(cx), "1");
    key(cx, "up");
    assert_eq!(replicas(cx), "2");
    key(cx, "down");
    assert_eq!(replicas(cx), "1");
    field(&panel, cx, "20", true);
    click(cx, "Increase containers");
    key(cx, "up");
    assert_eq!(replicas(cx), "20");
    field(&panel, cx, "0", true);
    click(cx, "Decrease containers");
    key(cx, "down");
    assert_eq!(replicas(cx), "0");
    assert_eq!(backend.writes(), 0, "stepping only changes the scale draft");
    for value in ["", "-1", "21", "1.5", "invalid"] {
        field(&panel, cx, value, true);
        assert!(panel.read_with(cx, |p, cx| p.compose_review_action(cx).is_none()));
        click(cx, "Scale");
        assert_eq!(backend.writes(), 0);
    }
    for count in [0, 20] {
        field(&panel, cx, &count.to_string(), true);
        if count == 20 {
            panel.update_in(cx, |p, window, cx| {
                p.compose_state.confirm_focus.focus(window, cx)
            });
            key(cx, "enter");
        } else {
            click(cx, &format!("Run {count}"));
        }
        wait_action(&panel, cx, &backend);
        assert_eq!(
            backend.requests("compose_run").last().unwrap()["action"],
            json!({"kind":"scale","service":"worker","replicas":count})
        );
        backend.fail_action("fixture preflight rejection");
        settle(&panel, cx);
        click(cx, "More actions for worker");
        click(cx, "Scale…");
    }
    select(&panel, cx, "c:cache");
    assert!(panel.read_with(cx, |p, _| p.compose_state.proposed.is_none()));
    cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    assert_eq!(backend.writes(), 2);
}

#[gpui::test]
fn no_cli_offers_existing_container_actions_with_the_complete_membership(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "p:stack");
    let mut data = compose_project();
    data["cli"] = Value::Null;
    data["cliError"] = json!("Compose is not installed");
    detail(&panel, cx, &backend, data);
    assert!(!has(cx, "Up"));
    assert!(!has(cx, "More"));
    assert!(has(cx, "Start the rest"));
    backend.hold_actions();
    click(cx, "Start the rest");
    wait_action(&panel, cx, &backend);
    let requested = backend.requests("project_action");
    assert_eq!(requested[0]["verb"], "start");
    assert_eq!(requested[0]["expectedIds"].as_array().unwrap().len(), 2);
    backend.fail_action("fixture rejection");
    settle(&panel, cx);
    click(cx, "Restart all");
    assert!(panel.read_with(cx, |p, _| p.review.is_some()));
    assert!(cx.debug_bounds("container-action-review").is_some());
    let bounds = cx.debug_bounds("confirm-container-action").unwrap();
    cx.simulate_click(bounds.center(), Default::default());
    draw(cx);
    wait_action(&panel, cx, &backend);
    assert_eq!(
        backend.requests("project_action")[1]["expectedIds"],
        requested[0]["expectedIds"]
    );
    key(cx, "escape");
    assert!(
        panel.read_with(cx, |p, _| p.review.is_some()),
        "dispatched operations cannot be dismissed as cancelled"
    );
    backend.fail_action("fixture rejection");
    settle(&panel, cx);
    key(cx, "escape");
    assert!(panel.read_with(cx, |p, _| p.review.is_none()));
}

#[gpui::test]
fn finished_job_cleanup_rechecks_immutable_ids_and_excludes_services(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "p:stack");
    click(cx, "Remove 1 finished job");
    assert!(panel.read_with(cx, |p, _| p.review.is_some()));
    assert!(cx.debug_bounds("container-action-review").is_some());
    let mut fresh = snapshot();
    fresh.containers[2].state = ContainerState::Running;
    backend.set_snapshot(fresh);
    panel.update(cx, |p, cx| p.refresh(cx));
    settle(&panel, cx);
    let confirm = cx.debug_bounds("confirm-container-action").unwrap();
    cx.simulate_click(confirm.center(), Default::default());
    draw(cx);
    assert_eq!(backend.writes(), 0);
    key(cx, "escape");
    backend.set_snapshot(snapshot());
    panel.update(cx, |p, cx| p.refresh(cx));
    settle(&panel, cx);
    backend.hold_actions();
    click(cx, "Remove 1 finished job");
    click(cx, "Remove");
    wait_action(&panel, cx, &backend);
    let requests = backend.requests("containers_remove");
    assert_eq!(requests[0]["containers"], json!(["c".repeat(64)]));
    backend.release_action(json!({"removed":1}));
    settle(&panel, cx);
    assert_eq!(backend.writes(), 1);
}

#[gpui::test]
fn compact_down_dialog_keeps_controls_visible_and_escape_does_not_write(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    cx.simulate_resize(size(px(560.), px(400.)));
    select(&panel, cx, "p:stack");
    down(cx);
    click(cx, "Delete the project’s volumes");
    draw(cx);
    let dialog = cx.debug_bounds("dialog-0").unwrap();
    assert!(dialog.left() >= px(0.));
    assert!(dialog.right() <= px(560.));
    let cancel = element(cx, "Cancel").bounds();
    assert!(cancel.bottom() <= px(400.));
    key(cx, "escape");
    assert!(panel.read_with(cx, |p, _| p.compose_state.proposed.is_none()));
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn compose_dialog_contains_focus_and_cannot_dispatch_after_hidden_or_retired(
    cx: &mut TestAppContext,
) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "p:stack");
    down(cx);
    for _ in 0..12 {
        key(cx, "tab");
        cx.update(|window, cx| {
            assert!(
                window
                    .active_dialog_focus_handle(cx)
                    .unwrap()
                    .contains_focused(window, cx)
            );
        });
    }
    // A backdrop click never reaches the background Up command.
    let up = element(cx, "Up").bounds();
    cx.simulate_click(up.center(), Default::default());
    draw(cx);
    assert_eq!(backend.writes(), 0);
    cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
    panel.update(cx, |p, cx| p.poll(false, cx));
    draw(cx);
    assert!(panel.read_with(cx, |p, _| p.compose_state.proposed.is_none()));
    cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));

    down(cx);
    panel.update(cx, |p, cx| p.retire(cx));
    draw(cx);
    assert!(panel.read_with(cx, |p, _| p.compose_state.proposed.is_none()));
    cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn retained_compose_confirmation_cannot_submit_or_close_a_reopened_dialog(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "p:stack");
    backend.hold_actions();
    down(cx);
    let (token, review) = panel.read_with(cx, |p, _| {
        (
            p.review_dialog.as_ref().unwrap().token,
            p.compose_state.proposed.clone().unwrap(),
        )
    });
    key(cx, "enter");
    cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    down(cx);
    panel.update_in(cx, |p, window, cx| {
        p.confirm_compose_review(token, &review, window, cx);
        p.dismiss_compose_review(token, window, cx);
    });
    draw(cx);
    cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
    assert_eq!(backend.writes(), 0);

    let (token, review) = panel.read_with(cx, |p, _| {
        (
            p.review_dialog.as_ref().unwrap().token,
            p.compose_state.proposed.clone().unwrap(),
        )
    });
    panel.update_in(cx, |p, window, cx| {
        p.confirm_compose_review(token, &review, window, cx);
        p.confirm_compose_review(token, &review, window, cx);
    });
    wait_action(&panel, cx, &backend);
    assert_eq!(backend.writes(), 1);
    assert_eq!(backend.requests("compose_run")[0]["project"], "stack");
    cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    backend.fail_action("fixture rejection");
    settle(&panel, cx);
}

#[gpui::test]
fn externally_closed_compose_dialog_cannot_submit_into_a_replacement_without_polling(
    cx: &mut TestAppContext,
) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "p:stack");
    down(cx);
    let (token, review) = panel.read_with(cx, |p, _| {
        (
            p.review_dialog.as_ref().unwrap().token,
            p.compose_state.proposed.clone().unwrap(),
        )
    });
    cx.update(|window, cx| {
        window.close_dialog(cx);
        window.open_dialog(cx, |dialog, _, _| dialog.title("Replacement dialog"));
    });
    // No draw or poll is allowed to clean up the old registration first.
    panel.update_in(cx, |p, window, cx| {
        p.confirm_compose_review(token, &review, window, cx);
        p.dismiss_compose_review(token, window, cx);
    });
    draw(cx);
    assert_eq!(backend.writes(), 0);
    cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
}
