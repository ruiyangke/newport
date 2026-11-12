//! Identities for remote apps shown as apps of their own: on Windows, the
//! AppUserModelID the taskbar groups an app's windows by.
//!
//! An ID is `Newport.Remote.<server key>.<app key>`, each key
//! [`KEY_LENGTH`] lowercase hex digits of a SHA-256:
//!
//! - the server key hashes the profile directory, a newline and the server's
//!   id, so a development profile's apps never group with the installed
//!   Newport's;
//! - the app key hashes the app's stable id ([`RemoteApp::stable_id`]).
//!
//! The Remote apps presenter routes each window to an app (its desktop file,
//! else its app id, else none) and Newport computes the ID from that, so
//! windows, shortcuts and pins agree. The rule can never change without
//! orphaning every pin a user has.
use sha2::{Digest, Sha256};

/// Hex digits kept of each hash: 96 bits.
pub const KEY_LENGTH: usize = 24;
/// Microsoft's limit for an AppUserModelID.
pub const MAX_ID_LENGTH: usize = 128;
const PREFIX: &str = "Newport.Remote";

/// The app a remote window belongs to, as the presenter routes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteApp<'a> {
    /// An app with a desktop file, by desktop id, with or without `.desktop`.
    Desktop(&'a str),
    /// An app id with no desktop file.
    AppId(&'a str),
    /// Windows with no identity: one app per server.
    Server,
}

impl RemoteApp<'_> {
    /// The text an app key hashes: `desktop:<id>` without the `.desktop`
    /// suffix, `app:<id>`, or `server`. Windows may report a desktop id either
    /// way, and catalogue entries carry the suffix; both give one key.
    pub fn stable_id(self) -> String {
        match self {
            Self::Desktop(id) => format!("desktop:{}", id.strip_suffix(".desktop").unwrap_or(id)),
            Self::AppId(id) => format!("app:{id}"),
            Self::Server => "server".into(),
        }
    }
}

