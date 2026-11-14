//! Classify a complete status read and bind clean-checkout details to its HEAD.
use crate::resources::Resource;
use newport_protocol::git::{Commit, Page, StatusEntry, StatusSummary};

type StatusPage = Page<StatusEntry, StatusSummary>;

#[derive(Debug, PartialEq, Eq)]
pub enum WorkingTree {
    Loading,
    Unavailable,
    Clean,
    Select,
}
impl WorkingTree {
    pub fn for_page(page: Option<&StatusPage>, resource: Option<&Resource>, invalid: bool) -> Self {
        let Some(resource) = resource else {
            return Self::Unavailable;
        };
        if invalid || resource.error.is_some() {
            Self::Unavailable
        } else if resource.pending {
            Self::Loading
        } else if let Some(page) = page {
            if !page.entries.is_empty() {
                Self::Select
            } else if page.metadata.truncated
                || page.next_cursor.is_some()
                || page.metadata.total_entries > 0
            {
                Self::Unavailable
            } else {
                Self::Clean
            }
        } else {
            Self::Unavailable
        }
    }
}

pub fn head_oid(status: &StatusPage) -> Option<&str> {
    status
        .metadata
        .head
        .oid
        .as_ref()
        .map(|oid| oid.hex.as_str())
        .filter(|oid| matches!(oid.len(), 40 | 64) && oid.bytes().all(|c| c.is_ascii_hexdigit()))
}

pub fn matching_commit<'a>(status: &StatusPage, commit: &'a Commit) -> Option<&'a Commit> {
    let oid = head_oid(status)?;
    (commit.oid.hex == oid).then_some(commit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::Resources;
    use newport_protocol::git::{Author, GroupCounts, Head, ObjectId, Path};

    fn status(oid: Option<&str>) -> StatusPage {
        Page {
            snapshot: "s".into(),
            entries: vec![],
            next_cursor: None,
            metadata: StatusSummary {
                head: Head {
                    oid: oid.map(|hex| ObjectId {
                        algorithm: "sha1".into(),
                        hex: hex.into(),
                    }),
                    name: None,
                    detached: false,
                    unborn: oid.is_none(),
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
    fn commit(oid: &str) -> Commit {
        Commit {
            oid: ObjectId {
                algorithm: "sha1".into(),
                hex: oid.into(),
            },
            parents: vec![],
            message: Path::new(b"subject"),
            message_truncated: false,
            author: Author {
                name: "Author".into(),
                email: "author@example.test".into(),
            },
            time: 1,
            offset_minutes: 0,
        }
    }

    #[test]
    fn commit_details_must_match_the_current_head() {
        for length in [40, 64] {
            let oid = "a".repeat(length);
            let status = status(Some(&oid));
            let current = commit(&oid);
            assert_eq!(head_oid(&status), Some(oid.as_str()));
            assert_eq!(matching_commit(&status, &current), Some(&current));
            assert!(matching_commit(&status, &commit(&"b".repeat(length))).is_none());
        }
        for oid in ["", "HEAD", "abc", &"g".repeat(40)] {
            let status = status(Some(oid));
            assert!(head_oid(&status).is_none());
            assert!(matching_commit(&status, &commit(oid)).is_none());
        }
        let unborn = status(None);
        assert!(head_oid(&unborn).is_none());
        assert!(matching_commit(&unborn, &commit("")).is_none());
    }

    #[test]
    fn refresh_and_partial_reads_cannot_claim_clean() {
        let mut resources = Resources::default();
        let clean = status(None);
        let first = resources.begin("status").unwrap();
        assert_eq!(
            WorkingTree::for_page(Some(&clean), resources.get("status"), false),
            WorkingTree::Loading
        );
        resources.complete_owned("status", first, Ok(true));
        assert_eq!(
            WorkingTree::for_page(Some(&clean), resources.get("status"), false),
            WorkingTree::Clean
        );
        let mut more = clean.clone();
        more.next_cursor = Some("more".into());
        let mut truncated = clean.clone();
        truncated.metadata.truncated = true;
        let mut counted = clean.clone();
        counted.metadata.total_entries = 3;
        for page in [Some(&more), Some(&truncated), Some(&counted), None] {
            assert_eq!(
                WorkingTree::for_page(page, resources.get("status"), false),
                WorkingTree::Unavailable
            );
        }
        let refresh = resources.begin("status").unwrap();
        assert_eq!(
            WorkingTree::for_page(Some(&clean), resources.get("status"), false),
            WorkingTree::Loading
        );
        resources.complete("status", refresh, Err("Disconnected".into()));
        assert_eq!(
            WorkingTree::for_page(Some(&clean), resources.get("status"), false),
            WorkingTree::Unavailable
        );
    }
}
