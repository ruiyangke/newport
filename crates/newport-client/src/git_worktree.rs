//! Worktree identity, review guards and mutation inputs.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

/// Match the desktop form's branch-derived directory name and sibling layout.
pub fn suggested_name(branch: &str) -> String {
    let mut result = String::new();
    let mut separator = false;
    for ch in branch.trim().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            result.push(ch);
            separator = false;
        } else if !separator {
            result.push('-');
            separator = true;
        }
    }
    result.trim_matches(['-', '.']).chars().take(64).collect()
}

pub fn suggested_path(main_root: &str, name: &str) -> String {
    if name.is_empty() || !main_root.starts_with('/') {
        return String::new();
    }
    let root = main_root.trim_end_matches('/');
    let (parent, base) = root.rsplit_once('/').unwrap_or(("", "worktree"));
    format!("{parent}/{base}-{name}")
}

/// A user edit, including an intentional empty value, stops automatic updates.
#[derive(Default)]
pub struct SuggestedField {
    displayed: String,
    edited: bool,
}
impl SuggestedField {
    pub fn value(&self, current: &str, suggestion: String) -> String {
        if !self.edited && current == self.displayed {
            suggestion
        } else {
            current.to_owned()
        }
    }
    pub fn update(&mut self, current: &str, suggestion: String) -> Option<String> {
        if current != self.displayed {
            self.edited = true;
        }
        if self.edited || current == suggestion {
            return None;
        }
        self.displayed = suggestion.clone();
        Some(suggestion)
    }
}

pub struct CreateTarget<'a> {
    pub name: &'a str,
    pub path: &'a str,
    pub branch: &'a str,
    pub oid: &'a str,
    pub new_branch: bool,
    pub locked: bool,
}

#[derive(Default)]
pub struct NameReview {
    name: String,
    generation: u64,
    result: Option<Result<(), String>>,
}
impl NameReview {
    pub fn request(&mut self, name: &str) -> Option<u64> {
        if self.name == name {
            return None;
        }
        self.name = name.to_owned();
        self.generation += 1;
        self.result = None;
        (!name.is_empty()).then_some(self.generation)
    }
    pub fn complete(&mut self, generation: u64, name: &str, result: Result<(), String>) -> bool {
        if self.generation != generation || self.name != name {
            return false;
        }
        self.result = Some(result);
        true
    }
    pub fn retry(&mut self) {
        self.name.clear();
        self.generation += 1;
        self.result = None;
    }
    pub fn ready(&self, name: &str) -> bool {
        self.name == name && matches!(self.result, Some(Ok(())))
    }
    pub fn pending(&self, name: &str) -> bool {
        !name.is_empty() && (self.name != name || self.result.is_none())
    }
    pub fn error(&self) -> Option<&str> {
        self.result
            .as_ref()
            .and_then(|result| result.as_ref().err())
            .map(String::as_str)
    }
}

impl crate::backend::Backend {
    /// A narrow local-branch query, paged without retaining the full listing.
    /// This is a hint; Git still checks again when creating the worktree.
    pub fn check_worktree_branch(
        &self,
        server: &str,
        repo: &str,
        branch: &str,
    ) -> Result<(), String> {
        if branch.is_empty()
            || branch.len() > 1024
            || branch.starts_with('-')
            || branch.chars().any(char::is_control)
        {
            return Err("Enter a valid new branch name.".into());
        }
        check_branch_pages(branch, |cursor| {
            self.worktree_branches(server, repo, branch, cursor)
        })
    }

    /// Query one exact registration, not the currently loaded worktree page.
    /// The agent remains authoritative under its write lock after this hint.
    pub fn check_worktree_name(&self, server: &str, repo: &str, name: &str) -> Result<(), String> {
        if !valid_name(name) {
            return Err("Enter a worktree name without slashes or control characters.".into());
        }
        let page = self.call(
            "git_request",
            json!({"serverId":server,"request":{
                "method":"repo.worktrees","params":{"repoId":repo,"name":name,"pageSize":1}
            }}),
        )?;
        check_name_response(name, &page)
    }
}