fn key(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .take(KEY_LENGTH / 2)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The key of a server in the profile at `profile_directory`, as Newport
/// opens it.
pub fn server_key(profile_directory: &str, server_id: &str) -> String {
    key(&format!("{profile_directory}\n{server_id}"))
}

pub fn app_key(app: RemoteApp) -> String {
    key(&app.stable_id())
}

/// The AppUserModelID of `app`'s windows on the server with `server_key`.
pub fn app_user_model_id(server_key: &str, app: RemoteApp) -> String {
    format!("{PREFIX}.{server_key}.{}", app_key(app))
}

/// Whether `id` is a remote app's, as [`app_user_model_id`] makes them.
pub fn is_remote_app_id(id: &str) -> bool {
    id.strip_prefix(PREFIX)
        .is_some_and(|rest| rest.starts_with('.'))
}

/// Microsoft's rules for an application-defined AppUserModelID: at most 128
/// characters, no spaces, and `Company.Product[.SubProduct[.Version]]`.
pub fn is_valid_app_user_model_id(id: &str) -> bool {
    let sections = id.split('.').count();
    id.chars().count() <= MAX_ID_LENGTH
        && !id.chars().any(char::is_whitespace)
        && (2..=4).contains(&sections)
        && id.split('.').all(|section| !section.is_empty())
}

/// The option that opens a remote app by its ID: `--remote-app <id>`.
pub const REMOTE_APP_OPTION: &str = "--remote-app";
/// The option that runs one of the app's actions, with `--remote-app`:
/// `--action <id>`.
pub const ACTION_OPTION: &str = "--action";
/// The option that names the profile directory, as `NEWPORT_DATA_DIR` does:
/// `--data-dir <path>`.
pub const DATA_DIR_OPTION: &str = "--data-dir";

/// The AppUserModelID of Newport's own windows, so they never join the
/// taskbar group of the pin or shortcut that started Newport: `Newport.App`
/// for the default profile, and `Newport.App.<key>` for the profile at
/// `data_directory` (the `NEWPORT_DATA_DIR` or `--data-dir` value), keyed as a
/// server is.
pub fn newport_app_user_model_id(data_directory: Option<&str>) -> String {
    match data_directory {
        None => NEWPORT_APP_ID.into(),
        Some(directory) => format!("{NEWPORT_APP_ID}.{}", key(directory)),
    }
}
const NEWPORT_APP_ID: &str = "Newport.App";

/// What follows the program name to open the app with `id` in the profile
/// at `data_directory` (`None` for the default one).
pub fn relaunch_arguments(data_directory: Option<&str>, id: &str, action: Option<&str>) -> String {
    let mut arguments = String::new();
    if let Some(directory) = data_directory {
        arguments.push_str(&format!("{DATA_DIR_OPTION} {} ", quote_argument(directory)));
    }
    arguments.push_str(&format!("{REMOTE_APP_OPTION} {}", quote_argument(id)));
    if let Some(action) = action {
        arguments.push_str(&format!(" {ACTION_OPTION} {}", quote_argument(action)));
    }
    arguments
}

/// What a taskbar pin or a shortcut runs to open the app with `id`: Newport
/// at `executable`, quoted as Windows parses a command line.
pub fn relaunch_command(
    executable: &str,
    data_directory: Option<&str>,
    id: &str,
    action: Option<&str>,
) -> String {
    // The program name ends at the next quote, with no escapes: a Windows
    // path can't hold one.
    format!(
        "\"{executable}\" {}",
        relaunch_arguments(data_directory, id, action)
    )
}

/// What a taskbar pin of Newport's own window runs in the profile at
/// `data_directory`: Newport alone would open the default profile.
pub fn profile_command(executable: &str, data_directory: &str) -> String {
    format!(
        "\"{executable}\" {DATA_DIR_OPTION} {}",
        quote_argument(data_directory)
    )
}

/// What a Newport shortcut's arguments open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LaunchArguments {
    /// The profile directory, `None` for the default profile.
    pub data_directory: Option<String>,
    pub remote_app: Option<String>,
}

/// Reads the options Newport's shortcuts carry; anything else is ignored.
pub fn launch_arguments(arguments: &str) -> LaunchArguments {
    let mut launch = LaunchArguments::default();
    let mut arguments = split_arguments(arguments).into_iter();
    while let Some(argument) = arguments.next() {
        let slot = match argument.as_str() {
            DATA_DIR_OPTION => &mut launch.data_directory,
            REMOTE_APP_OPTION => &mut launch.remote_app,
            _ => continue,
        };
        *slot = arguments.next();
    }
    launch
}

/// Whether a shortcut to Newport's own executable, with these arguments and
/// AppUserModelID, should be given this profile's ID: it opens this profile
/// rather than a remote app, and has no ID or another profile's.
pub fn needs_newport_id(arguments: &str, app_id: &str, data_directory: Option<&str>) -> bool {
    let launch = launch_arguments(arguments);
    let ours = newport_app_user_model_id(data_directory);
    launch.remote_app.is_none()
        && launch.data_directory.as_deref() == data_directory
        && app_id != ours
        && (app_id.is_empty()
            || app_id == NEWPORT_APP_ID
            || app_id.starts_with(&format!("{NEWPORT_APP_ID}.")))
}

/// Arguments after the program name, split as the C runtime and
/// `CommandLineToArgvW` split them.
pub fn split_arguments(arguments: &str) -> Vec<String> {
    let mut parsed = Vec::new();
    let mut chars = arguments.chars().peekable();
    loop {
        while chars.next_if(|c| *c == ' ' || *c == '\t').is_some() {}
        if chars.peek().is_none() {
            return parsed;
        }
        let mut argument = String::new();
        let mut quoted = false;
        while let Some(&c) = chars.peek() {
            if !quoted && (c == ' ' || c == '\t') {
                break;
            }
            chars.next();
            match c {
                '\\' => {
                    let mut count = 1;
                    while chars.next_if_eq(&'\\').is_some() {
                        count += 1;
                    }
                    if chars.peek() == Some(&'"') {
                        argument.extend(std::iter::repeat_n('\\', count / 2));
                        if count % 2 == 1 {
                            chars.next();
                            argument.push('"');
                        }
                    } else {
                        argument.extend(std::iter::repeat_n('\\', count));
                    }
                }
                '"' => quoted = !quoted,
                _ => argument.push(c),
            }
        }
        parsed.push(argument);
    }
}

