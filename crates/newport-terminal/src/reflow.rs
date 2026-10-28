//! Keeps the cursor on its own text when a width change reflows the primary
//! screen. The cursor is recorded as a logical position — how many lines it
//! sits above the end of the output, and its offset within its wrapped line —
//! and found again in the reflowed grid.
use alacritty_terminal::{
    grid::{Dimensions, Grid},
    index::{Column, Line, Point},
    term::cell::{Cell, Flags},
};

pub(crate) struct Anchor {
    /// Logical lines between the cursor's line and the last line of output.
    lines_above_end: usize,
    /// Cells before the cursor within its logical line.
    offset: usize,
}

/// The cursor's logical position, when it is on or above the output. Below
/// the output nothing reflows around it, and the grid keeps it in place.
pub(crate) fn anchor(grid: &Grid<Cell>) -> Option<Anchor> {
    let cursor = &grid.cursor;
    let end = last_output(grid)?;
    if cursor.point.line > end {
        return None;
    }
    let mut offset = cursor.point.column.0 + usize::from(cursor.input_needs_wrap);
    let mut line = logical_start(grid, cursor.point.line);
    while line < cursor.point.line {
        offset += content_cells(grid, line);
        line += 1;
    }
    let lines_above_end = (cursor.point.line.0..end.0)
        .filter(|&line| !wraps(grid, Line(line)))
        .count();
    Some(Anchor {
        lines_above_end,
        offset,
    })
}

/// First row of the anchored logical line; negative when it is in history.
pub(crate) fn anchored_line(grid: &Grid<Cell>, anchor: &Anchor) -> Option<Line> {
    let mut start = logical_start(grid, last_output(grid)?);
    for _ in 0..anchor.lines_above_end {
        if start <= grid.topmost_line() {
            return None;
        }
        start = logical_start(grid, start - 1);
    }
    Some(start)
}

/// Blank rows below both the output and the cursor.
pub(crate) fn blank_rows_at_bottom(grid: &Grid<Cell>) -> usize {
    let used = last_output(grid).map_or(-1, |line| line.0);
    let lowest = used.max(grid.cursor.point.line.0);
    (grid.bottommost_line().0 - lowest) as usize
}

/// Move the cursor to the anchored position, starting at `start`. A position
/// still in history leaves the cursor on the top row.
pub(crate) fn place(grid: &mut Grid<Cell>, start: Line, anchor: &Anchor) {
    let columns = grid.columns();
    let mut line = start;
    let mut remaining = anchor.offset;
    while line < grid.bottommost_line()
        && wraps(grid, line)
        && remaining >= content_cells(grid, line)
    {
        remaining -= content_cells(grid, line);
        line += 1;
    }
    let cursor = &mut grid.cursor;
    cursor.point = Point::new(line.max(Line(0)), Column(remaining.min(columns - 1)));
    cursor.input_needs_wrap = remaining >= columns;
}

fn last_output(grid: &Grid<Cell>) -> Option<Line> {
    (0..grid.screen_lines() as i32)
        .rev()
        .map(Line)
        .find(|&line| !grid[line].is_clear())
}

fn wraps(grid: &Grid<Cell>, line: Line) -> bool {
    grid[line][Column(grid.columns() - 1)]
        .flags
        .contains(Flags::WRAPLINE)
}

/// Cells of a row that belong to its logical line: a wide character that did
/// not fit leaves a spacer in the last column of a wrapped row.
fn content_cells(grid: &Grid<Cell>, line: Line) -> usize {
    let last = &grid[line][Column(grid.columns() - 1)];
    if wraps(grid, line) && last.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
        grid.columns() - 1
    } else {
        grid.columns()
    }
}

fn logical_start(grid: &Grid<Cell>, mut line: Line) -> Line {
    while line > grid.topmost_line() && wraps(grid, line - 1) {
        line -= 1;
    }
    line
}
