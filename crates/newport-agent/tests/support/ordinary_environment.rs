//! Evaluate the actual env command twice in each shell, preserving absence vs
//! empty values. These ownership fixtures grant no rendering or GUI lease.
use super::BIN;
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    process::Command,
};

struct Fixture {
    home: tempfile::TempDir,
    root: PathBuf,
    _lease: fs::File,
}

impl Fixture {
    fn new(features: &str) -> Self {
        let home = tempfile::Builder::new()
            .prefix("newport-env-'")
            .tempdir_in("/tmp")
            .unwrap();
        let root = home.path().join(".cache/newport/clipboard");
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        for (name, value) in [
            ("features", features),
            ("display", ":99"),
            ("Xauthority", "owned fixture cookie"),
        ] {
            fs::write(root.join(name), value).unwrap();
            fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o600)).unwrap();
        }
        let lease = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(root.join("agent.sock.lock"))
            .unwrap();
        fs2::FileExt::lock_exclusive(&lease).unwrap();
        Self {
            home,
            root,
            _lease: lease,
        }
    }

    fn run(&self, shell: &str, values: &[(&str, &str)]) -> BTreeMap<String, String> {
        let mut command = Command::new(shell);
        let script = if shell == "fish" {
            command.arg("--no-config");
            "set -l refresh ($AGENT env --shell fish); eval (string join \\n -- $refresh | string collect); set refresh ($AGENT env --shell fish); eval (string join \\n -- $refresh | string collect); /usr/bin/env -0"
        } else {
            if shell == "bash" {
                command.args(["--noprofile", "--norc"]);
            }
            command.arg("-eu");
            "eval \"$(\"$AGENT\" env --shell sh)\"; eval \"$(\"$AGENT\" env --shell sh)\"; /usr/bin/env -0"
        };
        command
            .args(["-c", script])
            .env_clear()
            .env("HOME", self.home.path())
            .env("PATH", "/usr/bin:/bin")
            .env("AGENT", BIN)
            .env("XDG_RUNTIME_DIR", self.home.path())
            .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/host/session-bus");
        for (name, value) in values {
            command.env(name, value);
        }
        let output = command.output().unwrap();
        assert!(output.status.success(), "shell={shell}: {output:?}");
        String::from_utf8(output.stdout)
            .unwrap()
            .split('\0')
            .filter_map(|entry| entry.split_once('='))
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect()
    }

    fn owned(&self) -> (String, String) {
        (
            self.root.join("Xauthority").display().to_string(),
            self.root.join("wayland.sock").display().to_string(),
        )
    }
}

fn check(shell: &str) {
    for features in ["clipboard browser", "gui browser", "browser", ""] {
        let fixture = Fixture::new(features);
        for initial in [
            vec![],
            vec![("DISPLAY", ""), ("XAUTHORITY", ""), ("WAYLAND_DISPLAY", "")],
        ] {
            let result = fixture.run(shell, &initial);
            for name in ["DISPLAY", "XAUTHORITY", "WAYLAND_DISPLAY"] {
                assert_eq!(
                    result.get(name).map(String::as_str),
                    if initial.is_empty() { None } else { Some("") },
                    "{shell} {features} {name}"
                );
            }
            assert_eq!(result["PATH"].matches("/.local/bin").count(), 1);
        }
        let (authority, wayland) = fixture.owned();
        let result = fixture.run(
            shell,
            &[
                ("DISPLAY", ":99"),
                ("XAUTHORITY", &authority),
                ("WAYLAND_DISPLAY", &wayland),
                ("WAYLAND_SOCKET", "42"),
            ],
        );
        for name in ["DISPLAY", "XAUTHORITY", "WAYLAND_DISPLAY"] {
            assert!(
                !result.contains_key(name),
                "{shell} {features} {name}: {result:?}"
            );
        }
        assert_eq!(result["WAYLAND_SOCKET"], "42");
        assert_eq!(
            result["XDG_RUNTIME_DIR"],
            fixture.home.path().to_str().unwrap()
        );
        assert_eq!(
            result["DBUS_SESSION_BUS_ADDRESS"],
            "unix:path=/host/session-bus"
        );

        // A matching display number does not make the host's cookie ours.
        let host = [
            ("DISPLAY", ":99"),
            ("XAUTHORITY", "/host/.Xauthority"),
            ("WAYLAND_DISPLAY", "wayland-0"),
            ("WAYLAND_SOCKET", ""),
        ];
        let result = fixture.run(shell, &host);
        for (name, value) in host {
            assert_eq!(result[name], value, "{shell} {name}");
        }

        // Existing GUI-marker behavior stays intact; these markers are not
        // evidence that a tmux session belongs to the current generation.
        let gui = [
            ("DISPLAY", ":99"),
            ("XAUTHORITY", authority.as_str()),
            ("NEWPORT_GUI_DISPLAY", ":99"),
            ("NEWPORT_GUI_AUTHORITY", authority.as_str()),
            ("WAYLAND_DISPLAY", wayland.as_str()),
            ("NEWPORT_GUI_WAYLAND_DISPLAY", wayland.as_str()),
        ];
        let result = fixture.run(shell, &gui);
        for (name, value) in gui {
            assert_eq!(result[name], value, "{shell} {name}");
        }

        let uncertain = [
            ("DISPLAY", ":99"),
            ("XAUTHORITY", "/tmp/newport-runtime-unrecognized/Xauthority"),
            (
                "WAYLAND_DISPLAY",
                "/tmp/newport-runtime-unrecognized/wayland.sock",
            ),
        ];
        let result = fixture.run(shell, &uncertain);
        for (name, value) in uncertain {
            assert_eq!(result[name], value, "{shell} {name}");
        }

        // Group-readable or substituted records do not prove ownership.
        let pair = [("DISPLAY", ":99"), ("XAUTHORITY", authority.as_str())];
        let record = fixture.root.join("display");
        fs::set_permissions(&record, fs::Permissions::from_mode(0o640)).unwrap();
        for (name, value) in pair {
            assert_eq!(
                fixture.run(shell, &pair)[name],
                value,
                "unsafe proof {shell} {name}"
            );
        }
        fs::remove_file(&record).unwrap();
        let substituted = fixture.home.path().join("other-display");
        fs::write(&substituted, ":99").unwrap();
        fs::set_permissions(&substituted, fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&substituted, &record).unwrap();
        for (name, value) in pair {
            assert_eq!(
                fixture.run(shell, &pair)[name],
                value,
                "symlink proof {shell} {name}"
            );
        }

        fs::remove_file(fixture.root.join("display")).unwrap();
        let pair = [("DISPLAY", ":99"), ("XAUTHORITY", authority.as_str())];
        let result = fixture.run(shell, &pair);
        for (name, value) in pair {
            assert_eq!(result[name], value, "missing proof {shell} {name}");
        }
    }
}

#[test]
fn ordinary_shell_projection_preserves_host_displays_and_clears_only_owned_clipboard_pairs() {
    for shell in ["sh", "bash"] {
        check(shell);
    }
}

#[test]
#[ignore = "Requires fish; explicitly run in the local shared-shell Docker fixture"]
fn fish_ordinary_shell_projection_matches_sh_and_bash() {
    check("fish");
}
