//! A terminal kept beside a shell on its server, so a client that attaches
//! later can be brought to the same state. It runs the client's own engine
//! and handler on the same output; rows that scroll into history are kept as
//! written text instead of grid cells.
use crate::{
    Effects, Engine, Result,
    engine::SCROLLBACK_ROWS,
    snapshot::{self, WrittenRow},
};
use alacritty_terminal::{
    grid::{Dimensions, Grid},
    index::Line,
    term::{TermMode, cell::Cell},
};
use std::collections::VecDeque;

/// History rows gathered in the engine before they are moved to the text
/// store, so a burst of output is written out in one pass.
const SPILL_BATCH: usize = 256;
/// The most bytes a full [`Snapshot`] takes, unless the screen alone takes
/// more. The oldest history is left out of a snapshot that would take more.
pub const SNAPSHOT_BUDGET: usize = 15 << 20;
/// The bytes of written history text kept. A row with a colour on every cell
/// takes kilobytes, so heavy output reaches this bound before
/// [`SCROLLBACK_ROWS`]. The rest of [`SNAPSHOT_BUDGET`] is the screen's.
const HISTORY_BYTES: usize = SNAPSHOT_BUDGET / 2;

/// Output that brings a fresh [`Engine`] to a replica's state; see
/// [`Engine::restore`].
pub struct Snapshot {
    /// The visible screen alone.
    pub screen: Vec<u8>,
    /// The screen with as much of the newest history as fits in
    /// [`SNAPSHOT_BUDGET`]. It replaces what `screen` drew: history can't be
    /// added above a screen that is already drawn.
    pub full: Vec<u8>,
}

pub struct Replica {
    engine: Engine,
    history: VecDeque<WrittenRow>,
    /// What `history` takes in a snapshot.
    history_bytes: usize,
    title: Option<String>,
    prompted: bool,
}

impl Replica {
    pub fn new(columns: u16, rows: u16) -> Result<Self> {
        Ok(Self {
            engine: Engine::new(columns, rows)?,
            history: VecDeque::new(),
            history_bytes: 0,
            title: None,
            prompted: false,
        })
    }

