//! Immutable commit actions.
use base64::Engine as _;
use newport_protocol::git::{Action, CheckoutTarget, ResetMode};
use serde_json::Value;

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
pub fn commit_action(kind: &str, detail: &Value, selected: &str, parent: usize) -> Option<Action> {
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
    let target = oid.to_owned();
    let mainline = if parents.len() > 1 {
        u32::try_from(parent.checked_add(1)?).ok()?
    } else {
        0
    };
    Some(match kind {
        "rebase" => Action::Rebase {
            upstream_oid: target,
            onto_oid: None,
            committer: None,
        },
        "merge" => Action::Merge { target_oid: target },
        "merge.fast_forward" => Action::FastForward { target_oid: target },
        "reset" => Action::Reset {
            target_oid: target,
            expected_oid: String::new(),
            mode: ResetMode::Soft,
        },
        "commit.amend" => Action::Amend {
            expected_oid: target,
            message: String::new(),
            committer: None,
            author: None,
        },
        "checkout" => Action::Checkout {
            target: CheckoutTarget::Detached { oid: target },
        },
        "cherry_pick" => Action::CherryPick {
            target_oid: target,
            mainline,
            author: None,
        },
        "revert" => Action::Revert {
            target_oid: target,
            mainline,
            author: None,
        },
        _ => return None,
    })
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
    pub fn selected_index(&self, action: &Action) -> Option<usize> {
        let (target_oid, mainline) = match action {
            Action::CherryPick {
                target_oid,
                mainline,
                ..
            }
            | Action::Revert {
                target_oid,
                mainline,
                ..
            } => (target_oid, *mainline),
            _ => return None,
        };
        if target_oid != &self.target {
            return None;
        }
        let parent = usize::try_from(mainline).ok()?.checked_sub(1)?;
        (parent < self.parents.len()).then_some(parent)
    }
    pub fn select_parent(&self, action: &mut Action, index: usize) -> bool {
        if self.selected_index(action).is_none() || index >= self.parents.len() {
            return false;
        }
        let Some(parent) = index
            .checked_add(1)
            .and_then(|value| u32::try_from(value).ok())
        else {
            return false;
        };
        match action {
            Action::CherryPick { mainline, .. } | Action::Revert { mainline, .. } => {
                *mainline = parent
            }
            _ => return false,
        }
        true
    }
}
#[cfg(test)]
mod tests {
    use super::{Action, CheckoutTarget};
    fn view(action: &Action) -> serde_json::Value {
        serde_json::to_value(action).unwrap()
    }

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
                assert_eq!(view(&action)["mainline"], index + 1);
                assert_eq!(view(&action)["targetOid"], oid);
            }
            let saved = action.clone();
            assert!(!review.select_parent(&mut action, 3));
            assert_eq!(action, saved);
            match &mut action {
                Action::CherryPick { target_oid, .. } | Action::Revert { target_oid, .. } => {
                    *target_oid = "f".repeat(40)
                }
                _ => unreachable!(),
            }
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
        for mainline in [0, 3, u32::MAX] {
            let action = Action::Revert {
                target_oid: oid.clone(),
                mainline,
                author: None,
            };
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
            assert_eq!(
                view(&action),
                serde_json::json!({"kind":kind,"targetOid":oid})
            );
        }
        assert_eq!(
            super::commit_action("rebase", &detail, &oid, 1).unwrap(),
            Action::Rebase {
                upstream_oid: oid.clone(),
                onto_oid: None,
                committer: None
            }
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
        assert_eq!(view(&action)["mainline"], 2);
        assert_eq!(view(&action)["targetOid"], oid);
        assert!(super::commit_action("revert", &detail, &oid, 2).is_none());
        assert!(super::commit_action("revert", &detail, &"b".repeat(40), 0).is_none());
        detail["parents"] = serde_json::json!([]);
        assert!(matches!(
            super::commit_action("cherry_pick", &detail, &oid, 0).unwrap(),
            Action::CherryPick { mainline: 0, .. }
        ));
        assert_eq!(
            view(&super::commit_action("reset", &detail, &oid, 0).unwrap())["mode"],
            "soft"
        );
        let checkout = super::commit_action("checkout", &detail, &oid, 0).unwrap();
        assert_eq!(
            checkout,
            Action::Checkout {
                target: CheckoutTarget::Detached { oid: oid.clone() }
            }
        );
        assert!(super::commit_action("checkout", &detail, &"b".repeat(40), 0).is_none());
    }
}
