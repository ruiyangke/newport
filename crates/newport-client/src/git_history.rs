//! Immutable commit actions shared by desktop frontends.
use base64::Engine as _;
use serde_json::{Value, json};

/// Return an amend prefill only when the complete protocol bytes are lossless UTF-8.
pub fn complete_commit_message(detail: &Value) -> Option<String> {
    if detail["messageTruncated"] != false {
        return None;
    }
    let message = detail["message"]["display"].as_str()?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(detail["message"]["bytesB64"].as_str()?)
        .ok()?;
    (std::str::from_utf8(&bytes).ok()? == message).then(|| message.to_owned())
}
/// Build an action for the exact inspected commit. The UI comparison parent is
/// zero-based; Git replay mainline parents are one-based. Reset/amend callers
/// add the reviewed HEAD/message before dispatch through the guarded backend.
pub fn commit_action(kind: &str, detail: &Value, selected: &str, parent: usize) -> Option<Value> {
    let oid = detail["oid"]["hex"].as_str()?;
    let parents = detail["parents"].as_array()?;
    if !matches!(
        kind,
        "cherry_pick"
            | "revert"
            | "checkout"
            | "commit.amend"
            | "reset"
            | "merge"
            | "merge.fast_forward"
            | "rebase"
    ) || oid != selected
        || !matches!(oid.len(), 40 | 64)
        || !oid.bytes().all(|c| c.is_ascii_hexdigit())
        || oid.bytes().all(|c| c == b'0')
        || parent >= parents.len().max(1)
    {
        return None;
    }
    if kind == "rebase" {
        return Some(json!({"kind":"rebase","upstreamOid":oid}));
    }
    if matches!(kind, "merge" | "merge.fast_forward") {
        return Some(json!({"kind":kind,"targetOid":oid}));
    }
    if kind == "reset" {
        return Some(json!({"kind":"reset","targetOid":oid,"mode":"soft"}));
    }
    if kind == "commit.amend" {
        return Some(json!({"kind":"commit.amend","expectedOid":oid}));
    }
    if kind == "checkout" {
        return Some(json!({"kind":"checkout","target":{"kind":"detached","oid":oid}}));
    }
    let mut action = json!({"kind":kind,"targetOid":oid});
    if parents.len() > 1 {
        action["mainline"] = json!(parent + 1);
    }
    Some(action)
}

