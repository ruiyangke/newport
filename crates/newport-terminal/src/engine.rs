use crate::handler::{Extensions, Handler};
use crate::{Error, Frame, Mode, Point, PointCoordinate, Result, RgbColor, ScrollViewport};
use alacritty_terminal::{
    Term,
    event::{Event, EventListener, WindowSize},
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point as GridPoint},
    term::{ClipboardType, Config, Osc52, TermMode},
    vte::ansi::Processor,
};
use std::{cell::RefCell, rc::Rc, time::Instant};

/// Host colors, independent of application OSC overrides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    pub foreground: RgbColor,
    pub background: RgbColor,
    pub cursor: RgbColor,
    pub palette: [RgbColor; 256],
}
impl Default for Theme {
    fn default() -> Self {
        let base = [
            0x000000, 0xcd0000, 0x00cd00, 0xcdcd00, 0x0000ee, 0xcd00cd, 0x00cdcd, 0xe5e5e5,
            0x7f7f7f, 0xff0000, 0x00ff00, 0xffff00, 0x5c5cff, 0xff00ff, 0x00ffff, 0xffffff,
        ];
        let palette = std::array::from_fn(|i| {
            let value = if i < 16 {
                base[i]
            } else if i < 232 {
                let n = i - 16;
                let level = |v: usize| if v == 0 { 0 } else { 55 + 40 * v as u32 };
                level(n / 36) << 16 | level(n / 6 % 6) << 8 | level(n % 6)
            } else {
                (8 + 10 * (i - 232) as u32) * 0x010101
            };
            RgbColor {
                r: (value >> 16) as u8,
                g: (value >> 8) as u8,
                b: value as u8,
            }
        });
        Self {
            foreground: palette[7],
            background: palette[0],
            cursor: palette[7],
            palette,
        }
    }
}
#[derive(Default, Debug)]
pub struct Effects {
    pub redraw: bool,
    pub title: Option<Option<String>>,
    pub replies: Vec<Vec<u8>>,
    pub bell: bool,
    pub copy: Option<String>,
}
#[derive(Clone, Default)]
pub(crate) struct Listener(Rc<RefCell<Vec<Event>>>);
impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        self.0.borrow_mut().push(event);
    }
}
#[derive(Clone, Copy)]
pub(crate) struct Size {
    pub columns: u16,
    pub rows: u16,
    pub width: u16,
    pub height: u16,
}
impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows.into()
    }
    fn screen_lines(&self) -> usize {
        self.rows.into()
    }
    fn columns(&self) -> usize {
        self.columns.into()
    }
}

