// Native desktop owners are optional; all formats remain available through the agent.
use std::{
    env, fs,
    io::{Seek, Write},
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
/// Whether this shell is on a desktop of the server's own, not one Newport
/// provides: a display from a Newport session, this one or an earlier one,
/// doesn't count. Newport's commands defer to the server's own there.
pub(crate) fn real_desktop(selection: Option<&std::path::Path>) -> bool {
    let wayland = env::var("WAYLAND_DISPLAY").unwrap_or_default();
    let authority = env::var("XAUTHORITY").unwrap_or_default();
    let fake_wayland = selection
        .is_some_and(|root| wayland == root.join("wayland.sock").to_string_lossy())
        || wayland.starts_with("/tmp/newport-runtime-")
        || crate::paths::integration_display_path(&wayland);
    let fake_x = authority.starts_with("/tmp/newport-runtime-")
        || crate::x_endpoint::is_authority_path(&authority)
        || crate::paths::integration_display_path(&authority)
        || selection.is_some_and(|root| {
            fs::read_to_string(root.join("display"))
                .ok()
                .is_some_and(|v| env::var("DISPLAY").ok().as_deref() == Some(v.trim()))
        });
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
    if env::var("NEWPORT_CLIPBOARD_NATIVE").as_deref() == Ok("0") {
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
    let fake_x = fs::read_to_string(root.join("display"))
        .ok()
        .is_some_and(|v| env::var("DISPLAY").ok().as_deref() == Some(v.trim()));
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
