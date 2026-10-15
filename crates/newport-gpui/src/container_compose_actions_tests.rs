use super::{ContainerComposeAction, ContainerComposeTarget};
use crate::containers::{
    ContainersPanel,
    compose_jobs::ComposeReview,
    tests::{
        FakeBackend, click, compose_project, draw, element, fixture, select, settle, snapshot,
    },
};
use gpui::{
    Entity, KeyDownEvent, TestAppContext, VisualTestContext, accesskit,
    base::test_support::{ElementSnapshot, snapshots},
};
use gpui_kit as gpui;
use newport_client::containers::Action;
use newport_domain::compose::{ComposeJob, JobState};
use newport_domain::containers as domain;
use serde_json::{Value, json};
use std::{thread, time::Duration};

fn menu_item(cx: &mut VisualTestContext, label: &str) -> Option<ElementSnapshot> {
    cx.update(|window, _| {
        snapshots(window).into_iter().find(|node| {
            node.role() == Some(accesskit::Role::MenuItem) && node.label() == Some(label)
        })
    })
}
fn click_menu(cx: &mut VisualTestContext, label: &str) {
    let position = menu_item(cx, label)
        .unwrap_or_else(|| panic!("missing {label}"))
        .bounds()
        .center();
    cx.simulate_click(position, Default::default());
    draw(cx);
}
fn close_menu(cx: &mut VisualTestContext) {
    let keystroke = gpui_kit::Keystroke::parse("escape").unwrap();
    cx.simulate_event(KeyDownEvent {
        physical_key: None,
        key_metadata: None,
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    cx.simulate_event(gpui_kit::KeyUpEvent {
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
    panic!("container Compose action was not dispatched");
}

/// Accept a new read without redrawing away the already-painted menu. This
/// exercises a queued callback against newer retained data, not a new menu.
fn settle_client_only(panel: &Entity<ContainersPanel>, cx: &mut VisualTestContext) {
    for _ in 0..300 {
        if panel.update(cx, |p, _| {
            p.client.poll(true);
            !p.client.has_pending()
        }) {
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!("fixture client read did not settle");
}

#[gpui::test]
fn container_compose_menu_stopped_build_service_sends_exact_flags_without_navigation(
    cx: &mut TestAppContext,
) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "c:stack-worker-1");
    click(cx, "Inspect");
    backend.hold_actions();
    for (label, build, pull) in [
        ("Recreate", false, false),
        ("Pull & recreate", false, true),
        ("Rebuild", true, false),
    ] {
        panel.update(cx, |p, _| {
            p.review = Some(Action::Remove {
                containers: vec!["b".repeat(64)],
                names: vec!["stack-worker-1".into()],
            });
            p.compose_state.proposed = Some(ComposeReview::Down {
                project: "stack".into(),
                volumes: false,
                orphans: false,
            });
        });
        click(cx, "More actions for stack-worker-1");
        assert!(
            menu_item(cx, "Pause").is_none(),
            "stopped service has Compose commands without Pause"
        );
        assert!(menu_item(cx, "Rebuild").is_some());
        click_menu(cx, label);
        wait_held(&backend);
        assert_eq!(
            backend.requests("compose_run").last().unwrap()["project"],
            "stack"
        );
        assert_eq!(
            backend.requests("compose_run").last().unwrap()["action"],
            json!({"kind":"up", "services":["worker"], "build":build, "pull":pull, "forceRecreate":true})
        );
        panel.read_with(cx, |p, _| {
            assert_eq!(p.client.selected_container().unwrap().id, "b".repeat(64));
            assert_eq!(p.tab, 1);
            assert!(p.review.is_none());
            assert!(p.compose_state.proposed.is_none());
            assert!(p.compose_state.show_activity_for.is_none());
        });
        backend.fail_action("Compose service could not be recreated");
        settle(&panel, cx);
        assert!(cx.debug_bounds("containers-error-compose-start").is_some());
        assert_eq!(panel.read_with(cx, |p, _| p.tab), 1);
    }
    click(cx, "More actions for stack-worker-1");
    click_menu(cx, "Recreate");
    wait_held(&backend);
    backend.set_compose_jobs(Ok(json!({"jobs":[ComposeJob {
        job: "fixture-job".into(), project: "stack".into(),
        command: "docker compose up --force-recreate worker".into(),
        state: JobState::Running, started_at: 1_000,
        finished_at: None, exit_code: None, output: String::new(), offset: 0,
    }]})));
    backend.release_action(json!({"job":"fixture-job"}));
    settle(&panel, cx);
    panel.read_with(cx, |p, _| {
        assert_eq!(p.client.selected_container().unwrap().id, "b".repeat(64));
        assert_eq!(p.tab, 1, "a successful start must not open Activity");
        assert!(p.compose_state.show_activity_for.is_none());
    });
    assert!(cx.debug_bounds("containers-error-compose-start").is_none());
    click(cx, "More actions for stack-worker-1");
    assert_eq!(menu_item(cx, "Recreate").unwrap().disabled(), Some(true));
    close_menu(cx);
    select(&panel, cx, "c:cache");
    assert!(
        cx.debug_bounds("containers-error-compose-start").is_none(),
        "project-scoped errors must not leak to standalone containers"
    );
    assert_eq!(backend.writes(), 4);
}

#[gpui::test]
fn container_compose_menu_hides_unsupported_build_cli_and_oneoff_choices(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "c:stack-web-1");
    click(cx, "More actions for stack-web-1");
    assert!(menu_item(cx, "Pause").is_some());
    assert!(menu_item(cx, "Recreate").is_some());
    assert!(menu_item(cx, "Pull & recreate").is_some());
    assert!(menu_item(cx, "Rebuild").is_none());
    close_menu(cx);
    select(&panel, cx, "c:stack-run-1");
    assert!(!cx.update(|window, _| {
        snapshots(window)
            .iter()
            .any(|node| node.label() == Some("More actions for stack-run-1"))
    }));
    let mut source = snapshot();
    source.containers[2].state = domain::ContainerState::Running;
    backend.set_snapshot(source);
    panel.update(cx, |p, _| p.client.refresh());
    settle(&panel, cx);
    click(cx, "More actions for stack-run-1");
    assert!(menu_item(cx, "Pause").is_some());
    assert!(menu_item(cx, "Recreate").is_none());
    close_menu(cx);
    let mut detail = compose_project();
    detail["cli"] = Value::Null;
    detail["cliError"] = json!("Compose is not installed");
    backend.set_compose_project(Ok(detail));
    select(&panel, cx, "c:stack-web-1");
    panel.update(cx, |p, _| p.client.retry_compose());
    settle(&panel, cx);
    click(cx, "More actions for stack-web-1");
    assert!(menu_item(cx, "Pause").is_some());
    assert!(menu_item(cx, "Recreate").is_none());
    close_menu(cx);
    select(&panel, cx, "c:stack-worker-1");
    assert!(!cx.update(|window, _| {
        snapshots(window)
            .iter()
            .any(|node| node.label() == Some("More actions for stack-worker-1"))
    }));
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn container_compose_menu_rejects_stale_selection_and_recreated_or_relabeled_identity(
    cx: &mut TestAppContext,
) {
    let (panel, cx, backend) = fixture(cx);
    for selection in ["c:stack-worker-1", "p:stack", "c:cache"] {
        select(&panel, cx, "c:stack-web-1");
        click(cx, "More actions for stack-web-1");
        let stale_at = menu_item(cx, "Recreate")
            .unwrap_or_else(|| panic!("missing Recreate before {selection}"))
            .bounds()
            .center();
        // Preserve the painted menu while its originating selection changes.
        panel.update(cx, |p, cx| {
            p.selected = Some(selection.into());
            p.sync_selection(cx);
        });
        cx.simulate_click(stale_at, Default::default());
        settle(&panel, cx);
        assert_eq!(backend.writes(), 0, "stale callback after {selection}");
        close_menu(cx);
    }
    for changed in ["id", "service", "oneoff", "project"] {
        backend.set_snapshot(snapshot());
        panel.update(cx, |p, _| p.client.refresh());
        settle(&panel, cx);
        select(&panel, cx, "c:stack-web-1");
        click(cx, "More actions for stack-web-1");
        let mut source = snapshot();
        match changed {
            "id" => source.containers[0].id = "e".repeat(64),
            "service" => source.containers[0].compose.as_mut().unwrap().service = "worker".into(),
            "oneoff" => source.containers[0].compose.as_mut().unwrap().oneoff = true,
            "project" => source.containers[0].compose.as_mut().unwrap().project = "other".into(),
            _ => unreachable!(),
        }
        backend.set_snapshot(source);
        panel.update(cx, |p, _| p.client.refresh());
        settle_client_only(&panel, cx);
        click_menu(cx, "Recreate");
        settle(&panel, cx);
        assert_eq!(
            backend.writes(),
            0,
            "stale callback after {changed} changed"
        );
        close_menu(cx);
    }
}

#[gpui::test]
fn container_compose_menu_blocks_duplicate_and_engine_actions_without_clearing_rejected_drafts(
    cx: &mut TestAppContext,
) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "c:stack-web-1");
    backend.hold_actions();
    click(cx, "More actions for stack-web-1");
    click_menu(cx, "Recreate");
    wait_held(&backend);
    click(cx, "More actions for stack-web-1");
    assert_eq!(menu_item(cx, "Recreate").unwrap().disabled(), Some(true));
    assert_eq!(menu_item(cx, "Pause").unwrap().disabled(), Some(true));
    click_menu(cx, "Recreate");
    let target = panel.read_with(cx, |p, _| {
        ContainerComposeTarget::from_container(p.client.selected_container().unwrap()).unwrap()
    });
    panel.update(cx, |p, cx| {
        p.compose_state.proposed = Some(ComposeReview::Down {
            project: "stack".into(),
            volumes: false,
            orphans: false,
        });
        p.run_container_compose(&target, ContainerComposeAction::PullRecreate, cx);
        assert!(
            p.compose_state.proposed.is_some(),
            "a rejected dispatch leaves a competing draft intact"
        );
    });
    assert_eq!(backend.count("compose_run"), 1);
    close_menu(cx);
    backend.fail_action("Compose failed to start");
    settle(&panel, cx);
    click(cx, "Restart");
    wait_held(&backend);
    click(cx, "More actions for stack-web-1");
    assert_eq!(menu_item(cx, "Recreate").unwrap().disabled(), Some(true));
    click_menu(cx, "Recreate");
    assert_eq!(backend.count("compose_run"), 1);
    close_menu(cx);
    backend.fail_action("Restart failed");
    settle(&panel, cx);
    assert_eq!(backend.writes(), 2);
}

#[gpui::test]
fn container_compose_menu_revalidates_build_and_cli_after_opening(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "c:stack-worker-1");
    for change in ["build", "cli", "error"] {
        backend.set_compose_project(Ok(compose_project()));
        panel.update(cx, |p, _| p.client.retry_compose());
        settle(&panel, cx);
        click(cx, "More actions for stack-worker-1");
        let mut detail = compose_project();
        if change == "build" {
            detail["services"][1]["build"] = json!(false);
        }
        if change == "cli" {
            detail["cli"] = Value::Null;
        }
        backend.set_compose_project(if change == "error" {
            Err("Compose configuration read failed".into())
        } else {
            Ok(detail)
        });
        panel.update(cx, |p, _| p.client.retry_compose());
        settle_client_only(&panel, cx);
        click_menu(cx, "Rebuild");
        settle(&panel, cx);
        assert_eq!(backend.writes(), 0, "stale Rebuild callback after {change}");
        close_menu(cx);
    }
    // Keep the fixture's accessible command contract explicit.
    assert!(element(cx, "Start").visible());
}
