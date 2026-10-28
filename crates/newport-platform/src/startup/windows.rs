use std::{
    io::{self, ErrorKind},
    path::Path,
};
use winreg::{
    RegKey, RegValue,
    enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_BINARY},
};

const NAME: &str = "Newport";
const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVAL: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

pub(super) struct Registration {
    root: RegKey,
}

fn optional<T>(result: io::Result<T>) -> io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

impl Registration {
    pub fn current() -> Result<Self, String> {
        Ok(Self {
            root: RegKey::predef(HKEY_CURRENT_USER),
        })
    }

    fn read(&self, path: &str) -> io::Result<Option<RegValue<'static>>> {
        let Some(key) = optional(self.root.open_subkey_with_flags(path, KEY_READ))? else {
            return Ok(None);
        };
        optional(key.get_raw_value(NAME))
    }

    pub fn enabled(&self) -> Result<bool, String> {
        self.read_enabled().map_err(|error| error.to_string())
    }

    fn read_enabled(&self) -> io::Result<bool> {
        let Some(key) = optional(self.root.open_subkey_with_flags(RUN, KEY_READ))? else {
            return Ok(false);
        };
        if optional(key.get_value::<String, _>(NAME))?.is_none_or(|value| value.is_empty()) {
            return Ok(false);
        }
        let Some(approval) = self.read(APPROVAL)? else {
            return Ok(true);
        };
        // Preserve a user's Task Manager decision. A nonzero FILETIME denotes a
        // disabled startup entry.
        if approval.vtype != REG_BINARY || approval.bytes.len() != 12 {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "Invalid Newport startup approval value.",
            ));
        }
        Ok(approval.bytes[4..].iter().all(|byte| *byte == 0) && approval.bytes[0] != 3)
    }

    fn write(&self, path: &str, value: Option<&RegValue<'_>>) -> io::Result<()> {
        if let Some(value) = value {
            self.root.create_subkey(path)?.0.set_raw_value(NAME, value)
        } else {
            if let Some(key) = optional(self.root.open_subkey_with_flags(path, KEY_SET_VALUE))? {
                optional(key.delete_value(NAME))?;
            }
            Ok(())
        }
    }

    pub fn set_enabled(&self, executable: &Path, enabled: bool) -> Result<(), String> {
        use winreg::types::ToRegValue;
        if !enabled {
            return self.write(RUN, None).map_err(|error| error.to_string());
        }
        let previous_run = self.read(RUN).map_err(|error| error.to_string())?;
        let previous_approval = self.read(APPROVAL).map_err(|error| error.to_string())?;
        // Quote the executable, including paths under Program Files. Arguments
        // are fixed and the command is interpreted by CreateProcess, not a shell.
        let mut command: std::ffi::OsString = "\"".into();
        command.push(executable.as_os_str());
        command.push("\" --autostart");
        self.write(RUN, Some(&command.to_reg_value()))
            .map_err(|error| error.to_string())?;
        let approval = RegValue {
            vtype: REG_BINARY,
            bytes: vec![2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0].into(),
        };
        if let Err(error) = self.write(APPROVAL, Some(&approval)) {
            // Do not leave a partially enabled entry after a failed operation.
            let run_rollback = self.write(RUN, previous_run.as_ref());
            let approval_rollback = self.write(APPROVAL, previous_approval.as_ref());
            return Err(format!(
                "Cannot enable startup: {error}; restoring registration: {run_rollback:?}; restoring approval: {approval_rollback:?}"
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolated_registry_roundtrip_quotes_paths_and_observes_task_manager() {
        // All nested keys live below an inert test subtree, never the OS Run key.
        let path = format!(r"Software\NewportTests\{}", uuid::Uuid::new_v4());
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let root = hkcu.create_subkey(&path).unwrap().0;
        let registration = Registration { root };
        let executable = Path::new(r"C:\Program Files\Newport\newport.exe");
        assert!(!registration.enabled().unwrap());
        registration.set_enabled(executable, true).unwrap();
        assert!(registration.enabled().unwrap());
        let command: String = registration
            .root
            .open_subkey(RUN)
            .unwrap()
            .get_value(NAME)
            .unwrap();
        assert_eq!(
            command,
            r#""C:\Program Files\Newport\newport.exe" --autostart"#
        );
        registration
            .write(
                APPROVAL,
                Some(&RegValue {
                    vtype: REG_BINARY,
                    bytes: vec![3, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0].into(),
                }),
            )
            .unwrap();
        assert!(!registration.enabled().unwrap());
        registration.set_enabled(executable, true).unwrap();
        assert!(registration.enabled().unwrap());
        registration.set_enabled(executable, false).unwrap();
        registration.set_enabled(executable, false).unwrap();
        assert!(!registration.enabled().unwrap());
        drop(registration);
        hkcu.delete_subkey_all(path).unwrap();
    }
}