fn check_branch_pages(
    branch: &str,
    mut load: impl FnMut(Option<&str>) -> Result<Value, String>,
) -> Result<(), String> {
    let expected = STANDARD.encode(format!("refs/heads/{branch}"));
    let mut cursor: Option<String> = None;
    let mut visited = std::collections::HashSet::new();
    let mut identity = None;
    loop {
        let page = load(cursor.as_deref())?;
        crate::git_branch::BranchChoices::from_page(&page)?;
        let current = (page["snapshot"].clone(), page["metadata"].clone());
        if identity
            .as_ref()
            .is_some_and(|previous| *previous != current)
        {
            return Err("Branches changed while checking this name. Check again.".into());
        }
        identity = Some(current);
        let rows = page["entries"]
            .as_array()
            .ok_or("Invalid branch name response.")?;
        if rows.len() > 20 || rows.iter().any(|entry| entry["remote"] != false) {
            return Err("Invalid local branch name response.".into());
        }
        if rows
            .iter()
            .any(|entry| entry["reference"]["bytesB64"] == expected)
        {
            return Err(
                "That branch already exists. Choose another name or use Existing branch.".into(),
            );
        }
        match page["nextCursor"].as_str() {
            None => return Ok(()),
            Some(next) if !rows.is_empty() && visited.insert(next.to_owned()) => {
                cursor = Some(next.to_owned())
            }
            _ => return Err("The branch name check did not advance. Check again.".into()),
        }
    }
}

