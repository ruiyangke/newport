use super::{home, Result};
use std::{
    fs,
    os::unix::fs::{symlink, MetadataExt, PermissionsExt},
    path::Path,
    process::Command,
};
fn execute(c: &mut Command) -> Result<String> {
    let result = c.output()?;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(String::from_utf8(result.stdout)?)
}
fn backups(path: &Path) -> Result<Vec<String>> {
    let mut names = fs::read_dir(path)?
        .map(|e| Ok(e?.file_name().to_string_lossy().into_owned()))
        .collect::<std::io::Result<Vec<_>>>()?;
    names.retain(|n| n.contains(".newport-backup-"));
    names.sort();
    Ok(names)
}
pub fn run() -> Result<()> {
    let agent = home().join(".local/bin/newport-agent");
    for (shell, rc_name) in [
        ("bash", ".bashrc"),
        ("zsh", ".zshrc"),
        ("fish", ".config/fish/conf.d/newport.fish"),
    ] {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let binary = home.join(".local/bin/newport-agent");
        let rc = home.join(rc_name);
        fs::create_dir_all(binary.parent().unwrap())?;
        fs::create_dir_all(rc.parent().unwrap())?;
        fs::copy(&agent, &binary)?;
        let original = "# user configuration\n";
        fs::write(&rc, original)?;
        let shell_path = execute(Command::new("which").arg(shell))?;
        let command = |program: &std::ffi::OsStr| {
            let mut c = Command::new(program);
            c.env("HOME", home)
                .env("SHELL", shell_path.trim())
                .env("RC", &rc);
            for key in [
                "ZDOTDIR",
                "XDG_CONFIG_HOME",
                "DISPLAY",
                "WAYLAND_DISPLAY",
                "XAUTHORITY",
            ] {
                c.env_remove(key);
            }
            c
        };
        let install = || execute(command(binary.as_os_str()).arg("install"));
        install()?;
        let installed = fs::read_to_string(&rc)?;
        assert!(installed.starts_with(original));
        assert_eq!(installed.matches("# >>> Newport >>>").count(), 1);
        install()?;
        assert_eq!(fs::read_to_string(&rc)?, installed);
        let state = home.join(".cache/newport/clipboard");
        fs::create_dir_all(&state)?;
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700))?;
        fs::write(state.join("features"), "clipboard browser")?;
        fs::write(state.join("display"), ":99")?;
        let run = |script: &str| {
            let args: &[&str] = match shell {
                "bash" => &["--noprofile", "--norc", "-eu"],
                "zsh" => &["-f"],
                _ => &["--no-config"],
            };
            execute(
                command(shell.as_ref())
                    .args(args)
                    .args(["-c", &format!("source \"$RC\"; source \"$RC\"; {script}")]),
            )
        };
        let output = run(if shell == "fish" {
            "printf '%s\\n' $DISPLAY; count (string match -- $HOME/.local/bin $PATH)"
        } else {
            "printf '%s\\n' \"$DISPLAY\" \"$PATH\""
        })?;
        let lines: Vec<_> = output.trim().lines().collect();
        assert_eq!(lines[0], ":99");
        if shell == "fish" {
            assert_eq!(lines[1], "1");
        } else {
            assert_eq!(
                lines[1]
                    .split(':')
                    .filter(|p| Path::new(p) == binary.parent().unwrap())
                    .count(),
                1
            );
        }
        fs::remove_file(&binary)?;
        assert_eq!(run("echo survived")?.trim(), "survived");
        fs::write(&binary, "#!/bin/sh\nexit 1\n")?;
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))?;
        assert_eq!(run("echo survived")?.trim(), "survived");
        fs::copy(&agent, &binary)?;
        fs::write(&rc, original)?;
        fs::set_permissions(&rc, fs::Permissions::from_mode(0o444))?;
        let before = fs::metadata(&rc)?;
        let saved = backups(rc.parent().unwrap())?;
        install()?;
        assert_eq!(fs::read_to_string(&rc)?, original);
        let after = fs::metadata(&rc)?;
        assert_eq!(before.ino(), after.ino());
        assert_eq!(before.mode(), after.mode());
        assert_eq!(backups(rc.parent().unwrap())?, saved);
        fs::set_permissions(&rc, fs::Permissions::from_mode(0o600))?;
        fs::remove_file(&rc)?;
        let protected = home.join("protected");
        fs::write(&protected, original)?;
        symlink(&protected, &rc)?;
        install()?;
        assert!(fs::symlink_metadata(&rc)?.is_symlink());
        assert_eq!(fs::read_to_string(protected)?, original);
        println!(
            "{shell}: repeated install, missing/failing agent, read-only and symlink checks passed"
        );
    }
    Ok(())
}
