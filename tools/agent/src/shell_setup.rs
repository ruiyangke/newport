//! Idempotent, guarded shell startup integration.
use fs2::FileExt;
use std::{
    env, fs,
    io::{self, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

const START: &str = "# >>> Porthop >>>";
const END: &str = "# <<< Porthop <<<";

pub fn block(fish: bool) -> String {
    let body = if fish {
        "if test -x \"$HOME/.local/bin/porthop-agent\"\n    if set -l _porthop_env (\"$HOME/.local/bin/porthop-agent\" env --shell fish 2>/dev/null)\n        eval (string join \\n -- $_porthop_env | string collect)\n    end\nend"
    } else {
        "if [ -x \"$HOME/.local/bin/porthop-agent\" ]; then\n    if _porthop_env=$(\"$HOME/.local/bin/porthop-agent\" env 2>/dev/null); then\n        eval \"$_porthop_env\"\n    fi\n    unset _porthop_env\nfi"
    };
    format!("{START}\n{body}\n{END}\n")
}

fn merge(original: &str, block: &str) -> io::Result<String> {
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    let mut offset = 0;
    for line in original.split_inclusive('\n') {
        match line.trim_end_matches(['\r', '\n']) {
            START => starts.push(offset),
            END => ends.push(offset + line.len()),
            _ => {}
        }
        offset += line.len();
    }
    match (starts.as_slice(), ends.as_slice()) {
        ([], []) => Ok(format!(
            "{original}{}{block}",
            if original.is_empty() || original.ends_with('\n') {
                ""
            } else {
                "\n"
            }
        )),
        ([start], [end]) if start < end => Ok(format!(
            "{}{block}{}",
            &original[..*start],
            &original[*end..]
        )),
        _ => Err(io::Error::other(
            "Porthop shell markers are incomplete or duplicated; leaving file unchanged",
        )),
    }
}

fn patch(path: &Path, fish: bool) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(m) if m.is_file() && m.uid() == unsafe { libc::geteuid() } => Some(m),
        Ok(_) => {
            return Err(io::Error::other(format!(
                "{} is not a regular file owned by this account",
                path.display()
            )))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    if let Some(meta) = &metadata {
        // Atomic rename can replace a read-only file in a writable directory.
        // Respect the file's permissions even when running as root.
        if meta.mode() & 0o200 == 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} is read-only; leaving it unchanged", path.display()),
            ));
        }
        // Also respect ACLs and filesystem protections, without writing bytes.
        fs::OpenOptions::new().write(true).open(path)?;
    }
    let original = if metadata.is_some() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };
    let updated = merge(&original, &block(fish))?;
    if updated == original {
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Missing shell directory"))?;
    fs::create_dir_all(parent)?;
    if metadata.is_some() {
        let mut backup = tempfile::Builder::new()
            .prefix(&format!(
                "{}.porthop-backup-",
                path.file_name().unwrap().to_string_lossy()
            ))
            .tempfile_in(parent)?;
        backup.write_all(original.as_bytes())?;
        backup.as_file().sync_all()?;
        backup.keep().map_err(|e| e.error)?;
    }
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.as_file().set_permissions(
        metadata
            .as_ref()
            .map(|m| m.permissions())
            .unwrap_or_else(|| fs::Permissions::from_mode(0o600)),
    )?;
    file.write_all(updated.as_bytes())?;
    file.as_file().sync_all()?;
    // Do not overwrite a concurrent edit or a newly introduced symlink.
    match (&metadata, fs::symlink_metadata(path)) {
        (Some(before), Ok(after))
            if after.is_file()
                && before.ino() == after.ino()
                && before.mode() == after.mode()
                && before.uid() == after.uid()
                && fs::read_to_string(path)? == original =>
        {
            fs::OpenOptions::new().write(true).open(path)?;
        }
        (None, Err(e)) if e.kind() == io::ErrorKind::NotFound => {}
        _ => {
            return Err(io::Error::other(
                "Shell configuration changed during setup; try again",
            ))
        }
    }
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}

