use crate::{Engine, Error, Point, Result};
use alacritty_terminal::{
    grid::Dimensions,
    index::{Column, Line, Point as GridPoint, Side},
    selection::{Selection, SelectionType},
};
#[derive(Clone, Copy, Debug)]
pub struct SelectionPosition {
    pub column: f64,
    pub row: f64,
}
#[derive(Default)]
pub(crate) struct SelectionState {
    rectangle: bool,
    dragging: bool,
}
impl Engine {
    fn selection_point(
        &self,
        position: SelectionPosition,
        drag: bool,
    ) -> Result<(GridPoint, Side)> {
        if !position.column.is_finite() || !position.row.is_finite() {
            return Err(Error::InvalidValue);
        }
        let mut x = position.column;
        if drag && !self.selection.rectangle {
            if position.row < 0. {
                x = 0.;
            } else if position.row >= self.terminal.screen_lines() as f64 {
                x = self.terminal.columns() as f64;
            }
        }
        let side = if x < 0. || x.fract() < 0.5 && x < self.terminal.columns() as f64 {
            Side::Left
        } else {
            Side::Right
        };
        let column = Column((x.floor().max(0.) as usize).min(self.terminal.columns() - 1));
        let line = Line(
            (position.row.floor().max(0.) as i32).min(self.terminal.screen_lines() as i32 - 1)
                - self.terminal.grid().display_offset() as i32,
        );
        Ok((GridPoint::new(line, column), side))
    }
    pub fn clear_selection(&mut self) -> Result<()> {
        self.terminal.selection = None;
        self.selection = Default::default();
        Ok(())
    }
    pub fn begin_selection(
        &mut self,
        position: SelectionPosition,
        click_count: usize,
        extend: bool,
        rectangle: bool,
    ) -> Result<()> {
        self.selection.rectangle = rectangle;
        let (point, side) = self.selection_point(position, false)?;
        if extend && let Some(selection) = self.terminal.selection.as_mut() {
            selection.update(point, side);
        } else {
            let ty = if rectangle {
                SelectionType::Block
            } else {
                match click_count {
                    2 => SelectionType::Semantic,
                    3.. => SelectionType::Lines,
                    _ => SelectionType::Simple,
                }
            };
            self.terminal.selection = Some(Selection::new(ty, point, side));
        }
        self.selection.dragging = true;
        Ok(())
    }
    pub fn update_selection(&mut self, position: SelectionPosition) -> Result<()> {
        let (point, side) = self.selection_point(position, true)?;
        if self.selection.dragging
            && let Some(selection) = self.terminal.selection.as_mut()
        {
            selection.update(point, side);
        }
        Ok(())
    }
    pub fn end_selection(&mut self, position: Option<SelectionPosition>) -> Result<()> {
        if let Some(position) = position {
            self.update_selection(position)?;
        }
        self.selection.dragging = false;
        Ok(())
    }
    pub fn select_range(&mut self, start: Point, end: Point, rectangle: bool) -> Result<()> {
        let (start, end) = (self.point(start)?, self.point(end)?);
        let (a, b) = if start <= end {
            (Side::Left, Side::Right)
        } else {
            (Side::Right, Side::Left)
        };
        let mut selection = Selection::new(
            if rectangle {
                SelectionType::Block
            } else {
                SelectionType::Simple
            },
            start,
            a,
        );
        selection.update(end, b);
        self.terminal.selection = Some(selection);
        self.selection = Default::default();
        Ok(())
    }
    pub fn select_word(&mut self, point: Point) -> Result<()> {
        self.terminal.selection = Some(Selection::new(
            SelectionType::Semantic,
            self.point(point)?,
            Side::Left,
        ));
        self.selection = Default::default();
        Ok(())
    }
    pub fn select_line(&mut self, point: Point) -> Result<()> {
        self.terminal.selection = Some(Selection::new(
            SelectionType::Lines,
            self.point(point)?,
            Side::Left,
        ));
        self.selection = Default::default();
        Ok(())
    }
    pub fn select_all(&mut self) -> Result<()> {
        let mut selection = Selection::new(
            SelectionType::Lines,
            GridPoint::new(Line(-(self.terminal.history_size() as i32)), Column(0)),
            Side::Left,
        );
        selection.update(
            GridPoint::new(
                Line(self.terminal.screen_lines() as i32 - 1),
                Column(self.terminal.columns() - 1),
            ),
            Side::Right,
        );
        self.terminal.selection = Some(selection);
        self.selection = Default::default();
        Ok(())
    }
    pub fn selected_text(&self) -> Result<Option<String>> {
        Ok(self.terminal.selection_to_string())
    }
}
