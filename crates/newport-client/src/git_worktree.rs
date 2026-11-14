//! Worktree identity, review guards and mutation inputs.
use crate::git_branch::BranchPage;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use newport_protocol::git::{Action, Page, Path, Reply, Request, Worktree, WorktreeMetadata};

/// One page of a worktree listing.
pub type WorktreePage = Page<Worktree, WorktreeMetadata>;

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
        let server = uuid::Uuid::parse_str(server).map_err(|error| error.to_string())?;
        let reply = self
            .git(
                server,
                Request::Worktrees {
                    repo_id: repo.into(),
                    name: Some(name.into()),
                    page_size: 1,
                    cursor: None,
                    filter: String::new(),
                    branch: None,
                    at_snapshot: None,
                },
                false,
            )
            .map_err(|error| error.message)?;
        let Reply::Worktrees(page) = reply else {
            return Err("Invalid worktree name response.".into());
        };
        check_name_response(name, &page)
    }
}

fn check_branch_pages(
    branch: &str,
    mut load: impl FnMut(Option<&str>) -> Result<BranchPage, String>,
) -> Result<(), String> {
    let expected = STANDARD.encode(format!("refs/heads/{branch}"));
    let mut cursor: Option<String> = None;
    let mut visited = std::collections::HashSet::new();
    let mut identity = None;
    loop {
        let page = load(cursor.as_deref())?;
        crate::git_branch::BranchChoices::from_page(&page)?;
        let current = (page.snapshot.clone(), page.metadata.clone());
        if identity
            .as_ref()
            .is_some_and(|previous| *previous != current)
        {
            return Err("Branches changed while checking this name. Check again.".into());
        }
        identity = Some(current);
        let rows = &page.entries;
        if rows.len() > 20 || rows.iter().any(|entry| entry.remote) {
            return Err("Invalid local branch name response.".into());
        }
        if rows
            .iter()
            .any(|entry| entry.reference.bytes_b64 == expected)
        {
            return Err(
                "That branch already exists. Choose another name or use Existing branch.".into(),
            );
        }
        match page.next_cursor.as_deref() {
            None => return Ok(()),
            Some(next) if !rows.is_empty() && visited.insert(next.to_owned()) => {
                cursor = Some(next.to_owned())
            }
            _ => return Err("The branch name check did not advance. Check again.".into()),
        }
    }
}

