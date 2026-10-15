use super::{ContainerLogEntry, ContainersPanel, LogWindow, Row, log_line};
use crate::containers::tests::{click, draw, element, fixture, select, settle};
use gpui::{
    Entity, Focusable, ListOffset, Modifiers, MouseButton, TestAppContext, VisualTestContext,
    point, px,
};
use gpui_kit as gpui;
use newport_domain::containers::{ContainerLogPage, LogStream};

fn entry(message: &str) -> ContainerLogEntry {
    ContainerLogEntry {
        at: Some(1_700_000_000_000),
        message: message.into(),
        ..Default::default()
    }
}
fn inject(
    panel: &Entity<ContainersPanel>,
    cx: &mut VisualTestContext,
    entries: &[ContainerLogEntry],
    epoch: u64,
    first: u64,
    end: u64,
) {
    panel.update(cx, |p, cx| {
        p.logs.sync(
            Some(LogWindow {
                entries,
                epoch,
                first,
                end,
            }),
            cx,
        );
        cx.notify();
    });
    draw(cx);
}

#[gpui::test]
fn hidden_container_logs_defer_projection_until_return(cx: &mut TestAppContext) {
    use std::{rc::Rc, thread, time::Duration};
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "p:stack");
    click(cx, "Logs");
    settle(&panel, cx);
    let previous = panel.read_with(cx, |p, _| p.logs.rows.clone());
    backend.set_logs(Ok(ContainerLogPage {
        entries: vec![entry("arrived while hidden")],
        cursor: Some("hidden-tail".into()),
    }));
    panel.update(cx, |p, _| p.client.reset_logs());
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
        assert_eq!(
            p.client.log_window().unwrap().entries[0].message,
            "arrived while hidden"
        );
        assert!(
            Rc::ptr_eq(&previous, &p.logs.rows),
            "hidden output retains its old projection"
        );
    });
    let calls = backend.count("container_logs");
    panel.update(cx, |p, cx| p.poll(true, cx));
    draw(cx);
    panel.read_with(cx, |p, _| {
        assert!(!Rc::ptr_eq(&previous, &p.logs.rows));
        assert_eq!(p.logs.source[0].message, "arrived while hidden");
    });
    assert_eq!(
        backend.count("container_logs"),
        calls,
        "return reuses cached output"
    );
}

#[gpui::test]
fn short_logs_start_at_top_while_following_new_output(cx: &mut TestAppContext) {
    let (panel, cx, _backend) = fixture(cx);
    for target in ["p:stack", "c:cache"] {
        select(&panel, cx, target);
        click(cx, "Logs");
        for count in [3, 200, 3] {
            let entries = vec![entry("log output"); count];
            inject(&panel, cx, &entries, 950, 0, count as u64);
            for height in [1000., 1200.] {
                cx.simulate_resize(gpui::size(px(980.), px(height)));
                draw(cx);
                let viewport = cx.debug_bounds("container-log-viewport").unwrap();
                assert!(panel.read_with(cx, |p, _| p.logs.list.is_following_tail()));
                if count == 3 {
                    let first =
                        panel.read_with(cx, |p, _| p.logs.selection.text_position(0, 0).unwrap());
                    assert!((first.y - viewport.top() - px(9.)).abs() < px(3.));
                } else {
                    assert!(panel.read_with(cx, |p, _| {
                        p.logs
                            .selection
                            .text_position(count - 1, 0)
                            .is_some_and(|last| {
                                last.y >= viewport.top() && last.y < viewport.bottom()
                            })
                    }));
                }
            }
        }
    }
}

