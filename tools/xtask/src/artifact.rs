//! `newport-build.json`: binds a reusable build artifact to its commit and exact contents.
use crate::root;
use crate::util::{git_head, read_json, sha256_file};
use anyhow::{bail, ensure, Context, Result};
use clap::Subcommand;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

const MANIFEST: &str = "newport-build.json";

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Write the manifest for a directory.
    Record { directory: PathBuf },
    /// Require the directory to match its manifest and this checkout's commit.
    Verify { directory: PathBuf },
}

pub fn run(action: Action) -> Result<()> {
    match action {
        Action::Record { directory } => record(&directory),
        Action::Verify { directory } => verify(&directory),
    }
}

#[derive(Deserialize)]
struct Manifest {
    commit: String,
    files: BTreeMap<String, String>,
}

fn resolve(directory: &Path) -> PathBuf {
    if directory.is_absolute() {
        directory.to_owned()
    } else {
        root().join(directory)
    }
}

/// Every regular file below `directory` except the manifest, as `/`-separated
/// relative path and SHA-256, depth first in name order.
fn snapshot(directory: &Path) -> Result<Vec<(String, String)>> {
    fn walk(base: &Path, relative: &str, files: &mut Vec<(String, String)>) -> Result<()> {
        let mut entries = fs::read_dir(base.join(relative))
            .with_context(|| format!("Cannot read {}", base.join(relative).display()))?
            .collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry
                .file_name()
                .into_string()
                .map_err(|name| anyhow::anyhow!("Invalid artifact file name: {name:?}"))?;
            let path = if relative.is_empty() {
                name
            } else {
                format!("{relative}/{name}")
            };
            if path == MANIFEST {
                continue;
            }
            let kind = entry.file_type()?;
            if kind.is_dir() {
                walk(base, &path, files)?;
            } else if kind.is_file() {
                let hash = sha256_file(&entry.path())?;
                files.push((path, hash));
            } else {
                bail!("Unexpected artifact entry: {path}");
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(&resolve(directory), "", &mut files)?;
    ensure!(!files.is_empty(), "Empty artifact: {}", directory.display());
    Ok(files)
}

fn manifest_json(commit: &str, files: &[(String, String)]) -> String {
    let files = files
        .iter()
        .map(|(path, hash)| format!("{}:{}", serde_json::json!(path), serde_json::json!(hash)))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"commit\":{},\"files\":{{{files}}}}}\n",
        serde_json::json!(commit)
    )
}

pub fn record(directory: &Path) -> Result<()> {
    let files = snapshot(directory)?;
    let path = resolve(directory).join(MANIFEST);
    fs::write(&path, manifest_json(&git_head()?, &files))
        .with_context(|| format!("Cannot write {}", path.display()))
}

pub fn verify(directory: &Path) -> Result<()> {
    let expected: Manifest = read_json(&resolve(directory).join(MANIFEST))?;
    let files: BTreeMap<_, _> = snapshot(directory)?.into_iter().collect();
    ensure!(
        expected.commit == git_head()? && expected.files == files,
        "Artifact does not match this checkout: {}",
        directory.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(result: Result<()>) -> String {
        result.unwrap_err().to_string()
    }

    #[test]
    fn rejects_stale_changed_incomplete_and_extra_contents() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path();
        fs::write(path.join("payload"), "original").unwrap();
        assert!(verify(path).is_err());
        record(path).unwrap();
        verify(path).unwrap();
        fs::write(path.join("payload"), "changed").unwrap();
        assert!(message(verify(path)).contains("does not match"));
        fs::write(path.join("payload"), "original").unwrap();
        fs::write(path.join("extra"), "unexpected").unwrap();
        assert!(message(verify(path)).contains("does not match"));
        fs::remove_file(path.join("extra")).unwrap();
        let text = fs::read_to_string(path.join(MANIFEST)).unwrap();
        let mut manifest: serde_json::Value = serde_json::from_str(&text).unwrap();
        manifest["commit"] = "0".repeat(40).into();
        fs::write(path.join(MANIFEST), manifest.to_string()).unwrap();
        assert!(message(verify(path)).contains("does not match"));
        record(path).unwrap();
        fs::remove_file(path.join("payload")).unwrap();
        assert!(message(verify(path)).contains("Empty artifact"));
    }

    #[test]
    fn nested_files_use_slash_paths_and_any_key_order() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path();
        fs::create_dir(path.join("a")).unwrap();
        fs::write(path.join("a/b"), "one").unwrap();
        fs::write(path.join("a-c"), "two").unwrap();
        record(path).unwrap();
        let text = fs::read_to_string(path.join(MANIFEST)).unwrap();
        assert!(text.starts_with("{\"commit\":") && text.ends_with("}}\n"));
        assert!(text.find("\"a/b\"").unwrap() < text.find("\"a-c\"").unwrap());
        let manifest: serde_json::Value = serde_json::from_str(&text).unwrap();
        let files = manifest["files"].as_object().unwrap();
        assert_eq!(files.len(), 2);
        let reordered = format!(
            "{{\"files\":{{\"a/b\":{},\"a-c\":{}}},\"commit\":{}}}",
            files["a/b"], files["a-c"], manifest["commit"]
        );
        fs::write(path.join(MANIFEST), reordered).unwrap();
        verify(path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path();
        fs::write(path.join("payload"), "original").unwrap();
        std::os::unix::fs::symlink(path.join("payload"), path.join("alias")).unwrap();
        assert!(message(record(path)).contains("Unexpected artifact entry: alias"));
    }

    #[test]
    fn relative_directories_resolve_from_the_repository_root() {
        assert_eq!(
            resolve(Path::new("resources/agents")),
            root().join("resources/agents")
        );
    }
}
