//! The Tauri app that Newport replaced kept web view data outside the profile:
//! WebKit storage for its window, its window placement, and WebKit's
//! per-process temporary folders. Nothing reads them now; this removes them.
//! Only paths that are unambiguously the web view's go: the app's own
//! temporary folder stays, since the system may use it for this app too.
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) fn remove(launch: &crate::startup::Launch, debug: bool, profile_override: bool) {
    remove_with(launch, debug, profile_override, || {
        Some((
            PathBuf::from(std::env::var_os("HOME")?),
            std::env::temp_dir(),
        ))
    });
}

fn remove_with(
    launch: &crate::startup::Launch,
    debug: bool,
    profile_override: bool,
    paths: impl FnOnce() -> Option<(PathBuf, PathBuf)>,
) {
    if !crate::startup::legacy_webview_cleanup(launch, debug, profile_override) {
        return;
    }
    let Some((home, temporary)) = paths() else {
        return;
    };
    for storage in ["app.newport", "newport"] {
        remove_all(&home.join("Library/WebKit").join(storage));
    }
    let tauri = home.join("Library/Application Support/app.newport");
    remove_file(&tauri.join(".window-state.json"));
    // Its folder goes once nothing else is in it.
    let _ = fs::remove_dir(&tauri);
    if let Ok(entries) = fs::read_dir(&temporary) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let webkit = name.to_str().is_some_and(|name| {
                name.starts_with("com.apple.WebKit.")
                    && (name.ends_with("+app.newport") || name.contains("+newport-"))
            });
            if webkit {
                // Only an empty one: WebKit's folders hold nothing once its
                // processes are gone.
                let _ = fs::remove_dir(entry.path());
            }
        }
    }
}

fn remove_all(path: &Path) {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir())
        && let Err(error) = fs::remove_dir_all(path)
    {
        eprintln!("Could not remove {}: {error}", path.display());
    }
}

fn remove_file(path: &Path) {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
        && let Err(error) = fs::remove_file(path)
    {
        eprintln!("Could not remove {}: {error}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::startup::{Check, Launch};
    use std::cell::Cell;

    #[test]
    fn isolated_development_and_check_startups_never_resolve_or_remove_legacy_paths() {
        let owned = tempfile::tempdir().unwrap();
        let home = owned.path().join("home");
        let temporary = owned.path().join("temporary");
        fs::create_dir_all(&temporary).unwrap();
        let paths = [
            home.join("Library/WebKit/app.newport/sentinel"),
            home.join("Library/WebKit/newport/sentinel"),
            home.join("Library/Application Support/app.newport/.window-state.json"),
        ];
        for path in &paths {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"production fixture").unwrap();
        }
        let retired_temp = temporary.join("com.apple.WebKit.fixture+app.newport");
        fs::create_dir(&retired_temp).unwrap();
        let keep = temporary.join("unrelated");
        fs::create_dir(&keep).unwrap();
        let resolutions = Cell::new(0);
        for check in [None, Some(Check::Backend), Some(Check::UpdateHealth)] {
            for debug in [false, true] {
                for profile_override in [false, true] {
                    if check.is_none() && !debug && !profile_override {
                        continue;
                    }
                    remove_with(
                        &Launch {
                            check,
                            ..Default::default()
                        },
                        debug,
                        profile_override,
                        || {
                            resolutions.set(resolutions.get() + 1);
                            Some((home.clone(), temporary.clone()))
                        },
                    );
                    assert_eq!(
                        resolutions.get(),
                        0,
                        "ineligible startup resolved production paths"
                    );
                    for path in &paths {
                        assert_eq!(fs::read(path).unwrap(), b"production fixture");
                    }
                    assert!(retired_temp.is_dir());
                }
            }
        }
        remove_with(&Launch::default(), false, false, || {
            resolutions.set(resolutions.get() + 1);
            Some((home, temporary))
        });
        assert_eq!(resolutions.get(), 1);
        assert!(paths.iter().all(|path| !path.exists()));
        assert!(!retired_temp.exists());
        assert!(keep.is_dir());
    }
}
