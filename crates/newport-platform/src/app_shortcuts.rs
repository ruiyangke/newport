//! Shell shortcuts (`.lnk` files) that carry an AppUserModelID, so the
//! taskbar groups what they start under the ID: Newport's own shortcuts, and
//! the Start menu shortcuts of remote apps. Also what else the shell keeps by
//! an AppUserModelID: an app's jump list, and the name and icon its
//! notifications show. Windows only; elsewhere there are no folders and
//! nothing is written.
//!
//! Every shell call runs on one thread of its own in a single-threaded COM
//! apartment, which the shell's objects need, and waits for it: call from a
//! worker, never from the UI thread.
use std::path::{Path, PathBuf};

#[cfg(windows)]
#[path = "app_shortcuts/windows.rs"]
pub(crate) mod os;

/// A shortcut, as written or read back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shortcut {
    pub target: PathBuf,
    pub arguments: String,
    /// The icon file, `None` for the target's own, and its index.
    pub icon: Option<(PathBuf, i32)>,
    pub description: String,
    /// Empty when the shortcut has none.
    pub app_id: String,
}

/// The current user's Start menu programs folder.
pub fn programs_folder() -> Option<PathBuf> {
    #[cfg(windows)]
    return os::programs_folder();
    #[cfg(not(windows))]
    None
}

/// Where the shell keeps the current user's pins: `TaskBar` holds a
/// shortcut per taskbar pin, and `ImplicitAppShortcuts` one per pinned
/// window that had no shortcut of its own.
pub fn pinned_folder() -> Option<PathBuf> {
    #[cfg(windows)]
    return os::pinned_folder();
    #[cfg(not(windows))]
    None
}

/// Writes the shortcut whole: a temporary file in the same folder, moved
/// over `path`. Creates the folder.
pub fn write(path: &Path, shortcut: &Shortcut) -> Result<(), String> {
    #[cfg(windows)]
    return os::write(path, shortcut);
    #[cfg(not(windows))]
    {
        let _ = (path, shortcut);
        Err(unsupported())
    }
}

pub fn read(path: &Path) -> Result<Shortcut, String> {
    #[cfg(windows)]
    return os::read(path);
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(unsupported())
    }
}

/// The shortcut's AppUserModelID, empty when it has none.
pub fn read_app_id(path: &Path) -> Result<String, String> {
    read(path).map(|shortcut| shortcut.app_id)
}

/// Gives an existing shortcut another AppUserModelID, keeping the rest.
pub fn set_app_id(path: &Path, app_id: &str) -> Result<(), String> {
    let shortcut = read(path)?;
    write(
        path,
        &Shortcut {
            app_id: app_id.to_owned(),
            ..shortcut
        },
    )
}

/// Removes the shortcut; one already gone is no error.
pub fn remove(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("Cannot remove {}: {error}", path.display()))
        }
        _ => Ok(()),
    }
}

/// The `.lnk` files directly in `folder`, none when it doesn't exist.
pub fn list(folder: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut links: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
        })
        .collect();
    links.sort();
    links
}

/// Gives the process an AppUserModelID of its own, so its windows group
/// under it rather than under whatever shortcut started it. Call before any
/// window or tray icon exists.
pub fn set_process_app_id(app_id: &str) -> Result<(), String> {
    #[cfg(windows)]
    return os::set_process_app_id(app_id);
    #[cfg(not(windows))]
    {
        let _ = app_id;
        Ok(())
    }
}

/// A task in an app's jump list: a command, shown by its title.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Task {
    pub title: String,
    pub target: PathBuf,
    pub arguments: String,
    /// The icon file and its index, `None` for the target's own.
    pub icon: Option<(PathBuf, i32)>,
}

/// Replaces the tasks of the jump list the taskbar shows for `app_id`'s
/// button and pins. No tasks removes the list.
pub fn write_jump_list(app_id: &str, tasks: &[Task]) -> Result<(), String> {
    #[cfg(windows)]
    return if tasks.is_empty() {
        os::remove_jump_list(app_id)
    } else {
        os::write_jump_list(app_id, tasks)
    };
    #[cfg(not(windows))]
    {
        let _ = (app_id, tasks);
        Err(unsupported())
    }
}

/// Removes `app_id`'s jump list; one it never had is no error.
pub fn remove_jump_list(app_id: &str) -> Result<(), String> {
    #[cfg(windows)]
    return os::remove_jump_list(app_id);
    #[cfg(not(windows))]
    {
        let _ = app_id;
        Err(unsupported())
    }
}

/// Where the current user's AppUserModelIDs are registered, under
/// `HKEY_CURRENT_USER`: what Windows notifications of an ID show.
pub const APP_IDS_KEY: &str = r"Software\Classes\AppUserModelId";

/// Registers `app_id` under the `HKEY_CURRENT_USER` key `key` (in practice
/// [`APP_IDS_KEY`]) with the name and icon image its notifications show.
pub fn register_app_id(
    key: &str,
    app_id: &str,
    display_name: &str,
    icon: Option<&Path>,
) -> Result<(), String> {
    #[cfg(windows)]
    return os::register_app_id(key, app_id, display_name, icon);
    #[cfg(not(windows))]
    {
        let _ = (key, app_id, display_name, icon);
        Err(unsupported())
    }
}

/// Removes `app_id`'s registration; one never made is no error.
pub fn unregister_app_id(key: &str, app_id: &str) -> Result<(), String> {
    #[cfg(windows)]
    return os::unregister_app_id(key, app_id);
    #[cfg(not(windows))]
    {
        let _ = (key, app_id);
        Err(unsupported())
    }
}

/// The IDs registered under `key`, none when it doesn't exist.
pub fn registered_app_ids(key: &str) -> Vec<String> {
    #[cfg(windows)]
    return os::registered_app_ids(key);
    #[cfg(not(windows))]
    {
        let _ = key;
        Vec::new()
    }
}

#[cfg(not(windows))]
fn unsupported() -> String {
    "Shortcuts are only written on Windows.".into()
}

#[cfg(all(test, windows))]
mod tests;
