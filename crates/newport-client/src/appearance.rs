//! Appearance choices and persistence, independent of the native UI framework.
//! Preferences remain confined to the explicitly isolated native data directory.
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::PathBuf,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "Follow system",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

pub struct AppearanceStore {
    path: Option<PathBuf>,
}

impl AppearanceStore {
    pub fn from_env() -> Self {
        Self::in_directory(
            std::env::var_os("NEWPORT_DATA_DIR")
                .filter(|base| !base.is_empty())
                .map(PathBuf::from),
        )
    }

    fn in_directory(directory: Option<PathBuf>) -> Self {
        Self {
            path: directory.map(|base| base.join("gpui-appearance.json")),
        }
    }

    /// Only a missing preference defaults silently. Surface unreadable or invalid
    /// files to the UI, where the caller can choose System as a recovery value.
    pub fn load(&self) -> io::Result<Appearance> {
        let Some(path) = &self.path else {
            return Ok(Appearance::default());
        };
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Appearance::default()),
            Err(error) => Err(error),
        }
    }

    /// Replace the preference atomically; failed writes leave the old file intact.
    /// With no isolated data directory, callers may still change session state.
    pub fn save(&self, appearance: Appearance) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec(&appearance)?;
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
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("newport-appearance-{}", uuid::Uuid::new_v4())))
        }

        fn store(&self) -> AppearanceStore {
            AppearanceStore::in_directory(Some(self.0.clone()))
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn choices_serialize_as_reference_values() {
        for (choice, expected) in [
            (Appearance::System, "\"system\""),
            (Appearance::Light, "\"light\""),
            (Appearance::Dark, "\"dark\""),
        ] {
            assert_eq!(serde_json::to_string(&choice).unwrap(), expected);
            assert_eq!(
                serde_json::from_str::<Appearance>(expected).unwrap(),
                choice
            );
        }
    }

    #[test]
    fn missing_preference_defaults_and_changes_survive_reload() {
        let directory = TestDirectory::new();
        let store = directory.store();
        assert_eq!(store.load().unwrap(), Appearance::System);
        for choice in Appearance::ALL {
            store.save(choice).unwrap();
            assert_eq!(directory.store().load().unwrap(), choice);
            assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
        }
    }

    #[test]
    fn corrupt_preferences_are_reported_and_can_be_replaced() {
        let directory = TestDirectory::new();
        fs::create_dir_all(&directory.0).unwrap();
        let path = directory.0.join("gpui-appearance.json");
        for invalid in ["{", "\"unknown\"", "null"] {
            fs::write(&path, invalid).unwrap();
            assert_eq!(
                directory.store().load().unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), invalid);
        }
        directory.store().save(Appearance::Light).unwrap();
        assert_eq!(directory.store().load().unwrap(), Appearance::Light);
    }

    #[test]
    fn missing_isolated_directory_never_persists() {
        let store = AppearanceStore::in_directory(None);
        assert_eq!(store.load().unwrap(), Appearance::System);
        store.save(Appearance::Dark).unwrap();
        assert_eq!(store.load().unwrap(), Appearance::System);
    }

    #[test]
    fn failed_replacement_is_reported_and_temporary_file_is_removed() {
        let directory = TestDirectory::new();
        let path = directory.0.join("gpui-appearance.json");
        fs::create_dir_all(&path).unwrap();
        assert!(directory.store().load().is_err());
        assert!(directory.store().save(Appearance::Dark).is_err());
        assert!(path.is_dir());
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
    }
}
