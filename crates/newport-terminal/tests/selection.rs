use newport_terminal::{Engine, ScrollViewport, SelectionPosition};
fn position(column: f64, row: f64) -> SelectionPosition {
    SelectionPosition { column, row }
}
fn engine() -> Engine {
    let mut engine = Engine::new(20, 4).unwrap();
    engine
        .advance(b"alpha beta gamma\r\none two three\r\nlast line")
        .unwrap();
    engine
}
fn selected(engine: &Engine) -> String {
    engine.selected_text().unwrap().unwrap_or_default()
}

#[test]
fn click_is_empty_and_drag_uses_cell_edges() {
    let mut engine = engine();
    engine
        .begin_selection(position(0.1, 0.5), 1, false, false)
        .unwrap();
    assert_eq!(selected(&engine), "");
    engine.update_selection(position(4.9, 0.5)).unwrap();
    assert_eq!(selected(&engine), "alpha");
    engine.end_selection(Some(position(4.9, 0.5))).unwrap();
    engine
        .begin_selection(position(9.9, 0.5), 1, true, false)
        .unwrap();
    assert_eq!(selected(&engine), "alpha beta");
    engine
        .begin_selection(position(9.9, 0.5), 1, false, false)
        .unwrap();
    engine.update_selection(position(6.1, 0.5)).unwrap();
    assert_eq!(selected(&engine), "beta");
}

#[test]
fn word_and_line_drag_expand_by_semantic_unit() {
    let mut engine = engine();
    engine
        .begin_selection(position(2.5, 0.5), 2, false, false)
        .unwrap();
    assert_eq!(selected(&engine), "alpha");
    engine.update_selection(position(8.5, 0.5)).unwrap();
    assert_eq!(selected(&engine), "alpha beta");
    engine
        .begin_selection(position(2.5, 0.5), 3, false, false)
        .unwrap();
    assert_eq!(selected(&engine), "alpha beta gamma\n");
    engine.update_selection(position(5.5, 1.5)).unwrap();
    assert_eq!(selected(&engine), "alpha beta gamma\none two three\n");
}

#[test]
fn rectangular_selection_is_not_joined_across_rows() {
    let mut engine = engine();
    engine
        .begin_selection(position(0.1, 0.5), 1, false, true)
        .unwrap();
    engine.update_selection(position(2.9, 1.5)).unwrap();
    assert_eq!(selected(&engine), "alp\none");
}

#[test]
fn anchors_follow_output_and_screen_switches_invalidate_them() {
    let mut engine = engine();
    engine
        .begin_selection(position(0.1, 0.5), 1, false, false)
        .unwrap();
    engine.advance(b"\r\nnext\r\nnew").unwrap();
    engine.scroll(ScrollViewport::Top);
    engine.update_selection(position(4.9, 0.5)).unwrap();
    assert_eq!(selected(&engine), "alpha");
    engine.advance(b"\x1b[?1049h\x1b[Halt").unwrap();
    engine.update_selection(position(2.9, 0.5)).unwrap();
    assert_eq!(selected(&engine), "");
    engine
        .begin_selection(position(0.1, 0.5), 1, true, false)
        .unwrap();
    engine.update_selection(position(2.9, 0.5)).unwrap();
    assert_eq!(selected(&engine), "alt");
}

#[test]
fn clear_and_reconnect_invalidate_selection_anchor() {
    let mut engine = engine();
    for reconnect in [false, true] {
        engine
            .begin_selection(position(0.1, 0.5), 1, false, false)
            .unwrap();
        engine.update_selection(position(4.9, 0.5)).unwrap();
        if reconnect {
            engine.reconnect().unwrap();
        } else {
            engine.clear_selection().unwrap();
        }
        engine
            .begin_selection(position(6.1, 0.5), 1, true, false)
            .unwrap();
        assert_eq!(selected(&engine), "");
        engine.update_selection(position(9.9, 0.5)).unwrap();
        assert_eq!(selected(&engine), "beta");
    }
}

#[test]
fn wide_graphemes_are_selected_whole() {
    let mut engine = Engine::new(20, 3).unwrap();
    engine.advance("A界e\u{301}🙂Z".as_bytes()).unwrap();
    engine
        .begin_selection(position(1.1, 0.5), 1, false, false)
        .unwrap();
    engine.update_selection(position(5.9, 0.5)).unwrap();
    assert_eq!(selected(&engine), "界e\u{301}🙂");
}
