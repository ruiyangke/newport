//! User-visible guarantees for links, accessible text, titles and reconnect.
use newport_terminal::{Engine, Point, PointCoordinate, ScrollViewport};

fn active(x: u16, y: u32) -> Point {
    Point::Active(PointCoordinate { x, y })
}

#[test]
fn fragmented_osc_links_follow_wide_cells_history_and_screen_switches() {
    let mut engine = Engine::new(12, 2).unwrap();
    for byte in "\x1b]8;id=docs;HTTPS://example.org/docs\x1b\\文档\x1b]8;;\x1b\\ plain".as_bytes()
    {
        let effects = engine.advance(&[*byte]).unwrap();
        assert!(effects.replies.is_empty());
        assert!(effects.title.is_none());
    }
    for column in 0..4 {
        assert_eq!(
            engine.link_at(active(column, 0)).unwrap().as_deref(),
            Some("https://example.org/docs")
        );
    }
    assert!(engine.link_at(active(5, 0)).unwrap().is_none());
    engine.advance(b"\r\nsecond\r\nthird").unwrap();
    assert_eq!(
        engine
            .link_at(Point::Screen(PointCoordinate { x: 1, y: 0 }))
            .unwrap()
            .as_deref(),
        Some("https://example.org/docs")
    );
    assert!(
        engine
            .link_at(Point::Screen(PointCoordinate { x: 0, y: 100 }))
            .unwrap()
            .is_none()
    );
    assert!(engine.link_at(active(12, 0)).unwrap().is_none());
    engine.advance(b"\x1b[?1049h").unwrap();
    assert!(engine.link_at(active(0, 0)).unwrap().is_none());
}

#[test]
fn concealed_http_links_are_not_actionable() {
    let mut engine = Engine::new(30, 2).unwrap();
    engine
        .advance(
            b"\x1b]8;;file:///etc/passwd\x07file\x1b]8;;\x07 \x1b[8m\x1b]8;;https://host\x07hidden",
        )
        .unwrap();
    for column in 0..12 {
        assert!(engine.link_at(active(column, 0)).unwrap().is_none());
    }
}

#[test]
fn accessible_text_tracks_concealment_alternate_screen_and_progress_rewrites() {
    let mut engine = Engine::new(30, 4).unwrap();
    engine
        .advance("A界e\u{301}\r\n\x1b[8mhidden\x1b[0m visible".as_bytes())
        .unwrap();
    assert_eq!(
        engine.visible_lines().unwrap(),
        ["A界e\u{301}", "       visible"]
    );
    engine.advance(b"\x1b[?1049hAlternate").unwrap();
    let alternate = engine.visible_lines().unwrap().join("\n");
    assert!(alternate.contains("Alternate"));
    assert!(!alternate.contains("visible"));

    let mut engine = Engine::new(20, 2).unwrap();
    engine.advance(b"first\r\nsecond\r\nthird").unwrap();
    assert_eq!(engine.visible_lines().unwrap(), ["second", "third"]);
    engine.scroll(ScrollViewport::Top);
    assert_eq!(engine.visible_lines().unwrap(), ["first", "second"]);
    engine.scroll(ScrollViewport::Bottom);
    engine.advance(b"\r\x1b[Kdone").unwrap();
    assert_eq!(engine.visible_lines().unwrap(), ["second", "done"]);
}

#[test]
fn remote_titles_are_sanitized_and_bounded_in_utf16_units() {
    let mut engine = Engine::new(80, 24).unwrap();
    let effects = engine
        .advance("\x1b]2; \u{202e}safe\nname\u{7f} \x07".as_bytes())
        .unwrap();
    assert_eq!(effects.title, Some(Some("safename".into())));
    let effects = engine
        .advance(format!("\x1b]2;{}\x07", "😀".repeat(100)).as_bytes())
        .unwrap();
    let title = effects.title.flatten().unwrap();
    assert_eq!(title.encode_utf16().count(), 120);
    assert_eq!(title, "😀".repeat(60));
    assert!(engine.resize(1, 10, 8, 16).is_err());
}

#[test]
fn reconnect_resets_application_cursor_and_paste_modes_without_erasing_primary_output() {
    use newport_terminal::Mode;
    let mut engine = Engine::new(80, 24).unwrap();
    engine
        .advance(b"shell\x1b[?1049h\x1b[?2004h\x1b[?1hTUI")
        .unwrap();
    assert!(engine.alternate_screen().unwrap());
    assert!(engine.mode(Mode::BRACKETED_PASTE).unwrap());
    assert!(engine.mode(Mode::DECCKM).unwrap());
    engine.reconnect().unwrap();
    assert!(!engine.alternate_screen().unwrap());
    assert!(!engine.mode(Mode::BRACKETED_PASTE).unwrap());
    assert!(!engine.mode(Mode::DECCKM).unwrap());
    assert_eq!(engine.frame().unwrap().rows[0].cells[0].text, "s");
}
