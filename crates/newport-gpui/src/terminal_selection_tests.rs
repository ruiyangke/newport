use super::TerminalView;
use gpui_kit as gpui;
use gpui_kit::{
    AppContext, Entity, Modifiers, MouseButton, MouseDownEvent, Pixels, Point, TestAppContext,
    VisualTestContext, point, px, size,
};
use std::time::{Duration, Instant};

pub(super) fn fixture<'a>(
    cx: &'a mut TestAppContext,
    text: &str,
) -> (Entity<TerminalView>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        gpui::component::init(cx);
        crate::theme::install(false, cx);
    });
    let mut view = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let terminal = cx.new(|cx| {
            let mut terminal = TerminalView::new("selection-fixture-no-transport".into(), cx);
            terminal.close();
            terminal
        });
        terminal.read(cx).focus.clone().focus(window, cx);
        view = Some(terminal.clone());
        gpui::base::Root::new(terminal, window, cx)
    });
    cx.simulate_resize(size(px(600.), px(300.)));
    draw(cx);
    let view = view.unwrap();
    view.update(cx, |view, cx| {
        view.engine.advance(text.as_bytes()).unwrap();
        cx.notify();
    });
    draw(cx);
    (view, cx)
}

fn draw(cx: &mut VisualTestContext) {
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
}

fn at(view: &Entity<TerminalView>, column: f32, row: f32, cx: &VisualTestContext) -> Point<Pixels> {
    view.read_with(cx, |view, _| {
        view.bounds.origin + point(view.cell.width * column, view.cell.height * row)
    })
}

fn selected(view: &Entity<TerminalView>, cx: &VisualTestContext) -> String {
    view.read_with(cx, |view, _| {
        assert!(view.session.is_none(), "UI tests never open a remote PTY");
        assert!(
            view.input.is_empty(),
            "local gestures never queue remote input"
        );
        view.engine.selected_text().unwrap().unwrap_or_default()
    })
}

fn click(position: Point<Pixels>, count: usize, modifiers: Modifiers, cx: &mut VisualTestContext) {
    cx.simulate_event(MouseDownEvent {
        position,
        modifiers,
        button: MouseButton::Left,
        click_count: count,
        first_mouse: false,
    });
    cx.simulate_mouse_up(position, MouseButton::Left, modifiers);
    draw(cx);
}

fn drag(from: Point<Pixels>, to: Point<Pixels>, modifiers: Modifiers, cx: &mut VisualTestContext) {
    cx.simulate_mouse_down(from, MouseButton::Left, modifiers);
    cx.simulate_mouse_move(to, Some(MouseButton::Left), modifiers);
    cx.simulate_mouse_up(to, MouseButton::Left, modifiers);
    draw(cx);
}

#[gpui::test]
fn terminal_link_hover_tracks_modifiers_without_mouse_movement(cx: &mut TestAppContext) {
    let (view, cx) = fixture(
        cx,
        "\x1b[?1049h\x1b[?1000h\x1b[34;1mhttp://localhost:5178/\x1b[0m plain",
    );
    let position = at(&view, 3.2, 0.3, cx);
    cx.simulate_mouse_move(position, None, Modifiers::default());
    draw(cx);
    assert!(view.read_with(cx, |view, _| view.hovered_link.is_none()));

    for modifiers in [
        Modifiers {
            control: true,
            ..Default::default()
        },
        Modifiers {
            platform: true,
            ..Default::default()
        },
    ] {
        cx.simulate_event(gpui::ModifiersChangedEvent {
            modifiers,
            ..Default::default()
        });
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, _| view.hovered_link.clone())
                .as_deref(),
            Some("http://localhost:5178/"),
            "either modifier enables the pointer and underline without moving the mouse"
        );
        cx.simulate_event(gpui::ModifiersChangedEvent::default());
        draw(cx);
        assert!(view.read_with(cx, |view, _| view.hovered_link.is_none()));
    }

    let control = Modifiers {
        control: true,
        ..Default::default()
    };
    cx.simulate_mouse_move(position, None, control);
    draw(cx);
    assert!(view.read_with(cx, |view, _| view.hovered_link.is_some()));
    cx.simulate_mouse_move(at(&view, 25.2, 0.3, cx), None, control);
    draw(cx);
    assert!(view.read_with(cx, |view, _| view.hovered_link.is_none()));
}

