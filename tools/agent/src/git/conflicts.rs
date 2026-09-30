//! Resolve a conflicted path to one of its existing sides. The chosen content is
//! always a blob already recorded in the index; client bytes are never accepted.
use super::{
    discard::{guard_path, write_file},
    operations::{self, IndexLock},
    protocol::{ConflictSide, Error},
    repository,
};
use git2::Repository;
use serde_json::{json, Value};
use std::{ffi::OsStr, fs, os::unix::ffi::OsStrExt, path::Path};

fn engine(_: git2::Error) -> Error {
    Error::new("GIT_ERROR", "Cannot read the conflicted index entry.")
}
fn io_error(_: std::io::Error) -> Error {
    Error::new("IO_ERROR", "Cannot prepare the conflicted file.")
}
fn unknown() -> Error {
    Error::new(
        "OUTCOME_UNKNOWN",
        "The conflict may be partly resolved. Inspect its saved outcome before retrying.",
    )
}
fn unsupported() -> Error {
    Error::new(
        "UNSUPPORTED_CONFLICT",
        "Only regular text or binary files can be resolved by choosing a side. Symlink, submodule and filtered paths must be resolved with other tools.",
    )
}

pub fn apply(
    repo: &Repository,
    paths: &[Vec<u8>],
    side: ConflictSide,
    expected_oid: Option<&str>,
    expected: &str,
) -> Result<Value, Error> {
    // A status entry can carry both sides of a rename; only one holds a conflict.
    if paths.is_empty() || paths.len() > 2 {
        return Err(Error::invalid("Resolve exactly one conflicted file."));
    }
    let root = repo
        .workdir()
        .ok_or_else(|| Error::invalid("Resolving a conflict requires a working tree."))?;
    let mut lock = IndexLock::acquire(repo)?;
    if repository::fingerprint(repo)? != expected {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "Refresh the conflict before resolving it.",
        ));
    }
    let original = repo.index().map_err(engine)?;
    let mut found = None;
    for bytes in paths {
        let candidate = Path::new(OsStr::from_bytes(bytes));
        let Ok(conflict) = original.conflict_get(candidate) else {
            continue;
        };
        if conflict.ancestor.is_none() && conflict.our.is_none() && conflict.their.is_none() {
            continue;
        }
        if found.is_some() {
            return Err(Error::invalid("Resolve exactly one conflicted file."));
        }
        found = Some((bytes, conflict));
    }
    let (path_bytes, conflict) =
        found.ok_or_else(|| Error::new("NOT_CONFLICTED", "This file has no recorded conflict."))?;
    let path = Path::new(OsStr::from_bytes(path_bytes));
    guard_path(repo, root, path_bytes, &original, &original)?;
    let chosen = match side {
        ConflictSide::Base => conflict.ancestor,
        ConflictSide::Ours => conflict.our,
        ConflictSide::Theirs => conflict.their,
    };
    // Bind the write to the side the caller actually read, including its absence.
    if chosen.as_ref().map(|entry| entry.id.to_string()).as_deref() != expected_oid {
        return Err(Error::new(
            "STALE_CONFLICT",
            "This side changed since the conflict was read. Refresh and choose again.",
        ));
    }
    if let Some(entry) = &chosen {
        if !matches!(entry.mode, 0o100644 | 0o100755) {
            return Err(unsupported());
        }
    }
    let temporary = tempfile::tempdir_in(repo.path()).map_err(io_error)?;
    let index_path = temporary.path().join("index");
    if repo.path().join("index").exists() {
        fs::copy(repo.path().join("index"), &index_path).map_err(io_error)?;
    }
    let mut prepared = operations::private_index(repo, &index_path)?;
    prepared.conflict_remove(path).map_err(engine)?;
    if let Some(entry) = &chosen {
        // Stat fields stay zero so Git re-examines the file it is about to see.
        prepared
            .add(&git2::IndexEntry {
                ctime: git2::IndexTime::new(0, 0),
                mtime: git2::IndexTime::new(0, 0),
                dev: 0,
                ino: 0,
                mode: entry.mode,
                uid: 0,
                gid: 0,
                file_size: 0,
                id: entry.id,
                flags: 0,
                flags_extended: 0,
                path: path_bytes.clone(),
            })
            .map_err(engine)?;
    } else if let Err(e) = prepared.remove_path(path) {
        if e.code() != git2::ErrorCode::NotFound {
            return Err(engine(e));
        }
    }
    prepared.write().map_err(engine)?;
    let fresh = Repository::open(repo.path()).map_err(engine)?;
    if repository::fingerprint(&fresh)? != expected {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "The conflict changed while it was being resolved.",
        ));
    }
    // Write the file before publishing the index. An interrupted write leaves the
    // conflict recorded and retryable; the reverse would claim a resolution the
    // working file does not have.
    match &chosen {
        Some(entry) => {
            let blob = repo.find_blob(entry.id).map_err(engine)?;
            let mode = if entry.mode == 0o100755 {
                git2::FileMode::BlobExecutable
            } else {
                git2::FileMode::Blob
            };
            write_file(root, path, blob.content(), mode).map_err(|_| unknown())?;
        }
        None => match fs::remove_file(root.join(path)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(unknown()),
        },
    }
    lock.publish(&index_path, repo).map_err(|_| unknown())?;
    Ok(json!({
        "side": side,
        "resolvedPaths": 1,
        "deleted": chosen.is_none(),
        "indexChanged": true,
        "workingTreeChanged": true,
        "refreshRequired": true
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::protocol::Action;
    use std::fs;

    /// A repository stopped in a real merge conflict, plus an unrelated edit.
    fn conflicted() -> (tempfile::TempDir, Repository) {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        let signature = git2::Signature::now("Test", "test@example.test").unwrap();
        fs::write(temp.path().join("file"), "base\n").unwrap();
        fs::write(temp.path().join("other"), "other\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("file")).unwrap();
        index.add_path(Path::new("other")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let base = repo
            .commit(Some("HEAD"), &signature, &signature, "base", &tree, &[])
            .unwrap();
        drop(tree);

        let write_commit = |content: &str, message: &str, parent: git2::Oid| {
            fs::write(temp.path().join("file"), content).unwrap();
            let mut index = repo.index().unwrap();
            index.add_path(Path::new("file")).unwrap();
            index.write().unwrap();
            let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
            let parent = repo.find_commit(parent).unwrap();
            let oid = repo
                .commit(None, &signature, &signature, message, &tree, &[&parent])
                .unwrap();
            drop(tree);
            oid
        };
        let ours = write_commit("ours\n", "ours", base);
        let theirs = write_commit("theirs\n", "theirs", base);
        repo.reference("refs/heads/main", ours, true, "ours")
            .unwrap();
        repo.set_head("refs/heads/main").unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        let their_commit = repo.find_commit(theirs).unwrap();
        repo.merge(
            &[&repo.find_annotated_commit(theirs).unwrap()],
            None,
            Some(git2::build::CheckoutBuilder::new().force()),
        )
        .unwrap();
        drop(their_commit);
        assert!(repo.index().unwrap().has_conflicts());
        (temp, repo)
    }
    fn sides(repo: &Repository) -> (Option<String>, Option<String>, Option<String>) {
        let index = repo.index().unwrap();
        let conflict = index.conflict_get(Path::new("file")).unwrap();
        let id =
            |entry: &Option<git2::IndexEntry>| entry.as_ref().map(|entry| entry.id.to_string());
        (
            id(&conflict.ancestor),
            id(&conflict.our),
            id(&conflict.their),
        )
    }
    fn resolve(
        repo: &Repository,
        side: ConflictSide,
        expected_oid: Option<String>,
    ) -> Result<Value, Error> {
        operations::apply(
            repo,
            &Action::ConflictResolve {
                entry_ids: vec!["entry".into()],
                side,
                expected_oid,
            },
            &[b"file".to_vec()],
            &repository::fingerprint(repo).unwrap(),
        )
    }
    fn staged(repo: &Repository, path: &str) -> Vec<u8> {
        let index = repo.index().unwrap();
        let entry = index.get_path(Path::new(path), 0).unwrap();
        repo.find_blob(entry.id).unwrap().content().to_vec()
    }

    #[test]
    fn resolves_to_each_side_and_clears_the_conflict() {
        for (side, expected) in [
            (ConflictSide::Ours, "ours\n"),
            (ConflictSide::Theirs, "theirs\n"),
            (ConflictSide::Base, "base\n"),
        ] {
            let (temp, repo) = conflicted();
            let (base, ours, theirs) = sides(&repo);
            let oid = match side {
                ConflictSide::Base => base,
                ConflictSide::Ours => ours,
                ConflictSide::Theirs => theirs,
            };
            resolve(&repo, side, oid).unwrap();
            let repo = Repository::open(temp.path()).unwrap();
            // The working file and the index both hold the chosen side.
            assert_eq!(
                fs::read(temp.path().join("file")).unwrap(),
                expected.as_bytes()
            );
            assert_eq!(staged(&repo, "file"), expected.as_bytes());
            assert!(!repo.index().unwrap().has_conflicts());
            // Unrelated files and the merge itself are untouched.
            assert_eq!(fs::read(temp.path().join("other")).unwrap(), b"other\n");
            assert_eq!(repo.state(), git2::RepositoryState::Merge);
            assert!(!repo.path().join("index.lock").exists());
        }
    }

    #[test]
    fn refuses_a_side_that_changed_and_leaves_the_conflict_recorded() {
        let (temp, repo) = conflicted();
        let (_, ours, _) = sides(&repo);
        for wrong in [Some("0".repeat(40)), None, ours.clone()] {
            // Only the exact identity of the requested side is accepted.
            if wrong == ours {
                continue;
            }
            let error = resolve(&repo, ConflictSide::Ours, wrong).unwrap_err();
            assert_eq!(error.code, "STALE_CONFLICT");
        }
        let repo = Repository::open(temp.path()).unwrap();
        assert!(repo.index().unwrap().has_conflicts());
        assert!(!repo.path().join("index.lock").exists());
    }

    /// Build a conflict from two real branch edits rather than a synthetic index.
    fn conflict_between(
        ours: &dyn Fn(&std::path::Path, &Repository),
        theirs: &dyn Fn(&std::path::Path, &Repository),
    ) -> (tempfile::TempDir, Repository) {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        let signature = git2::Signature::now("Test", "test@example.test").unwrap();
        fs::write(temp.path().join("file"), "base\n").unwrap();
        fs::write(temp.path().join("other"), "other\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("file")).unwrap();
        index.add_path(Path::new("other")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let base = repo
            .commit(Some("HEAD"), &signature, &signature, "base", &tree, &[])
            .unwrap();
        drop(tree);
        let build = |mutate: &dyn Fn(&std::path::Path, &Repository), message: &str| {
            repo.set_head_detached(base).unwrap();
            repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
                .unwrap();
            let mut index = repo.index().unwrap();
            index
                .read_tree(&repo.find_commit(base).unwrap().tree().unwrap())
                .unwrap();
            index.write().unwrap();
            mutate(temp.path(), &repo);
            let mut index = repo.index().unwrap();
            index.write().unwrap();
            let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
            let parent = repo.find_commit(base).unwrap();
            let oid = repo
                .commit(None, &signature, &signature, message, &tree, &[&parent])
                .unwrap();
            drop(tree);
            oid
        };
        let our_oid = build(ours, "ours");
        let their_oid = build(theirs, "theirs");
        repo.reference("refs/heads/main", our_oid, true, "ours")
            .unwrap();
        repo.set_head("refs/heads/main").unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        repo.merge(
            &[&repo.find_annotated_commit(their_oid).unwrap()],
            None,
            Some(git2::build::CheckoutBuilder::new().force()),
        )
        .unwrap();
        assert!(repo.index().unwrap().has_conflicts());
        (temp, repo)
    }

    #[test]
    fn resolving_to_an_absent_side_deletes_the_file() {
        // Ours edits the file; theirs removes it.
        let (temp, repo) = conflict_between(
            &|root, repo| {
                fs::write(root.join("file"), "ours\n").unwrap();
                let mut index = repo.index().unwrap();
                index.add_path(Path::new("file")).unwrap();
                index.write().unwrap();
            },
            &|root, repo| {
                fs::remove_file(root.join("file")).unwrap();
                let mut index = repo.index().unwrap();
                index.remove_path(Path::new("file")).unwrap();
                index.write().unwrap();
            },
        );
        let (_, _, theirs) = sides(&repo);
        assert!(theirs.is_none(), "expected a modify/delete conflict");
        resolve(&repo, ConflictSide::Theirs, None).unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        assert!(!temp.path().join("file").exists());
        assert!(repo
            .index()
            .unwrap()
            .get_path(Path::new("file"), 0)
            .is_none());
        assert!(!repo.index().unwrap().has_conflicts());
        assert_eq!(fs::read(temp.path().join("other")).unwrap(), b"other\n");
    }

    #[test]
    fn refuses_unsupported_modes_without_touching_the_index() {
        // Ours replaces the file with a symlink; theirs edits its content.
        let (temp, repo) = conflict_between(
            &|root, repo| {
                fs::remove_file(root.join("file")).unwrap();
                std::os::unix::fs::symlink("other", root.join("file")).unwrap();
                let mut index = repo.index().unwrap();
                index.add_path(Path::new("file")).unwrap();
                index.write().unwrap();
            },
            &|root, repo| {
                fs::write(root.join("file"), "theirs\n").unwrap();
                let mut index = repo.index().unwrap();
                index.add_path(Path::new("file")).unwrap();
                index.write().unwrap();
            },
        );
        let (_, ours, theirs) = sides(&repo);
        let error = resolve(&repo, ConflictSide::Ours, ours).unwrap_err();
        assert_eq!(error.code, "UNSUPPORTED_CONFLICT");
        // The other side of the same conflict still resolves normally.
        resolve(&repo, ConflictSide::Theirs, theirs).unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"theirs\n");
        assert!(!repo.index().unwrap().has_conflicts());
    }
}
