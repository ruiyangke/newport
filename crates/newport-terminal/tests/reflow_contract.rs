//! Width reflow keeps paragraphs intact and the cursor on its own text.
use newport_terminal::{Engine, PointCoordinate};
fn text(engine: &mut Engine) -> String {
    engine.select_all().unwrap();
    let text = engine.selected_text().unwrap().unwrap();
    engine.clear_selection().unwrap();
    text.trim_end_matches('\n').to_owned()
}
#[test]
fn width_reflow_preserves_typical_shell_paragraphs_across_utf8_chunks() {
    for chunk in [1, 2, 3, 1024] {
        for input in [
            "123456789abcdef",
            "12345中x",
            "123456789🫩",
            "long history entry\r\nmore history here\r\n123456789abcdef",
        ] {
            for sizes in [[14, 6], [6, 14]] {
                let mut engine = Engine::new(10, 3).unwrap();
                for bytes in input.as_bytes().chunks(chunk) {
                    engine.advance(bytes).unwrap();
                }
                let expected = input.replace("\r\n", "\n");
                for columns in sizes {
                    engine.resize(columns, 3, 8, 16).unwrap();
                    assert_eq!(text(&mut engine), expected);
                }
                if input.is_ascii() {
                    engine.advance(b"!").unwrap();
                    assert_eq!(text(&mut engine), format!("{expected}!"));
                }
            }
        }
    }
}
#[test]
fn the_cursor_stays_on_its_text_when_output_below_it_reflows() {
    for (columns, rows) in [(6, 3), (6, 4), (4, 6), (14, 3)] {
        let mut engine = Engine::new(10, 3).unwrap();
        engine.advance(b"prompt\r\n123456789\x1b[1;2H").unwrap();
        engine.resize(columns, rows, 8, 16).unwrap();
        engine.advance(b"!").unwrap();
        assert_eq!(text(&mut engine), "p!ompt\n123456789", "{columns}x{rows}");
    }
}
#[test]
fn the_cursor_keeps_its_place_inside_a_wrapped_line() {
    let mut engine = Engine::new(10, 3).unwrap();
    engine
        .advance(b"0123456789abcdef\r\nnext\x1b[2;3H")
        .unwrap();
    engine.resize(4, 3, 8, 16).unwrap();
    engine.advance(b"!").unwrap();
    assert_eq!(text(&mut engine), "0123456789ab!def\nnext");
}
#[test]
fn a_combining_suffix_can_arrive_after_reflow() {
    let mut engine = Engine::new(10, 3).unwrap();
    engine.advance("123456789e".as_bytes()).unwrap();
    engine.resize(6, 3, 8, 16).unwrap();
    engine.advance("́!".as_bytes()).unwrap();
    assert_eq!(text(&mut engine), "123456789é!");
    // The reflowed first row stays on screen above the cursor.
    assert_eq!(
        engine.cursor_position().unwrap(),
        PointCoordinate { x: 5, y: 1 }
    );
}

#[test]
fn the_cursor_follows_a_wide_character_across_the_new_margin() {
    let mut engine = Engine::new(10, 3).unwrap();
    engine.advance("12345中x".as_bytes()).unwrap();
    engine.resize(14, 3, 8, 16).unwrap();
    engine.resize(6, 3, 8, 16).unwrap();
    assert_eq!(text(&mut engine), "12345中x");
    engine.advance(b"!").unwrap();
    assert_eq!(text(&mut engine), "12345中x!");
}
