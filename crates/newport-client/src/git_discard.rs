//! Immutable discard targets; execution keeps the originally reviewed snapshot.
use newport_protocol::git::{
    Action, DiscardSource, HunkSelection, Page, StatusEntry, StatusSummary,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Index,
    Head,
}
#[derive(Clone, Debug)]
pub struct Review {
    snapshot: String,
    entry: String,
    index: bool,
    hunks: Option<HunkSelection>,
}
impl Review {
    pub fn file(
        status: Option<&Page<StatusEntry, StatusSummary>>,
        entry: Option<&StatusEntry>,
    ) -> Result<Self, String> {
        let invalid =
            || "Review an individual, non-conflicted file before discarding changes.".to_owned();
        let (Some(status), Some(entry)) = (status, entry) else {
            return Err(invalid());
        };
        if status.snapshot.is_empty() || entry.entry_id.is_empty() {
            return Err(invalid());
        }
        let known = status
            .entries
            .iter()
            .find(|row| row.entry_id == entry.entry_id)
            .ok_or_else(invalid)?;
        if known != entry
            || status.metadata.integration.is_some()
            || status.entries.iter().any(|row| row.conflicted)
            || (entry.untracked && entry.path.display.ends_with('/'))
            || !(entry.staged || entry.unstaged || entry.untracked)
        {
            return Err(invalid());
        }
        Ok(Self {
            snapshot: status.snapshot.clone(),
            entry: entry.entry_id.clone(),
            index: entry.unstaged || entry.untracked,
            hunks: None,
        })
    }
    pub fn selection(
        review: &crate::git_selection::Review,
        hunks: &[String],
        lines: Option<&[String]>,
    ) -> Result<Self, String> {
        if review.staged() {
            return Err("Only unstaged changes can be partially discarded.".into());
        }
        let Action::Stage {
            mut entry_ids,
            hunks,
        } = review.action(hunks, lines)?
        else {
            return Err("Only unstaged changes can be partially discarded.".into());
        };
        let entry = entry_ids
            .pop()
            .filter(|_| entry_ids.is_empty())
            .ok_or("Select one reviewed file.")?;
        Ok(Self {
            snapshot: review.snapshot().into(),
            entry,
            index: true,
            hunks,
        })
    }
    pub fn snapshot(&self) -> &str {
        &self.snapshot
    }
    pub fn matches(&self, snapshot: &str, entry: &str) -> bool {
        self.snapshot == snapshot && self.entry == entry
    }
    pub fn supports_index(&self) -> bool {
        self.index
    }
    pub fn partial(&self) -> bool {
        self.hunks.is_some()
    }
    pub fn line_count(&self) -> Option<usize> {
        self.hunks.as_ref()?.lines.as_ref().map(Vec::len)
    }
    pub fn action(&self, source: Source) -> Result<Action, String> {
        if (source == Source::Index && !self.index) || (source == Source::Head && self.partial()) {
            return Err("The restore source does not match the reviewed changes.".into());
        }
        Ok(Action::Discard {
            entry_ids: vec![self.entry.clone()],
            source: match source {
                Source::Index => DiscardSource::Index,
                Source::Head => DiscardSource::Head,
            },
            hunks: self.hunks.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_protocol::git::{GroupCounts, Head, IntegrationState, Path};
    fn entry(id: &str, path: &str) -> StatusEntry {
        StatusEntry {
            entry_id: id.into(),
            path: Path::new(path.as_bytes()),
            old_path: None,
            flags: 0,
            staged: false,
            unstaged: false,
            untracked: false,
            conflicted: false,
            conflict: None,
        }
    }
    fn status(entries: Vec<StatusEntry>) -> Page<StatusEntry, StatusSummary> {
        Page {
            snapshot: "s".into(),
            entries,
            next_cursor: None,
            metadata: StatusSummary {
                head: Head {
                    oid: None,
                    name: None,
                    detached: false,
                    unborn: true,
                },
                operation_state: "none".into(),
                integration: None,
                ahead: None,
                behind: None,
                basis: "local".into(),
                upstream_ref: None,
                upstream: None,
                truncated: false,
                total_entries: 0,
                group_counts: GroupCounts {
                    staged: 0,
                    unstaged: 0,
                    untracked: 0,
                    conflicted: 0,
                },
                matched_entries: None,
            },
        }
    }
    #[test]
    fn whole_file_uses_reviewed_entry_snapshot_and_explicit_restore_source() {
        let mut entry = entry("e", "file");
        entry.staged = true;
        let mut status = status(vec![entry.clone()]);
        let review = Review::file(Some(&status), Some(&entry)).unwrap();
        assert!(review.matches("s", "e"));
        assert!(!review.matches("new", "e"));
        assert!(!review.matches("s", "other"));
        assert_eq!(
            review.action(Source::Head).unwrap(),
            Action::Discard {
                entry_ids: vec!["e".into()],
                source: DiscardSource::Head,
                hunks: None
            }
        );
        assert!(review.action(Source::Index).is_err());
        status.entries[0].unstaged = true;
        assert!(Review::file(Some(&status), Some(&entry)).is_err());
        let review = Review::file(Some(&status), Some(&status.entries[0])).unwrap();
        assert!(matches!(
            review.action(Source::Index).unwrap(),
            Action::Discard {
                source: DiscardSource::Index,
                ..
            }
        ));
        status.metadata.integration = Some(IntegrationState {
            kind: "merge".into(),
            managed: true,
            can_continue: false,
            can_abort: true,
            can_skip: false,
        });
        assert!(Review::file(Some(&status), Some(&status.entries[0])).is_err());
        status.metadata.integration = None;
        let mut conflicted = super::tests::entry("c", "conflict");
        conflicted.conflicted = true;
        status.entries.push(conflicted);
        assert!(Review::file(Some(&status), Some(&status.entries[0])).is_err());
        assert!(Review::file(None, Some(&status.entries[0])).is_err());
        assert!(Review::file(Some(&status), None).is_err());
    }
    #[test]
    fn collapsed_untracked_directories_are_not_discard_targets() {
        let mut entry = entry("e", "directory/");
        entry.untracked = true;
        let status = status(vec![entry.clone()]);
        assert!(Review::file(Some(&status), Some(&entry)).is_err());
    }
    #[test]
    fn partial_discard_preserves_ids_and_rejects_staged_or_head_comparisons() {
        let mut page = crate::git_diff_page::tests::page(true);
        page.metadata.source_snapshot = Some("s".into());
        page.metadata.entry_id = Some("e".into());
        page.entries[0].hunks[0].id = Some("h".into());
        page.entries[0].hunks[0].lines[0].id = Some("l".into());
        let selection = crate::git_selection::Review::from_page(&page).unwrap();
        let review = Review::selection(&selection, &["h".into()], Some(&["l".into()])).unwrap();
        assert_eq!(review.line_count(), Some(1));
        assert_eq!(
            review.action(Source::Index).unwrap(),
            Action::Discard {
                entry_ids: vec!["e".into()],
                source: DiscardSource::Index,
                hunks: Some(HunkSelection {
                    ids: vec!["h".into()],
                    lines: Some(vec!["l".into()]),
                    context_lines: 3
                })
            }
        );
        assert!(review.action(Source::Head).is_err());
        assert!(Review::selection(&selection, &["invalid".into()], None).is_err());
        page.metadata.side = Some(newport_protocol::git::Side::HeadToIndex);
        let staged = crate::git_selection::Review::from_page(&page).unwrap();
        assert!(Review::selection(&staged, &["h".into()], None).is_err());
    }
}
