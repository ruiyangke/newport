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
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    let mut offset = 0;
    for line in original.split_inclusive('\n') {
        let length = line.len();
        let line = line.trim_end_matches(['\r', '\n']);
        if crate::migration::shell_start(line) {
            starts.push(offset);
        } else if crate::migration::shell_end(line) {
            ends.push(offset + length);
        }
        offset += length;
    }
    match (starts.as_slice(), ends.as_slice()) {
        ([], []) => Ok(None),
        ([start], [end]) if start < end => Ok(Some(*start..*end)),
        _ => Err(io::Error::other(
            "Newport shell markers are incomplete or duplicated; leaving file unchanged",
        )),
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

/// Undo only the recorded managed block, preserving current surrounding text.
/// Recognizing the original block also makes partial rollback crash-safe.
pub(crate) fn restore_block(before: &str, after: &str, current: &str) -> io::Result<String> {
    let after_range =
        block_range(after)?.ok_or_else(|| io::Error::other("Recorded shell block is missing"))?;
    let installed = &after[after_range];
    if merge(before, installed)? != after {
        return Err(io::Error::other(
            "Shell record changes content outside its managed block",
        ));
    }
    let original = block_range(before)?.map(|range| &before[range]);
    let current_range = block_range(current)?;
    let present = current_range.clone().map(|range| &current[range]);
    if present == original {
        return Ok(current.to_owned());
    }
    if present != Some(installed) {
        return Err(io::Error::other(
            "Newport shell block was edited; preserving it for review",
        ));
    }
    let range = current_range.unwrap();
    Ok(format!(
        "{}{}{}",
        &current[..range.start],
        original.unwrap_or(""),
        &current[range.end..]
    ))
}

pub(crate) fn plan(
    transaction: &mut crate::helper_transaction::Transaction,
) -> io::Result<Vec<String>> {
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
        "fish" => {
            let dir = env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".config"))
                .join("fish/conf.d");
            vec![crate::migration::fish_config(&dir)]
        }
        _ => {
            return Ok(vec![format!(
                "Automatic setup is unavailable for shell {shell:?}. Configure your shell manually."
            )])
        }
    };
    let mut notices = Vec::new();
    for path in paths {
        // Never follow a dotfile manager's link. Check effective permissions
        // and the directory needed for atomic replacement before planning.
        let check = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} is a symlink", path.display()),
            )),
            Ok(_) => crate::helper_transaction::check_write_permission(&path),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                crate::helper_transaction::check_write_permission(&path)
            }
            Err(error) => Err(error),
        };
        if let Err(error) = check {
            if error.kind() != io::ErrorKind::PermissionDenied {
                return Err(error);
            }
            notices.push(format!("{} was left unchanged: {error}. Configure this file through its owner or dotfile manager.", path.display()));
            continue;
        }
        plan_path(transaction, &path, shell == "fish")?;
    }
    Ok(notices)
}

fn plan_path(
    transaction: &mut crate::helper_transaction::Transaction,
    path: &Path,
    fish: bool,
) -> io::Result<()> {
    use crate::helper_transaction::Snapshot;
    let before = transaction.read(path)?;
    let (original, mode) = match &before {
        Snapshot::Missing => (String::new(), 0o600),
        Snapshot::File { mode, .. } => {
            if mode & 0o200 == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("{} is read-only; leaving it unchanged", path.display()),
                ));
            }
            (
                String::from_utf8(transaction.bytes(&before)?).map_err(io::Error::other)?,
                *mode,
            )
        }
        Snapshot::Link(_) => {
            return Err(io::Error::other(
                "Shell configuration is a symlink; leaving it unchanged",
            ))
        }
    };
    let updated = merge(&original, &block(fish))?;
    let after = transaction.file(updated.as_bytes(), mode)?;
    transaction.plan_shell(path, before, after)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn block_recovery_preserves_surroundings_and_restores_legacy_content() {
        for before in [
            "# original\n",
            "# original without newline",
            "",
            "# prefix\n# >>> Porthop >>>\nold command\n# <<< Porthop <<<\n# suffix\n",
        ] {
            let after = merge(before, &block(false)).unwrap();
            let current = format!("# new prefix\n{after}# new suffix\n");
            let restored = restore_block(before, &after, &current).unwrap();
            assert!(restored.starts_with("# new prefix\n"));
            assert!(restored.ends_with("# new suffix\n"));
            assert!(!restored.contains(START));
            assert_eq!(
                restored.contains("old command"),
                before.contains("old command")
            );
            assert_eq!(restore_block(before, &after, &restored).unwrap(), restored);
        }
    }
    #[test]
    fn block_recovery_refuses_changed_blocks_and_invalid_ownership() {
        let before = "# original\n";
        let after = merge(before, &block(false)).unwrap();
        for current in [
            after.replace(" env ", " changed "),
            after.replace(END, "# removed end"),
            format!("{after}{}", block(false)),
        ] {
            assert!(restore_block(before, &after, &current).is_err());
        }
        assert!(restore_block(before, &format!("# unowned change\n{after}"), &after).is_err());
        // A manually removed block is already restored; surrounding edits survive.
        assert_eq!(
            restore_block(before, &after, "# only user content\n").unwrap(),
            "# only user content\n"
        );
    }
    fn patch(path: &Path, fish: bool) -> io::Result<()> {
        let mut transaction = crate::helper_transaction::Transaction::open(
            path.parent().unwrap(),
            uuid::Uuid::new_v4(),
        )?;
        plan_path(&mut transaction, path, fish)?;
        transaction.apply()
    }
    #[test]
    fn replaces_legacy_brand_block_without_duplicating_shell_setup() {
        let old =
            "export CUSTOM=yes\n# >>> Porthop >>>\nold command\n# <<< Porthop <<<\n# user footer\n";
        let new = merge(old, &block(false)).unwrap();
        assert!(new.starts_with("export CUSTOM=yes\n"));
        assert!(new.ends_with("# user footer\n"));
        assert!(!new.contains("Porthop"));
        assert_eq!(new.matches(START).count(), 1);
        assert_eq!(merge(&new, &block(false)).unwrap(), new);
        assert!(merge("# >>> Porthop >>>\nincomplete", &block(false)).is_err());
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
    fn atomic_edit_preserves_permissions_and_backup() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(".bashrc");
        fs::write(&p, "# mine\n").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o640)).unwrap();
        patch(&p, false).unwrap();
        patch(&p, false).unwrap();
        assert_eq!(fs::metadata(&p).unwrap().mode() & 0o777, 0o640);
        let objects = dir
            .path()
            .join(".local/share/newport/needle/helpers/objects");
        assert!(fs::read_dir(objects)
            .unwrap()
            .flatten()
            .any(|e| fs::read(e.path()).unwrap() == b"# mine\n"));
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
            assert!(!dir
                .path()
                .join(".local/share/newport/needle/helpers/journal.json")
                .exists());
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
