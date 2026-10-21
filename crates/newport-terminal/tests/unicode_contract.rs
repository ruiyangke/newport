//! Grapheme segmentation and cell-width contracts. Expectations come from
//! pinned Ghostty's UUCODE tables and `src/unicode/grapheme.zig`, not from
//! snapshots of the engine under test.
//!
//! Cell width is a terminal policy in addition to Unicode segmentation. The
//! Prepend contract additionally covers Newport's upstream patch for GB9b.
use newport_terminal::{CellWide, Engine, PaletteIndex, PointCoordinate, StyleColor};

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
    // Original cases: unicode-provider-wifi, unicode-provider-harp,
    // unicode-provider-tired-face. U+1F6DC, U+1FA89, U+1FAE9 are EAW=W.
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
    // Original cases: unicode-provider-new-mark, unicode-provider-nag-mundari,
    // unicode-provider-mark-after-control. Both marks have GCB=Extend.
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
fn regional_indicators_pair_without_collapsing_a_standalone_indicator() {
    // Original case: flags. GB12/13 pair regional indicators; Ghostty uses
    // a two-cell emoji presentation even for the final unpaired indicator.
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
fn zwj_requires_an_extended_pictographic_base_to_join_an_emoji() {
    // Original case: unicode-provider-zwj. GB11 does not join a + ZWJ + laptop.
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
    assert_single_row(
        "👩‍💻!",
        &[
            ("👩‍💻", CellWide::Wide),
            ("", CellWide::SpacerTail),
            ("!", CellWide::Narrow),
        ],
        3,
    );
}

#[test]
fn selectors_only_change_valid_emoji_variation_bases() {
    // Original cases: text-selector, unicode-provider-promoted-new-character,
    // unicode-provider-isolated-selector. The pinned variation-sequences table
    // contains U+2764, but neither U+1F600 nor U+1FAE9. Invalid VS are discarded.
    for (input, text) in [
        ("😀\u{fe0e}!", "😀"),
        ("🫩\u{fe0f}!", "🫩"),
        ("❤\u{fe0f}!", "❤️"),
    ] {
        assert_single_row(
            input,
            &[
                (text, CellWide::Wide),
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
    assert_single_row("\u{fe0f}!", &[("!", CellWide::Narrow)], 1);
}

#[test]
fn indic_clusters_follow_ghosttys_shared_grapheme_width_policy() {
    // Original cases: unicode-provider-indic, spacing-mark. These remain single
    // clusters, but the shared width kernel promotes the cluster to width 2
    // when a nonzero-width continuation is appended (consonant/SpacingMark).
    for cluster in ["क्‍ष", "का"] {
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
fn prepend_clusters_adopt_the_base_width_across_fragments_and_controls() {
    // Original case: prepend. GB9b joins the prefix to its following base.
    // Prepend adds no cell beyond that base, including multiple prefixes.
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
}

#[test]
fn prepend_ascii_at_the_margin_does_not_wrap_or_underflow() {
    // Newport's public minimum is two columns. The patch's upstream Zig test
    // also covers a single column, where the pending-wrap cursor remains x=0.
    for columns in [2, 4] {
        for wrap in [true, false] {
            for chunk_size in CHUNKS {
                let mut engine = Engine::new(columns, 3).unwrap();
                let mode = if wrap { "" } else { "\x1b[?7l" };
                let prefix = "x".repeat(usize::from(columns - 1));
                feed(&mut engine, &format!("{mode}{prefix}\u{600}a"), chunk_size);
                assert_eq!(
                    engine.cursor_position().unwrap(),
                    PointCoordinate {
                        x: columns - 1,
                        y: 0,
                    }
                );
                {
                    let frame = engine.frame().unwrap();
                    let cell = &frame.rows[0].cells[usize::from(columns - 1)];
                    assert_eq!(cell.text, "\u{600}a");
                    assert_eq!(cell.wide, CellWide::Narrow);
                }
                feed(&mut engine, "!", chunk_size);
                if wrap {
                    assert_eq!(
                        engine.visible_lines().unwrap(),
                        [format!("{prefix}\u{600}a"), "!".into()]
                    );
                } else {
                    assert_eq!(engine.visible_lines().unwrap(), [format!("{prefix}!")]);
                }
            }
        }
    }
}

#[test]
fn prepend_uses_the_current_cell_and_preserves_its_original_attributes() {
    for chunk_size in CHUNKS {
        let mut engine = Engine::new(10, 3).unwrap();
        feed(
            &mut engine,
            "\u{600}\x1b[2;1Hxyz\x1b[1;2H\x1b[31ma!",
            chunk_size,
        );
        assert_eq!(
            engine.cursor_position().unwrap(),
            PointCoordinate { x: 2, y: 0 }
        );
        let frame = engine.frame().unwrap();
        assert_eq!(frame.rows[0].cells[0].text, "\u{600}a");
        assert_ne!(
            frame.rows[0].cells[0].style.fg_color,
            StyleColor::Palette(PaletteIndex::RED)
        );
        assert_eq!(frame.rows[0].cells[1].text, "!");
        assert_eq!(
            frame.rows[0].cells[1].style.fg_color,
            StyleColor::Palette(PaletteIndex::RED)
        );
        assert_eq!(frame.visible_lines(), ["\u{600}a!", "xyz"]);
    }
}

#[test]
fn extend_after_prepend_does_not_join_a_subsequent_ascii_base() {
    // GB9b applies to the immediately preceding property, not any Prepend
    // somewhere earlier in a cluster. Extend × ASCII is a break.
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
fn nonmoving_controls_do_not_break_a_partially_received_grapheme() {
    // Original cases: unknown-osc, unknown-dcs, cancel-control, unknown-control,
    // null-boundary, escape-title, control-boundary, sync-ignored-control,
    // sync-null, sync-title. Controls are filtered before Ghostty segmentation.
    let controls = [
        "\0",
        "\x18",
        "\x1b[31m",
        "\x1b[999z",
        "\x1b]2;title\x07",
        "\x1b]999;ignored\x07",
        "\x1bPignored\x1b\\",
    ];
    for control in controls {
        for synchronized in [false, true] {
            let (begin, end) = if synchronized {
                ("\x1b[?2026h", "\x1b[?2026l")
            } else {
                ("", "")
            };
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
    // Original cases: isolated, combining-cr. A mark at column zero has no
    // preceding cell. CR is a cursor movement, unlike SGR/title/NUL above.
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
    // Original case: unicode-provider-right-margin. The new emoji needs two
    // cells, so the final column is a spacer and the emoji starts the next row.
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
        drop(frame);
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
