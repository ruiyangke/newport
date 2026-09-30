//! Restore selected status paths without affecting other staging or working files.
use super::{
    checkout,
    operations::{self, IndexLock},
    protocol::{DiscardSource, Error, HunkSelection},
    repository,
};
use git2::{DiffOptions, Index, Patch, Repository};
use serde_json::{json, Value};
use std::{
    ffi::OsStr,
    fs,
    os::unix::ffi::OsStrExt,
    path::{Component, Path},
};
fn engine(_: git2::Error) -> Error {
    Error::new("GIT_ERROR", "Cannot prepare selected files for discard.")
}
fn io_error(_: std::io::Error) -> Error {
    Error::new("IO_ERROR", "Cannot inspect selected files for discard.")
}
fn unknown() -> Error {
    Error::new("OUTCOME_UNKNOWN","Discard may have restored or removed selected files. Inspect its saved outcome before retrying.")
}
/// Reject paths that a working-file write must never touch: escapes, replaced
/// parent directories, non-regular files, submodules and unsupported filters.
pub(super) fn guard_path(
    repo: &Repository,
    root: &Path,
    bytes: &[u8],
    baseline: &Index,
    original: &Index,
) -> Result<(), Error> {
    let path = Path::new(OsStr::from_bytes(bytes));
    if bytes.is_empty()
        || bytes.contains(&0)
        || path.components().any(
            |c| !matches!(c,Component::Normal(n) if !n.as_bytes().eq_ignore_ascii_case(b".git")),
        )
    {
        return Err(Error::invalid("Invalid selected path."));
    }
    let mut parent = path.parent();
    while let Some(p) = parent {
        match fs::symlink_metadata(root.join(p)) {
            Ok(m) if !m.is_dir() => {
                return Err(Error::new(
                    "CHECKOUT_CONFLICT",
                    "A selected file has a replaced parent directory.",
                ))
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_error(e)),
        }
        parent = p.parent();
    }
    match fs::symlink_metadata(root.join(path)) {
        Ok(m) if !m.is_file() && !m.file_type().is_symlink()=>return Err(Error::new("UNSUPPORTED_CAPABILITY","Discard directories and special files individually; recursive deletion is not supported.")),
        Ok(_)=>{},Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},Err(e)=>return Err(io_error(e)),
    }
    if baseline
        .get_path(path, 0)
        .is_some_and(|e| e.mode == 0o160000)
        || original
            .get_path(path, 0)
            .is_some_and(|e| e.mode == 0o160000)
    {
        return Err(Error::new(
            "UNSUPPORTED_CAPABILITY",
            "Submodule discard is not supported.",
        ));
    }
    for attr in ["filter", "working-tree-encoding"] {
        let value = repo
            .get_attr_bytes(path, attr, git2::AttrCheckFlags::FILE_THEN_INDEX)
            .map_err(engine)?;
        if !matches!(
            git2::AttrValue::from_bytes(value),
            git2::AttrValue::Unspecified | git2::AttrValue::False
        ) {
            return Err(Error::new(
                "UNSUPPORTED_FILTER",
                "This file requires an unsupported filter or encoding.",
            ));
        }
    }
    Ok(())
}

fn unsupported() -> Error {
    Error::new(
        "UNSUPPORTED_HUNKS",
        "Discarding selected hunks requires an unstaged text file without conflicts, renames, or mode changes. Discard the whole file instead.",
    )
}

