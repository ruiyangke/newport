//! Height and width changes keep output, styles, selection and the saved cursor together.
use newport_terminal::{Cell, CellWide, Engine, Point, PointCoordinate, Row, ScrollViewport};

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
fn same_row(actual: &Row, expected: &Row) {
    assert_eq!(actual.wrapped, expected.wrapped);
    assert_eq!(actual.cells.len(), expected.cells.len());
    for (actual, expected) in actual.cells.iter().zip(&expected.cells) {
        assert_eq!(actual.text, expected.text);
        assert_eq!(actual.wide, expected.wide);
        assert_eq!(actual.style, expected.style);
        assert_eq!(actual.foreground, expected.foreground);
        assert_eq!(actual.background, expected.background);
    }
}

#[test]
fn height_shrink_keeps_output_selection_and_saved_cursor_together() {
    for cursor_row in [1_u32, 2, 3] {
        let mut engine = Engine::new(10, 3).unwrap();
        feed(&mut engine, format!("a\r\nb\r\nc\x1b[{cursor_row};2H\x1b7"));
        engine
            .select_range(active(0, 0), active(0, 2), false)
            .unwrap();
        let selected = engine.selected_text().unwrap();
        engine.resize(10, 2, 8, 16).unwrap();
        assert_eq!(history(&mut engine), 1);
        assert_eq!(retained_rows(&mut engine)[0].cells[0].text, "a");
        assert_eq!(lines(&mut engine), ["b", "c"]);
        assert_eq!(engine.selected_text().unwrap(), selected);
        let cursor = engine.cursor_position().unwrap();
        assert_eq!(cursor.y, cursor_row.saturating_sub(2));
        // Restoring DECSC tests the saved cursor through the public protocol,
        // rather than accessing the previous engine's private saved-grid state.
        feed(&mut engine, b"\x1b8");
        let restored = engine.cursor_position().unwrap();
        assert_eq!((restored.x, restored.y), (cursor.x, cursor.y));
        feed(&mut engine, b"!");
        assert_eq!(engine.cursor_position().unwrap().x, 2);
    }
}

#[test]
fn height_shrink_consumes_only_unused_tail_rows_and_preserves_hidden_primary_output() {
    let mut empty = Engine::new(10, 3).unwrap();
    feed(&mut empty, b"a\r\nb\x1b[1;2H");
    empty.resize(10, 2, 8, 16).unwrap();
    assert_eq!(history(&mut empty), 0);
    assert_eq!(lines(&mut empty), ["a", "b"]);

    let mut styled = Engine::new(10, 3).unwrap();
    feed(&mut styled, b"a\r\nb\r\n\x1b[41m\x1b[2K\x1b[0m\x1b[1;2H");
    let before = styled.frame().unwrap().rows[2].clone();
    styled.resize(10, 2, 8, 16).unwrap();
    assert_eq!(history(&mut styled), 1);
    same_row(&styled.frame().unwrap().rows[1], &before);

    let mut alternate = Engine::new(10, 3).unwrap();
    feed(&mut alternate, b"a\r\nb\r\nc\x1b[1;2H\x1b[?1049h\x1b[HALT");
    alternate.resize(10, 2, 8, 16).unwrap();
    assert_eq!(lines(&mut alternate), ["ALT"]);
    assert_eq!(history(&mut alternate), 0);
    feed(&mut alternate, b"\x1b[?1049l");
    assert_eq!(history(&mut alternate), 1);
    assert_eq!(retained_rows(&mut alternate)[0].cells[0].text, "a");
    assert_eq!(lines(&mut alternate), ["b", "c"]);
}

#[test]
fn narrowing_preserves_styled_tail_rows() {
    let mut styled = Engine::new(10, 3).unwrap();
    feed(
        &mut styled,
        b"prompt\r\n123456789\x1b[3;1H\x1b[41m\x1b[2K\x1b[0m\x1b[1;2H",
    );
    let background = cell(&mut styled, 2, 0).background;
    assert!(background.is_some(), "the tail row has a background color");
    styled.resize(6, 3, 8, 16).unwrap();
    assert!(history(&mut styled) > 0, "styled rows are not unused space");
    let colored = retained_rows(&mut styled)
        .iter()
        .flat_map(|row| &row.cells)
        .filter(|cell| cell.background == background)
        .count();
    assert_eq!(colored, 10, "colored blanks must survive reflow");
}

#[test]
fn narrowing_keeps_the_complete_cursor_paragraph() {
    let mut engine = Engine::new(10, 3).unwrap();
    feed(&mut engine, b"1234567890\r\nabcdefghij \r\x1b[K\x1b[2;2H");
    assert!(engine.frame().unwrap().rows[1].wrapped);
    engine.resize(6, 3, 8, 16).unwrap();
    assert_eq!(history(&mut engine), 1);
    // Ghostty reflows the complete paragraph, preserving its suffix.
    assert_eq!(lines(&mut engine), ["7890", "abcdef", "ghij"]);
    assert_eq!(retained_text(&mut engine), "1234567890abcdefghij");
    let frame = engine.frame().unwrap();
    assert!(frame.rows[1].wrapped);
}

#[test]
fn narrowing_keeps_a_partial_emoji_presentation_sequence_joinable() {
    let mut engine = Engine::new(10, 3).unwrap();
    feed(&mut engine, "12345678❤");
    engine.resize(6, 3, 8, 16).unwrap();
    feed(&mut engine, "️!");
    assert_eq!(lines(&mut engine), ["123456", "78❤️!"]);
    assert_eq!(cell(&mut engine, 0, 4).text, "5");
}

#[test]
fn height_growth_above_bottom_preserves_full_history_selection_and_saved_cursor() {
    let mut engine = Engine::new(10, 3).unwrap();
    let mut input = (0..5004)
        .map(|line| format!("{line:06}\r\n"))
        .collect::<String>();
    input.push_str("\x1b[1;2H\x1b7");
    feed(&mut engine, input);
    engine
        .select_range(active(0, 0), active(5, 1), false)
        .unwrap();
    engine.scroll(ScrollViewport::Top);
    let selected = engine.selected_text().unwrap();
    // Ghostty prunes complete pages at its configured history limit. Test
    // that resizing preserves every retained row, independently of that
    // engine-specific page granularity.
    let retained_history = history(&mut engine);
    assert!(retained_history > 0);
    let before = retained_rows(&mut engine);
    let offset = engine.viewport_top().unwrap();
    let saved = engine.cursor_position().unwrap();
    engine.resize(10, 5, 8, 16).unwrap();
    assert_eq!(history(&mut engine), retained_history);
    assert_eq!(engine.viewport_top().unwrap(), offset);
    let cursor = engine.cursor_position().unwrap();
    assert_eq!((cursor.y, cursor.x), (0, 1));
    let after = retained_rows(&mut engine);
    for (actual, expected) in after.iter().zip(&before) {
        same_row(actual, expected);
    }
    assert!(
        after[before.len()..]
            .iter()
            .flat_map(|row| &row.cells)
            .all(|cell| cell.text.trim().is_empty())
    );
    assert_eq!(engine.selected_text().unwrap(), selected);
    feed(&mut engine, b"\x1b8");
    let restored = engine.cursor_position().unwrap();
    assert_eq!((restored.x, restored.y), (saved.x, saved.y));
}
