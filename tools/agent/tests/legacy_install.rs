//! Upgrade real Porthop layouts through the public installation commands.
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::Path,
    process::{Command, Output},
    sync::OnceLock,
};

const BIN: &str = env!("CARGO_BIN_EXE_newport-agent");
const OLD_BLOCK: &str = "# >>> Porthop >>>\neval old-porthop-environment\n# <<< Porthop <<<\n";
const ALIASES: [&str; 4] = ["xclip", "xsel", "wl-paste", "xdg-open"];

fn run(home: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .env("HOME", home)
        .env("SHELL", "/bin/bash")
        .env("ZDOTDIR", home.join("zsh"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .output()
        .unwrap()
}

fn success(home: &Path, args: &[&str]) -> String {
    let output = run(home, args);
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| {
        Sha256::digest(fs::read(BIN).unwrap())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    })
}

fn write(home: &Path, name: &str, contents: impl AsRef<[u8]>) {
    let path = home.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn fixture(home: &Path) {
    write(
        home,
        ".local/bin/porthop-agent",
        b"\x7fELF...porthop-agent/5...",
    );
    write(
        home,
        ".local/bin/porthop-clip",
        "#!/usr/bin/env bash\n# Porthop clipboard helper\n",
    );
    write(
        home,
        ".local/bin/porthop-open",
        "#!/usr/bin/env bash\n# Porthop browser helper\n",
    );
    let bin = home.join(".local/bin");
    for (alias, target) in [
        ("xclip", "porthop-agent"),
        ("xsel", "./porthop-clip"),
        ("wl-paste", "porthop-clip"),
        ("xdg-open", "porthop-open"),
        ("porthop-browser", "porthop-agent"),
    ] {
        // Cover relative, absolute, and subsequently dangling helper links.
        let target = if alias == "xdg-open" {
            bin.join(target)
        } else {
            target.into()
        };
        symlink(target, bin.join(alias)).unwrap();
    }
    for name in [
        ".bashrc",
        ".bash_profile",
        ".bash_login",
        ".profile",
        ".zshrc",
        "zsh/.zshrc",
    ] {
        write(home, name, format!("# my prefix\n{OLD_BLOCK}# my suffix\n"));
        fs::set_permissions(home.join(name), fs::Permissions::from_mode(0o640)).unwrap();
    }
    for name in [
        ".config/fish/conf.d/porthop.fish",
        "config/fish/conf.d/porthop.fish",
    ] {
        write(home, name, OLD_BLOCK);
    }
    write(
        home,
        ".cache/porthop/clipboard/snapshot.tar",
        "cached clipboard",
    );
    write(home, ".local/share/porthop/shell-setup.lock", "");
    write(home, ".cache/newport/clipboard/keep", "current state");
}

fn removed(home: &Path, name: &str) {
    assert!(
        fs::symlink_metadata(home.join(name)).is_err(),
        "left {name}"
    );
}

#[test]
fn manual_and_managed_installs_remove_porthop_and_recreate_only_requested_links() {
    for mode in ["manual", "managed", "integrations"] {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        fixture(home);
        if mode != "manual" {
            assert!(!run(home, &["deployment", "install", &"0".repeat(64)])
                .status
                .success());
            assert!(
                home.join(".local/bin/porthop-agent").is_file(),
                "a failed activation must preserve Porthop"
            );
            success(home, &["deployment", "prepare", hash()]);
            assert!(
                home.join(".local/bin/porthop-agent").is_file(),
                "staging must preserve Porthop"
            );
            assert_eq!(
                fs::read_link(home.join(".local/bin/xclip")).unwrap(),
                Path::new("porthop-agent")
            );
        }
        let args = match mode {
            "manual" => vec!["install"],
            "managed" => vec!["deployment", "install", hash()],
            _ => vec!["deployment", "install", hash(), "--integrations"],
        };
        for _ in 0..2 {
            success(home, &args);
            for name in [
                "porthop-agent",
                "porthop-browser",
                "porthop-clip",
                "porthop-open",
            ] {
                removed(home, &format!(".local/bin/{name}"));
            }
            for alias in ALIASES {
                if mode == "managed" {
                    removed(home, &format!(".local/bin/{alias}"));
                } else {
                    assert_eq!(
                        fs::read_link(home.join(".local/bin").join(alias)).unwrap(),
                        Path::new("newport-agent")
                    );
                }
            }
            for name in [
                ".bashrc",
                ".bash_profile",
                ".bash_login",
                ".profile",
                ".zshrc",
                "zsh/.zshrc",
            ] {
                let contents = fs::read_to_string(home.join(name)).unwrap();
                assert!(!contents.contains("Porthop"), "{name}");
                assert!(contents.contains("# my prefix\n# my suffix\n"), "{name}");
                assert_eq!(
                    fs::metadata(home.join(name)).unwrap().permissions().mode() & 0o777,
                    0o640
                );
                let integrated = mode != "managed" && matches!(name, ".bashrc" | ".bash_profile");
                assert_eq!(
                    contents.matches("# >>> Newport >>>").count(),
                    usize::from(integrated)
                );
            }
            for name in [
                ".config/fish/conf.d/porthop.fish",
                "config/fish/conf.d/porthop.fish",
                ".cache/porthop",
                ".local/share/porthop",
            ] {
                removed(home, name);
            }
            assert_eq!(
                fs::read_to_string(home.join(".cache/newport/clipboard/keep")).unwrap(),
                "current state"
            );
        }
    }
}

#[test]
fn dangling_old_links_and_renamed_entry_points_are_removed() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let bin = home.join(".local/bin");
    fs::create_dir_all(&bin).unwrap();
    for name in ALIASES {
        symlink(bin.join("porthop-agent"), bin.join(name)).unwrap();
    }
    for name in ["porthop-agent", "porthop-browser"] {
        symlink("newport-agent", bin.join(name)).unwrap();
    }
    // Unrelated broken dotfile links must not block cleanup or be replaced.
    symlink("missing-dotfiles", home.join(".bashrc")).unwrap();
    success(home, &["install"]);
    assert_eq!(
        fs::read_link(home.join(".bashrc")).unwrap(),
        Path::new("missing-dotfiles")
    );
    removed(home, ".local/bin/porthop-agent");
    removed(home, ".local/bin/porthop-browser");
    for name in ALIASES {
        assert_eq!(
            fs::read_link(bin.join(name)).unwrap(),
            Path::new("newport-agent")
        );
    }
}

#[test]
fn cleanup_finds_custom_names_and_link_chains_without_touching_other_commands() {
    for agent_present in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let bin = home.join(".local/bin");
        fs::create_dir_all(&bin).unwrap();
        if agent_present {
            write(
                home,
                ".local/bin/porthop-agent",
                b"\x7fELF...porthop-agent/5...",
            );
        }
        for (name, target) in [
            ("my-copy", "porthop-agent"),
            ("my paste", "./porthop-agent"),
            ("my-open", "../bin/porthop-agent"),
            ("a-copy", "z-copy"),
            ("z-copy", "my-copy"),
            ("old-clip", "porthop-clip"),
            ("old-open", "porthop-open"),
            ("old-browser", "porthop-browser"),
            ("porthop-browser", "newport-agent"),
        ] {
            symlink(target, bin.join(name)).unwrap();
        }
        symlink(bin.join("porthop-agent"), bin.join("absolute-copy")).unwrap();
        for (name, target) in [
            ("other-copy", "/another/bin/porthop-agent"),
            ("current-copy", "newport-agent"),
            ("porthop-personal", "newport-agent"),
            ("cycle-a", "cycle-b"),
            ("cycle-b", "cycle-a"),
        ] {
            symlink(target, bin.join(name)).unwrap();
        }
        write(home, ".local/bin/my-script", "exec porthop-agent\n");
        write(home, "outside/keep", "user data");
        symlink(home.join("outside"), bin.join("linked-directory")).unwrap();
        // Directory entries are inspected, not recursively traversed.
        symlink(bin.join("porthop-agent"), home.join("outside/outside-copy")).unwrap();

        success(home, &["deployment", "prepare", hash()]);
        for _ in 0..2 {
            success(home, &["deployment", "install", hash()]);
            for name in [
                "my-copy",
                "my paste",
                "my-open",
                "a-copy",
                "z-copy",
                "old-clip",
                "old-open",
                "old-browser",
                "porthop-browser",
                "absolute-copy",
                "porthop-agent",
            ] {
                removed(home, &format!(".local/bin/{name}"));
            }
            for (name, target) in [
                ("other-copy", "/another/bin/porthop-agent"),
                ("current-copy", "newport-agent"),
                ("porthop-personal", "newport-agent"),
                ("cycle-a", "cycle-b"),
                ("cycle-b", "cycle-a"),
            ] {
                assert_eq!(fs::read_link(bin.join(name)).unwrap(), Path::new(target));
            }
            assert_eq!(
                fs::read_to_string(bin.join("my-script")).unwrap(),
                "exec porthop-agent\n"
            );
            assert_eq!(
                fs::read_link(bin.join("linked-directory")).unwrap(),
                home.join("outside")
            );
            assert_eq!(
                fs::read_link(home.join("outside/outside-copy")).unwrap(),
                bin.join("porthop-agent")
            );
            assert_eq!(
                fs::read_to_string(home.join("outside/keep")).unwrap(),
                "user data"
            );
        }
    }
}

