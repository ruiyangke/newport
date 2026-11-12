//! Windows installer handoff. The OS launch follows complete service drain;
//! rejection restarts the existing app, never claiming installation succeeded.
#[cfg(windows)]
use newport_platform::updates::{VerifiedUpdate, windows::PreparedInstaller};
#[cfg(windows)]
use std::{
    path::PathBuf,
    process::{Command, Stdio},
};

#[cfg(windows)]
pub struct Request {
    installer: PreparedInstaller,
    executable: PathBuf,
}

#[cfg(windows)]
impl Request {
    pub fn prepare(package: &VerifiedUpdate) -> Result<Self, String> {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        if !executable.is_absolute()
            || !executable
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
            || !std::fs::symlink_metadata(&executable)
                .map_err(|error| error.to_string())?
                .is_file()
        {
            return Err("Updates require an installed Windows executable.".into());
        }
        Ok(Self {
            installer: PreparedInstaller::prepare(package)?,
            executable,
        })
    }

    pub fn begin(self) -> Result<Installation, String> {
        // Keep preparation owned across service drain. Revalidation and OS
        // launch happen together afterward, under the platform's file lock.
        Ok(Installation(self))
    }
}

#[cfg(windows)]
pub struct Installation(Request);

#[cfg(windows)]
impl Installation {
    pub async fn run(self, shutdown: Result<(), String>) -> Result<(), String> {
        tokio::task::spawn_blocking(move || {
            let Request {
                installer,
                executable,
            } = self.0;
            handoff(
                shutdown,
                move || installer.launch().map(|_| ()),
                move || {
                    Command::new(&executable)
                        .current_dir(executable.parent().ok_or("Missing app directory.")?)
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::inherit())
                        .spawn()
                        .map(|_| ())
                        .map_err(|error| format!("Could not restart Newport: {error}"))
                },
            )
        })
        .await
        .map_err(|error| format!("Installer handoff failed: {error}"))?
    }
}

fn handoff(
    shutdown: Result<(), String>,
    launch: impl FnOnce() -> Result<(), String>,
    restart: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    shutdown?;
    if let Err(error) = launch() {
        restart()
            .map_err(|restart| format!("{error} The existing app could not restart: {restart}"))?;
        return Err(format!("{error} The existing app was restarted."));
    }
    // NSIS owns installation and relaunch from this point. Do not start a second
    // app alongside it, and do not wait while still executing the app it replaces.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn failed_drain_never_launches_or_restarts() {
        assert_eq!(
            handoff(
                Err("drain failed".into()),
                || panic!("launched"),
                || panic!("restarted")
            )
            .unwrap_err(),
            "drain failed"
        );
    }

    #[test]
    fn successful_handoff_leaves_relaunch_to_the_installer() {
        let calls = RefCell::new(Vec::new());
        handoff(
            Ok(()),
            || {
                calls.borrow_mut().push("launch");
                Ok(())
            },
            || panic!("Duplicate relaunch"),
        )
        .unwrap();
        assert_eq!(*calls.borrow(), ["launch"]);
    }

    #[test]
    fn rejected_launch_restarts_once_and_reports_restart_failure() {
        for restarted in [false, true] {
            let calls = RefCell::new(Vec::new());
            let error = handoff(
                Ok(()),
                || {
                    calls.borrow_mut().push("launch");
                    Err("Launch cancelled".into())
                },
                || {
                    calls.borrow_mut().push("restart");
                    if restarted {
                        Ok(())
                    } else {
                        Err("launch denied".into())
                    }
                },
            )
            .unwrap_err();
            assert_eq!(*calls.borrow(), ["launch", "restart"]);
            assert!(error.contains("Launch cancelled"));
            assert!(error.contains(if restarted {
                "existing app was restarted"
            } else {
                "existing app could not restart"
            }));
        }
    }
}