#[gpui::test]
#[cfg(target_os = "macos")]
fn terminal_links_handle_macos_control_click_translation(cx: &mut TestAppContext) {
    use std::{cell::RefCell, rc::Rc};
    let (view, cx) = fixture(cx, "\x1b[?1000hhttp://localhost:5178/ plain");
    let events = Rc::new(RefCell::new(Vec::new()));
    let capture = events.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &super::OpenLink, _| {
            capture.borrow_mut().push(event.0.clone());
        })
    });
    let position = at(&view, 3.2, 0.3, cx);
    let control = Modifiers {
        control: true,
        ..Default::default()
    };
    let plain = Modifiers::default();

    // Match gpui_macos::window::handle_input: both button events are Right
    // with Control removed, although flags-changed still reports Control.
    cx.simulate_event(gpui::ModifiersChangedEvent {
        modifiers: control,
        ..Default::default()
    });
    cx.simulate_mouse_down(position, MouseButton::Right, plain);
    draw(cx);
    assert!(view.read_with(cx, |view, _| view.hovered_link.is_some()));
    cx.simulate_mouse_up(position, MouseButton::Right, plain);
    draw(cx);
    assert_eq!(&*events.borrow(), &["http://localhost:5178/"]);

    // A plain secondary click must not become a link activation.
    cx.simulate_event(gpui::ModifiersChangedEvent::default());
    cx.simulate_mouse_down(position, MouseButton::Right, plain);
    cx.simulate_mouse_up(position, MouseButton::Right, plain);
    draw(cx);
    assert_eq!(events.borrow().len(), 1);

    // Ctrl released before the mouse: native GPUI now sends Left up. Consume
    // it without opening the URL or retaining a stale pending link gesture.
    cx.simulate_event(gpui::ModifiersChangedEvent {
        modifiers: control,
        ..Default::default()
    });
    cx.simulate_mouse_down(position, MouseButton::Right, plain);
    cx.simulate_event(gpui::ModifiersChangedEvent::default());
    cx.simulate_mouse_up(position, MouseButton::Left, plain);
    draw(cx);
    assert!(view.read_with(cx, |view, _| view.link_press.is_none()));
    assert_eq!(events.borrow().len(), 1);

    // Drag events retain the physical Left button and Control flag.
    cx.simulate_mouse_move(position, None, control);
    cx.simulate_mouse_down(position, MouseButton::Right, plain);
    cx.simulate_mouse_move(at(&view, 9.2, 0.3, cx), Some(MouseButton::Left), control);
    cx.simulate_mouse_up(at(&view, 9.2, 0.3, cx), MouseButton::Right, plain);
    draw(cx);
    assert_eq!(events.borrow().len(), 1);
    assert!(view.read_with(cx, |view, _| view.link_press.is_none()));
}

#[gpui::test]
fn plain_terminal_urls_open_with_either_modifier_and_cancel_on_drag(cx: &mut TestAppContext) {
    use std::{cell::RefCell, rc::Rc};
    let (view, cx) = fixture(
        cx,
        "\x1b[?1049h\x1b[?1000h\x1b[34;1mhttp://localhost:5178/\x1b[0m",
    );
    let events = Rc::new(RefCell::new(Vec::new()));
    let capture = events.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &super::OpenLink, _| {
            capture.borrow_mut().push(event.0.clone());
        })
    });
    let position = at(&view, 3.2, 0.3, cx);
    click(position, 1, Modifiers::default(), cx);
    assert!(events.borrow().is_empty());
    for modifiers in [
        Modifiers {
            control: true,
            ..Default::default()
        },
        Modifiers {
            platform: true,
            ..Default::default()
        },
    ] {
        click(position, 1, modifiers, cx);
        drag(position, at(&view, 9.2, 0.3, cx), modifiers, cx);
    }
    assert_eq!(
        &*events.borrow(),
        &["http://localhost:5178/", "http://localhost:5178/"]
    );
    cx.simulate_mouse_down(
        position,
        MouseButton::Left,
        Modifiers {
            control: true,
            ..Default::default()
        },
    );
    view.update(cx, |view, cx| {
        view.engine
            .advance(b"\r\x1b[Khttp://localhost:5191/")
            .unwrap();
        cx.notify();
    });
    draw(cx);
    cx.simulate_mouse_up(
        position,
        MouseButton::Left,
        Modifiers {
            control: true,
            ..Default::default()
        },
    );
    draw(cx);
    assert_eq!(
        events.borrow().len(),
        2,
        "rewritten output must not retarget a press"
    );
}