#[gpui::test]
fn filters_search_copy_and_failed_refresh_use_the_same_retained_lines(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "p:stack");
    let mut normal = entry("connected successfully");
    normal.stream = LogStream::Stderr;
    normal.container = Some("api".into());
    let mut error = entry("ERROR Café request failed");
    error.container = Some("worker".into());
    backend.set_logs(Ok(ContainerLogPage {
        entries: vec![normal, entry("WARN retrying"), error],
        cursor: Some("next".into()),
    }));
    panel.update(cx, |p, cx| {
        p.client.reset_logs();
        p.sync_selection(cx);
    });
    settle(&panel, cx);
    assert_eq!(panel.read_with(cx, |p, _| p.logs.rows.len()), 3);
    assert!(panel.read_with(cx, |p, _| matches!(
        &p.logs.rows[0],
        Row::Line {
            tone: log_line::Tone::Normal,
            ..
        }
    )));
    let reads = backend.count("container_logs");
    click(cx, "Warnings");
    assert_eq!(panel.read_with(cx, |p, _| p.logs.rows.len()), 2);
    click(cx, "Errors");
    assert_eq!(panel.read_with(cx, |p, _| p.logs.rows.len()), 1);
    click(cx, "All");
    let find = panel.read_with(cx, |p, _| p.logs.find.clone());
    cx.update(|window, cx| find.read(cx).focus_handle(cx).focus(window, cx));
    cx.simulate_input("WORKER");
    draw(cx);
    assert_eq!(panel.read_with(cx, |p, _| p.logs.rows.len()), 1);
    assert_eq!(
        backend.count("container_logs"),
        reads,
        "local filters must not refetch logs"
    );
    click(cx, "Copy visible logs");
    assert!(
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()).unwrap())
            .ends_with("worker: ERROR Café request failed")
    );
    backend.set_logs(Err("fixture connection lost".into()));
    click(cx, "Refresh container logs");
    settle(&panel, cx);
    assert_eq!(
        panel.read_with(cx, |p, _| p.logs.rows.len()),
        1,
        "failed polls retain the last successful output"
    );
    assert!(cx.debug_bounds("containers-error-container-logs").is_some());
    assert_eq!(
        panel.read_with(cx, |p, cx| p.preferences(cx).log_query),
        "WORKER"
    );
    assert_eq!(backend.count("compose_run"), 0);
}

#[gpui::test]
fn identical_lines_keep_anchor_and_exact_unseen_count_through_bounded_eviction(
    cx: &mut TestAppContext,
) {
    let (panel, cx, _backend) = fixture(cx);
    select(&panel, cx, "p:stack");
    let entries = vec![entry("repeated heartbeat"); 2_000];
    inject(&panel, cx, &entries, 700, 0, 2_000);
    panel.update(cx, |p, cx| {
        p.logs.list.scroll_to(ListOffset {
            item_ix: 1_200,
            offset_in_item: px(3.),
        });
        cx.notify();
    });
    draw(cx);
    let before = panel.read_with(cx, |p, _| p.logs.list.logical_scroll_top());
    assert!(!panel.read_with(cx, |p, _| p.logs.list.is_following_tail()));
    inject(&panel, cx, &entries, 700, 400, 2_400);
    let after = panel.read_with(cx, |p, _| p.logs.list.logical_scroll_top());
    assert_eq!(after.item_ix + 400, before.item_ix);
    assert_eq!(after.offset_in_item, before.offset_in_item);
    assert_eq!(panel.read_with(cx, |p, _| p.logs.unseen), 400);
    inject(&panel, cx, &entries, 700, 400, 2_400);
    assert_eq!(
        panel.read_with(cx, |p, _| p.logs.unseen),
        400,
        "empty poll must not increase arrivals"
    );
    assert!(
        panel.read_with(cx, |p, _| p.logs.selection.text_position(0, 0).is_none()),
        "offscreen lines must stay virtualized"
    );
    click(cx, "400 new lines");
    assert!(panel.read_with(cx, |p, _| p.logs.list.is_following_tail()));
    assert_eq!(panel.read_with(cx, |p, _| p.logs.unseen), 0);
    inject(&panel, cx, &entries, 700, 500, 2_500);
    assert!(panel.read_with(cx, |p, _| p.logs.list.is_following_tail()));
    // New scope with identical text must start at its own tail with no arrivals.
    inject(&panel, cx, &entries, 701, 0, 2_000);
    assert_eq!(panel.read_with(cx, |p, _| p.logs.unseen), 0);
    assert_eq!(panel.read_with(cx, |p, _| p.logs.epoch), Some(701));
}

