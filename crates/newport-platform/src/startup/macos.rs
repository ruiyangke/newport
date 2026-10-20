use plist::{Dictionary, Value};
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

// The LaunchAgent label, matching the plist file name.
const LABEL: &str = "Newport";
pub(super) struct Registration {
    directory: PathBuf,
}

impl Registration {
    pub fn current() -> Result<Self, String> {
        Ok(Self {
            directory: dirs::home_dir()
                .ok_or("Cannot locate the home directory.")?
                .join("Library/LaunchAgents"),
        })
    }
    fn path(&self) -> PathBuf {
        self.directory.join("Newport.plist")
    }

    pub fn enabled(&self) -> Result<bool, String> {
        let file = match fs::File::open(self.path()) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.to_string()),
        };
        let value = Value::from_reader(file).map_err(|error| error.to_string())?;
        let value = value
            .as_dictionary()
            .ok_or("Invalid Newport login registration.")?;
        // A disabled registration is not an enabled preference. Do not silently
        // interpret an unreadable or unrelated file as the user's preference.
        if value.get("Label").and_then(Value::as_string) != Some(LABEL) {
            return Err("Unexpected label in Newport login registration.".into());
        }
        Ok(
            value.get("RunAtLoad").and_then(Value::as_boolean) == Some(true)
                && value.get("Disabled").and_then(Value::as_boolean) != Some(true)
                && value
                    .get("ProgramArguments")
                    .and_then(Value::as_array)
                    .is_some_and(|args| {
                        args.first()
                            .and_then(Value::as_string)
                            .is_some_and(|path| !path.is_empty())
                    }),
        )
    }

    /// Enabled, and launching this executable. A registration left by a
    /// copy of the app that moved or was replaced starts nothing useful, so
    /// it reads as off and switching it on writes this path.
    pub fn enabled_for(&self, executable: &Path) -> Result<bool, String> {
        if !self.enabled()? {
            return Ok(false);
        }
        let value = Value::from_file(self.path()).map_err(|error| error.to_string())?;
        Ok(value
            .as_dictionary()
            .and_then(|value| value.get("ProgramArguments"))
            .and_then(Value::as_array)
            .and_then(|args| args.first())
            .and_then(Value::as_string)
            .is_some_and(|program| Path::new(program) == executable))
    }

    pub fn set_enabled(&self, executable: &Path, enabled: bool) -> Result<(), String> {
        if !enabled {
            return match fs::remove_file(self.path()) {
                Ok(()) => crate::filesystem::sync_directory(&self.directory)
                    .map_err(|error| error.to_string()),
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.to_string()),
            };
        }
        let executable = executable
            .to_str()
            .ok_or("The login executable path is not valid UTF-8.")?;
        let mut value = Dictionary::new();
        value.insert("Label".into(), Value::String(LABEL.into()));
        value.insert(
            "ProgramArguments".into(),
            Value::Array(vec![
                Value::String(executable.into()),
                Value::String("--autostart".into()),
            ]),
        );
        value.insert("RunAtLoad".into(), Value::Boolean(true));
        crate::filesystem::create_private_directory_all(&self.directory)
            .map_err(|error| error.to_string())?;
        let temporary = self
            .directory
            .join(format!(".Newport-{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut file = crate::filesystem::private_file_options()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            Value::Dictionary(value)
                .to_writer_xml(&mut file)
                .map_err(std::io::Error::other)?;
            file.sync_all()?;
            fs::rename(&temporary, self.path())?;
            crate::filesystem::sync_directory(&self.directory)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result.map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_roundtrip_preserves_special_paths_and_is_idempotent() {
        let directory =
            std::env::temp_dir().join(format!("newport-startup-{}", uuid::Uuid::new_v4()));
        let registration = Registration {
            directory: directory.clone(),
        };
        assert!(!registration.enabled().unwrap());
        let executable =
            Path::new("/Applications/Tools & <Work>/Newport.app/Contents/MacOS/newport");
        registration.set_enabled(executable, true).unwrap();
        assert!(registration.enabled().unwrap());
        let value = Value::from_file(registration.path()).unwrap();
        let args = value.as_dictionary().unwrap()["ProgramArguments"]
            .as_array()
            .unwrap();
        assert_eq!(
            args,
            &vec![
                Value::String(executable.to_str().unwrap().into()),
                Value::String("--autostart".into())
            ]
        );
        registration
            .set_enabled(
                Path::new("/Applications/Newport.app/Contents/MacOS/newport"),
                true,
            )
            .unwrap();
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        // The registration now launches the second path, not the first.
        assert!(!registration.enabled_for(executable).unwrap());
        assert!(
            registration
                .enabled_for(Path::new(
                    "/Applications/Newport.app/Contents/MacOS/newport"
                ))
                .unwrap()
        );
        registration.set_enabled(executable, false).unwrap();
        registration.set_enabled(executable, false).unwrap();
        assert!(!registration.enabled().unwrap());
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn corrupt_registration_is_an_error_and_disabled_registration_is_not_enabled() {
        let directory =
            std::env::temp_dir().join(format!("newport-startup-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        let registration = Registration {
            directory: directory.clone(),
        };
        fs::write(registration.path(), b"not a plist").unwrap();
        assert!(registration.enabled().is_err());
        registration
            .set_enabled(
                Path::new("/Applications/Newport.app/Contents/MacOS/newport"),
                true,
            )
            .unwrap();
        let mut value = Value::from_file(registration.path()).unwrap();
        value
            .as_dictionary_mut()
            .unwrap()
            .insert("Disabled".into(), Value::Boolean(true));
        value.to_file_xml(registration.path()).unwrap();
        assert!(!registration.enabled().unwrap());
        fs::remove_dir_all(directory).unwrap();
    }
}