fn check_name_response(name: &str, page: &Value) -> Result<(), String> {
    let rows = page["entries"]
        .as_array()
        .ok_or("Invalid worktree name response.")?;
    if rows.len() > 1 || page.get("nextCursor") != Some(&Value::Null) {
        return Err("Invalid worktree name response.".into());
    }
    if let Some(row) = rows.first() {
        if row["name"]["bytesB64"] != STANDARD.encode(name) {
            return Err("Worktree name check returned a different name. Check again.".into());
        }
        return Err("That worktree name already exists. Choose another name.".into());
    }
    Ok(())
}
impl CreateTarget<'_> {
    pub fn action(&self) -> Result<Value, String> {
        if !valid_name(self.name) {
            return Err("Enter a worktree name without slashes or control characters.".into());
        }
        if !self.path.starts_with('/')
            || self.path.contains('\0')
            || self.path.split('/').any(|part| matches!(part, "." | ".."))
            || self.path.ends_with('/')
        {
            return Err(
                "Choose an absolute destination under an existing parent directory.".into(),
            );
        }
        if self.branch.is_empty()
            || self.branch.starts_with('-')
            || self.branch.chars().any(char::is_control)
            || !matches!(self.oid.len(), 40 | 64)
            || !self.oid.bytes().all(|b| b.is_ascii_hexdigit())
            || self.oid.bytes().all(|b| b == b'0')
        {
            return Err("Select a branch and its complete reviewed commit.".into());
        }
        Ok(
            json!({"kind":"worktree.add","name":self.name,"path":{"display":self.path,"bytesB64":STANDARD.encode(self.path)},"branch":self.branch,"expectedOid":self.oid,"newBranch":self.new_branch,"locked":self.locked}),
        )
    }
}
/// Only known, byte-exact linked worktrees can be managed. Main/bare checkouts
/// and incomplete rows remain inspectable but never become mutation targets.
pub fn lock_target(entry: &Value) -> Option<(String, bool)> {
    let name = entry["name"]["display"].as_str()?;
    let locked = entry["locked"].as_bool()?;
    (entry["kind"] == "linked"
        && matches!(entry["state"].as_str(), Some("available" | "missing"))
        && valid_name(name)
        && entry["name"]["bytesB64"] == STANDARD.encode(name))
    .then(|| (name.to_owned(), locked))
}
pub fn removal_target(entry: &Value) -> Option<(String, bool)> {
    let (name, locked) = lock_target(entry)?;
    if locked || entry["current"] != false {
        return None;
    }
    Some((name, entry["state"] == "missing"))
}
pub fn review_matches(name: &str, snapshot: &str, selected: &Value, page: &Value) -> bool {
    !snapshot.is_empty()
        && page["snapshot"].as_str() == Some(snapshot)
        && lock_target(selected).is_some_and(|(current, _)| current == name)
        && page["entries"]
            .as_array()
            .is_some_and(|rows| rows.contains(selected))
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.contains('/')
        && !name.chars().any(char::is_control)
        && !matches!(name, "." | "..")
}
pub fn repair_path_valid(path: &str) -> bool {
    path.starts_with('/')
        && path != "/"
        && !path.contains('\0')
        && !path.split('/').any(|p| matches!(p, "." | ".."))
}
pub fn remove_action(name: &str, missing: bool) -> Result<Value, String> {
    if !valid_name(name) {
        return Err("Select a linked worktree with an exact name.".into());
    }
    // Never force, and never prune all registrations: one reviewed name only.
    Ok(json!({"kind":if missing {"worktree.prune"} else {"worktree.remove"},"name":name}))
}
pub fn repair_action(name: &str, path: &str) -> Result<Value, String> {
    if !valid_name(name) || !repair_path_valid(path) {
        return Err("Choose an absolute path to the moved linked checkout.".into());
    }
    Ok(
        json!({"kind":"worktree.repair","name":name,"path":{"display":path,"bytesB64":STANDARD.encode(path)}}),
    )
}
pub fn lock_action(name: &str, reason: &str, unlock: bool) -> Result<Value, String> {
    if !valid_name(name) {
        return Err("Select a linked worktree with an exact name.".into());
    }
    if !unlock && (reason.len() > 4096 || reason.contains('\0')) {
        return Err("Use a lock reason of at most 4096 bytes without null characters.".into());
    }
    let mut action =
        json!({"kind":if unlock {"worktree.unlock"} else {"worktree.lock"},"name":name});
    if !unlock && !reason.is_empty() {
        action["reason"] = json!(reason);
    }
    Ok(action)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn name_reviews_reject_out_of_order_results_and_require_current_success() {
        let mut review = NameReview::default();
        let a = review.request("a").unwrap();
        let b = review.request("b").unwrap();
        let again = review.request("a").unwrap();
        assert!(!review.complete(a, "a", Ok(())));
        assert!(!review.complete(b, "b", Ok(())));
        assert!(review.pending("a"));
        assert!(!review.ready("a"));
        assert!(review.complete(again, "a", Err("Disconnected".into())));
        assert_eq!(review.error(), Some("Disconnected"));
        assert!(!review.ready("a"));
        review.retry();
        assert!(!review.complete(again, "a", Ok(())));
        let retry = review.request("a").unwrap();
        assert!(review.complete(retry, "a", Ok(())));
        assert!(review.ready("a"));
        assert!(!review.ready("b"));
        assert!(review.pending("b"));
        assert_eq!(review.request(""), None);
        assert!(!review.complete(retry, "a", Ok(())));
        assert!(!review.pending(""));
        assert!(!review.ready(""));
    }

    fn branch_page(names: &[&str], cursor: Value) -> Value {
        json!({"snapshot":"branches", "metadata":{}, "nextCursor":cursor,
            "entries":names.iter().map(|name| json!({"remote":false,"reference":{"bytesB64":STANDARD.encode(format!("refs/heads/{name}"))}})).collect::<Vec<_>>()})
    }
    #[test]
    fn branch_name_check_uses_exact_case_sensitive_identity_beyond_the_first_page() {
        let mut calls = 0;
        let error = check_branch_pages("topic", |cursor| {
            calls += 1;
            Ok(if calls == 1 {
                assert!(cursor.is_none());
                branch_page(&["Topic", "topic-other"], json!("second"))
            } else {
                assert_eq!(cursor, Some("second"));
                branch_page(&["topic"], Value::Null)
            })
        })
        .unwrap_err();
        assert!(error.contains("already exists"));
        assert_eq!(calls, 2);
        assert!(
            check_branch_pages("topic", |_| Ok(branch_page(
                &["Topic", "topic-other"],
                Value::Null
            )))
            .is_ok()
        );
        assert!(check_branch_pages("开发", |_| Ok(branch_page(&["开发"], Value::Null))).is_err());
    }
    #[test]
    fn branch_name_check_rejects_incomplete_changing_and_looping_pages() {
        for invalid in [json!({}), branch_page(&[], json!("more"))] {
            assert!(check_branch_pages("topic", |_| Ok(invalid.clone())).is_err());
        }
        let mut calls = 0;
        assert!(
            check_branch_pages("topic", |_| {
                calls += 1;
                let mut page = branch_page(&["topic-other"], json!("same"));
                if calls == 2 {
                    page["snapshot"] = json!("changed");
                }
                Ok(page)
            })
            .unwrap_err()
            .contains("changed")
        );
        assert!(
            check_branch_pages("topic", |_| Ok(branch_page(
                &["topic-other"],
                json!("same")
            )))
            .unwrap_err()
            .contains("advance")
        );
        assert!(
            check_branch_pages("topic", |_| Err("Connection lost".into()))
                .unwrap_err()
                .contains("Connection lost")
        );
    }

    #[test]
    fn name_check_requires_a_complete_exact_registration_result() {
        let row = json!({"name":{"display":"topic","bytesB64":STANDARD.encode("topic")}});
        assert!(check_name_response("topic", &json!({"entries":[],"nextCursor":null})).is_ok());
        assert!(
            check_name_response("topic", &json!({"entries":[row],"nextCursor":null}))
                .unwrap_err()
                .contains("already exists")
        );
        for page in [
            json!({}),
            json!({"entries":[],"nextCursor":"more"}),
            json!({"entries":[row,row],"nextCursor":null}),
        ] {
            assert!(check_name_response("topic", &page).is_err());
        }
        assert!(
            check_name_response("other", &json!({"entries":[row],"nextCursor":null}))
                .unwrap_err()
                .contains("different name")
        );
    }
    #[test]
    fn suggestions_match_desktop_sibling_layout_and_names() {
        assert_eq!(suggested_name("  agent/fix login  "), "agent-fix-login");
        assert_eq!(suggested_name("...--agent///auth---.."), "agent-auth");
        assert_eq!(suggested_name("开发/工作"), "");
        assert_eq!(suggested_name(&"a".repeat(80)).len(), 64);
        assert_eq!(
            suggested_path("/srv/app/", "agent-auth"),
            "/srv/app-agent-auth"
        );
        assert_eq!(suggested_path("/app", "review"), "/app-review");
        assert_eq!(suggested_path("/", "review"), "/worktree-review");
        assert_eq!(suggested_path("/srv/app", ""), "");
        assert_eq!(suggested_path("relative", "review"), "");
    }

    #[test]
    fn suggestions_follow_the_branch_until_a_user_edits_or_clears_the_field() {
        let mut name = SuggestedField::default();
        assert_eq!(name.value("", "first".into()), "first");
        assert_eq!(name.update("", "first".into()), Some("first".into()));
        assert_eq!(name.update("first", "first".into()), None);
        assert_eq!(name.value("first", "second".into()), "second");
        assert_eq!(name.update("first", "second".into()), Some("second".into()));
        assert_eq!(name.update("custom", "third".into()), None);
        assert_eq!(name.update("custom", "fourth".into()), None);
        let mut cleared = SuggestedField::default();
        assert_eq!(
            cleared.update("", "generated".into()),
            Some("generated".into())
        );
        assert_eq!(cleared.update("", "another".into()), None);
        assert_eq!(cleared.update("", "later".into()), None);
    }
    #[test]
    fn management_actions_preserve_exact_scope_without_force_or_global_pruning() {
        assert_eq!(
            remove_action("topic", false).unwrap(),
            json!({"kind":"worktree.remove","name":"topic"})
        );
        assert_eq!(
            remove_action("topic", true).unwrap(),
            json!({"kind":"worktree.prune","name":"topic"})
        );
        let path = "/srv/开发 checkout";
        assert_eq!(
            repair_action("topic", path).unwrap(),
            json!({"kind":"worktree.repair","name":"topic","path":{"display":path,"bytesB64":STANDARD.encode(path)}})
        );
        assert_eq!(
            lock_action("topic", "Keep offline checkout", false).unwrap(),
            json!({"kind":"worktree.lock","name":"topic","reason":"Keep offline checkout"})
        );
        assert_eq!(
            lock_action("topic", "ignored old reason", true).unwrap(),
            json!({"kind":"worktree.unlock","name":"topic"})
        );
    }
    #[test]
    fn management_rejects_ambiguous_targets_and_invalid_reason_before_dispatch() {
        for name in ["", "..", "../other", "bad\0name", "bad\nname"] {
            assert!(remove_action(name, false).is_err());
            assert!(repair_action(name, "/srv/checkout").is_err());
            assert!(lock_action(name, "reason", false).is_err());
        }
        for path in [
            "/",
            "relative",
            "/srv/../other",
            "/srv/./checkout",
            "/srv/checkout\0bad",
        ] {
            assert!(repair_action("topic", path).is_err());
        }
        assert!(lock_action("topic", &"é".repeat(2048), false).is_ok());
        assert!(lock_action("topic", &"é".repeat(2049), false).is_err());
        assert!(lock_action("topic", "bad\0reason", false).is_err());
    }
    #[test]
    fn explicit_name_destination_and_protection_are_independent() {
        let target = CreateTarget {
            name: "review-copy",
            path: "/srv/my checkout",
            branch: "topic",
            oid: &"a".repeat(40),
            new_branch: false,
            locked: true,
        };
        let action = target.action().unwrap();
        assert_eq!(action["name"], "review-copy");
        assert_eq!(
            action["path"]["bytesB64"],
            STANDARD.encode("/srv/my checkout")
        );
        assert_eq!(action["locked"], true);
        assert_eq!(action["newBranch"], false);
        for name in ["", ".", "..", "bad/name", "bad\nname"] {
            assert!(CreateTarget { name, ..target }.action().is_err());
        }
        for path in ["/", "relative", "/srv/../work", "/srv/work/"] {
            assert!(CreateTarget { path, ..target }.action().is_err());
        }
        assert!(
            CreateTarget {
                oid: "partial",
                ..target
            }
            .action()
            .is_err()
        );
    }
}

