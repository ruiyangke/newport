//! Partial index changes refer to server-generated IDs, never client patch text.
use newport_protocol::git::{Action, HunkSelection, Side};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub struct Review {
    snapshot: String,
    entry: String,
    staged: bool,
    context: u32,
    hunks: BTreeMap<String, BTreeSet<String>>,
    deletions: BTreeSet<String>,
    complete_hunks: BTreeSet<String>,
}
impl Review {
    pub fn from_page(page: &crate::git_diff_page::DiffPage) -> Result<Self, String> {
        let unavailable = || {
            "Individual hunks are unavailable for this comparison. Use the file actions.".to_owned()
        };
        if page.next_cursor.as_deref() == Some("") {
            return Err(unavailable());
        }
        let meta = &page.metadata;
        if meta.read_only || meta.truncated == Some(true) || meta.total_files != Some(1) {
            return Err(unavailable());
        }
        let snapshot = meta
            .source_snapshot
            .as_deref()
            .filter(|v| !v.is_empty())
            .ok_or_else(unavailable)?;
        let entry = meta
            .entry_id
            .as_deref()
            .filter(|v| !v.is_empty())
            .ok_or_else(unavailable)?;
        let staged = match meta.side {
            Some(Side::HeadToIndex) => true,
            Some(Side::IndexToWorktree) => false,
            _ => return Err(unavailable()),
        };
        let context = meta
            .context_lines
            .filter(|value| *value <= 25)
            .ok_or_else(unavailable)?;
        let files = &page.entries;
        if files.is_empty() {
            return Err(unavailable());
        }
        let mut hunks: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut deletions = BTreeSet::new();
        let mut totals = BTreeMap::new();
        let mut completed: BTreeMap<String, BTreeSet<u64>> = BTreeMap::new();
        for file in files {
            // Accumulated pages contain repeated fragments of the same file/hunk.
            let text_mode = |mode: u32| matches!(mode, 0o100644 | 0o100755);
            let modes = match file.status.as_str() {
                "Modified" => file.old_mode == file.new_mode && text_mode(file.old_mode),
                "Added" | "Untracked" => text_mode(file.new_mode),
                "Deleted" => text_mode(file.old_mode),
                _ => false,
            };
            if file.file_index != 0
                || file.binary
                || file.omission_reason.is_some()
                || !modes
                || file.old_path.bytes_b64 != file.new_path.bytes_b64
            {
                return Err(unavailable());
            }
            for hunk in &file.hunks {
                let id = hunk
                    .id
                    .as_deref()
                    .filter(|v| !v.is_empty())
                    .ok_or_else(unavailable)?;
                let lines = hunks.entry(id.to_owned()).or_default();
                let total = Some(hunk.total_lines)
                    .filter(|n| *n > 0)
                    .ok_or_else(unavailable)?;
                if totals
                    .insert(id.to_owned(), total)
                    .is_some_and(|old| old != total)
                {
                    return Err(unavailable());
                }
                for line in &hunk.lines {
                    if !line.line_complete {
                        continue;
                    }
                    completed
                        .entry(id.to_owned())
                        .or_default()
                        .insert(line.line_index);
                    if matches!(line.origin.as_str(), "+" | "-") {
                        let id = line
                            .id
                            .as_deref()
                            .filter(|v| !v.is_empty())
                            .ok_or_else(unavailable)?;
                        lines.insert(id.to_owned());
                        if line.origin == "-" {
                            deletions.insert(id.to_owned());
                        }
                    }
                }
            }
        }
        // Context-only prefixes and unfinished lines must not disable earlier
        // selectable lines. Whole-hunk actions require every logical line,
        // including context and EOF notes, across all loaded fragments.
        hunks.retain(|_, lines| !lines.is_empty());
        if hunks.is_empty() {
            return Err(unavailable());
        }
        let complete_hunks = hunks
            .keys()
            .filter(|id| {
                let total = totals[*id];
                completed.get(*id).is_some_and(|indices| {
                    indices.len() as u64 == total
                        && indices.first() == Some(&0)
                        && indices.last() == Some(&(total - 1))
                })
            })
            .cloned()
            .collect();
        Ok(Self {
            snapshot: snapshot.into(),
            entry: entry.into(),
            staged,
            context,
            hunks,
            deletions,
            complete_hunks,
        })
    }
    pub fn snapshot(&self) -> &str {
        &self.snapshot
    }
    pub fn staged(&self) -> bool {
        self.staged
    }
    pub fn matches(&self, snapshot: &str, entry: &str, staged: bool, context: u8) -> bool {
        self.snapshot == snapshot
            && self.entry == entry
            && self.staged == staged
            && self.context == u32::from(context)
    }
    pub fn contains_hunk(&self, id: &str) -> bool {
        self.hunks.contains_key(id)
    }
    pub fn hunk_complete(&self, id: &str) -> bool {
        self.complete_hunks.contains(id)
    }
    pub fn contains_line(&self, hunk: &str, line: &str) -> bool {
        self.hunks
            .get(hunk)
            .is_some_and(|lines| lines.contains(line))
    }
    /// A partially selected run becomes fully selected; a full run is cleared.
    /// Validate the entire group before touching selection.
    pub fn toggle_lines(
        &self,
        selected: &mut BTreeSet<String>,
        lines: &[String],
    ) -> Result<(), String> {
        self.hunks_for_lines(lines)?;
        let clear = lines.iter().all(|line| selected.contains(line));
        for line in lines {
            if clear {
                selected.remove(line);
            } else {
                selected.insert(line.clone());
            }
        }
        Ok(())
    }
    /// Selecting an addition alone does not imply selecting its replacement's deletion.
    pub fn retains_deletions(&self, selected: &BTreeSet<String>) -> bool {
        self.hunks.values().any(|lines| {
            lines
                .iter()
                .any(|line| selected.contains(line) && !self.deletions.contains(line))
                && lines
                    .iter()
                    .any(|line| !selected.contains(line) && self.deletions.contains(line))
        })
    }
    /// Resolve selections against this review, never against a fresh diff.
    pub fn hunks_for_lines(&self, lines: &[String]) -> Result<Vec<String>, String> {
        let hunks: Vec<_> = self
            .hunks
            .iter()
            .filter(|(_, known)| lines.iter().any(|line| known.contains(line)))
            .map(|(hunk, _)| hunk.clone())
            .collect();
        self.action(&hunks, Some(lines))?;
        Ok(hunks)
    }
    pub fn action(&self, ids: &[String], lines: Option<&[String]>) -> Result<Action, String> {
        let invalid =
            || "Select complete hunks or changed lines from the reviewed comparison.".to_owned();
        let unique: BTreeSet<_> = ids.iter().collect();
        if ids.is_empty()
            || ids.len() > 1024
            || unique.len() != ids.len()
            || ids.iter().any(|id| !self.hunks.contains_key(id))
            || (lines.is_none() && ids.iter().any(|id| !self.hunk_complete(id)))
        {
            return Err(invalid());
        }
        if let Some(lines) = lines {
            let unique: BTreeSet<_> = lines.iter().collect();
            if lines.is_empty()
                || unique.len() != lines.len()
                || lines
                    .iter()
                    .any(|line| !ids.iter().any(|id| self.hunks[id].contains(line)))
            {
                return Err(invalid());
            }
        }
        let hunks = Some(HunkSelection {
            ids: ids.to_vec(),
            lines: lines.map(<[String]>::to_vec),
            context_lines: self.context,
        });
        let action = if self.staged {
            Action::Unstage {
                entry_ids: vec![self.entry.clone()],
                hunks,
            }
        } else {
            Action::Stage {
                entry_ids: vec![self.entry.clone()],
                hunks,
            }
        };
        Ok(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_diff_page::DiffPage;
    use newport_protocol::git::{Binary, HunkFragment, LineFragment};

    /// A changed line, or a context line when `id` is `None` and `origin` is
    /// not a change.
    fn line(index: u64, id: Option<&str>, origin: &str, complete: bool) -> LineFragment {
        LineFragment {
            line_index: index,
            byte_offset: 0,
            line_complete: complete,
            origin: origin.into(),
            old_line: None,
            new_line: None,
            content_bytes_b64: Binary::default(),
            id: id.map(Into::into),
        }
    }
    fn hunk(id: &str, total: u64, lines: Vec<LineFragment>) -> HunkFragment {
        HunkFragment {
            id: Some(id.into()),
            index: 0,
            old_start: 0,
            old_lines: 0,
            new_start: 0,
            new_lines: 0,
            total_lines: total,
            lines,
        }
    }
    /// Lines numbered in order, as one complete hunk.
    fn changes(id: &str, lines: &[(&str, &str)]) -> HunkFragment {
        hunk(
            id,
            lines.len() as u64,
            lines
                .iter()
                .enumerate()
                .map(|(index, (line_id, origin))| line(index as u64, Some(line_id), origin, true))
                .collect(),
        )
    }
    pub(super) fn page() -> DiffPage {
        let mut page = crate::git_diff_page::tests::page(true);
        page.snapshot = "diff".into();
        page.metadata.entry_id = Some("entry".into());
        page.entries[0].hunks = vec![changes("h1", &[("l1", "+")])];
        page
    }
    #[test]
    fn context_is_bounded_before_typed_action_construction() {
        let mut page = page();
        page.metadata.context_lines = Some(25);
        let review = Review::from_page(&page).unwrap();
        assert!(matches!(
            review.action(&["h1".into()], None).unwrap(),
            Action::Stage {
                hunks: Some(HunkSelection {
                    context_lines: 25,
                    ..
                }),
                ..
            }
        ));
        for context in [26, u32::MAX] {
            page.metadata.context_lines = Some(context);
            assert!(Review::from_page(&page).is_err());
        }
    }

    #[test]
    fn loaded_lines_are_actionable_before_the_hunk_is_complete() {
        for side in [Side::IndexToWorktree, Side::HeadToIndex] {
            let mut page = page();
            page.next_cursor = Some("more".into());
            page.metadata.side = Some(side);
            page.entries[0].hunks[0] = hunk(
                "h1",
                3,
                vec![
                    line(0, None, " ", true),
                    line(1, Some("l1"), "+", true),
                    line(2, None, "+", false),
                ],
            );
            let review = Review::from_page(&page).unwrap();
            assert!(review.contains_line("h1", "l1"));
            assert!(!review.hunk_complete("h1"));
            assert!(review.action(&["h1".into()], None).is_err());
            assert!(review.action(&["h1".into()], Some(&["l1".into()])).is_ok());
            assert!(
                review
                    .action(&["h1".into()], Some(&["unloaded".into()]))
                    .is_err()
            );
            let mut second = page.entries[0].clone();
            second.hunks[0].lines = vec![line(2, Some("l2"), "+", true)];
            // Other hunks remain unloaded; this hunk is now complete.
            page.entries.push(second);
            let review = Review::from_page(&page).unwrap();
            assert!(review.hunk_complete("h1"));
            assert!(review.action(&["h1".into()], None).is_ok());
            assert!(
                review
                    .action(&["h1".into()], Some(&["l1".into(), "l2".into()]))
                    .is_ok()
            );
        }
    }
    #[test]
    fn context_only_tail_does_not_block_earlier_hunks() {
        let mut page = page();
        page.next_cursor = Some("more".into());
        page.entries[0]
            .hunks
            .push(hunk("h2", 5, vec![line(0, None, " ", true)]));
        let review = Review::from_page(&page).unwrap();
        assert!(review.action(&["h1".into()], None).is_ok());
        assert!(!review.contains_hunk("h2"));
    }
    #[test]
    fn missing_lines_and_eof_notes_do_not_make_a_complete_hunk() {
        let mut page = page();
        page.entries[0].hunks[0] = hunk(
            "h1",
            3,
            vec![line(0, Some("l1"), "+", true), line(2, None, ">", true)],
        );
        assert!(!Review::from_page(&page).unwrap().hunk_complete("h1"));
        page.next_cursor = Some("more".into());
        assert!(!Review::from_page(&page).unwrap().hunk_complete("h1"));
        page.next_cursor = Some(String::new());
        assert!(Review::from_page(&page).is_err());
    }
    #[test]
    fn preserves_snapshot_context_side_and_opaque_ids() {
        let mut page = page();
        for staged in [false, true] {
            page.metadata.side = Some(if staged {
                Side::HeadToIndex
            } else {
                Side::IndexToWorktree
            });
            let review = Review::from_page(&page).unwrap();
            assert!(review.matches("status", "entry", staged, 3));
            assert!(!review.matches("newer", "entry", staged, 3));
            assert!(!review.matches("status", "other", staged, 3));
            assert!(!review.matches("status", "entry", !staged, 3));
            assert!(!review.matches("status", "entry", staged, 0));
            assert_eq!(
                review.action(&["h1".into()], None).unwrap(),
                if staged {
                    Action::Unstage {
                        entry_ids: vec!["entry".into()],
                        hunks: Some(HunkSelection {
                            ids: vec!["h1".into()],
                            lines: None,
                            context_lines: 3,
                        }),
                    }
                } else {
                    Action::Stage {
                        entry_ids: vec!["entry".into()],
                        hunks: Some(HunkSelection {
                            ids: vec!["h1".into()],
                            lines: None,
                            context_lines: 3,
                        }),
                    }
                }
            );
            let action = review.action(&["h1".into()], Some(&["l1".into()])).unwrap();
            let hunks = match action {
                Action::Stage { hunks, .. } | Action::Unstage { hunks, .. } => hunks.unwrap(),
                _ => panic!("wrong action"),
            };
            assert_eq!(hunks.lines, Some(vec!["l1".into()]));
        }
    }
    #[test]
    fn rejects_incomplete_unsupported_and_invalid_selections() {
        let original = page();
        let edits: [fn(&mut DiffPage); 9] = [
            |p| p.metadata.read_only = true,
            |p| p.metadata.total_files = Some(2),
            |p| p.metadata.side = Some(Side::HeadToWorktree),
            |p| p.metadata.truncated = Some(true),
            |p| p.entries[0].old_mode = 0o120000,
            |p| p.entries[0].old_path = newport_protocol::git::Path::new(b"other"),
            |p| p.entries[0].hunks[0].id = Some(String::new()),
            |p| p.entries[0].binary = true,
            |p| p.entries[0].omission_reason = Some("binary".into()),
        ];
        for edit in edits {
            let mut page = original.clone();
            edit(&mut page);
            assert!(Review::from_page(&page).is_err());
        }
        let review = Review::from_page(&original).unwrap();
        for ids in [
            vec![],
            vec!["unknown".into()],
            vec!["h1".into(), "h1".into()],
        ] {
            assert!(review.action(&ids, None).is_err());
        }
        for lines in [
            vec![],
            vec!["unknown".into()],
            vec!["l1".into(), "l1".into()],
        ] {
            assert!(review.action(&["h1".into()], Some(&lines)).is_err());
        }
    }
    #[test]
    fn selected_lines_resolve_only_their_hunks_and_preserve_unselected_deletions() {
        let mut page = page();
        page.entries[0].hunks = vec![
            changes("h1", &[("add", "+"), ("remove", "-")]),
            changes("h2", &[("other", "+")]),
        ];
        let review = Review::from_page(&page).unwrap();
        assert_eq!(review.hunks_for_lines(&["other".into()]).unwrap(), ["h2"]);
        assert_eq!(
            review
                .hunks_for_lines(&["other".into(), "add".into()])
                .unwrap(),
            ["h1", "h2"]
        );
        assert!(review.hunks_for_lines(&["context".into()]).is_err());
        assert!(review.hunks_for_lines(&[]).is_err());
        assert!(
            review
                .hunks_for_lines(&["add".into(), "add".into()])
                .is_err()
        );
        assert!(review.contains_line("h1", "add"));
        assert!(!review.contains_line("h2", "add"));
        assert!(review.retains_deletions(&BTreeSet::from(["add".into()])));
        assert!(!review.retains_deletions(&BTreeSet::from(["other".into()])));
        assert!(!review.retains_deletions(&BTreeSet::from(["add".into(), "remove".into()])));
    }
    #[test]
    fn paginated_fragments_share_hunk_identity() {
        let mut page = page();
        page.entries[0].hunks[0].total_lines = 2;
        let mut second = page.entries[0].clone();
        second.hunks[0].lines = vec![line(1, Some("l2"), "+", true)];
        page.entries.push(second);
        let review = Review::from_page(&page).unwrap();
        assert!(
            review
                .action(&["h1".into()], Some(&["l1".into(), "l2".into()]))
                .is_ok()
        );
    }
    #[test]
    fn toggling_groups_completes_mixed_selection_and_preserves_other_runs() {
        let mut page = page();
        page.entries[0].hunks[0] = changes("h1", &[("a", "-"), ("b", "+"), ("other", "+")]);
        let review = Review::from_page(&page).unwrap();
        let mut selection = BTreeSet::from(["a".into(), "other".into()]);
        let group = ["a".into(), "b".into()];
        review.toggle_lines(&mut selection, &group).unwrap();
        assert_eq!(
            selection,
            BTreeSet::from(["a".into(), "b".into(), "other".into()])
        );
        review.toggle_lines(&mut selection, &group).unwrap();
        assert_eq!(selection, BTreeSet::from(["other".into()]));
        assert!(
            review
                .toggle_lines(&mut selection, &["a".into(), "unknown".into()])
                .is_err()
        );
        assert_eq!(selection, BTreeSet::from(["other".into()]));
    }
}
