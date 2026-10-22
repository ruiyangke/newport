//! Window placement belongs to the isolated native client, never the backend.
//! It is saved with the workspace in `ui-state.json` (see `ui_state`).
use gpui_kit::{App, Bounds, Window, WindowBounds, point, px, size};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(super) struct Placement {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    maximized: bool,
    #[serde(default)]
    display_uuid: Option<[u8; 16]>,
}

impl Placement {
    #[cfg(test)]
    pub(super) fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
            maximized: false,
            display_uuid: None,
        }
    }

    #[cfg(test)]
    pub(super) fn width(self) -> f32 {
        self.width
    }

    fn from_bounds(bounds: Bounds<gpui_kit::Pixels>) -> Self {
        Self {
            x: bounds.origin.x.into(),
            y: bounds.origin.y.into(),
            width: bounds.size.width.into(),
            height: bounds.size.height.into(),
            maximized: false,
            display_uuid: None,
        }
    }

    fn valid(self) -> bool {
        [self.x, self.y, self.width, self.height]
            .into_iter()
            .all(f32::is_finite)
            && self.width > 0.
            && self.height > 0.
    }

    fn fit(mut self, screen: Self) -> Self {
        self.width = self.width.max(640.).min(screen.width.max(640.));
        self.height = self.height.max(480.).min(screen.height.max(480.));
        self.x = self
            .x
            .clamp(screen.x, screen.x + (screen.width - self.width).max(0.));
        self.y = self
            .y
            .clamp(screen.y, screen.y + (screen.height - self.height).max(0.));
        self
    }

    fn bounds(self) -> WindowBounds {
        let bounds = Bounds::new(
            point(px(self.x), px(self.y)),
            size(px(self.width), px(self.height)),
        );
        if self.maximized {
            WindowBounds::Maximized(bounds)
        } else {
            WindowBounds::Windowed(bounds)
        }
    }
}

pub struct WindowState {
    placement: Placement,
    display_id: Option<gpui_kit::DisplayId>,
    pending: Option<(Placement, Instant)>,
    /// The settled placement changed since it was last taken for saving.
    changed: bool,
}

impl WindowState {
    /// The window's placement from the saved state, fitted to a display that
    /// is still there, or a centered default.
    pub fn load(saved: Option<Placement>, cx: &App) -> Self {
        let fallback =
            Placement::from_bounds(Bounds::centered(None, size(px(1074.), px(726.)), cx));
        let mut placement = saved
            .filter(|placement| {
                let valid = placement.valid();
                if !valid {
                    eprintln!("Invalid native window state; using default placement");
                }
                valid
            })
            .unwrap_or(fallback);
        // macOS GPUI bounds are display-local. Stable UUIDs survive display-ID changes.
        let display = cx
            .displays()
            .into_iter()
            .find(|d| {
                placement.display_uuid.is_some()
                    && d.uuid().ok().map(|id| *id.as_bytes()) == placement.display_uuid
            })
            .or_else(|| cx.primary_display());
        if let Some(display) = &display {
            placement = placement.fit(Placement::from_bounds(display.visible_bounds()));
            placement.display_uuid = display.uuid().ok().map(|id| *id.as_bytes());
        }
        Self {
            placement,
            display_id: display.map(|d| d.id()),
            pending: None,
            changed: false,
        }
    }

    pub fn bounds(&self) -> WindowBounds {
        self.placement.bounds()
    }

    pub fn display_id(&self) -> Option<gpui_kit::DisplayId> {
        self.display_id
    }

    pub fn capture(&mut self, window: &Window, cx: &App) {
        #[cfg(windows)]
        let minimized = raw_window_handle::HasWindowHandle::window_handle(window)
            .ok()
            .and_then(|handle| newport_platform::window::is_minimized(handle).ok())
            .unwrap_or(false);
        #[cfg(not(windows))]
        let minimized = false;
        let display_uuid = window
            .display(cx)
            .and_then(|d| d.uuid().ok())
            .map(|id| *id.as_bytes());
        self.capture_observed(
            window.window_bounds(),
            window.is_maximized(),
            minimized,
            display_uuid,
        );
    }

    fn capture_observed(
        &mut self,
        bounds: WindowBounds,
        maximized: bool,
        minimized: bool,
        display_uuid: Option<[u8; 16]>,
    ) {
        // Windows sends move notifications while minimizing. Its bounds then
        // look windowed and IsZoomed is false even when restore will maximize.
        // Keep the last presentation and any legitimate pending normal resize.
        if minimized {
            return;
        }
        if let WindowBounds::Fullscreen(bounds) = bounds {
            // Preserve a resize immediately before fullscreen using the actual
            // platform restore bounds, rather than an animated intermediate.
            if !self.placement.maximized {
                let display_uuid = self.placement.display_uuid;
                self.placement = Placement::from_bounds(bounds);
                self.placement.display_uuid = display_uuid;
                self.changed = true;
            }
            self.pending = None;
            return;
        }
        if maximized {
            self.pending = None;
            self.placement.maximized = true;
            self.placement.display_uuid = display_uuid;
            self.changed = true;
        } else {
            let mut placement = Placement::from_bounds(bounds.get_bounds());
            placement.display_uuid = display_uuid;
            self.pending = Some((placement, Instant::now()));
        }
    }