#[gpui::test]
fn terminal_links_require_a_modified_click_and_preserve_selection(cx: &mut TestAppContext) {
    use std::{cell::RefCell, rc::Rc};
    let (view, cx) = fixture(
        cx,
        "\x1b]8;id=docs;https://example.org/docs\x07Documentation\x1b]8;;\x07 plain",
    );
    let events = Rc::new(RefCell::new(Vec::new()));
    let capture = events.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &super::OpenLink, _| {
            capture.borrow_mut().push(event.0.clone());
        })
    });
    let position = at(&view, 3.2, 0.3, cx);
    click(position, 2, Modifiers::default(), cx);
    assert_eq!(selected(&view, cx), "Documentation");
    assert!(events.borrow().is_empty());
    click(
        position,
        1,
        Modifiers {
            platform: true,
            ..Default::default()
        },
        cx,
    );
    assert_eq!(&*events.borrow(), &["https://example.org/docs"]);
    assert_eq!(selected(&view, cx), "Documentation");
    click(
        position,
        1,
        Modifiers {
            control: true,
            ..Default::default()
        },
        cx,
    );
    assert_eq!(events.borrow().len(), 2);
    drag(
        position,
        at(&view, 9.2, 0.3, cx),
        Modifiers {
            platform: true,
            ..Default::default()
        },
        cx,
    );
    assert_eq!(events.borrow().len(), 2, "dragging a link must not open it");
    click(
        at(&view, 16.2, 0.3, cx),
        1,
        Modifiers {
            platform: true,
            ..Default::default()
        },
        cx,
    );
    assert_eq!(
        events.borrow().len(),
        2,
        "unlinked text is not inferred as a URL"
    );
}

#[gpui::test]
fn terminal_link_replacement_and_remote_mouse_mode_do_not_retarget_a_press(
    cx: &mut TestAppContext,
) {
    use std::{cell::RefCell, rc::Rc};
    let (view, cx) = fixture(
        cx,
        "\x1b[?1000h\x1b[?1006h\x1b]8;id=shared;https://example.org/first\x07Link\x1b]8;;\x07",
    );
    let events = Rc::new(RefCell::new(Vec::new()));
    let capture = events.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &super::OpenLink, _| {
            capture.borrow_mut().push(event.0.clone());
        })
    });
    let position = at(&view, 1.2, 0.3, cx);
    let modifiers = Modifiers {
        platform: true,
        ..Default::default()
    };
    click(position, 1, modifiers, cx);
    assert_eq!(&*events.borrow(), &["https://example.org/first"]);
    cx.simulate_mouse_down(position, MouseButton::Left, modifiers);
    view.update(cx, |view, cx| {
        view.engine
            .advance(b"\r\x1b]8;id=shared;https://example.org/replaced\x07Link\x1b]8;;\x07")
            .unwrap();
        cx.notify();
    });
    draw(cx);
    cx.simulate_mouse_up(position, MouseButton::Left, modifiers);
    draw(cx);
    assert_eq!(
        events.borrow().len(),
        1,
        "the same OSC ID with a new URL must not retarget the gesture"
    );
    assert_eq!(selected(&view, cx), "");
}

#[gpui::test]
fn double_and_triple_click_select_unicode_words_and_lines(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx, "one Cafe\u{301} 服务\r\nnext line");
    let position = at(&view, 6.2, 0.3, cx);
    click(position, 2, Modifiers::default(), cx);
    assert_eq!(selected(&view, cx), "Cafe\u{301}");
    click(position, 3, Modifiers::default(), cx);
    assert_eq!(selected(&view, cx), "one Cafe\u{301} 服务\n");
    let wide = at(&view, 10.2, 0.3, cx);
    click(wide, 2, Modifiers::default(), cx);
    assert_eq!(selected(&view, cx), "服务");
}