#[cfg(test)]
mod review_tests {
    use super::{lock_target, removal_target, repair_path_valid, review_matches};
    #[test]
    fn removal_excludes_locked_current_and_unknown_worktrees() {
        let mut entry = serde_json::json!({"kind":"linked","name":{"display":"linked","bytesB64":"bGlua2Vk"},"locked":false,"current":false,"state":"available"});
        assert_eq!(removal_target(&entry), Some(("linked".into(), false)));
        entry["state"] = serde_json::json!("missing");
        assert_eq!(removal_target(&entry), Some(("linked".into(), true)));
        entry["locked"] = serde_json::json!(true);
        assert!(removal_target(&entry).is_none());
        entry["locked"] = serde_json::json!(false);
        entry["current"] = serde_json::json!(true);
        assert!(removal_target(&entry).is_none());
        entry["current"] = serde_json::json!(false);
        entry["state"] = serde_json::json!("unknown");
        assert!(removal_target(&entry).is_none());
    }
    #[test]
    fn repair_path_requires_absolute_unambiguous_identity() {
        assert!(repair_path_valid("/home/user/moved checkout"));
        for path in [
            "",
            "relative",
            "/home/../other",
            "/home/./other",
            "/work\0bad",
        ] {
            assert!(!repair_path_valid(path));
        }
    }
    use serde_json::json;

