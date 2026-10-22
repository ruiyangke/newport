//! The Tauri app that Newport replaced kept web view data outside the profile:
//! WebKit storage for its window, its window placement, and WebKit's
//! per-process temporary folders. Nothing reads them now; this removes them.
//! Only paths that are unambiguously the web view's go: the app's own
//! temporary folder stays, since the system may use it for this app too.
use std::{fs, path::Path};

pub(crate) fn remove() {
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
        return;
    };
    for storage in ["app.newport", "newport"] {
        remove_all(&home.join("Library/WebKit").join(storage));
    }
    let tauri = home.join("Library/Application Support/app.newport");
    remove_file(&tauri.join(".window-state.json"));
    // Its folder goes once nothing else is in it.
    let _ = fs::remove_dir(&tauri);
    let temporary = std::env::temp_dir();
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
