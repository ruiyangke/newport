//! Exercise the installed startup blocks using real shells when available.
use std::{fs, os::unix::fs::PermissionsExt, process::Command};
const BIN: &str = env!("CARGO_BIN_EXE_porthop-agent");

#[test]
fn installed_blocks_are_repeatable_and_survive_agent_removal() {
    for shell in ["bash", "zsh", "fish"] {
        if Command::new(shell).arg("--version").output().is_err() {
            continue;
        }
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join(".local/bin");
        fs::create_dir_all(&bin).unwrap();
        let agent = bin.join("porthop-agent");
        fs::copy(BIN, &agent).unwrap();
        fs::set_permissions(&agent, fs::Permissions::from_mode(0o700)).unwrap();
        if shell == "bash" {
            fs::write(
                home.path().join(".bash_profile"),
                "# existing login settings\n",
            )
            .unwrap();
            fs::write(home.path().join(".profile"), "# inactive profile\n").unwrap();
        }
        let install = || {
            Command::new(&agent)
                .arg("install")
                .env("HOME", home.path())
                .env("SHELL", format!("/bin/{shell}"))
                .env_remove("ZDOTDIR")
                .env_remove("XDG_CONFIG_HOME")
                .output()
                .unwrap()
        };
        assert!(install().status.success());
        let rc = home.path().join(match shell {
            "bash" => ".bashrc",
            "zsh" => ".zshrc",
            _ => ".config/fish/conf.d/porthop.fish",
        });
        let original = fs::read(&rc).unwrap();
        if shell == "bash" {
            assert!(fs::read_to_string(home.path().join(".bash_profile"))
                .unwrap()
                .contains("# >>> Porthop >>>"));
            assert_eq!(
                fs::read_to_string(home.path().join(".profile")).unwrap(),
                "# inactive profile\n"
            );
        }
        assert!(install().status.success());
        assert_eq!(fs::read(&rc).unwrap(), original);
        let state = home.path().join(".cache/porthop/clipboard");
        fs::create_dir_all(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(state.join("features"), "clipboard browser").unwrap();
        fs::write(state.join("display"), ":99").unwrap();
        let script = if shell == "fish" {
            "source $RC; source $RC; printf '%s\\n' $DISPLAY; count (string match -- $HOME/.local/bin $PATH)"
        } else {
            ". \"$RC\"; . \"$RC\"; printf '%s\\n' \"$DISPLAY\" \"$PATH\""
        };
        let run = |script: &str, display: Option<&str>| {
            let mut cmd = Command::new(shell);
            if shell == "fish" {
                cmd.arg("--no-config");
            } else if shell == "zsh" {
                cmd.arg("-f");
            } else {
                cmd.args(["--noprofile", "--norc", "-eu"]);
            }
            cmd.args(["-c", script])
                .env("HOME", home.path())
                .env("RC", &rc)
                .env_remove("ZDOTDIR")
                .env_remove("XDG_CONFIG_HOME")
                .env_remove("DISPLAY")
                .env_remove("WAYLAND_DISPLAY")
                .env_remove("XAUTHORITY");
            if let Some(display) = display {
                cmd.env("DISPLAY", display);
            }
            cmd.output().unwrap()
        };
        let result = run(script, None);
        assert!(
            result.status.success(),
            "{shell}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let text = String::from_utf8(result.stdout).unwrap();
        assert!(text.starts_with(":99\n"), "{shell}: {text}");
        if shell == "fish" {
            assert_eq!(text, ":99\n1\n");
        } else {
            assert_eq!(text.matches(bin.to_str().unwrap()).count(), 1);
        }
        let result = run(script, Some(":0"));
        assert!(String::from_utf8_lossy(&result.stdout).starts_with(":0\n"));
        fs::remove_file(&agent).unwrap();
        let alive = if shell == "fish" {
            "source $RC; printf alive"
        } else {
            ". \"$RC\"; printf alive"
        };
        let result = run(alive, None);
        assert!(result.status.success());
        assert_eq!(result.stdout, b"alive");
        fs::write(&agent, "#!/bin/sh\necho 'exit 99'\nexit 1\n").unwrap();
        fs::set_permissions(&agent, fs::Permissions::from_mode(0o700)).unwrap();
        let result = run(alive, None);
        assert!(result.status.success(), "{shell}");
        assert_eq!(result.stdout, b"alive");
    }
}
