use newport_terminal::{
    Engine, PaletteIndex, Point, PointCoordinate, RgbColor, ScrollViewport, Theme,
};

fn point(x: u16, y: u32) -> Point {
    Point::Active(PointCoordinate { x, y })
}
fn rgb(value: u32) -> RgbColor {
    RgbColor {
        r: (value >> 16) as u8,
        g: (value >> 8) as u8,
        b: value as u8,
    }
}
fn theme() -> Theme {
    Theme {
        foreground: rgb(0xeeeeef),
        background: rgb(0x1e1f21),
        cursor: rgb(0xeeeeef),
        palette: [rgb(0x123456); 256],
    }
}
fn all(engine: &mut Engine) -> String {
    engine.select_all().unwrap();
    engine
        .selected_text()
        .unwrap()
        .unwrap_or_default()
        .trim_end_matches('\n')
        .to_owned()
}

#[test]
fn split_utf8_graphemes_and_progress_rewrites() {
    let mut engine = Engine::new(30, 4).unwrap();
    for byte in "A界e\u{301} 👩🏽‍💻\r\nprogress 10%\rprogress 90%".as_bytes() {
        engine.advance(&[*byte]).unwrap();
    }
    assert_eq!(
        engine.visible_lines().unwrap(),
        ["A界e\u{301} 👩🏽‍💻", "progress 90%"]
    );
    let frame = engine.frame().unwrap();
    assert_eq!(frame.rows[0].cells[1].text, "界");
    assert_eq!(frame.rows[0].cells[3].text, "e\u{301}");
    assert_eq!(frame.rows[0].cells[5].text, "👩🏽‍💻");
    assert_eq!(frame.rows[0].cells[7].text, "");
}

#[test]
fn titles_bell_replies_and_clipboard_isolation() {
    let mut engine = Engine::new(30, 4).unwrap();
    let effects = engine.advance(b"\x1b]2;hello\x07\x07\x1b[6n").unwrap();
    assert_eq!(effects.title, Some(Some("hello".into())));
    assert!(effects.bell);
    assert_eq!(effects.replies.concat(), b"\x1b[1;1R");
    // A program may copy text out (tmux, editors), but never read the host
    // clipboard: the query gets no answer.
    let effects = engine
        .advance(b"\x1b]52;c;?\x07\x1b]52;c;aGVsbG8=\x07")
        .unwrap();
    assert!(effects.replies.is_empty());
    assert!(effects.title.is_none());
    assert_eq!(effects.copy.as_deref(), Some("hello"));
    // Split across reads, as SSH delivers it; the latest copy wins.
    let mut copied = None;
    for chunk in b"\x1b]52;c;b25l\x07\x1b]52;c;dHdv\x07".chunks(3) {
        copied = engine.advance(chunk).unwrap().copy.or(copied);
    }
    assert_eq!(copied.as_deref(), Some("two"));
    // tmux leaves the destination empty, which means the clipboard.
    let effects = engine.advance(b"\x1b]52;;dG11eA==\x07").unwrap();
    assert_eq!(effects.copy.as_deref(), Some("tmux"));
    // Not the primary selection, not a request to clear the clipboard.
    for refused in [&b"\x1b]52;p;aGVsbG8=\x07"[..], b"\x1b]52;c;\x07"] {
        assert!(engine.advance(refused).unwrap().copy.is_none());
    }
}

#[test]
fn resize_preserves_output_below_cursor() {
    for chunk in [1, 5, 64] {
        let mut engine = Engine::new(10, 4).unwrap();
        for bytes in b"prompt\r\n123456789abcdef\x1b[1;2H".chunks(chunk) {
            engine.advance(bytes).unwrap();
        }
        engine.resize(6, 3, 8, 16).unwrap();
        assert_eq!(all(&mut engine).replace('\n', ""), "prompt123456789abcdef");
        engine.resize(20, 8, 8, 16).unwrap();
        assert_eq!(all(&mut engine).replace('\n', ""), "prompt123456789abcdef");
    }
}

#[test]
fn height_shrink_keeps_output_below_cursor_and_alternate_screen_restores_primary() {
    let mut engine = Engine::new(20, 5).unwrap();
    engine
        .advance(b"first\r\nsecond\r\nthird\r\nfourth\r\nfifth\x1b[H")
        .unwrap();
    engine.resize(20, 2, 8, 16).unwrap();
    let output = "first\nsecond\nthird\nfourth\nfifth";
    assert_eq!(all(&mut engine), output);
    assert_eq!(engine.visible_lines().unwrap(), ["fourth", "fifth"]);
    engine.advance(b"\x1b[?1049hAlternate").unwrap();
    assert_eq!(engine.visible_lines().unwrap(), ["Alternate"]);
    engine.advance(b"\x1b[?1049l").unwrap();
    assert_eq!(all(&mut engine), output);
}