#[test]
fn unrelated_commands_and_files_outside_owned_state_are_preserved() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    for name in ["porthop-agent", "porthop-clip", "porthop-open", "xclip"] {
        write(home, &format!(".local/bin/{name}"), "my command\n");
    }
    let bin = home.join(".local/bin");
    for name in ["wl-paste", "xdg-open", "porthop-browser"] {
        symlink("/somewhere/else/porthop-agent", bin.join(name)).unwrap();
    }
    write(home, ".local/share/porthop/mine", "keep");
    write(home, ".local/share/porthop/shell-setup.lock", "");
    write(home, "external/clipboard/keep", "keep");
    fs::create_dir_all(home.join(".cache")).unwrap();
    symlink(home.join("external"), home.join(".cache/porthop")).unwrap();
    write(
        home,
        "config/fish/conf.d/porthop.fish",
        format!("# my settings\n{OLD_BLOCK}"),
    );
    success(home, &["install"]);
    for name in ["porthop-agent", "porthop-clip", "porthop-open", "xclip"] {
        assert_eq!(fs::read_to_string(bin.join(name)).unwrap(), "my command\n");
    }
    for name in ["wl-paste", "xdg-open", "porthop-browser"] {
        assert_eq!(
            fs::read_link(bin.join(name)).unwrap(),
            Path::new("/somewhere/else/porthop-agent")
        );
    }
    removed(home, ".cache/porthop");
    removed(home, ".local/share/porthop/shell-setup.lock");
    assert_eq!(
        fs::read_to_string(home.join("external/clipboard/keep")).unwrap(),
        "keep"
    );
    assert_eq!(
        fs::read_to_string(home.join(".local/share/porthop/mine")).unwrap(),
        "keep"
    );
    assert_eq!(
        fs::read_to_string(home.join("config/fish/conf.d/porthop.fish")).unwrap(),
        "# my settings\n"
    );
}

