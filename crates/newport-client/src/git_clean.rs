//! Classify a complete status read and bind clean-checkout details to its HEAD.
use crate::resources::Resource;
use serde_json::Value;

#[derive(Debug, PartialEq, Eq)]
pub enum WorkingTree {
    Loading,
    Unavailable,
    Clean,
    Select,
}
impl WorkingTree {
    pub fn for_page(page: &Value, resource: Option<&Resource>, invalid: bool) -> Self {
        let Some(resource) = resource else {
            return Self::Unavailable;
        };
        if invalid || resource.error.is_some() {
            Self::Unavailable
        } else if resource.pending {
            Self::Loading
        } else if page["entries"].as_array().is_some_and(Vec::is_empty) {
            if page["metadata"]["truncated"] == true
                || page["nextCursor"].is_string()
                || page["metadata"]["totalEntries"]
                    .as_u64()
                    .is_some_and(|total| total > 0)
            {
                Self::Unavailable
            } else {
                Self::Clean
            }
        } else if page["entries"].is_array() {
            Self::Select
        } else {
            Self::Unavailable
        }
    }
}

pub fn head_oid(status: &Value) -> Option<&str> {
    status["metadata"]["head"]["oid"]["hex"]
        .as_str()
        .filter(|oid| matches!(oid.len(), 40 | 64) && oid.bytes().all(|c| c.is_ascii_hexdigit()))
}

pub fn matching_commit<'a>(status: &Value, commit: &'a Value) -> Option<&'a Value> {
    let oid = head_oid(status)?;
    (commit["oid"]["hex"].as_str() == Some(oid)).then_some(commit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::Resources;
    use serde_json::json;

    #[test]
    fn commit_details_must_match_the_current_head() {
        for length in [40, 64] {
            let oid = "a".repeat(length);
            let status = json!({"metadata":{"head":{"oid":{"hex":oid}}}});
            let commit = json!({"oid":{"hex":oid},"message":{"display":"subject"}});
            assert_eq!(head_oid(&status), Some(oid.as_str()));
            assert_eq!(matching_commit(&status, &commit), Some(&commit));
            let stale = json!({"oid":{"hex":"b".repeat(length)}});
            assert!(matching_commit(&status, &stale).is_none());
        }
        for oid in ["", "HEAD", "abc", &"g".repeat(40)] {
            let status = json!({"metadata":{"head":{"oid":{"hex":oid}}}});
            assert!(head_oid(&status).is_none());
            assert!(matching_commit(&status, &json!({"oid":{"hex":oid}})).is_none());
        }
        let unborn = json!({"metadata":{"head":{"unborn":true}}});
        assert!(head_oid(&unborn).is_none());
        assert!(matching_commit(&unborn, &Value::Null).is_none());
    }

    #[test]
    fn refresh_and_partial_reads_cannot_claim_clean() {
        let mut resources = Resources::default();
        let clean = json!({"entries":[],"metadata":{"totalEntries":0},"nextCursor":null});
        let first = resources.begin("status").unwrap();
        assert_eq!(
            WorkingTree::for_page(&clean, resources.get("status"), false),
            WorkingTree::Loading
        );
        resources.complete("status", first, Ok(clean.clone()));
        assert_eq!(
            WorkingTree::for_page(&clean, resources.get("status"), false),
            WorkingTree::Clean
        );
        for page in [
            json!({"entries":[],"nextCursor":"more"}),
            json!({"entries":[],"metadata":{"truncated":true}}),
            json!({"entries":[],"metadata":{"totalEntries":3}}),
            Value::Null,
        ] {
            assert_eq!(
                WorkingTree::for_page(&page, resources.get("status"), false),
                WorkingTree::Unavailable
            );
        }
        let refresh = resources.begin("status").unwrap();
        assert_eq!(
            WorkingTree::for_page(&clean, resources.get("status"), false),
            WorkingTree::Loading
        );
        resources.complete("status", refresh, Err("Disconnected".into()));
        assert_eq!(
            WorkingTree::for_page(&clean, resources.get("status"), false),
            WorkingTree::Unavailable
        );
    }
}
