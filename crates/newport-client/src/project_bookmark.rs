//! Inputs for adding existing remote repositories to the local project library.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use newport_protocol::git::{Reply, StatusSummary};
/// Files currently accepts UTF-8 absolute paths. The wire bytes are the
/// identity; `display` may be lossy and must never be used to navigate.
pub fn files_path(path: &newport_protocol::git::Path) -> Option<String> {
    let bytes = STANDARD.decode(&path.bytes_b64).ok()?;
    let path = String::from_utf8(bytes).ok()?;
    (path.starts_with('/') && !path.contains('\0')).then_some(path)
}

#[derive(Clone, Debug)]
pub struct Add {
    pub server: uuid::Uuid,
    pub name: String,
    pub path: newport_protocol::git::Path,
}
pub fn add_arguments(server: &str, name: &str, path: &str) -> Result<Add, String> {
    let name = name.trim();
    if server.is_empty() {
        return Err("Select a server first.".into());
    }
    if name.is_empty() {
        return Err("Enter a project name.".into());
    }
    if name.len() > 256 || name.chars().any(char::is_control) {
        return Err("Enter a project name of up to 256 bytes without control characters.".into());
    }
    if !path.starts_with('/') || path.contains('\0') {
        return Err("Enter an absolute repository path on this server.".into());
    }
    Ok(Add {
        server: uuid::Uuid::parse_str(server)
            .map_err(|_| "Select a valid server first.".to_owned())?,
        name: name.into(),
        path: newport_protocol::git::Path::new(path.as_bytes()),
    })
}
/// Only a complete status read is evidence of a clean working tree.
#[derive(Debug, PartialEq)]
pub struct Status {
    pub branch: String,
    pub changes: u64,
    /// Comparison with locally stored upstream refs; never implies a fetch.
    pub ahead: Option<u64>,
}
pub fn status(value: &StatusSummary) -> Option<Status> {
    if value.truncated {
        return None;
    }
    let head = &value.head;
    let branch = if head.detached {
        let oid = &head.oid.as_ref()?.hex;
        format!("Detached {}", oid.chars().take(8).collect::<String>())
    } else {
        let name = head.name.as_ref()?.display.as_str();
        name.strip_prefix("refs/heads/").unwrap_or(name).to_owned()
    };
    Some(Status {
        branch,
        changes: value.total_entries,
        ahead: value.ahead,
    })
}

/// A cached successful value is useful during a retry, but is not evidence that
/// the most recent status check succeeded.
pub struct Reading<'a> {
    pub status: Option<Status>,
    pub pending: bool,
    pub error: Option<&'a str>,
    pub previous: bool,
}
pub fn reading(resource: Option<&crate::resources::Resource>) -> Reading<'_> {
    let status = resource.and_then(|r| match r.git()? {
        Reply::StatusSummary(summary) => status(summary),
        _ => None,
    });
    let pending = resource.is_some_and(|r| r.display_pending());
    let error = resource.and_then(|r| r.error.as_deref());
    let previous = status.is_some() && (pending || error.is_some());
    Reading {
        status,
        pending,
        error,
        previous,
    }
}

#[cfg(test)]
mod status_tests {
    use super::*;
    use newport_protocol::git::{GroupCounts, Head, ObjectId, Path};
    fn summary(total: u64, ahead: Option<u64>) -> StatusSummary {
        StatusSummary {
            head: Head {
                oid: None,
                name: Some(Path::new(b"refs/heads/main")),
                detached: false,
                unborn: false,
            },
            operation_state: "none".into(),
            integration: None,
            ahead,
            behind: None,
            basis: "local".into(),
            upstream_ref: None,
            upstream: None,
            truncated: false,
            total_entries: total,
            group_counts: GroupCounts {
                staged: 0,
                unstaged: 0,
                untracked: 0,
                conflicted: 0,
            },
            matched_entries: None,
        }
    }
    fn reply(value: &StatusSummary) -> Result<Reply, crate::request_error::RequestError> {
        Ok(Reply::StatusSummary(Box::new(value.clone())))
    }
    #[test]
    fn status_requires_complete_counts_and_handles_detached_and_unborn_heads() {
        let mut value = summary(0, None);
        assert_eq!(
            status(&value),
            Some(Status {
                branch: "main".into(),
                changes: 0,
                ahead: None,
            })
        );
        value.truncated = true;
        assert_eq!(status(&value), None);
        value.truncated = false;
        value.total_entries = 2;
        value.head = Head {
            oid: Some(ObjectId {
                algorithm: "sha1".into(),
                hex: "0123456789abcdef".into(),
            }),
            name: None,
            detached: true,
            unborn: false,
        };
        assert_eq!(
            status(&value),
            Some(Status {
                branch: "Detached 01234567".into(),
                changes: 2,
                ahead: None,
            })
        );
        value.head.oid = None;
        assert_eq!(status(&value), None);
    }

