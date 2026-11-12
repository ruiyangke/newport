//! The window's own preferences: sidebar width, the icon-only sidebar, the
//! theme and whether Option is Meta in terminals. They live in the profile's
//! preferences.json, which the backend owns and writes; the window only reads
//! it once at startup, before the first frame, and sends changes through
//! `set_window_preferences`.
use crate::appearance::Appearance;
use serde_json::{Map, Value};
use std::{fs, io, path::Path};

const FILE: &str = "preferences.json";

#[derive(Debug, PartialEq)]
pub struct WindowPreferences {
    pub sidebar_width: Option<f32>,
    pub sidebar_compact: bool,
    pub appearance: Appearance,
    /// macOS: Option sends Meta keys in terminals instead of typing characters.
    pub terminal_option_as_meta: bool,
}
impl Default for WindowPreferences {
    fn default() -> Self {
        Self {
            sidebar_width: None,
            sidebar_compact: false,
            appearance: Appearance::default(),
            terminal_option_as_meta: true,
        }
    }
}

/// Reads the saved preferences. A missing file means defaults; an unreadable
/// or invalid one is reported, and defaults stand in for this session.
pub fn load(directory: Option<&Path>) -> (WindowPreferences, Option<String>) {
    let Some(directory) = directory else {
        return Default::default();
    };
    let (values, error) = match read(&directory.join(FILE)) {
        Ok(values) => (values.unwrap_or_default(), None),
        Err(e) => (Map::new(), Some(format!("Couldn’t read preferences: {e}"))),
    };
    let preferences = WindowPreferences {
        sidebar_width: values
            .get("sidebarWidth")
            .and_then(Value::as_f64)
            .filter(|width| (176.0..=300.0).contains(width))
            .map(|width| width as f32),
        sidebar_compact: values
            .get("sidebarCompact")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        appearance: values
            .get("appearance")
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default(),
        terminal_option_as_meta: values
            .get("terminalOptionAsMeta")
            .and_then(Value::as_bool)
            .unwrap_or(true),
    };
    (preferences, error)
}

fn read_value(path: &Path) -> io::Result<Option<Value>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
fn read(path: &Path) -> io::Result<Option<Map<String, Value>>> {
    match read_value(path)? {
        Some(Value::Object(map)) => Ok(Some(map)),
        Some(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "preferences are not a JSON object",
        )),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    struct Directory(PathBuf);
    impl Directory {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn tempdir() -> Directory {
        let path = std::env::temp_dir().join(format!("newport-window-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Directory(path)
    }

    #[test]
    fn missing_files_are_defaults_and_saved_values_are_read() {
        let directory = tempdir();
        assert_eq!(load(Some(directory.path())), Default::default());
        assert_eq!(load(None), Default::default());
        assert!(
            load(None).0.terminal_option_as_meta,
            "Option is Meta by default"
        );
        fs::write(
            directory.path().join(FILE),
            json!({"sidebarWidth": 240.0, "sidebarCompact": true, "appearance": "dark",
                "terminalOptionAsMeta": false, "gitClientId": "kept"})
            .to_string(),
        )
        .unwrap();
        let (preferences, error) = load(Some(directory.path()));
        assert_eq!(error, None);
        assert_eq!(preferences.sidebar_width, Some(240.0));
        assert!(preferences.sidebar_compact);
        assert_eq!(preferences.appearance, Appearance::Dark);
        assert!(!preferences.terminal_option_as_meta);
    }

    #[test]
    fn unreadable_preferences_are_reported_with_defaults() {
        let directory = tempdir();
        fs::write(directory.path().join(FILE), "not json").unwrap();
        let (preferences, error) = load(Some(directory.path()));
        assert!(error.unwrap().contains("Couldn’t read preferences"));
        assert_eq!(preferences.appearance, Appearance::System);
        assert_eq!(preferences.sidebar_width, None);
    }
}