#[gpui::test]
fn word_selection_keeps_urls_and_host_ports_whole(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx, "https://example.org/a:b host:8080");
    for (column, expected) in [(11.2, "https://example.org/a:b"), (27.2, "host:8080")] {
        click(at(&view, column, 0.3, cx), 2, Modifiers::default(), cx);
        assert_eq!(selected(&view, cx), expected);
    }
}

#[gpui::test]
fn rectangular_drag_and_shift_extension_preserve_their_anchors(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx, "alpha beta\r\nALPHA BETA");
    let start = at(&view, 0.1, 0.3, cx);
    let end = at(&view, 5.1, 1.3, cx);
    drag(
        start,
        end,
        Modifiers {
            alt: true,
            ..Default::default()
        },
        cx,
    );
    assert_eq!(selected(&view, cx), "alpha\nALPHA");
    let end = at(&view, 5.1, 0.3, cx);
    drag(start, end, Modifiers::default(), cx);
    assert_eq!(selected(&view, cx), "alpha");
    click(
        at(&view, 9.8, 0.3, cx),
        1,
        Modifiers {
            shift: true,
            ..Default::default()
        },
        cx,
    );
    assert_eq!(selected(&view, cx), "alpha beta");
    drag(
        at(&view, 10.1, 0.3, cx),
        at(&view, 6.1, 0.3, cx),
        Modifiers::default(),
        cx,
    );
    assert_eq!(selected(&view, cx), "beta");
}

#[gpui::test]
fn word_drag_extends_by_whole_words(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx, "one two three");
    cx.simulate_event(MouseDownEvent {
        position: at(&view, 5.2, 0.3, cx),
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count: 2,
        first_mouse: false,
    });
    let end = at(&view, 10.2, 0.3, cx);
    cx.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    draw(cx);
    assert_eq!(selected(&view, cx), "two three");
}

#[gpui::test]
fn select_all_and_shift_page_navigation_stay_local(cx: &mut TestAppContext) {
    let text = (0..80)
        .map(|index| format!("row {index}\r\n"))
        .collect::<String>();
    let (view, cx) = fixture(cx, &text);
    let page = view.read_with(cx, |view, _| usize::from(view.dimensions.1 - 1));
    let bottom = view.read_with(cx, |view, _| view.engine.viewport_top().unwrap());
    assert!(bottom >= page, "fixture has at least a page of history");
    cx.simulate_keystrokes("shift-pageup");
    assert_eq!(
        view.read_with(cx, |view, _| view.engine.viewport_top().unwrap()),
        bottom - page
    );
    cx.simulate_keystrokes("shift-pagedown");
    assert_eq!(
        view.read_with(cx, |view, _| view.engine.viewport_top().unwrap()),
        bottom
    );
    if !cfg!(target_os = "macos") {
        return;
    }
    cx.simulate_keystrokes("cmd-a cmd-c");
    let selection = selected(&view, cx);
    assert!(selection.starts_with("row 0\n"));
    assert!(selection.contains("row 79\n"));
    cx.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some(selection)
        )
    });
}

#[gpui::test]
fn platform_copy_shortcuts_use_the_terminal_selection(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx, "one Cafe\u{301} 服务");
    click(at(&view, 6.2, 0.3, cx), 2, Modifiers::default(), cx);
    let shortcuts: &[&str] = if cfg!(target_os = "macos") {
        &["cmd-c"]
    } else {
        &["ctrl-shift-c", "ctrl-insert"]
    };
    for shortcut in shortcuts {
        cx.update(|_, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string("unchanged".into()));
        });
        cx.simulate_keystrokes(shortcut);
        cx.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some("Cafe\u{301}".into()),
                "{shortcut} must copy the complete selected Unicode text"
            );
        });
        assert_eq!(selected(&view, cx), "Cafe\u{301}");
    }
    // Ctrl+C still belongs to the remote shell, even with selected text.
    cx.update(|_, cx| {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("unchanged".into()));
    });
    cx.simulate_keystrokes("ctrl-c");
    cx.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("unchanged".into())
        );
    });
}

