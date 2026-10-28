//! Native sidebar sizing. Width and the icon-only mode are window
//! preferences (newport_client::window_preferences); this owns no files.
use gpui_kit::component::resizable::{
    ResizablePanel, ResizablePanelGroup, ResizableState, h_resizable, resizable_panel,
};
use gpui_kit::{prelude::*, *};

pub const DEFAULT_WIDTH: f32 = 184.;
pub const MIN_WIDTH: f32 = 176.;

pub fn maximum(viewport: f32) -> f32 {
    (viewport - 440.).clamp(MIN_WIDTH, 300.)
}

fn clamp_width(width: f32, viewport: f32) -> f32 {
    let width = if width.is_finite() {
        width
    } else {
        DEFAULT_WIDTH
    };
    width.clamp(MIN_WIDTH, maximum(viewport))
}

pub struct SidebarLayout {
    pub state: Entity<ResizableState>,
    preferred_width: f32,
    compact: bool,
    viewport: Option<f32>,
}

impl SidebarLayout {
    /// The saved width (None for the default) and icon-only mode.
    pub fn new(width: Option<f32>, compact: bool, cx: &mut App) -> Self {
        Self {
            state: cx.new(|_| ResizableState::default()),
            preferred_width: width.map_or(DEFAULT_WIDTH, |w| clamp_width(w, f32::MAX)),
            compact,
            viewport: None,
        }
    }

    /// UI tests must never load or overwrite the running app's preferences.
    #[cfg(test)]
    pub(crate) fn isolated(cx: &mut App) -> Self {
        Self {
            state: cx.new(|_| ResizableState::default()),
            preferred_width: DEFAULT_WIDTH,
            compact: false,
            viewport: None,
        }
    }

    pub fn compact(&self) -> bool {
        self.compact
    }
    pub fn set_compact(&mut self, compact: bool) {
        self.compact = compact;
    }
    /// The width to save: the user's choice, not the window-constrained one.
    pub fn preferred_width(&self) -> f32 {
        self.preferred_width
    }
    pub fn width(&self, viewport: f32) -> f32 {
        clamp_width(self.preferred_width, viewport)
    }

    /// Call once per render, even when collapsed. Reinitialize layout when the
    /// window changes size so Kit's proportional resize cannot change the user's
    /// preferred width. A temporarily narrow window never overwrites preference.
    pub fn prepare(&mut self, viewport: f32, cx: &mut App) {
        if self.viewport != Some(viewport) {
            self.viewport = Some(viewport);
            self.state.update(cx, |state, _| state.clear());
        }
    }

    pub fn group(&self) -> ResizablePanelGroup {
        h_resizable("shell-sidebar-layout").with_state(&self.state)
    }

    pub fn sidebar_panel(&self, viewport: f32) -> ResizablePanel {
        resizable_panel()
            .size(px(self.width(viewport)))
            .size_range(px(MIN_WIDTH)..px(maximum(viewport)))
            .flex_none()
    }

    pub fn content_panel(&self) -> ResizablePanel {
        resizable_panel().size_range(px(440.)..Pixels::MAX)
    }

    /// Invoke from the group's on_resize callback (pointer release). True when
    /// the preferred width changed and should be saved.
    pub fn capture_resize(&mut self, viewport: f32, cx: &App) -> bool {
        let Some(width) = self.state.read(cx).sizes().first().copied() else {
            return false;
        };
        let width = clamp_width(f32::from(width), viewport);
        let changed = width != self.preferred_width;
        self.preferred_width = width;
        changed
    }

    /// A parent action/menu can use this for accessible keyboard resizing.
    /// Clearing state also works while collapsed, preserving the requested size
    /// for the next time the two-panel group is rendered.
    pub fn set_width(&mut self, width: f32, viewport: f32, cx: &mut App) {
        self.preferred_width = clamp_width(width, viewport);
        self.state.update(cx, |state, _| state.clear());
    }

    pub fn reset(&mut self, viewport: f32, cx: &mut App) {
        self.set_width(DEFAULT_WIDTH, viewport, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_WIDTH, clamp_width, maximum};

    #[test]
    fn width_limits_leave_room_for_content() {
        assert_eq!(maximum(1200.), 300.);
        assert_eq!(maximum(700.), 260.);
        assert_eq!(maximum(616.), 176.);
        assert_eq!(maximum(400.), 176.);
        assert_eq!(clamp_width(999., 700.), 260.);
        assert_eq!(clamp_width(0., 1200.), 176.);
        assert_eq!(clamp_width(f32::NAN, 1200.), DEFAULT_WIDTH);
    }

    #[test]
    fn temporary_viewport_constraint_does_not_mutate_preference() {
        let preferred_width = 280.;
        assert_eq!(clamp_width(preferred_width, 640.), 200.);
        assert_eq!(clamp_width(preferred_width, 1200.), 280.);
    }
}
