use std::{path::Path, process::Command};
#[path = "support/x11.rs"]
mod x11;

const GENERIC: &str = env!("CARGO_BIN_EXE_newport-agent");

#[test]
fn generic_target_selects_both_backends_and_defaults_to_x11() {
    for options in [
        vec![],
        vec!["--backend", "x11"],
        vec!["--backend", "wayland"],
    ] {
        let wayland = options.contains(&"wayland");
        let script = if wayland {
            "test -S \"$WAYLAND_DISPLAY\" && test -z \"${DISPLAY-}${XAUTHORITY-}${WAYLAND_SOCKET-}\" || exit 2; printf '%s\\n' \"$WAYLAND_DISPLAY\"; exit 7"
        } else {
            "test -S /tmp/.X11-unix/X${DISPLAY#:} && test -f \"$XAUTHORITY\" && test -z \"${WAYLAND_DISPLAY-}${WAYLAND_SOCKET-}\" || exit 2; ln /tmp/.X11-unix/X${DISPLAY#:} \"$NEWPORT_TEST_SOCKET_WITNESS\" || exit 3; printf '/tmp/.X11-unix/X%s\\n%s\\n' \"${DISPLAY#:}\" \"$XAUTHORITY\"; exit 7"
        };
        let home = tempfile::tempdir().unwrap();
        let witness = x11::socket_witness();
        let original = witness.path().join("socket");
        let result = Command::new(GENERIC)
            .env("HOME", home.path())
            .env("NEWPORT_TEST_SOCKET_WITNESS", &original)
            .arg("display")
            .args(options)
            .args(["--", "sh", "-c", script])
            .env("DISPLAY", ":99")
            .env("WAYLAND_DISPLAY", "existing")
            .env("WAYLAND_SOCKET", "17")
            .output()
            .unwrap();
        assert_eq!(
            result.status.code(),
            Some(7),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let output = String::from_utf8(result.stdout).unwrap();
        let paths: Vec<_> = output.lines().collect();
        assert_eq!(paths.len(), if wayland { 1 } else { 2 });
        if wayland {
            assert!(!Path::new(paths[0]).exists());
        } else {
            x11::assert_socket_released(Path::new(paths[0]), &original);
            assert!(!Path::new(paths[1]).parent().unwrap().exists());
        }
    }
}

#[test]
fn rejects_invalid_or_conflicting_backend_options() {
    for args in [
        vec!["--backend"],
        vec!["--backend", "unknown"],
        vec!["--backend", "wayland", "--display", "100"],
        vec!["--display", "100", "--backend", "wayland"],
        vec!["--service"],
        vec!["--backend", "wayland", "--service", "--", "true"],
        vec!["--backend", "wayland", "--socket", "/tmp/unused.sock"],
        vec![
            "--backend",
            "wayland",
            "--service",
            "--socket",
            "relative.sock",
        ],
    ] {
        let home = tempfile::tempdir().unwrap();
        assert!(!Command::new(GENERIC)
            .env("HOME", home.path())
            .arg("display")
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    }
}
