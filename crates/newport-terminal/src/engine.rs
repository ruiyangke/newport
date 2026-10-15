use crate::{Frame, Palette, Result, RgbColor, frame::Renderer};
use libghostty_vt::{
    Terminal,
    selection::{FormatOptions, SelectLineOptions, SelectWordOptions, Selection},
    terminal::{Mode, Point, ScrollViewport},
};
use std::{
    cell::{Cell, Ref, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};

/// Host-provided colors, kept separate from application OSC overrides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    pub foreground: RgbColor,
    pub background: RgbColor,
    pub cursor: RgbColor,
    pub palette: [RgbColor; 256],
}

#[derive(Default, Debug)]
pub struct Effects {
    pub redraw: bool,
    pub title: Option<Option<String>>,
    pub replies: Vec<Vec<u8>>,
    pub bell: bool,
}

struct Shared {
    renderer: RefCell<Renderer>,
    effects: RefCell<Effects>,
    hold: Cell<Option<Instant>>,
    callback_error: Cell<Option<libghostty_vt::Error>>,
}

/// UI-thread-confined terminal state. Ghostty and its callbacks are !Send;
/// SSH worker threads pass bytes to this object rather than sharing it.
pub struct Engine {
    terminal: Terminal<'static, 'static>,
    shared: Rc<Shared>,
    theme: Option<Theme>,
    input: crate::input::InputState,
    selection: crate::selection::SelectionState,
}

impl Engine {
    pub fn new(columns: u16, rows: u16) -> Result<Self> {
        validate(columns, rows)?;
        let shared = Rc::new(Shared {
            renderer: RefCell::new(Renderer::new()?),
            effects: RefCell::new(Effects::default()),
            hold: Cell::new(None),
            callback_error: Cell::new(None),
        });
        let mut terminal = Terminal::new(columns, rows)?;
        terminal.set_scrollback_max_lines(Some(5000))?;
        // Ghostty's small default byte budget would prune normal output long
        // before Newport's line limit. The line limit owns retention here.
        terminal.set_scrollback_max_bytes(None)?;
        terminal.set_default_mode(Mode::GRAPHEME_CLUSTER, true)?;
        terminal.on_pty_write({
            let shared = shared.clone();
            move |_, bytes| {
                if let Ok(mut effects) = shared.effects.try_borrow_mut() {
                    effects.replies.push(bytes.to_vec());
                }
            }
        })?;
        terminal.on_bell({
            let shared = shared.clone();
            move |_| {
                if let Ok(mut effects) = shared.effects.try_borrow_mut() {
                    effects.bell = true;
                }
            }
        })?;
        terminal.on_title_changed({
            let shared = shared.clone();
            move |terminal| match terminal.title() {
                Ok(value) => {
                    if let Ok(mut effects) = shared.effects.try_borrow_mut() {
                        effects.title = Some(Some(sanitize_title(value)));
                    }
                }
                Err(error) => shared.callback_error.set(Some(error)),
            }
        })?;
        terminal.on_render_hold({
            let shared = shared.clone();
            move |terminal, held| {
                if held {
                    match shared.renderer.try_borrow_mut() {
                        Ok(mut renderer) => {
                            if let Err(error) = renderer.update(terminal) {
                                shared.callback_error.set(Some(error));
                            }
                        }
                        Err(_) => shared
                            .callback_error
                            .set(Some(libghostty_vt::Error::InvalidValue)),
                    }
                    shared.hold.set(Some(Instant::now()));
                } else {
                    shared.hold.set(None);
                }
            }
        })?;
        // No clipboard handlers: remote OSC must never read or modify the
        // host clipboard. The UI handles explicit copy/paste separately.
        shared.renderer.borrow_mut().update(&terminal)?;
        Ok(Self {
            terminal,
            shared,
            theme: None,
            input: crate::input::InputState::new(columns, rows)?,
            selection: crate::selection::SelectionState::new()?,
        })
    }