#[gpui::test]
fn new_line_pill_is_centered_for_container_and_project_logs(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    for target in ["p:stack", "c:cache"] {
        select(&panel, cx, target);
        click(cx, "Logs");
        let mut entries = vec![entry("existing output"); 120];
        inject(&panel, cx, &entries, 850, 0, 120);
        panel.update(cx, |p, cx| {
            p.logs.list.scroll_to(ListOffset::default());
            cx.notify();
        });
        draw(cx);
        entries.push(entry("first new line"));
        inject(&panel, cx, &entries, 850, 0, 121);
        for width in [980., 650.] {
            cx.simulate_resize(gpui::size(px(width), px(1000.)));
            draw(cx);
            let viewport = cx.debug_bounds("container-log-viewport").unwrap();
            let pill = element(cx, "1 new line").bounds();
            assert!(
                (pill.center().x - viewport.center().x).abs() <= px(1.),
                "{target}"
            );
            assert!(
                (viewport.bottom() - pill.bottom() - px(12.)).abs() <= px(2.),
                "{target}"
            );
            assert!(pill.origin.x > viewport.origin.x && pill.right() < viewport.right());
        }
        entries.push(entry("second new line"));
        inject(&panel, cx, &entries, 850, 0, 122);
        click(cx, "2 new lines");
        assert!(panel.read_with(cx, |p, _| p.logs.list.is_following_tail()));
        assert_eq!(panel.read_with(cx, |p, _| p.logs.unseen), 0);
    }
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn wrapped_literal_text_supports_drag_copy_and_clears_selection_on_scope_change(
    cx: &mut TestAppContext,
) {
    let (panel, cx, _backend) = fixture(cx);
    select(&panel, cx, "p:stack");
    let mut first = entry(&format!(
        "Café **literal** <tag> 🦀 {} tail",
        "longword".repeat(20)
    ));
    first.container = Some("api".into());
    let mut second = entry("Café second e\u{301} tail");
    second.container = Some("服务".into());
    let entries = vec![first, second];
    inject(&panel, cx, &entries, 900, 0, 2);
    let literal = panel.read_with(cx, |p, _| {
        p.logs.rows.iter().filter_map(Row::text).collect::<Vec<_>>()
    });
    let start = literal[0].find("Café").unwrap();
    let end = literal[1].find(" tail").unwrap();
    let (from, to, wrapped_end) = panel.read_with(cx, |p, _| {
        (
            p.logs.selection.text_position(0, start).unwrap() + point(px(0.1), px(5.)),
            p.logs.selection.text_position(1, end).unwrap() + point(px(0.1), px(5.)),
            p.logs.selection.text_position(0, literal[0].len()).unwrap(),
        )
    });
    assert!(
        wrapped_end.y > from.y + px(20.),
        "long unbroken messages must wrap inside the pane"
    );
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    draw(cx);
    let expected = format!("{}\n{}", &literal[0][start..], &literal[1][..end]);
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
    inject(&panel, cx, &entries, 901, 0, 2);
    assert!(
        cx.update(gpui_kit::base::TextSelection::selected_text)
            .is_empty()
    );
}

#[gpui::test]
fn keyboard_scroll_is_contained_and_dataset_loading_clears_old_output(cx: &mut TestAppContext) {
    let (panel, cx, _backend) = fixture(cx);
    select(&panel, cx, "p:stack");
    let entries = (0..300)
        .map(|i| entry(&format!("row {i}")))
        .collect::<Vec<_>>();
    inject(&panel, cx, &entries, 800, 0, 300);
    panel.update_in(cx, |p, window, cx| p.logs.focus.focus(window, cx));
    let outer = panel.read_with(cx, |p, _| p.detail_scroll.offset());
    cx.simulate_keystrokes("home");
    draw(cx);
    assert!(!panel.read_with(cx, |p, _| p.logs.list.is_following_tail()));
    assert_eq!(
        panel.read_with(cx, |p, _| p.logs.list.logical_scroll_top().item_ix),
        0
    );
    cx.simulate_keystrokes("pagedown");
    draw(cx);
    assert!(panel.read_with(cx, |p, _| p.logs.list.logical_scroll_top().item_ix) > 0);
    assert_eq!(panel.read_with(cx, |p, _| p.detail_scroll.offset()), outer);
    cx.simulate_keystrokes("end");
    draw(cx);
    assert!(panel.read_with(cx, |p, _| p.logs.list.is_following_tail()));
    inject(&panel, cx, &[], 801, 0, 0);
    assert_eq!(panel.read_with(cx, |p, _| p.logs.rows.len()), 0);
    assert_eq!(panel.read_with(cx, |p, _| p.logs.source.len()), 0);
    assert_eq!(panel.read_with(cx, |p, _| p.logs.list.item_count()), 0);
}