#[test]
fn height_shrink_moves_output_below_the_cursor_into_view() {
    let mut engine = Engine::new(20, 4).unwrap();
    engine
        .advance(b"one\r\ntwo\r\nthree\r\nfour\x1b[2;1H")
        .unwrap();
    engine.resize(20, 3, 8, 16).unwrap();
    assert_eq!(engine.visible_lines().unwrap(), ["two", "three", "four"]);
    engine.advance(b"!").unwrap();
    assert_eq!(engine.visible_lines().unwrap(), ["!wo", "three", "four"]);
    // The saved cursor moves with its text too.
    let mut engine = Engine::new(20, 4).unwrap();
    engine
        .advance(b"one\r\ntwo\x1b7\r\nthree\r\nfour\x1b[1;1H")
        .unwrap();
    engine.resize(20, 3, 8, 16).unwrap();
    engine.advance(b"\x1b8!").unwrap();
    assert_eq!(all(&mut engine), "one\ntwo!\nthree\nfour");
}

#[test]
fn scrollback_and_owned_frames_survive_updates() {
    let mut engine = Engine::new(20, 2).unwrap();
    engine.advance(b"first\r\nsecond\r\nthird").unwrap();
    let old_frame = engine.frame().unwrap().clone();
    assert_eq!(old_frame.visible_lines(), ["second", "third"]);
    engine.scroll(ScrollViewport::Top);
    assert_eq!(engine.visible_lines().unwrap(), ["first", "second"]);
    engine.scroll(ScrollViewport::Bottom);
    engine.advance(b"\r\nfourth").unwrap();
    assert_eq!(old_frame.visible_lines(), ["second", "third"]);
    assert_eq!(engine.visible_lines().unwrap(), ["third", "fourth"]);
}

#[test]
fn colors_queries_and_osc_overrides_survive_theme_change() {
    let mut engine = Engine::new(20, 2).unwrap();
    engine.set_theme(theme()).unwrap();
    let response = engine.advance(b"\x1b]11;?\x07").unwrap();
    assert!(String::from_utf8_lossy(&response.replies.concat()).contains("1e1e/1f1f/2121"));
    engine
        .advance(b"\x1b]4;1;#ff0044\x07\x1b[31mred\x1b[0m plain")
        .unwrap();
    assert_eq!(
        engine.frame().unwrap().rows[0].cells[0].foreground,
        Some(rgb(0xff0044))
    );
    let mut light = theme();
    light.background = rgb(0xffffff);
    light.palette[PaletteIndex::RED.0 as usize] = rgb(0x00ff00);
    engine.set_theme(light).unwrap();
    assert_eq!(
        engine.frame().unwrap().rows[0].cells[0].foreground,
        Some(rgb(0xff0044))
    );
    assert_eq!(engine.frame().unwrap().background, rgb(0xffffff));
}

#[test]
fn selection_joins_wrapped_lines_and_can_select_words() {
    let mut engine = Engine::new(5, 4).unwrap();
    engine.advance(b"abcdefgh\r\nworld").unwrap();
    engine
        .select_range(point(2, 0), point(1, 1), false)
        .unwrap();
    assert_eq!(engine.selected_text().unwrap().as_deref(), Some("cdefg"));
    engine.clear_selection().unwrap();
    assert_eq!(engine.selected_text().unwrap(), None);
    engine.select_word(point(2, 2)).unwrap();
    assert_eq!(engine.selected_text().unwrap().as_deref(), Some("world"));
}

#[test]
fn hidden_and_non_http_links_are_not_actionable() {
    let mut engine = Engine::new(40, 3).unwrap();
    engine
        .advance(b"\x1b]8;;https://example.org/docs\x07docs\x1b]8;;\x07 \x1b[8mhidden\x1b[0m")
        .unwrap();
    assert_eq!(
        engine.link_at(point(0, 0)).unwrap().as_deref(),
        Some("https://example.org/docs")
    );
    assert_eq!(engine.visible_lines().unwrap(), ["docs"]);
    engine
        .advance(b"\r\n\x1b]8;;file:///etc/passwd\x07file")
        .unwrap();
    assert!(engine.link_at(point(0, 1)).unwrap().is_none());
}

