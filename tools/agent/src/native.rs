// Native desktop owners are optional; all formats remain available through the agent.
use std::{
    env, fs,
    io::{Seek, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
/// Whether this shell names an owned Newport GUI display. The paired markers
/// select helper behavior; session selection and IPC still enforce permission.
pub(crate) fn gui_display() -> bool {
    gui_x11_display() || gui_wayland_display()
}
fn gui_x11_display() -> bool {
    let display = env::var("DISPLAY").unwrap_or_default();
    let authority = env::var("XAUTHORITY").unwrap_or_default();
    !display.is_empty()
        && !authority.is_empty()
        && env::var("NEWPORT_GUI_DISPLAY").ok().as_deref() == Some(display.as_str())
        && env::var("NEWPORT_GUI_AUTHORITY").ok().as_deref() == Some(authority.as_str())
}
fn gui_wayland_display() -> bool {
    let display = env::var("WAYLAND_DISPLAY").unwrap_or_default();
    let runtime = env::var("XDG_RUNTIME_DIR").unwrap_or_default();
    !display.is_empty()
        && env::var_os("WAYLAND_SOCKET").is_none()
        && env::var("NEWPORT_GUI_WAYLAND_DISPLAY").ok().as_deref() == Some(display.as_str())
        // An absolute socket is independent of the user's runtime directory.
        // Relative names still require the matching directory marker. Neither
        // form overrides an inherited Wayland connection fd.
        && (display.starts_with('/')
            || (!runtime.is_empty()
                && env::var("NEWPORT_GUI_RUNTIME_DIR").ok().as_deref()
                    == Some(runtime.as_str())))
}

/// Clipboard protocol endpoints are not rendering displays. These proofs are
/// deliberately narrower than a recognizable path prefix: an old shell whose
/// ownership records disappeared cannot authorize clearing another display.
pub(crate) struct ClipboardEnvironment {
    pub x11: Option<(String, String)>,
    pub wayland: Option<String>,
}

fn owned_record(path: &Path) -> Option<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
        || metadata.len() > 4096
    {
        return None;
    }
    fs::read(path).ok()
}

fn clipboard_root(path: &Path) -> Option<PathBuf> {
    let root = path.canonicalize().ok()?;
    let home = PathBuf::from(env::var_os("HOME")?);
    let standalone = crate::paths::standalone_root(&home);
    let standalone = fs::symlink_metadata(&standalone)
        .ok()
        .filter(|metadata| metadata.is_dir())
        .and_then(|_| standalone.canonicalize().ok());
    if standalone.as_ref() != Some(&root)
        && !crate::paths::integration_display_path(&root.join("wayland.sock").to_string_lossy())
    {
        return None;
    }
    let metadata = fs::symlink_metadata(&root).ok()?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return None;
    }
    owned_record(&root.join("agent.sock.lock"))?;
    owned_record(&root.join("features"))?;
    Some(root)
}

fn clipboard_x11(display: &str, authority: &str) -> bool {
    if display.is_empty() || authority.is_empty() {
        return false;
    }
    let authority = Path::new(authority);
    if authority
        .file_name()
        .is_none_or(|name| name != "Xauthority")
    {
        return false;
    }
    let Some(root) = authority.parent().and_then(clipboard_root) else {
        return false;
    };
    owned_record(&root.join("Xauthority")).is_some()
        && owned_record(&root.join("display"))
            .and_then(|value| String::from_utf8(value).ok())
            .is_some_and(|value| value.trim() == display)
}

pub(crate) fn clipboard_environment() -> ClipboardEnvironment {
    let display = env::var("DISPLAY").unwrap_or_default();
    let authority = env::var("XAUTHORITY").unwrap_or_default();
    let wayland = env::var("WAYLAND_DISPLAY").unwrap_or_default();
    let owned_wayland = Path::new(&wayland)
        .file_name()
        .is_some_and(|name| name == "wayland.sock")
        && Path::new(&wayland)
            .parent()
            .and_then(clipboard_root)
            .is_some();
    ClipboardEnvironment {
        x11: (!gui_x11_display() && clipboard_x11(&display, &authority))
            .then_some((display, authority)),
        wayland: (!gui_wayland_display() && owned_wayland).then_some(wayland),
    }
}

