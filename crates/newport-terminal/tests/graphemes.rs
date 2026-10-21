//! Grapheme clusters keep search, selection, links, styles and cursor reports in sync.
use newport_terminal::{
    Cell, CellWide, Engine, Point, PointCoordinate, Position, Row, ScrollViewport, StyleColor,
};

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
fn history(engine: &mut Engine) -> usize {
    engine.frame().unwrap().scrollback_rows
}
/// Inspect retained output without replacing the user's selection. Viewport
/// snapshots are copied before scrolling, then the original viewport is restored.
fn retained_rows(engine: &mut Engine) -> Vec<Row> {
    let original = engine.viewport_top().unwrap();
    let count = {
        let frame = engine.frame().unwrap();
        frame.scrollback_rows + frame.rows.len()
    };
    let mut rows = Vec::with_capacity(count);
    while rows.len() < count {
        engine.scroll(ScrollViewport::Row(rows.len()));
        let frame = engine.frame().unwrap();
        let skip = rows.len() - frame.viewport_top;
        rows.extend(frame.rows.iter().skip(skip).cloned());
    }
    engine.scroll(ScrollViewport::Row(original));
    rows
}
fn retained_text(engine: &mut Engine) -> String {
    retained_rows(engine)
        .iter()
        .flat_map(|row| &row.cells)
        .filter(|cell| !matches!(cell.wide, CellWide::SpacerHead | CellWide::SpacerTail))
        .flat_map(|cell| cell.text.chars())
        .filter(|&c| c != ' ')
        .collect()
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
        .select_range(active(2, 0), active(4, 0), false)
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
    // Ghostty's pinned Unicode data classifies both emoji as two cells.
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
fn promotion_preserves_color_links_and_focus_changes_between_chunks() {
    let mut engine = Engine::new(10, 2).unwrap();
    feed(
        &mut engine,
        "\x1b]8;;https://newport.app\x07\x1b[31m123456789❤",
    );
    assert!(engine.encode_focus(false).unwrap().is_empty());
    feed(&mut engine, "️");
    let cell = cell(&mut engine, 1, 0);
    assert_eq!(
        cell.style.fg_color,
        StyleColor::Palette(newport_terminal::PaletteIndex::RED)
    );
    assert_eq!(cell.wide, CellWide::Wide);
    assert_eq!(
        engine.link_at(active(1, 1)).unwrap().as_deref(),
        Some("https://newport.app")
    );
    assert_eq!(lines(&mut engine), ["123456789", "❤️"]);
}

#[test]
fn long_combining_run_is_bounded_without_corrupting_adjacent_output() {
    // Ghostty intentionally caps a cell at 64 suffix codepoints, bounding
    // memory and CPU. Excess marks must not escape into adjacent cells or block later output.
    let mut engine = Engine::new(10, 2).unwrap();
    let input = format!("e{}!", "\u{301}".repeat(16_384));
    for chunk in input.as_bytes().chunks(97) {
        feed(&mut engine, chunk);
    }
    let retained = format!("e{}", "\u{301}".repeat(64));
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

// REP uses Terminal.previous_char (a scalar), not the previous cell's complete
// grapheme. Terminal.printRepeat prints that base again; appending a combining
// mark/ZWJ suffix does not replace previous_char.
#[test]
fn rep_retains_original_cluster_and_repeats_base_with_current_attributes() {
    for chunk_size in [1, 2, 3, 1024] {
        for (input, expected, cursor) in [
            ("é\x1b[2b!", "éee!", 4),
            ("👩‍💻\x1b[2b!", "👩‍💻👩👩!", 7),
            ("🇺🇸\x1b[2b!", "🇺🇸🇺🇺!", 5),
            ("中\x1b[2b!", "中中中!", 7),
            ("🫩\x1b[3b!", "🫩🫩🫩🫩!", 9),
            ("❤️\x1b[b!", "❤️❤!", 4),
            ("❤️\x1b[0b!", "❤️❤!", 4),
            ("❤️\x7f\x1b[2b!", "❤️❤❤!", 5),
            ("\x1b[2b!", "!", 1),
        ] {
            let mut engine = Engine::new(20, 3).unwrap();
            for chunk in input.as_bytes().chunks(chunk_size) {
                feed(&mut engine, chunk);
            }
            assert_eq!(lines(&mut engine), [expected], "chunk {chunk_size}");
            assert_eq!(engine.cursor_position().unwrap().x, cursor);
        }
        let mut styled = Engine::new(20, 3).unwrap();
        for chunk in "é\x1b[31m\x1b[2b".as_bytes().chunks(chunk_size) {
            feed(&mut styled, chunk);
        }
        assert_eq!(cell(&mut styled, 0, 0).text, "é");
        assert_ne!(
            cell(&mut styled, 0, 0).style.fg_color,
            StyleColor::Palette(newport_terminal::PaletteIndex::RED)
        );
        for x in [1, 2] {
            let repeated = cell(&mut styled, 0, x);
            assert_eq!(repeated.text, "e");
            assert_eq!(
                repeated.style.fg_color,
                StyleColor::Palette(newport_terminal::PaletteIndex::RED)
            );
        }
        let mut wrapped = Engine::new(10, 3).unwrap();
        for chunk in "👨‍👩‍👧‍👦\x1b[15b!".as_bytes().chunks(chunk_size) {
            feed(&mut wrapped, chunk);
        }
        assert_eq!(history(&mut wrapped), 1);
        assert_eq!(
            retained_text(&mut wrapped),
            format!("👨‍👩‍👧‍👦{}!", "👨".repeat(15))
        );
        let retained = retained_rows(&mut wrapped);
        assert_eq!(retained[0].cells[0].text, "👨‍👩‍👧‍👦");
        for row in retained {
            for (x, current) in row.cells.iter().enumerate() {
                if current.wide == CellWide::Wide {
                    assert_eq!(row.cells[x + 1].wide, CellWide::SpacerTail);
                }
            }
        }
    }
}
