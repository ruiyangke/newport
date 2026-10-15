//! Native-only sidebar sizing. Preferences never touch the backend config.
use gpui_kit::component::resizable::{
    ResizablePanel, ResizablePanelGroup, ResizableState, h_resizable, resizable_panel,
};
use gpui_kit::{prelude::*, *};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

pub const DEFAULT_WIDTH: f32 = 204.;
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

#[derive(Serialize, Deserialize)]
struct Preferences {
    sidebar_width: f32,
}

fn load_width(path: Option<&Path>) -> io::Result<f32> {
    let Some(path) = path else {
        return Ok(DEFAULT_WIDTH);
    };
    match fs::read(path) {
        Ok(bytes) => {
            let preferences: Preferences = serde_json::from_slice(&bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            Ok(clamp_width(preferences.sidebar_width, f32::MAX))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(DEFAULT_WIDTH),
        Err(error) => Err(error),
    }
}

fn save_width(path: &Path, width: f32) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(&Preferences {
        sidebar_width: width,
    })?;
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub struct SidebarLayout {
    pub state: Entity<ResizableState>,
    preferred_width: f32,
    viewport: Option<f32>,
    preferences_path: Option<PathBuf>,
}

impl SidebarLayout {
    pub fn new(cx: &mut App) -> (Self, Option<io::Error>) {
        // No fallback to Newport/Porthop application data: the native prototype
        // may only persist inside the explicitly selected isolated data dir.
        let preferences_path = std::env::var_os("NEWPORT_DATA_DIR")
            .filter(|path| !path.is_empty())
            .map(|base| PathBuf::from(base).join("gpui-shell-preferences.json"));
        let (preferred_width, load_error) = match load_width(preferences_path.as_deref()) {
            Ok(width) => (width, None),
            Err(error) => (DEFAULT_WIDTH, Some(error)),
        };
        (
            Self {
                state: cx.new(|_| ResizableState::default()),
                preferred_width,
                viewport: None,
                preferences_path,
            },
            load_error,
        )
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

    /// Invoke from the group's on_resize callback (pointer release). The parent
    /// should notify and surface any returned persistence error as a notice.
    pub fn capture_resize(&mut self, viewport: f32, cx: &App) -> io::Result<()> {
        if let Some(width) = self.state.read(cx).sizes().first().copied() {
            self.preferred_width = clamp_width(f32::from(width), viewport);
            self.save()?;
        }
        Ok(())
    }

    /// A parent action/menu can use this for accessible keyboard resizing.
    /// Clearing state also works while collapsed, preserving the requested size
    /// for the next time the two-panel group is rendered.
    pub fn set_width(&mut self, width: f32, viewport: f32, cx: &mut App) -> io::Result<()> {
        self.preferred_width = clamp_width(width, viewport);
        self.state.update(cx, |state, _| state.clear());
        self.save()
    }

    pub fn reset(&mut self, viewport: f32, cx: &mut App) -> io::Result<()> {
        self.set_width(DEFAULT_WIDTH, viewport, cx)
    }

    fn save(&self) -> io::Result<()> {
        let Some(path) = &self.preferences_path else {
            return Ok(());
        };
        save_width(path, self.preferred_width)
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_WIDTH, clamp_width, load_width, maximum, save_width};
    use std::{fs, io, path::PathBuf};

    struct TestDirectory(PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("newport-sidebar-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn preference_load_distinguishes_missing_corrupt_and_unreadable_files() {
        let directory = TestDirectory::new();
        let path = directory.0.join("preferences.json");
        assert_eq!(load_width(None).unwrap(), DEFAULT_WIDTH);
        assert_eq!(load_width(Some(&path)).unwrap(), DEFAULT_WIDTH);
        fs::write(&path, "invalid").unwrap();
        assert_eq!(
            load_width(Some(&path)).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "invalid");
        assert!(load_width(Some(&directory.0)).is_err());
        save_width(&path, 280.).unwrap();
        assert_eq!(load_width(Some(&path)).unwrap(), 280.);
        save_width(&path, 999.).unwrap();
        assert_eq!(load_width(Some(&path)).unwrap(), 300.);
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
    }

    #[test]
    fn failed_preference_replacement_cleans_up_temporary_file() {
        let directory = TestDirectory::new();
        let path = directory.0.join("preferences.json");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("keep"), "original").unwrap();
        assert!(save_width(&path, 250.).is_err());
        assert_eq!(fs::read_to_string(path.join("keep")).unwrap(), "original");
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
    }

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
