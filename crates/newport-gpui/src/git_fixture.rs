//! Git replies for tests, built from the protocol's own types.
use newport_client::git_diff_page::DiffPage;
use newport_protocol::git::{
    Binary, DiffMetadata, FileFragment, HunkFragment, LineFragment, ObjectId, Page, Path, Side,
};

pub(crate) fn object(hex: &str) -> ObjectId {
    ObjectId {
        algorithm: "sha1".into(),
        hex: hex.into(),
    }
}

/// Metadata of a working-tree comparison of one file.
pub(crate) fn working_metadata(snapshot: &str, entry: &str) -> DiffMetadata {
    DiffMetadata {
        read_only: false,
        commit_oid: None,
        parent_oid: None,
        parent_index: None,
        parents: None,
        source_snapshot: Some(snapshot.into()),
        entry_id: Some(entry.into()),
        side: Some(Side::IndexToWorktree),
        context_lines: Some(3),
        selected_path: None,
        truncated: None,
        total_files: Some(1),
        total_units: Some(1),
        has_omissions: Some(false),
    }
}

/// Metadata of a commit's comparison of `path` with its first parent.
pub(crate) fn commit_metadata(commit: &str, path: &[u8]) -> DiffMetadata {
    DiffMetadata {
        read_only: true,
        commit_oid: Some(object(commit)),
        parents: Some(vec![]),
        selected_path: Some(Path::new(path)),
        source_snapshot: None,
        entry_id: None,
        side: None,
        total_files: None,
        ..working_metadata("", "")
    }
}

pub(crate) fn diff_page(
    snapshot: &str,
    metadata: DiffMetadata,
    entries: Vec<FileFragment>,
    next_cursor: Option<&str>,
) -> DiffPage {
    Page {
        snapshot: snapshot.into(),
        entries,
        next_cursor: next_cursor.map(Into::into),
        metadata,
    }
}

/// A modified file whose old and new paths are `path`.
pub(crate) fn file(index: u64, path: &str, hunks: Vec<HunkFragment>) -> FileFragment {
    FileFragment {
        file_index: index,
        old_path: Path::new(path.as_bytes()),
        new_path: Path::new(path.as_bytes()),
        old_oid: None,
        new_oid: None,
        old_mode: 0o100644,
        new_mode: 0o100644,
        status: "Modified".into(),
        binary: false,
        additions: 0,
        deletions: 0,
        omission_reason: None,
        hunks,
    }
}

pub(crate) fn hunk(index: u64, id: Option<&str>, lines: Vec<LineFragment>) -> HunkFragment {
    HunkFragment {
        id: id.map(Into::into),
        index,
        old_start: 1,
        old_lines: 1,
        new_start: 1,
        new_lines: 1,
        total_lines: lines.len() as u64,
        lines,
    }
}

/// A complete line piece.
pub(crate) fn line(index: u64, origin: &str, id: Option<&str>, bytes: &[u8]) -> LineFragment {
    LineFragment {
        line_index: index,
        byte_offset: 0,
        line_complete: true,
        origin: origin.into(),
        old_line: None,
        new_line: Some(index + 1),
        content_bytes_b64: Binary(bytes.to_vec()),
        id: id.map(Into::into),
    }
}

/// A writable, non-bare repository rooted at `/repo`, on `refs/heads/main`.
pub(crate) fn repository(id: &str) -> newport_protocol::git::Repository {
    use newport_protocol::git::{Head, Repository, RepositoryCapabilities};
    Repository {
        repo_id: id.into(),
        common_repo_id: id.into(),
        root: Path::new(b"/repo"),
        bare: false,
        object_format: "sha1".into(),
        head: Head {
            oid: Some(object(&"a".repeat(40))),
            name: Some(Path::new(b"refs/heads/main")),
            detached: false,
            unborn: false,
        },
        operation_state: "none".into(),
        integration: None,
        capabilities: RepositoryCapabilities {
            read_only: false,
            working_tree: true,
        },
    }
}

/// A read-only repository without a working tree or a named HEAD.
pub(crate) fn read_only_repository(id: &str) -> newport_protocol::git::Repository {
    let mut repo = repository(id);
    repo.capabilities.read_only = true;
    repo.capabilities.working_tree = false;
    repo.head.name = None;
    repo.head.oid = None;
    repo
}

/// An unstaged modification of `path`.
pub(crate) fn status_entry(id: &str, path: &str) -> newport_protocol::git::StatusEntry {
    newport_protocol::git::StatusEntry {
        entry_id: id.into(),
        path: Path::new(path.as_bytes()),
        old_path: None,
        flags: 0,
        staged: false,
        unstaged: true,
        untracked: false,
        conflicted: false,
        conflict: None,
    }
}