    /// Follows the shell's output. The effects carry the replies to terminal
    /// queries, for the holder to answer while no client is attached.
    pub fn advance(&mut self, bytes: &[u8]) -> Result<Effects> {
        let effects = self.engine.advance(bytes)?;
        if let Some(title) = &effects.title {
            self.title.clone_from(title);
        }
        let mode = *self.engine.terminal.mode();
        self.prompted |=
            !mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::BRACKETED_PASTE);
        self.spill();
        Ok(effects)
    }

    /// Whether the shell has turned bracketed paste on at a prompt, so its
    /// being off means a command is running. A tab tells running commands
    /// apart this way, and a restored tab never saw the prompt itself.
    pub fn prompted(&self) -> bool {
        self.prompted
    }

    /// The title the program set last, if it set one.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// What the shell's programs report about themselves, for a held
    /// shell's record: the entry that most needs the person, as its report.
    pub fn agent_report(&self) -> Option<String> {
        self.engine
            .program_statuses()
            .most_urgent()
            .map(crate::ProgramStatus::report)
    }

    /// The colours the program's colour queries are answered with: those of
    /// the tab that last used the shell, which the server can't know itself.
    pub fn set_colors(
        &mut self,
        foreground: crate::RgbColor,
        background: crate::RgbColor,
        cursor: crate::RgbColor,
    ) -> Result<()> {
        self.engine.set_theme(crate::Theme {
            foreground,
            background,
            cursor,
            ..crate::Theme::default()
        })
    }

    pub fn resize(&mut self, columns: u16, rows: u16) -> Result<()> {
        let (width, height) = self.engine.cell_size();
        self.engine.resize(columns, rows, width, height)?;
        Ok(())
    }

    fn spill(&mut self) {
        let term = &self.engine.terminal;
        // The primary screen's history doesn't grow while the alternate
        // screen is active, and the alternate screen has none.
        if term.mode().contains(TermMode::ALT_SCREEN) || term.history_size() < SPILL_BATCH {
            return;
        }
        self.move_history();
    }

    /// Moves the active grid's history to the text store, dropping the
    /// oldest stored rows past either bound.
    fn move_history(&mut self) {
        for mut row in grid_history(self.engine.terminal.grid()) {
            row.text.shrink_to_fit();
            self.history_bytes += row.written_len();
            self.history.push_back(row);
        }
        while self.history.len() > SCROLLBACK_ROWS || self.history_bytes > HISTORY_BYTES {
            let Some(row) = self.history.pop_front() else {
                break;
            };
            self.history_bytes -= row.written_len();
        }
        self.engine.terminal.grid_mut().clear_history();
    }

    pub fn snapshot(&mut self) -> Snapshot {
        let engine = &mut self.engine;
        // The alternate screen's rows and cursor, read before swapping to
        // the primary screen, which alacritty keeps as the inactive grid.
        let alternate = engine
            .terminal
            .mode()
            .contains(TermMode::ALT_SCREEN)
            .then(|| {
                let mut rows = String::new();
                snapshot::write_grid(&mut rows, engine.terminal.grid());
                let mut cursor = String::new();
                snapshot::write_active_cursor(&mut cursor, &engine.terminal, &engine.extensions);
                engine.terminal.swap_alt();
                (rows, cursor)
            });
        // The grid's history, written aside rather than moved: a client
        // restored from this snapshot keeps it in its grid, and growing the
        // window pulls it back onto the screen there and here alike.
        let unspilled = grid_history(engine.terminal.grid());
        let mut screen = String::new();
        snapshot::write_grid(&mut screen, engine.terminal.grid());
        snapshot::write_state(
            &mut screen,
            &engine.terminal,
            &engine.extensions,
            alternate.as_ref().map(|(rows, _)| rows.as_str()),
            self.title.as_deref(),
            &engine.program_statuses,
        );
        match &alternate {
            Some((rows, cursor)) => {
                screen.push_str(cursor);
                // Swapping back would clear the alternate screen; replaying
                // it restores the replica, entered as the program entered it.
                let enter = if engine.extensions.primary_saved.is_some() {
                    "\x1b[?47h"
                } else {
                    "\x1b[?1049h"
                };
                let back = format!("{enter}\x1b[H{rows}{cursor}");
                let _ = engine.advance(back.as_bytes());
            }
            None => {
                snapshot::write_active_cursor(&mut screen, &engine.terminal, &engine.extensions)
            }
        }

        // The newest history that fits beside the screen, which is never cut.
        let room = SNAPSHOT_BUDGET.saturating_sub(screen.len());
        let history = || self.history.iter().chain(&unspilled);
        let (kept, bytes) = newest_that_fit(history().rev(), room);
        let mut full = String::with_capacity(bytes + screen.len());
        for row in history().skip(self.history.len() + unspilled.len() - kept) {
            full.push_str(&row.text);
            if !row.wraps {
                full.push_str("\r\n");
            }
        }
        full.push_str(&screen);
        Snapshot {
            screen: screen.into_bytes(),
            full: full.into_bytes(),
        }
    }
}

/// The grid's history rows written out, oldest first.
fn grid_history(grid: &Grid<Cell>) -> Vec<WrittenRow> {
    (1..=grid.history_size() as i32)
        .rev()
        .map(|h| snapshot::write_row(grid, Line(-h)))
        .collect()
}