    pub fn set_theme(&mut self, theme: Theme) -> Result<()> {
        if self.theme.as_ref() == Some(&theme) {
            return Ok(());
        }
        self.terminal
            .set_default_fg_color(Some(theme.foreground))?
            .set_default_bg_color(Some(theme.background))?
            .set_default_cursor_color(Some(theme.cursor))?
            .set_default_color_palette(Some(Palette(theme.palette)))?;
        self.theme = Some(theme);
        Ok(())
    }

    pub fn advance(&mut self, bytes: &[u8]) -> Result<Effects> {
        self.terminal.vt_write(bytes);
        self.check_callback_error()?;
        let mut effects = std::mem::take(&mut *self.shared.effects.borrow_mut());
        // A hold may have captured a newer completed frame even if a later
        // hold is still active when this write finishes.
        effects.redraw = !bytes.is_empty();
        Ok(effects)
    }

    pub fn flush_expired(&mut self) -> Result<Effects> {
        self.flush_expired_at(Instant::now())
    }

    fn flush_expired_at(&mut self, now: Instant) -> Result<Effects> {
        let expired =
            self.shared.hold.get().is_some_and(|start| {
                now.saturating_duration_since(start) >= Duration::from_secs(1)
            });
        if expired {
            self.terminal.set_mode(Mode::SYNC_OUTPUT, false)?;
            self.shared.hold.set(None);
        }
        Ok(Effects {
            redraw: expired,
            ..Effects::default()
        })
    }

