//! Host startup policy; the platform crate only owns OS registration.
use newport_platform::startup::Startup;

#[derive(Default, Debug, PartialEq, Eq)]
pub struct Launch {
    pub autostart: bool,
    pub check: Option<Check>,
    /// A taskbar pin or shortcut asked for a remote app.
    pub remote_app: Option<RemoteApp>,
    /// `--data-dir <path>`: the profile directory, as `NEWPORT_DATA_DIR`
    /// names it.
    pub data_directory: Option<String>,
    /// `--remove-remote-apps`: the uninstaller removes the Start shortcuts
    /// and icons of remote apps.
    pub remove_remote_apps: bool,
}
/// `--remote-app <id> [--action <action>]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteApp {
    pub id: String,
    pub action: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    Backend,
    UpdateHealth,
}
impl Launch {
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        use newport_domain::remote_app_identity::{
            ACTION_OPTION, DATA_DIR_OPTION, REMOTE_APP_OPTION, is_valid_app_user_model_id,
        };
        let mut launch = Self::default();
        let mut action = None;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                REMOTE_APP_OPTION | ACTION_OPTION | DATA_DIR_OPTION => {
                    let value = args
                        .next()
                        .filter(|value| !value.is_empty() && !value.starts_with("--"))
                        .ok_or_else(|| format!("{arg} needs a value."))?;
                    if arg == DATA_DIR_OPTION {
                        if launch.data_directory.replace(value).is_some() {
                            return Err("Choose only one profile directory.".into());
                        }
                    } else if arg == ACTION_OPTION {
                        if action.replace(value).is_some() {
                            return Err("Choose only one action.".into());
                        }
                    } else {
                        if !is_valid_app_user_model_id(&value) {
                            return Err(format!("Not a remote app: {value}"));
                        }
                        if launch.remote_app.is_some() {
                            return Err("Choose only one remote app.".into());
                        }
                        launch.remote_app = Some(RemoteApp {
                            id: value,
                            action: None,
                        });
                    }
                }
                "--autostart" => launch.autostart = true,
                "--remove-remote-apps" => launch.remove_remote_apps = true,
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
        match (&mut launch.remote_app, action) {
            (Some(app), action) => app.action = action,
            (None, Some(_)) => return Err(format!("{ACTION_OPTION} needs {REMOTE_APP_OPTION}.")),
            (None, None) => {}
        }
        if launch.remote_app.is_some() && launch.check.is_some() {
            return Err("A startup check can't open a remote app.".into());
        }
        if launch.remove_remote_apps && (launch.remote_app.is_some() || launch.check.is_some()) {
            return Err("--remove-remote-apps runs on its own.".into());
        }
        Ok(launch)
    }
    pub fn activate_existing(&self) -> bool {
        !self.autostart && self.check.is_none()
    }
    /// Started for a login item or a remote app: the window stays hidden, and
    /// Newport waits in the tray.
    pub fn hidden(&self) -> bool {
        self.autostart || self.remote_app.is_some()
    }
}

fn available(debug: bool, profile_override: bool) -> bool {
    !debug && !profile_override
}

/// The retired production web view lived outside the profile. Development,
/// overridden profiles and startup checks must never inspect those paths.
#[cfg(target_os = "macos")]
pub fn legacy_webview_cleanup(launch: &Launch, debug: bool, profile_override: bool) -> bool {
    launch.check.is_none() && available(debug, profile_override)
}

pub fn profile_policy(debug: bool, path: Option<&std::path::Path>) -> Result<(), String> {
    match path {
        Some(path) if path.as_os_str().is_empty() => {
            Err("The profile directory override is empty.".into())
        }
        None if debug => Err("Development builds require an explicit NEWPORT_DATA_DIR.".into()),
        _ => Ok(()),
    }
}

