//! Reflow must preserve logical paragraphs, including spaces and output below
//! the cursor. Expected strings are independent of the xterm fixture snapshots.
use newport_terminal::{Engine, PointCoordinate};

fn feed(engine: &mut Engine, input: &str, chunk: usize) {
    for bytes in input.as_bytes().chunks(chunk) {
        engine.advance(bytes).unwrap();
    }
}

fn text(engine: &mut Engine) -> String {
    engine.select_all().unwrap();
    let text = engine.selected_text().unwrap().unwrap();
    engine.clear_selection().unwrap();
    text.trim_end_matches('\n').to_owned()
}

#[test]
fn width_changes_preserve_complete_paragraphs_and_append_at_their_end() {
    for chunk in [1, 2, 3, 1024] {
        for (input, expected, sizes, cursor) in [
            ("123456789abcdef", "123456789abcdef!", vec![14], (2, 1)),
            ("123456789abcdef", "123456789abcdef!", vec![6], (4, 2)),
            ("123456789abcdef", "123456789abcdef!", vec![14, 6], (4, 2)),
            ("123456789abcdef", "123456789abcdef!", vec![6, 14], (2, 1)),
            ("1234567890", "1234567890!", vec![6], (5, 1)),
            ("12345中x", "12345中x!", vec![6], (4, 1)),
            ("123456789🫩", "123456789🫩!", vec![6], (5, 1)),
            (
                "123456789abcdef\r\nprompt",
                "123456789abcdef\nprompt!",
                vec![6],
                (1, 2),
            ),
            (
                "123456789abcdef\r\nprompt",
                "123456789abcdef\nprompt!",
                vec![14, 6],
                (1, 2),
            ),
            (
                "long history entry\r\nmore history here\r\n123456789abcdef",
                "long history entry\nmore history here\n123456789abcdef!",
                vec![14],
                (2, 2),
            ),
            (
                "long history entry\r\nmore history here\r\n123456789abcdef",
                "long history entry\nmore history here\n123456789abcdef!",
                vec![6],
                (4, 2),
            ),
        ] {
            let mut engine = Engine::new(10, 3).unwrap();
            feed(&mut engine, input, chunk);
            let before = input.replace("\r\n", "\n");
            assert_eq!(text(&mut engine), before);
            for columns in sizes {
                engine.resize(columns, 3, 8, 16).unwrap();
                assert_eq!(text(&mut engine), before, "cols {columns}, chunk {chunk}");
            }
            feed(&mut engine, "!", chunk);
            assert_eq!(
                text(&mut engine),
                expected,
                "input {input:?}, chunk {chunk}"
            );
            assert_eq!(
                engine.cursor_position().unwrap(),
                PointCoordinate {
                    x: cursor.0,
                    y: cursor.1
                }
            );
        }
    }
}

#[test]
fn a_cursor_above_preserved_output_stays_on_its_original_text() {
    for chunk in [1, 2, 3, 1024] {
        for variant in ["current", "saved", "alternate", "extra-space"] {
            let mut engine = Engine::new(10, 3).unwrap();
            feed(&mut engine, "prompt\r\n123456789\x1b[1;2H", chunk);
            if variant == "saved" {
                feed(&mut engine, "\x1b7", chunk);
            }
            if variant == "alternate" {
                feed(&mut engine, "\x1b[?1049hALT", chunk);
            }
            let rows = if variant == "extra-space" { 5 } else { 3 };
            if variant == "extra-space" {
                engine.resize(10, rows, 8, 16).unwrap();
            }
            engine.resize(6, rows, 8, 16).unwrap();
            if variant == "alternate" {
                feed(&mut engine, "\x1b[?1049l", chunk);
            }
            if variant == "saved" {
                feed(&mut engine, "\x1b8", chunk);
            }
            assert_eq!(text(&mut engine), "prompt\n123456789");
            assert_eq!(
                engine.cursor_position().unwrap(),
                PointCoordinate { x: 1, y: 0 }
            );
            feed(&mut engine, "!", chunk);
            assert_eq!(
                text(&mut engine),
                "p!ompt\n123456789",
                "{variant}, chunk {chunk}"
            );
        }
    }
}

#[test]
fn cursor_reflows_with_its_cell_or_clamps_to_the_top_active_row() {
    for chunk in [1, 2, 3, 1024] {
        for (columns, expected, before_cursor) in [
            (
                14,
                "123456789ab!defghijklmnop",
                PointCoordinate { x: 11, y: 0 },
            ),
            // Five new rows require two history rows. The cursor's row falls
            // into history, so Screen.cursorReload clamps it to active row 0
            // while preserving column 5. Only the subsequent write replaces i.
            (
                6,
                "123456789abcdefgh!jklmnop",
                PointCoordinate { x: 5, y: 0 },
            ),
        ] {
            let mut engine = Engine::new(10, 3).unwrap();
            feed(&mut engine, "123456789abcdefghijklmnop\x1b[2;2H", chunk);
            engine.resize(columns, 3, 8, 16).unwrap();
            assert_eq!(text(&mut engine), "123456789abcdefghijklmnop");
            assert_eq!(engine.cursor_position().unwrap(), before_cursor);
            feed(&mut engine, "!", chunk);
            assert_eq!(text(&mut engine), expected);
        }
    }
}

#[test]
fn a_partial_cluster_survives_reflow_before_its_remaining_bytes_arrive() {
    for chunk in [1, 2, 3, 1024] {
        let mut engine = Engine::new(10, 3).unwrap();
        feed(&mut engine, "123456789ab👩‍", chunk);
        engine.resize(14, 3, 8, 16).unwrap();
        assert_eq!(text(&mut engine), "123456789ab👩‍");
        feed(&mut engine, "💻!", chunk);
        assert_eq!(text(&mut engine), "123456789ab👩‍💻!");
        assert_eq!(
            engine.cursor_position().unwrap(),
            PointCoordinate { x: 13, y: 0 }
        );
    }
}

#[test]
fn output_pushed_into_history_survives_until_a_subsequent_write() {
    for chunk in [1, 2, 3, 1024] {
        for (rows, expected) in [
            (3, "prompt\n1!3456789abcdef"),
            (2, "prompt\n1234567!9abcdef"),
        ] {
            let mut engine = Engine::new(10, 3).unwrap();
            feed(&mut engine, "prompt\r\n123456789abcdef\x1b[1;2H", chunk);
            engine.resize(6, rows, 8, 16).unwrap();
            assert_eq!(text(&mut engine), "prompt\n123456789abcdef");
            assert_eq!(
                engine.cursor_position().unwrap(),
                PointCoordinate { x: 1, y: 0 }
            );
            feed(&mut engine, "!", chunk);
            assert_eq!(text(&mut engine), expected);
        }
        for cursor_row in [1, 2] {
            let mut engine = Engine::new(10, 3).unwrap();
            feed(
                &mut engine,
                &format!("a\r\nb\r\nc\x1b[{cursor_row};2H"),
                chunk,
            );
            engine.resize(10, 2, 8, 16).unwrap();
            assert_eq!(text(&mut engine), "a\nb\nc");
            feed(&mut engine, "!", chunk);
            assert_eq!(text(&mut engine), "a\nb!\nc");
        }
    }
}
