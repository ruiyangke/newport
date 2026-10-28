//! Actual Fish evaluates only a successful explicit attachment check. Backend
//! replies come from the existing real-agent fixture; no GUI is acquired here.
use super::*;
use std::collections::BTreeMap;

const ARGS: [&str; 3] = ["literal $(touch INJECTED)", "", "space argument"];
const SCRIPT: &str = r#"
function record_state
    printf '%s\0' "$PWD" (count $argv) $argv
    for name in PATH NEWPORT_SESSION NEWPORT_TERMINAL NEWPORT_TERMINAL_EPOCH DISPLAY XAUTHORITY NEWPORT_GUI_DISPLAY NEWPORT_GUI_AUTHORITY WAYLAND_DISPLAY WAYLAND_SOCKET NEWPORT_GUI_WAYLAND_DISPLAY XDG_RUNTIME_DIR NEWPORT_GUI_RUNTIME_DIR DBUS_SESSION_BUS_ADDRESS DBUS_STARTER_ADDRESS GDK_BACKEND
        printf '%s\0' "$name"
        if set -q $name
            printf '%s\0' set (count $$name) $$name
        else
            printf '%s\0' unset
        end
    end
end
record_state $argv > "$HOME/state-before"
if test "$A12_FLOW" != gui
    "$AGENT" env --check-gui --shell fish > "$HOME/check.fish" 2> "$HOME/check.err"
    set -l checked_status $status
    printf '%s\n' $checked_status > "$HOME/check.status"
    if test $checked_status -eq 0
        source "$HOME/check.fish"
        set -l refresh_status $status
        printf '%s\n' $refresh_status > "$HOME/source.status"
        if test $refresh_status -ne 0
            exit $refresh_status
        end
        printf 'applied\n' > "$HOME/applied"
    end
    record_state $argv > "$HOME/state-after-check"
    if test $checked_status -ne 0
        exit $checked_status
    end
    if test "$A12_FLOW" = check
        exit 0
    end
end
"$AGENT" gui --program "$PROGRAM" -- $argv > "$HOME/launch.out" 2> "$HOME/launch.err"
set -l launch_status $status
printf '%s\n' $launch_status > "$HOME/launch.status"
record_state $argv > "$HOME/state-after-gui"
exit $launch_status
"#;

fn command(f: &Fixture, native: bool, flow: &str) -> Command {
    let executable = [
        "/usr/bin/fish",
        "/opt/homebrew/bin/fish",
        "/usr/local/bin/fish",
    ]
    .into_iter()
    .find(|path| Path::new(path).is_file())
    .expect("Actual Fish is required for this selected attachment transaction test");
    let mut command = Command::new(executable);
    command.args(["--no-config", "-c", SCRIPT]).args(ARGS);
    f.environment(&mut command, native);
    command
        .env("AGENT", BIN)
        .env("PROGRAM", &f.program)
        .env("A12_FLOW", flow)
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/fixture-explicit-bus",
        )
        .env("DBUS_STARTER_ADDRESS", "unix:path=/fixture-starter")
        .env(
            "PATH",
            format!(
                "{}:{}:/usr/bin:/bin",
                f.home.path().join(".cache/newport/gui-bin").display(),
                f.home.path().join("other-terminal-gui-bin").display()
            ),
        );
    command
}

