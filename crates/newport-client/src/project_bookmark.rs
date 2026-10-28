//! Inputs for adding existing remote repositories to the local project library.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
/// Files currently accepts UTF-8 absolute paths. The wire bytes are the
/// identity; `display` may be lossy and must never be used to navigate.
pub fn files_path(project: &Value, server: &str) -> Option<String> {
    if project["serverId"].as_str() != Some(server) {
        return None;
    }
    let bytes = STANDARD
        .decode(project["path"]["bytesB64"].as_str()?)
        .ok()?;
    let path = String::from_utf8(bytes).ok()?;
    (path.starts_with('/') && !path.contains('\0')).then_some(path)
}

pub fn add_arguments(server: &str, name: &str, path: &str) -> Result<Value, String> {
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
    Ok(
        json!({"serverId":server,"name":name,"path":{"display":path,"bytesB64":STANDARD.encode(path)}}),
    )
}
/// Only a complete status read is evidence of a clean working tree.
#[derive(Debug, PartialEq)]
pub struct Status {
    pub branch: String,
    pub changes: u64,
    /// Comparison with locally stored upstream refs; never implies a fetch.
    pub ahead: Option<u64>,
}
pub fn status(value: &Value) -> Option<Status> {
    if value["truncated"] != false {
        return None;
    }
    let changes = value["totalEntries"].as_u64()?;
    let head = &value["head"];
    let branch = if head["detached"] == true {
        let oid = head["oid"]["hex"].as_str()?;
        format!("Detached {}", oid.chars().take(8).collect::<String>())
    } else {
        let name = head["name"]["display"].as_str()?;
        name.strip_prefix("refs/heads/").unwrap_or(name).to_owned()
    };
    Some(Status {
        branch,
        changes,
        ahead: value["ahead"].as_u64(),
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
    let status = resource.and_then(|r| status(&r.value));
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
    #[test]
    fn status_requires_complete_counts_and_handles_detached_and_unborn_heads() {
        let mut value = json!({"truncated":false,"totalEntries":0,"head":{"name":{"display":"refs/heads/main"},"detached":false}});
        assert_eq!(
            status(&value),
            Some(Status {
                branch: "main".into(),
                changes: 0,
                ahead: None,
            })
        );
        value["truncated"] = json!(true);
        assert_eq!(status(&value), None);
        value["truncated"] = json!(false);
        value["totalEntries"] = Value::Null;
        assert_eq!(status(&value), None);
        value["totalEntries"] = json!(2);
        value["head"] = json!({"detached":true,"oid":{"hex":"0123456789abcdef"}});
        assert_eq!(
            status(&value),
            Some(Status {
                branch: "Detached 01234567".into(),
                changes: 2,
                ahead: None,
            })
        );
        value["head"]["oid"] = Value::Null;
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
        let value = json!({"truncated":false,"totalEntries":0,"ahead":3,"head":{"name":{"display":"refs/heads/main"}}});
        resources.complete("status", initial, Ok(value.clone()));
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
        resources.complete("status", retry, Ok(value));
        assert!(!reading(resources.get("status")).previous);
    }

    #[test]
    fn background_observations_keep_cached_status_stable_and_errors_visible() {
        let mut resources = crate::resources::Resources::default();
        let value = json!({"truncated":false,"totalEntries":2,
            "head":{"name":{"display":"refs/heads/main"}},"ahead":3});
        let first = resources.begin("status").unwrap();
        resources.complete("status", first, Ok(value.clone()));
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
        resources.complete("status", retry, Ok(value));
        let recovered = reading(resources.get("status"));
        assert!(!recovered.pending && !recovered.previous && recovered.error.is_none());
    }

    #[test]
    fn absent_or_invalid_ahead_is_unknown_and_partial_status_is_not_clean() {
        let mut value = json!({"truncated":false,"totalEntries":0,"head":{"name":{"display":"refs/heads/main"}}});
        for ahead in [Value::Null, json!(-1), json!("3")] {
            value["ahead"] = ahead;
            assert_eq!(status(&value).unwrap().ahead, None);
        }
        value["ahead"] = json!(0);
        assert_eq!(status(&value).unwrap().ahead, Some(0));
        value["truncated"] = json!(true);
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
    fn files_navigation_uses_exact_bytes_and_rejects_invalid_or_foreign_paths() {
        let mut project = json!({"serverId":"server", "path":{"display":"/wrong", "bytesB64":STANDARD.encode("/home/日本語/project ")}});
        assert_eq!(
            files_path(&project, "server").as_deref(),
            Some("/home/日本語/project ")
        );
        assert!(files_path(&project, "other").is_none());
        for bytes in [b"relative".as_slice(), b"/nul\0path", b"/invalid\xff"] {
            project["path"]["bytesB64"] = json!(STANDARD.encode(bytes));
            assert!(files_path(&project, "server").is_none());
        }
        project["path"]["bytesB64"] = json!("invalid base64");
        assert!(files_path(&project, "server").is_none());
    }
    #[test]
    fn adding_a_bookmark_preserves_path_bytes_and_explicit_display_name() {
        let args = add_arguments("server", " Project label ", "/srv/开发 repository ").unwrap();
        assert_eq!(args["name"], "Project label");
        assert_eq!(args["path"]["display"], "/srv/开发 repository ");
        assert_eq!(
            args["path"]["bytesB64"],
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
