//! The Start menu shortcuts of the remote apps a person added: one
//! `<App>.lnk` per app in one folder, [`FOLDER`], under the Start menu's
//! programs. Windows 11 neither lists nor finds a shortcut in a folder inside
//! a Start menu folder, so servers get no folders of their own: an app's
//! name gets its server's only when two servers' apps share it
//! ([`crate::remote_app_identity::distinct_name`]).
//!
//! Every profile, and every build, shares the folder. A shortcut belongs to
//! the profile its arguments name ([`owned_by`]); the others' names are
//! taken, and never changed.
use crate::{
    remote_app_identity::{NamedApp, distinct_name, launch_arguments, relaunch_arguments},
    remote_app_registry::Entry,
    windows_file_names,
};

pub const FOLDER: &str = "Newport Apps";
pub const EXTENSION: &str = ".lnk";
/// A name with nothing Windows can keep in it.
const FALLBACK: &str = "Remote app";
/// Numbered names tried after a name that is taken.
const MAX_NUMBER: usize = 99;

/// A shortcut as Newport writes it, or reads one back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Link {
    pub target: String,
    pub arguments: String,
    /// The icon file, empty for the target's own, and its index.
    pub icon: (String, i32),
    pub description: String,
    pub app_id: String,
}

/// Whether the shortcut opens a remote app in the profile at
/// `data_directory` (`None` for the default profile).
pub fn owned_by(link: &Link, data_directory: Option<&str>) -> bool {
    let launch = launch_arguments(&link.arguments);
    launch.remote_app.is_some() && launch.data_directory.as_deref() == data_directory
}

fn same_name(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// The file name, extension included, of each added app's shortcut, by ID,
/// in a folder `folder_length` UTF-16 units long where `taken` names belong
/// to others. Names are the apps' distinct names, made safe for Windows; a
/// name already used gets " (2)", " (3)" and so on. An app whose name can't
/// fit gets no shortcut.
pub fn file_names(
    added: &[&Entry],
    taken: &[String],
    folder_length: usize,
) -> Vec<(String, String)> {
    let mut added = added.to_vec();
    // The same names for the same apps on every run.
    added.sort_by(|a, b| a.id.cmp(&b.id));
    let named: Vec<_> = added
        .iter()
        .map(|entry| NamedApp {
            server_id: &entry.server,
            server_name: &entry.server_name,
            name: &entry.name,
        })
        .collect();
    let mut used: Vec<String> = taken.to_vec();
    let mut names = Vec::new();
    for (entry, app) in added.iter().zip(&named) {
        let name = windows_file_names::file_name(&distinct_name(*app, &named), FALLBACK);
        let chosen = (1..=MAX_NUMBER).find_map(|number| {
            let suffix = if number == 1 {
                String::new()
            } else {
                format!(" ({number})")
            };
            let base =
                windows_file_names::fit(&name, &format!("{suffix}{EXTENSION}"), folder_length)?;
            let candidate = format!("{base}{suffix}{EXTENSION}");
            (!used.iter().any(|known| same_name(known, &candidate))).then_some(candidate)
        });
        if let Some(chosen) = chosen {
            used.push(chosen.clone());
            names.push((entry.id.clone(), chosen));
        }
    }
    names
}

/// What brings the folder to what's wanted.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// File names to remove.
    pub remove: Vec<String>,
    /// File names to write, with what.
    pub write: Vec<(String, Link)>,
}

/// From the profile's own shortcuts (`owned`) to `wanted`, by file name:
/// shortcuts no app wants go, and wanted ones that are missing or differ are
/// written.
pub fn plan(owned: &[(String, Link)], wanted: &[(String, Link)]) -> Plan {
    Plan {
        remove: owned
            .iter()
            .filter(|(name, _)| !wanted.iter().any(|(want, _)| same_name(want, name)))
            .map(|(name, _)| name.clone())
            .collect(),
        write: wanted
            .iter()
            .filter(|(want, link)| {
                !owned
                    .iter()
                    .any(|(name, known)| same_name(want, name) && known == link)
            })
            .cloned()
            .collect(),
    }
}