    #[test]
    fn lock_target_requires_exact_linked_worktree_identity() {
        let mut entry = json!({"kind":"linked","name":{"display":"linked","bytesB64":"bGlua2Vk"},"locked":false,"state":"available"});
        assert_eq!(lock_target(&entry), Some(("linked".into(), false)));
        entry["locked"] = json!(true);
        assert_eq!(lock_target(&entry), Some(("linked".into(), true)));
        entry["name"]["bytesB64"] = json!("b3RoZXI=");
        assert_eq!(lock_target(&entry), None);
        entry["name"]["bytesB64"] = json!("bGlua2Vk");
        entry["kind"] = json!("main");
        assert_eq!(lock_target(&entry), None);
        entry["kind"] = json!("linked");
        entry["locked"] = serde_json::Value::Null;
        assert_eq!(lock_target(&entry), None);
    }
    #[test]
    fn reviews_reject_changed_snapshot_target_and_removed_rows() {
        let entry = json!({"kind":"linked","name":{"display":"linked","bytesB64":"bGlua2Vk"},"locked":false,"state":"available"});
        let page = json!({"snapshot":"reviewed","entries":[entry.clone()]});
        assert!(review_matches("linked", "reviewed", &entry, &page));
        assert!(!review_matches("other", "reviewed", &entry, &page));
        assert!(!review_matches("linked", "old", &entry, &page));
        assert!(!review_matches(
            "linked",
            "reviewed",
            &entry,
            &json!({"snapshot":"reviewed","entries":[]})
        ));
        let mut unavailable = entry.clone();
        unavailable["state"] = json!("unknown");
        assert!(lock_target(&unavailable).is_none());
        assert!(!review_matches(
            "linked",
            "",
            &entry,
            &json!({"snapshot":"","entries":[entry]})
        ));
    }
}
