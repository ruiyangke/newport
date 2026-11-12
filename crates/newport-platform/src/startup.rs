//! Login registration, independent of the UI host. The host decides whether
//! installation features are available; an isolated profile must use disabled().
use std::path::PathBuf;

// Settings RPCs may run concurrently. Keep each write and its readback together.
static REGISTRATION_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(target_os = "macos")]
#[path = "startup/macos.rs"]
mod os;
#[cfg(windows)]
#[path = "startup/windows.rs"]
mod os;

#[derive(Clone)]
pub struct Startup {
    executable: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Settings {
    pub enabled: bool,
    pub available: bool,
}

impl Startup {
    pub fn disabled() -> Self {
        Self { executable: None }
    }

    /// Only the installed host should construct this. No registration is changed
    /// until the user explicitly enables or disables launch at login.
    pub fn installed(executable: PathBuf) -> Result<Self, String> {
        if !executable.is_absolute() || !executable.is_file() {
            return Err("The login executable must be an existing absolute file path.".into());
        }
        #[cfg(windows)]
        if executable.to_string_lossy().contains('"') {
            return Err("The login executable contains an invalid quote.".into());
        }
        Ok(Self {
            executable: Some(executable),
        })
    }

    /// Moves a login registration written by an earlier release to the system's
    /// own login items, so the user keeps launch at login. Changes nothing for
    /// an isolated profile.
    pub fn adopt_legacy_registration(&self) -> Result<(), String> {
        let _guard = REGISTRATION_LOCK
            .lock()
            .map_err(|error| error.to_string())?;
        #[cfg(target_os = "macos")]
        if let Some(executable) = &self.executable {
            return os::Registration::current()?.adopt_legacy(executable);
        }
        Ok(())
    }

    pub fn settings(&self) -> Result<Settings, String> {
        let _guard = REGISTRATION_LOCK
            .lock()
            .map_err(|error| error.to_string())?;
        self.read_settings()
    }

    fn read_settings(&self) -> Result<Settings, String> {
        if self.executable.is_none() {
            return Ok(Settings {
                enabled: false,
                available: false,
            });
        }
        #[cfg(any(target_os = "macos", windows))]
        let enabled = os::Registration::current()?.enabled();
        #[cfg(any(target_os = "macos", windows))]
        return Ok(Settings {
            enabled: enabled?,
            available: true,
        });
        #[cfg(not(any(target_os = "macos", windows)))]
        Err("Launch at login is unsupported on this platform.".into())
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<Settings, String> {
        let _guard = REGISTRATION_LOCK
            .lock()
            .map_err(|error| error.to_string())?;
        let executable = self
            .executable
            .as_ref()
            .ok_or("Launch at login is available in the installed app.")?;
        if enabled && !executable.is_file() {
            return Err("The login executable no longer exists at its installed path.".into());
        }
        #[cfg(any(target_os = "macos", windows))]
        {
            #[cfg(target_os = "macos")]
            os::Registration::current()?.set_enabled(enabled)?;
            #[cfg(windows)]
            os::Registration::current()?.set_enabled(executable, enabled)?;
            let settings = self.read_settings()?;
            if settings.enabled != enabled {
                return Err("Could not verify the launch-at-login setting.".into());
            }
            Ok(settings)
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = (executable, enabled);
            Err("Launch at login is unsupported on this platform.".into())
        }
    }
}

/// Whether the system opened the app as a login item. Only meaningful while
/// the app is finishing its launch; the system gives login launches no
/// arguments.
pub fn launched_at_login() -> bool {
    #[cfg(target_os = "macos")]
    return os::launched_at_login();
    #[cfg(not(target_os = "macos"))]
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moved_executable_cannot_be_registered() {
        let executable =
            std::env::temp_dir().join(format!("newport-startup-exe-{}", uuid::Uuid::new_v4()));
        std::fs::write(&executable, b"test executable").unwrap();
        let startup = Startup::installed(executable.clone()).unwrap();
        std::fs::remove_file(executable).unwrap();
        assert_eq!(
            startup.set_enabled(true),
            Err("The login executable no longer exists at its installed path.".into())
        );
    }

    #[test]
    fn disabled_profiles_never_access_or_change_login_registration() {
        let startup = Startup::disabled();
        assert_eq!(
            startup.settings().unwrap(),
            Settings {
                enabled: false,
                available: false
            }
        );
        assert!(startup.set_enabled(true).is_err());
        assert!(startup.set_enabled(false).is_err());
        assert!(Startup::installed(PathBuf::from("relative/newport")).is_err());
    }
}
