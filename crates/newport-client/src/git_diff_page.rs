//! Check Git diff pages before publishing them to client state, and join
//! continuation pages at their file, hunk and line boundaries.
use newport_protocol::git::{self, FileFragment, HunkFragment, LineFragment, ObjectId, Path};

/// One page of a working-tree or commit diff.
pub type DiffPage = git::Page<FileFragment, git::DiffMetadata>;

type Result<T> = std::result::Result<T, String>;
fn invalid(field: &str) -> String {
    format!("Invalid Git diff response ({field}). Refresh to retry.")
}
/// Counts stay within the range every consumer represents exactly.
fn count(value: u64) -> Result<u64> {
    (value <= 9_007_199_254_740_991)
        .then_some(value)
        .ok_or_else(|| invalid("count"))
}
fn required<T>(value: Option<T>, field: &str) -> Result<T> {
    value.ok_or_else(|| invalid(field))
}
fn hex(value: &str, len: usize) -> Result<()> {
    if value.len() != len
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid("identifier"));
    }
    Ok(())
}
fn oid(value: &ObjectId) -> Result<()> {
    let len = match value.algorithm.as_str() {
        "sha1" => 40,
        "sha256" => 64,
        _ => return Err(invalid("object algorithm")),
    };
    hex(&value.hex, len)
}
fn bytes(value: &Path) -> Result<()> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    STANDARD
        .decode(&value.bytes_b64)
        .map_err(|_| invalid("base64"))?;
    Ok(())
}

#[derive(Clone, Copy)]
struct Piece<'a> {
    index: u64,
    offset: u64,
    complete: bool,
    origin: &'a str,
    old: Option<u64>,
    new: Option<u64>,
    length: u64,
}
impl<'a> Piece<'a> {
    fn read(value: &'a LineFragment, working: bool) -> Result<Self> {
        let length = value.content_bytes_b64.0.len() as u64;
        let complete = value.line_complete;
        let origin = value.origin.as_str();
        if length > 4096
            || (!complete && length != 4096)
            || !matches!(origin, " " | "+" | "-" | "=" | ">" | "<")
        {
            return Err(invalid("line piece"));
        }
        if working {
            if let Some(id) = &value.id {
                hex(id, 64)?;
            }
            if (complete && matches!(origin, "+" | "-")) != value.id.is_some() {
                return Err(invalid("line selection ID"));
            }
        }
        Ok(Self {
            index: count(value.line_index)?,
            offset: count(value.byte_offset)?,
            complete,
            origin,
            old: value.old_line.map(count).transpose()?,
            new: value.new_line.map(count).transpose()?,
            length,
        })
    }
    fn follows(self, before: Self) -> Result<()> {
        if self.index != before.index + u64::from(before.complete)
            || self.offset
                != if before.complete {
                    0
                } else {
                    before.offset + before.length
                }
            || (!before.complete
                && (self.origin != before.origin
                    || self.old != before.old
                    || self.new != before.new))
        {
            return Err(invalid("line continuation"));
        }
        Ok(())
    }
}

