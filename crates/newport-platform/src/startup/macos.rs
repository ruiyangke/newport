use objc2::{msg_send, rc::Retained};
use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager};
use objc2_service_management::{SMAppService, SMAppServiceStatus};
use plist::Value;
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

/// Shown when the user has switched Newport off in System Settings: only they
/// can switch it back on there.
const NEEDS_APPROVAL: &str = "Allow Newport in System Settings → General → Login Items.";
// The LaunchAgent label an earlier release wrote, matching its plist file name.
const LEGACY_LABEL: &str = "Newport";

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum LoginItem {
    Enabled,
    /// Registered, but switched off in System Settings.
    NeedsApproval,
    /// Includes an app the system cannot find as a bundle.
    Off,
}

impl LoginItem {
    fn from_status(status: SMAppServiceStatus) -> Self {
        match status {
            SMAppServiceStatus::Enabled => Self::Enabled,
            SMAppServiceStatus::RequiresApproval => Self::NeedsApproval,
            _ => Self::Off,
        }
    }
}

pub(super) struct Registration {
    legacy_directory: PathBuf,
}

impl Registration {
    pub fn current() -> Result<Self, String> {
        Ok(Self {
            legacy_directory: dirs::home_dir()
                .ok_or("Cannot locate the home directory.")?
                .join("Library/LaunchAgents"),
        })
    }

    fn legacy_path(&self) -> PathBuf {
        self.legacy_directory.join("Newport.plist")
    }

    fn status(&self) -> LoginItem {
        // SAFETY: a plain class method with no arguments.
        let service = unsafe { SMAppService::mainAppService() };
        // SAFETY: a plain getter on a live service.
        LoginItem::from_status(unsafe { service.status() })
    }

    /// The system follows the app when it moves, so only the registration
    /// itself is read. One the user switched off in System Settings reads as off.
    pub fn enabled(&self) -> Result<bool, String> {
        Ok(self.status() == LoginItem::Enabled)
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        if enabled {
            self.register()
        } else {
            self.unregister()
        }
    }

    fn register(&self) -> Result<(), String> {
        // SAFETY: a plain class method with no arguments.
        let service = unsafe { SMAppService::mainAppService() };
        // SAFETY: a plain call on a live service; the error is returned, not thrown.
        unsafe { service.registerAndReturnError() }
            .map_err(|error| error.localizedDescription().to_string())?;
        match self.status() {
            LoginItem::NeedsApproval => Err(NEEDS_APPROVAL.into()),
            _ => Ok(()),
        }
    }

    fn unregister(&self) -> Result<(), String> {
        if self.status() == LoginItem::Off {
            return Ok(());
        }
        // SAFETY: a plain class method with no arguments.
        let service = unsafe { SMAppService::mainAppService() };
        // SAFETY: a plain call on a live service; the error is returned, not thrown.
        unsafe { service.unregisterAndReturnError() }
            .map_err(|error| error.localizedDescription().to_string())
    }

    /// Hands a LaunchAgent that an earlier release wrote for this executable
    /// over to the system's login items, so the user keeps launch at login.
    pub fn adopt_legacy(&self, executable: &Path) -> Result<(), String> {
        self.adopt_legacy_with(executable, self.status(), || self.register())
    }

