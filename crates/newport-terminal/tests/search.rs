use newport_terminal::{Engine, MAX_MATCHES, Point, PointCoordinate, Position, ScrollViewport};
fn make_engine(columns: u16, rows: u16, text: &str) -> Engine {
    let mut engine = Engine::new(columns, rows).unwrap();
    engine.advance(text.as_bytes()).unwrap();
    engine
}
fn point(row: u32, column: u16) -> Position {
    Position { row, column }
}
#[test]
fn literal_and_case_rules() {
    let mut engine = make_engine(80, 3, "Alpha ALPHA alpha a.*[x] ÉTÉ été");
    assert_eq!(engine.search("alpha", false).unwrap().matches.len(), 3);
    assert_eq!(engine.search("ALPHA", false).unwrap().matches.len(), 3);
    assert_eq!(engine.search("Alpha", true).unwrap().matches.len(), 1);
    assert_eq!(engine.search("a.*[x]", false).unwrap().matches.len(), 1);
    assert_eq!(engine.search("été", false).unwrap().matches.len(), 2);
    assert!(engine.search("", false).unwrap().matches.is_empty());
    let mut greek = make_engine(20, 2, "ΟΣ");
    assert_eq!(greek.search("ΟΣ", false).unwrap().matches.len(), 1);
}
#[test]
fn wraps_join_but_hard_line_endings_do_not() {
    let mut wrapped = make_engine(5, 4, "abcdefgh");
    assert_eq!(
        wrapped.search("defg", true).unwrap().matches,
        vec![point(0, 3)..=point(1, 1)]
    );
    let mut hard = make_engine(5, 4, "abcde\r\nfgh");
    assert!(hard.search("defg", true).unwrap().matches.is_empty());
}
#[test]
fn includes_scrollback_without_moving_viewport() {
    let mut engine = make_engine(20, 2, "old needle\r\nline two\r\nline three");
    let before = engine.frame().unwrap().viewport_top;
    let results = engine.search("needle", true).unwrap();
    assert_eq!(results.matches, vec![point(0, 4)..=point(0, 9)]);
    assert_eq!(engine.frame().unwrap().viewport_top, before);
}
#[test]
fn wide_and_combining_text_maps_back_to_owning_cells() {
    let mut engine = make_engine(20, 2, "A界e\u{301}🙂Z");
    assert_eq!(
        engine.search("界e\u{301}🙂", true).unwrap().matches,
        vec![point(0, 1)..=point(0, 5)]
    );
    assert_eq!(
        engine.search("e\u{301}", true).unwrap().matches,
        vec![point(0, 3)..=point(0, 3)]
    );
    assert!(
        engine.search("é", true).unwrap().matches.is_empty(),
        "no implicit canonical normalization"
    );
}
#[test]
fn wide_character_wrapping_skips_the_leading_spacer() {
    let mut engine = make_engine(4, 3, "abc界z");
    assert_eq!(
        engine.search("c界z", true).unwrap().matches,
        vec![point(0, 2)..=point(1, 2)]
    );
}
#[test]
fn concealed_cells_are_boundaries_not_removed_text() {
    let mut engine = make_engine(40, 2, "left\x1b[8msecret\x1b[28mright");
    assert!(engine.search("secret", false).unwrap().matches.is_empty());
    assert!(
        engine
            .search("leftright", false)
            .unwrap()
            .matches
            .is_empty()
    );
    assert_eq!(engine.search("right", false).unwrap().matches.len(), 1);
}
#[test]
fn exact_cap_is_not_reported_as_truncated() {
    let mut exact = make_engine(100, 102, &"x".repeat(MAX_MATCHES));
    let results = exact.search("x", true).unwrap();
    assert_eq!(results.matches.len(), MAX_MATCHES);
    assert!(!results.truncated);
    let mut extra = make_engine(100, 102, &"x".repeat(MAX_MATCHES + 1));
    let results = extra.search("x", true).unwrap();
    assert_eq!(results.matches.len(), MAX_MATCHES);
    assert!(results.truncated);
    assert!(
        results
            .matches
            .windows(2)
            .all(|m| m[0].end() < m[1].start())
    );
}
#[test]
fn prefix_fallback_and_nonoverlap_are_correct() {
    let mut engine = make_engine(40, 2, "abababac banana");
    assert_eq!(
        engine.search("ababac", true).unwrap().matches,
        vec![point(0, 2)..=point(0, 7)]
    );
    assert_eq!(
        engine.search("ana", true).unwrap().matches,
        vec![point(0, 10)..=point(0, 12)]
    );
}

#[test]
fn search_does_not_consume_pending_paint_or_selection() {
    let mut engine = make_engine(20, 2, "first");
    engine.frame().unwrap();
    engine.advance(b"\rsecond").unwrap();
    engine
        .select_range(
            Point::Active(PointCoordinate { x: 0, y: 0 }),
            Point::Active(PointCoordinate { x: 5, y: 0 }),
            false,
        )
        .unwrap();
    assert_eq!(engine.search("second", true).unwrap().matches.len(), 1);
    assert_eq!(engine.selected_text().unwrap().as_deref(), Some("second"));
    let frame = engine.frame().unwrap();
    assert_eq!(frame.visible_lines(), ["second"]);
    assert!(frame.rows[0].cells[..6].iter().all(|cell| cell.selected));
    drop(frame);
    engine.advance(b"\r\nthird\r\nfourth").unwrap();
    engine.scroll(ScrollViewport::Top);
    let before = engine.frame().unwrap().viewport_top;
    assert_eq!(engine.search("fourth", true).unwrap().matches.len(), 1);
    assert_eq!(engine.frame().unwrap().viewport_top, before);
    engine.advance(b"\rFIFTH").unwrap();
    engine.scroll(ScrollViewport::Bottom);
    assert_eq!(engine.visible_lines().unwrap(), ["third", "FIFTHh"]);
}

#[test]
fn search_preserves_synchronized_output_snapshot() {
    let mut engine = make_engine(20, 2, "before");
    engine.advance(b"\x1b[?2026h\rafter!").unwrap();
    assert_eq!(engine.search("after", true).unwrap().matches.len(), 1);
    assert_eq!(engine.visible_lines().unwrap(), ["before"]);
    engine.advance(b"\x1b[?2026l").unwrap();
    assert_eq!(engine.visible_lines().unwrap(), ["after!"]);
}
