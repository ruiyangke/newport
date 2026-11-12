//! Search decorations in terminal cell coordinates; they never affect PTY size.
use newport_terminal::Position;
use std::ops::RangeInclusive;

type Match = RangeInclusive<Position>;

pub(super) fn border_color(dark: bool, active: bool) -> u32 {
    match (dark, active) {
        (true, true) => 0xffb74d,
        (false, true) => 0xf57c00,
        (true, false) => 0xc9b458,
        (false, false) => 0xd8b43a,
    }
}

/// A wrapped match contributes one bounded segment per visible terminal row.
/// Search ranges include a wide character's spacer, so the border covers both
/// columns without shaping text or altering the parser's grid coordinates.
pub(super) fn segments(
    matches: &[Match],
    active: Option<&Match>,
    offset: usize,
    rows: usize,
    columns: usize,
) -> Vec<(usize, usize, usize, bool)> {
    if rows == 0 || columns == 0 {
        return Vec::new();
    }
    let first = offset as i64;
    let last = first + rows as i64 - 1;
    let begin = matches.partition_point(|range| i64::from(range.end().row) < first);
    let mut result = Vec::new();
    for range in matches[begin..]
        .iter()
        .take_while(|range| i64::from(range.start().row) <= last)
    {
        let start_line = i64::from(range.start().row);
        let end_line = i64::from(range.end().row);
        for line in start_line.max(first)..=end_line.min(last) {
            let start = if line == start_line {
                usize::from(range.start().column)
            } else {
                0
            };
            let end = if line == end_line {
                usize::from(range.end().column).saturating_add(1)
            } else {
                columns
            };
            let end = end.min(columns);
            if start < end {
                result.push((
                    (line - first) as usize,
                    start,
                    end - start,
                    active == Some(range),
                ));
            }
        }
    }
    result.sort_by_key(|segment| segment.3);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_terminal::{Engine, ScrollViewport};
    fn point(row: u32, column: u16) -> Position {
        Position { row, column }
    }
    fn span(row: u32, start: u16, end: u16) -> Match {
        point(row, start)..=point(row, end)
    }
    #[test]
    fn wrapped_matches_clip_to_the_viewport_and_keep_active_identity() {
        let wrapped = point(4, 6)..=point(7, 2);
        let found = vec![span(0, 0, 2), wrapped.clone(), span(10, 0, 2)];
        assert_eq!(
            segments(&found, Some(&wrapped), 5, 3, 8),
            vec![(0, 0, 8, true), (1, 0, 8, true), (2, 0, 3, true)]
        );
        assert!(segments(&found, None, 0, 0, 8).is_empty());
    }
    #[test]
    fn real_wide_text_uses_both_cells_before_and_after_scrollback() {
        let mut engine = Engine::new(8, 2).unwrap();
        engine
            .advance("a界b\r\nsecond\r\nthird".as_bytes())
            .unwrap();
        let results = engine.search("界", true).unwrap();
        assert_eq!(results.matches.len(), 1);
        let frame = engine.frame().unwrap();
        assert!(segments(&results.matches, None, frame.viewport_top, 2, 8).is_empty());
        engine.scroll(ScrollViewport::Top);
        let frame = engine.frame().unwrap();
        assert_eq!(
            segments(
                &results.matches,
                results.matches.first(),
                frame.viewport_top,
                frame.rows.len(),
                usize::from(frame.columns)
            ),
            vec![(0, 1, 2, true)]
        );
    }
    #[test]
    fn active_border_is_painted_after_overlapping_inactive_matches() {
        let found = [span(0, 0, 3), span(0, 1, 4)];
        let borders = segments(&found, found.first(), 0, 2, 8);
        assert_eq!(borders, vec![(0, 1, 4, false), (0, 0, 4, true)]);
        assert_eq!(border_color(false, true), 0xf57c00);
        assert_eq!(border_color(true, false), 0xc9b458);
    }
}
