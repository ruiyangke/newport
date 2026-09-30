//! Server-derived hunk selection. Patch bytes never cross the write API.
use super::protocol::{Error, HunkSelection};
use git2::{DiffOptions, Index, Patch, Repository};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, ffi::OsStr, os::unix::ffi::OsStrExt, path::Path};

fn engine(_: git2::Error) -> Error {
    Error::new(
        "HUNK_APPLY_FAILED",
        "The selected hunks could not be applied. Refresh the diff; no index change was published.",
    )
}
fn unsupported() -> Error {
    Error::new("UNSUPPORTED_HUNKS", "Partial staging currently requires an added, deleted or modified regular text file without conflicts, renames, or mode changes. Use whole-file staging for this change.")
}

/// Bind a line to its hunk identifier (which already covers paths, modes and
/// ranges), its position and its exact bytes. Only changed lines are addressable.
pub(super) fn line_id(hunk: &str, ordinal: usize, line: &git2::DiffLine<'_>) -> Option<String> {
    if !matches!(line.origin(), '+' | '-') {
        return None;
    }
    let mut hash = Sha256::new();
    hash.update(b"newport-git-line-v1\0");
    hash.update((hunk.len() as u64).to_be_bytes());
    hash.update(hunk.as_bytes());
    hash.update((ordinal as u64).to_be_bytes());
    hash.update([line.origin() as u8]);
    hash.update((line.content().len() as u64).to_be_bytes());
    hash.update(line.content());
    Some(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Split into lines that keep their terminators, so concatenation is lossless
/// for files with or without a final newline.
pub(super) fn split_lines(content: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, byte) in content.iter().enumerate() {
        if *byte == b'\n' {
            out.push(&content[start..=i]);
            start = i + 1;
        }
    }
    if start < content.len() {
        out.push(&content[start..]);
    }
    out
}

pub(super) fn blob_bytes(repo: &Repository, oid: git2::Oid) -> Result<Vec<u8>, Error> {
    if oid.is_zero() {
        return Ok(Vec::new());
    }
    Ok(repo.find_blob(oid).map_err(engine)?.content().to_vec())
}

/// Rebuild file content from the old side by applying only the selected changed
/// lines. Context and deletion bytes are checked against the old content, so a
/// diff that does not describe `old` fails instead of publishing wrong bytes.
pub(super) fn reconstruct(
    old: &[u8],
    patch: &Patch<'_>,
    apply: &Positions,
) -> Result<Vec<u8>, Error> {
    let source = split_lines(old);
    let mut out: Vec<u8> = Vec::with_capacity(old.len());
    let mut cursor = 0usize;
    let mismatch = || engine(git2::Error::from_str("diff does not describe this content"));
    for h in 0..patch.num_hunks() {
        let (hunk, lines) = patch.hunk(h).map_err(engine)?;
        // Ranges are 1-based, except that a hunk touching no old line records the
        // line it is inserted after, which is already the 0-based position.
        let start = if hunk.old_lines() == 0 {
            hunk.old_start() as usize
        } else {
            (hunk.old_start() as usize).saturating_sub(1)
        };
        if start < cursor || start > source.len() {
            return Err(mismatch());
        }
        for line in &source[cursor..start] {
            out.extend_from_slice(line);
        }
        cursor = start;
        for l in 0..lines {
            let line = patch.line_in_hunk(h, l).map_err(engine)?;
            match line.origin() {
                ' ' | '-' => {
                    let existing = source.get(cursor).ok_or_else(mismatch)?;
                    if *existing != line.content() {
                        return Err(mismatch());
                    }
                    if line.origin() == ' ' || !apply.contains(&(h, l)) {
                        out.extend_from_slice(existing);
                    }
                    cursor += 1;
                }
                '+' => {
                    if apply.contains(&(h, l)) {
                        out.extend_from_slice(line.content());
                    }
                }
                // End-of-file newline markers carry no file content.
                '=' | '>' | '<' => {}
                _ => return Err(unsupported()),
            }
        }
    }
    for line in &source[cursor..] {
        out.extend_from_slice(line);
    }
    Ok(out)
}

/// Bind the identifier to raw paths, modes, ranges and exact bytes, including EOF
/// markers. Display strings are deliberately excluded (they can be lossy).
pub(super) fn id(patch: &Patch<'_>, ordinal: usize) -> Result<String, Error> {
    let mut hash = Sha256::new();
    hash.update(b"newport-git-hunk-v1\0");
    let delta = patch.delta();
    for file in [delta.old_file(), delta.new_file()] {
        let path = file.path_bytes().unwrap_or_default();
        hash.update((path.len() as u64).to_be_bytes());
        hash.update(path);
        hash.update(i32::from(file.mode()).to_be_bytes());
    }
    let (hunk, lines) = patch.hunk(ordinal).map_err(engine)?;
    for value in [
        hunk.old_start(),
        hunk.old_lines(),
        hunk.new_start(),
        hunk.new_lines(),
    ] {
        hash.update(value.to_be_bytes());
    }
    for i in 0..lines {
        let line = patch.line_in_hunk(ordinal, i).map_err(engine)?;
        hash.update([line.origin() as u8]);
        hash.update((line.content().len() as u64).to_be_bytes());
        hash.update(line.content());
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Positions of changed diff lines as (hunk ordinal, line ordinal).
pub(super) type Positions = HashSet<(usize, usize)>;

/// Identify every hunk of the comparison and mark the requested ones, refusing
/// identifiers that the regenerated comparison no longer contains.
pub(super) fn chosen_hunks(
    patch: &Patch<'_>,
    ids: &[String],
) -> Result<(Vec<String>, Vec<bool>), Error> {
    let selected: HashSet<&str> = ids.iter().map(String::as_str).collect();
    if selected.len() != ids.len() {
        return Err(Error::invalid("Hunk identifiers must be unique."));
    }
    let mut hunk_ids = Vec::new();
    let mut chosen = Vec::new();
    for i in 0..patch.num_hunks() {
        let hunk = id(patch, i)?;
        chosen.push(selected.contains(hunk.as_str()));
        hunk_ids.push(hunk);
    }
    if chosen.iter().filter(|v| **v).count() != selected.len() {
        return Err(Error::new(
            "STALE_HUNK",
            "A selected hunk is not in this diff. Refresh changes before selecting it again.",
        ));
    }
    Ok((hunk_ids, chosen))
}

/// Map the requested hunks and lines onto diff positions, returning every
/// addressable change and the subset this request selects.
pub(super) fn resolve(
    patch: &Patch<'_>,
    selection: &HunkSelection,
    chosen: &[bool],
    hunk_ids: &[String],
) -> Result<(Positions, Positions), Error> {
    let requested = match &selection.lines {
        Some(values) => {
            if values.is_empty() || values.len() > 100_000 {
                return Err(Error::invalid("Select between 1 and 100000 lines."));
            }
            let unique: HashSet<&str> = values.iter().map(String::as_str).collect();
            if unique.len() != values.len() {
                return Err(Error::invalid("Line identifiers must be unique."));
            }
            Some(unique)
        }
        None => None,
    };
    let (mut all, mut picked): (Positions, Positions) = Default::default();
    let mut matched = 0usize;
    let mut per_hunk = vec![0usize; chosen.len()];
    for h in 0..patch.num_hunks() {
        let (_, lines) = patch.hunk(h).map_err(engine)?;
        for l in 0..lines {
            let line = patch.line_in_hunk(h, l).map_err(engine)?;
            let Some(line) = line_id(&hunk_ids[h], l, &line) else {
                continue;
            };
            all.insert((h, l));
            let take = match &requested {
                Some(set) => {
                    if set.contains(line.as_str()) {
                        matched += 1;
                        if !chosen[h] {
                            return Err(Error::invalid(
                                "Selected lines must belong to a selected hunk.",
                            ));
                        }
                        true
                    } else {
                        false
                    }
                }
                None => chosen[h],
            };
            if take {
                picked.insert((h, l));
                per_hunk[h] += 1;
            }
        }
    }
    if let Some(set) = &requested {
        if matched != set.len() {
            return Err(Error::new(
                "STALE_LINE",
                "A selected line is not in this diff. Refresh changes before selecting it again.",
            ));
        }
        if chosen.iter().zip(&per_hunk).any(|(c, n)| *c && *n == 0) {
            return Err(Error::invalid(
                "Each selected hunk must include at least one selected line.",
            ));
        }
    }
    Ok((all, picked))
}

/// Resolve the selected lines, rebuild the file from the old side and publish it.
/// Nothing is written unless replaying every change reproduces the known new
/// side exactly, so a diff that no longer describes the file fails closed.
#[allow(clippy::too_many_arguments)]
fn publish(
    repo: &Repository,
    index: &mut Index,
    path_bytes: &[u8],
    unstage: bool,
    status: git2::Delta,
    patch: &Patch<'_>,
    selection: &HunkSelection,
    chosen: &[bool],
    hunk_ids: &[String],
) -> Result<(), Error> {
    let (all, picked) = resolve(patch, selection, chosen, hunk_ids)?;
    let old = blob_bytes(repo, patch.delta().old_file().id())?;
    let new = if unstage {
        blob_bytes(repo, patch.delta().new_file().id())?
    } else if status == git2::Delta::Deleted {
        Vec::new()
    } else {
        let root = repo.workdir().ok_or_else(unsupported)?;
        std::fs::read(root.join(Path::new(OsStr::from_bytes(path_bytes))))
            .map_err(|_| unsupported())?
    };
    if reconstruct(&old, patch, &all)? != new {
        return Err(unsupported());
    }
    // Unstaging reverts the selected lines, so the index keeps everything else.
    let effective: Positions = if unstage {
        all.difference(&picked).copied().collect()
    } else {
        picked
    };
    let content = reconstruct(&old, patch, &effective)?;
    let path = Path::new(OsStr::from_bytes(path_bytes));
    let absent = status
        == if unstage {
            git2::Delta::Added
        } else {
            git2::Delta::Deleted
        };
    if content.is_empty() && absent {
        return index.remove_path(path).map_err(engine);
    }
    let mode = if status == git2::Delta::Added {
        patch.delta().new_file().mode()
    } else {
        patch.delta().old_file().mode()
    };
    let id = repo.blob(&content).map_err(engine)?;
    // Stat fields stay zero so Git re-examines the working file, matching the
    // entries the whole-hunk path publishes from an applied tree.
    let entry = git2::IndexEntry {
        ctime: git2::IndexTime::new(0, 0),
        mtime: git2::IndexTime::new(0, 0),
        dev: 0,
        ino: 0,
        mode: i32::from(mode) as u32,
        uid: 0,
        gid: 0,
        file_size: 0,
        id,
        flags: 0,
        flags_extended: 0,
        path: path_bytes.to_vec(),
    };
    index.add(&entry).map_err(engine)
}

pub(super) fn apply(
    repo: &Repository,
    index: &mut Index,
    paths: &[Vec<u8>],
    unstage: bool,
    selection: &HunkSelection,
) -> Result<(), Error> {
    if paths.len() != 1
        || selection.ids.is_empty()
        || selection.ids.len() > 10_000
        || selection.context_lines > 100
    {
        return Err(Error::invalid(
            "Select hunks from exactly one file with contextLines at most 100.",
        ));
    }
    if index.has_conflicts() || repo.state() != git2::RepositoryState::Clean {
        return Err(unsupported());
    }
    let mut options = DiffOptions::new();
    options
        .context_lines(selection.context_lines)
        .disable_pathspec_match(true)
        .pathspec(&paths[0])
        .max_size(2 * 1024 * 1024);
    if !unstage {
        // A file that is not in the index yet only appears as an untracked
        // delta, and its content is needed to address its lines.
        options.include_untracked(true).show_untracked_content(true);
    }
    let head = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let diff = if unstage {
        repo.diff_tree_to_index(head.as_ref(), Some(index), Some(&mut options))
    } else {
        repo.diff_index_to_workdir(Some(index), Some(&mut options))
    }
    .map_err(engine)?;
    if diff.deltas().len() != 1 {
        return Err(unsupported());
    }
    let patch = Patch::from_diff(&diff, 0)
        .map_err(engine)?
        .ok_or_else(unsupported)?;
    let delta = patch.delta();
    // Working-tree diffs report a file missing from the index as untracked.
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
    let (hunk_ids, chosen) = chosen_hunks(&patch, &selection.ids)?;
    // Rebuilding content covers line selection and the added/deleted cases that
    // libgit2's whole-hunk apply cannot express.
    if selection.lines.is_some() || status != git2::Delta::Modified {
        return publish(
            repo, index, &paths[0], unstage, status, &patch, selection, &chosen, &hunk_ids,
        );
    }
    let mut ranges = Vec::new();
    for i in 0..patch.num_hunks() {
        let (h, _) = patch.hunk(i).map_err(engine)?;
        ranges.push((h.old_start(), h.old_lines(), h.new_start(), h.new_lines()));
    }
    // Undo staged changes by applying the reverse comparison. Verify the reverse
    // hunk layout before using ordinals; never assume symmetry from line numbers.
    let reversed;
    let to_apply = if unstage {
        options.reverse(true);
        reversed = repo
            .diff_tree_to_index(head.as_ref(), Some(index), Some(&mut options))
            .map_err(engine)?;
        let reverse_patch = Patch::from_diff(&reversed, 0)
            .map_err(engine)?
            .ok_or_else(unsupported)?;
        if reverse_patch.num_hunks() != ranges.len() {
            return Err(unsupported());
        }
        for (i, &(old_start, old_lines, new_start, new_lines)) in ranges.iter().enumerate() {
            let (h, _) = reverse_patch.hunk(i).map_err(engine)?;
            if (h.old_start(), h.old_lines(), h.new_start(), h.new_lines())
                != (new_start, new_lines, old_start, old_lines)
            {
                return Err(unsupported());
            }
        }
        &reversed
    } else {
        &diff
    };
    let tree = repo
        .find_tree(index.write_tree_to(repo).map_err(engine)?)
        .map_err(engine)?;
    let mut ordinal = 0;
    let mut options = git2::ApplyOptions::new();
    options.hunk_callback(|_| {
        let apply = chosen.get(ordinal).copied().unwrap_or(false);
        ordinal += 1;
        apply
    });
    let result = repo
        .apply_to_tree(&tree, to_apply, Some(&mut options))
        .map_err(engine)?;
    drop(options);
    if ordinal != chosen.len() {
        return Err(engine(git2::Error::from_str("hunk count changed")));
    }
    let path = Path::new(OsStr::from_bytes(&paths[0]));
    let entry = result.get_path(path, 0).ok_or_else(unsupported)?;
    // Preserve all other entries and index flags; publish through the caller's
    // private index and final snapshot revalidation.
    index.add(&entry).map_err(engine)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{operations, protocol::Action, repository};
    use std::fs;

    fn fixture() -> (tempfile::TempDir, Repository, String, String) {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        let before: String = (0..40).map(|i| format!("line {i}\n")).collect();
        let after = before
            .replace("line 2\n", "changed 2\nextra\n")
            .replace("line 32\n", "changed 32\n");
        fs::write(temp.path().join("file"), &before).unwrap();
        fs::write(temp.path().join("other"), "original\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("file")).unwrap();
        index.add_path(Path::new("other")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let author = git2::Signature::now("Test", "test@example.test").unwrap();
        repo.commit(Some("HEAD"), &author, &author, "initial", &tree, &[])
            .unwrap();
        drop(tree);
        fs::write(temp.path().join("file"), &after).unwrap();
        fs::write(temp.path().join("other"), "staged unrelated\n").unwrap();
        index.add_path(Path::new("other")).unwrap();
        index.write().unwrap();
        (temp, repo, before, after)
    }
    fn ids(repo: &Repository, unstage: bool, context: u32) -> Vec<String> {
        let mut options = DiffOptions::new();
        options.pathspec("file").context_lines(context);
        let tree = repo.head().unwrap().peel_to_tree().unwrap();
        let diff = if unstage {
            repo.diff_tree_to_index(Some(&tree), None, Some(&mut options))
        } else {
            repo.diff_index_to_workdir(None, Some(&mut options))
        }
        .unwrap();
        let patch = Patch::from_diff(&diff, 0).unwrap().unwrap();
        (0..patch.num_hunks())
            .map(|i| id(&patch, i).unwrap())
            .collect()
    }
    fn action(unstage: bool, ids: Vec<String>, context_lines: u32) -> Action {
        let hunks = Some(HunkSelection {
            ids,
            lines: None,
            context_lines,
        });
        if unstage {
            Action::Unstage {
                entry_ids: vec!["entry".into()],
                hunks,
            }
        } else {
            Action::Stage {
                entry_ids: vec!["entry".into()],
                hunks,
            }
        }
    }
    fn content(repo: &Repository, path: &str) -> Vec<u8> {
        let entry = repo.index().unwrap().get_path(Path::new(path), 0).unwrap();
        repo.find_blob(entry.id).unwrap().content().to_vec()
    }
    #[test]
    fn stage_and_unstage_preserve_other_hunks_working_files_and_index_entries() {
        for context in [0, 3, 10] {
            let (temp, repo, before, after) = fixture();
            let ids = ids(&repo, false, context);
            assert_eq!(ids.len(), 2);
            let expected = repository::fingerprint(&repo).unwrap();
            operations::apply(
                &repo,
                &action(false, vec![ids[1].clone()], context),
                &[b"file".to_vec()],
                &expected,
            )
            .unwrap();
            let repo = Repository::open(temp.path()).unwrap();
            let partial = before.replace("line 32\n", "changed 32\n");
            assert_eq!(content(&repo, "file"), partial.as_bytes());
            assert_eq!(content(&repo, "other"), b"staged unrelated\n");
            assert_eq!(
                fs::read(temp.path().join("file")).unwrap(),
                after.as_bytes()
            );
            let staged = self::ids(&repo, true, context);
            operations::apply(
                &repo,
                &action(true, staged, context),
                &[b"file".to_vec()],
                &repository::fingerprint(&repo).unwrap(),
            )
            .unwrap();
            let repo = Repository::open(temp.path()).unwrap();
            assert_eq!(content(&repo, "file"), before.as_bytes());
            assert_eq!(content(&repo, "other"), b"staged unrelated\n");
            assert_eq!(
                fs::read(temp.path().join("file")).unwrap(),
                after.as_bytes()
            );
            assert!(!repo.path().join("index.lock").exists());
        }
    }
    #[test]
    fn unstage_one_hunk_retains_the_other_staged_hunk() {
        let (temp, repo, before, after) = fixture();
        let selected = action(false, ids(&repo, false, 3), 3);
        operations::apply(
            &repo,
            &selected,
            &[b"file".to_vec()],
            &repository::fingerprint(&repo).unwrap(),
        )
        .unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        assert_eq!(content(&repo, "file"), after.as_bytes());
        let staged = ids(&repo, true, 3);
        assert_eq!(staged.len(), 2);
        operations::apply(
            &repo,
            &action(true, vec![staged[0].clone()], 3),
            &[b"file".to_vec()],
            &repository::fingerprint(&repo).unwrap(),
        )
        .unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        assert_eq!(
            content(&repo, "file"),
            before.replace("line 32\n", "changed 32\n").as_bytes()
        );
        assert_eq!(
            fs::read(temp.path().join("file")).unwrap(),
            after.as_bytes()
        );
    }

    #[test]
    fn invalid_selection_leaves_index_bytes_unchanged() {
        let (temp, repo, _, _) = fixture();
        let ids = ids(&repo, false, 3);
        let original = fs::read(repo.path().join("index")).unwrap();
        for (selection, code) in [
            (action(false, vec![], 3), "INVALID_REQUEST"),
            (
                action(false, vec![ids[0].clone(), ids[0].clone()], 3),
                "INVALID_REQUEST",
            ),
            (action(false, vec![ids[0].clone()], 101), "INVALID_REQUEST"),
            (action(false, vec!["unknown".into()], 3), "STALE_HUNK"),
            (action(false, vec![ids[0].clone()], 0), "STALE_HUNK"),
        ] {
            let repo = Repository::open(temp.path()).unwrap();
            let expected = repository::fingerprint(&repo).unwrap();
            assert_eq!(
                operations::apply(&repo, &selection, &[b"file".to_vec()], &expected)
                    .unwrap_err()
                    .code,
                code
            );
            assert_eq!(fs::read(repo.path().join("index")).unwrap(), original);
            assert!(!repo.path().join("index.lock").exists());
        }
    }
    #[test]
    fn stale_file_and_external_lock_are_never_overwritten() {
        let (temp, repo, _, _) = fixture();
        let selection = action(false, ids(&repo, false, 3), 3);
        let expected = repository::fingerprint(&repo).unwrap();
        let original = fs::read(repo.path().join("index")).unwrap();
        fs::write(repo.path().join("index.lock"), "external").unwrap();
        assert_eq!(
            operations::apply(&repo, &selection, &[b"file".to_vec()], &expected)
                .unwrap_err()
                .code,
            "REPOSITORY_BUSY"
        );
        assert_eq!(
            fs::read(repo.path().join("index.lock")).unwrap(),
            b"external"
        );
        fs::remove_file(repo.path().join("index.lock")).unwrap();
        fs::write(temp.path().join("file"), "new edits\n").unwrap();
        assert_eq!(
            operations::apply(&repo, &selection, &[b"file".to_vec()], &expected)
                .unwrap_err()
                .code,
            "STALE_SNAPSHOT"
        );
        assert_eq!(fs::read(repo.path().join("index")).unwrap(), original);
    }
    #[test]
    fn unsupported_changes_and_filters_preserve_index() {
        for variant in ["binary", "mode", "symlink", "filter"] {
            let (temp, repo, _, _) = fixture();
            let selection = action(false, ids(&repo, false, 3), 3);
            let path = temp.path().join("file");
            match variant {
                "binary" => fs::write(&path, b"binary\0bytes").unwrap(),
                "mode" => {
                    use std::os::unix::fs::PermissionsExt;
                    repo.config()
                        .unwrap()
                        .set_bool("core.filemode", true)
                        .unwrap();
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
                }
                "symlink" => {
                    fs::remove_file(&path).unwrap();
                    std::os::unix::fs::symlink("other", &path).unwrap();
                }
                "filter" => {
                    fs::write(temp.path().join(".gitattributes"), "file filter=custom\n").unwrap()
                }
                _ => unreachable!(),
            }
            let original = fs::read(repo.path().join("index")).unwrap();
            let expected = repository::fingerprint(&repo).unwrap();
            let error =
                operations::apply(&repo, &selection, &[b"file".to_vec()], &expected).unwrap_err();
            assert!(
                matches!(
                    error.code.as_str(),
                    "UNSUPPORTED_HUNKS" | "UNSUPPORTED_FILTER"
                ),
                "{variant}: {error}"
            );
            assert_eq!(fs::read(repo.path().join("index")).unwrap(), original);
        }
    }

    #[test]
    fn missing_final_newline_roundtrips_exact_bytes() {
        let (temp, _repo, before, _) = fixture();
        let after = before.trim_end().replace("line 39", "last line changed");
        fs::write(temp.path().join("file"), &after).unwrap();
        for unstage in [false, true] {
            let repo = Repository::open(temp.path()).unwrap();
            let selection = action(unstage, ids(&repo, unstage, 3), 3);
            operations::apply(
                &repo,
                &selection,
                &[b"file".to_vec()],
                &repository::fingerprint(&repo).unwrap(),
            )
            .unwrap();
            let repo = Repository::open(temp.path()).unwrap();
            assert_eq!(
                content(&repo, "file"),
                if unstage {
                    before.as_bytes()
                } else {
                    after.as_bytes()
                }
            );
            assert_eq!(
                fs::read(temp.path().join("file")).unwrap(),
                after.as_bytes()
            );
        }
    }

    #[test]
    fn legacy_whole_file_payload_is_unchanged() {
        let action: Action =
            serde_json::from_value(serde_json::json!({"kind":"stage","entryIds":["one"]})).unwrap();
        assert_eq!(
            serde_json::to_value(action).unwrap(),
            serde_json::json!({"kind":"stage","entryIds":["one"]})
        );
    }

    // ---- line-level selection ----

    /// Every addressable line of the single-file comparison, as
    /// (hunk id, line id, origin, content).
    fn lines_of(
        repo: &Repository,
        unstage: bool,
        context: u32,
        path: &str,
    ) -> Vec<(String, String, char, String)> {
        let mut options = DiffOptions::new();
        options.pathspec(path).context_lines(context);
        if !unstage {
            options.include_untracked(true).show_untracked_content(true);
        }
        let tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
        let diff = if unstage {
            repo.diff_tree_to_index(tree.as_ref(), None, Some(&mut options))
        } else {
            repo.diff_index_to_workdir(None, Some(&mut options))
        }
        .unwrap();
        let patch = Patch::from_diff(&diff, 0).unwrap().unwrap();
        let mut out = Vec::new();
        for h in 0..patch.num_hunks() {
            let hunk = id(&patch, h).unwrap();
            let (_, count) = patch.hunk(h).unwrap();
            for l in 0..count {
                let line = patch.line_in_hunk(h, l).unwrap();
                if let Some(line_id) = line_id(&hunk, l, &line) {
                    out.push((
                        hunk.clone(),
                        line_id,
                        line.origin(),
                        String::from_utf8_lossy(line.content()).into_owned(),
                    ));
                }
            }
        }
        out
    }
    fn line_action(unstage: bool, ids: Vec<String>, lines: Vec<String>, context: u32) -> Action {
        let hunks = Some(HunkSelection {
            ids,
            lines: Some(lines),
            context_lines: context,
        });
        let entry_ids = vec!["entry".into()];
        if unstage {
            Action::Unstage { entry_ids, hunks }
        } else {
            Action::Stage { entry_ids, hunks }
        }
    }
    fn commit_all(temp: &tempfile::TempDir, repo: &Repository, files: &[(&str, &str)]) {
        let mut index = repo.index().unwrap();
        for (name, body) in files {
            fs::write(temp.path().join(name), body).unwrap();
            index.add_path(Path::new(name)).unwrap();
        }
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let author = git2::Signature::now("Test", "test@example.test").unwrap();
        repo.commit(Some("HEAD"), &author, &author, "initial", &tree, &[])
            .unwrap();
    }
    /// A hunk holding two adjacent additions: a debug line and a real change.
    fn adjacent() -> (tempfile::TempDir, Repository) {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        commit_all(
            &temp,
            &repo,
            &[("file", "a\nb\nc\n"), ("other", "original\n")],
        );
        fs::write(temp.path().join("file"), "a\nDEBUG\nkeep\nb\nc\n").unwrap();
        fs::write(temp.path().join("other"), "staged unrelated\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("other")).unwrap();
        index.write().unwrap();
        (temp, repo)
    }
    fn run(repo: &Repository, action: &Action, path: &[u8]) -> Result<(), Error> {
        let expected = repository::fingerprint(repo).unwrap();
        operations::apply(repo, action, &[path.to_vec()], &expected).map(|_| ())
    }

    #[test]
    fn stages_one_line_of_a_hunk_and_leaves_the_rest_unstaged() {
        for context in [0, 3] {
            let (temp, repo) = adjacent();
            let lines = lines_of(&repo, false, context, "file");
            assert_eq!(lines.len(), 2, "expected two addressable additions");
            let keep = lines.iter().find(|l| l.3 == "keep\n").unwrap();
            run(
                &repo,
                &line_action(false, vec![keep.0.clone()], vec![keep.1.clone()], context),
                b"file",
            )
            .unwrap();
            let repo = Repository::open(temp.path()).unwrap();
            assert_eq!(content(&repo, "file"), b"a\nkeep\nb\nc\n");
            // The unselected debug line stays in the working file only.
            assert_eq!(
                fs::read(temp.path().join("file")).unwrap(),
                b"a\nDEBUG\nkeep\nb\nc\n"
            );
            assert_eq!(content(&repo, "other"), b"staged unrelated\n");
            assert!(!repo.path().join("index.lock").exists());
        }
    }

    #[test]
    fn unstages_one_line_and_retains_the_other_staged_line() {
        let (temp, repo) = adjacent();
        let all = lines_of(&repo, false, 3, "file");
        let hunk = all[0].0.clone();
        run(
            &repo,
            &line_action(
                false,
                vec![hunk],
                all.iter().map(|l| l.1.clone()).collect(),
                3,
            ),
            b"file",
        )
        .unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        assert_eq!(content(&repo, "file"), b"a\nDEBUG\nkeep\nb\nc\n");
        let staged = lines_of(&repo, true, 3, "file");
        let debug = staged.iter().find(|l| l.3 == "DEBUG\n").unwrap();
        run(
            &repo,
            &line_action(true, vec![debug.0.clone()], vec![debug.1.clone()], 3),
            b"file",
        )
        .unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        assert_eq!(content(&repo, "file"), b"a\nkeep\nb\nc\n");
        assert_eq!(
            fs::read(temp.path().join("file")).unwrap(),
            b"a\nDEBUG\nkeep\nb\nc\n"
        );
        assert_eq!(content(&repo, "other"), b"staged unrelated\n");
    }

    #[test]
    fn stages_selected_lines_of_a_new_file() {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        commit_all(&temp, &repo, &[("other", "original\n")]);
        fs::write(temp.path().join("fresh"), "1\n2\n3\n").unwrap();
        let lines = lines_of(&repo, false, 3, "fresh");
        assert_eq!(lines.len(), 3);
        let picked: Vec<String> = lines
            .iter()
            .filter(|l| l.3 != "2\n")
            .map(|l| l.1.clone())
            .collect();
        run(
            &repo,
            &line_action(false, vec![lines[0].0.clone()], picked, 3),
            b"fresh",
        )
        .unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        assert_eq!(content(&repo, "fresh"), b"1\n3\n");
        assert_eq!(fs::read(temp.path().join("fresh")).unwrap(), b"1\n2\n3\n");
    }

    #[test]
    fn stages_a_deletion_partially_then_completely() {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        commit_all(
            &temp,
            &repo,
            &[("file", "1\n2\n3\n"), ("other", "original\n")],
        );
        fs::remove_file(temp.path().join("file")).unwrap();
        let lines = lines_of(&repo, false, 3, "file");
        assert_eq!(lines.len(), 3);
        let one = lines.iter().find(|l| l.3 == "1\n").unwrap();
        run(
            &repo,
            &line_action(false, vec![one.0.clone()], vec![one.1.clone()], 3),
            b"file",
        )
        .unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        // A partial deletion keeps the entry with the remaining lines.
        assert_eq!(content(&repo, "file"), b"2\n3\n");
        let rest = lines_of(&repo, false, 3, "file");
        run(
            &repo,
            &line_action(
                false,
                vec![rest[0].0.clone()],
                rest.iter().map(|l| l.1.clone()).collect(),
                3,
            ),
            b"file",
        )
        .unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        // Selecting every remaining line stages the deletion itself.
        assert!(repo
            .index()
            .unwrap()
            .get_path(Path::new("file"), 0)
            .is_none());
        assert_eq!(content(&repo, "other"), b"original\n");
    }

    #[test]
    fn preserves_a_missing_final_newline() {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        commit_all(&temp, &repo, &[("file", "a\nb")]);
        fs::write(temp.path().join("file"), "a\nB").unwrap();
        let lines = lines_of(&repo, false, 3, "file");
        assert_eq!(lines.len(), 2, "expected a deletion and an addition");
        run(
            &repo,
            &line_action(
                false,
                vec![lines[0].0.clone()],
                lines.iter().map(|l| l.1.clone()).collect(),
                3,
            ),
            b"file",
        )
        .unwrap();
        let repo = Repository::open(temp.path()).unwrap();
        assert_eq!(content(&repo, "file"), b"a\nB");
    }

    #[test]
    fn refuses_stale_unknown_and_mismatched_line_selections() {
        let (temp, repo) = adjacent();
        let lines = lines_of(&repo, false, 3, "file");
        let hunk = lines[0].0.clone();
        let before = content(&repo, "file");
        let cases: Vec<(Vec<String>, Vec<String>, &str)> = vec![
            (vec![hunk.clone()], vec!["0".repeat(64)], "STALE_LINE"),
            (vec![hunk.clone()], vec![], "INVALID_REQUEST"),
            (
                vec![hunk.clone()],
                vec![lines[0].1.clone(), lines[0].1.clone()],
                "INVALID_REQUEST",
            ),
        ];
        for (ids, selected, code) in cases {
            let error = run(&repo, &line_action(false, ids, selected, 3), b"file").unwrap_err();
            assert_eq!(serde_json::to_value(&error).unwrap()["code"], code);
        }
        // Nothing was published by any refusal.
        let repo = Repository::open(temp.path()).unwrap();
        assert_eq!(content(&repo, "file"), before);
        assert!(!repo.path().join("index.lock").exists());
    }

    #[test]
    fn requires_every_selected_hunk_to_contribute_a_line() {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        let body: String = (0..40).map(|i| format!("line {i}\n")).collect();
        commit_all(&temp, &repo, &[("file", &body)]);
        let edited = body
            .replace("line 2\n", "changed 2\n")
            .replace("line 32\n", "changed 32\n");
        fs::write(temp.path().join("file"), &edited).unwrap();
        let lines = lines_of(&repo, false, 3, "file");
        let hunks: Vec<String> = {
            let mut seen: Vec<String> = Vec::new();
            for line in &lines {
                if !seen.contains(&line.0) {
                    seen.push(line.0.clone());
                }
            }
            seen
        };
        assert_eq!(hunks.len(), 2);
        let first: Vec<String> = lines
            .iter()
            .filter(|l| l.0 == hunks[0])
            .map(|l| l.1.clone())
            .collect();
        // Both hunks selected, but only one contributes lines.
        let error = run(
            &repo,
            &line_action(false, hunks.clone(), first.clone(), 3),
            b"file",
        )
        .unwrap_err();
        assert_eq!(
            serde_json::to_value(&error).unwrap()["code"],
            "INVALID_REQUEST"
        );
        // A line outside the selected hunks is refused too.
        let other: String = lines
            .iter()
            .find(|l| l.0 == hunks[1])
            .map(|l| l.1.clone())
            .unwrap();
        let error = run(
            &repo,
            &line_action(false, vec![hunks[0].clone()], vec![other], 3),
            b"file",
        )
        .unwrap_err();
        assert_eq!(
            serde_json::to_value(&error).unwrap()["code"],
            "INVALID_REQUEST"
        );
        assert!(repo
            .index()
            .unwrap()
            .get_path(Path::new("file"), 0)
            .is_some());
        assert_eq!(content(&repo, "file"), body.as_bytes());
    }
}
