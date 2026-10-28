//! Reviewed local branch actions.
//!
//! Review state is immutable: a refresh must never silently substitute a newer
//! branch OID, tracking token, or repository snapshot into a pending action.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchReview {
    repo_id: String,
    snapshot: String,
    name: String,
    reference: String,
    oid: String,
    current: bool,
    upstream: Option<String>,
    tracking_token: Option<String>,
}

impl BranchReview {
    /// Capture a local row from `repo.branches` together with its reviewed
    /// status snapshot. Display strings are usable as Git arguments only when
    /// they exactly round-trip the protocol's original bytes.
    pub fn from_entry(repo_id: &str, snapshot: &str, entry: &Value) -> Result<Self, String> {
        if repo_id.is_empty() || snapshot.is_empty() {
            return Err("A reviewed repository and snapshot are required.".into());
        }
        if entry["remote"] != false {
            return Err("Select a local branch.".into());
        }
        let name = exact_text(&entry["name"])
            .ok_or("The branch name is unavailable or cannot be represented exactly.")?;
        let reference = branch_reference(entry)
            .ok_or("The branch reference is unavailable or cannot be represented exactly.")?;
        let oid = object_id(entry).ok_or("The branch's complete commit ID is unavailable.")?;
        let current = entry["current"]
            .as_bool()
            .ok_or("The branch's checked-out state is unavailable.")?;
        let upstream = match &entry["upstream"] {
            Value::Null => None,
            // This is display-only. A selected target is independently checked
            // against its original bytes before it can become a Git argument.
            value => Some(
                value["display"]
                    .as_str()
                    .ok_or("Invalid upstream display.")?
                    .to_owned(),
            ),
        };
        let tracking_token = (entry["tracking"]["editable"] == true)
            .then(|| entry["tracking"]["token"].as_str())
            .flatten()
            .filter(|token| !token.is_empty())
            .map(str::to_owned);
        Ok(Self {
            repo_id: repo_id.to_owned(),
            snapshot: snapshot.to_owned(),
            name: name.to_owned(),
            reference: reference.to_owned(),
            oid: oid.to_owned(),
            current,
            upstream,
            tracking_token,
        })
    }

    pub fn repo_id(&self) -> &str {
        &self.repo_id
    }
    pub fn snapshot(&self) -> &str {
        &self.snapshot
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn reference(&self) -> &str {
        &self.reference
    }
    pub fn oid(&self) -> &str {
        &self.oid
    }
    pub fn upstream(&self) -> Option<&str> {
        self.upstream.as_deref()
    }
    /// The UI also checks its server/workspace generation and fresh-read state.
    /// Even when this matches, the agent checks the OID/configuration again
    /// under its repository lock before starting a write.
    pub fn matches(&self, repo_id: &str, snapshot: &str) -> bool {
        self.repo_id == repo_id && self.snapshot == snapshot
    }
    pub fn can_delete(&self) -> bool {
        !self.current
    }
    pub fn can_set_upstream(&self) -> bool {
        self.tracking_token.is_some()
    }

    /// Force bypasses Git's merged-commit check only; Git still refuses branches
    /// checked out by any worktree. Force is explicit for each confirmation.
    pub fn delete_action(&self, force: bool) -> Result<Value, String> {
        if !self.can_delete() {
            return Err("The current branch cannot be deleted.".into());
        }
        Ok(json!({
            "kind": "branch.delete",
            "name": self.name,
            "expectedOid": self.oid,
            "force": force,
        }))
    }

    /// A target must be an exact, loaded local or remote-tracking branch row.
    /// Preserve its full reference to avoid collisions between `origin/main`
    /// as a local name and the remote-tracking branch of the same short name.
    /// `None` clears tracking. This writes local config only, with no fetch/push.
    pub fn set_upstream_action(&self, upstream: Option<&Value>) -> Result<Value, String> {
        let token = self
            .tracking_token
            .as_deref()
            .ok_or("Upstream configuration is unavailable or inherited and cannot be edited.")?;
        let reference = upstream
            .map(|entry| {
                self.upstream_reference(entry).ok_or_else(|| {
                    "Choose a loaded branch other than this branch as the upstream.".to_owned()
                })
            })
            .transpose()?;
        Ok(json!({
            "kind": "branch.set_upstream",
            "name": self.name,
            "expectedOid": self.oid,
            "expectedToken": token,
            "upstream": reference,
        }))
    }

    /// Filter paginated picker rows without allocating actions or accumulating
    /// the entire branch list. Invalid/missing OIDs and lossy names are excluded.
    pub fn upstream_reference<'a>(&self, entry: &'a Value) -> Option<&'a str> {
        object_id(entry)?;
        let reference = branch_reference(entry)?;
        (reference != self.reference).then_some(reference)
    }
}