/// Check one wire page, before it enters Resources or a cache. `working`
/// selects a working-tree comparison; `from_start` a page read without a
/// cursor, which must also begin at the first file, hunk and line.
pub fn validate(page: &DiffPage, working: bool, from_start: bool) -> Result<()> {
    crate::git_page::validate_page(page)?;
    let meta = &page.metadata;
    if meta.read_only == working {
        return Err(invalid("comparison mode"));
    }
    if count(required(meta.context_lines, "context lines")?.into())? > 100 {
        return Err(invalid("context lines"));
    }
    let total = count(required(meta.total_units, "total units")?)?;
    let omissions = required(meta.has_omissions, "omissions")?;
    let total_files = if working {
        required(meta.source_snapshot.as_ref(), "source snapshot")?;
        required(meta.entry_id.as_ref(), "entry")?;
        required(meta.side, "side")?;
        Some(count(required(meta.total_files, "total files")?)?)
    } else {
        oid(required(meta.commit_oid.as_ref(), "commit")?)?;
        meta.parent_oid.as_ref().map(oid).transpose()?;
        let index = meta.parent_index.map(count).transpose()?;
        let parents = required(meta.parents.as_ref(), "parents")?;
        if parents.len() > 10000 {
            return Err(invalid("array"));
        }
        for parent in parents {
            oid(parent)?;
        }
        if parents.is_empty() {
            if index.is_some() || meta.parent_oid.is_some() {
                return Err(invalid("root parent"));
            }
        } else if index
            .and_then(|i| usize::try_from(i).ok())
            .and_then(|i| parents.get(i))
            .is_none_or(|p| Some(&p.hex) != meta.parent_oid.as_ref().map(|o| &o.hex))
        {
            return Err(invalid("selected parent"));
        }
        bytes(required(meta.selected_path.as_ref(), "selected path")?)?;
        None
    };
    let files = &page.entries;
    let mut units = 0;
    let mut previous_file = None;
    let mut tail: Option<(Piece<'_>, Option<u64>)> = None;
    for (fi, file) in files.iter().enumerate() {
        if fi > 0 && tail.is_some_and(|(p, n)| !p.complete || n.is_some_and(|n| p.index + 1 != n)) {
            return Err(invalid("unfinished file"));
        }
        let file_index = count(file.file_index)?;
        if previous_file.is_some_and(|p| file_index <= p)
            || (working && from_start && file_index != fi as u64)
        {
            return Err(invalid("file order"));
        }
        previous_file = Some(file_index);
        bytes(&file.old_path)?;
        bytes(&file.new_path)?;
        file.old_oid.as_ref().map(oid).transpose()?;
        file.new_oid.as_ref().map(oid).transpose()?;
        count(file.additions)?;
        count(file.deletions)?;
        let omitted = match file.omission_reason.as_deref() {
            None => false,
            Some("binary" | "file_size_limit") => true,
            Some(_) => return Err(invalid("omission reason")),
        };
        let hunks = &file.hunks;
        if hunks.len() > 5000 {
            return Err(invalid("array"));
        }
        if omitted && (!omissions || !hunks.is_empty()) {
            return Err(invalid("omitted content"));
        }
        if hunks.is_empty() {
            units += 1;
        }
        let mut previous_hunk = None;
        for (hi, hunk) in hunks.iter().enumerate() {
            if let Some((last, total_lines)) = tail
                && (!last.complete || total_lines.is_some_and(|n| last.index + 1 != n))
            {
                return Err(invalid("unfinished hunk"));
            }
            let index = count(hunk.index)?;
            if previous_hunk.is_some_and(|p| index != p + 1)
                || ((from_start || fi > 0) && hi == 0 && index != 0)
            {
                return Err(invalid("hunk order"));
            }
            previous_hunk = Some(index);
            for value in [
                hunk.old_start,
                hunk.old_lines,
                hunk.new_start,
                hunk.new_lines,
            ] {
                count(value)?;
            }
            let total_lines = if working {
                hex(required(hunk.id.as_deref(), "hunk")?, 64)?;
                let n = count(hunk.total_lines)?;
                if n == 0 {
                    return Err(invalid("hunk length"));
                }
                Some(n)
            } else {
                None
            };
            let pieces = &hunk.lines;
            if pieces.len() > 5000 {
                return Err(invalid("array"));
            }
            if pieces.is_empty() {
                return Err(invalid("empty hunk"));
            }
            let mut previous_piece = None;
            for line in pieces {
                let piece = Piece::read(line, working)?;
                if total_lines.is_some_and(|n| piece.index >= n) {
                    return Err(invalid("hunk length"));
                }
                if let Some(before) = previous_piece {
                    piece.follows(before)?;
                } else if (from_start || fi > 0 || hi > 0)
                    && (piece.index != 0 || piece.offset != 0)
                {
                    return Err(invalid("hunk start"));
                }
                previous_piece = Some(piece);
                units += 1;
                if units > 5000 {
                    return Err(invalid("page units"));
                }
            }
            tail = previous_piece.map(|p| (p, total_lines));
        }
    }
    let more = page.next_cursor.is_some();
    if units > 5000 || units > total || (more && units == 0) {
        return Err(invalid("page units"));
    }
    if !more && tail.is_some_and(|(p, n)| !p.complete || n.is_some_and(|n| p.index + 1 != n)) {
        return Err(invalid("incomplete final hunk"));
    }
    if from_start
        && ((more && units >= total)
            || (!more && units != total)
            || total_files
                .is_some_and(|n| files.len() as u64 > n || (!more && files.len() as u64 != n)))
    {
        return Err(invalid("diff completeness"));
    }
    Ok(())
}

/// Whether two fragments describe the same file, so a continuation extends
/// it rather than starting another.
fn same_file(a: &FileFragment, b: &FileFragment) -> bool {
    a.file_index == b.file_index
        && a.old_path == b.old_path
        && a.new_path == b.new_path
        && a.old_oid == b.old_oid
        && a.new_oid == b.new_oid
        && a.old_mode == b.old_mode
        && a.new_mode == b.new_mode
        && a.status == b.status
        && a.binary == b.binary
        && a.omission_reason == b.omission_reason
        && a.additions == b.additions
        && a.deletions == b.deletions
}

/// Join a checked continuation page onto an accepted one, into canonical
/// files and hunks. The accepted page is borrowed and stays untouched on
/// every failure.
pub fn merge(
    previous: &DiffPage,
    mut incoming: DiffPage,
    cursor: &str,
    working: bool,
) -> Result<DiffPage> {
    crate::git_page::validate_page_continuation(previous, &incoming, cursor)?;
    validate(&incoming, working, false)?;
    let mut files = previous.entries.clone();
    let fragments = std::mem::take(&mut incoming.entries);
    if fragments.is_empty() {
        return Err(invalid("empty continuation"));
    }
    for mut fragment in fragments {
        let tail = files
            .last_mut()
            .ok_or_else(|| invalid("missing previous file"))?;
        let old_index = tail.file_index;
        let new_index = fragment.file_index;
        if new_index != old_index {
            if new_index < old_index || (working && new_index != old_index + 1) {
                return Err(invalid("file continuation order"));
            }
            if let Some(hunk) = tail.hunks.last() {
                finished_hunk(hunk, working)?;
            }
            if let Some(hunk) = fragment.hunks.first() {
                if hunk.index != 0 {
                    return Err(invalid("file hunk start"));
                }
                starts_hunk(hunk, working)?;
            }
            files.push(fragment);
            continue;
        }
        if !same_file(tail, &fragment) {
            return Err(invalid("file continuation header"));
        }
        let hunks = std::mem::take(&mut fragment.hunks);
        if tail.hunks.is_empty() || hunks.is_empty() {
            return Err(invalid("repeated empty file"));
        }
        for mut hunk in hunks {
            let last = tail.hunks.last_mut().expect("nonempty hunks");
            if hunk.index != last.index {
                if hunk.index != last.index + 1 {
                    return Err(invalid("hunk continuation order"));
                }
                finished_hunk(last, working)?;
                starts_hunk(&hunk, working)?;
                tail.hunks.push(hunk);
                continue;
            }
            if (
                last.old_start,
                last.old_lines,
                last.new_start,
                last.new_lines,
            ) != (
                hunk.old_start,
                hunk.old_lines,
                hunk.new_start,
                hunk.new_lines,
            ) {
                return Err(invalid("hunk continuation header"));
            }
            if working && (last.id != hunk.id || last.total_lines != hunk.total_lines) {
                return Err(invalid("hunk continuation identity"));
            }
            let first = hunk.lines.first().ok_or_else(|| invalid("empty hunk"))?;
            let before = last
                .lines
                .last()
                .ok_or_else(|| invalid("empty previous hunk"))?;
            Piece::read(first, working)?.follows(Piece::read(before, working)?)?;
            last.lines.append(&mut hunk.lines);
        }
    }
    let loaded: u64 = files
        .iter()
        .map(|file| {
            if file.hunks.is_empty() {
                1
            } else {
                file.hunks.iter().map(|h| h.lines.len() as u64).sum()
            }
        })
        .sum();
    let total = incoming.metadata.total_units.unwrap_or_default();
    let more = incoming.next_cursor.is_some();
    if loaded > total || (more && loaded >= total) || (!more && loaded != total) {
        return Err(invalid("cumulative diff units"));
    }
    if working {
        let total_files = incoming.metadata.total_files.unwrap_or_default();
        if files.len() as u64 > total_files || (!more && files.len() as u64 != total_files) {
            return Err(invalid("cumulative diff files"));
        }
    }
    incoming.entries = files;
    Ok(incoming)
}

fn starts_hunk(hunk: &HunkFragment, working: bool) -> Result<()> {
    let first = hunk.lines.first().ok_or_else(|| invalid("empty hunk"))?;
    let piece = Piece::read(first, working)?;
    if piece.index != 0 || piece.offset != 0 {
        return Err(invalid("hunk start"));
    }
    Ok(())
}

fn finished_hunk(hunk: &HunkFragment, working: bool) -> Result<()> {
    let last = hunk.lines.last().ok_or_else(|| invalid("empty hunk"))?;
    let piece = Piece::read(last, working)?;
    if !piece.complete || (working && piece.index + 1 != hunk.total_lines) {
        return Err(invalid("unfinished hunk"));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use newport_protocol::git::{Binary, DiffMetadata, Side};

    pub(crate) fn object() -> ObjectId {
        ObjectId {
            algorithm: "sha1".into(),
            hex: "a".repeat(40),
        }
    }

    /// A one-line diff page of a working-tree or commit comparison.
    pub(crate) fn page(working: bool) -> DiffPage {
        let path = Path::new(b"file");
        let metadata = if working {
            DiffMetadata {
                read_only: false,
                source_snapshot: Some("status".into()),
                entry_id: Some("file-id".into()),
                side: Some(Side::IndexToWorktree),
                context_lines: Some(3),
                has_omissions: Some(false),
                total_units: Some(1),
                total_files: Some(1),
                ..empty_metadata()
            }
        } else {
            DiffMetadata {
                read_only: true,
                commit_oid: Some(object()),
                parents: Some(vec![]),
                selected_path: Some(path.clone()),
                context_lines: Some(3),
                has_omissions: Some(false),
                total_units: Some(1),
                ..empty_metadata()
            }
        };
        git::Page {
            snapshot: "snapshot".into(),
            next_cursor: None,
            metadata,
            entries: vec![FileFragment {
                file_index: 0,
                old_path: path.clone(),
                new_path: path,
                old_oid: None,
                new_oid: Some(object()),
                old_mode: 33188,
                new_mode: 33188,
                status: "Modified".into(),
                binary: false,
                omission_reason: None,
                additions: 1,
                deletions: 0,
                hunks: vec![HunkFragment {
                    id: working.then(|| "b".repeat(64)),
                    index: 0,
                    old_start: 0,
                    old_lines: 0,
                    new_start: 1,
                    new_lines: 1,
                    total_lines: 1,
                    lines: vec![LineFragment {
                        id: working.then(|| "c".repeat(64)),
                        line_index: 0,
                        byte_offset: 0,
                        line_complete: true,
                        origin: "+".into(),
                        old_line: None,
                        new_line: Some(1),
                        content_bytes_b64: Binary(b"x\n".to_vec()),
                    }],
                }],
            }],
        }
    }

    pub(crate) fn empty_metadata() -> DiffMetadata {
        DiffMetadata {
            read_only: false,
            commit_oid: None,
            parent_oid: None,
            parent_index: None,
            parents: None,
            source_snapshot: None,
            entry_id: None,
            side: None,
            context_lines: None,
            selected_path: None,
            truncated: None,
            total_files: None,
            total_units: None,
            has_omissions: None,
        }
    }

    fn line(page: &mut DiffPage) -> &mut LineFragment {
        &mut page.entries[0].hunks[0].lines[0]
    }

    #[test]
    fn both_comparisons_accept_their_own_metadata() {
        for working in [true, false] {
            validate(&page(working), working, true).unwrap();
            assert!(validate(&page(working), !working, true).is_err());
        }
    }

    #[test]
    fn missing_metadata_never_becomes_an_empty_comparison() {
        let edits: [fn(&mut DiffMetadata); 5] = [
            |m| m.context_lines = None,
            |m| m.total_units = None,
            |m| m.has_omissions = None,
            |m| m.source_snapshot = None,
            |m| m.total_files = None,
        ];
        for edit in edits {
            let mut response = page(true);
            edit(&mut response.metadata);
            assert!(validate(&response, true, true).is_err());
        }
        let edits: [fn(&mut DiffMetadata); 3] = [
            |m| m.commit_oid = None,
            |m| m.parents = None,
            |m| m.selected_path = None,
        ];
        for edit in edits {
            let mut response = page(false);
            edit(&mut response.metadata);
            assert!(validate(&response, false, true).is_err());
        }
        let mut response = page(true);
        response.entries[0].hunks[0].lines.clear();
        assert!(validate(&response, true, true).is_err());
        let mut response = page(true);
        response.snapshot.clear();
        assert!(validate(&response, true, true).is_err());
    }

    #[test]
    fn wrong_mode_ids_counts_and_parent_comparisons_are_rejected() {
        let edits: [fn(&mut DiffPage); 11] = [
            |p| p.metadata.read_only = true,
            |p| p.metadata.total_units = Some(2),
            |p| p.metadata.total_files = Some(2),
            |p| p.metadata.context_lines = Some(101),
            |p| p.entries[0].file_index = 1,
            |p| p.entries[0].hunks[0].index = 1,
            |p| p.entries[0].hunks[0].total_lines = 2,
            |p| p.entries[0].hunks[0].id = Some("bad".into()),
            |p| line(p).id = None,
            |p| line(p).origin = "invalid-origin".into(),
            |p| line(p).line_index = 9_007_199_254_740_992,
        ];
        for edit in edits {
            let mut response = page(true);
            edit(&mut response);
            assert!(validate(&response, true, true).is_err());
        }
        let mut historical = page(false);
        historical.metadata.parents = Some(vec![object()]);
        assert!(validate(&historical, false, true).is_err());
        historical.metadata.parent_index = Some(0);
        historical.metadata.parent_oid = Some(object());
        validate(&historical, false, true).unwrap();
    }

    #[test]
    fn partial_page_can_start_mid_line_but_initial_and_final_boundaries_are_checked() {
        let mut first = page(true);
        first.next_cursor = Some("next".into());
        first.metadata.total_units = Some(2);
        let piece = line(&mut first);
        piece.content_bytes_b64 = Binary(vec![b'x'; 4096]);
        piece.line_complete = false;
        piece.id = None;
        validate(&first, true, true).unwrap();
        let mut last = page(true);
        last.metadata.total_units = Some(2);
        line(&mut last).byte_offset = 4096;
        validate(&last, true, false).unwrap();
        assert!(validate(&last, true, true).is_err());
        first.next_cursor = None;
        assert!(validate(&first, true, true).is_err());
    }

    #[test]
    fn omissions_and_empty_comparisons_have_explicit_consistent_metadata() {
        let mut omitted = page(true);
        omitted.metadata.has_omissions = Some(true);
        omitted.entries[0].hunks.clear();
        omitted.entries[0].omission_reason = Some("binary".into());
        omitted.entries[0].binary = true;
        validate(&omitted, true, true).unwrap();
        omitted.metadata.has_omissions = Some(false);
        assert!(validate(&omitted, true, true).is_err());
        omitted.metadata.has_omissions = Some(true);
        omitted.entries[0].omission_reason = Some("unknown".into());
        assert!(validate(&omitted, true, true).is_err());
        let mut empty = page(true);
        empty.entries.clear();
        empty.metadata.total_units = Some(0);
        empty.metadata.total_files = Some(0);
        validate(&empty, true, true).unwrap();
        empty.next_cursor = Some("not-empty".into());
        assert!(validate(&empty, true, true).is_err());
    }

    #[test]
    fn duplicates_gaps_and_unfinished_lines_cannot_hide_between_hunks() {
        for index in [0, 2] {
            let mut response = page(true);
            response.metadata.total_units = Some(2);
            response.entries[0].hunks[0].total_lines = 3;
            let mut next = line(&mut response).clone();
            next.line_index = index;
            response.entries[0].hunks[0].lines.push(next);
            assert!(validate(&response, true, true).is_err());
        }
        let mut response = page(true);
        response.metadata.total_units = Some(2);
        response.metadata.total_files = Some(2);
        let mut empty_file = response.entries[0].clone();
        empty_file.file_index = 1;
        empty_file.hunks.clear();
        response.entries[0].hunks[0].total_lines = 2;
        response.entries.push(empty_file);
        assert!(validate(&response, true, true).is_err());
    }
}

#[cfg(test)]
mod continuation_tests {
    use super::{DiffPage, merge, tests::page, validate};
    use newport_protocol::git::Binary;

    fn pair(working: bool) -> (DiffPage, DiffPage) {
        let mut first = page(working);
        first.metadata.total_units = Some(2);
        first.entries[0].hunks[0].total_lines = 2;
        first.next_cursor = Some("next".into());
        let mut last = first.clone();
        last.next_cursor = None;
        let line = &mut last.entries[0].hunks[0].lines[0];
        line.line_index = 1;
        line.new_line = Some(2);
        (first, last)
    }

    #[test]
    fn repeated_file_and_hunk_headers_join_without_duplicate_rows() {
        for working in [true, false] {
            let (first, last) = pair(working);
            validate(&first, working, true).unwrap();
            let result = merge(&first, last, "next", working).unwrap();
            assert_eq!(result.entries.len(), 1);
            assert_eq!(result.entries[0].hunks.len(), 1);
            assert_eq!(result.entries[0].hunks[0].lines.len(), 2);
            assert_eq!(first.entries[0].hunks[0].lines.len(), 1);
        }
    }

    #[test]
    fn mismatched_headers_and_discontinuous_pieces_preserve_the_previous_page() {
        for working in [true, false] {
            let (first, last) = pair(working);
            let original = first.clone();
            let edits: [fn(&mut DiffPage); 6] = [
                |p| p.entries[0].old_mode = 33261,
                |p| p.entries[0].old_path = newport_protocol::git::Path::new(b"other"),
                |p| p.entries[0].additions = 8,
                |p| p.entries[0].hunks[0].old_start = 10,
                |p| p.entries[0].hunks[0].lines[0].line_index = 0,
                |p| p.entries[0].hunks[0].lines[0].byte_offset = 1,
            ];
            for edit in edits {
                let mut changed = last.clone();
                edit(&mut changed);
                assert!(merge(&first, changed, "next", working).is_err());
                assert_eq!(first, original);
            }
            assert!(merge(&first, last.clone(), "wrong", working).is_err());
            let mut changed = last;
            changed.metadata.context_lines = Some(10);
            assert!(merge(&first, changed, "next", working).is_err());
        }
    }

    #[test]
    fn final_piece_can_supply_an_id_but_not_change_the_continued_line() {
        let (mut first, mut last) = pair(true);
        for p in [&mut first, &mut last] {
            p.entries[0].hunks[0].total_lines = 1;
        }
        let before = &mut first.entries[0].hunks[0].lines[0];
        before.id = None;
        before.line_complete = false;
        before.content_bytes_b64 = Binary(vec![b'x'; 4096]);
        let after = &mut last.entries[0].hunks[0].lines[0];
        after.line_index = 0;
        after.new_line = Some(1);
        after.byte_offset = 4096;
        validate(&first, true, true).unwrap();
        let result = merge(&first, last.clone(), "next", true).unwrap();
        assert!(result.entries[0].hunks[0].lines[1].id.is_some());
        let edits: [fn(&mut DiffPage); 3] = [
            |p| p.entries[0].hunks[0].lines[0].origin = "-".into(),
            |p| p.entries[0].hunks[0].lines[0].new_line = Some(3),
            |p| p.entries[0].hunks[0].lines[0].byte_offset = 8192,
        ];
        for edit in edits {
            let mut changed = last.clone();
            edit(&mut changed);
            assert!(merge(&first, changed, "next", true).is_err());
        }
    }

    #[test]
    fn new_hunks_and_files_must_follow_finished_predecessors() {
        let (mut first, mut last) = pair(true);
        first.entries[0].hunks[0].total_lines = 1;
        let hunk = &mut last.entries[0].hunks[0];
        hunk.total_lines = 1;
        hunk.index = 1;
        hunk.id = Some("d".repeat(64));
        hunk.lines[0].line_index = 0;
        assert_eq!(
            merge(&first, last.clone(), "next", true).unwrap().entries[0]
                .hunks
                .len(),
            2
        );
        first.entries[0].hunks[0].total_lines = 2;
        assert!(merge(&first, last.clone(), "next", true).is_err());
        first.entries[0].hunks[0].total_lines = 1;
        for p in [&mut first, &mut last] {
            p.metadata.total_files = Some(2);
        }
        last.entries[0].file_index = 1;
        // The next file cannot start at the second hunk.
        assert!(merge(&first, last.clone(), "next", true).is_err());
        last.entries[0].hunks[0].index = 0;
        assert_eq!(
            merge(&first, last.clone(), "next", true)
                .unwrap()
                .entries
                .len(),
            2
        );
        last.entries[0].file_index = 2;
        assert!(merge(&first, last, "next", true).is_err());
    }

    #[test]
    fn cumulative_counts_reject_early_ends_and_spurious_continuations() {
        for working in [true, false] {
            let (first, mut last) = pair(working);
            last.next_cursor = Some("more".into());
            assert!(merge(&first, last, "next", working).is_err());
            let (mut first, mut last) = pair(working);
            for p in [&mut first, &mut last] {
                p.metadata.total_units = Some(3);
            }
            assert!(merge(&first, last.clone(), "next", working).is_err());
            last.entries.clear();
            assert!(merge(&first, last, "next", working).is_err());
        }
    }

    #[test]
    fn many_pages_can_accumulate_more_than_wire_limits() {
        let mut first = page(true);
        first.metadata.total_units = Some(400);
        first.metadata.total_files = Some(400);
        first.next_cursor = Some("next".into());
        let mut file = first.entries[0].clone();
        file.hunks.clear();
        let files = |start| {
            (start..start + 200)
                .map(|i| {
                    let mut f = file.clone();
                    f.file_index = i;
                    f
                })
                .collect::<Vec<_>>()
        };
        first.entries = files(0);
        let mut last = first.clone();
        last.next_cursor = None;
        last.entries = files(200);
        validate(&first, true, true).unwrap();
        assert_eq!(
            merge(&first, last, "next", true).unwrap().entries.len(),
            400
        );
    }
}
