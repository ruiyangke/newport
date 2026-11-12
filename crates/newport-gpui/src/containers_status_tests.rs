use super::ContainersPanel;
use super::tests::{FakeBackend, click, draw, element, fixture, select, settle, snapshot};
use gpui::{Entity, TestAppContext, VisualTestContext};
use gpui_kit as gpui;
use newport_client::containers::Action;
use newport_domain::compose::{ComposeJob, JobState};
use newport_domain::containers::{
    ContainerHealth, ContainerInspect, ContainerState, HealthResult, ProjectVerb,
};
use serde_json::json;
use std::{thread, time::Duration};

fn refresh(panel: &Entity<ContainersPanel>, cx: &mut VisualTestContext) {
    panel.update(cx, |panel, cx| panel.refresh(cx));
    settle(panel, cx);
}

fn labels(cx: &mut VisualTestContext) -> Vec<String> {
    cx.update(|window, _| {
        gpui::base::test_support::snapshots(window)
            .iter()
            .filter_map(|node| node.label().map(str::to_owned))
            .collect()
    })
}

fn health(backend: &FakeBackend, output: &str) {
    let mut data = snapshot();
    data.containers[0].health = Some(ContainerHealth::Unhealthy);
    data.containers[0].restart_count = 3;
    backend.set_inspect(Ok(ContainerInspect {
        id: data.containers[0].id.clone(),
        name: data.containers[0].name.clone(),
        last_health: vec![
            HealthResult {
                exit_code: 1,
                output: "obsolete failure".into(),
                ..Default::default()
            },
            HealthResult {
                exit_code: 2,
                output: output.into(),
                ..Default::default()
            },
            HealthResult {
                exit_code: 0,
                output: "success".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    }));
    backend.set_snapshot(data);
}

fn project_status(cx: &mut VisualTestContext) -> String {
    cx.update(|window, _| {
        let node = gpui::base::test_support::snapshots(window)
            .into_iter()
            .find(|node| {
                node.role() == Some(gpui::accesskit::Role::Status)
                    && node
                        .path()
                        .iter()
                        .any(|part| part.to_string() == "compose-project-status")
            })
            .expect("project status is rendered");
        assert_eq!(node.role(), Some(gpui::accesskit::Role::Status));
        node.label().unwrap().to_owned()
    })
}

fn wait_project_action(
    panel: &Entity<ContainersPanel>,
    cx: &mut VisualTestContext,
    backend: &FakeBackend,
) {
    for _ in 0..300 {
        panel.update(cx, |p, cx| p.poll(true, cx));
        draw(cx);
        if backend.held_actions() == 1 {
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!("project action did not reach the fixture");
}

#[gpui::test]
fn project_header_tracks_each_engine_action_and_restores_snapshot_after_failure(
    cx: &mut TestAppContext,
) {
    let (panel, cx, backend) = fixture(cx);
    let mut data = snapshot();
    let mut other = data.containers[0].clone();
    other.id = "e".repeat(64);
    other.name = "other-web-1".into();
    other.compose.as_mut().unwrap().project = "other".into();
    data.containers.push(other);
    backend.set_snapshot(data);
    refresh(&panel, cx);
    select(&panel, cx, "p:stack");
    let original = project_status(cx);
    backend.hold_actions();
    for (verb, message) in [
        (ProjectVerb::Start, "Starting the project…"),
        (ProjectVerb::Stop, "Stopping the project…"),
        (ProjectVerb::Restart, "Restarting the project…"),
    ] {
        panel.update(cx, |p, cx| {
            p.run_container_action(
                Action::Project {
                    project: "stack".into(),
                    verb,
                    expected_ids: vec!["a".repeat(64), "b".repeat(64)],
                },
                cx,
            )
        });
        wait_project_action(&panel, cx, &backend);
        assert_eq!(
            project_status(cx),
            format!("{message} · 1 of 2 services running")
        );
        // Navigation cannot present this project's operation as another
        // container's status, and returning restores the pending status.
        panel.update_in(cx, |p, window, cx| p.select("c:cache".into(), window, cx));
        draw(cx);
        assert!(cx.debug_bounds("compose-project-status").is_none());
        panel.update_in(cx, |p, window, cx| p.select("p:other".into(), window, cx));
        draw(cx);
        assert!(project_status(cx).starts_with("Running · 1 of 1 service running"));
        panel.update_in(cx, |p, window, cx| p.select("p:stack".into(), window, cx));
        draw(cx);
        assert_eq!(
            project_status(cx),
            format!("{message} · 1 of 2 services running")
        );
        backend.fail_action("fixture project action rejected");
        settle(&panel, cx);
        assert_eq!(project_status(cx), original);
    }
    assert_eq!(backend.writes(), 3);
}

#[gpui::test]
fn project_header_shows_compose_start_before_receipt_and_restores_after_rejection(
    cx: &mut TestAppContext,
) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "p:stack");
    let original = project_status(cx);
    backend.hold_actions();
    click(cx, "Up");
    wait_project_action(&panel, cx, &backend);
    assert_eq!(
        project_status(cx),
        "Running a Compose command… · 1 of 2 services running"
    );
    assert!(!labels(cx).iter().any(|label| label == "View output"));
    backend.fail_action("fixture Compose start rejected");
    settle(&panel, cx);
    assert_eq!(project_status(cx), original);
    assert_eq!(backend.writes(), 1);
}

#[gpui::test]
fn project_output_link_is_in_header_and_hidden_on_activity(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    let job = ComposeJob {
        job: "running-project".into(),
        project: "stack".into(),
        command: "docker compose up --detach".into(),
        state: JobState::Running,
        started_at: 1_000,
        finished_at: None,
        exit_code: None,
        output: String::new(),
        offset: 0,
    };
    backend.set_compose_jobs(Ok(json!({"jobs": [job]})));
    backend.set_compose_output("running-project", Ok(json!(job)));
    select(&panel, cx, "p:stack");
    panel.update(cx, |p, _| p.client.retry_jobs());
    settle(&panel, cx);
    let output = element(cx, "View output");
    assert!(
        output
            .path()
            .iter()
            .any(|part| part.to_string() == "compose-project-status")
    );
    click(cx, "View output");
    settle(&panel, cx);
    assert_eq!(panel.read_with(cx, |p, _| p.tab), 1);
    assert!(!labels(cx).iter().any(|label| label == "View output"));
    click(cx, "Logs");
    settle(&panel, cx);
    assert_eq!(
        project_status(cx),
        "Running a Compose command… · 1 of 2 services running"
    );
    element(cx, "View output");
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn latest_failed_health_has_restart_context_and_accessible_status(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    health(&backend, "  connection refused\n");
    refresh(&panel, cx);
    select(&panel, cx, "c:stack-web-1");
    assert_eq!(
        element(cx, "Unhealthy").role(),
        Some(gpui::accesskit::Role::Status)
    );
    assert_eq!(
        element(
            cx,
            "Failing its health check. Restarted 3× so far. connection refused"
        )
        .role(),
        Some(gpui::accesskit::Role::Alert)
    );
    assert!(
        !labels(cx)
            .iter()
            .any(|label| label.contains("obsolete failure"))
    );
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn empty_failed_output_falls_back_to_exit_code_and_stopped_health_is_not_reused(
    cx: &mut TestAppContext,
) {
    let (panel, cx, backend) = fixture(cx);
    health(&backend, " \n\t ");
    refresh(&panel, cx);
    select(&panel, cx, "c:stack-web-1");
    element(
        cx,
        "Failing its health check. Restarted 3× so far. check exited 2",
    );
    let mut data = snapshot();
    data.containers[0].state = ContainerState::Exited;
    data.containers[0].exit_code = Some(1);
    data.containers[0].health = Some(ContainerHealth::Unhealthy);
    backend.set_snapshot(data);
    refresh(&panel, cx);
    element(cx, "Exited with an error (code 1).");
    assert!(
        !labels(cx)
            .iter()
            .any(|label| label.contains("check exited 2"))
    );
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn pending_action_is_exact_and_failure_restores_observed_state(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "c:stack-web-1");
    backend.hold_actions();
    click(cx, "Restart");
    for _ in 0..300 {
        panel.update(cx, |p, cx| p.poll(true, cx));
        draw(cx);
        if backend.held_actions() == 1 {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(backend.held_actions(), 1);
    assert_eq!(
        element(cx, "Restarting…").role(),
        Some(gpui::accesskit::Role::Status)
    );
    backend.fail_action("fixture restart rejected");
    settle(&panel, cx);
    assert_eq!(
        element(cx, "Running").role(),
        Some(gpui::accesskit::Role::Status)
    );
    assert!(!labels(cx).iter().any(|label| label == "Restarting…"));
    assert_eq!(
        panel.read_with(cx, |p, _| p.error("action")).as_deref(),
        Some("fixture restart rejected")
    );
    assert!(
        cx.debug_bounds("containers-error-action").is_some(),
        "the failure remains visible after progress clears"
    );
    assert_eq!(backend.writes(), 1);
}

#[gpui::test]
fn lifecycle_status_uses_snapshot_time_and_reports_finished_exit(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    let mut data = snapshot();
    data.containers[0].started_at = Some(1_000);
    data.containers[1].finished_at = Some(61_000);
    backend.set_snapshot(data);
    refresh(&panel, cx);
    select(&panel, cx, "c:stack-web-1");
    panel.update(cx, |p, cx| {
        p.updated = Some(91_000);
        cx.notify();
    });
    draw(cx);
    assert_eq!(
        element(cx, "Running for 1m").role(),
        Some(gpui::accesskit::Role::Status)
    );
    select(&panel, cx, "c:stack-worker-1");
    assert_eq!(
        element(cx, "Crashed since 30s ago · exit code 1").role(),
        Some(gpui::accesskit::Role::Status)
    );
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn identical_background_snapshot_advances_observed_container_time(cx: &mut TestAppContext) {
    use std::{cell::Cell, rc::Rc};
    let (panel, cx, backend) = fixture(cx);
    let now = chrono::Local::now().timestamp_millis();
    let mut data = snapshot();
    data.containers[0].started_at = Some(now - 121_000);
    backend.set_snapshot(data);
    refresh(&panel, cx);
    select(&panel, cx, "c:stack-web-1");
    panel.update(cx, |panel, cx| {
        panel.live = false;
        panel.sync_selection(cx);
    });
    settle(&panel, cx);
    panel.update(cx, |panel, cx| {
        panel.updated = Some(now - 61_000);
        cx.notify();
    });
    draw(cx);
    assert!(labels(cx).iter().any(|label| label == "Running for 1m"));
    let notifications = Rc::new(Cell::new(0));
    let observed = notifications.clone();
    let _observer =
        cx.update(|_, cx| cx.observe(&panel, move |_, _| observed.set(observed.get() + 1)));
    // The normal periodic refresh is deliberately not a manual refresh, whose
    // completion already forces presentation to clear its loading state.
    thread::sleep(Duration::from_millis(5_100));
    settle(&panel, cx);
    assert!(labels(cx).iter().any(|label| label == "Running for 2m"));
    assert_eq!(
        notifications.get(),
        1,
        "only the changed formatted age should repaint"
    );
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn recreated_container_does_not_show_previous_incarnations_health_failure(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    health(&backend, "old identity failure");
    refresh(&panel, cx);
    select(&panel, cx, "c:stack-web-1");
    element(
        cx,
        "Failing its health check. Restarted 3× so far. old identity failure",
    );
    let mut data = snapshot();
    data.containers[0].id = "e".repeat(64);
    data.containers[0].health = Some(ContainerHealth::Unhealthy);
    backend.set_snapshot(data);
    refresh(&panel, cx);
    element(cx, "Failing its health check.");
    assert!(
        !labels(cx)
            .iter()
            .any(|label| label.contains("old identity failure"))
    );
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn compose_output_link_captures_container_identity_and_opens_only_retained_job(
    cx: &mut TestAppContext,
) {
    let (panel, cx, backend) = fixture(cx);
    let job = ComposeJob {
        job: "running-web".into(),
        project: "stack".into(),
        command: "docker compose up --detach web".into(),
        state: JobState::Running,
        started_at: 1_000,
        finished_at: None,
        exit_code: None,
        output: String::new(),
        offset: 0,
    };
    backend.set_compose_jobs(Ok(json!({"jobs": [job]})));
    backend.set_compose_output("running-web", Ok(json!(job)));
    select(&panel, cx, "c:stack-web-1");
    panel.update(cx, |p, _| p.client.retry_jobs());
    settle(&panel, cx);
    element(cx, "Compose is working on stack…");
    element(cx, "View output");
    let id = snapshot().containers[0].id.clone();
    select(&panel, cx, "c:cache");
    assert!(!cx.update(
        |window, cx| panel.update(cx, |p, cx| p.open_container_activity(
            (&id, "stack-web-1"),
            "stack",
            "running-web",
            window,
            cx
        ))
    ));
    assert_eq!(
        panel.read_with(cx, |p, _| p.selected.clone()).as_deref(),
        Some("c:cache")
    );
    select(&panel, cx, "c:stack-web-1");
    let mut data = snapshot();
    data.containers[0].id = "e".repeat(64);
    backend.set_snapshot(data);
    refresh(&panel, cx);
    assert!(!cx.update(
        |window, cx| panel.update(cx, |p, cx| p.open_container_activity(
            (&id, "stack-web-1"),
            "stack",
            "running-web",
            window,
            cx
        ))
    ));
    click(cx, "View output");
    settle(&panel, cx);
    assert_eq!(
        panel.read_with(cx, |p, _| (p.selected.clone(), p.tab)),
        (Some("p:stack".into()), 1)
    );
    assert_eq!(backend.writes(), 0);
}

#[test]
fn an_off_podman_socket_offers_the_command_needle_names() {
    assert_eq!(
        super::podman_socket_command(
            "Podman is installed, but its socket isn't on. Turn it on with: systemctl --user enable --now podman.socket"
        )
        .as_deref(),
        Some("systemctl --user enable --now podman.socket")
    );
    assert_eq!(
        super::podman_socket_command("Docker isn't installed or isn't running on this server."),
        None
    );
}
