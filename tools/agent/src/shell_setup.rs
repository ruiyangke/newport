//! Idempotent, guarded shell startup integration.
use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

#[cfg(test)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

const START: &str = "# >>> Newport >>>";
const END: &str = "# <<< Newport <<<";

pub fn block(fish: bool) -> String {
    let body = if fish {
        "if test -x \"$HOME/.local/bin/newport-agent\"\n    if set -l _newport_env (\"$HOME/.local/bin/newport-agent\" env --shell fish 2>/dev/null)\n        eval (string join \\n -- $_newport_env | string collect)\n    end\nend"
    } else {
        "if [ -x \"$HOME/.local/bin/newport-agent\" ]; then\n    if _newport_env=$(\"$HOME/.local/bin/newport-agent\" env 2>/dev/null); then\n        eval \"$_newport_env\"\n    fi\n    unset _newport_env\nfi"
    };
    format!("{START}\n{body}\n{END}\n")
}

fn block_range(original: &str) -> io::Result<Option<std::ops::Range<usize>>> {
    marked_range(original, START, END)
}

fn marked_range(
    original: &str,
    start_marker: &str,
    end_marker: &str,
) -> io::Result<Option<std::ops::Range<usize>>> {
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    let mut offset = 0;
    for line in original.split_inclusive('\n') {
        let length = line.len();
        let line = line.trim_end_matches(['\r', '\n']);
        if line == start_marker {
            starts.push(offset);
        } else if line == end_marker {
            ends.push(offset + length);
        }
        offset += length;
    }
    match (starts.as_slice(), ends.as_slice()) {
        ([], []) => Ok(None),
        ([start], [end]) if start < end => Ok(Some(*start..*end)),
        _ => Err(io::Error::other(format!(
            "Shell markers {start_marker:?} are incomplete or duplicated; leaving file unchanged"
        ))),
    }
}
fn merge(original: &str, block: &str) -> io::Result<String> {
    match block_range(original)? {
        None => Ok(format!(
            "{original}{}{block}",
            if original.is_empty() || original.ends_with('\n') {
                ""
            } else {
                "\n"
            }
        )),
        Some(range) => Ok(format!(
            "{}{block}{}",
            &original[..range.start],
            &original[range.end..]
        )),
    }
}

/// Adds or refreshes Newport's block in the shell's startup files. Files
/// managed through links (dotfile managers) and read-only ones are left alone,
/// with a notice. Returns the notices.
pub(crate) fn apply() -> io::Result<Vec<String>> {
    let home =
        PathBuf::from(env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is required"))?);
    let mut notices = remove_porthop(&home)?;
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
        "fish" => {
            let dir = env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".config"))
                .join("fish/conf.d");
            vec![dir.join("newport.fish")]
        }
        _ => {
            notices.push(format!(
                "Automatic setup is unavailable for shell {shell:?}. Configure your shell manually."
            ));
            return Ok(notices);
        }
    };
    for path in paths {
        match write_block(&path, shell == "fish") {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => notices.push(format!(
                "{} was left unchanged: {error}. Configure this file through its owner or dotfile manager.",
                path.display()
            )),
            Err(error) => return Err(error),
        }
    }
    Ok(notices)
}

/// Remove the old brand's startup block even when integrations aren't being
/// installed, and from shells other than the current login shell.
pub(crate) fn remove_porthop(home: &Path) -> io::Result<Vec<String>> {
    let mut paths: Vec<_> = [
        ".bashrc",
        ".bash_profile",
        ".bash_login",
        ".profile",
        ".zshrc",
    ]
    .into_iter()
    .map(|name| home.join(name))
    .collect();
    if let Some(directory) = env::var_os("ZDOTDIR") {
        paths.push(PathBuf::from(directory).join(".zshrc"));
    }
    paths.push(home.join(".config/fish/conf.d/porthop.fish"));
    if let Some(directory) = env::var_os("XDG_CONFIG_HOME") {
        paths.push(PathBuf::from(directory).join("fish/conf.d/porthop.fish"));
    }
    paths.sort();
    paths.dedup();
    let mut notices = Vec::new();
    for path in paths {
        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.is_file() && !metadata.is_symlink() => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
            _ => {}
        }
        // Reading a managed link is safe, but editing it is left to its owner.
        // Don't report unrelated read-only files as needing legacy cleanup.
        let original = match fs::read_to_string(&path) {
            Ok(original) => original,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                notices.push(format!(
                    "{} could not be checked for Porthop shell setup: {error}. Check this file through its owner or dotfile manager.",
                    path.display()
                ));
                continue;
            }
            Err(error) => return Err(error),
        };
        if !original
            .lines()
            .any(|line| matches!(line, "# >>> Porthop >>>" | "# <<< Porthop <<<"))
        {
            continue;
        }
        let result = edit_file(
            &path,
            path.file_name().is_some_and(|n| n == "porthop.fish"),
            |original| {
                let Some(range) = marked_range(original, "# >>> Porthop >>>", "# <<< Porthop <<<")?
                else {
                    return Ok(original.to_owned());
                };
                Ok(format!(
                    "{}{}",
                    &original[..range.start],
                    &original[range.end..]
                ))
            },
        );
        match result {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => notices.push(format!(
                "{} was left unchanged: {error}. Remove its Porthop block through its owner or dotfile manager.",
                path.display()
            )),
            Err(error) => return Err(error),
        }
    }
    Ok(notices)
}

/// Writes the block into one file, atomically and with its permissions.
fn write_block(path: &Path, fish: bool) -> io::Result<()> {
    edit_file(path, false, |original| merge(original, &block(fish)))
}

fn edit_file(
    path: &Path,
    remove_empty: bool,
    update: impl FnOnce(&str) -> io::Result<String>,
) -> io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt as _;
    let (original, mode) = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} is a symlink", path.display()),
            ))
        }
        Ok(metadata) => {
            let mode = metadata.permissions().mode() & 0o7777;
            if mode & 0o200 == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("{} is read-only", path.display()),
                ));
            }
            (fs::read_to_string(path)?, mode)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => (String::new(), 0o600),
        Err(error) => return Err(error),
    };
    let updated = update(&original)?;
    if updated == original {
        return Ok(());
    }
    if remove_empty && updated.trim().is_empty() {
        return fs::remove_file(path);
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Shell configuration has no folder"))?;
    fs::create_dir_all(parent)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    staged.write_all(updated.as_bytes())?;
    staged
        .as_file()
        .set_permissions(fs::Permissions::from_mode(mode))?;
    staged.as_file().sync_all()?;
    staged.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn patch(path: &Path, fish: bool) -> io::Result<()> {
        write_block(path, fish)
    }
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
    fn atomic_edit_preserves_permissions_and_refuses_links() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(".bashrc");
        fs::write(&p, "# mine\n").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o640)).unwrap();
        patch(&p, false).unwrap();
        patch(&p, false).unwrap();
        assert_eq!(fs::metadata(&p).unwrap().mode() & 0o777, 0o640);
        let text = fs::read_to_string(&p).unwrap();
        assert!(text.starts_with("# mine\n"));
        assert_eq!(text.matches(START).count(), 1);
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&p, &link).unwrap();
        assert!(patch(&link, false).is_err());
    }
    #[test]
    fn read_only_shell_files_are_never_replaced() {
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
        let agent = bin.join("newport-agent");
        fs::write(&agent, "#!/bin/sh\necho 'exit 99'\nexit 1\n").unwrap();
        fs::set_permissions(agent, fs::Permissions::from_mode(0o700)).unwrap();
        let result = run();
        assert!(result.status.success());
        assert_eq!(result.stdout, b"alive");
    }
}
