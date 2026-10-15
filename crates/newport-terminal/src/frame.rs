use libghostty_vt::{
    RenderState, Terminal,
    error::Result,
    render::{CellIterator, Cursor, Dirty, RowIterator},
    screen::CellWide,
    style::{RgbColor, Style},
};

/// Owned visual cell data. A continuation cell has no text to paint.
#[derive(Clone, Debug)]
pub struct Cell {
    pub text: String,
    pub wide: CellWide,
    pub style: Style,
    pub foreground: Option<RgbColor>,
    pub background: Option<RgbColor>,
    pub selected: bool,
    pub hyperlink: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Row {
    pub cells: Vec<Cell>,
    pub wrapped: bool,
}

/// A viewport snapshot contains no borrowed Ghostty memory.
#[derive(Clone, Debug)]
pub struct Frame {
    pub columns: u16,
    pub rows: Vec<Row>,
    pub cursor: Cursor,
    pub foreground: RgbColor,
    pub background: RgbColor,
    pub cursor_color: Option<RgbColor>,
    pub palette: [RgbColor; 256],
    pub scrollback_rows: usize,
    /// Viewport top measured from the start of retained history.
    pub viewport_top: usize,
}

impl Frame {
    /// Text for accessibility. Concealed text and wide tails are not exposed.
    pub fn visible_lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .rows
            .iter()
            .map(|row| {
                let mut text = String::new();
                for cell in &row.cells {
                    match cell.wide {
                        CellWide::SpacerTail | CellWide::SpacerHead => continue,
                        _ => {}
                    }
                    if cell.style.invisible {
                        text.push(' ');
                        if cell.wide == CellWide::Wide {
                            text.push(' ');
                        }
                    } else if cell.text.is_empty() {
                        text.push(' ');
                    } else {
                        text.push_str(&cell.text);
                    }
                }
                text.trim_end_matches(' ').to_owned()
            })
            .collect();
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines
    }
}

/// Reuses the Ghostty render state and updates only dirty rows. Also used from
/// the synchronized-output callback to capture the exact pre-hold frame.
pub(crate) struct Renderer {
    pub(crate) state: RenderState<'static>,
    pub(crate) rows: RowIterator<'static>,
    pub(crate) cells: CellIterator<'static>,
    pub(crate) force_redraw: bool,
    pub frame: Option<Frame>,
}

impl Renderer {
    pub fn new() -> Result<Self> {
        Ok(Self {
            state: RenderState::new()?,
            rows: RowIterator::new()?,
            cells: CellIterator::new()?,
            force_redraw: false,
            frame: None,
        })
    }

    pub fn update(&mut self, terminal: &Terminal<'static, '_>) -> Result<()> {
        let snapshot = self.state.update(terminal)?;
        let columns = snapshot.cols()?;
        let height = usize::from(snapshot.rows()?);
        let resize = self
            .frame
            .as_ref()
            .is_none_or(|f| f.columns != columns || f.rows.len() != height);
        if resize || self.force_redraw {
            snapshot.set_dirty(Dirty::Full)?;
        }
        let scrollbar = terminal.scrollbar()?;
        let frame = self.frame.get_or_insert(Frame {
            columns,
            rows: Vec::new(),
            cursor: snapshot.cursor()?,
            foreground: RgbColor::default(),
            background: RgbColor::default(),
            cursor_color: None,
            palette: [RgbColor::default(); 256],
            scrollback_rows: 0,
            viewport_top: 0,
        });
        frame.columns = columns;
        frame.rows.resize_with(height, Row::default);
        frame.cursor = snapshot.cursor()?;
        let colors = snapshot.colors()?;
        frame.foreground = colors.foreground;
        frame.background = colors.background;
        frame.cursor_color = colors.cursor;
        frame.palette = colors.palette;
        frame.scrollback_rows = terminal.scrollback_rows()?;
        frame.viewport_top = scrollbar
            .offset
            .try_into()
            .map_err(|_| libghostty_vt::Error::InvalidValue)?;
        let mut rows = self.rows.update(&snapshot)?;
        while let Some((y, row)) = rows.next_dirty() {
            let target = &mut frame.rows[usize::from(y)];
            update_row(
                target,
                row,
                &mut self.cells,
                terminal,
                u32::from(y),
                crate::Point::Viewport,
            )?;
        }
        snapshot.clean()?;
        self.force_redraw = false;
        Ok(())
    }
    pub fn history_rows(&mut self, terminal: &Terminal<'static, '_>) -> Result<Vec<Row>> {
        let previous = self.state.overscan_request()?;
        let history = terminal
            .scrollback_rows()?
            .try_into()
            .map_err(|_| crate::Error::LimitExceeded)?;
        self.force_redraw = true;
        self.state.set_overscan(libghostty_vt::render::Overscan {
            above: history,
            below: history,
        })?;
        let result = (|| {
            let snapshot = self.state.update(terminal)?;
            let top = terminal.scrollbar()?.offset as i64;
            let mut rows = self.rows.update(&snapshot)?;
            let mut output = Vec::new();
            while let Some(row) = rows.next() {
                let y = u32::try_from(top + i64::from(row.viewport_y()?))
                    .map_err(|_| crate::Error::InvalidValue)?;
                let mut target = Row::default();
                update_row(
                    &mut target,
                    row,
                    &mut self.cells,
                    terminal,
                    y,
                    crate::Point::Screen,
                )?;
                output.push(target);
            }
            Ok(output)
        })();
        self.state.set_overscan(previous)?;
        result
    }
}

fn update_row(
    target: &mut Row,
    row: &libghostty_vt::render::RowIteration<'static, '_>,
    iterator: &mut CellIterator<'static>,
    terminal: &Terminal<'static, '_>,
    y: u32,
    space: fn(crate::PointCoordinate) -> crate::Point,
) -> Result<()> {
    target.wrapped = row.raw_row()?.is_wrapped()?;
    let mut cells = iterator.update(row)?;
    let mut x = 0;
    while let Some(cell) = cells.next() {
        let raw = cell.raw_cell()?;
        if x == target.cells.len() {
            target.cells.push(Cell {
                text: String::new(),
                wide: raw.wide()?,
                style: Style::default(),
                foreground: None,
                background: None,
                selected: false,
                hyperlink: None,
            });
        }
        let target = &mut target.cells[x];
        cell.graphemes_utf8(&mut target.text)?;
        target.wide = raw.wide()?;
        target.style = cell.style()?;
        target.foreground = cell.fg_color()?;
        target.background = cell.bg_color()?;
        // Erased/blank cells can store their background inline instead
        // of in a style record. Preserve that role for all renderers.
        if target.style.bg_color == crate::StyleColor::None
            && let Some(background) = target.background
        {
            target.style.bg_color = crate::StyleColor::Rgb(background);
        }
        target.selected = cell.is_selected()?;
        target.hyperlink = None;
        if raw.has_hyperlink()? && !target.style.invisible {
            let grid = terminal.grid_ref(space(crate::PointCoordinate { x: x as u16, y }))?;
            let mut uri = vec![0; 256];
            let length = match grid.hyperlink_uri(&mut uri) {
                Ok(length) => length,
                Err(crate::Error::OutOfSpace { required }) => {
                    uri.resize(required, 0);
                    grid.hyperlink_uri(&mut uri)?
                }
                Err(error) => return Err(error),
            };
            target.hyperlink = std::str::from_utf8(&uri[..length])
                .ok()
                .and_then(newport_domain::external_link::http_url);
        }
        x += 1;
    }
    target.cells.truncate(x);
    Ok(())
}