    fn adopt_legacy_with(
        &self,
        executable: &Path,
        status: LoginItem,
        register: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        // A missing or unreadable file is not ours to remove.
        let Ok(value) = Value::from_file(self.legacy_path()) else {
            return Ok(());
        };
        if !legacy_launches(&value, executable) {
            return Ok(());
        }
        // A registration the user already has, or switched off, stays as it is.
        if status == LoginItem::Off {
            register()?;
        }
        // The loaded job keeps running until logout; booting it out would
        // end the app when it was started by this very job.
        match fs::remove_file(self.legacy_path()) {
            Ok(()) => crate::filesystem::sync_directory(&self.legacy_directory)
                .map_err(|error| error.to_string()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
}

/// Whether an earlier release's LaunchAgent starts this executable.
fn legacy_launches(value: &Value, executable: &Path) -> bool {
    let Some(value) = value.as_dictionary() else {
        return false;
    };
    value.get("Label").and_then(Value::as_string) == Some(LEGACY_LABEL)
        && value
            .get("ProgramArguments")
            .and_then(Value::as_array)
            .and_then(|args| args.first())
            .and_then(Value::as_string)
            .is_some_and(|program| Path::new(program) == executable)
}

// Apple Event codes from AEDataModel.h.
const KEY_AE_PROP_DATA: u32 = u32::from_be_bytes(*b"prdt");
const LAUNCHED_AS_LOGIN_ITEM: u32 = u32::from_be_bytes(*b"lgit");

/// Login items open the app without arguments; the system marks that launch
/// in the open-application event. The event is only current while the app
/// finishes launching.
pub(super) fn launched_at_login() -> bool {
    let manager = NSAppleEventManager::sharedAppleEventManager();
    let Some(event) = manager.currentAppleEvent() else {
        return false;
    };
    let reason: Option<Retained<NSAppleEventDescriptor>> =
        // SAFETY: the selector takes an AEKeyword (u32) and returns an optional descriptor.
        unsafe { msg_send![&*event, paramDescriptorForKeyword: KEY_AE_PROP_DATA] };
    reason.is_some_and(|reason| is_login_launch(reason.enumCodeValue()))
}

fn is_login_launch(code: u32) -> bool {
    code == LAUNCHED_AS_LOGIN_ITEM
}

#[cfg(test)]
mod tests {
    use super::*;
    use plist::Dictionary;
    use std::cell::Cell;

    const EXECUTABLE: &str = "/Applications/Newport.app/Contents/MacOS/newport";

    fn legacy(label: &str, program: &str) -> Value {
        let mut value = Dictionary::new();
        value.insert("Label".into(), Value::String(label.into()));
        value.insert(
            "ProgramArguments".into(),
            Value::Array(vec![
                Value::String(program.into()),
                Value::String("--autostart".into()),
            ]),
        );
        value.insert("RunAtLoad".into(), Value::Boolean(true));
        Value::Dictionary(value)
    }

    fn directory_with(contents: Option<&Value>) -> (PathBuf, Registration) {
        let directory =
            std::env::temp_dir().join(format!("newport-startup-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        let registration = Registration {
            legacy_directory: directory.clone(),
        };
        if let Some(contents) = contents {
            contents.to_file_xml(registration.legacy_path()).unwrap();
        }
        (directory, registration)
    }

    #[test]
    fn statuses_map_to_what_the_settings_page_can_show() {
        assert_eq!(
            LoginItem::from_status(SMAppServiceStatus::Enabled),
            LoginItem::Enabled
        );
        assert_eq!(
            LoginItem::from_status(SMAppServiceStatus::RequiresApproval),
            LoginItem::NeedsApproval
        );
        assert_eq!(
            LoginItem::from_status(SMAppServiceStatus::NotRegistered),
            LoginItem::Off
        );
        assert_eq!(
            LoginItem::from_status(SMAppServiceStatus::NotFound),
            LoginItem::Off
        );
    }

    #[test]
    fn only_a_login_item_launch_code_starts_hidden() {
        assert!(is_login_launch(u32::from_be_bytes(*b"lgit")));
        assert!(!is_login_launch(u32::from_be_bytes(*b"svit")));
        assert!(!is_login_launch(0));
    }

    #[test]
    fn a_legacy_agent_is_ours_only_for_this_executable() {
        let executable = Path::new(EXECUTABLE);
        assert!(legacy_launches(&legacy("Newport", EXECUTABLE), executable));
        assert!(!legacy_launches(
            &legacy("Newport", "/Volumes/Old/Newport.app/Contents/MacOS/newport"),
            executable
        ));
        assert!(!legacy_launches(&legacy("Other", EXECUTABLE), executable));
        assert!(!legacy_launches(&Value::Boolean(true), executable));
    }

    #[test]
    fn adopting_registers_then_removes_the_legacy_agent() {
        let (directory, registration) = directory_with(Some(&legacy("Newport", EXECUTABLE)));
        let registered = Cell::new(false);
        registration
            .adopt_legacy_with(Path::new(EXECUTABLE), LoginItem::Off, || {
                assert!(registration.legacy_path().exists());
                registered.set(true);
                Ok(())
            })
            .unwrap();
        assert!(registered.get());
        assert!(!registration.legacy_path().exists());
        // A second launch finds nothing to adopt.
        registration
            .adopt_legacy_with(Path::new(EXECUTABLE), LoginItem::Off, || {
                panic!("registered again")
            })
            .unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn a_failed_registration_keeps_the_legacy_agent() {
        let (directory, registration) = directory_with(Some(&legacy("Newport", EXECUTABLE)));
        let result = registration.adopt_legacy_with(Path::new(EXECUTABLE), LoginItem::Off, || {
            Err("denied".into())
        });
        assert_eq!(result, Err("denied".into()));
        assert!(registration.legacy_path().exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn adopting_leaves_an_existing_registration_and_other_files_alone() {
        let (directory, registration) = directory_with(Some(&legacy("Newport", EXECUTABLE)));
        for status in [LoginItem::Enabled, LoginItem::NeedsApproval] {
            let (inner, other) = directory_with(Some(&legacy("Newport", EXECUTABLE)));
            other
                .adopt_legacy_with(Path::new(EXECUTABLE), status, || {
                    panic!("registered over an existing registration")
                })
                .unwrap();
            assert!(!other.legacy_path().exists());
            fs::remove_dir(inner).unwrap();
        }
        let elsewhere = legacy("Newport", "/Volumes/Old/Newport.app/Contents/MacOS/newport");
        elsewhere.to_file_xml(registration.legacy_path()).unwrap();
        registration
            .adopt_legacy_with(Path::new(EXECUTABLE), LoginItem::Off, || {
                panic!("registered for another copy")
            })
            .unwrap();
        assert!(registration.legacy_path().exists());
        fs::write(registration.legacy_path(), b"not a plist").unwrap();
        registration
            .adopt_legacy_with(Path::new(EXECUTABLE), LoginItem::Off, || {
                panic!("registered for a corrupt file")
            })
            .unwrap();
        assert!(registration.legacy_path().exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn nothing_to_adopt_without_a_legacy_agent() {
        let (directory, registration) = directory_with(None);
        registration
            .adopt_legacy_with(Path::new(EXECUTABLE), LoginItem::Off, || {
                panic!("registered without a legacy agent")
            })
            .unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
