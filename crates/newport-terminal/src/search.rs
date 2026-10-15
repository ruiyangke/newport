//! Literal, Unicode-aware search across Ghostty's retained screen.
use crate::{CellWide, Point, PointCoordinate, Result, frame::Renderer};
use libghostty_vt::{Terminal, render::Overscan};
use std::{collections::VecDeque, ops::RangeInclusive};

pub const MAX_MATCHES: usize = 10_000;

/// Screen coordinates, ordered by row then column, including retained history.
/// These are snapshots: recompute search after output, pruning, or reflow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub row: u32,
    pub column: u16,
}
impl Position {
    pub fn point(self) -> Point {
        Point::Screen(PointCoordinate {
            x: self.column,
            y: self.row,
        })
    }
}

#[derive(Default, Debug, Clone)]
pub struct SearchResults {
    pub matches: Vec<RangeInclusive<Position>>,
    /// True only after observing an additional match beyond the retained cap.
    pub truncated: bool,
}

impl Renderer {
    pub(crate) fn search(
        &mut self,
        terminal: &Terminal<'static, '_>,
        query: &str,
        match_case: bool,
    ) -> Result<SearchResults> {
        let query: Vec<char> = if match_case {
            query.chars().collect()
        } else {
            query.chars().flat_map(char::to_lowercase).collect()
        };
        if query.is_empty() {
            return Ok(SearchResults::default());
        }
        // Share the renderer's state: a separate RenderState would consume the
        // terminal's dirty flags and could leave the visible frame stale. The
        // temporary overscan snapshot traverses rows linearly without resolving
        // every history cell through the terminal's linked page list.
        let previous = self.state.overscan_request()?;
        let history = terminal
            .scrollback_rows()?
            .try_into()
            .map_err(|_| crate::Error::LimitExceeded)?;
        self.force_redraw = true;
        self.state.set_overscan(Overscan {
            above: history,
            below: history,
        })?;
        let result = (|| {
            let snapshot = self.state.update(terminal)?;
            let top = i64::try_from(terminal.scrollbar()?.offset)
                .map_err(|_| crate::Error::LimitExceeded)?;
            let columns = snapshot.cols()?;
            let mut rows = self.rows.update(&snapshot)?;
            let mut matcher = Matcher::new(query);
            let mut results = SearchResults::default();
            let mut text = String::new();
            while let Some(row) = rows.next() {
                let y = u32::try_from(top + i64::from(row.viewport_y()?))
                    .map_err(|_| crate::Error::InvalidValue)?;
                let mut cells = self.cells.update(row)?;
                let mut column = 0;
                while let Some(cell) = cells.next() {
                    let position = Position { row: y, column };
                    column += 1;
                    if cell.style()?.invisible {
                        matcher.reset();
                        continue;
                    }
                    let wide = cell.raw_cell()?.wide()?;
                    if matches!(wide, CellWide::SpacerTail | CellWide::SpacerHead) {
                        continue;
                    }
                    let end = Position {
                        column: if wide == CellWide::Wide {
                            column.min(columns - 1)
                        } else {
                            position.column
                        },
                        ..position
                    };
                    cell.graphemes_utf8(&mut text)?;
                    if text.is_empty() {
                        text.push(' ');
                    }
                    for character in text.chars() {
                        if match_case {
                            if append_match(&mut matcher, &mut results, character, position, end) {
                                return Ok(results);
                            }
                        } else {
                            for folded in character.to_lowercase() {
                                if append_match(&mut matcher, &mut results, folded, position, end) {
                                    return Ok(results);
                                }
                            }
                        }
                    }
                }
                if !row.raw_row()?.is_wrapped()? {
                    matcher.reset();
                }
            }
            Ok(results)
        })();
        // Restore even when collection fails; never retain a history-sized
        // snapshot request in the normal paint loop. force_redraw also covers
        // searches in an empty history where the overscan request stays zero.
        self.state.set_overscan(previous)?;
        result
    }
}

fn append_match(
    matcher: &mut Matcher,
    results: &mut SearchResults,
    character: char,
    start: Position,
    end: Position,
) -> bool {
    if let Some(found) = matcher.advance(character, start, end) {
        if results.matches.len() == MAX_MATCHES {
            results.truncated = true;
            return true;
        }
        results.matches.push(found);
    }
    false
}

/// KMP matching with coordinates for the currently matched prefix. This keeps
/// long wrapped lines from allocating a second complete copy of the grid.
struct Matcher {
    query: Vec<char>,
    prefix: Vec<usize>,
    matched: VecDeque<(Position, Position)>,
}
impl Matcher {
    fn new(query: Vec<char>) -> Self {
        let mut prefix = vec![0; query.len()];
        for index in 1..query.len() {
            let mut length = prefix[index - 1];
            while length > 0 && query[index] != query[length] {
                length = prefix[length - 1];
            }
            if query[index] == query[length] {
                length += 1;
            }
            prefix[index] = length;
        }
        Self {
            query,
            prefix,
            matched: VecDeque::new(),
        }
    }
    fn reset(&mut self) {
        self.matched.clear();
    }
    fn advance(
        &mut self,
        character: char,
        start: Position,
        end: Position,
    ) -> Option<RangeInclusive<Position>> {
        while !self.matched.is_empty() && character != self.query[self.matched.len()] {
            let retained = self.prefix[self.matched.len() - 1];
            self.matched.drain(..self.matched.len() - retained);
        }
        if character == self.query[self.matched.len()] {
            self.matched.push_back((start, end));
        }
        if self.matched.len() == self.query.len() {
            let start = self.matched.front().expect("nonempty query").0;
            let end = self.matched.back().expect("nonempty query").1;
            self.reset();
            Some(start..=end)
        } else {
            None
        }
    }
}