#[test]
fn protected_shell_files_report_manual_cleanup_and_malformed_blocks_stop_before_links() {
    for linked in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        write(home, "managed-rc", OLD_BLOCK);
        if linked {
            symlink(home.join("managed-rc"), home.join(".bashrc")).unwrap();
        } else {
            write(home, ".bashrc", OLD_BLOCK);
            fs::set_permissions(home.join(".bashrc"), fs::Permissions::from_mode(0o444)).unwrap();
        }
        success(home, &["deployment", "prepare", hash()]);
        let output = success(home, &["deployment", "install", hash()]);
        assert!(output.contains("Remove its Porthop block"), "{output}");
        assert_eq!(fs::read_to_string(home.join(".bashrc")).unwrap(), OLD_BLOCK);
        assert_eq!(
            fs::read_to_string(home.join("managed-rc")).unwrap(),
            OLD_BLOCK
        );
    }
    for block in [
        "# >>> Porthop >>>\nincomplete\n".to_owned(),
        OLD_BLOCK.repeat(2),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        fixture(home);
        write(home, ".bashrc", &block);
        assert!(!run(home, &["install"]).status.success());
        assert_eq!(fs::read_to_string(home.join(".bashrc")).unwrap(), block);
        assert!(home.join(".local/bin/porthop-agent").is_file());
        assert_eq!(
            fs::read_link(home.join(".local/bin/xclip")).unwrap(),
            Path::new("porthop-agent")
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn installation_stops_the_obsolete_agent_before_removing_its_state() {
    use std::{io::Write, os::unix::process::CommandExt};
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let sleep = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|path| path.join("sleep"))
        .find(|path| path.is_file())
        .unwrap();
    let bin = home.join(".local/bin");
    fs::create_dir_all(&bin).unwrap();
    let agent = bin.join("porthop-agent");
    fs::copy(sleep, &agent).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&agent)
        .unwrap()
        .write_all(b"porthop-agent/5")
        .unwrap();
    // A parallel test's fork can briefly inherit the copy's write descriptor
    // until it execs. Linux refuses to run the copy during that interval.
    let mut command = Command::new(&agent);
    command.arg0("sleep").arg("30");
    let mut process = (0..100)
        .find_map(|_| match command.spawn() {
            Ok(child) => Some(child),
            Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(std::time::Duration::from_millis(20));
                None
            }
            Err(error) => panic!("could not start the old agent fixture: {error}"),
        })
        .expect("the old agent fixture stayed busy");
    let output = run(home, &["install"]);
    let stopped = (0..100).any(|_| {
        if process.try_wait().unwrap().is_some() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        false
    });
    let _ = process.kill();
    let _ = process.wait();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stopped);
    removed(home, ".local/bin/porthop-agent");
}
