//! Grapheme clusters keep search, selection, links, styles and cursor reports in sync.
use newport_terminal::{Cell, CellWide, Engine, Point, PointCoordinate, Position, StyleColor};

fn active(x: u16, y: u32) -> Point {
    Point::Active(PointCoordinate { x, y })
}
fn feed(engine: &mut Engine, input: impl AsRef<[u8]>) {
    engine.advance(input.as_ref()).unwrap();
}
fn lines(engine: &mut Engine) -> Vec<String> {
    engine.visible_lines().unwrap()
}
fn cell(engine: &mut Engine, row: usize, column: usize) -> Cell {
    engine.frame().unwrap().rows[row].cells[column].clone()
}
#[test]
fn grapheme_search_selection_and_cursor_queries_use_the_same_span() {
    let mut engine = Engine::new(20, 3).unwrap();
    feed(&mut engine, "a👩‍💻🇺🇸z");
    let matches = engine.search("👩‍💻", false).unwrap();
    assert_eq!(
        matches.matches,
        [Position { row: 0, column: 1 }..=Position { row: 0, column: 2 }]
    );
    engine
        .select_range(active(1, 0), active(4, 0), false)
        .unwrap();
    assert_eq!(engine.selected_text().unwrap().as_deref(), Some("👩‍💻🇺🇸"));
    let reply = engine.advance(b"\x1b[6n").unwrap();
    assert_eq!(reply.replies, [b"\x1b[1;7R".to_vec()]);
}

#[test]
fn pinned_unicode_widths_keep_search_selection_links_and_cursor_in_sync() {
    let mut engine = Engine::new(20, 3).unwrap();
    let input = "\x1b[31m\x1b]8;;https://newport.app\x07a🪉🫩z";
    for byte in input.as_bytes() {
        feed(&mut engine, [*byte]);
    }
    // The pinned Unicode width data classifies both emoji as two cells.
    for (column, character) in [(1, "🪉"), (3, "🫩")] {
        let cell = cell(&mut engine, 0, column);
        assert_eq!(cell.text, character);
        assert_eq!(cell.wide, CellWide::Wide);
        assert_eq!(
            cell.style.fg_color,
            StyleColor::Palette(newport_terminal::PaletteIndex::RED)
        );
        assert_eq!(
            engine.link_at(active(column as u16, 0)).unwrap().as_deref(),
            Some("https://newport.app")
        );
    }
    let range = Position { row: 0, column: 1 }..=Position { row: 0, column: 4 };
    assert_eq!(
        engine.search("🪉🫩", false).unwrap().matches.as_slice(),
        std::slice::from_ref(&range)
    );
    engine
        .select_range(range.start().point(), range.end().point(), false)
        .unwrap();
    assert_eq!(engine.selected_text().unwrap().as_deref(), Some("🪉🫩"));
    assert_eq!(
        engine.advance(b"\x1b[6n").unwrap().replies,
        [b"\x1b[1;7R".to_vec()]
    );
}

#[test]
fn variation_selector_widens_an_emoji_with_its_color_and_link() {
    let mut engine = Engine::new(10, 2).unwrap();
    feed(&mut engine, "\x1b]8;;https://newport.app\x07\x1b[31m❤");
    feed(&mut engine, "️!");
    for column in [0, 1] {
        assert_eq!(
            engine.link_at(active(column, 0)).unwrap().as_deref(),
            Some("https://newport.app")
        );
    }
    let cell = cell(&mut engine, 0, 0);
    assert_eq!(cell.wide, CellWide::Wide);
    assert_eq!(
        cell.style.fg_color,
        StyleColor::Palette(newport_terminal::PaletteIndex::RED)
    );
    assert_eq!(lines(&mut engine), ["❤️!"]);
    assert_eq!(engine.cursor_position().unwrap().x, 3);
}

/// In the last column there is no room to widen, so the cell stays narrow.
#[test]
fn variation_selector_at_the_margin_keeps_color_links_and_width() {
    let mut engine = Engine::new(10, 2).unwrap();
    feed(
        &mut engine,
        "\x1b]8;;https://newport.app\x07\x1b[31m123456789❤",
    );
    assert!(engine.encode_focus(false).unwrap().is_empty());
    feed(&mut engine, "️");
    let cell = cell(&mut engine, 0, 9);
    assert_eq!(
        cell.style.fg_color,
        StyleColor::Palette(newport_terminal::PaletteIndex::RED)
    );
    assert_eq!(cell.wide, CellWide::Narrow);
    assert_eq!(
        engine.link_at(active(9, 0)).unwrap().as_deref(),
        Some("https://newport.app")
    );
    assert_eq!(lines(&mut engine), ["123456789❤️"]);
}

#[test]
fn long_combining_run_keeps_nine_marks_and_preserves_adjacent_output() {
    // A cell holds up to nine zero-width characters.
    let mut engine = Engine::new(10, 2).unwrap();
    let input = format!("e{}!", "\u{301}".repeat(16_384));
    for chunk in input.as_bytes().chunks(97) {
        feed(&mut engine, chunk);
    }
    let retained = format!("e{}", "\u{301}".repeat(9));
    assert_eq!(lines(&mut engine), [format!("{retained}!")]);
    assert_eq!(cell(&mut engine, 0, 0).text, retained);
    assert_eq!(cell(&mut engine, 0, 1).text, "!");
    assert_eq!(engine.cursor_position().unwrap().x, 2);
    assert_eq!(
        engine.search(&retained, true).unwrap().matches,
        [Position { row: 0, column: 0 }..=Position { row: 0, column: 0 }]
    );
    engine
        .select_range(active(0, 0), active(1, 0), false)
        .unwrap();
    assert_eq!(
        engine.selected_text().unwrap(),
        Some(format!("{retained}!"))
    );
    feed(&mut engine, "\r\nnext");
    assert_eq!(lines(&mut engine), [format!("{retained}!"), "next".into()]);
}

// REP repeats the last character received, including a combining mark, until
// a control resets it; it does not repeat a whole cluster.
#[test]
fn rep_repeats_the_last_scalar() {
    for chunk_size in [1, 2, 3, 1024] {
        for (input, expected, cursor) in [
            ("é\x1b[2b!", "é́́!", 2),
            ("👩‍💻\x1b[2b!", "👩‍💻💻💻!", 7),
            ("中\x1b[2b!", "中中中!", 7),
            ("❤️\x7f\x1b[2b!", "❤️!", 3),
        ] {
            let mut engine = Engine::new(20, 3).unwrap();
            for chunk in input.as_bytes().chunks(chunk_size) {
                feed(&mut engine, chunk);
            }
            assert_eq!(lines(&mut engine), [expected]);
            assert_eq!(engine.cursor_position().unwrap().x, cursor);
        }
    }
}
