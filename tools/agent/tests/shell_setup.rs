//! Exercise the installed startup blocks using real shells when available.
use std::{
    fs,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    process::Command,
};
const BIN: &str = env!("CARGO_BIN_EXE_newport-agent");

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
        ["deployment", "activate", &hash, "missing"].as_slice(),
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
fn removal_plan_composes_install_history_without_mutating_targets_or_receipts() {
    let home = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        Command::new(BIN)
            .args(args)
            .env("HOME", home.path())
            .env("SHELL", "/bin/bash")
            .env_remove("ZDOTDIR")
            .env_remove("XDG_CONFIG_HOME")
            .run_output()
    };
    let protocol = run(&["deployment", "protocol"]);
    let protocol: serde_json::Value = serde_json::from_slice(&protocol.stdout).unwrap();
    assert_eq!(protocol["helperRemovalPlan"], true);
    assert!(!run(&["deployment", "helper-removal-plan"]).status.success());
    assert!(!home.path().join(".local").exists());

    let shell = home.path().join(".bashrc");
    fs::write(&shell, "# original\n").unwrap();
    install_managed(home.path());
    let operation = uuid::Uuid::new_v4().to_string();
    let managed = run(&[
        "deployment",
        "helpers",
        protocol["buildSha256"].as_str().unwrap(),
        "--operation",
        &operation,
    ]);
    assert!(
        managed.status.success(),
        "{}",
        String::from_utf8_lossy(&managed.stderr)
    );
    for _ in 0..2 {
        let result = run(&["install"]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let content = format!("{}# added later\n", fs::read_to_string(&shell).unwrap());
    fs::write(&shell, &content).unwrap();
    let root = home.path().join(".local/share/newport/needle/helpers");
    let receipt = fs::read(root.join("journal.json")).unwrap();
    let objects = fs::read_dir(root.join("objects")).unwrap().count();
    let alias = home.path().join(".local/bin/newport-browser");
    let destination = fs::read_link(&alias).unwrap();
    let result = run(&["deployment", "helper-removal-plan"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    let files = report["files"].as_array().unwrap();
    assert_eq!(
        files
            .iter()
            .find(|f| f["path"] == shell.to_str().unwrap())
            .unwrap()["status"],
        "restore"
    );
    assert_eq!(
        files
            .iter()
            .find(|f| f["path"] == alias.to_str().unwrap())
            .unwrap()["status"],
        "remove"
    );
    assert_eq!(fs::read_to_string(&shell).unwrap(), content);
    assert_eq!(fs::read_link(&alias).unwrap(), destination);
    assert_eq!(fs::read(root.join("journal.json")).unwrap(), receipt);
    assert_eq!(fs::read_dir(root.join("objects")).unwrap().count(), objects);
    let repeated = run(&["deployment", "helper-removal-plan"]);
    assert_eq!(repeated.stdout, result.stdout);

    let edited = content.replace(" env ", " user-change ");
    assert_ne!(edited, content);
    fs::write(&shell, &edited).unwrap();
    let result = run(&["deployment", "helper-removal-plan"]);
    assert!(result.status.success());
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    let row = report["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == shell.to_str().unwrap())
        .unwrap();
    assert_eq!(row["status"], "conflict");
    assert!(row["fingerprint"].is_null());
    assert_eq!(fs::read_to_string(&shell).unwrap(), edited);
    fs::remove_file(root.join("journal.json")).unwrap();
    let missing = run(&["deployment", "helper-removal-plan"]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("evidence is missing"));
    assert!(!root.join("journal.json").exists());
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
            assert!(fs::read_to_string(home.path().join(".bash_profile"))
                .unwrap()
                .contains("# >>> Newport >>>"));
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
        assert!(text.starts_with(":99\n"), "{shell}: {text}");
        if shell == "fish" {
            assert_eq!(text, ":99\n1\n");
        } else {
            assert_eq!(text.matches(bin.to_str().unwrap()).count(), 1);
        }
        let result = run(script, Some(":0"));
        assert!(String::from_utf8_lossy(&result.stdout).starts_with(":0\n"));
        drop(lease);
        assert!(!Command::new(&agent)
            .arg("env")
            .env("HOME", home.path())
            .env_remove("NEWPORT_SESSION")
            .run_output()
            .status
            .success());
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
        for name in ["xclip", "wl-paste", "xdg-open", "newport-browser"] {
            assert!(fs::symlink_metadata(bin.join(name)).is_err(), "{name}");
        }
        assert_eq!(fs::read_to_string(&rc).unwrap(), content);
    }
}

#[test]
fn journaled_activation_records_helper_ownership_and_never_replays_completed_install() {
    use sha2::{Digest, Sha256};
    let home = tempfile::tempdir().unwrap();
    let hash: String = Sha256::digest(fs::read(BIN).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let id = uuid::Uuid::new_v4().to_string();
    let run = |args: &[&str]| {
        Command::new(BIN)
            .args(args)
            .env("HOME", home.path())
            .env("SHELL", "/bin/bash")
            .env_remove("ZDOTDIR")
            .env_remove("XDG_CONFIG_HOME")
            .run_output()
    };
    let prepare = run(&["deployment", "prepare", &hash]);
    assert!(
        prepare.status.success(),
        "{}",
        String::from_utf8_lossy(&prepare.stderr)
    );
    let args = [
        "deployment",
        "activate",
        &hash,
        "missing",
        "--operation",
        &id,
        "--integrations",
    ];
    let result = run(&args);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let receipt: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(receipt["receipt"]["state"], "succeeded");
    let journal = home
        .path()
        .join(".local/share/newport/needle/helpers/journal.json");
    let saved = fs::read(&journal).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&saved).unwrap();
    assert_eq!(value["records"][0]["id"], id);
    assert_eq!(value["records"][0]["state"], "Applied");
    assert_eq!(value["records"][0]["changes"].as_array().unwrap().len(), 6);
    fs::write(home.path().join(".bashrc"), b"# later user edit\n").unwrap();
    let repeated = run(&args);
    assert!(repeated.status.success());
    assert_eq!(fs::read(&journal).unwrap(), saved);
    assert_eq!(
        fs::read(home.path().join(".bashrc")).unwrap(),
        b"# later user edit\n"
    );
}

#[test]
fn helper_only_operation_preserves_the_installed_entry_and_recovers_by_id() {
    use std::os::unix::fs::MetadataExt;
    let home = tempfile::tempdir().unwrap();
    let protocol = Command::new(BIN)
        .args(["deployment", "protocol"])
        .env("HOME", home.path())
        .run_output();
    let value: serde_json::Value = serde_json::from_slice(&protocol.stdout).unwrap();
    assert!(
        !home.path().join(".local/share/newport/needle").exists(),
        "capability probe is read-only"
    );
    let hash = install_managed(home.path());
    assert_eq!(value["helperOperations"], true);
    assert_eq!(value["buildSha256"], hash);
    let bin = home.path().join(".local/bin/newport-agent");
    let inode = fs::metadata(&bin).unwrap().ino();
    let run = |args: &[&str]| {
        Command::new(&bin)
            .args(args)
            .env("HOME", home.path())
            .env("SHELL", "/bin/bash")
            .run_output()
    };
    let id = uuid::Uuid::new_v4().to_string();
    let result = run(&["deployment", "helpers", &hash, "--operation", &id]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["receipt"]["intent"]["kind"], "helpers");
    assert_eq!(value["receipt"]["state"], "succeeded");
    assert_eq!(fs::metadata(&bin).unwrap().ino(), inode);
    let result = run(&["deployment", "recover", &id]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["receipt"]["state"], "rolledBack");
    assert_eq!(fs::metadata(&bin).unwrap().ino(), inode);
    assert_eq!(fs::read(&bin).unwrap(), fs::read(BIN).unwrap());
    assert!(!home.path().join(".bashrc").exists());
    assert!(fs::symlink_metadata(home.path().join(".local/bin/xclip")).is_err());
    // A second operation exercises conflict review followed by a surgical
    // restore, through the public CLI and its durable recovery receipt.
    let id = uuid::Uuid::new_v4().to_string();
    assert!(run(&["deployment", "helpers", &hash, "--operation", &id])
        .status
        .success());
    let shell = home.path().join(".bashrc");
    let installed = fs::read_to_string(&shell).unwrap();
    let edited = installed.replace(" env ", " edited ");
    assert_ne!(edited, installed);
    fs::write(&shell, &edited).unwrap();
    let result = run(&["deployment", "recover", &id]);
    let receipt: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(receipt["receipt"]["state"], "rollingBack");
    assert!(receipt["receipt"]["error"]
        .as_str()
        .unwrap()
        .contains("edited"));
    assert_eq!(fs::read_to_string(&shell).unwrap(), edited);
    fs::write(&shell, format!("# new prefix\n{installed}# new suffix\n")).unwrap();
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o640)).unwrap();
    let result = run(&["deployment", "recover", &id]);
    let receipt: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(receipt["receipt"]["state"], "rolledBack", "{receipt}");
    assert_eq!(
        fs::read_to_string(&shell).unwrap(),
        "# new prefix\n# new suffix\n"
    );
    assert_eq!(fs::metadata(&shell).unwrap().mode() & 0o777, 0o640);
    assert_eq!(fs::metadata(&bin).unwrap().ino(), inode);
    let id = uuid::Uuid::new_v4().to_string();
    assert!(run(&["deployment", "helpers", &hash, "--operation", &id])
        .status
        .success());
    let edited = fs::read_to_string(&shell)
        .unwrap()
        .replace(" env ", " customized ");
    fs::write(&shell, &edited).unwrap();
    let _ = run(&["deployment", "recover", &id]);
    let review = run(&["deployment", "review", &id]);
    assert!(
        review.status.success(),
        "{}",
        String::from_utf8_lossy(&review.stderr)
    );
    let review: serde_json::Value = serde_json::from_slice(&review.stdout).unwrap();
    let file = review["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == shell.to_str().unwrap())
        .unwrap();
    let index = file["index"].as_u64().unwrap().to_string();
    let fingerprint = file["fingerprint"].as_str().unwrap();
    let keep = run(&["deployment", "keep", &id, &index, fingerprint]);
    assert!(
        keep.status.success(),
        "{}",
        String::from_utf8_lossy(&keep.stderr)
    );
    let duplicate = run(&["deployment", "keep", &id, &index, fingerprint]);
    assert!(duplicate.status.success());
    let restored = run(&["deployment", "recover", &id]);
    let restored: serde_json::Value = serde_json::from_slice(&restored.stdout).unwrap();
    assert_eq!(restored["receipt"]["state"], "rolledBack");
    assert_eq!(fs::read_to_string(&shell).unwrap(), edited);
}

#[test]
fn managed_shell_files_keep_agent_active_and_report_durable_manual_setup() {
    use std::os::unix::fs::symlink;
    for mode in ["symlink", "read-only", "unsupported"] {
        let home = tempfile::tempdir().unwrap();
        install_managed(home.path());
        let bin = home.path().join(".local/bin/newport-agent");
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
        let run = |args: &[&str]| {
            let output = Command::new(&bin)
                .args(args)
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
        let protocol = run(&["deployment", "protocol"]);
        let hash = protocol["buildSha256"].as_str().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let result = run(&["deployment", "helpers", hash, "--operation", &id]);
        assert_eq!(result["activeBuild"], hash);
        assert_eq!(result["receipt"]["state"], "succeeded");
        let notice = result["receipt"]["error"].as_str().unwrap();
        assert!(notice.contains("Manual shell setup needed"));
        if mode != "unsupported" {
            assert!(notice.contains(rc.to_str().unwrap()));
        }
        assert_eq!(
            fs::read_to_string(&rc).unwrap(),
            "# managed configuration\n"
        );
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "# managed configuration\n"
        );
        if mode == "symlink" {
            assert_eq!(fs::read_link(&rc).unwrap(), target);
        }
        assert_eq!(
            run(&["deployment", "helpers", hash, "--operation", &id]),
            result
        );
        assert!(home
            .path()
            .join(".local/bin/newport-browser")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
        // A later binary-only activation neither reruns nor depends on shell setup.
        let update = uuid::Uuid::new_v4().to_string();
        let update = run(&["deployment", "activate", hash, hash, "--operation", &update]);
        assert_eq!(update["receipt"]["state"], "succeeded");
        assert_eq!(
            fs::read_to_string(&rc).unwrap(),
            "# managed configuration\n"
        );
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
fn cancelled_shell_plan_does_not_require_recovery_or_replace_agent() {
    let home = tempfile::tempdir().unwrap();
    install_managed(home.path());
    let bin = home.path().join(".local/bin/newport-agent");
    let rc = home.path().join(".bashrc");
    fs::write(&rc, "# >>> Newport >>>\nbroken block\n").unwrap();
    let run = |args: &[&str]| {
        let output = Command::new(&bin)
            .args(args)
            .env("HOME", home.path())
            .env("SHELL", "/bin/bash")
            .run_output();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let protocol = run(&["deployment", "protocol"]);
    let hash = protocol["buildSha256"].as_str().unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let result = run(&["deployment", "helpers", hash, "--operation", &id]);
    assert_eq!(result["activeBuild"], hash);
    assert_eq!(result["receipt"]["state"], "rolledBack");
    assert!(result["receipt"]["error"]
        .as_str()
        .unwrap()
        .contains("no helper changes remain"));
    assert!(fs::symlink_metadata(home.path().join(".local/bin/newport-browser")).is_err());
    let next = uuid::Uuid::new_v4().to_string();
    let result = run(&["deployment", "activate", hash, hash, "--operation", &next]);
    assert_eq!(result["receipt"]["state"], "succeeded");
}