pub fn install() -> io::Result<()> {
    let home =
        PathBuf::from(env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is required"))?);
    let shell = env::var("SHELL").unwrap_or_default();
    let shell = Path::new(&shell)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let paths = match shell {
        "bash" => {
            // Bash login shells read only the first existing login profile.
            let login = [".bash_profile", ".bash_login", ".profile"]
                .into_iter()
                .map(|name| home.join(name))
                .find(|path| fs::symlink_metadata(path).is_ok())
                .unwrap_or_else(|| home.join(".profile"));
            vec![home.join(".bashrc"), login]
        }
        "zsh" => vec![env::var_os("ZDOTDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.clone())
            .join(".zshrc")],
        "fish" => vec![env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("fish/conf.d/porthop.fish")],
        _ => {
            return Err(io::Error::other(
                "Shell not supported for automatic setup; configure it manually",
            ))
        }
    };
    let lock_dir = home.join(".local/share/porthop");
    fs::create_dir_all(&lock_dir)?;
    let lock = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_dir.join("shell-setup.lock"))?;
    lock.lock_exclusive()?;
    for path in paths {
        patch(&path, shell == "fish")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_content_and_installs_once() {
        let old = "export CUSTOM=hello\n# user settings";
        let once = merge(old, &block(false)).unwrap();
        assert!(once.starts_with(old));
        assert_eq!(merge(&once, &block(false)).unwrap(), once);
        assert_eq!(once.matches(START).count(), 1);
        let changed = once.replace("env 2>", "env --old 2>");
        assert_eq!(merge(&changed, &block(false)).unwrap(), once);
        assert!(merge(START, &block(false)).is_err());
        assert!(merge(&format!("{}{}", block(false), block(false)), &block(false)).is_err());
    }
    #[test]
    fn atomic_edit_preserves_permissions_and_backup() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(".bashrc");
        fs::write(&p, "# mine\n").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o640)).unwrap();
        patch(&p, false).unwrap();
        patch(&p, false).unwrap();
        assert_eq!(fs::metadata(&p).unwrap().mode() & 0o777, 0o640);
        let backups: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains("backup"))
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read_to_string(backups[0].path()).unwrap(), "# mine\n");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&p, &link).unwrap();
        assert!(patch(&link, false).is_err());
    }
    #[test]
    fn read_only_shell_files_are_never_replaced_or_backed_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".zshrc");
        for content in ["# user settings\n".to_owned(), block(false)] {
            fs::write(&path, &content).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
            let before = fs::metadata(&path).unwrap();
            assert_eq!(
                patch(&path, false).unwrap_err().kind(),
                io::ErrorKind::PermissionDenied
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), content);
            let after = fs::metadata(&path).unwrap();
            assert_eq!(before.ino(), after.ino());
            assert_eq!(after.mode() & 0o777, 0o444);
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    #[test]
    fn bash_startup_survives_missing_and_failing_agent() {
        let dir = tempfile::tempdir().unwrap();
        let rc = dir.path().join("rc");
        fs::write(&rc, block(false)).unwrap();
        let run = || {
            std::process::Command::new("bash")
                .args([
                    "--noprofile",
                    "--norc",
                    "-eu",
                    "-c",
                    ". \"$HOME/rc\"; printf alive",
                ])
                .env("HOME", dir.path())
                .output()
                .unwrap()
        };
        let result = run();
        assert!(result.status.success());
        assert_eq!(result.stdout, b"alive");
        let bin = dir.path().join(".local/bin");
        fs::create_dir_all(&bin).unwrap();
        let agent = bin.join("porthop-agent");
        fs::write(&agent, "#!/bin/sh\necho 'exit 99'\nexit 1\n").unwrap();
        fs::set_permissions(agent, fs::Permissions::from_mode(0o700)).unwrap();
        let result = run();
        assert!(result.status.success());
        assert_eq!(result.stdout, b"alive");
    }
}