pub(crate) fn real_desktop(selection: Option<&std::path::Path>) -> bool {
    let wayland = env::var("WAYLAND_DISPLAY").unwrap_or_default();
    let authority = env::var("XAUTHORITY").unwrap_or_default();
    let fake_wayland = gui_wayland_display()
        || selection.is_some_and(|root| wayland == root.join("wayland.sock").to_string_lossy())
        || wayland.starts_with("/tmp/newport-runtime-")
        || crate::paths::integration_display_path(&wayland);
    let fake_x = gui_x11_display()
        || authority.starts_with("/tmp/newport-runtime-")
        || crate::x_endpoint::is_authority_path(&authority)
        || crate::paths::integration_display_path(&authority)
        || clipboard_x11(&env::var("DISPLAY").unwrap_or_default(), &authority);
    (env::var("DISPLAY").is_ok_and(|v| !v.is_empty()) && !fake_x)
        || (!wayland.is_empty() && !fake_wayland)
        || env::var_os("WAYLAND_SOCKET").is_some()
}

pub fn find(name: &str) -> Option<PathBuf> {
    let ours = env::current_exe().ok()?.canonicalize().ok()?;
    env::split_paths(&env::var_os("PATH").unwrap_or_default())
        .map(|p| p.join(name))
        .find(|p| {
            p.is_file()
                && p.metadata()
                    .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
                && p.canonicalize().is_ok_and(|p| p != ours)
        })
}
fn publish(mut command: Command, data: Vec<u8>) -> bool {
    let Ok(root) = crate::paths::runtime() else {
        return false;
    };
    let Ok(mut input) = tempfile::tempfile_in(root) else {
        return false;
    };
    if input.write_all(&data).is_err() || input.rewind().is_err() {
        return false;
    }
    let Ok(mut child) = command
        .stdin(Stdio::from(input))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Err(_) => break false,
            _ if Instant::now() >= deadline => break false,
            _ => thread::sleep(Duration::from_millis(10)),
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    success
}
pub fn update(path: &std::path::Path) -> &'static str {
    if gui_display() || env::var("NEWPORT_CLIPBOARD_NATIVE").as_deref() == Ok("0") {
        return "Clipboard synced.";
    }
    let Ok(formats) = crate::snapshot::read(path) else {
        return "Clipboard synced.";
    };
    let target = ["text/plain", "image/png", "text/html", "text/uri-list"]
        .into_iter()
        .find(|t| formats.contains_key(*t))
        .unwrap_or("text/plain");
    let data = formats.get(target).cloned().unwrap_or_default();
    let wayland = env::var("WAYLAND_DISPLAY").unwrap_or_default();
    let root = path.parent().unwrap();
    let fake_wayland = wayland == root.join("wayland.sock").to_string_lossy()
        || crate::paths::integration_display_path(&wayland);
    let fake_x = clipboard_x11(
        &env::var("DISPLAY").unwrap_or_default(),
        &env::var("XAUTHORITY").unwrap_or_default(),
    );
    if !fake_wayland
        && (env::var_os("WAYLAND_DISPLAY").is_some()
            || env::var_os("WAYLAND_SOCKET").is_some()
            || env::var_os("XDG_RUNTIME_DIR").is_some())
    {
        if let Some(tool) = find("wl-copy") {
            let mut command = Command::new(tool);
            command.args(["--type", target]);
            if publish(command, data.clone()) {
                return "Wayland clipboard updated.";
            }
        }
    }
    if let Some(tool) = find("xclip").filter(|_| !fake_x) {
        let mut command = Command::new(tool);
        command
            .env(
                "DISPLAY",
                env::var("DISPLAY").unwrap_or_else(|_| ":0".into()),
            )
            .args([
                "-selection",
                "clipboard",
                "-in",
                "-silent",
                "-target",
                if target == "text/plain" {
                    "UTF8_STRING"
                } else {
                    target
                },
            ]);
        if publish(command, data) {
            return "X clipboard updated.";
        }
    }
    "Clipboard synced."
}
