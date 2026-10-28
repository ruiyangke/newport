//! Recorded conflict identities and agent-managed recovery capabilities.
use serde_json::{Value, json};

/// Resolve only an explicitly recorded side; null means a recorded deletion,
/// while missing metadata must never be interpreted as permission to delete.
pub fn resolution(entry: &Value, side: &str) -> Option<Value> {
    if !matches!(side, "ours" | "theirs" | "base") || entry["conflicted"] != true {
        return None;
    }
    let id = entry["entryId"].as_str().filter(|id| !id.is_empty())?;
    let recorded = entry["conflict"].as_object()?.get(side)?;
    let oid = if recorded.is_null() {
        Value::Null
    } else {
        let oid = recorded["oid"]["hex"].as_str()?;
        if !matches!(oid.len(), 40 | 64)
            || !oid.bytes().all(|b| b.is_ascii_hexdigit())
            || oid.bytes().all(|b| b == b'0')
        {
            return None;
        }
        json!(oid)
    };
    Some(json!({"kind":"conflict.resolve","entryIds":[id],"side":side,"expectedOid":oid}))
}

/// Visibility is separate from readiness: Continue can be supported yet
/// temporarily disabled while the user resolves conflicts.
pub fn recovery_supported(status: &Value, kind: &str) -> bool {
    let state = &status["metadata"]["integration"];
    let flag = match kind {
        "integration.continue" => "canContinue",
        "integration.abort" => "canAbort",
        "integration.skip" => "canSkip",
        _ => return false,
    };
    state["managed"] == true && state[flag] == true
}

pub fn recovery_allowed(status: &Value, kind: &str) -> bool {
    recovery_supported(status, kind)
        && (kind != "integration.continue"
            || status["entries"]
                .as_array()
                .is_some_and(|entries| entries.iter().all(|entry| entry["conflicted"] == false)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_requires_recorded_content_or_explicit_deletion() {
        for length in [40, 64] {
            let oid = "a".repeat(length);
            let mut entry = json!({"entryId":"entry","conflicted":true,"conflict":{"ours":{"oid":{"hex":oid}},"theirs":null}});
            assert_eq!(resolution(&entry, "ours").unwrap()["expectedOid"], oid);
            assert!(resolution(&entry, "theirs").unwrap()["expectedOid"].is_null());
            assert!(resolution(&entry, "base").is_none());
            assert!(resolution(&entry, "working").is_none());
            for invalid in ["invalid".to_owned(), "0".repeat(length)] {
                entry["conflict"]["ours"]["oid"]["hex"] = json!(invalid);
                assert!(resolution(&entry, "ours").is_none());
            }
            entry["conflicted"] = json!(false);
            assert!(resolution(&entry, "theirs").is_none());
            entry["conflicted"] = json!(true);
            entry["entryId"] = json!("");
            assert!(resolution(&entry, "theirs").is_none());
        }
    }

    #[test]
    fn recovery_preserves_capabilities_while_conflicts_block_continue() {
        let mut status = json!({"metadata":{"integration":{"managed":true,"canContinue":true,"canAbort":true,"canSkip":false}},"entries":[]});
        assert!(recovery_allowed(&status, "integration.continue"));
        assert!(recovery_allowed(&status, "integration.abort"));
        assert!(!recovery_supported(&status, "integration.skip"));
        assert!(!recovery_supported(&status, "unknown"));
        for entries in [json!([{"conflicted":true}]), json!([{}]), Value::Null] {
            status["entries"] = entries;
            assert!(recovery_supported(&status, "integration.continue"));
            assert!(!recovery_allowed(&status, "integration.continue"));
            assert!(recovery_allowed(&status, "integration.abort"));
        }
        status["entries"] = json!([{"conflicted":false}]);
        assert!(recovery_allowed(&status, "integration.continue"));
        status["metadata"]["integration"]["managed"] = json!(false);
        assert!(!recovery_allowed(&status, "integration.abort"));
        assert!(!recovery_supported(&status, "integration.continue"));
    }
}
