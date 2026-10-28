//! Plan a read-only return to the saved checkout after a linked folder vanishes.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::Value;

fn path_bytes(path: &Value) -> Option<Vec<u8>> {
    let bytes = STANDARD.decode(path["bytesB64"].as_str()?).ok()?;
    (bytes.starts_with(b"/") && !bytes.contains(&0)).then_some(bytes)
}

pub fn unavailable(code: Option<&str>) -> bool {
    matches!(code, Some("REPO_NOT_FOUND" | "CHECKOUT_UNAVAILABLE"))
}

/// Only an explicit unavailable-checkout code is evidence that navigation helps.
/// Compare the requested checkout to the bookmark, so a canonicalized symlink
/// or subdirectory of the saved project cannot be mistaken for another checkout.
pub fn destination(
    code: Option<&str>,
    server: &str,
    project: &Value,
    opened_path: &Value,
) -> Option<Value> {
    if !unavailable(code)
        || server.is_empty()
        || project["serverId"].as_str() != Some(server)
        || project["id"].as_str().is_none_or(str::is_empty)
    {
        return None;
    }
    let root = path_bytes(&project["path"])?;
    let opened = path_bytes(opened_path)?;
    (root != opened).then(|| project["path"].clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn path(bytes: &[u8]) -> Value {
        json!({"display":"same lossy display","bytesB64":STANDARD.encode(bytes)})
    }
    #[test]
    fn returns_exact_saved_bytes_only_for_a_missing_different_checkout() {
        let project = json!({"id":"project","serverId":"server","path":path(b"/project/\xff")});
        let linked = path(b"/linked/\xfe");
        assert_eq!(
            destination(Some("REPO_NOT_FOUND"), "server", &project, &linked),
            Some(project["path"].clone())
        );
        assert_eq!(
            destination(Some("CHECKOUT_UNAVAILABLE"), "server", &project, &linked),
            Some(project["path"].clone())
        );
        assert!(
            destination(Some("REPO_NOT_FOUND"), "server", &project, &project["path"]).is_none()
        );
        assert!(destination(Some("REPO_NOT_FOUND"), "another-server", &project, &linked).is_none());
        for code in [
            None,
            Some("AUTH_FAILED"),
            Some("OUTCOME_UNKNOWN"),
            Some("STALE_SNAPSHOT"),
            Some("REPO_REPLACED"),
            Some("IO_ERROR"),
            Some("GIT_UNAVAILABLE"),
        ] {
            assert!(destination(code, "server", &project, &linked).is_none());
        }
        for invalid in [
            path(b"relative"),
            path(b"/bad\0path"),
            json!({"display":"/valid","bytesB64":"invalid!"}),
        ] {
            assert!(destination(Some("REPO_NOT_FOUND"), "server", &project, &invalid).is_none());
            let mut bad = project.clone();
            bad["path"] = invalid;
            assert!(destination(Some("REPO_NOT_FOUND"), "server", &bad, &linked).is_none());
        }
    }
}
