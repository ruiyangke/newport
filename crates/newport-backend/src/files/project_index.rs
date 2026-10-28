//! Worktree file names for Quick Open. No file contents or repository writes.
use super::{Operations, path_valid};
use crate::manager::Shared;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    id: Uuid,
    operation: Uuid,
    root: String,
    expected_connection: Vec<serde_json::Value>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Index {
    paths: Vec<String>,
    skipped: usize,
}
use crate::ssh::shell_quote as quote;
fn command(root: &str) -> Result<String> {
    path_valid(root)?;
    ensure!(
        root.starts_with('/') && !root.chars().any(char::is_control),
        "Choose an absolute worktree path."
    );
    let root = quote(root);
    // Canonical root checks prevent accidentally indexing a containing repository.
    // An empty NUL record separates the listing from tracked-but-deleted paths.
    Ok(format!(
        "cd -- {root} && test \"$(pwd -P)\" = {root} && test \"$(git rev-parse --show-toplevel)\" = {root} && printf 'NPI1\\000' && git ls-files --cached --others --exclude-standard -z && printf '\\000' && git ls-files --deleted -z"
    ))
}
fn supported(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 16_384
        && !path.chars().any(|c| c.is_control() || c == '\u{fffd}')
        && path
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".." | ".git"))
}
fn parse(output: &str) -> Result<Index> {
    ensure!(
        output.ends_with('\0'),
        "Incomplete project file index. Try again."
    );
    let mut records = output.split_terminator('\0');
    ensure!(
        records.next() == Some("NPI1"),
        "Invalid project file index."
    );
    let mut paths = BTreeSet::new();
    let mut skipped = 0;
    let mut separated = false;
    for path in records.by_ref() {
        if path.is_empty() {
            separated = true;
            break;
        }
        if supported(path) {
            paths.insert(path.to_owned());
        } else {
            skipped += 1;
        }
        ensure!(
            paths.len() <= 100_000,
            "This worktree has more than 100,000 files. Use the file explorer to open a file."
        );
    }
    ensure!(separated, "Incomplete project file index. Try again.");
    for deleted in records {
        paths.remove(deleted);
    }
    Ok(Index {
        paths: paths.into_iter().collect(),
        skipped,
    })
}
pub async fn dispatch(
    state: &Shared,
    operations: &Operations,
    request: Request,
) -> Result<Index, String> {
    let command = command(&request.root).map_err(|e| e.to_string())?;
    let expected = serde_json::Value::Array(request.expected_connection);
    operations
        .run_with_connection(
            state,
            request.id,
            request.operation,
            Some(&expected),
            async |server| {
                let output = crate::ssh::execute(&server, &command, None)
                    .await
                    .map_err(anyhow::Error::msg)?;
                parse(&output)
            },
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nul_records_preserve_paths_and_remove_deleted_duplicates() {
        let result = parse("NPI1\0src/main.rs\0space name.rs\0src/main.rs\0gone.rs\0../escape\0bad\nname\0\0gone.rs\0").unwrap();
        assert_eq!(result.paths, ["space name.rs", "src/main.rs"]);
        assert_eq!(result.skipped, 2);
        assert!(parse("NPI1\0src/main.rs\0").is_err());
        assert!(parse("NPI1\0\0").unwrap().paths.is_empty());
        assert!(command("relative").is_err());
    }
    // Runs the server's script through a local `sh` and Git, standing in for
    // the Linux host; Windows paths aren't server paths.
    #[cfg(unix)]
    #[test]
    fn actual_git_index_honors_ignores_untracked_and_deleted_files() {
        use std::{fs, process::Command};
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project's $(touch SHOULD_NOT_EXIST)");
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .arg(&root)
                .status()
                .unwrap()
                .success()
        );
        fs::write(root.join("tracked.rs"), "").unwrap();
        fs::write(root.join("deleted.rs"), "").unwrap();
        assert!(
            Command::new("git")
                .current_dir(&root)
                .args(["add", "tracked.rs", "deleted.rs"])
                .status()
                .unwrap()
                .success()
        );
        fs::remove_file(root.join("deleted.rs")).unwrap();
        fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
        fs::create_dir(root.join("ignored")).unwrap();
        fs::write(root.join("ignored/output"), "").unwrap();
        fs::create_dir(root.join("nested")).unwrap();
        fs::write(root.join("nested/untracked file.rs"), "").unwrap();
        let output = Command::new("sh")
            .arg("-c")
            .arg(command(root.to_str().unwrap()).unwrap())
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let index = parse(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
        assert_eq!(
            index.paths,
            [".gitignore", "nested/untracked file.rs", "tracked.rs"]
        );
        assert!(!root.join("SHOULD_NOT_EXIST").exists());
        let output = Command::new("sh")
            .arg("-c")
            .arg(command(root.join("nested").to_str().unwrap()).unwrap())
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "a subdirectory must not index its containing repository"
        );
    }
}