/// One argument after the program name, quoted when it must be, so the C
/// runtime and `CommandLineToArgvW` read it back unchanged.
pub fn quote_argument(argument: &str) -> String {
    if !argument.is_empty() && !argument.contains([' ', '\t', '\n', '\x0b', '"']) {
        return argument.to_owned();
    }
    let mut quoted = String::from('"');
    let mut backslashes = 0;
    for c in argument.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                // Backslashes before a quote are escaped, and so is the quote.
                quoted.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.extend(std::iter::repeat_n('\\', backslashes));
                quoted.push(c);
                backslashes = 0;
            }
        }
    }
    // Before the closing quote, too.
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    quoted
}

/// A remote app as its name is shown beside other servers' apps.
#[derive(Clone, Copy, Debug)]
pub struct NamedApp<'a> {
    pub server_id: &'a str,
    pub server_name: &'a str,
    pub name: &'a str,
}

/// Text shown as a name: single spaces, no control characters.
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `app`'s name among `apps`: its own name, with its server's name added only
/// when another server has an app of the same name, as "Calculator (lab)".
pub fn distinct_name(app: NamedApp, apps: &[NamedApp]) -> String {
    let name = clean(app.name);
    let collides = apps.iter().any(|other| {
        other.server_id != app.server_id && clean(other.name).to_lowercase() == name.to_lowercase()
    });
    if collides {
        format!("{name} ({})", clean(app.server_name))
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROFILE: &str = r"C:\Users\ada\AppData\Local\Newport";
    const SERVER: &str = "6f1c1f4e-1f43-4b8e-9d55-7f6a63a1b2c4";

    #[test]
    fn ids_are_stable() {
        let server = server_key(PROFILE, SERVER);
        assert_eq!(server, "6fe02d2fc8ee3025544aa1e7");
        assert_eq!(
            app_user_model_id(&server, RemoteApp::Desktop("org.gnome.Calculator.desktop")),
            "Newport.Remote.6fe02d2fc8ee3025544aa1e7.db207836d948804bf9711c8c"
        );
        assert_eq!(app_key(RemoteApp::Server), key("server"));
        assert_eq!(app_key(RemoteApp::AppId("xterm")), key("app:xterm"));
    }

    #[test]
    fn ids_follow_microsofts_rules() {
        let server = server_key(PROFILE, SERVER);
        for app in [
            RemoteApp::Desktop("org.gnome.Calculator.desktop"),
            RemoteApp::AppId("a very.long app id with spaces and dots".repeat(8).as_str()),
            RemoteApp::Server,
        ] {
            let id = app_user_model_id(&server, app);
            assert!(is_valid_app_user_model_id(&id), "{id}");
            assert_eq!(id.split('.').count(), 4, "{id}");
            assert_eq!(id.len(), PREFIX.len() + 2 * (1 + KEY_LENGTH));
            assert!(
                id.split('.').skip(2).all(|key| key.len() == KEY_LENGTH
                    && key
                        .chars()
                        .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase())),
                "{id}"
            );
        }
        assert!(is_remote_app_id(&app_user_model_id(
            &server,
            RemoteApp::Server
        )));
        assert!(!is_remote_app_id("Newport.App"));
        assert!(!is_remote_app_id("Newport.RemoteX.a.b"));
        assert!(!is_valid_app_user_model_id("Newport"));
        assert!(!is_valid_app_user_model_id("Newport.Remote.a.b.c"));
        assert!(!is_valid_app_user_model_id("Newport.Remote App"));
        assert!(!is_valid_app_user_model_id("Newport..App"));
        assert!(!is_valid_app_user_model_id(&format!(
            "Newport.{}",
            "a".repeat(121)
        )));
        assert!(is_valid_app_user_model_id(&format!(
            "Newport.{}",
            "a".repeat(120)
        )));
    }

    #[test]
    fn a_desktop_id_with_or_without_its_suffix_is_one_app() {
        assert_eq!(
            app_key(RemoteApp::Desktop("org.gnome.Calculator.desktop")),
            app_key(RemoteApp::Desktop("org.gnome.Calculator"))
        );
        assert_ne!(
            app_key(RemoteApp::Desktop("org.gnome.Calculator")),
            app_key(RemoteApp::AppId("org.gnome.Calculator"))
        );
        assert_eq!(
            RemoteApp::Desktop("org.gnome.Calculator.desktop").stable_id(),
            "desktop:org.gnome.Calculator"
        );
    }

    #[test]
    fn profiles_and_servers_get_their_own_keys() {
        let release = server_key(PROFILE, SERVER);
        let development = server_key(r"C:\NW\target\profile", SERVER);
        let other = server_key(PROFILE, "0c8a3e57-2a0d-4d6b-a7a5-3b4f0d9e8c11");
        assert_ne!(release, development);
        assert_ne!(release, other);
        let calculator = RemoteApp::Desktop("org.gnome.Calculator.desktop");
        assert_ne!(
            app_user_model_id(&release, calculator),
            app_user_model_id(&other, calculator)
        );
        // The newline keeps a profile ending in a server id's prefix apart.
        assert_ne!(server_key("a", "bc"), server_key("ab", "c"));
    }

    #[test]
    fn a_server_name_is_added_only_on_a_collision() {
        let lab = NamedApp {
            server_id: "1",
            server_name: "lab",
            name: "Calculator",
        };
        let build = NamedApp {
            server_id: "2",
            server_name: "build box",
            name: " calculator ",
        };
        let terminal = NamedApp {
            server_id: "2",
            server_name: "build box",
            name: "Terminal\n",
        };
        let apps = [lab, build, terminal];
        assert_eq!(distinct_name(lab, &apps), "Calculator (lab)");
        assert_eq!(distinct_name(build, &apps), "calculator (build box)");
        assert_eq!(distinct_name(terminal, &apps), "Terminal");
        assert_eq!(distinct_name(lab, &[lab, terminal]), "Calculator");
        // Its own entry, or a second of its windows, is no collision.
        assert_eq!(distinct_name(lab, &[lab, lab]), "Calculator");
    }

    #[test]
    fn a_relaunch_command_names_newport_and_the_app() {
        let id = "Newport.Remote.6fe02d2fc8ee3025544aa1e7.db207836d948804bf9711c8c";
        let executable = r"C:\Users\Ada Lovelace\AppData\Local\Programs\Newport\newport.exe";
        let command = relaunch_command(executable, None, id, None);
        assert_eq!(command, format!("\"{executable}\" --remote-app {id}"));
        let with_action = relaunch_command(executable, None, id, Some("new window"));
        let arguments = with_action
            .strip_prefix(&format!("\"{executable}\" "))
            .unwrap();
        assert_eq!(
            split_arguments(arguments),
            ["--remote-app", id, "--action", "new window"]
        );
    }

    #[test]
    fn another_profile_travels_with_the_command() {
        let id = "Newport.Remote.6fe02d2fc8ee3025544aa1e7.db207836d948804bf9711c8c";
        let profile = r"C:\NW\target\test profile\";
        let arguments = relaunch_arguments(Some(profile), id, Some("new-window"));
        assert_eq!(
            arguments,
            format!(
                r#"--data-dir "C:\NW\target\test profile\\" --remote-app {id} --action new-window"#
            )
        );
        assert_eq!(
            split_arguments(&arguments),
            [
                "--data-dir",
                profile,
                "--remote-app",
                id,
                "--action",
                "new-window"
            ]
        );
        assert_eq!(
            launch_arguments(&arguments),
            LaunchArguments {
                data_directory: Some(profile.into()),
                remote_app: Some(id.into()),
            }
        );
        let command = relaunch_command(r"C:\NW\newport.exe", Some(r"C:\p"), id, None);
        assert_eq!(
            command,
            format!(r#""C:\NW\newport.exe" --data-dir C:\p --remote-app {id}"#)
        );
        assert_eq!(launch_arguments(""), LaunchArguments::default());
        assert_eq!(
            launch_arguments("--autostart --data-dir"),
            LaunchArguments::default()
        );
    }

    #[test]
    fn newport_itself_reopens_its_profile() {
        let command = profile_command(r"C:\NW\newport.exe", r"C:\test profile");
        assert_eq!(
            command,
            r#""C:\NW\newport.exe" --data-dir "C:\test profile""#
        );
        let arguments = command.strip_prefix(r#""C:\NW\newport.exe" "#).unwrap();
        assert_eq!(
            launch_arguments(arguments).data_directory.as_deref(),
            Some(r"C:\test profile")
        );
        assert!(needs_newport_id(arguments, "", Some(r"C:\test profile")));
    }

    #[test]
    fn newport_has_one_id_per_profile() {
        assert_eq!(newport_app_user_model_id(None), "Newport.App");
        let test = newport_app_user_model_id(Some(PROFILE));
        assert_eq!(test, format!("Newport.App.{}", key(PROFILE)));
        assert_eq!(test.len(), "Newport.App.".len() + KEY_LENGTH);
        assert_ne!(test, newport_app_user_model_id(Some(r"C:\NW\profile")));
        for id in [newport_app_user_model_id(None), test] {
            assert!(is_valid_app_user_model_id(&id), "{id}");
        }
    }

    #[test]
    fn only_shortcuts_that_open_this_profile_get_its_id() {
        let test = newport_app_user_model_id(Some(r"C:\p"));
        let remote = "Newport.Remote.6fe02d2fc8ee3025544aa1e7.db207836d948804bf9711c8c";
        // The installer's shortcut, for the default profile.
        assert!(needs_newport_id("", "", None));
        assert!(needs_newport_id("", &test, None));
        assert!(!needs_newport_id("", "Newport.App", None));
        assert!(!needs_newport_id("", "", Some(r"C:\p")));
        assert!(needs_newport_id(
            r"--data-dir C:\p",
            "Newport.App",
            Some(r"C:\p")
        ));
        assert!(!needs_newport_id(r"--data-dir C:\p", &test, Some(r"C:\p")));
        // A remote app's shortcut, and someone else's ID, are left alone.
        assert!(!needs_newport_id(
            &format!("--remote-app {remote}"),
            remote,
            None
        ));
        assert!(!needs_newport_id(
            &format!("--remote-app {remote}"),
            "",
            None
        ));
        assert!(!needs_newport_id("", "Contoso.App", None));
        assert!(!needs_newport_id("", "Newport.Application", None));
    }

    #[test]
    fn quoted_arguments_read_back_unchanged() {
        for argument in [
            "plain",
            "",
            "two words",
            r#"say "hi""#,
            r"C:\path\",
            r"C:\path with space\",
            r#"back\"quote"#,
            r"\\server\share",
            "tab\there",
        ] {
            let quoted = quote_argument(argument);
            assert_eq!(split_arguments(&quoted), [argument], "{quoted}");
            assert_eq!(
                split_arguments(&format!("{quoted} next")),
                [argument, "next"],
                "{quoted}"
            );
        }
        assert_eq!(quote_argument("plain"), "plain");
    }
}