    #[test]
    fn failed_refresh_keeps_facts_but_cannot_present_them_as_current() {
        let mut resources = crate::resources::Resources::default();
        let initial = resources.begin("status").unwrap();
        let first = reading(resources.get("status"));
        assert!(first.pending);
        assert!(!first.previous);
        assert!(first.status.is_none());
        let value = summary(0, Some(3));
        resources.complete_git("status", initial, reply(&value));
        let ready = reading(resources.get("status"));
        assert!(!ready.pending && !ready.previous && ready.error.is_none());
        assert_eq!(ready.status.unwrap().ahead, Some(3));
        let refresh = resources.begin_refresh("status", true).unwrap();
        let pending = reading(resources.get("status"));
        assert!(pending.pending && pending.previous);
        assert_eq!(pending.status.unwrap().changes, 0);
        resources.complete("status", refresh, Err("Disconnected".into()));
        let failed = reading(resources.get("status"));
        assert!(failed.previous && !failed.pending);
        assert_eq!(failed.error, Some("Disconnected"));
        assert_eq!(failed.status.unwrap().ahead, Some(3));
        let retry = resources.begin_refresh("status", true).unwrap();
        assert!(reading(resources.get("status")).previous);
        resources.complete_git("status", retry, reply(&value));
        assert!(!reading(resources.get("status")).previous);
    }

    #[test]
    fn background_observations_keep_cached_status_stable_and_errors_visible() {
        let mut resources = crate::resources::Resources::default();
        let value = summary(2, Some(3));
        let first = resources.begin("status").unwrap();
        resources.complete_git("status", first, reply(&value));
        let background = resources.begin_refresh("status", false).unwrap();
        let cached = reading(resources.get("status"));
        assert!(!cached.pending && !cached.previous);
        assert_eq!(cached.status.unwrap().ahead, Some(3));
        resources.complete("status", background, Err("Offline".into()));
        let retry = resources.begin_refresh("status", false).unwrap();
        let failed = reading(resources.get("status"));
        assert!(!failed.pending && failed.previous);
        assert_eq!(failed.error, Some("Offline"));
        // A manual refresh joins this read and restores visible activity.
        assert!(resources.begin_refresh("status", true).is_none());
        assert!(reading(resources.get("status")).pending);
        resources.complete_git("status", retry, reply(&value));
        let recovered = reading(resources.get("status"));
        assert!(!recovered.pending && !recovered.previous && recovered.error.is_none());
    }

    #[test]
    fn absent_ahead_is_unknown_and_partial_status_is_not_clean() {
        let mut value = summary(0, None);
        assert_eq!(status(&value).unwrap().ahead, None);
        value.ahead = Some(0);
        assert_eq!(status(&value).unwrap().ahead, Some(0));
        value.truncated = true;
        assert!(status(&value).is_none());
        let mut resources = crate::resources::Resources::default();
        let request = resources.begin("status").unwrap();
        resources.complete("status", request, Err("Not a repository".into()));
        let failed = reading(resources.get("status"));
        assert!(failed.status.is_none() && !failed.previous);
        assert_eq!(failed.error, Some("Not a repository"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn files_navigation_uses_exact_bytes_and_rejects_invalid_paths() {
        let mut path = newport_protocol::git::Path {
            bytes_b64: STANDARD.encode("/home/日本語/project "),
            display: "/wrong".into(),
        };
        assert_eq!(files_path(&path).as_deref(), Some("/home/日本語/project "));
        for bytes in [b"relative".as_slice(), b"/nul\0path", b"/invalid\xff"] {
            path.bytes_b64 = STANDARD.encode(bytes);
            assert!(files_path(&path).is_none());
        }
        path.bytes_b64 = "invalid base64".into();
        assert!(files_path(&path).is_none());
    }
    #[test]
    fn adding_a_bookmark_preserves_path_bytes_and_explicit_display_name() {
        let args = add_arguments(
            "11111111-1111-4111-8111-111111111111",
            " Project label ",
            "/srv/开发 repository ",
        )
        .unwrap();
        assert_eq!(args.name, "Project label");
        assert_eq!(args.path.display, "/srv/开发 repository ");
        assert_eq!(
            args.path.bytes_b64,
            STANDARD.encode("/srv/开发 repository ")
        );
        for path in ["", "relative", "/srv/invalid\0name"] {
            assert!(add_arguments("s", "name", path).is_err());
        }
        for name in ["", " ", "bad\nname", &"é".repeat(129)] {
            assert!(add_arguments("s", name, "/repo").is_err());
        }
        assert!(add_arguments("", "name", "/repo").is_err());
    }
}