/// A taskbar pin of one of this profile's apps (`link.app_id`), brought up
/// to date: Newport at `executable`, opening the app in the profile at
/// `data_directory`, with an icon file that `exists`, else `icon` (the app's
/// own, empty for none). `None` when it is up to date. The ID, name and
/// description stay: the shell finds the pin by them.
pub fn repaired_pin(
    link: &Link,
    executable: &str,
    data_directory: Option<&str>,
    icon: &str,
    exists: impl Fn(&str) -> bool,
) -> Option<Link> {
    let launch = launch_arguments(&link.arguments);
    // Windows paths ignore case.
    let target = link.target.to_lowercase() == executable.to_lowercase();
    let arguments = launch.remote_app.as_deref() == Some(link.app_id.as_str())
        && launch.data_directory.as_deref() == data_directory;
    let icon_kept = link.icon.0.is_empty() || exists(&link.icon.0);
    if target && arguments && icon_kept {
        return None;
    }
    Some(Link {
        target: executable.to_owned(),
        arguments: if arguments {
            link.arguments.clone()
        } else {
            relaunch_arguments(data_directory, &link.app_id, None)
        },
        icon: if icon_kept {
            link.icon.clone()
        } else if !icon.is_empty() && exists(icon) {
            (icon.to_owned(), 0)
        } else {
            (String::new(), 0)
        },
        ..link.clone()
    })
}

/// Most tasks a jump list holds.
pub const MAX_TASKS: usize = 10;

/// A jump list task: one of the app's actions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    pub title: String,
    /// What follows Newport's program name.
    pub arguments: String,
}

