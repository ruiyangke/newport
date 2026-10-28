//! Grapheme clusters and their cell widths. Fragmentation must never change
//! the resulting grid.
use newport_terminal::{CellWide, Engine, PointCoordinate};

const CHUNKS: [usize; 4] = [1, 2, 3, 1024];

fn feed(engine: &mut Engine, input: &str, chunk_size: usize) {
    for chunk in input.as_bytes().chunks(chunk_size) {
        engine.advance(chunk).unwrap();
    }
}

fn assert_row(engine: &mut Engine, row: usize, expected: &[(&str, CellWide)]) {
    let frame = engine.frame().unwrap();
    let actual = &frame.rows[row].cells;
    for (column, &(text, width)) in expected.iter().enumerate() {
        assert_eq!(actual[column].text, text, "row {row}, column {column}");
        assert_eq!(actual[column].wide, width, "row {row}, column {column}");
    }
    assert!(
        actual[expected.len()..]
            .iter()
            .all(|cell| cell.text.is_empty())
    );
}

fn assert_single_row(input: &str, expected: &[(&str, CellWide)], cursor_x: u16) {
    for chunk_size in CHUNKS {
        let mut engine = Engine::new(24, 3).unwrap();
        feed(&mut engine, input, chunk_size);
        assert_eq!(
            engine.cursor_position().unwrap(),
            PointCoordinate { x: cursor_x, y: 0 },
            "input {input:?}, chunk size {chunk_size}"
        );
        assert_row(&mut engine, 0, expected);
    }
}

#[test]
fn modern_emoji_use_the_pinned_unicode_wide_property() {
    // U+1F6DC, U+1FA89 and U+1FAE9 are East Asian Wide.
    for emoji in ["🛜", "🪉", "🫩"] {
        assert_single_row(
            &format!("a{emoji}!"),
            &[
                ("a", CellWide::Narrow),
                (emoji, CellWide::Wide),
                ("", CellWide::SpacerTail),
                ("!", CellWide::Narrow),
            ],
            4,
        );
    }
}

#[test]
fn newly_recognized_extend_marks_share_the_base_cell() {
    // Both marks have Grapheme_Cluster_Break=Extend.
    for mark in ["\u{897}", "\u{1e4ec}"] {
        for control in ["", "\x1b[31m"] {
            assert_single_row(
                &format!("a{control}{mark}!"),
                &[
                    (&format!("a{mark}"), CellWide::Narrow),
                    ("!", CellWide::Narrow),
                ],
                2,
            );
        }
    }
}

#[test]
fn regional_indicators_pair_into_flags_and_a_lone_indicator_stays_wide() {
    assert_single_row(
        "🇺🇸🇨🇦🇬!",
        &[
            ("🇺🇸", CellWide::Wide),
            ("", CellWide::SpacerTail),
            ("🇨🇦", CellWide::Wide),
            ("", CellWide::SpacerTail),
            ("🇬", CellWide::Wide),
            ("", CellWide::SpacerTail),
            ("!", CellWide::Narrow),
        ],
        7,
    );
}

#[test]
fn zwj_sequences_and_skin_tones_form_one_emoji() {
    for emoji in ["👩‍💻", "👩🏽", "👩🏽‍💻", "👨‍👩‍👧‍👦"] {
        assert_single_row(
            &format!("{emoji}!"),
            &[
                (emoji, CellWide::Wide),
                ("", CellWide::SpacerTail),
                ("!", CellWide::Narrow),
            ],
            3,
        );
    }
    // A joiner after a letter does not pull the next emoji into the letter's cell.
    assert_single_row(
        "a‍💻!",
        &[
            ("a‍", CellWide::Narrow),
            ("💻", CellWide::Wide),
            ("", CellWide::SpacerTail),
            ("!", CellWide::Narrow),
        ],
        4,
    );
}

#[test]
fn emoji_presentation_selector_widens_and_text_selector_keeps_width() {
    for input in ["😀\u{fe0e}", "🫩\u{fe0f}", "❤\u{fe0f}", "1\u{fe0f}\u{20e3}"] {
        assert_single_row(
            &format!("{input}!"),
            &[
                (input, CellWide::Wide),
                ("", CellWide::SpacerTail),
                ("!", CellWide::Narrow),
            ],
            3,
        );
    }
    assert_single_row(
        "❤\u{fe0e}!",
        &[("❤\u{fe0e}", CellWide::Narrow), ("!", CellWide::Narrow)],
        2,
    );
}

#[test]
fn indic_conjuncts_and_spacing_marks_share_a_two_cell_cluster() {
    for cluster in ["क्‍ष", "का", "क्षि"] {
        assert_single_row(
            &format!("{cluster}!"),
            &[
                (cluster, CellWide::Wide),
                ("", CellWide::SpacerTail),
                ("!", CellWide::Narrow),
            ],
            3,
        );
    }
}

