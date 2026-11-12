//! Literal, Unicode-aware search across Alacritty's retained screen.
use crate::{Point, PointCoordinate};
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

pub(crate) fn search(
    terminal: &alacritty_terminal::Term<crate::engine::Listener>,
    query: &str,
    match_case: bool,
) -> SearchResults {
    use alacritty_terminal::{
        grid::Dimensions,
        index::{Column, Line},
        term::cell::Flags,
    };
    let query: Vec<char> = if match_case {
        query.chars().collect()
    } else {
        query.chars().flat_map(char::to_lowercase).collect()
    };
    let mut results = SearchResults::default();
    if query.is_empty() {
        return results;
    }
    let mut matcher = Matcher::new(query);
    let history = terminal.history_size() as i32;
    for y in -history..terminal.screen_lines() as i32 {
        for x in 0..terminal.columns() {
            let cell = &terminal.grid()[Line(y)][Column(x)];
            if cell.flags.contains(Flags::HIDDEN) {
                matcher.reset();
                continue;
            }
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            let position = Position {
                row: (y + history) as u32,
                column: x as u16,
            };
            let end = Position {
                column: (x + usize::from(cell.flags.contains(Flags::WIDE_CHAR)))
                    .min(terminal.columns() - 1) as u16,
                ..position
            };
            for character in
                std::iter::once(cell.c).chain(cell.zerowidth().unwrap_or_default().iter().copied())
            {
                if match_case {
                    if append_match(&mut matcher, &mut results, character, position, end) {
                        return results;
                    }
                } else {
                    for folded in character.to_lowercase() {
                        if append_match(&mut matcher, &mut results, folded, position, end) {
                            return results;
                        }
                    }
                }
            }
        }
        if !terminal.grid()[Line(y)][Column(terminal.columns() - 1)]
            .flags
            .contains(Flags::WRAPLINE)
        {
            matcher.reset();
        }
    }
    results
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
