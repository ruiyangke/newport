//! A terminal's state written back as output. Replayed into a fresh terminal
//! from the top-left of a blank screen, it reproduces the screen, history,
//! cursor, modes, colours and title.
use crate::{engine::Listener, handler::Extensions};
use alacritty_terminal::{
    Term,
    grid::{Dimensions, Grid},
    index::{Column, Line},
    term::{
        TermMode,
        cell::{Cell, Flags, Hyperlink},
    },
    vte::ansi::{CharsetIndex, Color, CursorShape, NamedColor, StandardCharset},
};
use std::fmt::Write;

/// Flags that describe a cell's place, not its look.
const SHAPE: Flags = Flags::WRAPLINE
    .union(Flags::WIDE_CHAR)
    .union(Flags::WIDE_CHAR_SPACER)
    .union(Flags::LEADING_WIDE_CHAR_SPACER);

/// One row written with its attributes, and whether it continues on the
/// next row. A continued row is written full width without a line break, so
/// the replaying terminal wraps it too and can reflow it later.
pub(crate) struct WrittenRow {
    pub text: String,
    pub wraps: bool,
}
impl WrittenRow {
    /// The bytes it takes in a snapshot, with its line break.
    pub fn written_len(&self) -> usize {
        self.text.len() + if self.wraps { 0 } else { 2 }
    }
}

/// What the current cell attributes are while writing, to emit only changes.
#[derive(PartialEq)]
struct Pen {
    fg: Color,
    bg: Color,
    underline: Option<Color>,
    flags: Flags,
    link: Option<Hyperlink>,
}
impl Pen {
    fn plain() -> Self {
        Pen {
            fg: Color::Named(NamedColor::Foreground),
            bg: Color::Named(NamedColor::Background),
            underline: None,
            flags: Flags::empty(),
            link: None,
        }
    }
}

fn pen(cell: &Cell) -> Pen {
    Pen {
        fg: cell.fg,
        bg: cell.bg,
        underline: cell.underline_color(),
        flags: cell.flags - SHAPE,
        link: cell.hyperlink(),
    }
}

fn blank(cell: &Cell) -> bool {
    cell.c == ' ' && pen(cell) == Pen::plain() && cell.zerowidth().is_none()
}

pub(crate) fn write_row(grid: &Grid<Cell>, line: Line) -> WrittenRow {
    let columns = grid.columns();
    let row = &grid[line];
    let wraps = row[Column(columns - 1)].flags.contains(Flags::WRAPLINE);
    let end = if wraps {
        columns
    } else {
        (0..columns)
            .rev()
            .find(|&x| !blank(&row[Column(x)]))
            .map_or(0, |x| x + 1)
    };
    let mut text = String::new();
    let mut current = Pen::plain();
    for x in 0..end {
        let cell = &row[Column(x)];
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        let next = pen(cell);
        if next != current {
            if next.link != current.link {
                write_link(&mut text, next.link.as_ref());
            }
            text.push_str(&sgr(cell));
            current = next;
        }
        text.push(cell.c);
        text.extend(cell.zerowidth().unwrap_or_default());
    }
    if current.link.is_some() {
        write_link(&mut text, None);
    }
    if current != Pen::plain() {
        text.push_str("\x1b[m");
    }
    WrittenRow { text, wraps }
}

/// Writes the screen's rows as one stream from a blank screen, so each lands
/// in place.
pub(crate) fn write_grid(out: &mut String, grid: &Grid<Cell>) {
    let bottom = grid.screen_lines() as i32;
    for y in 0..bottom {
        let row = write_row(grid, Line(y));
        out.push_str(&row.text);
        if y + 1 < bottom && !row.wraps {
            out.push_str("\r\n");
        }
    }
}

