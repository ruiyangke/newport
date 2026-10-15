//! Original xterm fixtures remain the compatibility oracle. Independently
//! asserted resize cases preserve output rather than copy xterm's data loss.
use newport_terminal::{
    Cell, CellWide, Engine, Point, PointCoordinate, Position, Row, ScrollViewport, StyleColor,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
    name: String,
    input: String,
    resize: Option<[u16; 2]>,
    #[serde(rename = "resizeAgain")]
    resize_again: Option<[u16; 2]>,
    after: Option<String>,
    cursor: [usize; 2],
    lines: Vec<String>,
    cells: Vec<Vec<(String, usize)>>,
}

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
fn grapheme_grid_preserves_reviewed_contracts_for_whole_and_fragmented_input() {
    assert_compatibility_grid(|_| true);
}

#[test]
fn resize_output_below_cursor_preserves_all_output() {
    for rows in [2, 3] {
        for chunk_size in [1, 2, 3, 1024] {
            let mut engine = Engine::new(10, 3).unwrap();
            for chunk in b"prompt\r\n123456789abcdef\x1b[1;2H".chunks(chunk_size) {
                feed(&mut engine, chunk);
            }
            engine.resize(6, rows, 8, 16).unwrap();
            assert_eq!(
                retained_text(&mut engine),
                "prompt123456789abcdef",
                "rows {rows}, chunk size {chunk_size}"
            );
            let cursor = engine.cursor_position().unwrap();
            assert!(cursor.y < u32::from(rows));
            assert!(cursor.x < 6);
        }
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
fn narrowing_preserves_nonempty_and_styled_tail_rows() {
    let mut engine = Engine::new(10, 3).unwrap();
    feed(&mut engine, b"prompt\r\n123456789abcdef\x1b[1;2H");
    engine.resize(6, 3, 8, 16).unwrap();
    assert_eq!(retained_text(&mut engine), "prompt123456789abcdef");

    let mut styled = Engine::new(10, 3).unwrap();
    feed(
        &mut styled,
        b"prompt\r\n123456789\x1b[3;1H\x1b[41m\x1b[2K\x1b[0m\x1b[1;2H",
    );
    let background = cell(&mut styled, 2, 0).background;
    assert!(background.is_some(), "fixture row has a background color");
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
    // Ghostty reflows the complete paragraph, preserving the suffix that
    // the old cursor-paragraph special case clipped away.
    assert_eq!(lines(&mut engine), ["7890", "abcdef", "ghij"]);
    assert_eq!(retained_text(&mut engine), "1234567890abcdefghij");
    let frame = engine.frame().unwrap();
    assert!(frame.rows[1].wrapped);
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

fn assert_compatibility_grid(include: impl Fn(&str) -> bool) {
    let cases: Vec<Fixture> =
        serde_json::from_str(include_str!("fixtures/terminal-graphemes.json")).unwrap();
    let mut failures = Vec::new();
    for mut case in cases {
        if !include(&case.name) {
            continue;
        }
        apply_reviewed_rep_expectation(&mut case);
        apply_reviewed_unicode_expectation(&mut case);
        apply_reviewed_resize_expectation(&mut case);
        for chunk_size in [1, 2, 3, 1024] {
            let mut engine = Engine::new(10, 3).unwrap();
            for chunk in case.input.as_bytes().chunks(chunk_size) {
                feed(&mut engine, chunk);
            }
            let [cols, rows] = case.resize_again.or(case.resize).unwrap_or([10, 3]);
            if let Some([cols, rows]) = case.resize {
                engine.resize(cols, rows, 8, 16).unwrap();
                if let Some([cols, rows]) = case.resize_again {
                    engine.resize(cols, rows, 8, 16).unwrap();
                }
                for chunk in case.after.as_ref().unwrap().as_bytes().chunks(chunk_size) {
                    feed(&mut engine, chunk);
                }
            }
            let cursor = engine.cursor_position().unwrap();
            let actual_cursor = [cursor.y as usize, usize::from(cursor.x)];
            // Ghostty reports the last cell while wrap is pending, whereas
            // xterm's fixture reports the position just beyond the right edge.
            let expected_cursor = [case.cursor[0], case.cursor[1].min(usize::from(cols - 1))];
            let mut lines = lines(&mut engine);
            lines.resize(rows as usize, String::new());
            let cells: Vec<Vec<_>> = engine
                .frame()
                .unwrap()
                .rows
                .iter()
                .map(|row| {
                    row.cells
                        .iter()
                        .map(|cell| {
                            let width = match cell.wide {
                                CellWide::SpacerHead | CellWide::SpacerTail => 0,
                                CellWide::Wide => 2,
                                CellWide::Narrow => 1,
                            };
                            (
                                if width == 0 {
                                    String::new()
                                } else {
                                    cell.text.trim_end_matches(' ').to_owned()
                                },
                                width,
                            )
                        })
                        .collect()
                })
                .collect();
            let same_cells = cells.len() == case.cells.len()
                && cells
                    .iter()
                    .zip(&case.cells)
                    .all(|(a, b)| a.len() == b.len())
                && cells
                    .iter()
                    .flatten()
                    .zip(case.cells.iter().flatten())
                    .all(|(a, b)| {
                        a.0 == b.0.trim_end_matches(' ')
                            && (a.1 == b.1 || (a.0.is_empty() && a.1 == 0 && b.1 == 1))
                    });
            if actual_cursor != expected_cursor || lines != case.lines || !same_cells {
                failures.push(format!("{} chunk {chunk_size}: cursor {actual_cursor:?} expected {expected_cursor:?}; lines {lines:?} expected {:?}; cells match {same_cells}", case.name, case.lines));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn reflow_preserves_partial_graphemes_and_clipping_never_changes_a_neighbor() {
    let mut grow = Engine::new(10, 3).unwrap();
    feed(&mut grow, "123456789ab👩‍");
    grow.resize(14, 3, 8, 16).unwrap();
    feed(&mut grow, "💻!");
    assert_eq!(lines(&mut grow), ["123456789ab👩‍💻!"]);
    assert_eq!(cell(&mut grow, 0, 11).text, "👩‍💻");

    let mut shrink = Engine::new(10, 3).unwrap();
    feed(&mut shrink, "12345678❤");
    shrink.resize(6, 3, 8, 16).unwrap();
    feed(&mut shrink, "️!");
    assert_eq!(lines(&mut shrink), ["123456", "78❤️!"]);
    assert_eq!(cell(&mut shrink, 0, 4).text, "5");

    let mut clipped = Engine::new(10, 3).unwrap();
    feed(&mut clipped, "\x1b[?1049h❤❤❤❤❤❤❤❤❤");
    clipped.resize(6, 3, 8, 16).unwrap();
    feed(&mut clipped, "️!");
    assert_eq!(cell(&mut clipped, 0, 4).text, "❤");
    assert_eq!(cell(&mut clipped, 0, 4).wide, CellWide::Narrow);
}

#[test]
fn clipped_grapheme_does_not_modify_previous_text() {
    let mut engine = Engine::new(10, 3).unwrap();
    for byte in "\x1b[?7l123456789👩‍💻!".as_bytes() {
        feed(&mut engine, [*byte]);
    }
    assert_eq!(lines(&mut engine), ["123456789!"]);
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
    // Ghostty's current Unicode data classifies both emoji as two cells.
    // Newport no longer overrides these widths with the old xterm table.
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
    // Ghostty intentionally caps a cell at 64 suffix codepoints. Retain that
    // memory/CPU bound rather than Newport's previous unbounded cluster data.
    // Excess marks must not escape into adjacent cells or block later output.
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

// These exact fixture alternatives are independently asserted, including cell
// widths, cursor positions and byte fragmentation, in newport-terminal's
// tests/unicode_contract.rs. They preserve the archived inputs and full grids.
// Prepend is deliberately absent: patch 005 fixes the engine so it satisfies
// the original expectation without an override.
type UnicodeRows = ([usize; 2], Vec<Vec<(&'static str, usize)>>);
fn reviewed_unicode_rows(name: &str) -> Option<UnicodeRows> {
    let (cursor, rows) = match name {
        // clipped_grapheme_does_not_modify_previous_text and core
        // clipped_grapheme tests: a rejected cluster cannot edit column 8.
        "no-wrap-wide" => (
            [0, 9],
            vec![vec![
                ("1", 1),
                ("2", 1),
                ("3", 1),
                ("4", 1),
                ("5", 1),
                ("6", 1),
                ("7", 1),
                ("8", 1),
                ("9", 1),
                ("!", 1),
            ]],
        ),
        // modern_emoji_use_the_pinned_unicode_wide_property
        "unicode-provider-wifi" => ([0, 4], vec![vec![("a", 1), ("🛜", 2), ("!", 1)]]),
        "unicode-provider-harp" => ([0, 4], vec![vec![("a", 1), ("🪉", 2), ("!", 1)]]),
        "unicode-provider-tired-face" => ([0, 4], vec![vec![("a", 1), ("🫩", 2), ("!", 1)]]),
        // newly_recognized_extend_marks_share_the_base_cell
        "unicode-provider-new-mark" | "unicode-provider-mark-after-control" => {
            ([0, 2], vec![vec![("a\u{897}", 1), ("!", 1)]])
        }
        "unicode-provider-nag-mundari" => ([0, 2], vec![vec![("a\u{1e4ec}", 1), ("!", 1)]]),
        // indic_clusters_follow_ghosttys_shared_grapheme_width_policy
        "unicode-provider-indic" => ([0, 3], vec![vec![("क्‍ष", 2), ("!", 1)]]),
        "spacing-mark" => ([0, 3], vec![vec![("का", 2), ("!", 1)]]),
        // zwj_requires_an_extended_pictographic_base_to_join_an_emoji
        "unicode-provider-zwj" => ([0, 4], vec![vec![("a‍", 1), ("💻", 2), ("!", 1)]]),
        // selectors_only_change_valid_emoji_variation_bases
        "text-selector" => ([0, 3], vec![vec![("😀", 2), ("!", 1)]]),
        "unicode-provider-promoted-new-character" => ([0, 3], vec![vec![("🫩", 2), ("!", 1)]]),
        "unicode-provider-isolated-selector" => ([0, 1], vec![vec![("!", 1)]]),
        // regional_indicators_pair_without_collapsing_a_standalone_indicator
        "flags" => ([0, 7], vec![vec![("🇺🇸", 2), ("🇨🇦", 2), ("🇬", 2), ("!", 1)]]),
        // nonmoving_controls_do_not_break_a_partially_received_grapheme
        "unknown-osc"
        | "unknown-dcs"
        | "cancel-control"
        | "unknown-control"
        | "sync-ignored-control"
        | "sync-title" => ([0, 3], vec![vec![("👩‍💻", 2), ("!", 1)]]),
        "null-boundary" | "escape-title" | "control-boundary" | "sync-null" => {
            ([0, 2], vec![vec![("a\u{301}", 1), ("!", 1)]])
        }
        // unanchored_marks_do_not_create_a_phantom_cell
        "isolated" => ([0, 1], vec![vec![("!", 1)]]),
        "combining-cr" => ([0, 1], vec![vec![("!", 1), ("b", 1), ("c", 1)]]),
        // modern_wide_emoji_wraps_before_the_right_margin
        "unicode-provider-right-margin" => (
            [1, 3],
            vec![
                vec![
                    ("1", 1),
                    ("2", 1),
                    ("3", 1),
                    ("4", 1),
                    ("5", 1),
                    ("6", 1),
                    ("7", 1),
                    ("8", 1),
                    ("9", 1),
                ],
                vec![("🪉", 2), ("!", 1)],
            ],
        ),
        _ => return None,
    };
    Some((cursor, rows))
}

fn apply_reviewed_unicode_expectation(case: &mut Fixture) {
    let Some((cursor, rows)) = reviewed_unicode_rows(&case.name) else {
        return;
    };
    assert!(case.resize.is_none() && case.resize_again.is_none());
    case.cursor = cursor;
    case.lines = vec![String::new(); 3];
    case.cells = vec![vec![(String::new(), 1); 10]; 3];
    for (y, row) in rows.into_iter().enumerate() {
        let mut x = 0;
        for (text, width) in row {
            case.lines[y].push_str(text);
            case.cells[y][x] = (text.into(), width);
            if width == 2 {
                case.cells[y][x + 1] = (String::new(), 0);
            }
            x += width;
        }
    }
}

// These rows follow complete-paragraph reflow, not xterm's cursor-paragraph
// clipping. The independent reflow_contract tests check every retained logical
// paragraph (including spaces) before new input, then check the resulting text
// and cursor after input. Thus visible-grid parity cannot hide lost history.
type ResizeRows = ([usize; 2], Vec<Vec<(String, usize)>>);
fn reviewed_resize_rows(name: &str) -> Option<ResizeRows> {
    let ascii = |rows: &[&str]| -> Vec<Vec<(String, usize)>> {
        rows.iter()
            .map(|row| {
                assert!(row.is_ascii());
                row.chars().map(|c| (c.to_string(), 1)).collect()
            })
            .collect()
    };
    match name {
        "unicode-provider-resized-new-character" => {
            let mut rows = ascii(&["123456", "789"]);
            rows[1].extend([("🫩".into(), 2), ("!".into(), 1)]);
            return Some(([1, 5], rows));
        }
        "resize-wide-clipped" => {
            let mut rows = ascii(&["12345", ""]);
            rows[1].extend([("中".into(), 2), ("x".into(), 1), ("!".into(), 1)]);
            return Some(([1, 4], rows));
        }
        "resize-reflow-grow" => {
            let mut rows = ascii(&["123456789ab"]);
            rows[0].extend([("👩‍💻".into(), 2), ("!".into(), 1)]);
            return Some(([0, 13], rows));
        }
        "resize-reflow-shrink" => {
            let mut rows = ascii(&["123456", "78"]);
            rows[1].extend([("❤️".into(), 2), ("!".into(), 1)]);
            return Some(([1, 5], rows));
        }
        _ => {}
    }
    let (cursor, rows): (_, &[&str]) = match name {
        "resize-prompt-grow" | "resize-shrink-grow-prompt" => ([1, 2], &["123456789abcde", "f!"]),
        "resize-prompt-shrink" | "resize-grow-shrink-prompt" => {
            ([2, 4], &["123456", "789abc", "def!"])
        }
        "resize-history-shrink" | "resize-grow-shrink-history" => ([2, 1], &["def", "prompt", "!"]),
        "resize-middle-grow" => ([0, 12], &["123456789ab!de", "fghijklmnop"]),
        // The original cursor row reflows into history. Clamp to active row 0,
        // retaining its column; the subsequent '!' replaces i, not old c.
        "resize-middle-shrink" => ([0, 5], &["defgh!", "jklmno", "p"]),
        "resize-margin-shrink" => ([1, 5], &["123456", "7890!"]),
        "resize-bottom-output-shrink" => ([0, 2], &["1!3456", "789abc", "def"]),
        "resize-shrink-both-top" => ([0, 2], &["7!9abc", "def"]),
        "resize-shrink-height-top" | "resize-shrink-height-middle" => ([0, 2], &["b!", "c"]),
        "resize-scrolled-grow" => ([2, 2], &["ere", "123456789abcde", "f!"]),
        "resize-scrolled-shrink" => ([2, 4], &["123456", "789abc", "def!"]),
        "resize-short-output-below"
        | "resize-short-output-saved"
        | "resize-short-output-alt"
        | "resize-short-output-extra-space" => ([0, 2], &["p!ompt", "123456", "789"]),
        _ => return None,
    };
    Some((cursor, ascii(rows)))
}

fn apply_reviewed_resize_expectation(case: &mut Fixture) {
    let Some((cursor, rows)) = reviewed_resize_rows(&case.name) else {
        return;
    };
    assert!(case.resize.is_some());
    let [columns, height] = case.resize_again.or(case.resize).unwrap();
    case.cursor = cursor;
    case.lines = vec![String::new(); usize::from(height)];
    case.cells = vec![vec![(String::new(), 1); usize::from(columns)]; usize::from(height)];
    for (y, row) in rows.into_iter().enumerate() {
        let mut x = 0;
        for (text, width) in row {
            case.lines[y].push_str(&text);
            case.cells[y][x] = (text, width);
            if width == 2 {
                case.cells[y][x + 1] = (String::new(), 0);
            }
            x += width;
        }
    }
}

#[test]
fn resize_fixtures_preserve_complete_paragraphs_and_reviewed_cursor_positions() {
    let cases: Vec<Fixture> =
        serde_json::from_str(include_str!("fixtures/terminal-graphemes.json")).unwrap();
    assert_eq!(
        cases
            .iter()
            .filter(|case| reviewed_resize_rows(&case.name).is_some())
            .count(),
        23
    );
    assert_compatibility_grid(|name| reviewed_resize_rows(name).is_some());
}

#[test]
fn unicode_fixtures_use_independently_reviewed_cell_contracts() {
    let cases: Vec<Fixture> =
        serde_json::from_str(include_str!("fixtures/terminal-graphemes.json")).unwrap();
    assert_eq!(
        cases
            .iter()
            .filter(|case| reviewed_unicode_rows(&case.name).is_some())
            .count(),
        27,
        "review each added or removed Unicode exception explicitly"
    );
    assert_compatibility_grid(|name| reviewed_unicode_rows(name).is_some());
}

// REP uses Terminal.previous_char (a scalar), not the previous cell's complete
// grapheme. Terminal.printRepeat prints that base again; appending a combining
// mark/ZWJ suffix does not replace previous_char. These explicit expectations
// retain the original xterm fixture input and keep checking every cell/cursor.
type RepRows = ([usize; 2], Vec<Vec<(&'static str, usize)>>);
fn reviewed_rep_rows(name: &str) -> Option<RepRows> {
    let (cursor, rows) = match name {
        // U+1FAE9 is wide in Ghostty's pinned Unicode table; REP retains its
        // base codepoint and therefore advances two cells for each copy.
        "unicode-provider-repeated-new-character" => (
            [0, 9],
            vec![vec![("🫩", 2), ("🫩", 2), ("🫩", 2), ("🫩", 2), ("!", 1)]],
        ),
        "resize-repeat" | "repeat-zwj" | "sync-repeat" => (
            [0, 7],
            vec![vec![("👩‍💻", 2), ("👩", 2), ("👩", 2), ("!", 1)]],
        ),
        // Repeated regional indicators undergo normal segmentation: two
        // repeated U indicators form one flag cluster, not two US flags.
        "repeat-flag" => ([0, 5], vec![vec![("🇺🇸", 2), ("🇺🇺", 2), ("!", 1)]]),
        "repeat-regional" => ([0, 5], vec![vec![("🇺🇺", 2), ("🇺", 2), ("!", 1)]]),
        "repeat-combining" => (
            [0, 5],
            vec![vec![("é", 1), ("e", 1), ("e", 1), ("e", 1), ("!", 1)]],
        ),
        // Neither VS16 nor its width promotion is repeated with the base.
        "repeat-default" | "repeat-zero" => ([0, 4], vec![vec![("❤️", 2), ("❤", 1), ("!", 1)]]),
        "repeat-after-control" | "del-repeat" => {
            ([0, 5], vec![vec![("❤️", 2), ("❤", 1), ("❤", 1), ("!", 1)]])
        }
        "repeat-after-repeat" => (
            [0, 7],
            vec![vec![("🇺🇸", 2), ("🇺🇺", 2), ("🇺🇺", 2), ("!", 1)]],
        ),
        // Sixteen wide cells occupy four rows at five cells per row; the
        // original family and first four repeated bases scroll into history.
        "repeat-wrap" => (
            [2, 3],
            vec![
                vec![("👨", 2); 5],
                vec![("👨", 2); 5],
                vec![("👨", 2), ("!", 1)],
            ],
        ),
        _ => return None,
    };
    Some((cursor, rows))
}

fn apply_reviewed_rep_expectation(case: &mut Fixture) {
    let Some((cursor, rows)) = reviewed_rep_rows(&case.name) else {
        return;
    };
    let [columns, height] = case.resize_again.or(case.resize).unwrap_or([10, 3]);
    case.cursor = cursor;
    case.lines = vec![String::new(); usize::from(height)];
    case.cells = vec![vec![(String::new(), 1); usize::from(columns)]; usize::from(height)];
    for (y, row) in rows.into_iter().enumerate() {
        let mut x = 0;
        for (text, width) in row {
            case.lines[y].push_str(text);
            case.cells[y][x] = (text.into(), width);
            if width == 2 {
                case.cells[y][x + 1] = (String::new(), 0);
            }
            x += width;
        }
    }
}

#[test]
fn rep_fixtures_use_reviewed_base_codepoint_semantics() {
    let cases: Vec<Fixture> =
        serde_json::from_str(include_str!("fixtures/terminal-graphemes.json")).unwrap();
    assert_eq!(
        cases
            .iter()
            .filter(|case| reviewed_rep_rows(&case.name).is_some())
            .count(),
        13,
        "review each added or removed REP exception explicitly"
    );
    assert_compatibility_grid(|name| reviewed_rep_rows(name).is_some());
}

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
