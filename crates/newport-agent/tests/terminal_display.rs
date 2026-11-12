//! `newport-agent display --terminal`: a plain Newport terminal's shell runs
//! with the terminal's clipboard display, and stays the terminal's process.
use std::{
    os::unix::process::ExitStatusExt,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const BIN: &str = env!("CARGO_BIN_EXE_newport-agent");

fn run(home: &Path, script: &str) -> (u32, std::process::Output) {
    let child = Command::new(BIN)
        .args(["display", "--terminal", &uuid::Uuid::new_v4().to_string()])
        .args(["--", "sh", "-c", script])
        .env("HOME", home)
        .env("XDG_RUNTIME_DIR", home)
        .env("WAYLAND_SOCKET", "9")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    (child.id(), child.wait_with_output().unwrap())
}

#[test]
fn the_shell_keeps_the_terminals_process_and_names_both_displays_while_it_runs() {
    let home = tempfile::Builder::new()
        .prefix("np-display-")
        .tempdir_in("/tmp")
        .unwrap();
    std::fs::set_permissions(
        home.path(),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    let (pid, output) = run(
        home.path(),
        "test -S \"$WAYLAND_DISPLAY\" && test -f \"$XAUTHORITY\" || exit 2; \
         printf '%s\\n' \"$$\" \"$DISPLAY\" \"$XAUTHORITY\" \"$WAYLAND_DISPLAY\" \"${WAYLAND_SOCKET-unset}\"; \
         kill -TERM $$",
    );
    // A signal ends the shell as it would without the display.
    assert_eq!(output.status.signal(), Some(libc::SIGTERM), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(
        lines[0],
        pid.to_string(),
        "the shell is the terminal's process"
    );
    assert!(lines[1].starts_with(':'), "{lines:?}");
    assert_eq!(lines[4], "unset");
    // The display ends with the shell.
    let started = Instant::now();
    while Path::new(lines[3]).exists() || Path::new(lines[2]).parent().unwrap().exists() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the display outlived the shell"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let (_, output) = run(home.path(), "exit 7");
    assert_eq!(output.status.code(), Some(7), "{output:?}");
}

/// The display isn't the shell's child: shells that reap only the jobs they
/// started would leave it a zombie once it ends.
#[cfg(target_os = "linux")]
#[test]
fn the_shell_has_no_child_it_did_not_start() {
    let home = tempfile::Builder::new()
        .prefix("np-display-")
        .tempdir_in("/tmp")
        .unwrap();
    std::fs::set_permissions(
        home.path(),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    // `read` is built in, so the shell starts no child to look.
    let (_, output) = run(
        home.path(),
        "test -S \"$WAYLAND_DISPLAY\" || exit 2; \
         read -r children < /proc/$$/task/$$/children; printf 'children=%s.\\n' \"$children\"",
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "children=.\n");
}
