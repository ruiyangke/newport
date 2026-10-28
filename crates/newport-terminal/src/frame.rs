use crate::{CellWide, Cursor, RgbColor, Style};
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

/// A viewport snapshot owns its cells independently of the emulator.
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
    pub(crate) detected_links: Vec<crate::links::DetectedLink>,
}

impl Frame {
    /// Resolve links from the same visible snapshot used for painting, including
    /// plain URLs emitted by applications that do not preserve OSC 8 metadata.
    pub fn link_at(&self, column: usize, row: usize) -> Option<&str> {
        let cells = &self.rows.get(row)?.cells;
        let cell = cells.get(column)?;
        let cell = if cell.wide == CellWide::SpacerTail {
            cells.get(column.checked_sub(1)?)?
        } else {
            cell
        };
        if cell.style.invisible || cell.wide == CellWide::SpacerHead {
            return None;
        }
        if let Some(url) = cell.hyperlink.as_deref() {
            return Some(url);
        }
        let offset = row * usize::from(self.columns) + column;
        let index = self
            .detected_links
            .partition_point(|link| *link.cells.end() < offset);
        self.detected_links
            .get(index)
            .filter(|link| link.cells.contains(&offset))
            .map(|link| link.url.as_str())
    }

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

use crate::engine::Listener;
use crate::{CursorPosition, CursorVisualStyle, PaletteIndex, StyleColor, Theme, Underline};
use alacritty_terminal::{
    Term,
    grid::Dimensions,
    index::{Column, Line, Point},
    selection::SelectionRange,
    term::{TermDamage, cell::Flags},
    vte::ansi::{Color, CursorShape, NamedColor},
};
#[derive(Default)]
pub(crate) struct Renderer {
    pub frame: Option<Frame>,
    invalid: bool,
}
impl Renderer {
    pub fn invalidate(&mut self) {
        self.invalid = true;
    }
    pub fn update(&mut self, term: &mut Term<Listener>, theme: &Theme) {
        let columns = term.columns() as u16;
        let height = term.screen_lines();
        let content = term.renderable_content();
        let display_offset = content.display_offset;
        let selection = content.selection;
        let cursor = content.cursor;
        let cursor_line = cursor.point.line.0 + display_offset as i32;
        let cursor = Cursor {
            visible: cursor.shape != CursorShape::Hidden,
            blinking: term.cursor_style().blinking,
            visual_style: match cursor.shape {
                CursorShape::Beam => CursorVisualStyle::Bar,
                CursorShape::Underline => CursorVisualStyle::Underline,
                CursorShape::HollowBlock => CursorVisualStyle::BlockHollow,
                _ => CursorVisualStyle::Block,
            },
            viewport: (cursor_line >= 0 && cursor_line < height as i32).then_some(CursorPosition {
                x: cursor.point.column.0 as u16,
                y: cursor_line as u16,
                at_wide_tail: false,
            }),
        };
        let mut dirty = vec![false; height];
        match term.damage() {
            TermDamage::Full => dirty.fill(true),
            TermDamage::Partial(lines) => {
                for line in lines {
                    if line.line < height {
                        dirty[line.line] = true;
                    }
                }
            }
        }
        if self.invalid
            || self
                .frame
                .as_ref()
                .is_none_or(|f| f.columns != columns || f.rows.len() != height)
        {
            dirty.fill(true);
        }
        // Selection changes are tracked separately by Alacritty's renderer.
        // Update the small selection mask each frame without reallocating text.
        let frame = self.frame.get_or_insert_with(|| Frame {
            columns,
            rows: Vec::new(),
            cursor,
            foreground: theme.foreground,
            background: theme.background,
            cursor_color: None,
            palette: theme.palette,
            scrollback_rows: 0,
            viewport_top: 0,
            detected_links: Vec::new(),
        });
        frame.columns = columns;
        frame.rows.resize_with(height, Row::default);
        for (y, dirty) in dirty.iter().enumerate() {
            let line = Line(y as i32 - display_offset as i32);
            if *dirty {
                frame.rows[y] = row(term, theme, line, selection.as_ref());
            } else {
                for (x, cell) in frame.rows[y].cells.iter_mut().enumerate() {
                    cell.selected = selection
                        .as_ref()
                        .is_some_and(|s| s.contains(Point::new(line, Column(x))));
                }
            }
        }
        frame.cursor = cursor;
        frame.foreground = color_at(term, theme, NamedColor::Foreground as usize);
        frame.background = color_at(term, theme, NamedColor::Background as usize);
        frame.cursor_color = Some(color_at(term, theme, NamedColor::Cursor as usize));
        frame.palette = std::array::from_fn(|i| color_at(term, theme, i));
        frame.scrollback_rows = term.history_size();
        frame.viewport_top = term.history_size() - display_offset;
        if dirty.iter().any(|d| *d) {
            frame.detected_links = crate::links::detect(&frame.rows, columns);
        }
        term.reset_damage();
        self.invalid = false;
    }
}
pub(crate) fn color_at(term: &Term<Listener>, theme: &Theme, index: usize) -> RgbColor {
    term.colors()[index].unwrap_or_else(|| match index {
        0..=255 => theme.palette[index],
        256 => theme.foreground,
        257 => theme.background,
        258 => theme.cursor,
        259..=266 => {
            let rgb = theme.palette[index - 259];
            RgbColor {
                r: (rgb.r as f32 * 0.66) as u8,
                g: (rgb.g as f32 * 0.66) as u8,
                b: (rgb.b as f32 * 0.66) as u8,
            }
        }
        _ => theme.foreground,
    })
}
fn style_color(color: Color) -> StyleColor {
    match color {
        Color::Spec(rgb) => StyleColor::Rgb(rgb),
        Color::Indexed(i) => StyleColor::Palette(PaletteIndex(i)),
        Color::Named(n) if (n as usize) < 256 => StyleColor::Palette(PaletteIndex(n as u8)),
        _ => StyleColor::None,
    }
}
fn resolved(term: &Term<Listener>, theme: &Theme, color: Color) -> Option<RgbColor> {
    match color {
        Color::Spec(rgb) => Some(rgb),
        Color::Indexed(i) => Some(color_at(term, theme, i as usize)),
        Color::Named(NamedColor::Foreground | NamedColor::Background) => None,
        Color::Named(n) => Some(color_at(term, theme, n as usize)),
    }
}
pub(crate) fn row(
    term: &Term<Listener>,
    theme: &Theme,
    line: Line,
    selection: Option<&SelectionRange>,
) -> Row {
    let cells = (0..term.columns())
        .map(|x| {
            let point = Point::new(line, Column(x));
            let cell = &term.grid()[point];
            let f = cell.flags;
            let wide = if f.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
                CellWide::SpacerHead
            } else if f.contains(Flags::WIDE_CHAR_SPACER) {
                CellWide::SpacerTail
            } else if f.contains(Flags::WIDE_CHAR) {
                CellWide::Wide
            } else {
                CellWide::Narrow
            };
            let text = if matches!(wide, CellWide::SpacerHead | CellWide::SpacerTail)
                || cell.c == ' ' && cell.zerowidth().is_none()
            {
                String::new()
            } else {
                std::iter::once(cell.c)
                    .chain(cell.zerowidth().unwrap_or_default().iter().copied())
                    .collect()
            };
            let underline = if f.contains(Flags::DOUBLE_UNDERLINE) {
                Underline::Double
            } else if f.contains(Flags::UNDERCURL) {
                Underline::Curly
            } else if f.contains(Flags::DOTTED_UNDERLINE) {
                Underline::Dotted
            } else if f.contains(Flags::DASHED_UNDERLINE) {
                Underline::Dashed
            } else if f.contains(Flags::UNDERLINE) {
                Underline::Single
            } else {
                Underline::None
            };
            let style = Style {
                bold: f.contains(Flags::BOLD),
                italic: f.contains(Flags::ITALIC),
                faint: f.contains(Flags::DIM),
                inverse: f.contains(Flags::INVERSE),
                invisible: f.contains(Flags::HIDDEN),
                strikethrough: f.contains(Flags::STRIKEOUT),
                underline,
                underline_color: cell.underline_color().map(style_color).unwrap_or_default(),
                fg_color: style_color(cell.fg),
                bg_color: style_color(cell.bg),
            };
            Cell {
                text,
                wide,
                style,
                foreground: resolved(term, theme, cell.fg),
                background: resolved(term, theme, cell.bg),
                selected: selection.is_some_and(|s| s.contains(point)),
                hyperlink: (!style.invisible)
                    .then(|| cell.hyperlink())
                    .flatten()
                    .and_then(|link| newport_domain::external_link::http_url(link.uri())),
            }
        })
        .collect();
    Row {
        cells,
        wrapped: term.grid()[line][Column(term.columns() - 1)]
            .flags
            .contains(Flags::WRAPLINE),
    }
}