#[test]
fn prepend_joins_the_following_base_and_takes_its_width() {
    for prefix in ["\u{600}", "\u{600}\u{601}"] {
        for control in ["", "\0", "\x1b[31m", "\x1b]2;title\x07"] {
            for base in ["a", "α", "界"] {
                let text = format!("{prefix}{base}");
                let mut cells = vec![(text.as_str(), CellWide::Narrow)];
                let cursor = if base == "界" {
                    cells[0].1 = CellWide::Wide;
                    cells.push(("", CellWide::SpacerTail));
                    3
                } else {
                    2
                };
                cells.push(("!", CellWide::Narrow));
                assert_single_row(&format!("{prefix}{control}{base}!"), &cells, cursor);
            }
        }
    }
    // Prepend joins only the character right after it, not one after a mark.
    assert_single_row(
        "\u{600}\u{601}\u{301}a!",
        &[
            ("\u{600}\u{601}\u{301}", CellWide::Narrow),
            ("a", CellWide::Narrow),
            ("!", CellWide::Narrow),
        ],
        3,
    );
}

#[test]
fn mode_2027_switches_between_clusters_and_per_character_widths() {
    let mut engine = Engine::new(24, 3).unwrap();
    let report = |engine: &mut Engine| engine.advance(b"\x1b[?2027$p").unwrap().replies.concat();
    assert_eq!(report(&mut engine), b"\x1b[?2027;1$y");
    feed(&mut engine, "\x1b[?2027l👩‍💻!", 1024);
    assert_eq!(report(&mut engine), b"\x1b[?2027;2$y");
    assert_eq!(engine.cursor_position().unwrap().x, 5);
    assert_row(
        &mut engine,
        0,
        &[
            ("👩‍", CellWide::Wide),
            ("", CellWide::SpacerTail),
            ("💻", CellWide::Wide),
            ("", CellWide::SpacerTail),
            ("!", CellWide::Narrow),
        ],
    );
    // A full reset turns clustering back on.
    feed(&mut engine, "\x1bc👩‍💻!", 1024);
    assert_eq!(engine.cursor_position().unwrap().x, 3);
}

#[test]
fn combining_marks_survive_fragmented_controls_and_synchronized_output() {
    for control in [
        "\0",
        "\x18",
        "\x1b[31m",
        "\x1b[999z",
        "\x1b]2;title\x07",
        "\x1b]999;ignored\x07",
        "\x1bPignored\x1b\\",
    ] {
        for (begin, end) in [("", ""), ("\x1b[?2026h", "\x1b[?2026l")] {
            assert_single_row(
                &format!("{begin}a{control}\u{301}!{end}"),
                &[("a\u{301}", CellWide::Narrow), ("!", CellWide::Narrow)],
                2,
            );
            assert_single_row(
                &format!("{begin}👩‍{control}💻!{end}"),
                &[
                    ("👩‍💻", CellWide::Wide),
                    ("", CellWide::SpacerTail),
                    ("!", CellWide::Narrow),
                ],
                3,
            );
        }
    }
}

#[test]
fn unanchored_marks_do_not_create_a_phantom_cell() {
    // A mark at column zero has no preceding cell. CR is a cursor movement,
    // unlike SGR, titles and NUL.
    assert_single_row("\u{301}!", &[("!", CellWide::Narrow)], 1);
    assert_single_row(
        "abc\r\u{301}!",
        &[
            ("!", CellWide::Narrow),
            ("b", CellWide::Narrow),
            ("c", CellWide::Narrow),
        ],
        1,
    );
}

#[test]
fn modern_wide_emoji_wraps_before_the_right_margin() {
    // The emoji needs two cells, so the final column is a spacer and the emoji
    // starts the next row.
    for chunk_size in CHUNKS {
        let mut engine = Engine::new(10, 3).unwrap();
        feed(&mut engine, "123456789🪉!", chunk_size);
        assert_eq!(
            engine.cursor_position().unwrap(),
            PointCoordinate { x: 3, y: 1 }
        );
        let frame = engine.frame().unwrap();
        assert_eq!(frame.visible_lines(), ["123456789", "🪉!"]);
        assert_eq!(frame.rows[0].cells[9].wide, CellWide::SpacerHead);
        assert!(frame.rows[0].wrapped);
        assert_row(
            &mut engine,
            1,
            &[
                ("🪉", CellWide::Wide),
                ("", CellWide::SpacerTail),
                ("!", CellWide::Narrow),
            ],
        );
    }
}