/// The tasks of an added app's jump list, in the profile at
/// `data_directory`: its first [`MAX_TASKS`] actions.
pub fn jump_list(entry: &Entry, data_directory: Option<&str>) -> Vec<Task> {
    entry
        .actions
        .iter()
        .take(MAX_TASKS)
        .map(|action| Task {
            title: action.name.clone(),
            arguments: relaunch_arguments(data_directory, &entry.id, Some(&action.id)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_app_registry::App;

    fn entry(id: &str, server: &str, server_name: &str, name: &str) -> Entry {
        Entry {
            id: id.into(),
            server: server.into(),
            server_name: server_name.into(),
            app: App::DesktopId(format!("{name}.desktop")),
            name: name.into(),
            added: true,
            last_used: 0,
            actions: Vec::new(),
        }
    }

    fn link(arguments: &str) -> Link {
        Link {
            target: r"C:\Newport\newport.exe".into(),
            arguments: arguments.into(),
            icon: (r"C:\p\remote-app-icons\x.ico".into(), 0),
            description: "Calculator on lab".into(),
            app_id: "Newport.Remote.a.b".into(),
        }
    }

    #[test]
    fn apps_are_named_by_their_distinct_names() {
        let lab = entry("1", "s1", "lab", "Calculator");
        let build = entry("2", "s2", "build: box", "Calculator");
        let terminal = entry("3", "s2", "build: box", "Terminal");
        let names = file_names(&[&terminal, &build, &lab], &[], 60);
        assert_eq!(
            names,
            [
                ("1".into(), "Calculator (lab).lnk".into()),
                ("2".into(), "Calculator (build box).lnk".into()),
                ("3".into(), "Terminal.lnk".into()),
            ]
        );
        assert_eq!(
            file_names(&[&lab, &terminal], &[], 60),
            [
                ("1".into(), "Calculator.lnk".into()),
                ("3".into(), "Terminal.lnk".into())
            ]
        );
    }

    #[test]
    fn taken_and_repeated_names_are_numbered() {
        // Another profile's Calculator, and two apps of one server sharing a name.
        let a = entry("a", "s1", "lab", "Calculator");
        let b = entry("b", "s1", "lab", "calculator");
        let taken = ["CALCULATOR.lnk".to_owned()];
        assert_eq!(
            file_names(&[&a, &b], &taken, 60),
            [
                ("a".into(), "Calculator (2).lnk".into()),
                ("b".into(), "calculator (3).lnk".into())
            ]
        );
    }

    #[test]
    fn names_are_safe_and_fit() {
        let odd = entry("a", "s1", "lab", "CON");
        let empty = entry("b", "s1", "lab", " ?? ");
        let long = entry("c", "s1", "lab", &"Long name ".repeat(30));
        let names = file_names(&[&odd, &empty, &long], &[], 200);
        assert_eq!(names[0].1, "CON_.lnk");
        assert_eq!(names[1].1, "Remote app.lnk");
        assert!(200 + 1 + names[2].1.len() < windows_file_names::MAX_PATH);
        assert!(names[2].1.starts_with("Long name Long"));
        // A folder with no room leaves the app out.
        assert!(file_names(&[&odd], &[], 256).is_empty());
    }

    #[test]
    fn a_shortcut_belongs_to_the_profile_it_opens() {
        let id = "Newport.Remote.a.b";
        assert!(owned_by(&link(&format!("--remote-app {id}")), None));
        assert!(!owned_by(
            &link(&format!("--remote-app {id}")),
            Some(r"C:\p")
        ));
        let other = link(&format!(
            r#"--data-dir "C:\test profile" --remote-app {id}"#
        ));
        assert!(owned_by(&other, Some(r"C:\test profile")));
        assert!(!owned_by(&other, None));
        assert!(!owned_by(&link(""), None), "Newport itself");
    }

    #[test]
    fn a_plan_removes_strays_and_writes_what_differs() {
        let current = link("--remote-app Newport.Remote.a.b");
        let outdated = Link {
            target: r"C:\Old\newport.exe".into(),
            ..current.clone()
        };
        let owned = vec![
            ("Calculator.lnk".to_owned(), current.clone()),
            ("terminal.lnk".to_owned(), outdated.clone()),
            ("Gone.lnk".to_owned(), current.clone()),
        ];
        let wanted = vec![
            ("Calculator.lnk".to_owned(), current.clone()),
            ("Terminal.lnk".to_owned(), current.clone()),
            ("Mousepad.lnk".to_owned(), current.clone()),
        ];
        assert_eq!(
            plan(&owned, &wanted),
            Plan {
                remove: vec!["Gone.lnk".into()],
                write: vec![
                    ("Terminal.lnk".into(), current.clone()),
                    ("Mousepad.lnk".into(), current.clone()),
                ],
            }
        );
        assert_eq!(plan(&wanted, &wanted), Plan::default());
        assert_eq!(
            plan(&owned, &[]).remove,
            ["Calculator.lnk", "terminal.lnk", "Gone.lnk"]
        );
    }

    #[test]
    fn a_pin_of_this_profile_is_repaired_only_when_outdated() {
        let id = "Newport.Remote.a.b";
        let executable = r"C:\Newport\newport.exe";
        let icon = r"C:\p\remote-app-icons\x.ico";
        let exists = |path: &str| path == icon;
        let current = Link {
            arguments: format!("--remote-app {id}"),
            ..link("")
        };
        assert_eq!(repaired_pin(&current, executable, None, icon, exists), None);
        // Case and the shell's own icon are fine.
        let shell = Link {
            target: r"c:\newport\NEWPORT.EXE".into(),
            icon: (String::new(), 0),
            ..current.clone()
        };
        assert_eq!(repaired_pin(&shell, executable, None, icon, exists), None);
        // Newport moved, and its old icon went with it.
        let moved = Link {
            target: r"C:\Old\newport.exe".into(),
            icon: (r"C:\Old\newport.exe".into(), 0),
            ..current.clone()
        };
        assert_eq!(
            repaired_pin(&moved, executable, None, icon, exists),
            Some(current.clone())
        );
        // A profile missing or another, and an icon that is gone with none to
        // replace it.
        let profile = r"C:\test profile";
        let missing = Link {
            icon: (r"C:\gone.ico".into(), 0),
            ..current.clone()
        };
        let repaired = repaired_pin(&missing, executable, Some(profile), "", exists).unwrap();
        assert_eq!(
            repaired,
            Link {
                arguments: format!(r#"--data-dir "{profile}" --remote-app {id}"#),
                icon: (String::new(), 0),
                ..current.clone()
            }
        );
        assert_eq!(
            repaired_pin(&repaired, executable, Some(profile), "", exists),
            None
        );
        let other = Link {
            arguments: format!(r"--data-dir C:\other --remote-app {id}"),
            ..current.clone()
        };
        assert_eq!(
            repaired_pin(&other, executable, None, icon, exists),
            Some(current.clone())
        );
        // Arguments naming another app.
        let wrong = Link {
            arguments: "--remote-app Newport.Remote.a.c".into(),
            ..current.clone()
        };
        assert_eq!(
            repaired_pin(&wrong, executable, None, icon, exists),
            Some(current)
        );
    }

    #[test]
    fn a_jump_list_runs_the_first_ten_actions() {
        use crate::remote_app_registry::Action;
        let mut app = entry("Newport.Remote.a.b", "s1", "lab", "Chromium");
        assert!(jump_list(&app, None).is_empty());
        app.actions = (0..12)
            .map(|n| Action {
                id: format!("action-{n}"),
                name: format!("Action {n}"),
            })
            .collect();
        let tasks = jump_list(&app, Some(r"C:\test profile"));
        assert_eq!(tasks.len(), MAX_TASKS);
        assert_eq!(
            tasks[0],
            Task {
                title: "Action 0".into(),
                arguments: r#"--data-dir "C:\test profile" --remote-app Newport.Remote.a.b --action action-0"#
                    .into(),
            }
        );
        assert_eq!(tasks[9].title, "Action 9");
    }
}