#[test]
fn clipboard_shortcuts_do_not_intercept_shell_or_altgr_input() {
    use super::{LocalShortcut, local_shortcut};
    use gpui::Keystroke;

    let shortcuts = if cfg!(target_os = "macos") {
        vec![
            ("cmd-a", LocalShortcut::SelectAll),
            ("cmd-c", LocalShortcut::Copy),
            ("cmd-v", LocalShortcut::Paste),
        ]
    } else {
        vec![
            ("ctrl-shift-c", LocalShortcut::Copy),
            ("ctrl-insert", LocalShortcut::Copy),
            ("ctrl-shift-v", LocalShortcut::Paste),
            ("shift-insert", LocalShortcut::Paste),
        ]
    };
    for (key, action) in shortcuts {
        assert_eq!(
            local_shortcut(&Keystroke::parse(key).unwrap()),
            Some(action),
            "{key}"
        );
    }
    for key in [
        "ctrl-a",
        "ctrl-c",
        "ctrl-v",
        "alt-ctrl-v",
        "alt-ctrl-shift-v",
        "insert",
        "ctrl-shift-insert",
        "alt-shift-insert",
        "cmd-alt-c",
    ] {
        assert_eq!(
            local_shortcut(&Keystroke::parse(key).unwrap()),
            None,
            "{key}"
        );
    }
}

#[gpui::test]
fn remote_mouse_mode_preserves_shift_selection_override(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx, "alpha beta\x1b[?1000h\x1b[?1006h");
    let start = at(&view, 0.1, 0.3, cx);
    let end = at(&view, 5.1, 0.3, cx);
    drag(start, end, Modifiers::default(), cx);
    assert_eq!(selected(&view, cx), "");
    drag(
        start,
        end,
        Modifiers {
            shift: true,
            ..Default::default()
        },
        cx,
    );
    assert_eq!(selected(&view, cx), "alpha");
    cx.simulate_keystrokes("shift-pageup");
    assert_eq!(selected(&view, cx), "alpha");
}

#[gpui::test]
fn dragging_outside_scrolls_history_until_release(cx: &mut TestAppContext) {
    let text = (0..80)
        .map(|index| format!("row {index}\r\n"))
        .collect::<String>();
    let (view, cx) = fixture(cx, &text);
    let bottom = view.read_with(cx, |view, _| view.engine.viewport_top().unwrap());
    let start = at(&view, 0.1, 2.3, cx);
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    draw(cx);
    let outside = at(&view, 2., -2., cx);
    cx.simulate_mouse_move(outside, Some(MouseButton::Left), Modifiers::default());
    view.update(cx, |view, _| {
        assert_eq!(view.selection_pointer, Some(outside));
        view.selection_scroll_at = Instant::now() - Duration::from_millis(60);
        assert!(view.scroll_selection());
        assert!(view.engine.viewport_top().unwrap() < bottom);
    });
    assert!(!selected(&view, cx).is_empty());
    cx.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    view.update(cx, |view, _| {
        assert!(!view.selecting);
        assert!(view.selection_pointer.is_none());
        view.selection_scroll_at = Instant::now() - Duration::from_millis(60);
        assert!(!view.scroll_selection());
    });
}

#[gpui::test]
#[ignore = "manual terminal frame benchmark; includes CPU layout/paint, excludes GPU presentation and SSH"]
fn benchmark_terminal_frames(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx, "");
    cx.simulate_resize(size(px(1100.), px(650.)));
    draw(cx);
    for (name, text) in [
        ("sparse", "$ echo hello\r\nhello\r\n$ ".to_string()),
        ("dense", "abcdefghijklmnopqrstuvwxyz0123456789 ".repeat(300)),
        ("emoji", "|a🪉🫩z| ".repeat(600)),
        (
            "styled",
            "\x1b[31merror \x1b[1;32mOK \x1b[0m界e\u{301} ".repeat(600),
        ),
    ] {
        view.update(cx, |view, cx| {
            view.engine.advance(b"\x1b[2J\x1b[H").unwrap();
            view.engine.advance(text.as_bytes()).unwrap();
            cx.notify();
        });
        draw(cx);
        let start = Instant::now();
        for _ in 0..60 {
            view.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
        eprintln!(
            "terminal {name}: {:.2} ms/frame (60 frames)",
            start.elapsed().as_secs_f64() * 1000. / 60.
        );
    }
}
