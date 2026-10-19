use super::{ComposeJob, ContainersPanel};
use crate::containers::tests::{FakeBackend, click, draw, element, fixture, select, settle};
use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use gpui_kit as gpui;
use newport_domain::compose::JobState;
use serde_json::{Value, json};
use std::{thread, time::Duration};

fn job(id: &str, project: &str, state: JobState, output: &str) -> Value {
    json!(ComposeJob {
        job: id.into(),
        project: project.into(),
        command: format!("docker compose {id}"),
        state,
        started_at: 1_000,
        finished_at: (!state.is_running()).then_some(3_000),
        exit_code: (!state.is_running()).then_some(if state == JobState::Failed { 17 } else { 0 }),
        output: output.into(),
        offset: output.len() as u64,
    })
}
fn jobs(backend: &FakeBackend, items: &[Value]) {
    backend.set_compose_jobs(Ok(json!({"jobs": items.iter().cloned().map(|mut value| {
        value["output"] = json!(""); value["offset"] = json!(0); value
    }).collect::<Vec<_>>()})));
    for value in items {
        backend.set_compose_output(value["job"].as_str().unwrap(), Ok(value.clone()));
    }
}
fn refresh_jobs(panel: &Entity<ContainersPanel>, cx: &mut VisualTestContext) {
    panel.update(cx, |p, _| p.client.retry_jobs());
    settle(panel, cx);
}
fn output(panel: &Entity<ContainersPanel>, cx: &VisualTestContext) -> String {
    panel.read_with(cx, |p, _| {
        p.activity
            .rows
            .iter()
            .map(|r| r.message.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    })
}
fn wait_for(panel: &Entity<ContainersPanel>, cx: &mut VisualTestContext, ready: impl Fn() -> bool) {
    for _ in 0..300 {
        panel.update(cx, |p, cx| p.poll(true, cx));
        draw(cx);
        if ready() {
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!("expected fixture request was not dispatched");
}
fn click_history(cx: &mut VisualTestContext, id: &str) {
    let label = cx.update(|window, _| {
        gpui_kit::base::test_support::snapshots(window)
            .iter()
            .filter_map(|node| node.label())
            .find(|label| label.contains(&format!("docker compose {id} ·")))
            .unwrap()
            .to_owned()
    });
    click(cx, &label);
}
fn button(
    cx: &mut VisualTestContext,
    label: &str,
) -> gpui_kit::base::test_support::ElementSnapshot {
    cx.update(|window, _| {
        gpui_kit::base::test_support::snapshots(window)
            .into_iter()
            .find(|node| {
                node.label() == Some(label) && node.role() == Some(gpui::accesskit::Role::Button)
            })
            .unwrap_or_else(|| panic!("missing button {label}"))
    })
}

#[gpui::test]
fn hidden_compose_output_defers_projection_until_return(cx: &mut TestAppContext) {
    use std::rc::Rc;
    let (panel, cx, backend) = fixture(cx);
    jobs(
        &backend,
        &[job("live", "stack", JobState::Running, "first\n")],
    );
    select(&panel, cx, "p:stack");
    refresh_jobs(&panel, cx);
    click(cx, "Activity");
    settle(&panel, cx);
    assert_eq!(output(&panel, cx), "first");
    let previous = panel.read_with(cx, |p, _| p.activity.rows.clone());
    backend.set_compose_output(
        "live",
        Ok(job("live", "stack", JobState::Running, "first\nsecond\n")),
    );
    panel.update(cx, |p, _| p.client.retry_job_output());
    for _ in 0..300 {
        panel.update(cx, |p, cx| p.poll(false, cx));
        cx.run_until_parked();
        if !panel.read_with(cx, |p, _| p.has_pending()) {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panel.read_with(cx, |p, _| {
        assert!(!p.has_pending());
        assert_eq!(p.client.job_output().unwrap()["output"], "first\nsecond\n");
        assert!(
            Rc::ptr_eq(&previous, &p.activity.rows),
            "hidden activity retains its old projection"
        );
    });
    let calls = backend.count("compose_job");
    panel.update(cx, |p, cx| p.poll(true, cx));
    draw(cx);
    assert_eq!(output(&panel, cx), "first\nsecond");
    assert_eq!(
        backend.count("compose_job"),
        calls,
        "return reuses cached output"
    );
}

#[gpui::test]
fn short_activity_output_starts_at_top_while_following(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    jobs(
        &backend,
        &[job("live", "stack", JobState::Running, "first\nsecond\n")],
    );
    select(&panel, cx, "p:stack");
    refresh_jobs(&panel, cx);
    click(cx, "Activity");
    settle(&panel, cx);
    for height in [1000., 1200.] {
        cx.simulate_resize(gpui::size(px(980.), px(height)));
        draw(cx);
        let viewport = cx.debug_bounds("compose-output-viewport").unwrap();
        let first = panel.read_with(cx, |p, _| p.activity.selection.text_position(0, 0).unwrap());
        assert!((first.y - viewport.top() - px(9.)).abs() < px(3.));
        assert!(panel.read_with(cx, |p, _| p.activity.list.is_following_tail()));
    }
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn history_stays_project_scoped_preserves_choice_and_falls_back_after_expiry(
    cx: &mut TestAppContext,
) {
    let (panel, cx, backend) = fixture(cx);
    let newest = job("latest", "stack", JobState::Succeeded, "latest output\n");
    let older = job("older", "stack", JobState::Failed, "older error\n");
    jobs(
        &backend,
        &[
            job(
                "foreign",
                "other",
                JobState::Running,
                "secret other project",
            ),
            newest.clone(),
            older.clone(),
        ],
    );
    select(&panel, cx, "p:stack");
    refresh_jobs(&panel, cx);
    click(cx, "Activity");
    settle(&panel, cx);
    assert_eq!(output(&panel, cx), "latest output");
    assert_eq!(panel.read_with(cx, |p, _| p.activity.data.jobs().len()), 2);
    click(cx, "Earlier commands (1)");
    assert_eq!(element(cx, "Earlier commands (1)").expanded(), Some(true));
    click_history(cx, "older");
    settle(&panel, cx);
    assert_eq!(output(&panel, cx), "older error");
    let current_label = format!(
        "Current command: docker compose older · {}",
        newport_domain::compose::job_status(&serde_json::from_value(older.clone()).unwrap(), 3_000)
            .label
    );
    assert!(button(cx, &current_label).visible());
    assert!(element(cx, "Copy command output").bounds().size.width > px(0.));
    jobs(
        &backend,
        &[
            job("newer", "stack", JobState::Succeeded, "newer output"),
            newest.clone(),
            older,
        ],
    );
    refresh_jobs(&panel, cx);
    assert_eq!(
        panel.read_with(cx, |p, _| p.client.selected_job().map(str::to_owned)),
        Some("older".into())
    );
    assert_eq!(output(&panel, cx), "older error");
    jobs(&backend, &[newest]);
    refresh_jobs(&panel, cx);
    assert_eq!(output(&panel, cx), "latest output");
    jobs(&backend, &[]);
    refresh_jobs(&panel, cx);
    assert!(panel.read_with(cx, |p, _| p.client.selected_job().is_none()));
    assert!(output(&panel, cx).is_empty());
    assert!(cx.debug_bounds("compose-activity-empty").is_some());
    assert!(cx.debug_bounds("compose-output-viewport").is_none());
    assert!(cx.debug_bounds("compose-output-footer").is_none());
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn history_and_current_command_share_status_icon_geometry(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    jobs(
        &backend,
        &[
            job("running", "stack", JobState::Running, "working\n"),
            job("succeeded", "stack", JobState::Succeeded, "done\n"),
            job("failed", "stack", JobState::Failed, "failed\n"),
            job("cancelled", "stack", JobState::Cancelled, "cancelled\n"),
        ],
    );
    select(&panel, cx, "p:stack");
    refresh_jobs(&panel, cx);
    click(cx, "Activity");
    settle(&panel, cx);
    click(cx, "Earlier commands (3)");
    for (id, status_selector, icon_selector) in [
        (
            "running",
            "compose-history-running-status",
            "compose-history-running-status-icon",
        ),
        (
            "succeeded",
            "compose-history-succeeded-status",
            "compose-history-succeeded-status-icon",
        ),
        (
            "failed",
            "compose-history-failed-status",
            "compose-history-failed-status-icon",
        ),
        (
            "cancelled",
            "compose-history-cancelled-status",
            "compose-history-cancelled-status-icon",
        ),
    ] {
        click_history(cx, id);
        settle(&panel, cx);
        let current = cx.debug_bounds("compose-current-status").unwrap();
        let current_icon = cx.debug_bounds("compose-current-status-icon").unwrap();
        let history = cx.debug_bounds(status_selector).unwrap();
        let history_icon = cx.debug_bounds(icon_selector).unwrap();
        assert_eq!(
            current.size, history.size,
            "{id} status must use the same layout"
        );
        assert_eq!(current_icon.size, gpui::size(px(12.), px(12.)));
        assert_eq!(current_icon.size, history_icon.size);
        assert_eq!(current_icon.origin.x, current.origin.x);
        assert_eq!(history_icon.origin.x, history.origin.x);
        assert!((history_icon.center().y - history.center().y).abs() <= px(1.));
        assert!(history.size.width > history_icon.size.width + px(5.));
    }
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn new_output_pill_stays_centered_and_returns_to_tail(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    let original = "existing output\n".repeat(120);
    jobs(
        &backend,
        &[job("live", "stack", JobState::Running, &original)],
    );
    select(&panel, cx, "p:stack");
    refresh_jobs(&panel, cx);
    click(cx, "Activity");
    settle(&panel, cx);
    panel.update(cx, |p, cx| {
        p.activity.list.scroll_to(gpui::ListOffset::default());
        cx.notify();
    });
    draw(cx);
    backend.set_compose_output(
        "live",
        Ok(job(
            "live",
            "stack",
            JobState::Running,
            &format!("{original}first new line\n"),
        )),
    );
    panel.update(cx, |p, _| p.client.retry_job_output());
    settle(&panel, cx);
    for width in [980., 650.] {
        cx.simulate_resize(gpui::size(px(width), px(1000.)));
        draw(cx);
        let viewport = cx.debug_bounds("compose-output-viewport").unwrap();
        let pill = button(cx, "1 new line").bounds();
        assert!((pill.center().x - viewport.center().x).abs() <= px(1.));
        assert!((viewport.bottom() - pill.bottom() - px(12.)).abs() <= px(2.));
        assert!(pill.origin.x > viewport.origin.x && pill.right() < viewport.right());
    }
    backend.set_compose_output(
        "live",
        Ok(job(
            "live",
            "stack",
            JobState::Running,
            &format!("{original}first new line\nsecond new line\n"),
        )),
    );
    panel.update(cx, |p, _| p.client.retry_job_output());
    settle(&panel, cx);
    click(cx, "2 new lines");
    assert!(panel.read_with(cx, |p, _| p.activity.list.is_following_tail()));
    assert_eq!(panel.read_with(cx, |p, _| p.activity.unseen), 0);
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn empty_activity_becomes_output_when_the_first_command_arrives(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "p:stack");
    click(cx, "Activity");
    settle(&panel, cx);
    assert!(cx.debug_bounds("compose-activity-empty").is_some());
    assert!(cx.debug_bounds("compose-output-viewport").is_none());
    jobs(
        &backend,
        &[job("first", "stack", JobState::Running, "Starting\n")],
    );
    refresh_jobs(&panel, cx);
    assert!(cx.debug_bounds("compose-activity-empty").is_none());
    assert!(cx.debug_bounds("compose-output-viewport").is_some());
    assert_eq!(output(&panel, cx), "Starting");
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn loading_retry_and_late_reply_never_replace_another_selection(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    jobs(
        &backend,
        &[
            job("latest", "stack", JobState::Succeeded, "latest\n"),
            job("older", "stack", JobState::Succeeded, "older\n"),
        ],
    );
    select(&panel, cx, "p:stack");
    refresh_jobs(&panel, cx);
    backend.hold_outputs(true);
    click(cx, "Activity");
    wait_for(&panel, cx, || backend.held_outputs("latest") == 1);
    assert!(panel.read_with(cx, |p, _| p.activity.data.output_pending()));
    assert!(output(&panel, cx).is_empty());
    click(cx, "Earlier commands (1)");
    click_history(cx, "older");
    assert!(output(&panel, cx).is_empty());
    backend.release_output("latest");
    wait_for(&panel, cx, || backend.held_outputs("older") == 1);
    assert!(
        output(&panel, cx).is_empty(),
        "obsolete output cannot flash in another command"
    );
    backend.hold_outputs(false);
    backend.release_output("older");
    settle(&panel, cx);
    assert_eq!(output(&panel, cx), "older");
    backend.set_compose_output("older", Err("fixture output unavailable".into()));
    panel.update(cx, |p, _| p.client.retry_job_output());
    settle(&panel, cx);
    assert_eq!(output(&panel, cx), "older");
    assert!(cx.debug_bounds("compose-activity-output-error").is_some());
    backend.set_compose_output(
        "older",
        Ok(job("older", "stack", JobState::Succeeded, "older\n")),
    );
    click(cx, "Try reading output again");
    settle(&panel, cx);
    assert!(cx.debug_bounds("compose-activity-output-error").is_none());
    backend.set_compose_jobs(Err("fixture history unavailable".into()));
    refresh_jobs(&panel, cx);
    assert_eq!(output(&panel, cx), "older");
    assert!(cx.debug_bounds("compose-activity-jobs-error").is_some());
    jobs(
        &backend,
        &[job("older", "stack", JobState::Succeeded, "older\n")],
    );
    click(cx, "Try reading history again");
    settle(&panel, cx);
    assert!(cx.debug_bounds("compose-activity-jobs-error").is_none());
    select(&panel, cx, "c:cache");
    assert!(output(&panel, cx).is_empty());
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn start_opens_its_activity_once_but_never_steals_navigation(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "p:stack");
    backend.hold_actions();
    click(cx, "Up");
    wait_for(&panel, cx, || backend.held_actions() == 1);
    assert_eq!(backend.count("compose_run"), 1);
    // The agent registers the job before returning its receipt. Any list read
    // dispatched after that receipt must include it; an empty fresh list would
    // correctly invalidate Activity selection, racing the output assertion.
    jobs(
        &backend,
        &[job("started", "stack", JobState::Running, "Starting web\n")],
    );
    backend.release_action(json!({"job":"started"}));
    settle(&panel, cx);
    assert_eq!(panel.read_with(cx, |p, _| p.tab), 1);
    assert_eq!(
        output(&panel, cx),
        "Starting web",
        "activity after start: {:?}",
        panel.read_with(cx, |p, _| (
            p.client.selected_job().map(str::to_owned),
            p.activity.data.output_error().map(str::to_owned),
            p.activity.data.output_pending(),
            p.client.job_output().cloned(),
        )),
    );
    assert!(!panel.read_with(cx, |p, _| p.client.can_compose_run("stack")));
    jobs(
        &backend,
        &[job(
            "started",
            "stack",
            JobState::Succeeded,
            "Starting web\n",
        )],
    );
    refresh_jobs(&panel, cx);
    click(cx, "Logs");
    click(cx, "More actions for stack");
    click(cx, "Pull images only");
    wait_for(&panel, cx, || backend.held_actions() == 1);
    panel.update_in(cx, |p, window, cx| p.select("c:cache".into(), window, cx));
    backend.release_action(json!({"job":"second"}));
    settle(&panel, cx);
    assert_eq!(
        panel.read_with(cx, |p, _| p.selected.clone()),
        Some("c:cache".into())
    );
    assert_eq!(panel.read_with(cx, |p, _| p.tab), 0);
    assert_eq!(backend.count("compose_run"), 2);
    assert_eq!(backend.count("compose_cancel"), 0);
}

fn late_start_respects_manual_tab(cx: &mut TestAppContext, keyboard: bool) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "p:stack");
    backend.hold_actions();
    click(cx, "Up");
    wait_for(&panel, cx, || backend.held_actions() == 1);
    assert!(panel.read_with(cx, |p, _| p.compose_state.show_activity_for.is_some()));
    if keyboard {
        panel.update_in(cx, |p, window, cx| p.tab_focus[0].focus(window, cx));
        cx.simulate_keystrokes("end");
        draw(cx);
    } else {
        click(cx, "Compose files");
    }
    assert_eq!(panel.read_with(cx, |p, _| p.tab), 2);
    assert!(panel.read_with(cx, |p, _| p.compose_state.show_activity_for.is_none()));
    backend.release_action(json!({"job":"late-start"}));
    settle(&panel, cx);
    assert_eq!(panel.read_with(cx, |p, _| p.tab), 2);
    assert_eq!(
        panel.read_with(cx, |p, _| p.selected.clone()),
        Some("p:stack".into())
    );
    assert_eq!(backend.count("compose_run"), 1);
    assert_eq!(backend.count("compose_cancel"), 0);
}

#[gpui::test]
fn late_start_preserves_pointer_tab_navigation(cx: &mut TestAppContext) {
    late_start_respects_manual_tab(cx, false);
}

#[gpui::test]
fn late_start_preserves_keyboard_tab_navigation(cx: &mut TestAppContext) {
    late_start_respects_manual_tab(cx, true);
}

#[gpui::test]
fn late_start_does_not_reopen_activity_after_the_page_was_hidden(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "p:stack");
    backend.hold_actions();
    click(cx, "Up");
    wait_for(&panel, cx, || backend.held_actions() == 1);
    assert!(panel.read_with(cx, |p, _| p.compose_state.show_activity_for.is_some()));
    panel.update(cx, |p, cx| p.poll(false, cx));
    assert!(panel.read_with(cx, |p, _| p.compose_state.show_activity_for.is_none()));
    backend.release_action(json!({"job":"hidden-start"}));
    for _ in 0..300 {
        panel.update(cx, |p, cx| p.poll(false, cx));
        if !panel.read_with(cx, |p, _| p.client.compose_run_pending()) {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert!(!panel.read_with(cx, |p, _| p.client.compose_run_pending()));
    settle(&panel, cx);
    assert_eq!(
        panel.read_with(cx, |p, _| p.tab),
        crate::containers::PROJECT_SERVICES
    );
    assert_eq!(
        panel.read_with(cx, |p, _| p.selected.clone()),
        Some("p:stack".into())
    );
    assert_eq!(backend.count("compose_run"), 1);
    assert_eq!(backend.count("compose_cancel"), 0);
}

#[gpui::test]
fn cancel_keeps_exact_job_and_stays_pending_until_terminal_output(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    jobs(
        &backend,
        &[job("running", "stack", JobState::Running, "Building\n")],
    );
    select(&panel, cx, "p:stack");
    refresh_jobs(&panel, cx);
    click(cx, "Activity");
    settle(&panel, cx);
    backend.hold_actions();
    click(cx, "Cancel");
    wait_for(&panel, cx, || backend.held_actions() == 1);
    assert_eq!(backend.requests("compose_cancel")[0]["job"], "running");
    click(cx, "Cancelling…");
    assert_eq!(backend.count("compose_cancel"), 1);
    backend.release_action(json!({"ok":true}));
    settle(&panel, cx);
    click(cx, "Cancelling…");
    assert_eq!(backend.count("compose_cancel"), 1);
    backend.set_compose_output(
        "running",
        Ok(job(
            "running",
            "stack",
            JobState::Cancelled,
            "Building\nStopped\n",
        )),
    );
    panel.update(cx, |p, _| p.client.retry_job_output());
    settle(&panel, cx);
    assert_eq!(output(&panel, cx), "Building\nStopped");
    assert_eq!(
        panel.read_with(cx, |p, _| p.activity.data.current().unwrap().state),
        JobState::Cancelled
    );
    assert!(!panel.read_with(cx, |p, _| p.client.compose_cancelling("running")));
    assert_eq!(panel.read_with(cx, |p, _| p.activity.rows.len()), 2);
    assert_eq!(backend.count("compose_run"), 0);
}

#[gpui::test]
fn view_output_selects_running_command_instead_of_old_history(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    jobs(
        &backend,
        &[
            job("running", "stack", JobState::Running, "building"),
            job("older", "stack", JobState::Succeeded, "old"),
        ],
    );
    select(&panel, cx, "p:stack");
    refresh_jobs(&panel, cx);
    click(cx, "Activity");
    settle(&panel, cx);
    click(cx, "Earlier commands (1)");
    click_history(cx, "older");
    settle(&panel, cx);
    click(cx, "Logs");
    click(cx, "View output");
    settle(&panel, cx);
    assert_eq!(panel.read_with(cx, |p, _| p.tab), 1);
    assert_eq!(output(&panel, cx), "building");
    select(&panel, cx, "c:stack-web-1");
    panel.update(cx, |p, cx| {
        assert!(!p.show_compose_output("stack", "running".into(), cx))
    });
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn another_pending_cancel_blocks_activation_and_failure_allows_exact_retry(
    cx: &mut TestAppContext,
) {
    let (panel, cx, backend) = fixture(cx);
    let mut data = panel.read_with(cx, |p, _| p.client.snapshot().unwrap().clone());
    data.containers
        .iter_mut()
        .find(|c| c.name == "cache")
        .unwrap()
        .compose = Some(newport_domain::containers::ComposeLabels {
        project: "other".into(),
        service: "cache".into(),
        ..Default::default()
    });
    backend.set_snapshot(data);
    jobs(
        &backend,
        &[
            job("a", "stack", JobState::Running, "A"),
            job("b", "other", JobState::Running, "B"),
        ],
    );
    panel.update(cx, |p, cx| p.refresh(cx));
    settle(&panel, cx);
    select(&panel, cx, "p:stack");
    refresh_jobs(&panel, cx);
    click(cx, "Activity");
    settle(&panel, cx);
    backend.hold_actions();
    click(cx, "Cancel");
    wait_for(&panel, cx, || backend.held_actions() == 1);
    panel.update_in(cx, |p, window, cx| p.select("p:other".into(), window, cx));
    draw(cx);
    click(cx, "Activity");
    for _ in 0..300 {
        panel.update(cx, |p, cx| p.poll(true, cx));
        draw(cx);
        if output(&panel, cx) == "B" {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(output(&panel, cx), "B");
    click(cx, "Cancel");
    assert_eq!(backend.requests("compose_cancel").len(), 1);
    backend.release_action(json!({"ok":true}));
    settle(&panel, cx);
    click(cx, "Cancel");
    wait_for(&panel, cx, || backend.held_actions() == 1);
    assert_eq!(backend.requests("compose_cancel")[1]["job"], "b");
    backend.fail_action("fixture cancellation failed");
    settle(&panel, cx);
    assert!(cx.debug_bounds("containers-error-compose-cancel").is_some());
    assert!(!panel.read_with(cx, |p, _| p.client.compose_cancelling("b")));
    click(cx, "Cancel");
    wait_for(&panel, cx, || backend.held_actions() == 1);
    assert_eq!(backend.requests("compose_cancel")[2]["job"], "b");
    backend.release_action(json!({"ok":true}));
    settle(&panel, cx);
    assert_eq!(output(&panel, cx), "B");
    assert_eq!(backend.count("compose_run"), 0);
}

#[gpui::test]
fn activity_preserves_literal_unicode_wrap_selection_and_copy(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    let first = format!("Café **literal** <tag> 🦀 {} tail", "longword".repeat(20));
    let second = "服务 second e\u{301} tail";
    let text = format!("{first}\n{second}\n");
    jobs(
        &backend,
        &[job("unicode", "stack", JobState::Succeeded, &text)],
    );
    select(&panel, cx, "p:stack");
    refresh_jobs(&panel, cx);
    click(cx, "Activity");
    settle(&panel, cx);
    let end = second.find(" tail").unwrap();
    let (from, to, last) = panel.read_with(cx, |p, _| {
        (
            p.activity.selection.text_position(0, 0).unwrap() + point(px(0.1), px(5.)),
            p.activity.selection.text_position(1, end).unwrap() + point(px(0.1), px(5.)),
            p.activity.selection.text_position(0, first.len()).unwrap(),
        )
    });
    assert!(last.y > from.y + px(20.));
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    draw(cx);
    let expected = format!("{first}\n{}", &second[..end]);
    assert_eq!(
        cx.update(gpui_kit::base::TextSelection::selected_text),
        expected
    );
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-c"
    } else {
        "ctrl-c"
    });
    assert_eq!(
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()).unwrap()),
        expected
    );
    click(cx, "Copy command output");
    assert_eq!(
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()).unwrap()),
        text
    );
    select(&panel, cx, "c:cache");
    assert!(
        cx.update(gpui_kit::base::TextSelection::selected_text)
            .is_empty()
    );
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn bounded_output_keeps_byte_anchor_new_line_count_and_keyboard_scrolling(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    let line = format!("Café {}\n", "repeated-heartbeat ".repeat(10));
    let original = line.repeat(6_000);
    jobs(
        &backend,
        &[job("large", "stack", JobState::Running, &original)],
    );
    select(&panel, cx, "p:stack");
    refresh_jobs(&panel, cx);
    click(cx, "Activity");
    settle(&panel, cx);
    assert!(
        panel.read_with(cx, |p, _| p.activity.data.output().unwrap().output.len()) <= 1_048_576
    );
    panel.update(cx, |p, cx| {
        p.activity.list.scroll_to(gpui::ListOffset {
            item_ix: 2_000,
            offset_in_item: px(3.),
        });
        cx.notify();
    });
    draw(cx);
    let (before, byte) = panel.read_with(cx, |p, _| {
        let at = p.activity.list.logical_scroll_top();
        (at, p.activity.rows[at.item_ix].offset)
    });
    assert!(!panel.read_with(cx, |p, _| p.activity.list.is_following_tail()));
    backend.set_compose_output(
        "large",
        Ok(job(
            "large",
            "stack",
            JobState::Running,
            &format!("{original}{}", line.repeat(1_000)),
        )),
    );
    panel.update(cx, |p, _| p.client.retry_job_output());
    settle(&panel, cx);
    panel.read_with(cx, |p, _| {
        let after = p.activity.list.logical_scroll_top();
        assert_eq!(
            p.activity.rows[after.item_ix].offset, byte,
            "prefix eviction must preserve the reader's logical line"
        );
        assert_eq!(after.offset_in_item, before.offset_in_item);
        assert_eq!(p.activity.unseen, 1_000);
        assert!(
            p.activity.selection.text_position(0, 0).is_none(),
            "off-screen lines stay virtualized"
        );
    });
    panel.update(cx, |p, _| p.client.retry_job_output());
    settle(&panel, cx);
    assert_eq!(panel.read_with(cx, |p, _| p.activity.unseen), 1_000);
    click(cx, "1000 new lines");
    assert!(panel.read_with(cx, |p, _| p.activity.list.is_following_tail()));
    panel.update_in(cx, |p, window, cx| p.activity.focus.focus(window, cx));
    let outer = panel.read_with(cx, |p, _| p.detail_scroll.offset());
    cx.simulate_keystrokes("home");
    draw(cx);
    assert_eq!(
        panel.read_with(cx, |p, _| p.activity.list.logical_scroll_top().item_ix),
        0
    );
    cx.simulate_keystrokes("pagedown");
    draw(cx);
    assert!(panel.read_with(cx, |p, _| p.activity.list.logical_scroll_top().item_ix) > 0);
    assert_eq!(panel.read_with(cx, |p, _| p.detail_scroll.offset()), outer);
    cx.simulate_keystrokes("end");
    draw(cx);
    assert!(panel.read_with(cx, |p, _| p.activity.list.is_following_tail()));
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn list_completion_finishes_status_when_output_fails_and_notification_opens_exact_job(
    cx: &mut TestAppContext,
) {
    use std::{cell::RefCell, rc::Rc};
    let (panel, cx, backend) = fixture(cx);
    let receipts = Rc::new(RefCell::new(Vec::new()));
    let captured = receipts.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(
            &panel,
            move |_, event: &crate::containers::ContainersEvent, _| {
                if let crate::containers::ContainersEvent::ComposeCompleted(job) = event {
                    captured.borrow_mut().push(job.clone());
                }
            },
        )
    });
    jobs(
        &backend,
        &[
            job("running", "stack", JobState::Running, "building\n"),
            job("history", "stack", JobState::Succeeded, "old\n"),
        ],
    );
    select(&panel, cx, "p:stack");
    refresh_jobs(&panel, cx);
    click(cx, "Activity");
    settle(&panel, cx);
    assert!(
        receipts.borrow().is_empty(),
        "historical completed jobs must not toast"
    );
    let finished = job("running", "stack", JobState::Failed, "building\n");
    jobs(&backend, &[finished]);
    backend.set_compose_output("running", Err("fixture disconnected".into()));
    refresh_jobs(&panel, cx);
    panel.update(cx, |p, _| p.client.retry_job_output());
    settle(&panel, cx);
    assert_eq!(
        panel.read_with(cx, |p, _| p.activity.data.current().unwrap().state),
        JobState::Failed
    );
    assert_eq!(output(&panel, cx), "building");
    assert_eq!(receipts.borrow().len(), 1);
    assert_eq!(receipts.borrow()[0].job, "running");
    refresh_jobs(&panel, cx);
    assert_eq!(receipts.borrow().len(), 1);
    select(&panel, cx, "c:cache");
    panel.update_in(cx, |p, window, cx| {
        assert!(p.open_compose_activity("stack", "running", window, cx))
    });
    settle(&panel, cx);
    assert_eq!(
        panel.read_with(cx, |p, _| p.selected.as_deref().map(str::to_owned)),
        Some("p:stack".into())
    );
    assert_eq!(
        panel.read_with(cx, |p, _| p.client.selected_job().map(str::to_owned)),
        Some("running".into())
    );
    assert_eq!(panel.read_with(cx, |p, _| p.tab), 1);
    jobs(&backend, &[]);
    refresh_jobs(&panel, cx);
    select(&panel, cx, "c:cache");
    panel.update_in(cx, |p, window, cx| {
        assert!(!p.open_compose_activity("stack", "running", window, cx))
    });
    assert_eq!(
        panel.read_with(cx, |p, _| p.selected.as_deref().map(str::to_owned)),
        Some("c:cache".into())
    );
    assert_eq!(backend.writes(), 0);
}