/// Captured merge parents for a replay review. Changing the comparison in a
/// different view must never substitute a new target or parent into this action.
#[derive(Clone, Debug)]
pub struct MainlineReview {
    target: String,
    parents: Vec<String>,
}
impl MainlineReview {
    pub fn from_commit(detail: &Value, selected: &str) -> Option<Self> {
        commit_action("cherry_pick", detail, selected, 0)?;
        let parents = detail["parents"].as_array()?;
        if parents.len() < 2 {
            return None;
        }
        let parents = parents
            .iter()
            .map(|parent| {
                let oid = parent["hex"].as_str()?;
                (oid.len() == selected.len()
                    && oid.bytes().all(|c| c.is_ascii_hexdigit())
                    && !oid.bytes().all(|c| c == b'0'))
                .then(|| oid.to_owned())
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            target: selected.to_owned(),
            parents,
        })
    }
    pub fn parents(&self) -> &[String] {
        &self.parents
    }
    pub fn selected_index(&self, action: &Value) -> Option<usize> {
        if !matches!(action["kind"].as_str(), Some("cherry_pick" | "revert"))
            || action["targetOid"].as_str() != Some(self.target.as_str())
        {
            return None;
        }
        let parent = usize::try_from(action["mainline"].as_u64()?)
            .ok()?
            .checked_sub(1)?;
        (parent < self.parents.len()).then_some(parent)
    }
    pub fn select_parent(&self, action: &mut Value, index: usize) -> bool {
        if self.selected_index(action).is_none() || index >= self.parents.len() {
            return false;
        }
        action["mainline"] = json!(index + 1);
        true
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn merge_replay_can_choose_each_captured_parent_without_changing_target() {
        let oid = "a".repeat(40);
        let parents = ['b', 'c', 'd'].map(|c| serde_json::json!({"hex":c.to_string().repeat(40)}));
        let detail = serde_json::json!({"oid":{"hex":oid},"parents":parents});
        let review = super::MainlineReview::from_commit(&detail, &oid).unwrap();
        for kind in ["cherry_pick", "revert"] {
            let mut action = super::commit_action(kind, &detail, &oid, 0).unwrap();
            for index in 0..3 {
                assert!(review.select_parent(&mut action, index));
                assert_eq!(review.selected_index(&action), Some(index));
                assert_eq!(action["mainline"], index + 1);
                assert_eq!(action["targetOid"], oid);
            }
            let saved = action.clone();
            assert!(!review.select_parent(&mut action, 3));
            assert_eq!(action, saved);
            action["targetOid"] = serde_json::json!("f".repeat(40));
            assert!(!review.select_parent(&mut action, 0));
        }
    }
    #[test]
    fn parent_choices_require_valid_merge_identity_and_replay_action() {
        let oid = "a".repeat(64);
        let mut detail = serde_json::json!({"oid":{"hex":oid},"parents":[{"hex":"b".repeat(64)},{"hex":"c".repeat(64)}]});
        let review = super::MainlineReview::from_commit(&detail, &oid).unwrap();
        for kind in ["merge", "rebase", "reset", "checkout", "commit.amend"] {
            let mut action = super::commit_action(kind, &detail, &oid, 0).unwrap();
            assert!(!review.select_parent(&mut action, 0));
        }
        for invalid in [
            serde_json::json!(0),
            serde_json::json!(3),
            serde_json::json!("1"),
            serde_json::Value::Null,
        ] {
            let action = serde_json::json!({"kind":"revert","targetOid":oid,"mainline":invalid});
            assert!(review.selected_index(&action).is_none());
        }
        for invalid in [
            "".to_owned(),
            "0".repeat(64),
            "b".repeat(40),
            "z".repeat(64),
        ] {
            detail["parents"][0]["hex"] = serde_json::json!(invalid);
            assert!(super::MainlineReview::from_commit(&detail, &oid).is_none());
        }
        detail["parents"] = serde_json::json!([]);
        assert!(super::MainlineReview::from_commit(&detail, &oid).is_none());
        assert!(super::commit_action("revert", &detail, &oid, 1).is_none());
        detail["parents"] = serde_json::json!([{"hex":"b".repeat(64)}]);
        assert!(super::MainlineReview::from_commit(&detail, &oid).is_none());
        assert!(super::commit_action("revert", &detail, &oid, 1).is_none());
    }
    #[test]
    fn history_integration_uses_captured_commit_without_replay_mainline() {
        let oid = "a".repeat(40);
        let detail = serde_json::json!({"oid":{"hex":oid},"parents":[{},{}]});
        for kind in ["merge", "merge.fast_forward"] {
            let action = super::commit_action(kind, &detail, &oid, 1).unwrap();
            assert_eq!(action, serde_json::json!({"kind":kind,"targetOid":oid}));
        }
        assert_eq!(
            super::commit_action("rebase", &detail, &oid, 1).unwrap(),
            serde_json::json!({"kind":"rebase","upstreamOid":oid})
        );
        assert!(super::commit_action("merge", &detail, &"b".repeat(40), 0).is_none());
    }
    #[test]
    fn amend_prefill_requires_complete_lossless_message() {
        let mut detail = serde_json::json!({"message":{"display":"hello","bytesB64":"aGVsbG8="},"messageTruncated":false});
        assert_eq!(
            super::complete_commit_message(&detail).as_deref(),
            Some("hello")
        );
        detail["messageTruncated"] = serde_json::json!(true);
        assert!(super::complete_commit_message(&detail).is_none());
        detail["messageTruncated"] = serde_json::json!(false);
        detail["message"]["bytesB64"] = serde_json::json!("/w==");
        assert!(super::complete_commit_message(&detail).is_none());
        detail["message"]["bytesB64"] = serde_json::json!("d29ybGQ=");
        assert!(super::complete_commit_message(&detail).is_none());
    }
    #[test]
    fn replay_captures_commit_and_one_based_mainline() {
        let oid = "a".repeat(40);
        let mut detail = serde_json::json!({"oid":{"hex":oid},"parents":[{},{}]});
        let action = super::commit_action("revert", &detail, &oid, 1).unwrap();
        assert_eq!(action["mainline"], 2);
        assert_eq!(action["targetOid"], oid);
        assert!(super::commit_action("revert", &detail, &oid, 2).is_none());
        assert!(super::commit_action("revert", &detail, &"b".repeat(40), 0).is_none());
        detail["parents"] = serde_json::json!([]);
        assert!(
            super::commit_action("cherry_pick", &detail, &oid, 0).unwrap()["mainline"].is_null()
        );
        assert_eq!(
            super::commit_action("reset", &detail, &oid, 0).unwrap()["mode"],
            "soft"
        );
        let checkout = super::commit_action("checkout", &detail, &oid, 0).unwrap();
        assert_eq!(
            checkout,
            serde_json::json!({"kind":"checkout","target":{"kind":"detached","oid":oid}})
        );
        assert!(super::commit_action("checkout", &detail, &"b".repeat(40), 0).is_none());
    }
}