/// A clean summary on `refs/heads/main` at `a…a`.
pub(crate) fn status_summary() -> newport_protocol::git::StatusSummary {
    newport_protocol::git::StatusSummary {
        head: repository("fixture").head,
        operation_state: "none".into(),
        integration: None,
        ahead: None,
        behind: None,
        basis: "local".into(),
        upstream_ref: None,
        upstream: None,
        truncated: false,
        total_entries: 0,
        group_counts: newport_protocol::git::GroupCounts {
            staged: 0,
            unstaged: 0,
            untracked: 0,
            conflicted: 0,
        },
        matched_entries: None,
    }
}

pub(crate) fn status_page(
    snapshot: &str,
    entries: Vec<newport_protocol::git::StatusEntry>,
    next_cursor: Option<&str>,
) -> crate::git_listing::StatusPage {
    Page {
        snapshot: snapshot.into(),
        entries,
        next_cursor: next_cursor.map(Into::into),
        metadata: status_summary(),
    }
}

/// A commit by "Author" with no parents.
pub(crate) fn commit(hex: &str, message: &str) -> newport_protocol::git::Commit {
    newport_protocol::git::Commit {
        oid: object(hex),
        parents: vec![],
        message: Path::new(message.as_bytes()),
        message_truncated: false,
        author: newport_protocol::git::Author {
            name: "Author".into(),
            email: "author@example.test".into(),
        },
        time: 1,
        offset_minutes: 0,
    }
}

pub(crate) fn history_page(
    snapshot: &str,
    entries: Vec<newport_protocol::git::Commit>,
    next_cursor: Option<&str>,
) -> crate::git_listing::Listing {
    crate::git_listing::Listing::History(Page {
        snapshot: snapshot.into(),
        entries,
        next_cursor: next_cursor.map(Into::into),
        metadata: newport_protocol::git::HistoryMetadata {
            resolved_revision: None,
            truncated: None,
        },
    })
}

/// A local branch `name` at `hex`, not checked out.
pub(crate) fn branch(name: &str, hex: &str) -> newport_protocol::git::Branch {
    newport_protocol::git::Branch {
        name: Path::new(name.as_bytes()),
        reference: Path::new(format!("refs/heads/{name}").as_bytes()),
        oid: object(hex),
        remote: false,
        current: false,
        upstream: None,
        tracking: None,
    }
}

pub(crate) fn stash(index: u64, oid: &str, message: &str) -> newport_protocol::git::Stash {
    newport_protocol::git::Stash {
        index,
        oid: oid.into(),
        previous_oid: String::new(),
        message: message.into(),
        message_truncated: false,
        time: 1,
    }
}

/// A lightweight tag `name` on commit `hex`.
pub(crate) fn tag(name: &str, hex: &str) -> newport_protocol::git::Tag {
    let oid = newport_protocol::git::TagObjectId {
        format: "sha1".into(),
        hex: hex.into(),
    };
    newport_protocol::git::Tag {
        name: Some(Path::new(name.as_bytes())),
        reference: Some(Path::new(format!("refs/tags/{name}").as_bytes())),
        symbolic_target: None,
        oid: oid.clone(),
        annotated: false,
        details_omitted: false,
        object_type: Some("commit".into()),
        target_oid: Some(oid.clone()),
        peeled_oid: Some(oid),
        peeled_type: Some("commit".into()),
        message: None,
        message_truncated: None,
        tagger: None,
    }
}

pub(crate) fn remote_names(
    snapshot: &str,
    names: &[&str],
    next_cursor: Option<&str>,
    total: u64,
) -> crate::git_listing::Listing {
    crate::git_listing::Listing::Remotes(Page {
        snapshot: snapshot.into(),
        entries: names
            .iter()
            .map(|name| newport_protocol::git::RemoteName {
                name: (*name).into(),
            })
            .collect(),
        next_cursor: next_cursor.map(Into::into),
        metadata: newport_protocol::git::TotalEntries {
            total_entries: total,
        },
    })
}

/// An available linked worktree at `path`, not the current checkout.
pub(crate) fn worktree(name: Option<&str>, path: &str) -> newport_protocol::git::Worktree {
    newport_protocol::git::Worktree {
        name: name.map(|name| Path::new(name.as_bytes())),
        kind: "linked".into(),
        path: Path::new(path.as_bytes()),
        git_dir: Path::new(format!("{path}/.git").as_bytes()),
        current: false,
        state: "available".into(),
        head: newport_protocol::git::Head {
            oid: None,
            name: None,
            detached: false,
            unborn: false,
        },
        locked: false,
        lock_reason: None,
        prunable: false,
    }
}

/// A worktree listing page holding `entries`.
pub(crate) fn worktree_page(
    snapshot: &str,
    entries: Vec<newport_protocol::git::Worktree>,
    next_cursor: Option<&str>,
) -> newport_client::git_worktree::WorktreePage {
    let total = entries.len() as u64;
    Page {
        snapshot: snapshot.into(),
        entries,
        next_cursor: next_cursor.map(Into::into),
        metadata: newport_protocol::git::WorktreeMetadata {
            list_token: "worktrees".into(),
            total_entries: total,
            matching_entries: total,
            current: None,
            main: None,
        },
    }
}