/// How many of `rows`, newest first, fit in `room` bytes, and the bytes
/// they take.
fn newest_that_fit<'a>(rows: impl Iterator<Item = &'a WrittenRow>, room: usize) -> (usize, usize) {
    let mut left = room;
    let kept = rows
        .take_while(|row| match left.checked_sub(row.written_len()) {
            Some(rest) => {
                left = rest;
                true
            }
            None => false,
        })
        .count();
    (kept, room - left)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::{grid::Dimensions, index::Column};
    use std::fmt::Write;

    /// Everything a snapshot must carry, from the active screen. Names
    /// alacritty generates for links without an id are left out: each
    /// terminal numbers its own.
    fn fingerprint(engine: &Engine) -> String {
        let out = raw_fingerprint(engine);
        let mut cleaned = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some(end) = rest.find("_alacritty\"") {
            let start = rest[..end].rfind('"').map_or(0, |i| i + 1);
            cleaned.push_str(&rest[..start]);
            cleaned.push_str("generated");
            rest = &rest[end + "_alacritty".len()..];
        }
        cleaned.push_str(rest);
        cleaned
    }

    fn raw_fingerprint(engine: &Engine) -> String {
        let term = &engine.terminal;
        let grid = term.grid();
        let mut out = String::new();
        let _ = writeln!(out, "mode {:?}", term.mode());
        let _ = writeln!(out, "handler {:?}", engine.extensions.modes);
        let _ = writeln!(out, "region {:?}", engine.extensions.scroll_region);
        let _ = writeln!(out, "charset {:?}", engine.extensions.active_charset);
        let _ = writeln!(out, "style {:?}", term.cursor_style());
        let colors = term.colors();
        for i in 0..259 {
            if let Some(c) = colors[i] {
                let _ = writeln!(out, "color {i} {c:?}");
            }
        }
        let cursor = &grid.cursor;
        let _ = writeln!(
            out,
            "cursor {:?} wrap {} charsets {:?} template {:?}",
            cursor.point, cursor.input_needs_wrap, cursor.charsets, cursor.template
        );
        let _ = writeln!(out, "saved {:?}", grid.saved_cursor.point);
        let _ = writeln!(out, "history {}", grid.history_size());
        for y in -(grid.history_size() as i32)..grid.screen_lines() as i32 {
            let row = &grid[Line(y)];
            let _ = write!(out, "{y}:");
            for x in 0..grid.columns() {
                let _ = write!(out, " {:?}", row[Column(x)]);
            }
            out.push('\n');
        }
        out
    }

    fn assert_same(got: &Engine, want: &Engine, what: &str) {
        let (got, want) = (fingerprint(got), fingerprint(want));
        if got != want {
            let first = got
                .lines()
                .zip(want.lines())
                .find(|(g, w)| g != w)
                .map(|(g, w)| {
                    let at = g.chars().zip(w.chars()).take_while(|(a, b)| a == b).count();
                    let start = at.saturating_sub(40);
                    let tail = |s: &str| s.chars().skip(start).take(300).collect::<String>();
                    format!("\n got: …{}\nwant: …{}", tail(g), tail(w))
                })
                .unwrap_or_else(|| "a different number of lines".into());
            panic!("{what}: {first}");
        }
    }

    struct Run {
        truth: Engine,
        replica: Replica,
        title: Option<String>,
    }

    fn run(columns: u16, rows: u16, output: &[u8]) -> Run {
        let mut truth = Engine::new(columns, rows).unwrap();
        let mut replica = Replica::new(columns, rows).unwrap();
        let mut title = None;
        // Feed in pieces, as output arrives, so spilling happens midway.
        for chunk in output.chunks(4096) {
            if let Some(t) = truth.advance(chunk).unwrap().title {
                title = t;
            }
            replica.advance(chunk).unwrap();
        }
        Run {
            truth,
            replica,
            title,
        }
    }

    fn restored(run: &mut Run) -> (Engine, Engine, Option<String>) {
        let (columns, rows) = (run.truth.size.columns, run.truth.size.rows);
        let snapshot = run.replica.snapshot();
        let mut full = Engine::new(columns, rows).unwrap();
        let title = full.restore(&snapshot.full).unwrap().title.flatten();
        let mut screen = Engine::new(columns, rows).unwrap();
        screen.restore(&snapshot.screen).unwrap();
        (full, screen, title)
    }

    /// The full snapshot restores everything; the screen part restores all
    /// but history. Both keep matching as `after` arrives.
    fn assert_restores(columns: u16, rows: u16, output: &[u8], after: &[u8]) {
        let mut run = run(columns, rows, output);
        let (mut full, mut screen, title) = restored(&mut run);
        assert_same(&full, &run.truth, "full snapshot");
        assert_eq!(title, run.title, "title");
        let visible = |engine: &mut Engine| engine.visible_lines().unwrap();
        assert_eq!(
            visible(&mut screen),
            visible(&mut run.truth),
            "screen snapshot"
        );
        assert_eq!(
            screen.terminal.grid().cursor.point,
            run.truth.terminal.grid().cursor.point
        );
        // Taking a snapshot leaves the replica as it was.
        let mut again = Engine::new(columns, rows).unwrap();
        again.restore(&run.replica.snapshot().full).unwrap();
        assert_same(&again, &run.truth, "second snapshot");
        if !after.is_empty() {
            run.truth.advance(after).unwrap();
            full.advance(after).unwrap();
            assert_same(&full, &run.truth, "after more output");
        }
    }

    #[test]
    fn a_shell_that_turned_bracketed_paste_on_at_its_prompt_has_prompted() {
        let mut replica = Replica::new(80, 24).unwrap();
        replica.advance(b"$ ").unwrap();
        assert!(!replica.prompted(), "no sign either way yet");
        replica.advance(b"\x1b[?1049h\x1b[?2004hvim").unwrap();
        assert!(
            !replica.prompted(),
            "a full-screen program's paste mode isn't a prompt"
        );
        replica
            .advance(b"\x1b[?2004l\x1b[?1049l\x1b[?2004h$ ")
            .unwrap();
        assert!(replica.prompted());
        replica.advance(b"\x1b[?2004lsleep 600\r\n").unwrap();
        assert!(replica.prompted(), "it stays known while a command runs");
    }

    #[test]
    fn restores_a_shell_screen() {
        assert_restores(
            80,
            24,
            b"$ ls\r\nCargo.toml  src\r\n$ ",
            b"echo hi\r\nhi\r\n$ ",
        );
    }

    #[test]
    fn restores_colours_attributes_and_links() {
        let output = concat!(
            "\x1b[1mbold\x1b[0m \x1b[2mdim\x1b[0m \x1b[3mitalic\x1b[0m \x1b[4munder\x1b[0m ",
            "\x1b[4:2mdouble\x1b[0m \x1b[4:3mcurl\x1b[0m \x1b[4:4mdots\x1b[0m \x1b[4:5mdash\x1b[0m ",
            "\x1b[7minverse\x1b[0m \x1b[8mhidden\x1b[0m \x1b[9mstrike\x1b[0m\r\n",
            "\x1b[31;42mred on green\x1b[0m \x1b[91;104mbright\x1b[0m ",
            "\x1b[38;5;208;48;5;17mindexed\x1b[0m \x1b[38;2;10;200;90;48;2;1;2;3mtrue\x1b[0m\r\n",
            "\x1b[4;58;5;196munderline colour\x1b[0m \x1b[4;58;2;9;8;7mrgb\x1b[0m\r\n",
            "\x1b]8;id=doc;https://newport.app/docs\x1b\\docs\x1b]8;;\x1b\\ ",
            "\x1b]8;;https://example.com\x1b\\anonymous\x1b]8;;\x1b\\ plain\r\n",
            "\x1b[44m  coloured blanks  \x1b[0m\r\n",
        );
        assert_restores(80, 24, output.as_bytes(), b"\x1b[32mnext\x1b[0m");
    }

    #[test]
    fn restores_wide_characters_and_clusters() {
        let output = "你好 wide\r\n👨\u{200d}👩\u{200d}👧 family 🇯🇵 flag e\u{301} combining ✔\u{fe0f} emoji\r\n";
        assert_restores(40, 10, output.as_bytes(), "世界".as_bytes());
    }

    #[test]
    fn restores_wrapped_rows_that_still_reflow() {
        let long = "0123456789".repeat(25);
        let output = format!("{long}\r\n$ ");
        let mut run = run(80, 24, output.as_bytes());
        let (mut full, _, _) = restored(&mut run);
        assert_same(&full, &run.truth, "before the resize");
        full.resize(120, 24, 8, 16).unwrap();
        run.truth.resize(120, 24, 8, 16).unwrap();
        assert_same(&full, &run.truth, "after the resize");
    }

    #[test]
    fn restores_history_longer_than_the_scrollback() {
        let mut output = String::new();
        for i in 0..SCROLLBACK_ROWS + 2 * SPILL_BATCH + 7 {
            let _ = write!(
                output,
                "\x1b[3{}mline {i}\x1b[0m {}\r\n",
                i % 8,
                "x".repeat(i % 97)
            );
        }
        assert_restores(80, 24, output.as_bytes(), b"tail\r\n");
    }

    #[test]
    fn restores_a_full_screen_program_and_the_shell_under_it() {
        let output = concat!(
            "$ seq 3\r\n1\r\n2\r\n3\r\n$ vim\r\n",
            "\x1b[?1049h\x1b[?1h\x1b=\x1b[H\x1b[2J",
            "\x1b[1;1H\x1b[33m  1 \x1b[0mfn main() {\x1b[2;1H\x1b[33m  2 \x1b[0m}",
            "\x1b[24;1H\x1b[7m-- INSERT --\x1b[0m\x1b[1;6H",
        );
        assert_restores(80, 24, output.as_bytes(), b"\x1b[?1l\x1b>\x1b[?1049l$ ");
    }

    #[test]
    fn restores_screens_switched_by_modes_47_and_1047() {
        assert_restores(80, 24, b"primary\x1b[?47hon 47", b"\x1b[?47lback");
        assert_restores(80, 24, b"primary\x1b[?1047hon 1047", b"\x1b[?1047lback");
    }

    #[test]
    fn restores_the_scrolling_region_and_origin_mode() {
        let mut output = String::from("top\x1b[5;20r\x1b[20;1H");
        for i in 0..30 {
            let _ = write!(output, "row {i}\r\n");
        }
        assert_restores(80, 24, output.as_bytes(), b"more\r\nand more\r\n");
        assert_restores(80, 24, b"\x1b[3;10r\x1b[?6h\x1b[2;4Hin", b"\x1b[Hhome");
    }

    #[test]
    fn restores_character_sets() {
        assert_restores(80, 24, b"\x1b(0lqqk\x1b(B text", b"\x1b(0x");
        assert_restores(80, 24, b"\x1b)0\x0elqk", b"mqj\x0fascii");
    }

    #[test]
    fn restores_a_pending_wrap() {
        let output = "x".repeat(80);
        assert_restores(80, 24, output.as_bytes(), b"next");
    }

    #[test]
    fn restores_the_saved_cursor_and_pen() {
        assert_restores(
            80,
            24,
            b"\x1b[5;10H\x1b7\x1b[1;1Htop\x1b[1;31m",
            b"red\x1b8saved",
        );
    }

    #[test]
    fn restores_modes() {
        let output = concat!(
            "\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b[?1004h\x1b[?2004h\x1b[?1h\x1b=",
            "\x1b[?25l\x1b[5 q\x1b[>1u\x1b[?5h\x1b[?1015h\x1b[?1016h\x1b[>4;2m",
            "\x1b[4h\x1b[20h\x1b[?2027l\x1b[?1007h",
        );
        assert_restores(80, 24, output.as_bytes(), b"text");
        assert_restores(80, 24, b"\x1b[?9h\x1b[?1003h\x1b[?1005h\x1b[?7l", b"text");
    }

    #[test]
    fn restores_colours_set_by_the_program_and_the_title() {
        let output = concat!(
            "\x1b]4;1;rgb:12/34/56\x1b\\\x1b]4;200;rgb:ab/cd/ef\x07",
            "\x1b]10;rgb:01/02/03\x1b\\\x1b]11;rgb:f0/f1/f2\x1b\\\x1b]12;rgb:aa/bb/cc\x1b\\",
            "\x1b]2;building newport\x07$ ",
        );
        assert_restores(80, 24, output.as_bytes(), b"\x1b[31mred");
    }

    #[test]
    fn restores_what_programs_reported_about_themselves() {
        let output = concat!(
            "\x1b]7501;state=working:app=claude-code:id=build\x07",
            "\x1b]7501;state=blocked:app=claude-code:kind=permission:msg=QmFzaA==\x07",
        );
        let mut run = run(80, 24, output.as_bytes());
        let (full, screen, _) = restored(&mut run);
        assert!(!run.truth.program_statuses().is_empty());
        assert_eq!(full.program_statuses(), run.truth.program_statuses());
        assert_eq!(screen.program_statuses(), run.truth.program_statuses());
    }

    #[test]
    fn restores_after_a_resize() {
        let mut run = run(80, 24, b"$ seq 100\r\n1\r\n2\r\n$ ");
        run.truth.resize(100, 30, 8, 16).unwrap();
        run.replica.resize(100, 30).unwrap();
        let (full, _, _) = restored(&mut run);
        assert_same(&full, &run.truth, "resized");
    }

    fn line_text(engine: &Engine, line: i32) -> String {
        let grid = engine.terminal.grid();
        let row = &grid[Line(line)];
        (0..grid.columns()).map(|x| row[Column(x)].c).collect()
    }

    /// Numbered rows with their own colours on every cell, `pen` added to
    /// each cell's.
    fn colour_on_every_cell(count: usize, columns: u16, pen: &str) -> String {
        let mut output = String::new();
        for i in 0..count {
            let _ = write!(output, "{i:06}");
            for x in 6..columns as usize {
                let _ = write!(
                    output,
                    "\x1b[{pen}38;2;{};{};{}m{}",
                    (i + x) % 256,
                    (i * 7 + x) % 256,
                    (i * 13 + x * 3) % 256,
                    char::from(b'a' + (x % 26) as u8)
                );
            }
            output.push_str("\x1b[m\r\n");
        }
        output
    }

    /// History rows a snapshot could carry: the text store's and the grid's.
    fn available(replica: &Replica) -> usize {
        replica.history.len() + replica.engine.terminal.history_size()
    }

    /// A client restored from the replica's snapshot, which fits the budget
    /// and shows the whole screen.
    fn restored_within_budget(run: &mut Run) -> Engine {
        let (columns, rows) = (run.truth.size.columns, run.truth.size.rows);
        let snapshot = run.replica.snapshot();
        assert!(
            snapshot.full.len() <= SNAPSHOT_BUDGET,
            "{}",
            snapshot.full.len()
        );
        let mut restored = Engine::new(columns, rows).unwrap();
        restored.restore(&snapshot.full).unwrap();
        assert_eq!(
            restored.visible_lines().unwrap(),
            run.truth.visible_lines().unwrap(),
            "the screen is whole"
        );
        restored
    }

    /// The restored history is the truth's newest; returns how many rows.
    fn assert_newest_history(restored: &Engine, truth: &Engine) -> usize {
        let kept = restored.terminal.grid().history_size();
        assert!(kept > 0, "some history is kept");
        for y in 1..=kept as i32 {
            assert_eq!(
                line_text(restored, -y),
                line_text(truth, -y),
                "the newest history rows are the ones kept"
            );
        }
        kept
    }

    #[test]
    fn a_full_scrollback_of_colour_on_every_cell_still_fits_one_snapshot() {
        let (columns, rows) = (200, 60);
        let output = colour_on_every_cell(SCROLLBACK_ROWS + rows as usize, columns, "");
        assert!(output.len() > SNAPSHOT_BUDGET, "the output is heavy enough");
        let mut run = run(columns, rows, output.as_bytes());
        assert!(run.replica.history_bytes <= HISTORY_BYTES);
        let restored = restored_within_budget(&mut run);
        let kept = assert_newest_history(&restored, &run.truth);
        assert!(kept < SCROLLBACK_ROWS, "{kept} history rows");
        assert_eq!(kept, available(&run.replica));
    }

    #[test]
    fn a_screen_too_large_to_share_the_budget_keeps_the_history_that_fits() {
        let (columns, rows) = (500, 300);
        let pen = "1;3;4;48;2;9;99;199;58;2;199;99;9;";
        let output = colour_on_every_cell(3 * rows as usize, columns, pen);
        let mut run = run(columns, rows, output.as_bytes());
        let restored = restored_within_budget(&mut run);
        let kept = assert_newest_history(&restored, &run.truth);
        assert!(
            kept < available(&run.replica),
            "the oldest kept rows are left out"
        );
    }

    #[test]
    fn heavy_history_under_a_full_screen_program_fits_and_comes_back_after_it() {
        let (columns, rows) = (200, 60);
        let mut output = colour_on_every_cell(SCROLLBACK_ROWS + rows as usize, columns, "");
        output.push_str("\x1b[?1049h\x1b[H\x1b[2J\x1b[33meditor\x1b[m\x1b[2;1Hline two");
        let mut run = run(columns, rows, output.as_bytes());
        let mut restored = restored_within_budget(&mut run);
        let leave = b"\x1b[?1049l";
        restored.advance(leave).unwrap();
        run.truth.advance(leave).unwrap();
        assert_eq!(
            restored.visible_lines().unwrap(),
            run.truth.visible_lines().unwrap(),
            "the shell's screen under the program"
        );
        assert_newest_history(&restored, &run.truth);
    }

    #[test]
    fn a_snapshot_leaves_the_replica_growing_as_the_client_restored_from_it() {
        let mut output = String::new();
        for i in 0..100 {
            let _ = write!(output, "line {i} {}\r\n", "w".repeat(i % 3 * 60));
        }
        output.push_str("$ ");
        let mut run = run(80, 24, output.as_bytes());
        assert!(
            run.replica.engine.terminal.history_size() > 0,
            "rows not yet in the text store"
        );
        let snapshot = run.replica.snapshot();
        let mut client = Engine::new(80, 24).unwrap();
        client.restore(&snapshot.full).unwrap();
        for (columns, rows) in [(80, 40), (120, 50)] {
            run.replica.resize(columns, rows).unwrap();
            client.resize(columns, rows, 8, 16).unwrap();
            let replica = &mut run.replica.engine;
            assert_eq!(
                replica.visible_lines().unwrap(),
                client.visible_lines().unwrap(),
                "at {columns}x{rows}"
            );
            assert_eq!(
                replica.terminal.grid().cursor.point,
                client.terminal.grid().cursor.point,
                "the cursor at {columns}x{rows}"
            );
        }
    }

    #[test]
    fn history_fits_up_to_the_last_byte_of_room() {
        let row = |text: &str, wraps| WrittenRow {
            text: text.into(),
            wraps,
        };
        // Newest first: five bytes with its line break, then two.
        let rows = [row("abc", false), row("de", true)];
        assert_eq!(newest_that_fit(rows.iter(), 7), (2, 7));
        assert_eq!(newest_that_fit(rows.iter(), 6), (1, 5));
        assert_eq!(newest_that_fit(rows.iter(), 5), (1, 5));
        assert_eq!(newest_that_fit(rows.iter(), 4), (0, 0));
    }

    #[test]
    fn a_snapshot_is_much_smaller_than_the_output_it_replaces() {
        let mut output = String::new();
        for i in 0..20_000 {
            let _ = write!(output, "{i:08} build log line\r\n");
        }
        let mut run = run(120, 40, output.as_bytes());
        let snapshot = run.replica.snapshot();
        assert!(
            snapshot.screen.len() < 8 * 1024,
            "{}",
            snapshot.screen.len()
        );
        assert!(snapshot.full.len() < output.len() / 2);
    }
}
