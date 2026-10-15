//! Regressions for a rejected wide glyph at a no-wrap right margin.
use newport_terminal::{CellWide, Engine};

#[test]
fn clipped_cluster_never_attaches_its_suffix_to_an_unrelated_cell() {
    for prefix in ["123456789", "123456789X"] {
        for cluster in ["👩‍💻", "👩🏽‍💻\u{301}", "界\u{301}"] {
            for chunk_size in [1, 2, 3, 1024] {
                let mut engine = Engine::new(10, 3).unwrap();
                let input = format!("\x1b[?7l{prefix}{cluster}!");
                for chunk in input.as_bytes().chunks(chunk_size) {
                    engine.advance(chunk).unwrap();
                }
                assert_eq!(
                    engine.visible_lines().unwrap(),
                    ["123456789!"],
                    "prefix {prefix}, cluster {cluster}, chunk size {chunk_size}"
                );
                {
                    let frame = engine.frame().unwrap();
                    assert_eq!(frame.rows[0].cells[8].text, "9");
                    assert_eq!(frame.rows[0].cells[9].text, "!");
                    assert_eq!(frame.rows[0].cells[8].wide, CellWide::Narrow);
                }
                engine.advance("\u{301}".as_bytes()).unwrap();
                assert_eq!(engine.visible_lines().unwrap(), ["123456789!\u{301}"]);
            }
        }
    }
}

#[test]
fn clipping_does_not_disable_subsequent_clusters_after_cursor_move_or_reset() {
    let mut engine = Engine::new(10, 3).unwrap();
    engine.advance("\x1b[?7l123456789👩".as_bytes()).unwrap();
    engine.advance("\x1b[2;1He\u{301}".as_bytes()).unwrap();
    assert_eq!(engine.visible_lines().unwrap(), ["123456789", "e\u{301}"]);
    engine.advance("\x1bc👩‍💻".as_bytes()).unwrap();
    assert_eq!(engine.visible_lines().unwrap(), ["👩‍💻"]);
}

#[test]
fn resize_clipped_base_does_not_retarget_suffix_to_identical_neighbor() {
    for screen in ["\x1b[?1049h", "\x1b[?7l"] {
        for chunk_size in [1, 2, 3, 1024] {
            let mut engine = Engine::new(10, 3).unwrap();
            let input = format!("{screen}❤❤❤❤❤❤❤❤❤");
            for chunk in input.as_bytes().chunks(chunk_size) {
                engine.advance(chunk).unwrap();
            }
            engine.resize(6, 3, 8, 16).unwrap();
            for chunk in "\u{fe0f}!".as_bytes().chunks(chunk_size) {
                engine.advance(chunk).unwrap();
            }
            assert_eq!(engine.visible_lines().unwrap(), ["❤❤❤❤❤!"]);
            let frame = engine.frame().unwrap();
            assert_eq!(frame.rows[0].cells[4].text, "❤");
            assert_eq!(frame.rows[0].cells[4].wide, CellWide::Narrow);
        }
    }
}

#[test]
fn resize_preserves_cluster_context_only_when_the_original_base_is_clipped() {
    let mut clipped = Engine::new(10, 3).unwrap();
    clipped.advance("\x1b[?1049h12345👩‍".as_bytes()).unwrap();
    clipped.resize(6, 3, 8, 16).unwrap();
    clipped.advance("💻!".as_bytes()).unwrap();
    assert_eq!(clipped.visible_lines().unwrap(), ["12345!"]);

    for columns in [6, 14] {
        let mut surviving = Engine::new(10, 3).unwrap();
        surviving.advance("\x1b[?1049ha❤".as_bytes()).unwrap();
        surviving.resize(columns, 3, 8, 16).unwrap();
        surviving.advance("\u{fe0f}!".as_bytes()).unwrap();
        assert_eq!(surviving.visible_lines().unwrap(), ["a❤️!"]);
    }
}
