use newport_terminal::{Engine, ScrollViewport};

#[test]
fn plain_colored_localhost_urls_work_without_osc8_in_full_screen_apps() {
    let mut engine = Engine::new(80, 4).unwrap();
    engine
        .advance(b"\x1b[?1049h\x1b[?1000hDemo: \x1b[34;1mhttp://localhost:5178/\x1b[0m")
        .unwrap();
    let frame = engine.frame().unwrap();
    for column in 6..28 {
        assert_eq!(frame.link_at(column, 0), Some("http://localhost:5178/"));
        assert!(
            frame.rows[0].cells[column].hyperlink.is_none(),
            "no OSC 8 metadata"
        );
    }
    assert_eq!(frame.link_at(5, 0), None);
    assert_eq!(frame.link_at(28, 0), None);
}

#[test]
fn url_ranges_follow_soft_wraps_unicode_and_punctuation() {
    let mut engine = Engine::new(16, 5).unwrap();
    let url = "https://example.org/a_(b)?q=1&x=2#part";
    engine.advance(format!("({url}).").as_bytes()).unwrap();
    let frame = engine.frame().unwrap();
    for offset in 1..=url.len() {
        assert_eq!(frame.link_at(offset % 16, offset / 16), Some(url));
    }
    assert_eq!(frame.link_at(0, 0), None);
    assert_eq!(
        frame.link_at((url.len() + 1) % 16, (url.len() + 1) / 16),
        None
    );
    drop(frame);
    engine
        .advance("\x1bc界e\u{301} http://host/路径".as_bytes())
        .unwrap();
    let frame = engine.frame().unwrap();
    for offset in 4..20 {
        assert_eq!(
            frame.link_at(offset % 16, offset / 16),
            Some("http://host/路径")
        );
    }
}

#[test]
fn explicit_links_win_and_concealed_text_and_unsafe_schemes_stay_inert() {
    let mut engine = Engine::new(80, 5).unwrap();
    engine.advance(b"\x1b]8;;https://target.example/\x07https://label.example/\x1b]8;;\x07\r\n\x1b[8mhttps://hidden.example/\x1b[0m\r\nfile:///etc/passwd javascript:alert(1)\r\nHTTPS://[::1]:8080/path").unwrap();
    let frame = engine.frame().unwrap();
    assert_eq!(frame.link_at(4, 0), Some("https://target.example/"));
    for row in 1..3 {
        for column in 0..80 {
            assert_eq!(frame.link_at(column, row), None);
        }
    }
    assert_eq!(frame.link_at(8, 3), Some("https://[::1]:8080/path"));
}

#[test]
fn cached_links_follow_rewrites_scrollback_and_screen_switches() {
    let mut engine = Engine::new(40, 3).unwrap();
    engine.advance(b"http://localhost:5178/").unwrap();
    assert!(engine.frame().unwrap().link_at(0, 0).is_some());
    engine.advance(b"\r\x1b[Kplain text").unwrap();
    assert!(engine.frame().unwrap().link_at(0, 0).is_none());
    engine
        .advance(b"\r\x1b[Khttp://localhost:5191/\r\na\r\nb\r\nc\r\nd")
        .unwrap();
    assert!(engine.frame().unwrap().link_at(0, 0).is_none());
    engine.scroll(ScrollViewport::Top);
    assert_eq!(
        engine.frame().unwrap().link_at(0, 0),
        Some("http://localhost:5191/")
    );
    engine.advance(b"\x1b[?1049h").unwrap();
    assert!(engine.frame().unwrap().link_at(0, 0).is_none());
}

#[test]
fn hard_newlines_do_not_join_urls_and_partial_urls_are_not_opened() {
    let mut engine = Engine::new(20, 3).unwrap();
    engine.advance(b"http://\r\nexample.org").unwrap();
    assert!(engine.frame().unwrap().link_at(0, 0).is_none());
    engine
        .advance(format!("\x1bchttps://host/{}", "a".repeat(100)).as_bytes())
        .unwrap();
    engine.scroll(ScrollViewport::Top);
    assert!(engine.frame().unwrap().link_at(0, 0).is_none());
}
