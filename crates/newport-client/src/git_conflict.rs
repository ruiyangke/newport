//! Recorded conflict identities and agent-managed recovery capabilities.
use newport_protocol::git::{Action, ConflictSide, Page, StatusEntry, StatusSummary};

/// Resolve only an explicitly recorded side: an absent side is a recorded
/// deletion, while an entry without conflict metadata is never resolvable.
pub fn resolution(entry: &StatusEntry, side: &str) -> Option<Action> {
    if !entry.conflicted || entry.entry_id.is_empty() {
        return None;
    }
    let conflict = entry.conflict.as_ref()?;
    let (recorded, side) = match side {
        "ours" => (&conflict.ours, ConflictSide::Ours),
        "theirs" => (&conflict.theirs, ConflictSide::Theirs),
        "base" => (&conflict.base, ConflictSide::Base),
        _ => return None,
    };
    let oid = match recorded {
        None => None,
        Some(recorded) => {
            let oid = recorded.oid.hex.as_str();
            if !matches!(oid.len(), 40 | 64)
                || !oid.bytes().all(|b| b.is_ascii_hexdigit())
                || oid.bytes().all(|b| b == b'0')
            {
                return None;
            }
            Some(oid.to_owned())
        }
    };
    Some(Action::ConflictResolve {
        entry_ids: vec![entry.entry_id.clone()],
        side,
        expected_oid: oid,
    })
}

/// Visibility is separate from readiness: Continue can be supported yet
/// temporarily disabled while the user resolves conflicts.
pub fn recovery_supported(status: &Page<StatusEntry, StatusSummary>, kind: &str) -> bool {
    let Some(state) = &status.metadata.integration else {
        return false;
    };
    let flag = match kind {
        "integration.continue" => state.can_continue,
        "integration.abort" => state.can_abort,
        "integration.skip" => state.can_skip,
        _ => return false,
    };
    state.managed && flag
}

pub fn recovery_allowed(status: &Page<StatusEntry, StatusSummary>, kind: &str) -> bool {
    recovery_supported(status, kind)
        && (kind != "integration.continue" || status.entries.iter().all(|entry| !entry.conflicted))
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_protocol::git::{
        Conflict, ConflictEntry, GroupCounts, Head, IntegrationState, ObjectId, Path,
    };
    use serde_json::Value;
    fn resolution(entry: &StatusEntry, side: &str) -> Option<Value> {
        super::resolution(entry, side).map(|action| serde_json::to_value(action).unwrap())
    }
    fn entry(conflicted: bool) -> StatusEntry {
        StatusEntry {
            entry_id: "entry".into(),
            path: Path::new(b"file"),
            old_path: None,
            flags: 0,
            staged: false,
            unstaged: false,
            untracked: false,
            conflicted,
            conflict: None,
        }
    }

    #[test]
    fn resolution_requires_recorded_content_or_explicit_deletion() {
        for length in [40, 64] {
            let oid = "a".repeat(length);
            let recorded = |hex: &str| ConflictEntry {
                mode: 0o100644,
                oid: ObjectId {
                    algorithm: "sha1".into(),
                    hex: hex.into(),
                },
                path: Path::new(b"file"),
            };
            let mut entry = entry(true);
            assert!(resolution(&entry, "theirs").is_none());
            entry.conflict = Some(Conflict {
                base: Some(recorded("")),
                ours: Some(recorded(&oid)),
                theirs: None,
            });
            assert_eq!(resolution(&entry, "ours").unwrap()["expectedOid"], oid);
            assert!(resolution(&entry, "theirs").unwrap()["expectedOid"].is_null());
            assert!(resolution(&entry, "base").is_none());
            assert!(resolution(&entry, "working").is_none());
            for invalid in ["invalid".to_owned(), "0".repeat(length)] {
                entry.conflict.as_mut().unwrap().ours = Some(recorded(&invalid));
                assert!(resolution(&entry, "ours").is_none());
            }
            entry.conflicted = false;
            assert!(resolution(&entry, "theirs").is_none());
            entry.conflicted = true;
            entry.entry_id.clear();
            assert!(resolution(&entry, "theirs").is_none());
        }
    }

    #[test]
    fn recovery_preserves_capabilities_while_conflicts_block_continue() {
        let mut status = Page {
            snapshot: "s".into(),
            entries: vec![],
            next_cursor: None,
            metadata: StatusSummary {
                head: Head {
                    oid: None,
                    name: None,
                    detached: false,
                    unborn: true,
                },
                operation_state: "merge".into(),
                integration: Some(IntegrationState {
                    kind: "merge".into(),
                    managed: true,
                    can_continue: true,
                    can_abort: true,
                    can_skip: false,
                }),
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
        };
        assert!(recovery_allowed(&status, "integration.continue"));
        assert!(recovery_allowed(&status, "integration.abort"));
        assert!(!recovery_supported(&status, "integration.skip"));
        assert!(!recovery_supported(&status, "unknown"));
        status.entries = vec![entry(true)];
        assert!(recovery_supported(&status, "integration.continue"));
        assert!(!recovery_allowed(&status, "integration.continue"));
        assert!(recovery_allowed(&status, "integration.abort"));
        status.entries = vec![entry(false)];
        assert!(recovery_allowed(&status, "integration.continue"));
        status.metadata.integration.as_mut().unwrap().managed = false;
        assert!(!recovery_allowed(&status, "integration.abort"));
        assert!(!recovery_supported(&status, "integration.continue"));
    }
}
