use crate::{Point, PointCoordinate, Result};
use libghostty_vt::{
    Terminal,
    selection::gesture::{
        Behavior, Behaviors, DragEvent, Geometry, Gesture, PressEvent, ReleaseEvent,
    },
};

/// Pointer location relative to the grid, in cell units. Fractions retain the
/// within-cell position; values outside the viewport support drag autoscroll.
#[derive(Clone, Copy, Debug)]
pub struct SelectionPosition {
    pub column: f64,
    pub row: f64,
}

// Ghostty's selection geometry uses integer pixels. Normalized subcell units
// preserve fractional GPUI cell widths without rounding to logical pixels.
const CELL_UNITS: u32 = 1024;

pub(crate) struct SelectionState {
    gesture: Gesture<'static>,
    press: PressEvent<'static>,
    drag: DragEvent<'static>,
    release: ReleaseEvent<'static>,
    rectangle: bool,
    pub append_newline: bool,
}

impl SelectionState {
    pub fn new() -> Result<Self> {
        let boundaries: Vec<char> = " ()[]{}'\",`".chars().collect();
        let mut press = PressEvent::new()?;
        press.set_word_boundary_codepoints(&boundaries)?;
        let mut drag = DragEvent::new()?;
        drag.set_word_boundary_codepoints(&boundaries)?;
        Ok(Self {
            gesture: Gesture::new()?,
            press,
            drag,
            release: ReleaseEvent::new()?,
            rectangle: false,
            append_newline: false,
        })
    }

    pub fn reset(&mut self, terminal: &Terminal<'_, '_>) {
        self.gesture.reset(terminal);
        self.rectangle = false;
        self.append_newline = false;
    }

    pub fn begin(
        &mut self,
        terminal: &Terminal<'static, '_>,
        position: SelectionPosition,
        click_count: usize,
        extend: bool,
        rectangle: bool,
    ) -> Result<()> {
        let point = position.point(terminal)?;
        if extend && self.gesture.anchor(terminal)?.is_some() {
            return self.update(terminal, position);
        }
        self.reset(terminal);
        self.rectangle = rectangle;
        self.append_newline = click_count >= 3;
        // The host already counts clicks using the platform's timing and
        // distance preferences. Do not maintain a competing double-click clock.
        let behavior = match click_count {
            2 => Behavior::Word,
            3.. => Behavior::Line,
            _ => Behavior::Cell,
        };
        self.press
            .set_position(position.x(), position.y())?
            .set_behaviors(&Behaviors::new().with_single_click_behavior(behavior))?;
        let selection = self
            .press
            .apply(&mut self.gesture, terminal, terminal.grid_ref(point)?)?;
        terminal.set_selection(selection.as_ref())?;
        Ok(())
    }

    pub fn update(
        &mut self,
        terminal: &Terminal<'static, '_>,
        mut position: SelectionPosition,
    ) -> Result<()> {
        if !self.rectangle {
            if position.row < 0.0 {
                position.column = 0.0;
            } else if position.row >= f64::from(terminal.rows()?) {
                position.column = f64::from(terminal.cols()?);
            }
        }
        let point = position.point(terminal)?;
        self.drag
            .set_position(position.x(), position.y())?
            .set_rectangle(self.rectangle)?;
        let selection = self.drag.apply(
            &mut self.gesture,
            terminal,
            terminal.grid_ref(point)?,
            Geometry {
                columns: u32::from(terminal.cols()?),
                cell_width: CELL_UNITS,
                padding_left: 0,
                screen_height: u32::from(terminal.rows()?) * CELL_UNITS,
            },
        )?;
        terminal.set_selection(selection.as_ref())?;
        Ok(())
    }

    pub fn end(
        &mut self,
        terminal: &Terminal<'static, '_>,
        position: Option<SelectionPosition>,
    ) -> Result<()> {
        let point = position
            .map(|position| position.point(terminal))
            .transpose()?;
        self.release.apply(
            &mut self.gesture,
            terminal,
            point.map(|point| terminal.grid_ref(point)).transpose()?,
        )
    }
}

impl SelectionPosition {
    fn point(self, terminal: &Terminal<'_, '_>) -> Result<Point> {
        if !self.column.is_finite() || !self.row.is_finite() {
            return Err(crate::Error::InvalidValue);
        }
        Ok(Point::Viewport(PointCoordinate {
            x: self
                .column
                .floor()
                .clamp(0.0, f64::from(terminal.cols()? - 1)) as u16,
            y: self.row.floor().clamp(0.0, f64::from(terminal.rows()? - 1)) as u32,
        }))
    }
    fn x(self) -> f64 {
        self.column.clamp(-1.0, 501.0) * f64::from(CELL_UNITS)
    }
    fn y(self) -> f64 {
        self.row.clamp(-1.0, 301.0) * f64::from(CELL_UNITS)
    }
}