fn check_name_response(name: &str, page: &WorktreePage) -> Result<(), String> {
    let rows = &page.entries;
    if rows.len() > 1 || page.next_cursor.is_some() {
        return Err("Invalid worktree name response.".into());
    }
    if let Some(row) = rows.first() {
        if row.name.as_ref().map(|name| name.bytes_b64.as_str())
            != Some(STANDARD.encode(name).as_str())
        {
            return Err("Worktree name check returned a different name. Check again.".into());
        }
        return Err("That worktree name already exists. Choose another name.".into());
    }
    Ok(())
}
impl CreateTarget<'_> {
    pub fn action(&self) -> Result<Action, String> {
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
        Ok(Action::WorktreeAdd {
            name: self.name.into(),
            path: Path::new(self.path.as_bytes()),
            branch: self.branch.into(),
            expected_oid: self.oid.into(),
            new_branch: self.new_branch,
            locked: self.locked,
        })
    }
}
/// Only known, byte-exact linked worktrees can be managed. Main/bare checkouts
/// and incomplete rows remain inspectable but never become mutation targets.
pub fn lock_target(entry: &Worktree) -> Option<(String, bool)> {
    let name = entry.name.as_ref()?;
    (entry.kind == "linked"
        && matches!(entry.state.as_str(), "available" | "missing")
        && valid_name(&name.display)
        && name.bytes_b64 == STANDARD.encode(&name.display))
    .then(|| (name.display.clone(), entry.locked))
}
pub fn removal_target(entry: &Worktree) -> Option<(String, bool)> {
    let (name, locked) = lock_target(entry)?;
    if locked || entry.current {
        return None;
    }
    Some((name, entry.state == "missing"))
}
pub fn review_matches(
    name: &str,
    snapshot: &str,
    selected: Option<&Worktree>,
    page: Option<&WorktreePage>,
) -> bool {
    let (Some(selected), Some(page)) = (selected, page) else {
        return false;
    };
    !snapshot.is_empty()
        && page.snapshot == snapshot
        && lock_target(selected).is_some_and(|(current, _)| current == name)
        && page.entries.contains(selected)
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
pub fn remove_action(name: &str, missing: bool) -> Result<Action, String> {
    if !valid_name(name) {
        return Err("Select a linked worktree with an exact name.".into());
    }
    // Never force, and never prune all registrations: one reviewed name only.
    Ok(if missing {
        Action::WorktreePrune { name: name.into() }
    } else {
        Action::WorktreeRemove { name: name.into() }
    })
}
pub fn repair_action(name: &str, path: &str) -> Result<Action, String> {
    if !valid_name(name) || !repair_path_valid(path) {
        return Err("Choose an absolute path to the moved linked checkout.".into());
    }
    Ok(Action::WorktreeRepair {
        name: name.into(),
        path: Path::new(path.as_bytes()),
    })
}
pub fn lock_action(name: &str, reason: &str, unlock: bool) -> Result<Action, String> {
    if !valid_name(name) {
        return Err("Select a linked worktree with an exact name.".into());
    }
    if !unlock && (reason.len() > 4096 || reason.contains('\0')) {
        return Err("Use a lock reason of at most 4096 bytes without null characters.".into());
    }
    Ok(if unlock {
        Action::WorktreeUnlock { name: name.into() }
    } else {
        Action::WorktreeLock {
            name: name.into(),
            reason: (!reason.is_empty()).then(|| reason.into()),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn worktree_row(path: &str) -> Worktree {
        Worktree {
            name: None,
            kind: "linked".into(),
            path: Path::new(path.as_bytes()),
            git_dir: Path::new(format!("{path}/.git").as_bytes()),
            current: false,
            state: "available".into(),
            head: newport_protocol::git::Head {
                oid: None,
                name: None,
                detached: false,
                unborn: false,
            },
            locked: false,
            lock_reason: None,
            prunable: false,
        }
    }
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

    fn branch_page(names: &[&str], cursor: Option<&str>) -> BranchPage {
        use newport_protocol::git::{Branch, ObjectId, Page, TotalEntries};
        Page {
            snapshot: "branches".into(),
            entries: names
                .iter()
                .map(|name| Branch {
                    name: Path::new(name.as_bytes()),
                    reference: Path::new(format!("refs/heads/{name}").as_bytes()),
                    oid: ObjectId {
                        algorithm: "sha1".into(),
                        hex: "a".repeat(40),
                    },
                    remote: false,
                    current: false,
                    upstream: None,
                    tracking: None,
                })
                .collect(),
            next_cursor: cursor.map(Into::into),
            metadata: TotalEntries { total_entries: 3 },
        }
    }
    #[test]
    fn branch_name_check_uses_exact_case_sensitive_identity_beyond_the_first_page() {
        let mut calls = 0;
        let error = check_branch_pages("topic", |cursor| {
            calls += 1;
            Ok(if calls == 1 {
                assert!(cursor.is_none());
                branch_page(&["Topic", "topic-other"], Some("second"))
            } else {
                assert_eq!(cursor, Some("second"));
                branch_page(&["topic"], None)
            })
        })
        .unwrap_err();
        assert!(error.contains("already exists"));
        assert_eq!(calls, 2);
        assert!(
            check_branch_pages("topic", |_| Ok(branch_page(
                &["Topic", "topic-other"],
                None
            )))
            .is_ok()
        );
        assert!(check_branch_pages("开发", |_| Ok(branch_page(&["开发"], None))).is_err());
    }
    #[test]
    fn branch_name_check_rejects_incomplete_changing_and_looping_pages() {
        let mut unnamed = branch_page(&["topic-other"], None);
        unnamed.snapshot.clear();
        for invalid in [unnamed, branch_page(&[], Some("more"))] {
            assert!(check_branch_pages("topic", |_| Ok(invalid.clone())).is_err());
        }
        let mut calls = 0;
        assert!(
            check_branch_pages("topic", |_| {
                calls += 1;
                let mut page = branch_page(&["topic-other"], Some("same"));
                if calls == 2 {
                    page.snapshot = "changed".into();
                }
                Ok(page)
            })
            .unwrap_err()
            .contains("changed")
        );
        assert!(
            check_branch_pages("topic", |_| Ok(branch_page(&["topic-other"], Some("same"))))
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
        let mut row = worktree_row("/topic");
        row.name = Some(Path::new(b"topic"));
        let page = |rows: Vec<newport_protocol::git::Worktree>, next: Option<&str>| {
            newport_protocol::git::Page {
                snapshot: "s".into(),
                entries: rows,
                next_cursor: next.map(Into::into),
                metadata: newport_protocol::git::WorktreeMetadata {
                    list_token: "token".into(),
                    total_entries: 1,
                    matching_entries: 1,
                    current: None,
                    main: None,
                },
            }
        };
        assert!(check_name_response("topic", &page(vec![], None)).is_ok());
        assert!(
            check_name_response("topic", &page(vec![row.clone()], None))
                .unwrap_err()
                .contains("already exists")
        );
        for invalid in [
            page(vec![], Some("more")),
            page(vec![row.clone(), row.clone()], None),
        ] {
            assert!(check_name_response("topic", &invalid).is_err());
        }
        assert!(
            check_name_response("other", &page(vec![row], None))
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
            Action::WorktreeRemove {
                name: "topic".into()
            }
        );
        assert_eq!(
            remove_action("topic", true).unwrap(),
            Action::WorktreePrune {
                name: "topic".into()
            }
        );
        let path = "/srv/开发 checkout";
        assert_eq!(
            repair_action("topic", path).unwrap(),
            Action::WorktreeRepair {
                name: "topic".into(),
                path: Path::new(path.as_bytes())
            }
        );
        assert_eq!(
            lock_action("topic", "Keep offline checkout", false).unwrap(),
            Action::WorktreeLock {
                name: "topic".into(),
                reason: Some("Keep offline checkout".into())
            }
        );
        assert_eq!(
            lock_action("topic", "ignored old reason", true).unwrap(),
            Action::WorktreeUnlock {
                name: "topic".into()
            }
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
        let Action::WorktreeAdd {
            name,
            path,
            locked,
            new_branch,
            ..
        } = target.action().unwrap()
        else {
            panic!("wrong action");
        };
        assert_eq!(name, "review-copy");
        assert_eq!(path.decode().unwrap(), b"/srv/my checkout");
        assert!(locked);
        assert!(!new_branch);
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
    use super::{WorktreePage, lock_target, removal_target, repair_path_valid, review_matches};
    use newport_protocol::git::{Head, Page, Path, Worktree, WorktreeMetadata};

    fn linked() -> Worktree {
        Worktree {
            name: Some(Path::new(b"linked")),
            kind: "linked".into(),
            path: Path::new(b"/srv/linked"),
            git_dir: Path::new(b"/srv/repo/.git/worktrees/linked"),
            current: false,
            state: "available".into(),
            head: Head {
                oid: None,
                name: None,
                detached: false,
                unborn: false,
            },
            locked: false,
            lock_reason: None,
            prunable: false,
        }
    }
    fn listing(snapshot: &str, entries: Vec<Worktree>) -> WorktreePage {
        Page {
            snapshot: snapshot.into(),
            entries,
            next_cursor: None,
            metadata: WorktreeMetadata {
                list_token: "token".into(),
                total_entries: 1,
                matching_entries: 1,
                current: None,
                main: None,
            },
        }
    }
    #[test]
    fn removal_excludes_locked_current_and_unknown_worktrees() {
        let mut entry = linked();
        assert_eq!(removal_target(&entry), Some(("linked".into(), false)));
        entry.state = "missing".into();
        assert_eq!(removal_target(&entry), Some(("linked".into(), true)));
        entry.locked = true;
        assert!(removal_target(&entry).is_none());
        entry.locked = false;
        entry.current = true;
        assert!(removal_target(&entry).is_none());
        entry.current = false;
        entry.state = "unknown".into();
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
    #[test]
    fn lock_target_requires_exact_linked_worktree_identity() {
        let mut entry = linked();
        assert_eq!(lock_target(&entry), Some(("linked".into(), false)));
        entry.locked = true;
        assert_eq!(lock_target(&entry), Some(("linked".into(), true)));
        // A name whose bytes are not its display text cannot be named exactly.
        entry.name = Some(Path {
            bytes_b64: "b3RoZXI=".into(),
            display: "linked".into(),
        });
        assert_eq!(lock_target(&entry), None);
        entry.name = Some(Path::new(b"bad\xffname"));
        assert_eq!(lock_target(&entry), None);
        entry.name = Some(Path::new(b"linked"));
        entry.kind = "main".into();
        assert_eq!(lock_target(&entry), None);
        entry.kind = "linked".into();
        entry.name = None;
        assert_eq!(lock_target(&entry), None);
    }
    #[test]
    fn reviews_reject_changed_snapshot_target_and_removed_rows() {
        let entry = linked();
        let page = listing("reviewed", vec![entry.clone()]);
        assert!(review_matches(
            "linked",
            "reviewed",
            Some(&entry),
            Some(&page)
        ));
        assert!(!review_matches(
            "other",
            "reviewed",
            Some(&entry),
            Some(&page)
        ));
        assert!(!review_matches("linked", "old", Some(&entry), Some(&page)));
        assert!(!review_matches(
            "linked",
            "reviewed",
            Some(&entry),
            Some(&listing("reviewed", vec![]))
        ));
        assert!(!review_matches("linked", "reviewed", None, Some(&page)));
        assert!(!review_matches("linked", "reviewed", Some(&entry), None));
        let mut unavailable = entry.clone();
        unavailable.state = "unknown".into();
        assert!(lock_target(&unavailable).is_none());
        assert!(!review_matches(
            "linked",
            "",
            Some(&entry),
            Some(&listing("", vec![entry.clone()]))
        ));
    }
}
