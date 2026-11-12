//! The shell-facing display environment, including incomplete and stale sessions.
#![cfg(target_os = "linux")]

use std::{
    fs,
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::Path,
    process::{Command, Output},
};

const BIN: &str = env!("CARGO_BIN_EXE_newport-agent");

fn private(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn exports(home: &Path, runtime: Option<&Path>) -> Output {
    let mut command = Command::new(BIN);
    command
        .args(["terminal", "display"])
        .current_dir(home)
        .env("HOME", home)
        .env_remove("XDG_RUNTIME_DIR");
    if let Some(runtime) = runtime {
        command.env("XDG_RUNTIME_DIR", runtime);
    }
    command.output().unwrap()
}

#[test]
fn a_missing_empty_or_relative_runtime_is_rejected_without_creating_a_display() {
    let home = tempfile::tempdir().unwrap();
    for runtime in [None, Some(Path::new("")), Some(Path::new("relative"))] {
        let output = exports(home.path(), runtime);
        assert!(!output.status.success(), "runtime={runtime:?}");
        assert!(output.stdout.is_empty());
        assert!(!home.path().join("relative").exists());
    }
}

#[test]
fn a_shell_preserves_literal_paths_with_spaces_quotes_and_substitutions() {
    let home = tempfile::tempdir().unwrap();
    let runtime = home.path().join("run '$(touch injected) 雪");
    private(&runtime);
    for shell in ["/bin/sh", "/bin/bash"] {
        let output = Command::new(shell)
            .args([
                "-c",
                "eval \"$(\"$AGENT\" terminal display)\" && printf '%s\\0' \"$WAYLAND_DISPLAY\" \"$XAUTHORITY\" \"$XDG_RUNTIME_DIR\"",
            ])
            .env("AGENT", BIN)
            .env("HOME", home.path())
            .env("XDG_RUNTIME_DIR", &runtime)
            .current_dir(home.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let expected = format!(
            "{}/newport-display/wayland-0\0{}/xauth_newport-display\0{}\0",
            runtime.display(),
            runtime.display(),
            runtime.display()
        );
        assert_eq!(output.stdout, expected.as_bytes());
        assert!(!home.path().join("injected").exists());
    }
}

#[test]
fn a_wayland_only_display_clears_stale_x11_and_attachment_variables() {
    let home = tempfile::tempdir().unwrap();
    let runtime = home.path().join("run");
    let display = runtime.join("newport-display");
    private(&display);
    let _listener = UnixListener::bind(display.join("display.sock")).unwrap();
    let environment = serde_json::json!({
        "WAYLAND_DISPLAY": "newport-display/wayland-0",
        "XDG_RUNTIME_DIR": runtime,
        "DISPLAY": null,
        "XAUTHORITY": null,
    });
    fs::write(display.join("environment.json"), environment.to_string()).unwrap();
    let mut command = Command::new("/bin/sh");
    command
        .args([
            "-c",
            "eval \"$(\"$AGENT\" terminal display)\" && printf '%s\\0' \"$WAYLAND_DISPLAY\" \"${DISPLAY-unset}\" \"${XAUTHORITY-unset}\" \"${WAYLAND_SOCKET-unset}\" \"${NEWPORT_DISPLAY-unset}\" \"${NEWPORT_GUI_WAYLAND_DISPLAY-unset}\" \"${DBUS_STARTER_ADDRESS-unset}\"",
        ])
        .env("AGENT", BIN)
        .env("HOME", home.path())
        .env("XDG_RUNTIME_DIR", &runtime);
    for name in [
        "DISPLAY",
        "XAUTHORITY",
        "WAYLAND_SOCKET",
        "NEWPORT_DISPLAY",
        "NEWPORT_GUI_WAYLAND_DISPLAY",
        "DBUS_STARTER_ADDRESS",
    ] {
        command.env(name, "stale");
    }
    let output = command.output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        output.stdout,
        format!(
            "{}/wayland-0\0unset\0unset\0unset\0unset\0unset\0unset\0",
            display.display()
        )
        .as_bytes()
    );
    assert!(!display.join("x11-display").exists());
}

#[test]
fn an_unsafe_display_folder_is_rejected_without_writing_through_it() {
    let home = tempfile::tempdir().unwrap();
    let runtime = home.path().join("run");
    private(&runtime);
    let display = runtime.join("newport-display");
    private(&display);
    fs::set_permissions(&display, fs::Permissions::from_mode(0o755)).unwrap();
    let output = exports(home.path(), Some(&runtime));
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!display.join("x11-display").exists());

    fs::remove_dir(&display).unwrap();
    let target = home.path().join("other");
    private(&target);
    std::os::unix::fs::symlink(&target, &display).unwrap();
    let output = exports(home.path(), Some(&runtime));
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
}

#[test]
fn terminal_exports_keep_the_same_socket_across_alias_replacement() {
    let home = tempfile::tempdir().unwrap();
    let runtime = home.path().join("run");
    private(&runtime);
    let output = exports(home.path(), Some(&runtime));
    assert!(output.status.success());
    let display = runtime.join("newport-display");
    let alias = display.join("wayland-0");
    for name in ["first", "second"] {
        let socket = runtime.join(name);
        let listener = UnixListener::bind(&socket).unwrap();
        let staged = display.join("next");
        std::os::unix::fs::symlink(&socket, &staged).unwrap();
        fs::rename(staged, &alias).unwrap();
        let again = exports(home.path(), Some(&runtime));
        assert!(again.status.success());
        assert_eq!(again.stdout, output.stdout);
        let connection = std::os::unix::net::UnixStream::connect(&alias).unwrap();
        let (_accepted, _) = listener.accept().unwrap();
        drop(connection);
    }
}
