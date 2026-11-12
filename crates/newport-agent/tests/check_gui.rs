//! `env --check-gui`: an inherited display claim is current only while it
//! names the account's running stable display.
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

const BIN: &str = env!("CARGO_BIN_EXE_newport-agent");

/// The account's stable display in `runtime`, naming the endpoints `claim`
/// gives a shell, while the returned listener is bound.
fn stable_display(home: &Path, runtime: &Path) -> std::os::unix::net::UnixListener {
    let folder = runtime.join("newport-display");
    fs::create_dir_all(&folder).unwrap();
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(home.join("Xauthority"), "authority").unwrap();
    let running = serde_json::json!({
        "DISPLAY": ":75",
        "XAUTHORITY": home.join("Xauthority"),
        "WAYLAND_DISPLAY": home.join("wayland.sock"),
        "WAYLAND_SOCKET": null,
    });
    fs::write(folder.join("environment.json"), running.to_string()).unwrap();
    std::os::unix::net::UnixListener::bind(folder.join("display.sock")).unwrap()
}

/// `env --check-gui` in a shell whose environment claims the display's
/// endpoints, with its markers.
fn claim(home: &Path) -> Command {
    let mut command = Command::new(BIN);
    command
        .args(["env", "--check-gui", "--shell", "sh"])
        .env("HOME", home)
        .env("NEWPORT_TERMINAL", uuid::Uuid::new_v4().to_string())
        .env("PATH", "/usr/bin:/bin")
        .env_remove("WAYLAND_SOCKET")
        .env_remove("NEWPORT_GUI_RUNTIME_DIR")
        .env("XDG_RUNTIME_DIR", home.join("user-runtime"))
        .env("DISPLAY", ":75")
        .env("NEWPORT_GUI_DISPLAY", ":75")
        .env("XAUTHORITY", home.join("Xauthority"))
        .env("NEWPORT_GUI_AUTHORITY", home.join("Xauthority"))
        .env("WAYLAND_DISPLAY", home.join("wayland.sock"))
        .env("NEWPORT_GUI_WAYLAND_DISPLAY", home.join("wayland.sock"));
    command
}

#[test]
fn a_claim_is_current_only_on_the_running_stable_display() {
    let home = tempfile::tempdir_in("/tmp").unwrap();
    let _socket = std::os::unix::net::UnixListener::bind(home.path().join("wayland.sock")).unwrap();
    // Not running: refused, with guidance.
    let output = claim(home.path()).output().unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("current Newport terminal"),
        "{output:?}"
    );
    let display = stable_display(home.path(), &home.path().join("user-runtime"));
    let output = claim(home.path()).output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("/.local/bin"));
    // A shell's endpoints alone, as a shell on the display has them.
    let output = claim(home.path())
        .env_remove("NEWPORT_GUI_DISPLAY")
        .env_remove("NEWPORT_GUI_AUTHORITY")
        .env_remove("NEWPORT_GUI_WAYLAND_DISPLAY")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    drop(display);
    fs::remove_file(
        home.path()
            .join("user-runtime/newport-display/display.sock"),
    )
    .unwrap();
    let output = claim(home.path()).output().unwrap();
    assert!(!output.status.success(), "{output:?}");
}

#[test]
fn malformed_claims_are_refused() {
    let home = tempfile::tempdir_in("/tmp").unwrap();
    let _socket = std::os::unix::net::UnixListener::bind(home.path().join("wayland.sock")).unwrap();
    let _display = stable_display(home.path(), &home.path().join("user-runtime"));
    for (name, value) in [
        ("WAYLAND_SOCKET", Some("")),
        ("WAYLAND_SOCKET", Some("99")),
        ("WAYLAND_DISPLAY", Some("")),
        ("WAYLAND_DISPLAY", Some("/host/wayland")),
        ("WAYLAND_DISPLAY", None),
        ("NEWPORT_GUI_WAYLAND_DISPLAY", Some("")),
        ("NEWPORT_GUI_WAYLAND_DISPLAY", Some("/different/claim")),
        ("DISPLAY", Some(":0")),
        ("XAUTHORITY", None),
        ("XAUTHORITY", Some("/host/authority")),
        ("NEWPORT_DISPLAY", Some("relative")),
    ] {
        let mut command = claim(home.path());
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
        let output = command.output().unwrap();
        assert!(!output.status.success(), "{name}={value:?}: {output:?}");
        assert!(output.stdout.is_empty());
    }
}