/// UI-thread-owned emulator. SSH workers deliver bytes; no local PTY is spawned.
pub struct Engine {
    pub(crate) terminal: Term<Listener>,
    parser: Processor,
    queries: crate::queries::Queries,
    extensions: Extensions,
    listener: Listener,
    theme: Theme,
    size: Size,
    renderer: crate::frame::Renderer,
    input: crate::input::InputState,
    pub(crate) selection: crate::selection::SelectionState,
}
impl Engine {
    pub fn new(columns: u16, rows: u16) -> Result<Self> {
        validate(columns, rows)?;
        let size = Size {
            columns,
            rows,
            width: 8,
            height: 16,
        };
        let listener = Listener::default();
        let terminal = Term::new(config(true), &size, listener.clone());
        Ok(Self {
            terminal,
            parser: Processor::new(),
            queries: Default::default(),
            extensions: Default::default(),
            listener,
            theme: Theme::default(),
            size,
            renderer: crate::frame::Renderer::default(),
            input: crate::input::InputState::new(columns, rows)?,
            selection: Default::default(),
        })
    }
    pub fn set_theme(&mut self, theme: Theme) -> Result<()> {
        if self.theme != theme {
            self.theme = theme;
            self.renderer.invalidate();
        }
        Ok(())
    }
    pub fn advance(&mut self, bytes: &[u8]) -> Result<Effects> {
        let mut rest = bytes;
        while !rest.is_empty() {
            let (read, query) = self.queries.next(rest);
            let mut handler = Handler {
                term: &mut self.terminal,
                ext: &mut self.extensions,
                listener: &self.listener,
            };
            self.parser.advance(&mut handler, &rest[..read]);
            if let Some(query) = query {
                self.listener
                    .send_event(Event::PtyWrite(query.reply(&self.terminal)));
            }
            rest = &rest[read..];
        }
        self.take_redraw();
        Ok(self.effects(!bytes.is_empty()))
    }
    fn take_redraw(&mut self) {
        if std::mem::take(&mut self.extensions.redraw) {
            self.renderer.invalidate();
        }
    }
    fn effects(&self, redraw: bool) -> Effects {
        let mut effects = Effects {
            redraw,
            ..Effects::default()
        };
        for event in self.listener.0.borrow_mut().drain(..) {
            match event {
                Event::Title(title) => effects.title = Some(Some(sanitize_title(&title))),
                Event::ResetTitle => effects.title = Some(None),
                Event::Bell => effects.bell = true,
                Event::PtyWrite(text) => effects.replies.push(text.into_bytes()),
                Event::ClipboardStore(ClipboardType::Clipboard, text)
                    if !text.is_empty() && text.len() <= 8 << 20 =>
                {
                    effects.copy = Some(text)
                }
                Event::ColorRequest(index, format) => effects.replies.push(
                    format(crate::frame::color_at(&self.terminal, &self.theme, index)).into_bytes(),
                ),
                Event::TextAreaSizeRequest(format) => effects.replies.push(
                    format(WindowSize {
                        num_cols: self.size.columns,
                        num_lines: self.size.rows,
                        cell_width: self.size.width,
                        cell_height: self.size.height,
                    })
                    .into_bytes(),
                ),
                _ => {}
            }
        }
        effects
    }
    pub fn flush_expired(&mut self) -> Result<Effects> {
        self.flush_expired_at(Instant::now())
    }
    fn flush_expired_at(&mut self, now: Instant) -> Result<Effects> {
        let expired = self
            .parser
            .sync_timeout()
            .sync_timeout()
            .is_some_and(|deadline| now >= deadline);
        if expired {
            let mut handler = Handler {
                term: &mut self.terminal,
                ext: &mut self.extensions,
                listener: &self.listener,
            };
            self.parser.stop_sync(&mut handler);
            self.take_redraw();
        }
        Ok(self.effects(expired))
    }
    pub fn frame(&mut self) -> Result<&Frame> {
        // The caller drains timeout effects via flush_expired; snapshotting never
        // consumes replies, clipboard writes, or titles from a synchronized batch.
        if self.extensions.modes.reverse_video {
            let mut theme = self.theme.clone();
            std::mem::swap(&mut theme.foreground, &mut theme.background);
            self.renderer.update(&mut self.terminal, &theme);
        } else {
            self.renderer.update(&mut self.terminal, &self.theme);
        }
        Ok(self.renderer.frame.as_ref().expect("frame initialized"))
    }
    pub fn resize(
        &mut self,
        columns: u16,
        rows: u16,
        cell_width: u16,
        cell_height: u16,
    ) -> Result<Effects> {
        validate(columns, rows)?;
        if cell_width == 0 || cell_height == 0 {
            return Err(Error::InvalidValue);
        }
        let size = Size {
            columns,
            rows,
            width: cell_width,
            height: cell_height,
        };
        let primary = !self.alternate_screen()?;
        let shrink = usize::from(rows) < self.terminal.screen_lines();
        // Grow first and shrink last, so the reflow has the most rows to use.
        let reflow_rows = if shrink {
            self.terminal.screen_lines() as u16
        } else {
            self.terminal.resize(Size {
                columns: self.terminal.columns() as u16,
                ..size
            });
            rows
        };
        if usize::from(columns) != self.terminal.columns() {
            // Reflow, keeping the cursor on its text.
            let anchor = primary
                .then(|| crate::reflow::anchor(self.terminal.grid()))
                .flatten();
            let reflow_size = Size {
                rows: reflow_rows,
                ..size
            };
            self.terminal.resize(reflow_size);
            if let Some(anchor) = anchor {
                self.restore_anchor(&anchor, reflow_size);
            }
        }
        if shrink && primary {
            self.shrink_primary(size);
        } else {
            self.terminal.resize(size);
        }
        self.size = size;
        self.extensions.end_cluster();
        self.input.resize(columns, rows, cell_width, cell_height);
        self.renderer.invalidate();
        Ok(self.effects(true))
    }
    /// Put the cursor back on its text after a reflow. When the reflow pushed
    /// that text into history, bring it back over blank rows at the bottom.
    fn restore_anchor(&mut self, anchor: &crate::reflow::Anchor, size: Size) {
        let Some(start) = crate::reflow::anchored_line(self.terminal.grid(), anchor) else {
            return;
        };
        let pull = (-start.0).max(0) as usize;
        let pull = pull.min(crate::reflow::blank_rows_at_bottom(self.terminal.grid()));
        if pull > 0 {
            // Dropping blank bottom rows and growing again pulls history back.
            self.terminal.grid_mut().cursor.point.line = Line(0);
            self.terminal.resize(Size {
                rows: size.rows - pull as u16,
                ..size
            });
            self.terminal.resize(size);
        }
        if let Some(start) = crate::reflow::anchored_line(self.terminal.grid(), anchor) {
            crate::reflow::place(self.terminal.grid_mut(), start, anchor);
        }
    }
    /// Shrink the primary screen without dropping output below the cursor:
    /// rows that no longer fit move into history. The cursor and saved cursor
    /// keep pointing at their text, or the top row once it is in history.
    fn shrink_primary(&mut self, size: Size) {
        let rows = size.rows as i32;
        let grid = self.terminal.grid();
        let cursor = grid.cursor.point.line.0;
        let saved = grid.saved_cursor.point.line.0;
        let last_output = (cursor..grid.screen_lines() as i32)
            .rev()
            .find(|&line| !grid[Line(line)].is_clear())
            .unwrap_or(cursor);
        let scrolled = (last_output + 1 - rows).max(0);
        self.terminal.grid_mut().cursor.point.line = Line(last_output);
        self.terminal.resize(size);
        let grid = self.terminal.grid_mut();
        grid.cursor.point.line = Line((cursor - scrolled).max(0));
        grid.saved_cursor.point.line = Line((saved - scrolled).clamp(0, rows - 1));
    }
    pub fn scroll(&mut self, scroll: ScrollViewport) {
        let scroll = match scroll {
            ScrollViewport::Top => Scroll::Top,
            ScrollViewport::Bottom => Scroll::Bottom,
            ScrollViewport::Delta(delta) => Scroll::Delta(
                delta
                    .saturating_neg()
                    .clamp(i32::MIN as isize, i32::MAX as isize) as i32,
            ),
            ScrollViewport::Row(row) => Scroll::Delta(
                (self.terminal.history_size().saturating_sub(row) as i64
                    - self.terminal.grid().display_offset() as i64)
                    .clamp(i32::MIN as i64, i32::MAX as i64) as i32,
            ),
        };
        self.terminal.scroll_display(scroll);
    }
    pub fn viewport_top(&self) -> Result<usize> {
        Ok(self.terminal.history_size() - self.terminal.grid().display_offset())
    }
    pub fn mode(&self, mode: Mode) -> Result<bool> {
        let flag = match mode {
            Mode::FOCUS_EVENT => TermMode::FOCUS_IN_OUT,
            Mode::BRACKETED_PASTE => TermMode::BRACKETED_PASTE,
            Mode::DECCKM => TermMode::APP_CURSOR,
            Mode::ALT_SCROLL => TermMode::ALTERNATE_SCROLL,
            Mode::X10_MOUSE => return Ok(self.extensions.modes.x10_mouse),
            Mode::NORMAL_MOUSE => TermMode::MOUSE_REPORT_CLICK,
            Mode::BUTTON_MOUSE => TermMode::MOUSE_DRAG,
            Mode::ANY_MOUSE => TermMode::MOUSE_MOTION,
        };
        Ok(self.terminal.mode().contains(flag))
    }
    pub fn kitty_keyboard_flags(&self) -> Result<crate::KittyKeyFlags> {
        Ok(crate::input::kitty_flags(*self.terminal.mode()))
    }
    pub fn alternate_screen(&self) -> Result<bool> {
        Ok(self.terminal.mode().contains(TermMode::ALT_SCREEN))
    }
    pub fn cursor_position(&self) -> Result<PointCoordinate> {
        let p = self.terminal.grid().cursor.point;
        Ok(PointCoordinate {
            x: p.column.0 as u16,
            y: p.line.0 as u32,
        })
    }
    pub fn reveal_position(&mut self, position: crate::Position) -> Result<()> {
        let row = position.row as usize;
        let top = self.viewport_top()?;
        let height = self.size.rows as usize;
        if row < top {
            self.scroll(ScrollViewport::Row(row));
        } else if row >= top + height {
            self.scroll(ScrollViewport::Row(row + 1 - height));
        }
        Ok(())
    }
    pub fn visible_lines(&mut self) -> Result<Vec<String>> {
        Ok(self.frame()?.visible_lines())
    }
    pub fn search(&mut self, query: &str, match_case: bool) -> Result<crate::SearchResults> {
        Ok(crate::search::search(&self.terminal, query, match_case))
    }
    pub fn history_rows(&mut self) -> Result<Vec<crate::Row>> {
        Ok(
            (-(self.terminal.history_size() as i32)..self.terminal.screen_lines() as i32)
                .map(|y| crate::frame::row(&self.terminal, &self.theme, Line(y), None))
                .collect(),
        )
    }
    pub fn encode_key(&mut self, event: crate::KeyInput<'_>) -> Result<Vec<u8>> {
        self.input
            .key(*self.terminal.mode(), &self.extensions.modes, event)
    }
    pub fn encode_mouse(&mut self, event: crate::MouseInput) -> Result<Vec<u8>> {
        self.input
            .mouse(*self.terminal.mode(), &self.extensions.modes, event)
    }
    pub fn encode_paste(&self, text: &str) -> Result<Vec<u8>> {
        let bracketed = self.mode(Mode::BRACKETED_PASTE)?;
        let text = if bracketed {
            text.to_owned()
        } else {
            text.replace("\r\n", "\n")
        };
        let text: String = text
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\t' | '\n' | '\r'))
            .map(|c| if !bracketed && c == '\n' { '\r' } else { c })
            .collect();
        Ok(if bracketed {
            format!("\x1b[200~{text}\x1b[201~").into_bytes()
        } else {
            text.into_bytes()
        })
    }
    pub fn encode_focus(&self, focused: bool) -> Result<Vec<u8>> {
        Ok(if self.mode(Mode::FOCUS_EVENT)? {
            if focused {
                b"\x1b[I".to_vec()
            } else {
                b"\x1b[O".to_vec()
            }
        } else {
            Vec::new()
        })
    }
    pub fn reconnect(&mut self) -> Result<Effects> {
        // Discard an incomplete sequence and synchronized batch from the old peer.
        self.parser = Processor::new();
        self.queries = Default::default();
        self.extensions.reset();
        self.input.reset();
        // Alacritty keeps an independent keyboard stack for each screen. Use
        // its public configuration API to reset both, including the inactive one.
        self.terminal.set_options(config(false));
        self.terminal.set_options(config(true));
        self.clear_selection()?;
        self.advance(b"\x1b[?1049l\x1b[r\x1b[0m\x1b[?25h\x1b[?2004l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1005l\x1b[?1004l\x1b[?1l\x1b>\x1b[4l\x1b[?6l\x1b[?7h\x1b[<999u\x1b[=0u")
    }
    pub(crate) fn point(&self, point: Point) -> Result<GridPoint> {
        let (p, offset) = match point {
            Point::Active(p) => (p, 0),
            Point::Viewport(p) => (p, self.terminal.grid().display_offset()),
            Point::Screen(p) | Point::History(p) => (p, self.terminal.history_size()),
        };
        let line = p.y as i64 - offset as i64;
        if p.x as usize >= self.terminal.columns()
            || line < -(self.terminal.history_size() as i64)
            || line >= self.terminal.screen_lines() as i64
        {
            return Err(Error::InvalidValue);
        }
        Ok(GridPoint::new(Line(line as i32), Column(p.x as usize)))
    }
    pub fn link_at(&self, point: Point) -> Result<Option<String>> {
        let Ok(mut p) = self.point(point) else {
            return Ok(None);
        };
        use alacritty_terminal::term::cell::Flags;
        let cell = &self.terminal.grid()[p];
        if cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
            return Ok(None);
        }
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            p.column.0 = p.column.0.saturating_sub(1);
        }
        let cell = &self.terminal.grid()[p];
        Ok((!cell.flags.contains(Flags::HIDDEN))
            .then(|| cell.hyperlink())
            .flatten()
            .and_then(|link| newport_domain::external_link::http_url(link.uri())))
    }
}
fn config(kitty_keyboard: bool) -> Config {
    Config {
        scrolling_history: 5000,
        kitty_keyboard,
        osc52: Osc52::OnlyCopy,
        semantic_escape_chars: " ()[]{}'\",`".into(),
        ..Config::default()
    }
}

fn validate(columns: u16, rows: u16) -> Result<()> {
    if (2..=500).contains(&columns) && (1..=300).contains(&rows) {
        Ok(())
    } else {
        Err(Error::InvalidValue)
    }
}
fn sanitize_title(value: &str) -> String {
    let value: String = value
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(*c as u32,0x061c|0x200e|0x200f|0x202a..=0x202e|0x2066..=0x2069)
        })
        .collect();
    let mut length = 0;
    value
        .trim()
        .chars()
        .take_while(|c| {
            length += c.len_utf16();
            length <= 120
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn synchronized_output_captures_before_hold_and_times_out() {
        let mut engine = Engine::new(20, 3).unwrap();
        engine.advance(b"first\x1b[?2026h\rsecond").unwrap();
        assert_eq!(engine.visible_lines().unwrap(), ["first"]);
        assert!(
            engine
                .flush_expired_at(Instant::now() + std::time::Duration::from_secs(2))
                .unwrap()
                .redraw
        );
        assert_eq!(engine.visible_lines().unwrap(), ["second"]);
    }
}