/// Rewrite one working file so the selected hunks or lines return to the index,
/// leaving every other edit, the index itself and other files untouched. Only
/// the unstaged comparison is supported; `head` discard stays whole-file.
fn selected_hunks(
    repo: &Repository,
    paths: &[Vec<u8>],
    source: DiscardSource,
    selection: &HunkSelection,
    expected: &str,
) -> Result<Value, Error> {
    if paths.len() != 1
        || selection.ids.is_empty()
        || selection.ids.len() > 10_000
        || selection.context_lines > 100
    {
        return Err(Error::invalid(
            "Select hunks from exactly one file with contextLines at most 100.",
        ));
    }
    if source != DiscardSource::Index {
        return Err(unsupported());
    }
    let root = repo
        .workdir()
        .ok_or_else(|| Error::invalid("Discard requires a working tree."))?;
    // Hold the index lock so the comparison cannot shift while it is rebuilt,
    // even though this path never publishes an index.
    let _lock = IndexLock::acquire(repo)?;
    if repository::fingerprint(repo)? != expected {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "Refresh selected files before discarding changes.",
        ));
    }
    let index = repo.index().map_err(engine)?;
    if index.has_conflicts() || repo.state() != git2::RepositoryState::Clean {
        return Err(unsupported());
    }
    let path = Path::new(OsStr::from_bytes(&paths[0]));
    guard_path(repo, root, &paths[0], &index, &index)?;
    let mut options = DiffOptions::new();
    options
        .context_lines(selection.context_lines)
        .disable_pathspec_match(true)
        .pathspec(&paths[0])
        .include_untracked(true)
        .show_untracked_content(true)
        .max_size(2 * 1024 * 1024);
    let diff = repo
        .diff_index_to_workdir(Some(&index), Some(&mut options))
        .map_err(engine)?;
    if diff.deltas().len() != 1 {
        return Err(unsupported());
    }
    let patch = Patch::from_diff(&diff, 0)
        .map_err(engine)?
        .ok_or_else(unsupported)?;
    let delta = patch.delta();
    let status = match delta.status() {
        git2::Delta::Untracked => git2::Delta::Added,
        other => other,
    };
    let modes = match status {
        git2::Delta::Modified => {
            delta.old_file().mode() == delta.new_file().mode()
                && matches!(
                    delta.old_file().mode(),
                    git2::FileMode::Blob | git2::FileMode::BlobExecutable
                )
        }
        git2::Delta::Added => matches!(
            delta.new_file().mode(),
            git2::FileMode::Blob | git2::FileMode::BlobExecutable
        ),
        git2::Delta::Deleted => matches!(
            delta.old_file().mode(),
            git2::FileMode::Blob | git2::FileMode::BlobExecutable
        ),
        _ => false,
    };
    if !modes
        || delta.old_file().path_bytes() != Some(paths[0].as_slice())
        || delta.new_file().path_bytes() != Some(paths[0].as_slice())
        || delta.old_file().is_binary()
        || delta.new_file().is_binary()
    {
        return Err(unsupported());
    }
    let (hunk_ids, chosen) = super::hunks::chosen_hunks(&patch, &selection.ids)?;
    let (all, picked) = super::hunks::resolve(&patch, selection, &chosen, &hunk_ids)?;
    let indexed = super::hunks::blob_bytes(repo, delta.old_file().id())?;
    let current = match fs::read(root.join(path)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(io_error(e)),
    };
    // Refuse unless replaying every change reproduces the file as it is now, so
    // a comparison that no longer describes it can never overwrite real work.
    if super::hunks::reconstruct(&indexed, &patch, &all)? != current {
        return Err(unsupported());
    }
    let keep: super::hunks::Positions = all.difference(&picked).copied().collect();
    let rebuilt = super::hunks::reconstruct(&indexed, &patch, &keep)?;
    if rebuilt == current {
        return Err(Error::invalid("The selection discards no change."));
    }
    if rebuilt.is_empty() && status == git2::Delta::Added {
        // Every added line was discarded, so the untracked file itself goes.
        match fs::remove_file(root.join(path)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(unknown()),
        }
    } else {
        let mode = if status == git2::Delta::Added {
            delta.new_file().mode()
        } else {
            delta.old_file().mode()
        };
        write_file(root, path, &rebuilt, mode).map_err(|_| unknown())?;
    }
    Ok(
        json!({"source":source,"discardedPaths":1,"indexChanged":false,"workingTreeChanged":true,"refreshRequired":true}),
    )
}

/// Replace the working file through a private temporary file in the same
/// directory, so an interrupted write never leaves partial content in place.
pub(super) fn write_file(
    root: &Path,
    path: &Path,
    content: &[u8],
    mode: git2::FileMode,
) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    let target = root.join(path);
    let parent = target.parent().unwrap_or(root);
    let mut temporary = tempfile::Builder::new()
        .prefix(".newport-discard-")
        .tempfile_in(parent)?;
    temporary.write_all(content)?;
    temporary.as_file().sync_all()?;
    let bits = if mode == git2::FileMode::BlobExecutable {
        0o755
    } else {
        0o644
    };
    temporary
        .as_file()
        .set_permissions(std::fs::Permissions::from_mode(bits))?;
    temporary.persist(&target)?;
    Ok(())
}

