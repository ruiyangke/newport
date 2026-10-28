//! Height and width changes keep output, styles, selection and the saved cursor together.
use newport_terminal::{Cell, CellWide, Engine, Row, ScrollViewport};

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
    // Alacritty reflows the complete paragraph, preserving its suffix.
    assert_eq!(lines(&mut engine), ["7890", "abcdef", "ghij"]);
    assert_eq!(retained_text(&mut engine), "1234567890abcdefghij");
    let frame = engine.frame().unwrap();
    assert!(frame.rows[1].wrapped);
}

#[test]
fn height_growth_pulls_history_into_the_screen() {
    let mut engine = Engine::new(10, 3).unwrap();
    for i in 0..5004 {
        engine.advance(format!("{i:06}\r\n").as_bytes()).unwrap();
    }
    engine.scroll(ScrollViewport::Top);
    let before = retained_text(&mut engine);
    let history_before = history(&mut engine);
    engine.resize(10, 5, 8, 16).unwrap();
    assert_eq!(history(&mut engine), history_before - 2);
    assert_eq!(retained_text(&mut engine), before);
}
#[test]
fn height_shrink_moves_output_into_history_instead_of_dropping_it() {
    for cursor in 1..=3 {
        let mut engine = Engine::new(10, 3).unwrap();
        engine
            .advance(format!("a\r\nb\r\nc\x1b[{cursor};1H").as_bytes())
            .unwrap();
        engine.resize(10, 2, 8, 16).unwrap();
        assert_eq!(history(&mut engine), 1, "cursor on row {cursor}");
        assert_eq!(lines(&mut engine), ["b", "c"], "cursor on row {cursor}");
    }
    // Blank rows below the output are dropped first.
    let mut engine = Engine::new(10, 4).unwrap();
    engine.advance(b"a\r\nb\x1b[1;1H").unwrap();
    engine.resize(10, 2, 8, 16).unwrap();
    assert_eq!(history(&mut engine), 0);
    assert_eq!(lines(&mut engine), ["a", "b"]);
}