/// alacritty names a link that came without an id itself, from a counter in
/// the process; the name means nothing to another terminal.
fn generated_id(id: &str) -> bool {
    id.strip_suffix("_alacritty")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn write_link(out: &mut String, link: Option<&Hyperlink>) {
    match link {
        Some(link) if generated_id(link.id()) => {
            let _ = write!(out, "\x1b]8;;{}\x1b\\", link.uri());
        }
        Some(link) => {
            let _ = write!(out, "\x1b]8;id={};{}\x1b\\", link.id(), link.uri());
        }
        None => out.push_str("\x1b]8;;\x1b\\"),
    }
}

fn color(out: &mut String, color: Color, base: usize) {
    match color {
        Color::Named(named) => {
            let n = named as usize;
            match n {
                0..8 => {
                    let _ = write!(out, ";{}", base + n);
                }
                8..16 => {
                    let _ = write!(out, ";{}", base + 60 + n - 8);
                }
                _ => {}
            }
        }
        Color::Indexed(i) => {
            let _ = write!(out, ";{};5;{i}", base + 8);
        }
        Color::Spec(c) => {
            let _ = write!(out, ";{};2;{};{};{}", base + 8, c.r, c.g, c.b);
        }
    }
}

/// The full SGR for a cell's look, starting from a reset.
pub(crate) fn sgr(cell: &Cell) -> String {
    let mut s = String::from("\x1b[0");
    for (flag, code) in [
        (Flags::BOLD, "1"),
        (Flags::DIM, "2"),
        (Flags::ITALIC, "3"),
        (Flags::UNDERLINE, "4"),
        (Flags::DOUBLE_UNDERLINE, "4:2"),
        (Flags::UNDERCURL, "4:3"),
        (Flags::DOTTED_UNDERLINE, "4:4"),
        (Flags::DASHED_UNDERLINE, "4:5"),
        (Flags::INVERSE, "7"),
        (Flags::HIDDEN, "8"),
        (Flags::STRIKEOUT, "9"),
    ] {
        if cell.flags.contains(flag) {
            s.push(';');
            s.push_str(code);
        }
    }
    color(&mut s, cell.fg, 30);
    color(&mut s, cell.bg, 40);
    match cell.underline_color() {
        Some(Color::Indexed(i)) => {
            let _ = write!(s, ";58;5;{i}");
        }
        Some(Color::Spec(c)) => {
            let _ = write!(s, ";58;2;{};{};{}", c.r, c.g, c.b);
        }
        Some(Color::Named(named)) if (named as usize) < 16 => {
            let _ = write!(s, ";58;5;{}", named as usize);
        }
        _ => {}
    }
    s.push('m');
    s
}

fn goto(out: &mut String, line: i32, column: usize) {
    let _ = write!(out, "\x1b[{};{}H", line + 1, column + 1);
}

/// The active screen's cursor, with the pending-wrap state a full last
/// column leaves: rewriting that cell puts the cursor back in it. With
/// origin mode on, positions count from the top of the scrolling region.
fn write_cursor(out: &mut String, grid: &Grid<Cell>, top: i32) {
    let cursor = &grid.cursor;
    let point = cursor.point;
    goto(out, point.line.0 - top, point.column.0);
    if cursor.input_needs_wrap {
        let cell = &grid[point.line][point.column];
        out.push_str(&sgr(cell));
        out.push(cell.c);
        out.extend(cell.zerowidth().unwrap_or_default());
    }
    out.push_str(&sgr(&cursor.template));
}

fn charset(index: CharsetIndex) -> (char, &'static str) {
    match index {
        CharsetIndex::G0 => ('(', "\x0f"),
        CharsetIndex::G1 => (')', "\x0e"),
        CharsetIndex::G2 => ('*', "\x1bn"),
        CharsetIndex::G3 => ('+', "\x1bo"),
    }
}

fn write_charsets(out: &mut String, grid: &Grid<Cell>, active: CharsetIndex) {
    for index in [
        CharsetIndex::G0,
        CharsetIndex::G1,
        CharsetIndex::G2,
        CharsetIndex::G3,
    ] {
        if grid.cursor.charsets[index] == StandardCharset::SpecialCharacterAndLineDrawing {
            let _ = write!(out, "\x1b{}0", charset(index).0);
        }
    }
    if active != CharsetIndex::G0 {
        out.push_str(charset(active).1);
    }
}

fn write_modes(out: &mut String, term: &Term<Listener>, ext: &Extensions) {
    let mode = *term.mode();
    for (flag, n) in [
        (TermMode::APP_CURSOR, 1),
        (TermMode::ORIGIN, 6),
        (TermMode::LINE_WRAP, 7),
        (TermMode::SHOW_CURSOR, 25),
        (TermMode::MOUSE_REPORT_CLICK, 1000),
        (TermMode::MOUSE_DRAG, 1002),
        (TermMode::MOUSE_MOTION, 1003),
        (TermMode::FOCUS_IN_OUT, 1004),
        (TermMode::UTF8_MOUSE, 1005),
        (TermMode::SGR_MOUSE, 1006),
        (TermMode::ALTERNATE_SCROLL, 1007),
        (TermMode::BRACKETED_PASTE, 2004),
    ] {
        let _ = write!(
            out,
            "\x1b[?{n}{}",
            if mode.contains(flag) { 'h' } else { 'l' }
        );
    }
    let modes = &ext.modes;
    for (on, n) in [
        (modes.reverse_video, 5),
        (modes.x10_mouse, 9),
        (modes.urxvt_mouse, 1015),
        (modes.pixel_mouse, 1016),
        (modes.grapheme_clustering, 2027),
    ] {
        let _ = write!(out, "\x1b[?{n}{}", if on { 'h' } else { 'l' });
    }
    if modes.modify_other_keys != 0 {
        let _ = write!(out, "\x1b[>4;{}m", modes.modify_other_keys);
    }
    out.push_str(if mode.contains(TermMode::APP_KEYPAD) {
        "\x1b="
    } else {
        "\x1b>"
    });
    if mode.contains(TermMode::INSERT) {
        out.push_str("\x1b[4h");
    }
    if mode.contains(TermMode::LINE_FEED_NEW_LINE) {
        out.push_str("\x1b[20h");
    }
    let kitty = [
        TermMode::DISAMBIGUATE_ESC_CODES,
        TermMode::REPORT_EVENT_TYPES,
        TermMode::REPORT_ALTERNATE_KEYS,
        TermMode::REPORT_ALL_KEYS_AS_ESC,
        TermMode::REPORT_ASSOCIATED_TEXT,
    ]
    .iter()
    .enumerate()
    .filter(|(_, flag)| mode.contains(**flag))
    .fold(0u8, |bits, (i, _)| bits | 1 << i);
    if kitty != 0 {
        let _ = write!(out, "\x1b[>{kitty}u");
    }
    let style = term.cursor_style();
    let shape = match style.shape {
        CursorShape::Block => Some(1),
        CursorShape::Underline => Some(3),
        CursorShape::Beam => Some(5),
        _ => None,
    };
    if let Some(n) = shape {
        let _ = write!(out, "\x1b[{} q", n + u8::from(!style.blinking));
    }
}

fn write_colors(out: &mut String, term: &Term<Listener>) {
    let colors = term.colors();
    for i in 0..256 {
        if let Some(c) = colors[i] {
            let _ = write!(
                out,
                "\x1b]4;{i};rgb:{:02x}/{:02x}/{:02x}\x1b\\",
                c.r, c.g, c.b
            );
        }
    }
    for (named, osc) in [
        (NamedColor::Foreground, 10),
        (NamedColor::Background, 11),
        (NamedColor::Cursor, 12),
    ] {
        if let Some(c) = colors[named as usize] {
            let _ = write!(
                out,
                "\x1b]{osc};rgb:{:02x}/{:02x}/{:02x}\x1b\\",
                c.r, c.g, c.b
            );
        }
    }
}

/// What follows the rows: the alternate screen's rows when it is active (the
/// primary screen is then the grid the rows came from), the primary saved
/// cursor, scrolling region, modes, colours and title. The active cursor
/// comes after, from [`write_active_cursor`].
pub(crate) fn write_state(
    out: &mut String,
    term: &Term<Listener>,
    ext: &Extensions,
    alternate: Option<&str>,
    title: Option<&str>,
    programs: &crate::ProgramStatuses,
) {
    out.push_str("\x1b[m");
    let primary = term.grid();
    if let Some(alternate) = alternate {
        // Modes 47 and 1047 keep the primary saved cursor aside and give it
        // back on leaving, so re-enter the same way with it saved first.
        // alacritty's 1049 swaps screens and keeps each one's own cursor;
        // xterm's saves this one first. Either way, leaving the alternate
        // screen comes back to the primary cursor.
        let enter = if let Some(saved) = &ext.primary_saved {
            let saved = saved.point;
            goto(out, saved.line.0, saved.column.0);
            out.push_str("\x1b7");
            "\x1b[?47h\x1b[H"
        } else {
            "\x1b[?1049h\x1b[H"
        };
        goto(
            out,
            primary.cursor.point.line.0,
            primary.cursor.point.column.0,
        );
        out.push_str(enter);
        out.push_str(alternate);
        out.push_str("\x1b[m");
    } else {
        let saved = primary.saved_cursor.point;
        goto(out, saved.line.0, saved.column.0);
        out.push_str("\x1b7");
    }
    // Setting the region homes the cursor, so it comes before the cursor.
    if let Some((top, bottom)) = ext.scroll_region {
        match bottom {
            Some(bottom) => {
                let _ = write!(out, "\x1b[{top};{bottom}r");
            }
            None => {
                let _ = write!(out, "\x1b[{top}r");
            }
        }
    }
    write_modes(out, term, ext);
    write_colors(out, term);
    if let Some(title) = title {
        let _ = write!(out, "\x1b]2;{title}\x07");
    }
    programs.write(out);
}

/// The active screen's charsets and cursor, last: everything before may move
/// the cursor or change the pen.
pub(crate) fn write_active_cursor(out: &mut String, term: &Term<Listener>, ext: &Extensions) {
    let grid = term.grid();
    write_charsets(out, grid, ext.active_charset);
    let top = match ext.scroll_region {
        Some((top, _)) if term.mode().contains(TermMode::ORIGIN) => top as i32 - 1,
        _ => 0,
    };
    write_cursor(out, grid, top);
}
