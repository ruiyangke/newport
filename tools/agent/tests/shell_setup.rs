//! Exercise the installed startup blocks using real shells when available.
use std::{
    fs,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    process::Command,
};
const BIN: &str = env!("CARGO_BIN_EXE_newport-agent");
#[path = "support/ordinary_environment.rs"]
mod ordinary_environment;

/// Tests run binaries they've just copied. A parallel test's fork can briefly
/// hold the copy's write descriptor, and Linux then refuses to execute it
/// (ETXTBSY) until that child execs. Retry instead of failing the test.
trait RunOutput {
    fn run_output(&mut self) -> std::process::Output;
}
impl RunOutput for Command {
    fn run_output(&mut self) -> std::process::Output {
        for _ in 0..100 {
            match self.output() {
                Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                result => return result.unwrap(),
            }
        }
        self.output().unwrap()
    }
}

/// Stage this build and point `~/.local/bin/newport-agent` at it, as the desktop does.
fn install_managed(home: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    let hash: String = Sha256::digest(fs::read(BIN).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    for args in [
        ["deployment", "prepare", &hash].as_slice(),
        ["deployment", "install", &hash].as_slice(),
    ] {
        let output = Command::new(BIN).args(args).env("HOME", home).run_output();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    hash
}

#[test]
fn a_posix_login_shell_gets_the_block_in_its_profile() {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join(".local/bin");
    fs::create_dir_all(&bin).unwrap();
    let agent = bin.join("newport-agent");
    fs::copy(BIN, &agent).unwrap();
    fs::set_permissions(&agent, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(home.path().join(".profile"), "# existing login settings\n").unwrap();
    let output = Command::new(&agent)
        .arg("install")
        .env("HOME", home.path())
        .env("SHELL", "/bin/ash")
        .run_output();
    assert!(output.status.success(), "{output:?}");
    let profile = fs::read_to_string(home.path().join(".profile")).unwrap();
    assert!(
        profile.starts_with("# existing login settings\n"),
        "{profile}"
    );
    assert!(profile.contains("# >>> Newport >>>"), "{profile}");
    assert!(!home.path().join(".bashrc").exists());
    // A login shell reading it gets Newport's commands on PATH.
    let output = Command::new("sh")
        .args(["-c", ". \"$HOME/.profile\"; printf '%s' \"$PATH\""])
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .run_output();
    assert!(
        String::from_utf8_lossy(&output.stdout).starts_with(&format!("{}:", bin.display())),
        "{output:?}"
    );
}

#[test]
fn installed_blocks_are_repeatable_and_survive_agent_removal() {
    for shell in ["bash", "zsh", "fish"] {
        if Command::new(shell).arg("--version").output().is_err() {
            continue;
        }
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join(".local/bin");
        fs::create_dir_all(&bin).unwrap();
        let agent = bin.join("newport-agent");
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
                .run_output()
        };
        assert!(install().status.success());
        let rc = home.path().join(match shell {
            "bash" => ".bashrc",
            "zsh" => ".zshrc",
            _ => ".config/fish/conf.d/newport.fish",
        });
        let original = fs::read(&rc).unwrap();
        if shell == "bash" {
            assert!(
                fs::read_to_string(home.path().join(".bash_profile"))
                    .unwrap()
                    .contains("# >>> Newport >>>")
            );
            assert_eq!(
                fs::read_to_string(home.path().join(".profile")).unwrap(),
                "# inactive profile\n"
            );
        }
        assert!(install().status.success());
        assert_eq!(fs::read(&rc).unwrap(), original);
        let state = home.path().join(".cache/newport/clipboard");
        fs::create_dir_all(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(state.join("features"), "clipboard browser").unwrap();
        fs::write(state.join("display"), ":99").unwrap();
        // This shell-only fixture models the live bridge's ownership lease;
        // metadata left by a dead bridge must not redirect a new shell.
        let lease = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(state.join("agent.sock.lock"))
            .unwrap();
        fs2::FileExt::lock_exclusive(&lease).unwrap();
        let script = if shell == "fish" {
            "source $RC; source $RC; set -q DISPLAY; or set DISPLAY unset; printf '%s\\n' \"$DISPLAY\"; count (string match -- $HOME/.local/bin $PATH)"
        } else {
            ". \"$RC\"; . \"$RC\"; printf '%s\\n' \"${DISPLAY-unset}\" \"$PATH\""
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
            cmd.env_remove("NEWPORT_SESSION");
            if let Some(display) = display {
                cmd.env("DISPLAY", display);
            }
            cmd.run_output()
        };
        let result = run(script, None);
        assert!(
            result.status.success(),
            "{shell}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let text = String::from_utf8(result.stdout).unwrap();
        assert!(text.starts_with("unset\n"), "{shell}: {text}");
        if shell == "fish" {
            assert_eq!(text, "unset\n1\n");
        } else {
            assert_eq!(text.matches(bin.to_str().unwrap()).count(), 1);
        }
        let result = run(script, Some(":0"));
        assert!(String::from_utf8_lossy(&result.stdout).starts_with(":0\n"));
        drop(lease);
        // Without a session, only the stable part: Needle's commands on PATH.
        let stable = Command::new(&agent)
            .arg("env")
            .env("HOME", home.path())
            .env_remove("NEWPORT_SESSION")
            .run_output();
        assert!(stable.status.success());
        let stable = String::from_utf8_lossy(&stable.stdout);
        assert!(
            stable.contains("/.local/bin") && !stable.contains("DISPLAY"),
            "{stable}"
        );
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

#[test]
fn invalid_shell_configuration_fails_before_alias_changes() {
    for content in [
        "# >>> Newport >>>\nincomplete",
        "# >>> Newport >>>\n# >>> Newport >>>\n# <<< Newport <<<\n",
    ] {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join(".local/bin");
        fs::create_dir_all(&bin).unwrap();
        let rc = home.path().join(".bashrc");
        fs::write(&rc, content).unwrap();
        let result = Command::new(BIN)
            .arg("install")
            .env("HOME", home.path())
            .env("SHELL", "/bin/bash")
            .run_output();
        assert!(!result.status.success());
        for name in ["xclip", "xsel", "wl-paste", "xdg-open", "newport-browser"] {
            assert!(fs::symlink_metadata(bin.join(name)).is_err(), "{name}");
        }
        assert_eq!(fs::read_to_string(&rc).unwrap(), content);
    }
}

#[test]
fn unwritable_shell_parent_is_skipped_without_creating_missing_directories() {
    for missing in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let parent = home.path().join("managed-config");
        fs::create_dir(&parent).unwrap();
        let zdotdir = if missing {
            parent.join("nested")
        } else {
            parent.clone()
        };
        let rc = zdotdir.join(".zshrc");
        if !missing {
            fs::write(&rc, "# user settings\n").unwrap();
        }
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o500)).unwrap();
        let output = Command::new(BIN)
            .arg("install")
            .env("HOME", home.path())
            .env("SHELL", "/bin/zsh")
            .env("ZDOTDIR", &zdotdir)
            .run_output();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let notice = String::from_utf8_lossy(&output.stderr);
        assert!(notice.contains("Manual shell setup needed"), "{notice}");
        assert!(notice.contains(rc.to_str().unwrap()), "{notice}");
        if missing {
            assert!(!zdotdir.exists());
        } else {
            assert_eq!(fs::read_to_string(rc).unwrap(), "# user settings\n");
        }
        assert!(
            fs::symlink_metadata(home.path().join(".local/bin/newport-browser"))
                .unwrap()
                .is_symlink()
        );
    }
}

#[test]
fn shell_files_newport_cant_write_get_a_notice_and_needle_still_installs() {
    use std::os::unix::fs::symlink;
    for mode in ["symlink", "read-only", "unsupported"] {
        let home = tempfile::tempdir().unwrap();
        let hash = install_managed(home.path());
        let target = home.path().join("managed-bashrc");
        fs::write(&target, "# managed configuration\n").unwrap();
        let rc = home.path().join(".bashrc");
        if mode == "symlink" {
            symlink(&target, &rc).unwrap();
        } else {
            fs::copy(&target, &rc).unwrap();
            if mode == "read-only" {
                fs::set_permissions(&rc, fs::Permissions::from_mode(0o400)).unwrap();
            }
        }
        let install = || {
            let output = Command::new(BIN)
                .args(["deployment", "install", &hash, "--integrations"])
                .env("HOME", home.path())
                .env(
                    "SHELL",
                    if mode == "unsupported" {
                        "/bin/tcsh"
                    } else {
                        "/bin/bash"
                    },
                )
                .env_remove("ZDOTDIR")
                .env_remove("XDG_CONFIG_HOME")
                .run_output();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
        };
        let result = install();
        assert_eq!(result["build"], hash.as_str());
        let notice = result["notice"].as_str().unwrap();
        assert!(notice.contains("Manual shell setup needed"));
        if mode != "unsupported" {
            assert!(notice.contains(rc.to_str().unwrap()));
        }
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "# managed configuration\n"
        );
        if mode == "symlink" {
            assert_eq!(fs::read_link(&rc).unwrap(), target);
        } else {
            assert_eq!(
                fs::read_to_string(&rc).unwrap(),
                "# managed configuration\n"
            );
        }
        assert_eq!(install(), result, "installing again changes nothing");
        assert!(
            home.path()
                .join(".local/bin/newport-browser")
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