    pub fn frame(&mut self) -> Result<Ref<'_, Frame>> {
        self.flush_expired()?;
        if self.shared.hold.get().is_none() {
            self.shared.renderer.borrow_mut().update(&self.terminal)?;
        }
        Ok(Ref::map(self.shared.renderer.borrow(), |renderer| {
            renderer.frame.as_ref().expect("initial frame exists")
        }))
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
            return Err(libghostty_vt::Error::InvalidValue);
        }
        self.terminal
            .resize(columns, rows, u32::from(cell_width), u32::from(cell_height))?;
        self.input.resize(columns, rows, cell_width, cell_height);
        self.check_callback_error()?;
        let mut effects = std::mem::take(&mut *self.shared.effects.borrow_mut());
        effects.redraw = true;
        Ok(effects)
    }

    pub fn scroll(&mut self, scroll: ScrollViewport) {
        self.terminal.scroll_viewport(scroll);
    }

    pub fn viewport_top(&self) -> Result<usize> {
        self.terminal
            .scrollbar()?
            .offset
            .try_into()
            .map_err(|_| crate::Error::LimitExceeded)
    }

    pub fn mode(&self, mode: Mode) -> Result<bool> {
        self.terminal.mode(mode)
    }

    pub fn kitty_keyboard_flags(&self) -> Result<crate::KittyKeyFlags> {
        self.terminal.kitty_keyboard_flags()
    }

    pub fn alternate_screen(&self) -> Result<bool> {
        Ok(self.terminal.active_screen()? == libghostty_vt::screen::Screen::Alternate)
    }

    pub fn cursor_position(&self) -> Result<crate::PointCoordinate> {
        Ok(crate::PointCoordinate {
            x: self.terminal.cursor_x()?,
            y: u32::from(self.terminal.cursor_y()?),
        })
    }

    pub fn reveal_position(&mut self, position: crate::Position) -> Result<()> {
        let row = position.row as usize;
        let top = self.viewport_top()?;
        let height = usize::from(self.terminal.rows()?);
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

    /// Search retained output without moving the viewport or changing selection.
    pub fn search(&mut self, query: &str, match_case: bool) -> Result<crate::SearchResults> {
        self.shared
            .renderer
            .borrow_mut()
            .search(&self.terminal, query, match_case)
    }

    /// Owned retained rows for one-shot command-log conversion, including history.
    pub fn history_rows(&mut self) -> Result<Vec<crate::Row>> {
        self.shared
            .renderer
            .borrow_mut()
            .history_rows(&self.terminal)
    }

    pub fn encode_key(&mut self, event: crate::KeyInput<'_>) -> Result<Vec<u8>> {
        self.input.key(&self.terminal, event)
    }

    pub fn encode_mouse(&mut self, event: crate::MouseInput) -> Result<Vec<u8>> {
        self.input.mouse(&self.terminal, event)
    }

    /// The caller invokes this only for an explicit user paste. Strip control
    /// bytes before encoding so a pasted escape cannot terminate paste mode.
    pub fn encode_paste(&self, text: &str) -> Result<Vec<u8>> {
        let bracketed = self.terminal.mode(Mode::BRACKETED_PASTE)?;
        // In legacy mode Ghostty translates every LF to CR. Collapse CRLF
        // first so Windows clipboard newlines do not submit two shell lines.
        let mut text = if bracketed {
            text.as_bytes().to_vec()
        } else {
            text.replace("\r\n", "\n").into_bytes()
        };
        let mut output = vec![
            0;
            text.len()
                .checked_add(12)
                .ok_or(libghostty_vt::Error::LimitExceeded)?
        ];
        let length = libghostty_vt::paste::encode(&mut text, bracketed, &mut output)?;
        output.truncate(length);
        Ok(output)
    }

    pub fn encode_focus(&self, focused: bool) -> Result<Vec<u8>> {
        if !self.terminal.mode(Mode::FOCUS_EVENT)? {
            return Ok(Vec::new());
        }
        let event = if focused {
            libghostty_vt::focus::Event::Gained
        } else {
            libghostty_vt::focus::Event::Lost
        };
        let mut output = vec![0; 3];
        let length = event.encode(&mut output)?;
        output.truncate(length);
        Ok(output)
    }

    /// A new SSH session must not inherit the disconnected program's parser,
    /// alternate screen, keyboard, mouse, or paste modes. CAN aborts a partial
    /// control sequence without clearing the retained primary-screen output.
    pub fn reconnect(&mut self) -> Result<Effects> {
        self.input.reset();
        self.clear_selection()?;
        self.advance(b"\x18\x1b[?2026l\x1b[?1049l\x1b[r\x1b[0m\x1b[?25h\x1b[?2004l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1004l\x1b[?1l\x1b[4l\x1b[?6l\x1b[?7h\x1b[<999u\x1b[=0u")
    }

    pub fn clear_selection(&mut self) -> Result<()> {
        self.selection.reset(&self.terminal);
        self.terminal.set_selection(None)?;
        Ok(())
    }

    pub fn begin_selection(
        &mut self,
        position: crate::SelectionPosition,
        click_count: usize,
        extend: bool,
        rectangle: bool,
    ) -> Result<()> {
        self.selection
            .begin(&self.terminal, position, click_count, extend, rectangle)
    }

    pub fn update_selection(&mut self, position: crate::SelectionPosition) -> Result<()> {
        self.selection.update(&self.terminal, position)
    }

    pub fn end_selection(&mut self, position: Option<crate::SelectionPosition>) -> Result<()> {
        self.selection.end(&self.terminal, position)
    }

    pub fn select_range(&mut self, start: Point, end: Point, rectangle: bool) -> Result<()> {
        self.selection.reset(&self.terminal);
        let selection = Selection::new(
            self.terminal.grid_ref(start)?,
            self.terminal.grid_ref(end)?,
            rectangle,
        );
        self.terminal.set_selection(Some(&selection))?;
        Ok(())
    }

    pub fn select_word(&mut self, point: Point) -> Result<()> {
        self.selection.reset(&self.terminal);
        let boundary: Vec<char> = " ()[]{}'\",`".chars().collect();
        let selection = self.terminal.select_word(
            SelectWordOptions::new(self.terminal.grid_ref(point)?)
                .with_boundary_codepoints(&boundary),
        )?;
        self.terminal.set_selection(selection.as_ref())?;
        Ok(())
    }

    pub fn select_line(&mut self, point: Point) -> Result<()> {
        self.selection.reset(&self.terminal);
        self.selection.append_newline = true;
        let selection = self
            .terminal
            .select_line(SelectLineOptions::new(self.terminal.grid_ref(point)?))?;
        self.terminal.set_selection(selection.as_ref())?;
        Ok(())
    }

    pub fn select_all(&mut self) -> Result<()> {
        self.selection.reset(&self.terminal);
        self.selection.append_newline = true;
        let selection = self.terminal.select_all()?;
        self.terminal.set_selection(selection.as_ref())?;
        Ok(())
    }

    pub fn selected_text(&self) -> Result<Option<String>> {
        let formatted = self
            .terminal
            .format_selection_alloc(None, FormatOptions::new().with_unwrap(true).with_trim(true))?;
        formatted
            .map(|bytes| {
                let mut text = String::from_utf8(bytes.to_vec())
                    .map_err(|_| libghostty_vt::Error::InvalidValue)?;
                // Whole-line clipboard gestures include their final line break,
                // as in the original terminal. Ghostty's plain formatter trims
                // trailing newlines even when a complete line was selected.
                if self.selection.append_newline && !text.ends_with('\n') {
                    text.push('\n');
                }
                Ok(text)
            })
            .transpose()
    }

    /// Resolve OSC 8 links only on visible, non-concealed text. URL policy is
    /// shared with the backend; escape sequences never open a URL themselves.
    pub fn link_at(&self, point: Point) -> Result<Option<String>> {
        let mut grid = match self.terminal.grid_ref(point) {
            Ok(grid) => grid,
            Err(libghostty_vt::Error::InvalidValue) => return Ok(None),
            Err(error) => return Err(error),
        };
        match grid.cell()?.wide()? {
            crate::CellWide::SpacerHead => return Ok(None),
            crate::CellWide::SpacerTail => {
                let Some(point) = previous_column(point) else {
                    return Ok(None);
                };
                grid = self.terminal.grid_ref(point)?;
            }
            _ => {}
        }
        if grid.style()?.invisible || !grid.cell()?.has_hyperlink()? {
            return Ok(None);
        }
        let mut bytes = vec![0; 256];
        let length = match grid.hyperlink_uri(&mut bytes) {
            Ok(length) => length,
            Err(libghostty_vt::Error::OutOfSpace { required }) => {
                bytes.resize(required, 0);
                grid.hyperlink_uri(&mut bytes)?
            }
            Err(error) => return Err(error),
        };
        let uri = std::str::from_utf8(&bytes[..length])
            .map_err(|_| libghostty_vt::Error::InvalidValue)?;
        Ok(newport_domain::external_link::http_url(uri))
    }

    fn check_callback_error(&self) -> Result<()> {
        self.shared.callback_error.take().map_or(Ok(()), Err)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.selection.reset(&self.terminal);
    }
}

fn previous_column(point: Point) -> Option<Point> {
    let (mut coordinate, space): (_, fn(libghostty_vt::terminal::PointCoordinate) -> Point) =
        match point {
            Point::Active(value) => (value, Point::Active),
            Point::Viewport(value) => (value, Point::Viewport),
            Point::Screen(value) => (value, Point::Screen),
            Point::History(value) => (value, Point::History),
        };
    coordinate.x = coordinate.x.checked_sub(1)?;
    Some(space(coordinate))
}

fn validate(columns: u16, rows: u16) -> Result<()> {
    if (2..=500).contains(&columns) && (1..=300).contains(&rows) {
        Ok(())
    } else {
        Err(libghostty_vt::Error::InvalidValue)
    }
}

fn sanitize_title(value: &str) -> String {
    let sanitized: String = value.chars().filter(|c| !c.is_control() && !matches!(*c as u32, 0x061c | 0x200e | 0x200f | 0x202a..=0x202e | 0x2066..=0x2069)).collect();
    let mut length = 0;
    sanitized
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
                .flush_expired_at(Instant::now() + Duration::from_secs(2))
                .unwrap()
                .redraw
        );
        assert_eq!(engine.visible_lines().unwrap(), ["second"]);
    }
}