pub fn apply(
    repo: &Repository,
    paths: &[Vec<u8>],
    source: DiscardSource,
    selection: Option<&HunkSelection>,
    expected: &str,
) -> Result<Value, Error> {
    if let Some(selection) = selection {
        return selected_hunks(repo, paths, source, selection, expected);
    }
    if paths.is_empty() || paths.len() > 20_000 {
        return Err(Error::invalid("Select files within the supported limit."));
    }
    let root = repo
        .workdir()
        .ok_or_else(|| Error::invalid("Discard requires a working tree."))?;
    let mut lock = IndexLock::acquire(repo)?;
    if repository::fingerprint(repo)? != expected {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "Refresh selected files before discarding changes.",
        ));
    }
    let original = repo.index().map_err(engine)?;
    if original.has_conflicts() {
        return Err(Error::new(
            "UNMERGED_INDEX",
            "Resolve the current conflicts before discarding changes.",
        ));
    }
    let mut baseline = Index::new().map_err(engine)?;
    if source == DiscardSource::Head {
        match repo.head() {
            Ok(head) => {
                let tree = head.peel_to_tree().map_err(engine)?;
                checkout::supported_files(repo, &tree)?;
                baseline.read_tree(&tree).map_err(engine)?;
            }
            Err(e) if e.code() == git2::ErrorCode::UnbornBranch => {}
            Err(e) => return Err(engine(e)),
        }
    }
    let baseline = if source == DiscardSource::Index {
        &original
    } else {
        &baseline
    };
    // Validate every selection before any working-file write. Never follow a
    // replaced parent symlink or recursively delete a directory selection.
    for bytes in paths {
        guard_path(repo, root, bytes, baseline, &original)?;
    }
    let temporary = tempfile::tempdir_in(repo.path()).map_err(io_error)?;
    let index_path = temporary.path().join("index");
    if repo.path().join("index").exists() {
        fs::copy(repo.path().join("index"), &index_path).map_err(io_error)?;
    }
    let mut prepared = operations::private_index(repo, &index_path)?;
    for bytes in paths {
        let path = Path::new(OsStr::from_bytes(bytes));
        if let Some(entry) = baseline.get_path(path, 0) {
            prepared.add(&entry).map_err(engine)?;
        } else if let Err(e) = prepared.remove_path(path) {
            if e.code() != git2::ErrorCode::NotFound {
                return Err(engine(e));
            }
        }
    }
    prepared.write().map_err(engine)?;
    let fresh = Repository::open(repo.path()).map_err(engine)?;
    if repository::fingerprint(&fresh)? != expected {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "Selected files changed while preparing discard.",
        ));
    }
    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout
        .force()
        .disable_pathspec_match(true)
        .update_index(false)
        .remove_untracked(false)
        .remove_ignored(false)
        .overwrite_ignored(false);
    let mut count = 0;
    for bytes in paths {
        let path = Path::new(OsStr::from_bytes(bytes));
        if baseline.get_path(path, 0).is_some() {
            checkout.path(path);
            count += 1;
        }
    }
    // No paths means all paths to libgit2: explicitly skip the checkout then.
    if count > 0 {
        repo.checkout_index(Some(&mut prepared), Some(&mut checkout))
            .map_err(|_| unknown())?;
    }
    for bytes in paths {
        let path = Path::new(OsStr::from_bytes(bytes));
        if baseline.get_path(path, 0).is_none() {
            match fs::remove_file(root.join(path)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(unknown()),
            }
        }
    }
    if source == DiscardSource::Head {
        lock.publish(&index_path, repo).map_err(|_| unknown())?;
    }
    Ok(
        json!({"source":source,"discardedPaths":paths.len(),"indexChanged":source==DiscardSource::Head,"workingTreeChanged":true,"refreshRequired":true}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Repository) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        let sig = git2::Signature::now("Test", "test@example.test").unwrap();
        let mut builder = repo.treebuilder(None).unwrap();
        builder
            .insert("file", repo.blob(b"head").unwrap(), 0o100644)
            .unwrap();
        builder
            .insert("keep", repo.blob(b"keep").unwrap(), 0o100644)
            .unwrap();
        let tree = repo.find_tree(builder.write().unwrap()).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        drop(tree);
        drop(builder);
        (tmp, repo)
    }
    fn run(repo: &Repository, paths: &[&[u8]], source: DiscardSource) -> Result<Value, Error> {
        operations::apply(
            repo,
            &super::super::protocol::Action::Discard {
                entry_ids: vec![],
                source,
                hunks: None,
            },
            &paths.iter().map(|p| p.to_vec()).collect::<Vec<_>>(),
            &repository::fingerprint(repo).unwrap(),
        )
    }
    #[test]
    fn index_discard_preserves_staging_and_unselected_edits() {
        let (tmp, repo) = fixture();
        fs::write(tmp.path().join("file"), "staged").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("file")).unwrap();
        index.write().unwrap();
        let bytes = fs::read(repo.path().join("index")).unwrap();
        fs::write(tmp.path().join("file"), "working").unwrap();
        fs::write(tmp.path().join("keep"), "unselected").unwrap();
        run(&repo, &[b"file"], DiscardSource::Index).unwrap();
        assert_eq!(fs::read(tmp.path().join("file")).unwrap(), b"staged");
        assert_eq!(fs::read(tmp.path().join("keep")).unwrap(), b"unselected");
        assert_eq!(fs::read(repo.path().join("index")).unwrap(), bytes);
    }
    #[test]
    fn head_discard_restores_selected_index_and_removes_selected_new_files() {
        let (tmp, repo) = fixture();
        let head = repo.head().unwrap().target();
        fs::write(tmp.path().join("file"), "staged").unwrap();
        fs::write(tmp.path().join("new"), "staged new").unwrap();
        fs::write(tmp.path().join("untracked"), "untracked").unwrap();
        fs::write(tmp.path().join("keep"), "keep staged").unwrap();
        let mut index = repo.index().unwrap();
        for p in ["file", "new", "keep"] {
            index.add_path(Path::new(p)).unwrap();
        }
        index.write().unwrap();
        run(&repo, &[b"file", b"new", b"untracked"], DiscardSource::Head).unwrap();
        let repo = Repository::open(tmp.path()).unwrap();
        assert_eq!(repo.head().unwrap().target(), head);
        assert_eq!(fs::read(tmp.path().join("file")).unwrap(), b"head");
        assert!(!tmp.path().join("new").exists());
        assert!(!tmp.path().join("untracked").exists());
        let index = repo.index().unwrap();
        assert!(index.get_path(Path::new("new"), 0).is_none());
        assert_eq!(
            repo.find_blob(index.get_path(Path::new("keep"), 0).unwrap().id)
                .unwrap()
                .content(),
            b"keep staged"
        );
    }
    #[test]
    fn deleted_files_restore_and_directory_selections_refuse_before_writes() {
        let (tmp, repo) = fixture();
        fs::remove_file(tmp.path().join("file")).unwrap();
        run(&repo, &[b"file"], DiscardSource::Index).unwrap();
        assert_eq!(fs::read(tmp.path().join("file")).unwrap(), b"head");
        fs::write(tmp.path().join("file"), "edit").unwrap();
        fs::create_dir(tmp.path().join("directory")).unwrap();
        fs::write(tmp.path().join("directory/keep"), "keep").unwrap();
        assert_eq!(
            run(&repo, &[b"file", b"directory/"], DiscardSource::Head)
                .unwrap_err()
                .code,
            "UNSUPPORTED_CAPABILITY"
        );
        assert_eq!(fs::read(tmp.path().join("file")).unwrap(), b"edit");
        assert!(tmp.path().join("directory/keep").exists());
    }
    #[test]
    fn stale_snapshot_and_parent_symlinks_are_refused() {
        let (tmp, repo) = fixture();
        let snapshot = repository::fingerprint(&repo).unwrap();
        fs::write(tmp.path().join("file"), "edit").unwrap();
        assert_eq!(
            apply(
                &repo,
                &[b"file".to_vec()],
                DiscardSource::Head,
                None,
                &snapshot
            )
            .unwrap_err()
            .code,
            "STALE_SNAPSHOT"
        );
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret"), "keep").unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("link")).unwrap();
        assert_eq!(
            run(&repo, &[b"link/secret"], DiscardSource::Head)
                .unwrap_err()
                .code,
            "CHECKOUT_CONFLICT"
        );
        assert_eq!(fs::read(outside.path().join("secret")).unwrap(), b"keep");
    }

    // ---- selected hunks and lines ----

    /// Commit a multi-line file, then edit two separated regions of it.
    fn partial() -> (tempfile::TempDir, Repository, String, String) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        let committed: String = (0..40).map(|i| format!("line {i}\n")).collect();
        fs::write(tmp.path().join("file"), &committed).unwrap();
        fs::write(tmp.path().join("keep"), "keep\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("file")).unwrap();
        index.add_path(Path::new("keep")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("Test", "test@example.test").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();
        drop(tree);
        let edited = committed
            .replace("line 2\n", "changed 2\n")
            .replace("line 32\n", "changed 32\n");
        fs::write(tmp.path().join("file"), &edited).unwrap();
        (tmp, repo, committed, edited)
    }
    /// Addressable changes of the unstaged comparison for one path.
    fn addressable(
        repo: &Repository,
        path: &str,
        context: u32,
    ) -> Vec<(String, String, char, String)> {
        let mut options = DiffOptions::new();
        options
            .pathspec(path)
            .context_lines(context)
            .include_untracked(true)
            .show_untracked_content(true);
        let diff = repo
            .diff_index_to_workdir(None, Some(&mut options))
            .unwrap();
        let patch = Patch::from_diff(&diff, 0).unwrap().unwrap();
        let mut out = Vec::new();
        for h in 0..patch.num_hunks() {
            let hunk = super::super::hunks::id(&patch, h).unwrap();
            let (_, count) = patch.hunk(h).unwrap();
            for l in 0..count {
                let line = patch.line_in_hunk(h, l).unwrap();
                if let Some(id) = super::super::hunks::line_id(&hunk, l, &line) {
                    out.push((
                        hunk.clone(),
                        id,
                        line.origin(),
                        String::from_utf8_lossy(line.content()).into_owned(),
                    ));
                }
            }
        }
        out
    }
    fn discard_selection(
        repo: &Repository,
        path: &[u8],
        ids: Vec<String>,
        lines: Option<Vec<String>>,
        source: DiscardSource,
    ) -> Result<Value, Error> {
        operations::apply(
            repo,
            &super::super::protocol::Action::Discard {
                entry_ids: vec!["entry".into()],
                source,
                hunks: Some(HunkSelection {
                    ids,
                    lines,
                    context_lines: 3,
                }),
            },
            &[path.to_vec()],
            &repository::fingerprint(repo).unwrap(),
        )
    }

    #[test]
    fn discards_one_hunk_and_keeps_the_other_edit_and_files() {
        let (tmp, repo, committed, _) = partial();
        let changes = addressable(&repo, "file", 3);
        let hunks: Vec<String> = {
            let mut seen: Vec<String> = Vec::new();
            for change in &changes {
                if !seen.contains(&change.0) {
                    seen.push(change.0.clone());
                }
            }
            seen
        };
        assert_eq!(hunks.len(), 2);
        discard_selection(
            &repo,
            b"file",
            vec![hunks[0].clone()],
            None,
            DiscardSource::Index,
        )
        .unwrap();
        // Only the first region returned to the committed content.
        assert_eq!(
            fs::read(tmp.path().join("file")).unwrap(),
            committed.replace("line 32\n", "changed 32\n").as_bytes()
        );
        assert_eq!(fs::read(tmp.path().join("keep")).unwrap(), b"keep\n");
        assert!(!repo.path().join("index.lock").exists());
    }

    #[test]
    fn discards_one_line_and_preserves_staged_content() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        fs::write(tmp.path().join("file"), "a\nb\nc\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("file")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("Test", "test@example.test").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();
        drop(tree);
        // Two adjacent additions; only the debug line is discarded.
        fs::write(tmp.path().join("file"), "a\nDEBUG\nkeep\nb\nc\n").unwrap();
        let changes = addressable(&repo, "file", 3);
        let debug = changes.iter().find(|c| c.3 == "DEBUG\n").unwrap();
        discard_selection(
            &repo,
            b"file",
            vec![debug.0.clone()],
            Some(vec![debug.1.clone()]),
            DiscardSource::Index,
        )
        .unwrap();
        assert_eq!(
            fs::read(tmp.path().join("file")).unwrap(),
            b"a\nkeep\nb\nc\n"
        );
        // The index was never republished by a worktree-only discard.
        let staged = repo.index().unwrap();
        let entry = staged.get_path(Path::new("file"), 0).unwrap();
        assert_eq!(repo.find_blob(entry.id).unwrap().content(), b"a\nb\nc\n");
    }

    #[test]
    fn discards_selected_lines_of_an_untracked_file_and_removes_it_when_empty() {
        let (tmp, repo, _, _) = partial();
        fs::write(tmp.path().join("fresh"), "1\n2\n3\n").unwrap();
        let changes = addressable(&repo, "fresh", 3);
        assert_eq!(changes.len(), 3);
        let two = changes.iter().find(|c| c.3 == "2\n").unwrap();
        discard_selection(
            &repo,
            b"fresh",
            vec![two.0.clone()],
            Some(vec![two.1.clone()]),
            DiscardSource::Index,
        )
        .unwrap();
        assert_eq!(fs::read(tmp.path().join("fresh")).unwrap(), b"1\n3\n");
        let rest = addressable(&repo, "fresh", 3);
        discard_selection(
            &repo,
            b"fresh",
            vec![rest[0].0.clone()],
            Some(rest.iter().map(|c| c.1.clone()).collect()),
            DiscardSource::Index,
        )
        .unwrap();
        // Discarding every added line removes the untracked file itself.
        assert!(!tmp.path().join("fresh").exists());
    }

    #[test]
    fn restores_selected_lines_of_a_deleted_file() {
        let (tmp, repo, committed, _) = partial();
        fs::remove_file(tmp.path().join("file")).unwrap();
        let changes = addressable(&repo, "file", 3);
        let first = changes.iter().find(|c| c.3 == "line 0\n").unwrap();
        discard_selection(
            &repo,
            b"file",
            vec![first.0.clone()],
            Some(vec![first.1.clone()]),
            DiscardSource::Index,
        )
        .unwrap();
        // Only the selected deletion came back; the rest stays deleted.
        assert_eq!(fs::read(tmp.path().join("file")).unwrap(), b"line 0\n");
        assert_eq!(committed.lines().count(), 40);
    }

    #[test]
    fn refuses_head_source_stale_and_empty_selections_without_writing() {
        let (tmp, repo, _, edited) = partial();
        let changes = addressable(&repo, "file", 3);
        let hunk = changes[0].0.clone();
        type Case = (
            Vec<String>,
            Option<Vec<String>>,
            DiscardSource,
            &'static str,
        );
        let cases: Vec<Case> = vec![
            (
                vec![hunk.clone()],
                None,
                DiscardSource::Head,
                "UNSUPPORTED_HUNKS",
            ),
            (
                vec!["0".repeat(64)],
                None,
                DiscardSource::Index,
                "STALE_HUNK",
            ),
            (
                vec![hunk.clone()],
                Some(vec!["0".repeat(64)]),
                DiscardSource::Index,
                "STALE_LINE",
            ),
            (vec![], None, DiscardSource::Index, "INVALID_REQUEST"),
        ];
        for (ids, lines, source, code) in cases {
            let error = discard_selection(&repo, b"file", ids, lines, source).unwrap_err();
            assert_eq!(serde_json::to_value(&error).unwrap()["code"], code);
            // No refusal may touch the working file.
            assert_eq!(
                fs::read(tmp.path().join("file")).unwrap(),
                edited.as_bytes()
            );
        }
        assert!(!repo.path().join("index.lock").exists());
    }
}