    /// Zoom emits normal-sized intermediate frames. Commit only after settling,
    /// so they cannot replace the last normal restore bounds. Returns the
    /// placement to save when the settled placement changed.
    pub fn settle(&mut self) -> Option<Placement> {
        if self
            .pending
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() >= Duration::from_millis(500))
        {
            self.placement = self.pending.take().unwrap().0;
            self.changed = true;
        }
        std::mem::take(&mut self.changed).then_some(self.placement)
    }

    /// The settled placement. Quit may interrupt zoom, so a pending one is
    /// never saved.
    pub fn placement(&self) -> Placement {
        self.placement
    }
}

#[cfg(test)]
mod tests {
    use super::{Placement, WindowState};
    fn rect(x: f32, y: f32, width: f32, height: f32) -> Placement {
        Placement {
            x,
            y,
            width,
            height,
            maximized: false,
            display_uuid: None,
        }
    }

    #[test]
    fn clamps_placement_to_usable_screen_bounds() {
        let screen = rect(-1920., 25., 1920., 1055.);
        let fitted = rect(-1800., 100., 1000., 700.).fit(screen);
        assert_eq!((fitted.x, fitted.y, fitted.width), (-1800., 100., 1000.));
        let fitted = rect(5000., -800., 3000., 2000.).fit(screen);
        assert_eq!(
            (fitted.x, fitted.y, fitted.width, fitted.height),
            (-1920., 25., 1920., 1055.)
        );
        assert_eq!(rect(0., 0., 1., 1.).fit(screen).width, 640.);
        assert!(!rect(f32::NAN, 0., 100., 100.).valid());
        assert!(!rect(0., 0., -1., 100.).valid());
    }

    fn state(placement: Placement) -> WindowState {
        WindowState {
            placement,
            display_id: None,
            pending: None,
            changed: false,
        }
    }

    #[test]
    fn only_settled_restore_bounds_and_maximized_state_are_saved() {
        let mut state = state(rect(15., 30., 900., 650.));
        state.placement.maximized = true;
        // An animation interrupted by Quit must not replace the normal bounds.
        state.pending = Some((rect(0., 0., 1700., 1000.), std::time::Instant::now()));
        assert!(state.settle().is_none(), "nothing settled yet");
        let saved = state.placement();
        assert!(saved.maximized);
        assert_eq!((saved.x, saved.width), (15., 900.));
        state.pending = Some((
            rect(20., 40., 950., 700.),
            std::time::Instant::now() - std::time::Duration::from_secs(1),
        ));
        let saved = state.settle().expect("a settled resize is saved");
        assert!(!saved.maximized);
        assert_eq!((saved.x, saved.width), (20., 950.));
        assert!(state.settle().is_none(), "saved once");
    }

    #[test]
    fn minimized_notifications_preserve_the_saved_restore_mode() {
        let mut state = state(rect(15., 30., 900., 650.));
        let display = Some([1; 16]);
        state.capture_observed(rect(0., 0., 1600., 1000.).bounds(), true, false, display);
        assert!(state.settle().is_some_and(|saved| saved.maximized));
        // Windows emits a move on minimize, with IsZoomed=false and normal
        // restore bounds even when it will restore the window maximized.
        state.capture_observed(rect(15., 30., 900., 650.).bounds(), false, true, None);
        if let Some((_, at)) = &mut state.pending {
            *at -= std::time::Duration::from_secs(1);
        }
        assert!(state.settle().is_none(), "minimizing changes nothing");
        let saved = state.placement();
        assert!(
            saved.maximized,
            "Minimize must not discard the restore mode"
        );
        assert_eq!(saved.display_uuid, display);
        assert_eq!(
            (saved.x, saved.y, saved.width, saved.height),
            (15., 30., 900., 650.)
        );

        // An actual unmaximize still updates the saved normal placement.
        state.capture_observed(rect(25., 40., 950., 700.).bounds(), false, false, display);
        state.pending.as_mut().unwrap().1 -= std::time::Duration::from_secs(1);
        let saved = state.settle().unwrap();
        assert!(!saved.maximized);
        assert_eq!((saved.x, saved.width), (25., 950.));
    }
}