#[test]
fn links_cover_wide_cells_and_selection_marks_dirty_rows() {
    let mut engine = Engine::new(30, 3).unwrap();
    engine
        .advance("\x1b]8;;https://example.org/docs\x07文档\x1b]8;;\x07 plain".as_bytes())
        .unwrap();
    for column in 0..4 {
        assert_eq!(
            engine.link_at(point(column, 0)).unwrap().as_deref(),
            Some("https://example.org/docs")
        );
    }
    assert!(
        engine.frame().unwrap().rows[0]
            .cells
            .iter()
            .all(|cell| !cell.selected)
    );
    engine
        .select_range(point(0, 0), point(3, 0), false)
        .unwrap();
    assert!(
        engine.frame().unwrap().rows[0].cells[..4]
            .iter()
            .all(|cell| cell.selected)
    );
    engine.clear_selection().unwrap();
    assert!(
        engine.frame().unwrap().rows[0]
            .cells
            .iter()
            .all(|cell| !cell.selected)
    );
}

#[test]
fn reconnect_aborts_partial_sequences_and_restores_the_primary_screen() {
    for unfinished in ["\x1b[", "\x1b]2;unfinished", "\x1bPunfinished"] {
        let mut engine = Engine::new(30, 3).unwrap();
        engine
            .advance(b"retained\x1b[?1049hfull screen\x1b[?2004h\x1b[?1004h")
            .unwrap();
        engine.advance(unfinished.as_bytes()).unwrap();
        engine.reconnect().unwrap();
        engine.advance(b"new session").unwrap();
        assert_eq!(
            engine.visible_lines().unwrap(),
            ["retained", "new session"],
            "{unfinished:?}"
        );
        assert!(engine.encode_focus(true).unwrap().is_empty());
        assert_eq!(engine.encode_paste("text").unwrap(), b"text");
    }
}

#[test]
fn reconnect_starts_the_new_session_below_the_retained_output() {
    for (old, expected) in [
        ("one\r\n$ ", ["one", "$", "$ new"]),
        ("one\r\ntwo\r\n", ["one", "two", "$ new"]),
        ("\x1b[2;3rone\r\n$ \x1b[?1049hfull", ["one", "$", "$ new"]),
    ] {
        let mut engine = Engine::new(30, 4).unwrap();
        engine.advance(old.as_bytes()).unwrap();
        engine.reconnect().unwrap();
        engine.advance(b"$ new").unwrap();
        assert_eq!(engine.visible_lines().unwrap(), expected, "{old:?}");
    }
}

#[test]
fn reset_restores_clustering_and_theme_updates_leave_sgr_intact() {
    let mut engine = Engine::new(30, 3).unwrap();
    engine.advance("\x1b[?2027l\x1bc👩🏽‍💻".as_bytes()).unwrap();
    assert_eq!(engine.frame().unwrap().rows[0].cells[0].text, "👩🏽‍💻");
    engine.advance(b"\x1b[1;3;4;38;2;20;30;40mstyled").unwrap();
    engine.set_theme(theme()).unwrap();
    let frame = engine.frame().unwrap();
    let cell = &frame.rows[0].cells[2];
    assert!(cell.style.bold && cell.style.italic);
    assert_eq!(
        cell.foreground,
        Some(RgbColor {
            r: 20,
            g: 30,
            b: 40
        })
    );
}

#[test]
fn reconnect_clears_keyboard_stacks_on_both_screens() {
    for alternate in [false, true] {
        let mut engine = Engine::new(20, 3).unwrap();
        engine
            .advance(b"retained\x1b[>31u\x1b[?1049h\x1b[>1u")
            .unwrap();
        if !alternate {
            engine.advance(b"\x1b[?1049l").unwrap();
        }
        engine.reconnect().unwrap();
        assert_eq!(engine.visible_lines().unwrap(), ["retained"]);
        assert!(engine.kitty_keyboard_flags().unwrap().is_empty());
        engine.advance(b"\x1b[?1049h").unwrap();
        assert!(engine.kitty_keyboard_flags().unwrap().is_empty());
    }
}

#[test]
fn upstream_keyboard_stack_limit_does_not_panic_or_pop_titles() {
    let mut engine = Engine::new(20, 3).unwrap();
    engine
        .advance(b"\x1b]2;retained title\x07\x1b[22;0t")
        .unwrap();
    // Regression for upstream #9071, fixed in our unmodified pinned revision.
    engine.advance(b"\x1b[>1u".repeat(4097).as_slice()).unwrap();
    assert_eq!(
        engine.advance(b"\x1b[?u").unwrap().replies.concat(),
        b"\x1b[?1u"
    );
    let effects = engine.advance(b"\x1b]2;temporary\x07\x1b[23;0t").unwrap();
    assert_eq!(effects.title, Some(Some("retained title".into())));
}