fn fields(bytes: &[u8]) -> Vec<String> {
    assert_eq!(
        bytes.last(),
        Some(&0),
        "Incomplete NUL-delimited fixture state"
    );
    bytes[..bytes.len() - 1]
        .split(|b| *b == 0)
        .map(|b| String::from_utf8(b.to_vec()).unwrap())
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
struct State {
    cwd: String,
    argv: Vec<String>,
    environment: BTreeMap<String, Option<Vec<String>>>,
}
fn state(f: &Fixture, name: &str) -> State {
    let values = fields(&fs::read(f.home.path().join(name)).unwrap());
    let mut values = values.into_iter();
    let cwd = values.next().unwrap();
    let count = values.next().unwrap().parse::<usize>().unwrap();
    let argv: Vec<_> = values.by_ref().take(count).collect();
    assert_eq!(argv.len(), count);
    let mut environment = BTreeMap::new();
    while let Some(name) = values.next() {
        let value = match values.next().unwrap().as_str() {
            "unset" => None,
            "set" => {
                let count = values.next().unwrap().parse::<usize>().unwrap();
                let value: Vec<_> = values.by_ref().take(count).collect();
                assert_eq!(value.len(), count);
                Some(value)
            }
            other => panic!("Invalid fixture presence tag: {other}"),
        };
        assert!(environment.insert(name, value).is_none());
    }
    assert_eq!(environment.len(), 16);
    State {
        cwd,
        argv,
        environment,
    }
}

fn retained(f: &Fixture) {
    assert!(f.home.path().join("wayland.sock").exists());
    assert_eq!(
        fs::read(f.home.path().join("Xauthority")).unwrap(),
        b"authority"
    );
    assert!(!f.home.path().join("working directory/INJECTED").exists());
}

#[test]
#[ignore = "Requires actual Fish; selected local Linux lane must provide it"]
fn current_mixed_and_native_checks_refresh_then_revalidate_each_execution_once() {
    for native in [false, true] {
        let mut f = Fixture::new(true);
        let child = command(&f, native, "check-then-gui").spawn().unwrap();
        let first = f.request(native);
        assert!(
            fs::read(f.home.path().join("check.fish"))
                .unwrap()
                .is_empty()
        );
        assert!(!f.home.path().join("applied").exists());
        assert!(!f.home.path().join("executed").exists());
        f.reply(&first, check::Outcome::Current {});

        // Current is point-in-time: the later actual GUI helper must issue its
        // own check even though this Fish process just sourced a valid refresh.
        let second = f.request(native);
        assert_ne!(first.id, second.id);
        assert!(f.home.path().join("applied").exists());
        assert!(!f.home.path().join("executed").exists());
        let before = state(&f, "state-before");
        let after = state(&f, "state-after-check");
        assert_eq!(before.argv, ARGS);
        assert_eq!(after.argv, ARGS);
        assert_eq!(before.cwd, after.cwd);
        let mut expected = before.environment.clone();
        expected.insert(
            "PATH".into(),
            Some(vec![
                f.home.path().join(".local/bin").to_str().unwrap().into(),
                f.home
                    .path()
                    .join("other-terminal-gui-bin")
                    .to_str()
                    .unwrap()
                    .into(),
                "/usr/bin".into(),
                "/bin".into(),
            ]),
        );
        assert_eq!(after.environment, expected);
        f.reply(&second, check::Outcome::Current {});
        let output = finish(child);
        assert_eq!(output.status.code(), Some(17), "{output:?}");
        assert!(output.stdout.is_empty() && output.stderr.is_empty());
        assert_eq!(state(&f, "state-after-gui"), after);
        assert_eq!(
            fs::read(f.home.path().join("executed")).unwrap(),
            b"executed\n"
        );
        let result = fields(&fs::read(f.home.path().join("launch.out")).unwrap());
        assert_eq!(&result[4..], ARGS);
        assert_eq!(result[0], before.cwd);
        assert_eq!(result[1], if native { "" } else { ":75" });
        assert_eq!(
            result[2],
            if native {
                String::new()
            } else {
                f.home.path().join("Xauthority").to_str().unwrap().into()
            }
        );
        assert_eq!(
            result[3],
            f.home.path().join("wayland.sock").to_str().unwrap()
        );
        assert_eq!(
            fs::read(f.home.path().join("source.status")).unwrap(),
            b"0\n"
        );
        retained(&f);
        f.idle();
    }
}

#[test]
#[ignore = "Requires actual Fish; selected local Linux lane must provide it"]
fn stale_and_permission_refusals_preserve_shell_and_endpoints_without_replay() {
    for native in [false, true] {
        for flow in ["check", "gui"] {
            for outcome in [
                check::Outcome::NotCurrent {
                    reason: check::Reason::StaleShell,
                },
                check::Outcome::Error {
                    message: "permission revoked".into(),
                },
            ] {
                let mut f = Fixture::new(true);
                let child = command(&f, native, flow).spawn().unwrap();
                let request = f.request(native);
                assert!(!f.home.path().join("executed").exists());
                f.reply(&request, outcome);
                refused(&mut f, child, flow);
            }
        }
    }
}

fn refused(f: &mut Fixture, child: Child, flow: &str) {
    let output = finish(child);
    assert!(!output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
    let (after, stdout, stderr) = if flow == "check" {
        ("state-after-check", "check.fish", "check.err")
    } else {
        ("state-after-gui", "launch.out", "launch.err")
    };
    assert_eq!(state(f, "state-before"), state(f, after));
    assert!(fs::read(f.home.path().join(stdout)).unwrap().is_empty());
    assert!(
        String::from_utf8(fs::read(f.home.path().join(stderr)).unwrap())
            .unwrap()
            .contains("current Newport terminal")
    );
    assert!(!f.home.path().join("applied").exists());
    assert!(!f.home.path().join("source.status").exists());
    assert!(!f.home.path().join("executed").exists());
    retained(f);
    f.idle(); // Any a/l/L replay or acquisition is an unexpected event.
}

#[test]
#[ignore = "Requires actual Fish; selected local Linux lane must provide it"]
fn malformed_inherited_claims_never_request_or_apply_a_refresh() {
    for native in [false, true] {
        for flow in ["check", "gui"] {
            for (name, value) in [
                ("WAYLAND_SOCKET", ""),
                ("WAYLAND_DISPLAY", "/foreign/wayland"),
                ("NEWPORT_TERMINAL_EPOCH", ""),
            ] {
                let mut f = Fixture::new(true);
                let child = command(&f, native, flow).env(name, value).spawn().unwrap();
                refused(&mut f, child, flow);
            }
        }
    }
}