/// A lazily loaded branch listing for an upstream picker. Its pagination
/// snapshot is independent of the status snapshot captured by `BranchReview`.
#[derive(Clone, Debug)]
pub struct BranchChoices {
    snapshot: String,
    metadata: Value,
    entries: Vec<Value>,
    next_cursor: Option<String>,
}

impl BranchChoices {
    pub fn from_page(page: &Value) -> Result<Self, String> {
        let snapshot = page["snapshot"]
            .as_str()
            .filter(|snapshot| !snapshot.is_empty())
            .ok_or("Invalid branch listing snapshot.")?;
        if !page["metadata"].is_object() {
            return Err("Invalid branch listing metadata.".into());
        }
        let entries = page["entries"]
            .as_array()
            .ok_or("Invalid branch listing.")?;
        let mut keys = std::collections::HashSet::new();
        let mut unique = Vec::with_capacity(entries.len());
        for entry in entries {
            let key = entry["reference"]["bytesB64"]
                .as_str()
                .filter(|key| !key.is_empty())
                .ok_or("Invalid branch listing reference.")?;
            if keys.insert(key) {
                unique.push(entry.clone());
            }
        }
        let next_cursor = match page.get("nextCursor") {
            Some(Value::Null) => None,
            Some(Value::String(cursor)) if !cursor.is_empty() => Some(cursor.clone()),
            _ => return Err("Invalid branch listing cursor.".into()),
        };
        Ok(Self {
            snapshot: snapshot.to_owned(),
            metadata: page["metadata"].clone(),
            entries: unique,
            next_cursor,
        })
    }

    /// A rejected page leaves the existing list untouched. The caller owns
    /// request generation checks so a late response cannot replace a new picker.
    pub fn append(&mut self, page: &Value, expected_cursor: &str) -> Result<(), String> {
        let incoming = Self::from_page(page)?;
        if self.next_cursor.as_deref() != Some(expected_cursor)
            || incoming.snapshot != self.snapshot
            || incoming.metadata != self.metadata
            || incoming.next_cursor.as_deref() == Some(expected_cursor)
        {
            return Err("The branch listing changed. Refresh before loading more.".into());
        }
        let known = self
            .entries
            .iter()
            .map(|entry| &entry["reference"]["bytesB64"])
            .collect::<std::collections::HashSet<_>>();
        let additions = incoming
            .entries
            .into_iter()
            .filter(|entry| !known.contains(&entry["reference"]["bytesB64"]))
            .collect::<Vec<_>>();
        if additions.is_empty() && incoming.next_cursor.is_some() {
            return Err("The next page did not add any branches. Refresh the list.".into());
        }
        self.entries.extend(additions);
        self.next_cursor = incoming.next_cursor;
        Ok(())
    }

    pub fn entries(&self) -> &[Value] {
        &self.entries
    }
    pub fn next_cursor(&self) -> Option<&str> {
        self.next_cursor.as_deref()
    }
    pub fn candidates<'a>(&'a self, review: &'a BranchReview) -> impl Iterator<Item = &'a Value> {
        self.entries
            .iter()
            .filter(|entry| review.upstream_reference(entry).is_some())
    }
    /// A full reference is the identity; shortened display names are ambiguous.
    pub fn find_reference(&self, reference: &str) -> Option<&Value> {
        self.entries
            .iter()
            .find(|entry| branch_reference(entry) == Some(reference))
    }
}

