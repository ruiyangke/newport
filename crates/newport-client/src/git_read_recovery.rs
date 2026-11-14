//! Bounded recovery of snapshot-bound diff reads. Never applies to writes.
use newport_protocol::git::{Page, Path, StatusEntry, StatusSummary, code};

/// Resolve a selected comparison against a refreshed status page. Identity is
/// the exact path, never lossy display text or the old snapshot's entry token.
pub fn matching_entry(
    page: &Page<StatusEntry, StatusSummary>,
    path: &Path,
    staged: bool,
) -> Option<StatusEntry> {
    if path.bytes_b64.is_empty() {
        return None;
    }
    page.entries
        .iter()
        .find(|entry| {
            entry.path.bytes_b64 == path.bytes_b64
                && !entry.entry_id.is_empty()
                && !entry.conflicted
                && if staged {
                    entry.staged
                } else {
                    entry.unstaged || entry.untracked
                }
        })
        .cloned()
}

#[derive(Default)]
pub struct Recovery {
    attempts: u8,
    target: Option<(Path, bool)>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Ignore,
    Refresh,
    Exhausted,
}
impl Recovery {
    pub fn begin(
        &mut self,
        code: Option<&str>,
        entry: Option<&StatusEntry>,
        staged: bool,
    ) -> Decision {
        let Some(entry) = entry.filter(|entry| !entry.path.bytes_b64.is_empty()) else {
            return Decision::Ignore;
        };
        if !matches!(code, Some(code::STALE_SNAPSHOT | code::STALE_ENTRY)) || self.target.is_some()
        {
            return Decision::Ignore;
        }
        if self.attempts >= 3 {
            return Decision::Exhausted;
        }
        self.attempts += 1;
        self.target = Some((entry.path.clone(), staged));
        Decision::Refresh
    }
    pub fn pending(&self) -> bool {
        self.target.is_some()
    }
    /// Rebind only to a current, exact byte path and the same comparison side.
    /// A removed/clean/filtered file is not evidence for a different selection.
    /// Do not eagerly fetch every status page to find a disappeared row.
    pub fn resolve(
        &mut self,
        page: Option<&Page<StatusEntry, StatusSummary>>,
    ) -> Option<(StatusEntry, bool)> {
        let (path, staged) = self.target.take()?;
        matching_entry(page?, &path, staged).map(|entry| (entry, staged))
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_protocol::git::GroupCounts;

    fn entry(id: &str) -> StatusEntry {
        StatusEntry {
            entry_id: id.into(),
            path: Path::new(b"a"),
            old_path: None,
            flags: 0,
            staged: false,
            unstaged: true,
            untracked: false,
            conflicted: false,
            conflict: None,
        }
    }
    fn page(entries: Vec<StatusEntry>) -> Page<StatusEntry, StatusSummary> {
        Page {
            snapshot: "same".into(),
            entries,
            next_cursor: None,
            metadata: StatusSummary {
                head: newport_protocol::git::Head {
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
    fn moving_repository_is_bounded_even_when_snapshot_stays_the_same() {
        let mut recovery = Recovery::default();
        for _ in 0..3 {
            assert_eq!(
                recovery.begin(Some("STALE_ENTRY"), Some(&entry("old")), false),
                Decision::Refresh
            );
            assert_eq!(
                recovery.begin(Some("STALE_ENTRY"), Some(&entry("old")), false),
                Decision::Ignore
            );
            assert_eq!(
                recovery.resolve(Some(&page(vec![entry("new")]))),
                Some((entry("new"), false))
            );
        }
        assert_eq!(
            recovery.begin(Some("STALE_SNAPSHOT"), Some(&entry("new")), false),
            Decision::Exhausted
        );
        recovery.reset();
        assert_eq!(
            recovery.begin(Some("STALE_SNAPSHOT"), Some(&entry("new")), false),
            Decision::Refresh
        );
    }
    #[test]
    fn errors_and_display_paths_cannot_redirect_recovery() {
        let mut recovery = Recovery::default();
        for code in [
            None,
            Some("CONNECTION_LOST"),
            Some("OUTCOME_UNKNOWN"),
            Some("AUTH_FAILED"),
        ] {
            assert_eq!(
                recovery.begin(code, Some(&entry("old")), false),
                Decision::Ignore
            );
        }
        assert_eq!(
            recovery.begin(Some("STALE_ENTRY"), None, false),
            Decision::Ignore
        );
        let mut other = entry("other");
        other.path = Path {
            bytes_b64: "/w==".into(),
            display: "a".into(),
        };
        let mut staged = entry("staged");
        staged.staged = true;
        staged.unstaged = false;
        for rows in [vec![], vec![other], vec![staged]] {
            assert_eq!(
                recovery.begin(Some("STALE_ENTRY"), Some(&entry("old")), false),
                Decision::Refresh
            );
            assert!(recovery.resolve(Some(&page(rows))).is_none());
            assert!(!recovery.pending());
        }
    }
}
