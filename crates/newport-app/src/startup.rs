//! Host startup policy; the platform crate only owns OS registration.
use newport_platform::startup::Startup;

#[derive(Default, Debug, PartialEq, Eq)]
pub struct Launch {
    pub autostart: bool,
    pub check: Option<Check>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    Backend,
    UpdateHealth,
}
impl Launch {
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut launch = Self::default();
        for arg in args {
            match arg.as_str() {
                "--autostart" => launch.autostart = true,
                "--check-backend" | "--check-update-health" => {
                    let check = if arg == "--check-backend" {
                        Check::Backend
                    } else {
                        Check::UpdateHealth
                    };
                    if launch.check.is_some_and(|previous| previous != check) {
                        return Err("Choose only one startup check.".into());
                    }
                    launch.check = Some(check);
                }
                _ => {}
            }
        }
        Ok(launch)
    }
    pub fn activate_existing(&self) -> bool {
        !self.autostart && self.check.is_none()
    }
}

fn available(debug: bool, profile_override: bool) -> bool {
    !debug && !profile_override
}

pub fn profile_policy(debug: bool, path: Option<&std::path::Path>) -> Result<bool, String> {
    match path {
        Some(path) if path.as_os_str().is_empty() => {
            Err("The profile directory override is empty.".into())
        }
        Some(_) => Ok(true),
        None if debug => Err("Development builds require an explicit NEWPORT_DATA_DIR.".into()),
        None => Ok(false),
    }
}

pub fn registration() -> Result<Startup, String> {
    // No development or overridden profile may register itself.
    if !available(
        cfg!(debug_assertions),
        newport_backend::profile::data_directory_override().is_some(),
    ) {
        return Ok(Startup::disabled());
    }
    Startup::installed(std::env::current_exe().map_err(|error| error.to_string())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn released_launch_uses_default_profile_and_debug_requires_an_override() {
        use std::path::Path;
        assert_eq!(profile_policy(false, None), Ok(false));
        assert!(profile_policy(true, None).is_err());
        for debug in [false, true] {
            assert!(profile_policy(debug, Some(Path::new(""))).is_err());
            assert_eq!(profile_policy(debug, Some(Path::new("fixture"))), Ok(true));
        }
    }

    #[test]
    fn login_launch_does_not_raise_an_existing_window() {
        let normal = Launch::from_args(Vec::new()).unwrap();
        assert!(normal.activate_existing());
        for args in [
            vec!["--autostart"],
            vec!["--check-backend"],
            vec!["--autostart", "--check-backend"],
            vec!["--check-update-health"],
            vec!["--autostart", "--check-update-health"],
        ] {
            assert!(
                !Launch::from_args(args.into_iter().map(str::to_owned))
                    .unwrap()
                    .activate_existing()
            );
        }
        assert!(Launch::from_args(["--autostart".into()]).unwrap().autostart);
        assert!(
            !Launch::from_args(["--autostart=false".into()])
                .unwrap()
                .autostart
        );
        assert_eq!(
            Launch::from_args(["--check-update-health".into()])
                .unwrap()
                .check,
            Some(Check::UpdateHealth)
        );
        assert!(
            Launch::from_args(["--check-backend".into(), "--check-update-health".into()]).is_err()
        );
    }

    #[test]
    fn only_installed_release_profiles_can_change_startup_registration() {
        for debug in [true, false] {
            for profile_override in [true, false] {
                assert_eq!(
                    available(debug, profile_override),
                    !debug && !profile_override
                );
            }
        }
    }
}