fn exact_text(value: &Value) -> Option<&str> {
    let display = value["display"].as_str()?;
    (!display.is_empty()
        && !display.chars().any(char::is_control)
        && value["bytesB64"].as_str()? == STANDARD.encode(display))
    .then_some(display)
}

fn branch_reference(entry: &Value) -> Option<&str> {
    let name = exact_text(&entry["name"])?;
    let reference = exact_text(&entry["reference"])?;
    let prefix = if entry["remote"].as_bool()? {
        "refs/remotes/"
    } else {
        "refs/heads/"
    };
    (!name.starts_with('-') && reference.strip_prefix(prefix)? == name).then_some(reference)
}

fn object_id(entry: &Value) -> Option<&str> {
    let oid = &entry["oid"];
    let expected = match oid["algorithm"]
        .as_str()
        .or_else(|| oid["format"].as_str())?
    {
        "sha1" => 40,
        "sha256" => 64,
        _ => return None,
    };
    let hex = oid["hex"].as_str()?;
    (hex.len() == expected
        && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
        && hex.bytes().any(|byte| byte != b'0'))
    .then_some(hex)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeBranch {
    pub name: String,
    pub reference: String,
    pub oid: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorktreeBranchUse {
    Existing,
    StartingPoint,
}
impl WorktreeBranchUse {
    pub fn candidate(self, entry: &Value, bare: bool) -> Option<WorktreeBranch> {
        match self {
            Self::Existing => WorktreeBranch::from_entry(entry, bare),
            Self::StartingPoint => Some(WorktreeBranch {
                name: exact_text(&entry["name"])?.into(),
                reference: branch_reference(entry)?.into(),
                oid: object_id(entry)?.into(),
            }),
        }
    }
}
impl WorktreeBranch {
    pub fn from_entry(entry: &Value, bare: bool) -> Option<Self> {
        if entry["remote"] != false
            || (!bare && entry["current"] != false)
            || !entry["current"].is_boolean()
        {
            return None;
        }
        Some(Self {
            name: exact_text(&entry["name"])?.into(),
            reference: branch_reference(entry)?.into(),
            oid: object_id(entry)?.into(),
        })
    }
}
impl crate::backend::Backend {
    pub fn worktree_branches(
        &self,
        server: &str,
        repo: &str,
        filter: &str,
        cursor: Option<&str>,
    ) -> Result<Value, String> {
        self.worktree_branch_choices(server, repo, filter, cursor, WorktreeBranchUse::Existing)
    }
    pub fn worktree_branch_choices(
        &self,
        server: &str,
        repo: &str,
        filter: &str,
        cursor: Option<&str>,
        purpose: WorktreeBranchUse,
    ) -> Result<Value, String> {
        let mut params = json!({"repoId":repo,"filter":filter,"pageSize":20});
        if purpose == WorktreeBranchUse::Existing {
            params["branchKind"] = json!("local");
        }
        if let Some(cursor) = cursor {
            params["cursor"] = json!(cursor);
        }
        let result = self.call(
            "git_request",
            json!({"serverId":server,"request":{"method":"repo.branches","params":params}}),
        )?;
        if result["entries"]
            .as_array()
            .is_none_or(|entries| entries.len() > 20)
        {
            return Err("Invalid worktree branch page.".into());
        }
        BranchChoices::from_page(&result)?;
        Ok(result)
    }
    pub fn worktree_branch_owner(
        &self,
        server: &str,
        repo: &str,
        branch: &WorktreeBranch,
    ) -> Result<Option<String>, String> {
        let result=self.call("git_request",json!({"serverId":server,"request":{"method":"repo.worktrees","params":{"repoId":repo,"branch":branch.reference,"pageSize":1}}}))?;
        let entries = result["entries"]
            .as_array()
            .ok_or("Invalid worktree ownership response.")?;
        if entries.len() > 1 || result.get("nextCursor") != Some(&Value::Null) {
            return Err("Invalid worktree ownership response.".into());
        }
        let Some(entry) = entries.first() else {
            return Ok(None);
        };
        if entry["head"]["name"]["bytesB64"] != STANDARD.encode(&branch.reference) {
            return Err("Worktree ownership changed. Select the branch again.".into());
        }
        Ok(Some(
            entry["path"]["display"]
                .as_str()
                .unwrap_or("another worktree")
                .into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire(text: &str) -> Value {
        json!({"display":text,"bytesB64":STANDARD.encode(text)})
    }
    fn entry(name: &str, remote: bool) -> Value {
        json!({
            "name":wire(name),
            "reference":wire(&format!("refs/{}/{name}", if remote {"remotes"} else {"heads"})),
            "oid":{"algorithm":"sha1","hex":"a".repeat(40)},
            "remote":remote,"current":false,"upstream":null,
            "tracking":{"editable":true,"token":"tracking-at-review"}
        })
    }

    #[test]
    fn starting_points_allow_current_and_remote_branches_but_keep_exact_identity() {
        let mut local = entry("origin/topic", false);
        local["current"] = json!(true);
        let remote = entry("origin/topic", true);
        let local_base = WorktreeBranchUse::StartingPoint
            .candidate(&local, false)
            .unwrap();
        let remote_base = WorktreeBranchUse::StartingPoint
            .candidate(&remote, false)
            .unwrap();
        assert_eq!(local_base.name, remote_base.name);
        assert_ne!(local_base.reference, remote_base.reference);
        assert!(
            WorktreeBranchUse::Existing
                .candidate(&local, false)
                .is_none()
        );
        assert!(
            WorktreeBranchUse::Existing
                .candidate(&remote, false)
                .is_none()
        );
        local["reference"]["bytesB64"] = json!(STANDARD.encode("refs/heads/other"));
        assert!(
            WorktreeBranchUse::StartingPoint
                .candidate(&local, false)
                .is_none()
        );
        let mut short = remote;
        short["oid"]["hex"] = json!("abcdef");
        assert!(
            WorktreeBranchUse::StartingPoint
                .candidate(&short, false)
                .is_none()
        );
    }

    #[test]
    fn worktree_choices_require_exact_local_names_commits_and_checked_out_state() {
        let mut local = entry("topic", false);
        assert_eq!(
            WorktreeBranch::from_entry(&local, false).unwrap().reference,
            "refs/heads/topic"
        );
        local["current"] = json!(true);
        assert!(WorktreeBranch::from_entry(&local, false).is_none());
        assert!(WorktreeBranch::from_entry(&local, true).is_some());
        local["current"] = Value::Null;
        assert!(WorktreeBranch::from_entry(&local, true).is_none());
        local["current"] = json!(false);
        local["name"]["display"] = json!("different");
        assert!(WorktreeBranch::from_entry(&local, false).is_none());
        local = entry("topic", false);
        local["oid"]["hex"] = json!("a".repeat(8));
        assert!(WorktreeBranch::from_entry(&local, false).is_none());
        local["oid"] = json!({"algorithm":"sha256","hex":"b".repeat(64)});
        assert!(WorktreeBranch::from_entry(&local, false).is_some());
        assert!(WorktreeBranch::from_entry(&entry("origin/topic", true), false).is_none());
    }

    #[test]
    fn review_pins_scope_oid_and_tracking_config_after_source_refreshes() {
        let mut source = entry("topic", false);
        source["upstream"] = wire("refs/remotes/origin/main");
        let review = BranchReview::from_entry("repo-a", "snapshot-a", &source).unwrap();
        source["oid"]["hex"] = json!("b".repeat(40));
        source["tracking"]["token"] = json!("tracking-after-review");
        assert!(review.matches("repo-a", "snapshot-a"));
        assert!(!review.matches("repo-b", "snapshot-a"));
        assert!(!review.matches("repo-a", "snapshot-b"));
        assert_eq!(review.upstream(), Some("refs/remotes/origin/main"));
        assert_eq!(review.oid(), "a".repeat(40));
        let action = review.set_upstream_action(None).unwrap();
        assert_eq!(action["expectedToken"], "tracking-at-review");
        assert_eq!(action["expectedOid"], "a".repeat(40));
        assert!(action["upstream"].is_null());
    }

    #[test]
    fn deletion_force_is_explicit_and_current_branch_is_never_deletable() {
        let mut source = entry("topic", false);
        let review = BranchReview::from_entry("repo", "snapshot", &source).unwrap();
        assert_eq!(
            review.delete_action(false).unwrap(),
            json!({
                "kind":"branch.delete","name":"topic","expectedOid":"a".repeat(40),"force":false
            })
        );
        assert_eq!(review.delete_action(true).unwrap()["force"], true);
        assert_eq!(review.delete_action(false).unwrap()["force"], false);
        source["current"] = json!(true);
        let review = BranchReview::from_entry("repo", "snapshot", &source).unwrap();
        assert!(!review.can_delete());
        assert!(review.delete_action(false).is_err());
        assert!(review.delete_action(true).is_err());
        assert!(review.can_set_upstream());
    }

    #[test]
    fn upstream_preserves_full_reference_for_local_remote_name_collisions() {
        let source = entry("topic", false);
        let review = BranchReview::from_entry("repo", "snapshot", &source).unwrap();
        for (remote, prefix) in [(true, "remotes"), (false, "heads")] {
            let target = entry("origin/main", remote);
            let action = review.set_upstream_action(Some(&target)).unwrap();
            assert_eq!(action["upstream"], format!("refs/{prefix}/origin/main"));
            assert_eq!(action["name"], "topic");
            assert_eq!(action["expectedToken"], "tracking-at-review");
        }
        assert!(review.upstream_reference(&source).is_none());
        assert!(review.set_upstream_action(Some(&source)).is_err());
    }

    #[test]
    fn uneditable_missing_or_empty_tracking_disables_set_and_clear_only() {
        for tracking in [
            Value::Null,
            json!({"editable":false,"token":"inherited"}),
            json!({"editable":true,"token":""}),
            json!({"editable":true}),
        ] {
            let mut source = entry("topic", false);
            source["tracking"] = tracking;
            let review = BranchReview::from_entry("repo", "snapshot", &source).unwrap();
            assert!(!review.can_set_upstream());
            assert!(review.set_upstream_action(None).is_err());
            assert!(
                review
                    .set_upstream_action(Some(&entry("main", false)))
                    .is_err()
            );
            assert!(review.delete_action(false).is_ok());
        }
    }

    #[test]
    fn malformed_or_lossy_branch_identity_never_becomes_a_write_target() {
        let source = entry("topic", false);
        let review = BranchReview::from_entry("repo", "snapshot", &source).unwrap();
        let mut bad_rows = vec![entry("topic", true)];
        for (field, replacement) in [
            ("name", wire("other")),
            ("reference", wire("refs/tags/topic")),
            ("oid", json!({"algorithm":"sha1","hex":"a".repeat(8)})),
            ("oid", json!({"algorithm":"sha256","hex":"a".repeat(40)})),
            ("oid", json!({"algorithm":"sha1","hex":"0".repeat(40)})),
            ("current", Value::Null),
        ] {
            let mut row = source.clone();
            row[field] = replacement;
            bad_rows.push(row);
        }
        for field in ["name", "reference"] {
            let mut row = source.clone();
            row[field]["bytesB64"] = json!(STANDARD.encode(b"topic\xff"));
            bad_rows.push(row);
        }
        for row in bad_rows {
            assert!(
                BranchReview::from_entry("repo", "snapshot", &row).is_err(),
                "{row}"
            );
        }
        let mut target = entry("main", false);
        target["reference"]["bytesB64"] = json!(STANDARD.encode(b"refs/heads/main\xff"));
        assert!(review.upstream_reference(&target).is_none());
        assert!(review.set_upstream_action(Some(&target)).is_err());
        assert!(BranchReview::from_entry("", "snapshot", &source).is_err());
        assert!(BranchReview::from_entry("repo", "", &source).is_err());
    }

    #[test]
    fn unicode_branch_names_and_sha256_objects_round_trip_without_normalization() {
        let mut row = entry("feature/café", false);
        row["oid"] = json!({"algorithm":"sha256","hex":"b".repeat(64)});
        let review = BranchReview::from_entry("repo", "snapshot", &row).unwrap();
        assert_eq!(review.name(), "feature/café");
        assert_eq!(review.reference(), "refs/heads/feature/café");
        assert_eq!(
            review.delete_action(false).unwrap()["expectedOid"],
            "b".repeat(64)
        );
    }

    fn page(entries: &[Value], next: Option<&str>) -> Value {
        json!({"snapshot":"branch-list-snapshot","metadata":{"totalEntries":3},"entries":entries,"nextCursor":next})
    }

    #[test]
    fn branch_choices_page_lazily_deduplicates_and_preserves_full_ref_identity() {
        let source = entry("topic", false);
        let review = BranchReview::from_entry("repo", "status-snapshot", &source).unwrap();
        let local = entry("origin/main", false);
        let remote = entry("origin/main", true);
        let mut choices =
            BranchChoices::from_page(&page(&[source.clone(), local.clone()], Some("cursor-2")))
                .unwrap();
        assert_eq!(choices.entries().len(), 2);
        assert_eq!(choices.candidates(&review).count(), 1);
        assert!(choices.find_reference("refs/remotes/origin/main").is_none());
        choices
            .append(&page(&[local.clone(), remote.clone()], None), "cursor-2")
            .unwrap();
        assert_eq!(choices.entries().len(), 3);
        assert_eq!(choices.candidates(&review).count(), 2);
        assert_eq!(
            choices.find_reference("refs/heads/origin/main"),
            Some(&local)
        );
        assert_eq!(
            choices.find_reference("refs/remotes/origin/main"),
            Some(&remote)
        );
        assert_eq!(choices.next_cursor(), None);
        assert!(choices.find_reference("origin/main").is_none());
    }

    #[test]
    fn branch_choices_reject_changed_snapshots_cursors_and_no_progress_without_mutating() {
        let first = page(&[entry("topic", false)], Some("cursor-2"));
        let valid = page(&[entry("main", false)], Some("cursor-3"));
        for (page, cursor) in [
            (valid.clone(), "obsolete-cursor"),
            (
                {
                    let mut v = valid.clone();
                    v["snapshot"] = json!("new-read");
                    v
                },
                "cursor-2",
            ),
            (
                {
                    let mut v = valid.clone();
                    v["metadata"]["totalEntries"] = json!(4);
                    v
                },
                "cursor-2",
            ),
            (
                {
                    let mut v = valid.clone();
                    v["nextCursor"] = json!("cursor-2");
                    v
                },
                "cursor-2",
            ),
            (page(&[entry("topic", false)], Some("cursor-3")), "cursor-2"),
        ] {
            let mut choices = BranchChoices::from_page(&first).unwrap();
            let entries = choices.entries().to_vec();
            assert!(choices.append(&page, cursor).is_err());
            assert_eq!(choices.entries(), entries);
            assert_eq!(choices.next_cursor(), Some("cursor-2"));
        }
        let mut choices = BranchChoices::from_page(&first).unwrap();
        choices.append(&page(&[], None), "cursor-2").unwrap();
        assert_eq!(choices.next_cursor(), None);
    }
}