pub fn registration(launch: &Launch) -> Result<Startup, String> {
    // No development or overridden profile may register itself.
    if !available(
        cfg!(debug_assertions),
        newport_backend::profile::data_directory_override().is_some(),
    ) {
        return Ok(Startup::disabled());
    }
    let startup = Startup::installed(std::env::current_exe().map_err(|error| error.to_string())?)?;
    // A startup check must not change the user's login items.
    if launch.check.is_none()
        && let Err(error) = startup.adopt_legacy_registration()
    {
        eprintln!("Launch at login could not be moved to Login Items: {error}");
    }
    Ok(startup)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn released_launch_uses_default_profile_and_debug_requires_an_override() {
        use std::path::Path;
        assert!(profile_policy(false, None).is_ok());
        assert!(profile_policy(true, None).is_err());
        for debug in [false, true] {
            assert!(profile_policy(debug, Some(Path::new(""))).is_err());
            assert!(profile_policy(debug, Some(Path::new("fixture"))).is_ok());
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

    #[test]
    fn a_remote_app_is_named_by_its_id_with_an_optional_action() {
        const ID: &str = "Newport.Remote.6fe02d2fc8ee3025544aa1e7.db207836d948804bf9711c8c";
        let parse = |args: &[&str]| Launch::from_args(args.iter().map(|arg| (*arg).to_owned()));
        let launch = parse(&["--remote-app", ID]).unwrap();
        assert_eq!(
            launch.remote_app,
            Some(RemoteApp {
                id: ID.into(),
                action: None
            })
        );
        assert!(launch.hidden());
        assert!(launch.activate_existing());
        assert_eq!(
            parse(&["--action", "new-window", "--remote-app", ID])
                .unwrap()
                .remote_app,
            Some(RemoteApp {
                id: ID.into(),
                action: Some("new-window".into())
            })
        );
        assert!(parse(&["--autostart"]).unwrap().hidden());
        assert!(!parse(&[]).unwrap().hidden());
        for args in [
            &["--remote-app"][..],
            &["--remote-app", ""],
            &["--remote-app", "--autostart"],
            &["--remote-app", "Newport Remote"],
            &["--remote-app", "Newport.Remote.a.b.c"],
            &["--remote-app", ID, "--remote-app", ID],
            &["--remote-app", ID, "--action"],
            &["--remote-app", ID, "--action", "a", "--action", "b"],
            &["--action", "new-window"],
            &["--remote-app", ID, "--check-backend"],
        ] {
            assert!(parse(args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn a_relaunch_command_parses_back_to_its_app() {
        const ID: &str = "Newport.Remote.6fe02d2fc8ee3025544aa1e7.db207836d948804bf9711c8c";
        let command = newport_domain::remote_app_identity::relaunch_command(
            r"C:\Program Files\Newport\newport.exe",
            None,
            ID,
            Some("new-window"),
        );
        // Neither the ID nor this action needs quotes.
        let arguments = command
            .strip_prefix("\"C:\\Program Files\\Newport\\newport.exe\" ")
            .unwrap()
            .split(' ')
            .map(str::to_owned);
        let launch = Launch::from_args(arguments).unwrap();
        assert_eq!(
            launch.remote_app,
            Some(RemoteApp {
                id: ID.into(),
                action: Some("new-window".into())
            })
        );
    }

    #[test]
    fn a_profile_directory_is_named_once() {
        const ID: &str = "Newport.Remote.6fe02d2fc8ee3025544aa1e7.db207836d948804bf9711c8c";
        let parse = |args: &[&str]| Launch::from_args(args.iter().map(|arg| (*arg).to_owned()));
        let profile = r"C:\NW\target\test profile";
        let launch = parse(&["--data-dir", profile, "--remote-app", ID]).unwrap();
        assert_eq!(launch.data_directory.as_deref(), Some(profile));
        assert_eq!(launch.remote_app.unwrap().id, ID);
        assert_eq!(parse(&[]).unwrap().data_directory, None);
        for args in [
            &["--data-dir"][..],
            &["--data-dir", ""],
            &["--data-dir", "--autostart"],
            &["--data-dir", "a", "--data-dir", "b"],
        ] {
            assert!(parse(args).is_err(), "{args:?}");
        }
        // What a pin of another profile runs reads back as it was written.
        let arguments = newport_domain::remote_app_identity::relaunch_arguments(
            Some(r"C:\NW\target\test profile\"),
            ID,
            None,
        );
        let launch = Launch::from_args(newport_domain::remote_app_identity::split_arguments(
            &arguments,
        ))
        .unwrap();
        assert_eq!(
            launch.data_directory.as_deref(),
            Some(r"C:\NW\target\test profile\")
        );
        assert_eq!(launch.remote_app.unwrap().id, ID);
    }

    #[test]
    fn removing_remote_apps_runs_alone() {
        let parse = |args: &[&str]| Launch::from_args(args.iter().map(|arg| (*arg).to_owned()));
        assert!(parse(&["--remove-remote-apps"]).unwrap().remove_remote_apps);
        assert!(!parse(&[]).unwrap().remove_remote_apps);
        assert!(parse(&["--remove-remote-apps", "--check-backend"]).is_err());
        assert!(
            parse(&[
                "--remove-remote-apps",
                "--remote-app",
                "Newport.Remote.6fe02d2fc8ee3025544aa1e7.db207836d948804bf9711c8c"
            ])
            .is_err()
        );
    }
}
