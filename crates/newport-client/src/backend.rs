//! One connection per call allows independent reads; no request is replayed.
//! Terminal opens retain their connection as an explicit lifetime lease.
use newport_platform::ipc::Stream;
use newport_platform::ipc::protocol;
use protocol::{MAX_REQUEST, MAX_RESPONSE, Request, ResponseFrame, VERSION};
use serde_json::Value;
use std::{
    io::{Read, Write},
    path::PathBuf,
    time::Duration,
};

#[derive(Clone)]
pub struct Backend {
    path: PathBuf,
}
pub struct PushTarget<'a> {
    pub remote: &'a str,
    pub token: &'a str,
    pub branch: &'a str,
    pub oid: &'a str,
    pub destination: &'a str,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StashMode {
    Apply,
    Pop,
    Drop,
}
impl StashMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Apply => "Apply",
            Self::Pop => "Pop",
            Self::Drop => "Drop",
        }
    }
    fn kind(self) -> &'static str {
        match self {
            Self::Apply => "stash.apply",
            Self::Pop => "stash.pop",
            Self::Drop => "stash.drop",
        }
    }
}
pub struct StashTarget<'a> {
    pub mode: StashMode,

    pub oid: &'a str,
    pub index: u64,
    pub token: &'a str,
    pub reinstate_index: bool,
}
impl Backend {
    /// One bounded request; uncertain responses are never replayed.
    pub fn activate_window(&self) -> Result<(), String> {
        self.call_with_timeout(
            protocol::ACTIVATE_WINDOW,
            serde_json::json!({}),
            Duration::from_secs(8),
        )
        .map(|_| ())
    }
    /// Called only for an explicit user gesture; terminal escape parsing has
    /// no connection to the platform opener.
    pub fn open_terminal_link(&self, url: &str) -> Result<(), String> {
        let url = newport_domain::external_link::http_url(url)
            .ok_or("Only http and https links can be opened.")?;
        self.call("open_url", serde_json::json!({"url":url}))
            .map(|_| ())
    }
    /// Connect to an explicitly selected local backend, without process-global
    /// environment changes. The same bounded IPC contract applies.
    pub fn at_path(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn socket_path(&self) -> &std::path::Path {
        &self.path
    }

    pub fn apply_reviewed_action(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        action: Value,
        operation: uuid::Uuid,
    ) -> Result<crate::git_operation::Operation, crate::git_operation::WriteError> {
        if repo.is_empty()
            || snapshot.is_empty()
            || !matches!(
                action["kind"].as_str(),
                Some(
                    "remote.add"
                        | "remote.rename"
                        | "remote.set_url"
                        | "remote.remove"
                        | "config.set"
                        | "branch.delete"
                        | "branch.set_upstream"
                        | "branch.delete_remote"
                        | "tag.delete_remote"
                        | "tag.push"
                        | "push.with_lease"
                        | "fetch"
                        | "pull.fast_forward"
                        | "push"
                        | "merge"
                        | "merge.fast_forward"
                        | "rebase"
                        | "conflict.resolve"
                        | "reset"
                        | "commit.amend"
                        | "checkout"
                        | "cherry_pick"
                        | "revert"
                        | "integration.continue"
                        | "integration.abort"
                        | "integration.skip"
                )
            )
        {
            return Err("A reviewed Git action and repository state are required".into());
        }
        self.call(
            "git_pending_operations",
            serde_json::json!({"serverId":server}),
        )?;
        let response = self.call("git_request",serde_json::json!({"serverId":server,"request":{"method":"operation.start","params":{"operationId":operation,"repoId":repo,"expectedSnapshot":snapshot,"action":action}}}))
            .map_err(|message|crate::git_operation::WriteError{message,dispatched:true})?;
        crate::git_operation::Operation::decode(response, &operation.to_string()).map_err(
            |message| crate::git_operation::WriteError {
                message,
                dispatched: true,
            },
        )
    }
    pub fn remove_worktree(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        target: (&str, bool),
        operation: uuid::Uuid,
    ) -> Result<crate::git_operation::Operation, crate::git_operation::WriteError> {
        let (name, missing) = target;
        let action = crate::git_worktree::remove_action(name, missing)?;
        if [repo, snapshot, name].iter().any(|s| s.is_empty()) {
            return Err("A reviewed worktree identity and listing are required".into());
        }
        self.call(
            "git_pending_operations",
            serde_json::json!({"serverId":server}),
        )?;
        let response = self.call("git_request", serde_json::json!({"serverId":server,"request":{"method":"operation.start","params":{"operationId":operation,"repoId":repo,"expectedSnapshot":snapshot,"action":action}}}))
            .map_err(|message|crate::git_operation::WriteError{message,dispatched:true})?;
        crate::git_operation::Operation::decode(response, &operation.to_string()).map_err(
            |message| crate::git_operation::WriteError {
                message,
                dispatched: true,
            },
        )
    }
    pub fn repair_worktree(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        target: (&str, &str),
        operation: uuid::Uuid,
    ) -> Result<crate::git_operation::Operation, crate::git_operation::WriteError> {
        let (name, path) = target;
        let action = crate::git_worktree::repair_action(name, path)?;
        if [repo, snapshot, name, path].iter().any(|s| s.is_empty()) {
            return Err("A reviewed worktree and its current path are required".into());
        }
        self.call(
            "git_pending_operations",
            serde_json::json!({"serverId":server}),
        )?;
        let response = self.call("git_request", serde_json::json!({"serverId":server,"request":{"method":"operation.start","params":{"operationId":operation,"repoId":repo,"expectedSnapshot":snapshot,"action":action}}}))
            .map_err(|message|crate::git_operation::WriteError{message,dispatched:true})?;
        crate::git_operation::Operation::decode(response, &operation.to_string()).map_err(
            |message| crate::git_operation::WriteError {
                message,
                dispatched: true,
            },
        )
    }
    pub fn create_worktree(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        target: crate::git_worktree::CreateTarget<'_>,
        operation: uuid::Uuid,
    ) -> Result<crate::git_operation::Operation, crate::git_operation::WriteError> {
        if repo.is_empty() || snapshot.is_empty() {
            return Err("A reviewed repository snapshot is required".into());
        }
        let action = target.action()?;
        self.call(
            "git_pending_operations",
            serde_json::json!({"serverId":server}),
        )?;
        let response = self.call("git_request", serde_json::json!({"serverId":server,"request":{"method":"operation.start","params":{"operationId":operation,"repoId":repo,"expectedSnapshot":snapshot,"action":action}}}))
            .map_err(|message|crate::git_operation::WriteError{message,dispatched:true})?;
        crate::git_operation::Operation::decode(response, &operation.to_string()).map_err(
            |message| crate::git_operation::WriteError {
                message,
                dispatched: true,
            },
        )
    }
    pub fn lock_worktree(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        target: (&str, &str, bool),
        operation: uuid::Uuid,
    ) -> Result<crate::git_operation::Operation, crate::git_operation::WriteError> {
        let (name, reason, unlock) = target;
        let action = crate::git_worktree::lock_action(name, reason, unlock)?;
        if [repo, snapshot, name].iter().any(|s| s.is_empty()) {
            return Err("A reviewed worktree listing and identity are required".into());
        }
        self.call(
            "git_pending_operations",
            serde_json::json!({"serverId":server}),
        )?;
        let response=self.call("git_request",serde_json::json!({"serverId":server,"request":{"method":"operation.start","params":{"operationId":operation,"repoId":repo,"expectedSnapshot":snapshot,"action":action}}})).map_err(|message|crate::git_operation::WriteError{message,dispatched:true})?;
        crate::git_operation::Operation::decode(response, &operation.to_string()).map_err(
            |message| crate::git_operation::WriteError {
                message,
                dispatched: true,
            },
        )
    }
    pub fn delete_tag(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        target: (&str, &str),
        operation: uuid::Uuid,
    ) -> Result<crate::git_operation::Operation, crate::git_operation::WriteError> {
        let (name, oid) = target;
        if [repo, snapshot, name, oid].iter().any(|s| s.is_empty()) {
            return Err("A reviewed tag identity and repository snapshot are required".into());
        }
        self.call(
            "git_pending_operations",
            serde_json::json!({"serverId":server}),
        )?;
        let response=self.call("git_request",serde_json::json!({"serverId":server,"request":{"method":"operation.start","params":{"operationId":operation,"repoId":repo,"expectedSnapshot":snapshot,"action":{"kind":"tag.delete","name":name,"expectedOid":oid}}}})).map_err(|message|crate::git_operation::WriteError{message,dispatched:true})?;
        crate::git_operation::Operation::decode(response, &operation.to_string()).map_err(
            |message| crate::git_operation::WriteError {
                message,
                dispatched: true,
            },
        )
    }
    pub fn create_tag(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        target: (&str, &str, &str),
        operation: uuid::Uuid,
    ) -> Result<crate::git_operation::Operation, crate::git_operation::WriteError> {
        let (name, oid, message) = target;
        if [repo, snapshot, name, oid].iter().any(|s| s.is_empty()) {
            return Err("A tag name, target commit and repository snapshot are required".into());
        }
        self.call(
            "git_pending_operations",
            serde_json::json!({"serverId":server}),
        )?;
        let mut action = serde_json::json!({"kind":"tag.create","name":name,"targetOid":oid});
        if !message.is_empty() {
            action["annotation"] = serde_json::json!({"message":message});
        }
        let response=self.call("git_request",serde_json::json!({"serverId":server,"request":{"method":"operation.start","params":{"operationId":operation,"repoId":repo,"expectedSnapshot":snapshot,"action":action}}})).map_err(|message|crate::git_operation::WriteError{message,dispatched:true})?;
        crate::git_operation::Operation::decode(response, &operation.to_string()).map_err(
            |message| crate::git_operation::WriteError {
                message,
                dispatched: true,
            },
        )
    }
    pub fn save_stash(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        options: (String, bool, bool),
        operation: uuid::Uuid,
    ) -> Result<crate::git_operation::Operation, crate::git_operation::WriteError> {
        if repo.is_empty() || snapshot.is_empty() {
            return Err("A reviewed repository snapshot is required".into());
        }
        self.call(
            "git_pending_operations",
            serde_json::json!({"serverId":server}),
        )?;
        let (message, include_untracked, keep_index) = options;
        let response = self.call("git_request", serde_json::json!({"serverId":server,"request":{"method":"operation.start","params":{
            "operationId":operation,"repoId":repo,"expectedSnapshot":snapshot,
            "action":{"kind":"stash.save","message":message,"includeUntracked":include_untracked,"keepIndex":keep_index}
        }}})).map_err(|message| crate::git_operation::WriteError {message,dispatched:true})?;
        crate::git_operation::Operation::decode(response, &operation.to_string()).map_err(
            |message| crate::git_operation::WriteError {
                message,
                dispatched: true,
            },
        )
    }
    pub fn modify_stash(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        target: StashTarget<'_>,
        operation: uuid::Uuid,
    ) -> Result<crate::git_operation::Operation, crate::git_operation::WriteError> {
        if [repo, snapshot, target.oid, target.token]
            .iter()
            .any(|s| s.is_empty())
        {
            return Err("A reviewed stash and repository snapshot are required".into());
        }
        self.call(
            "git_pending_operations",
            serde_json::json!({"serverId":server}),
        )?;
        let mut action = serde_json::json!({"kind":target.mode.kind(),"oid":target.oid,"index":target.index,"expectedToken":target.token});
        if target.mode != StashMode::Drop {
            action["reinstateIndex"] = serde_json::json!(target.reinstate_index);
        }
        let response=self.call("git_request",serde_json::json!({"serverId":server,"request":{"method":"operation.start","params":{
            "operationId":operation,"repoId":repo,"expectedSnapshot":snapshot,"action":action
        }}})).map_err(|message|crate::git_operation::WriteError {message,dispatched:true})?;
        crate::git_operation::Operation::decode(response, &operation.to_string()).map_err(
            |message| crate::git_operation::WriteError {
                message,
                dispatched: true,
            },
        )
    }
    pub fn push(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        target: PushTarget<'_>,
        operation: uuid::Uuid,
    ) -> Result<crate::git_operation::Operation, crate::git_operation::WriteError> {
        if [
            repo,
            snapshot,
            target.remote,
            target.token,
            target.branch,
            target.oid,
            target.destination,
        ]
        .iter()
        .any(|s| s.is_empty())
        {
            return Err("A reviewed local branch, remote and destination are required".into());
        }
        self.call(
            "git_pending_operations",
            serde_json::json!({"serverId":server}),
        )?;
        let response = self.call("git_request", serde_json::json!({"serverId":server,"request":{"method":"operation.start","params":{
            "operationId":operation,"repoId":repo,"expectedSnapshot":snapshot,
            "action":{"kind":"push","remote":target.remote,"expectedToken":target.token,"branch":target.branch,"expectedOid":target.oid,"destinationBranch":target.destination}
        }}})).map_err(|message| crate::git_operation::WriteError {message, dispatched:true})?;
        crate::git_operation::Operation::decode(response, &operation.to_string()).map_err(
            |message| crate::git_operation::WriteError {
                message,
                dispatched: true,
            },
        )
    }
    pub fn pull_fast_forward(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        target: (&str, &str, &str),
        operation: uuid::Uuid,
    ) -> Result<crate::git_operation::Operation, crate::git_operation::WriteError> {
        let (remote, token, branch) = target;
        if [repo, snapshot, remote, token, branch]
            .iter()
            .any(|s| s.is_empty())
        {
            return Err("A reviewed remote, branch and repository snapshot are required".into());
        }
        self.call(
            "git_pending_operations",
            serde_json::json!({"serverId":server}),
        )?;
        let response = self.call("git_request", serde_json::json!({"serverId":server,"request":{"method":"operation.start","params":{
            "operationId":operation,"repoId":repo,"expectedSnapshot":snapshot,
            "action":{"kind":"pull.fast_forward","remote":remote,"expectedToken":token,"remoteBranch":branch}
        }}})).map_err(|message| crate::git_operation::WriteError {message, dispatched:true})?;
        crate::git_operation::Operation::decode(response, &operation.to_string()).map_err(
            |message| crate::git_operation::WriteError {
                message,
                dispatched: true,
            },
        )
    }
    pub fn rename_branch(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        target: (&str, &str, &str),
        operation: uuid::Uuid,
    ) -> Result<crate::git_operation::Operation, crate::git_operation::WriteError> {
        let (name, new_name, expected_oid) = target;
        if repo.is_empty()
            || snapshot.is_empty()
            || name.is_empty()
            || new_name.trim().is_empty()
            || expected_oid.is_empty()
        {
            return Err("A reviewed branch, new name and snapshot are required".into());
        }
        self.call(
            "git_pending_operations",
            serde_json::json!({"serverId":server}),
        )?;
        let response = self.call("git_request", serde_json::json!({"serverId":server,"request":{"method":"operation.start","params":{
            "operationId":operation,"repoId":repo,"expectedSnapshot":snapshot,
            "action":{"kind":"branch.rename","name":name,"newName":new_name,"expectedOid":expected_oid}
        }}})).map_err(|message| crate::git_operation::WriteError {message, dispatched:true})?;
        crate::git_operation::Operation::decode(response, &operation.to_string()).map_err(
            |message| crate::git_operation::WriteError {
                message,
                dispatched: true,
            },
        )
    }
    pub fn create_branch(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        target: (&str, &str),
        operation: uuid::Uuid,
    ) -> Result<crate::git_operation::Operation, crate::git_operation::WriteError> {
        let (name, start_oid) = target;
        if repo.is_empty() || snapshot.is_empty() || name.trim().is_empty() || start_oid.is_empty()
        {
            return Err("A branch name, starting commit and reviewed snapshot are required".into());
        }
        self.call(
            "git_pending_operations",
            serde_json::json!({"serverId":server}),
        )?;
        let response = self.call("git_request", serde_json::json!({"serverId":server,"request":{"method":"operation.start","params":{
            "operationId":operation,"repoId":repo,"expectedSnapshot":snapshot,
            "action":{"kind":"branch.create","name":name,"startOid":start_oid}
        }}})).map_err(|message| crate::git_operation::WriteError {message, dispatched:true})?;
        crate::git_operation::Operation::decode(response, &operation.to_string()).map_err(
            |message| crate::git_operation::WriteError {
                message,
                dispatched: true,
            },
        )
    }
    pub fn checkout(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        target: (&str, &str),
        operation: uuid::Uuid,
    ) -> Result<crate::git_operation::Operation, crate::git_operation::WriteError> {
        let (branch, expected_oid) = target;
        if repo.is_empty() || snapshot.is_empty() || branch.is_empty() || expected_oid.is_empty() {
            return Err("A reviewed branch and repository snapshot are required".into());
        }
        self.call(
            "git_pending_operations",
            serde_json::json!({"serverId":server}),
        )?;
        let response = self.call("git_request", serde_json::json!({"serverId":server,"request":{"method":"operation.start","params":{
            "operationId":operation,"repoId":repo,"expectedSnapshot":snapshot,
            "action":{"kind":"checkout","target":{"kind":"branch","name":branch,"expectedOid":expected_oid}}
        }}})).map_err(|message| crate::git_operation::WriteError {message, dispatched:true})?;
        crate::git_operation::Operation::decode(response, &operation.to_string()).map_err(
            |message| crate::git_operation::WriteError {
                message,
                dispatched: true,
            },
        )
    }
    pub fn commit(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        message: &str,
        operation: uuid::Uuid,
    ) -> Result<crate::git_operation::Operation, crate::git_operation::WriteError> {
        use serde_json::json;
        if repo.is_empty()
            || snapshot.is_empty()
            || message.trim().is_empty()
            || message.contains('\0')
        {
            return Err("A current repository snapshot and commit message are required".into());
        }
        self.call("git_pending_operations", json!({"serverId":server}))?;
        let response = self.call("git_request", json!({"serverId":server,"request":{"method":"operation.start","params":{
            "operationId":operation,"repoId":repo,"expectedSnapshot":snapshot,"action":{"kind":"commit","message":message}
        }}})).map_err(|message| crate::git_operation::WriteError {message, dispatched:true})?;
        crate::git_operation::Operation::decode(response, &operation.to_string()).map_err(
            |message| crate::git_operation::WriteError {
                message,
                dispatched: true,
            },
        )
    }
    /// Use the exact snapshot and opaque entry identity the user reviewed.
    /// Refreshing here would silently authorize changes made after that review.
    pub fn change_index(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        entry: &str,
        stage: bool,
        operation: uuid::Uuid,
    ) -> Result<crate::git_operation::Operation, crate::git_operation::WriteError> {
        self.change_index_entries(
            server,
            repo,
            snapshot,
            &[entry.to_owned()],
            stage,
            operation,
        )
    }
    pub fn change_index_selection(
        &self,
        server: &str,
        repo: &str,
        review: &crate::git_selection::Review,
        hunks: &[String],
        lines: Option<&[String]>,
        operation: uuid::Uuid,
    ) -> Result<crate::git_operation::Operation, crate::git_operation::WriteError> {
        let action = review.action(hunks, lines)?;
        self.dispatch_index_action(server, repo, review.snapshot(), action, operation)
    }
    pub fn discard_changes(
        &self,
        server: &str,
        repo: &str,
        review: &crate::git_discard::Review,
        source: crate::git_discard::Source,
        operation: uuid::Uuid,
    ) -> Result<crate::git_operation::Operation, crate::git_operation::WriteError> {
        self.dispatch_index_action(
            server,
            repo,
            review.snapshot(),
            review.action(source)?,
            operation,
        )
    }
    /// Apply the reviewed visible group as one snapshot-checked operation.
    pub fn change_index_entries(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        entries: &[String],
        stage: bool,
        operation: uuid::Uuid,
    ) -> Result<crate::git_operation::Operation, crate::git_operation::WriteError> {
        use serde_json::json;
        if repo.is_empty()
            || snapshot.is_empty()
            || entries.is_empty()
            || entries.iter().any(String::is_empty)
        {
            return Err("Select a file from a current repository status first".into());
        }
        // Agent paths() accepts at most 10,000 identities. Bound the request
        // before the desktop bridge creates a pending-operation receipt.
        const MAX_INDEX_ENTRIES: usize = 10_000;
        let too_large = || {
            crate::git_operation::WriteError::from(
                "Too many files or too much path data. Narrow the filter and stage/unstage fewer files.",
            )
        };
        if entries.len() > MAX_INDEX_ENTRIES {
            return Err(too_large());
        }
        self.dispatch_index_action(
            server,
            repo,
            snapshot,
            json!({"kind":if stage {"stage"} else {"unstage"},"entryIds":entries}),
            operation,
        )
    }
    fn dispatch_index_action(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        action: Value,
        operation: uuid::Uuid,
    ) -> Result<crate::git_operation::Operation, crate::git_operation::WriteError> {
        use serde_json::json;
        if repo.is_empty() || snapshot.is_empty() {
            return Err("A reviewed repository and snapshot are required".into());
        }
        const MAX_GIT_FRAME: usize = 1024 * 1024;
        let too_large = || {
            crate::git_operation::WriteError::from(
                "Selection is too large. Narrow the filter or select fewer changes.",
            )
        };
        let params = json!({"operationId":operation,"repoId":repo,"expectedSnapshot":snapshot,"action":action});
        // The v4 envelope is positional. String-keyed MessagePack is a safe
        // upper bound here: the agent codec replaces these field names with
        // smaller integer keys, and all leaves in this request are strings.
        // The transport creates a UUID request id with this same encoded size.
        let encoded = rmp_serde::to_vec(&(3u8, operation.to_string(), "operation.start", &params))
            .map_err(|_| too_large())?;
        if encoded.len() > MAX_GIT_FRAME {
            return Err(too_large());
        }
        self.call("git_pending_operations", json!({"serverId": server}))?;
        let response = self
            .call(
                "git_request",
                json!({"serverId":server,"request":{"method":"operation.start","params":params}}),
            )
            .map_err(|message| crate::git_operation::WriteError {
                message,
                dispatched: true,
            })?;
        crate::git_operation::Operation::decode(response, &operation.to_string()).map_err(
            |message| crate::git_operation::WriteError {
                message,
                dispatched: true,
            },
        )
    }
    /// Dispatch exactly once. The caller retains `operation` for explicit recovery
    /// even if any subsequent response is lost or malformed.
    pub fn fetch(
        &self,
        server: &str,
        repo: &str,
        remote: &str,
        operation: uuid::Uuid,
    ) -> Result<crate::git_operation::Operation, crate::git_operation::WriteError> {
        use serde_json::json;
        self.call("git_pending_operations", json!({"serverId":server}))?;
        let detail = self.call("git_request", json!({"serverId":server,"request":{"method":"repo.remote","params":{"repoId":repo,"name":remote}}}))?;
        if detail["name"].as_str() != Some(remote) {
            return Err("Remote identity changed; refresh before fetching".into());
        }
        let token = detail["token"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("Remote configuration token is missing")?;
        let status = self.call("git_request", json!({"serverId":server,"request":{"method":"repo.status","params":{"repoId":repo,"pageSize":1}}}))?;
        let snapshot = status["snapshot"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("Repository snapshot is missing")?;
        let value = self
            .call(
                "git_request",
                json!({"serverId":server,"request":{"method":"operation.start","params":{
                    "operationId":operation,"repoId":repo,"expectedSnapshot":snapshot,
                    "action":{"kind":"fetch","remote":remote,"expectedToken":token}
                }}}),
            )
            .map_err(|message| crate::git_operation::WriteError {
                message,
                dispatched: true,
            })?;
        crate::git_operation::Operation::decode(value, &operation.to_string()).map_err(|message| {
            crate::git_operation::WriteError {
                message,
                dispatched: true,
            }
        })
    }
    pub fn git_operation(
        &self,
        server: &str,
        operation: uuid::Uuid,
    ) -> Result<crate::git_operation::Operation, String> {
        let value = self.call("git_request", serde_json::json!({"serverId":server,"request":{"method":"operation.get","params":{"operationId":operation}}}))?;
        crate::git_operation::Operation::decode(value, &operation.to_string())
    }
    pub fn bootstrap(
        &self,
        server: &str,
        method: &str,
        mut params: Value,
        operation: uuid::Uuid,
    ) -> Result<crate::git_operation::Operation, crate::git_operation::WriteError> {
        if !matches!(method, "repo.init" | "repo.clone") || !params.is_object() {
            return Err("Invalid repository creation request".into());
        }
        self.call(
            "git_pending_operations",
            serde_json::json!({"serverId":server}),
        )?;
        params["operationId"] = serde_json::json!(operation);
        let value = self
            .call(
                "git_request",
                serde_json::json!({"serverId":server,"request":{"method":method,"params":params}}),
            )
            .map_err(|message| crate::git_operation::WriteError {
                message,
                dispatched: true,
            })?;
        crate::git_operation::Operation::decode(value, &operation.to_string()).map_err(|message| {
            crate::git_operation::WriteError {
                message,
                dispatched: true,
            }
        })
    }
    pub fn add_project(&self, args: Value) -> Result<Value, String> {
        let server = args["serverId"].as_str().ok_or("Missing server")?;
        let name = args["name"].as_str().unwrap_or("").trim();
        if name.is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
            return Err(
                "Enter a project name of up to 256 bytes without control characters.".into(),
            );
        }
        let repo = self.call("git_request", serde_json::json!({"serverId":server,"request":{"method":"repo.open","params":{"path":args["path"]}}}))?;
        let save = (|| {
            let root = repo["root"]["bytesB64"]
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or("Repository returned an invalid root path")?;
            let existing =
                self.call("git_projects_list", serde_json::json!({"serverId":server}))?;
            if let Some(project) = existing.as_array().and_then(|rows| {
                rows.iter()
                    .find(|row| row["path"]["bytesB64"].as_str() == Some(root))
            }) {
                return Ok(project.clone());
            }
            self.call("git_projects_save", serde_json::json!({"project":{"id":uuid::Uuid::new_v4(),"serverId":server,"name":name,"path":repo["root"]}}))
        })();
        self.release_repository(server, &repo);
        save
    }
    pub fn release_repository(&self, server: &str, repo: &Value) {
        if let Some(id) = repo["repoId"].as_str() {
            let _ = self.call("git_request", serde_json::json!({"serverId":server,"request":{"method":"repo.close","params":{"repoId":id}}}));
        }
    }
    pub fn discover() -> Self {
        let base = std::env::var_os("NEWPORT_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                #[cfg(windows)]
                let root = dirs::data_local_dir().unwrap_or_default();
                #[cfg(not(windows))]
                let root = dirs::data_dir().unwrap_or_default();
                default_profile(&root)
            });
        Self {
            path: newport_platform::ipc::profile_endpoint(&base),
        }
    }
    pub fn call(&self, method: &str, args: Value) -> Result<Value, String> {
        self.call_with_timeout(method, args, request_timeout(method))
    }
    pub fn call_with_timeout(
        &self,
        method: &str,
        args: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        self.call_detailed_with_timeout(method, args, timeout)
            .map_err(|error| error.message)
    }
    pub fn call_detailed(
        &self,
        method: &str,
        args: Value,
    ) -> Result<Value, crate::request_error::RequestError> {
        self.call_detailed_with_timeout(method, args, request_timeout(method))
    }
    fn call_detailed_with_timeout(
        &self,
        method: &str,
        args: Value,
        timeout: Duration,
    ) -> Result<Value, crate::request_error::RequestError> {
        let git_page = method == "git_request"
            && args["request"]["method"]
                .as_str()
                .is_some_and(crate::git_page::is_page_method);
        let diff = if method == "git_request" {
            match args["request"]["method"].as_str() {
                Some("repo.diff_page") => Some(true),
                Some("repo.commit_diff_page") => Some(false),
                _ => None,
            }
        } else {
            None
        };
        let from_start = args["request"]["params"]["cursor"].is_null();
        let (mut value, _stream) = self.exchange_with_timeout(method, args, timeout)?;
        let validation = if let Some(working) = diff {
            crate::git_diff_page::decode(&mut value, working, from_start)
        } else if git_page {
            crate::git_page::validate(&value)
        } else {
            Ok(())
        };
        validation.map_err(|message| crate::request_error::RequestError {
            message,
            code: Some("PROTOCOL_ERROR".into()),
        })?;
        Ok(value)
    }

    pub(crate) fn open_terminal_lease(&self, args: Value) -> Result<Stream, String> {
        self.exchange_with_timeout(protocol::TERMINAL_OPEN_LEASE, args, Duration::from_secs(10))
            .map(|(_, stream)| stream)
            .map_err(|error| error.message)
    }

    fn exchange_with_timeout(
        &self,
        method: &str,
        args: Value,
        timeout: Duration,
    ) -> Result<(Value, Stream), crate::request_error::RequestError> {
        crate::agent_relay::await_registration(&self.path)?;
        let mut stream = newport_platform::ipc::connect_private(&self.path).map_err(|_| {
            format!(
                "Cannot reach Newport's backend. Start Newport with --native-client. Socket: {}",
                self.path.display()
            )
        })?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|e| e.to_string())?;
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .map_err(|e| e.to_string())?;
        let data = rmp_serde::to_vec_named(&Request {
            version: VERSION,
            method: method.into(),
            args,
        })
        .map_err(|e| e.to_string())?;
        if data.len() > MAX_REQUEST {
            return Err("Request is too large".into());
        }
        // Once any bytes have been sent, a disconnected write may have completed.
        let exchange = (|| -> Result<Value, String> {
            stream.write_all(&(data.len() as u32).to_be_bytes()).map_err(|e| e.to_string())?;
            stream.write_all(&data).map_err(|e| e.to_string())?;
            let mut awaiting_agent = false;
            let mut remaining = timeout;
            loop {
                if remaining.is_zero() { return Err("Backend response timed out".into()); }
                // Consent frames are sent at least every five seconds. Keep a
                // bounded liveness timeout even while the user is deciding.
                update_response_timeout(&stream, if awaiting_agent {
                    Duration::from_secs(30)
                } else { remaining }).map_err(|e| format!("Setting response timeout: {e}"))?;
                let started = std::time::Instant::now();
                let mut header = [0;4]; stream.read_exact(&mut header).map_err(|e| format!("Reading response header: {e}"))?;
                let len = u32::from_be_bytes(header) as usize;
                if !(1..=MAX_RESPONSE).contains(&len) { return Err("Invalid backend response size".into()); }
                let mut data = vec![0;len]; stream.read_exact(&mut data).map_err(|e| format!("Reading response body: {e}"))?;
                if !awaiting_agent { remaining = remaining.saturating_sub(started.elapsed()); }
                let reply: ResponseFrame = rmp_serde::from_slice(&data).map_err(|e| e.to_string())?;
                match reply {
                    ResponseFrame::AgentApproval(progress) => {
                        if progress.version != VERSION { return Err("Unsupported backend version".into()); }
                        awaiting_agent = progress.awaiting_agent;
                    }
                    ResponseFrame::Complete(reply) => {
                        if reply.version != VERSION { return Err("Unsupported backend version".into()); }
                        return Ok(serde_json::json!({"result":reply.result,"error":reply.error}));
                    }
                }
            }
        })().map_err(|e| {
            // These metrics reads cannot have an ambiguous remote write outcome.
            // Unknown commands retain the conservative warning and are never replayed.
            if matches!(method, "cockpit_history" | "cockpit_collect") {
                format!("Couldn’t load server metrics: backend connection interrupted ({e}).")
            } else {
                format!("Backend connection interrupted: {e}. The request was not retried; check the outcome before repeating a write.")
            }
        })?;
        if !exchange["error"].is_null() {
            return Err(crate::request_error::RequestError::from_response(
                &exchange["error"],
            ));
        }
        Ok((exchange["result"].clone(), stream))
    }
}

fn default_profile(root: &std::path::Path) -> PathBuf {
    // Match the shipping host's legacy-profile preference. A named pipe has no
    // filesystem socket entry; endpoint existence cannot select the profile.
    let legacy = root.join("Porthop");
    if legacy.try_exists().unwrap_or(true) {
        legacy
    } else {
        root.join("Newport")
    }
}

// Agent setup can upload for up to 15 minutes; allow its result to reach the UI.
fn request_timeout(method: &str) -> Duration {
    Duration::from_secs(
        if matches!(
            method,
            "cockpit_collect" | "reinstall_agent" | "setup_install" | "setup_finish"
        ) {
            960
        } else {
            600
        },
    )
}

fn update_response_timeout(stream: &Stream, timeout: Duration) -> std::io::Result<()> {
    match stream.set_read_timeout(Some(timeout)) {
        Ok(()) => Ok(()),
        // Darwin rejects setsockopt after the peer closes, including when the
        // final response is still buffered. This races an approval=false frame
        // immediately followed by completion and close. Drain nonblocking so a
        // complete reply survives; partial/missing data still fails immediately
        // and never turns an uncertain write into a successful one.
        #[cfg(target_os = "macos")]
        Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {
            stream.set_nonblocking(true)
        }
        Err(error) => Err(error),
    }
}

pub fn error_text(value: &Value) -> String {
    value
        .as_str()
        .or_else(|| value["message"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}

#[cfg(test)]
mod creation_transport_tests {
    use super::{
        Backend,
        protocol::{Request, Response, VERSION},
    };
    use newport_platform::ipc::{Listener, Stream};
    use serde_json::{Value, json};
    use std::{
        io::{Read, Write},
        path::PathBuf,
        thread,
        time::{Duration, Instant},
    };

    struct Host {
        path: PathBuf,
        listener: Listener,
    }
    #[test]
    fn profile_discovery_matches_the_host_without_requiring_a_socket_file() {
        let root = std::env::temp_dir().join(format!("np-profile-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        assert_eq!(super::default_profile(&root), root.join("Newport"));
        std::fs::create_dir(root.join("Porthop")).unwrap();
        std::fs::create_dir(root.join("Newport")).unwrap();
        assert_eq!(super::default_profile(&root), root.join("Porthop"));
        std::fs::remove_dir(root.join("Porthop")).unwrap();
        std::fs::write(root.join("Porthop"), "invalid legacy profile").unwrap();
        // Preserve the host's refusal to fall back to a different profile.
        assert_eq!(super::default_profile(&root), root.join("Porthop"));
        std::fs::remove_dir_all(root).unwrap();
    }
    impl Host {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("np-{}.sock", uuid::Uuid::new_v4().simple()));
            let listener = Listener::bind(&path).unwrap();
            listener.set_nonblocking(true).unwrap();
            Self { path, listener }
        }
        fn receive(&self) -> (Stream, Request) {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match self.listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(2))
                    }
                    other => panic!("Expected backend request: {other:?}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut header = [0; 4];
            stream.read_exact(&mut header).unwrap();
            let size = u32::from_be_bytes(header) as usize;
            assert!(size <= super::protocol::MAX_REQUEST);
            let mut data = vec![0; size];
            stream.read_exact(&mut data).unwrap();
            let request: Request = rmp_serde::from_slice(&data).unwrap();
            assert_eq!(request.version, VERSION);
            (stream, request)
        }
        fn reply(stream: &mut Stream, result: Value, error: Option<Value>) {
            let data = rmp_serde::to_vec_named(&Response {
                version: VERSION,
                result,
                error,
            })
            .unwrap();
            stream
                .write_all(&(data.len() as u32).to_be_bytes())
                .unwrap();
            stream.write_all(&data).unwrap();
        }
    }
    impl Drop for Host {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
    fn success(id: uuid::Uuid) -> Value {
        json!({"operationId":id,"repository":"created-repo","payloadHash":"hash","state":"succeeded","seq":1,"result":{"path":{"display":"/canonical","bytesB64":"L2Nhbm9uaWNhbA=="}}})
    }

    #[test]
    fn terminal_lease_child_process() {
        let Some(path) = std::env::var_os("NEWPORT_TEST_TERMINAL_LEASE_SOCKET") else {
            return;
        };
        let backend = Backend::at_path(path.into());
        let _lease = backend
            .open_terminal_lease(json!({"session":"owned"}))
            .unwrap();
        println!("LEASE_READY");
        std::io::stdout().flush().unwrap();
        loop {
            thread::park();
        }
    }

    #[test]
    fn terminal_lease_closes_when_client_process_is_killed_without_destructors() {
        use std::io::BufRead;
        struct Child(std::process::Child);
        impl Drop for Child {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let host = Host::new();
        let mut child = Child(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "backend::creation_transport_tests::terminal_lease_child_process",
                    "--nocapture",
                ])
                .env("NEWPORT_TEST_TERMINAL_LEASE_SOCKET", &host.path)
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let output = child.0.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            for line in std::io::BufReader::new(output)
                .lines()
                .map_while(Result::ok)
            {
                if line == "LEASE_READY" {
                    let _ = tx.send(());
                    break;
                }
            }
        });
        let (mut stream, request) = host.receive();
        assert_eq!(request.method, super::protocol::TERMINAL_OPEN_LEASE);
        Host::reply(&mut stream, Value::Null, None);
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        assert_eq!(stream.read(&mut [0]).unwrap(), 0);
    }

    #[test]
    fn terminal_lease_rejects_failed_open_and_releases_the_socket() {
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let task = thread::spawn(move || {
            let (mut stream, request) = host.receive();
            assert_eq!(request.method, super::protocol::TERMINAL_OPEN_LEASE);
            Host::reply(
                &mut stream,
                Value::Null,
                Some(json!("Terminal session already exists.")),
            );
            assert_eq!(stream.read(&mut [0]).unwrap(), 0);
        });
        assert_eq!(
            backend.open_terminal_lease(json!({})).unwrap_err(),
            "Terminal session already exists."
        );
        task.join().unwrap();
    }

    #[test]
    fn all_git_page_reads_validate_the_wire_envelope_before_publishing() {
        const METHODS: &[&str] = &[
            "repo.status",
            "repo.history",
            "repo.commit_files",
            "repo.branches",
            "repo.remote_names",
            "repo.remote_refs",
            "repo.stashes",
            "repo.tags",
            "repo.worktrees",
            "repo.blob_page",
            "repo.diff_page",
            "repo.commit_diff_page",
        ];
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        fn response(method: &str) -> Value {
            match method {
                "repo.diff_page" => crate::git_diff_page::tests::page(true),
                "repo.commit_diff_page" => crate::git_diff_page::tests::page(false),
                _ => json!({"snapshot":"s","entries":[],"nextCursor":null,"metadata":{}}),
            }
        }
        let task = thread::spawn(move || {
            for method in METHODS {
                for malformed in [false, true] {
                    let (mut stream, request) = host.receive();
                    assert_eq!(request.method, "git_request");
                    assert_eq!(request.args["request"]["method"], *method);
                    let mut response = response(method);
                    if malformed {
                        response.as_object_mut().unwrap().remove("nextCursor");
                    }
                    Host::reply(&mut stream, response, None);
                }
            }
        });
        let mut resources = crate::resources::Resources::default();
        for method in METHODS {
            let expected = response(method);
            let args = json!({"serverId":"s","request":{"method":method,"params":{"repoId":"r"}}});
            let generation = resources.begin(method).unwrap();
            resources.complete_detailed(
                method,
                generation,
                backend.call_detailed("git_request", args.clone()),
            );
            assert_eq!(resources.get(method).unwrap().value, expected, "{method}");
            let generation = resources.begin(method).unwrap();
            resources.complete_detailed(
                method,
                generation,
                backend.call_detailed("git_request", args),
            );
            let resource = resources.get(method).unwrap();
            assert_eq!(
                resource.error_code.as_deref(),
                Some("PROTOCOL_ERROR"),
                "{method}"
            );
            assert_eq!(
                resource.value, expected,
                "failed refresh must retain accepted data"
            );
            assert!(!resource.pending);
        }
        task.join().unwrap();
    }

    #[test]
    fn malformed_diff_content_keeps_the_accepted_page_and_ends_loading() {
        for working in [true, false] {
            let method = if working {
                "repo.diff_page"
            } else {
                "repo.commit_diff_page"
            };
            let host = Host::new();
            let backend = Backend::at_path(host.path.clone());
            let valid = crate::git_diff_page::tests::page(working);
            let mut invalid = valid.clone();
            invalid["entries"][0]["hunks"] = json!("not an array");
            let task = thread::spawn(move || {
                let (mut stream, _) = host.receive();
                Host::reply(&mut stream, invalid, None);
            });
            let mut resources = crate::resources::Resources::default();
            let generation = resources.begin(method).unwrap();
            resources.complete(method, generation, Ok(valid.clone()));
            let generation = resources.begin(method).unwrap();
            let args = json!({"serverId":"s","request":{"method":method,"params":{"repoId":"r"}}});
            resources.complete_detailed(
                method,
                generation,
                backend.call_detailed("git_request", args),
            );
            let state = resources.get(method).unwrap();
            assert_eq!(state.value, valid);
            assert_eq!(state.error_code.as_deref(), Some("PROTOCOL_ERROR"));
            assert!(!state.pending);
            task.join().unwrap();
        }
    }

    #[test]
    fn git_envelope_validation_does_not_intercept_errors_or_nonpage_responses() {
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let task = thread::spawn(move || {
            for method in [
                "repo.open",
                "repo.status_summary",
                "repo.remotes",
                "repo.diff",
                "repo.commit_diff",
                "operation.get",
            ] {
                let (mut stream, request) = host.receive();
                assert_eq!(request.args["request"]["method"], method);
                Host::reply(&mut stream, json!({"nonPage":"unchanged"}), None);
            }
            let (mut stream, _) = host.receive();
            Host::reply(
                &mut stream,
                Value::Null,
                Some(json!({"code":"STALE_SNAPSHOT","message":"Refresh the repository"})),
            );
        });
        for method in [
            "repo.open",
            "repo.status_summary",
            "repo.remotes",
            "repo.diff",
            "repo.commit_diff",
            "operation.get",
        ] {
            assert_eq!(
                backend
                    .call("git_request", json!({"request":{"method":method}}))
                    .unwrap(),
                json!({"nonPage":"unchanged"})
            );
        }
        let error = backend
            .call_detailed("git_request", json!({"request":{"method":"repo.status"}}))
            .unwrap_err();
        assert_eq!(error.code.as_deref(), Some("STALE_SNAPSHOT"));
        assert_eq!(error.message, "Refresh the repository");
        task.join().unwrap();
    }

    #[test]
    fn repository_settings_invalid_response_is_retryable_and_keeps_protocol_errors() {
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let valid = json!({"configuration":{"token":"token","fetchPrune":false,"userName":null,"userEmail":null}});
        let expected = valid.clone();
        let task = thread::spawn(move || {
            for (result, error) in [
                (
                    json!({"configuration":{"token":"token","fetchPrune":"false"}}),
                    None,
                ),
                (valid, None),
                (
                    Value::Null,
                    Some(
                        json!({"code":"REPOSITORY_CLOSED","message":"Open the repository again."}),
                    ),
                ),
            ] {
                let (mut stream, request) = host.receive();
                assert_eq!(request.method, "git_request");
                assert_eq!(
                    request.args,
                    json!({"serverId":"server","request":{"method":"repo.remotes","params":{"repoId":"repo"}}})
                );
                Host::reply(&mut stream, result, error);
            }
        });
        let mut resources = crate::resources::Resources::default();
        let generation = resources.begin("settings").unwrap();
        assert!(resources.complete_detailed(
            "settings",
            generation,
            backend.repository_settings("server", "repo")
        ));
        let failed = resources.get("settings").unwrap();
        assert!(!failed.pending);
        assert!(failed.value.is_null());
        assert!(failed.error.as_ref().unwrap().contains("Retry settings"));
        let generation = resources.begin("settings").unwrap();
        assert!(resources.complete_detailed(
            "settings",
            generation,
            backend.repository_settings("server", "repo")
        ));
        let ready = resources.get("settings").unwrap();
        assert_eq!(ready.value, expected);
        assert!(ready.error.is_none());
        let error = backend.repository_settings("server", "repo").unwrap_err();
        assert_eq!(error.code.as_deref(), Some("REPOSITORY_CLOSED"));
        task.join().unwrap();
    }

    #[test]
    fn detailed_reads_preserve_protocol_codes_without_parsing_messages() {
        for error in [
            json!({"code":"STALE_SNAPSHOT","message":"The repository changed"}),
            json!("STALE_SNAPSHOT: an ordinary unstructured error"),
        ] {
            let host = Host::new();
            let backend = Backend::at_path(host.path.clone());
            let expected = crate::request_error::RequestError::from_response(&error);
            let worker = thread::spawn(move || {
                let (mut stream, request) = host.receive();
                assert_eq!(request.args["request"]["method"], "repo.diff_page");
                Host::reply(&mut stream, Value::Null, Some(error));
                drop(stream);
                assert!(
                    matches!(host.listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
                );
            });
            assert_eq!(
                backend
                    .call_detailed(
                        "git_request",
                        json!({"request":{"method":"repo.diff_page"}})
                    )
                    .unwrap_err(),
                expected
            );
            worker.join().unwrap();
        }
    }

    #[test]
    fn partial_index_write_keeps_the_reviewed_ids_and_snapshot() {
        let page = json!({"nextCursor":null,"metadata":{"readOnly":false,"totalFiles":1,"sourceSnapshot":"reviewed","entryId":"file","side":"index_to_worktree","contextLines":3},"entries":[{"fileIndex":0,"status":"Modified","oldMode":33188,"newMode":33188,"oldPath":{"bytesB64":"Zg=="},"newPath":{"bytesB64":"Zg=="},"hunks":[{"id":"hunk","lines":[{"id":"line","origin":"+","lineComplete":true}]}]}]});
        let review = crate::git_selection::Review::from_page(&page).unwrap();
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let id = uuid::Uuid::new_v4();
        let worker = thread::spawn(move || {
            let (mut stream, request) = host.receive();
            assert_eq!(request.method, "git_pending_operations");
            Host::reply(&mut stream, json!([]), None);
            drop(stream);
            let (mut stream, request) = host.receive();
            assert_eq!(
                request.args["request"]["params"],
                json!({"operationId":id,"repoId":"repo","expectedSnapshot":"reviewed","action":{"kind":"stage","entryIds":["file"],"hunks":{"ids":["hunk"],"lines":["line"],"contextLines":3}}})
            );
            Host::reply(&mut stream, success(id), None);
        });
        backend
            .change_index_selection(
                "server",
                "repo",
                &review,
                &["hunk".into()],
                Some(&["line".into()]),
                id,
            )
            .unwrap();
        worker.join().unwrap();
        let backend = Backend::at_path(PathBuf::from("/nonexistent-newport-test.sock"));
        let error = backend
            .change_index_selection("server", "repo", &review, &["stale-hunk".into()], None, id)
            .unwrap_err();
        assert!(!error.dispatched);
    }

    #[test]
    fn discard_dispatches_once_with_the_reviewed_snapshot_and_restore_source() {
        for source in [
            crate::git_discard::Source::Index,
            crate::git_discard::Source::Head,
        ] {
            let entry = json!({"entryId":"e","unstaged":true});
            let review = crate::git_discard::Review::file(
                &json!({"snapshot":"reviewed","entries":[entry]}),
                &entry,
            )
            .unwrap();
            let expected = review.action(source).unwrap();
            let host = Host::new();
            let backend = Backend::at_path(host.path.clone());
            let id = uuid::Uuid::new_v4();
            let worker = thread::spawn(move || {
                let (mut stream, request) = host.receive();
                assert_eq!(request.method, "git_pending_operations");
                Host::reply(&mut stream, json!([]), None);
                drop(stream);
                let (mut stream, request) = host.receive();
                assert_eq!(
                    request.args["request"]["params"],
                    json!({"operationId":id,"repoId":"repo","expectedSnapshot":"reviewed","action":expected})
                );
                Host::reply(&mut stream, success(id), None);
            });
            backend
                .discard_changes("server", "repo", &review, source, id)
                .unwrap();
            worker.join().unwrap();
        }
    }
    #[test]
    fn index_group_uses_one_operation_and_the_reviewed_snapshot() {
        for stage in [true, false] {
            let host = Host::new();
            let backend = Backend::at_path(host.path.clone());
            let id = uuid::Uuid::new_v4();
            let worker = thread::spawn(move || {
                let (mut stream, request) = host.receive();
                assert_eq!(request.method, "git_pending_operations");
                Host::reply(&mut stream, json!([]), None);
                drop(stream);
                let (mut stream, request) = host.receive();
                let request = &request.args["request"];
                assert_eq!(request["method"], "operation.start");
                assert_eq!(request["params"]["expectedSnapshot"], "reviewed");
                assert_eq!(
                    request["params"]["action"],
                    json!({"kind":if stage {"stage"}else{"unstage"},"entryIds":["opaque-a","opaque-b"]})
                );
                Host::reply(&mut stream, success(id), None);
            });
            let result = backend
                .change_index_entries(
                    "server",
                    "repo",
                    "reviewed",
                    &["opaque-a".into(), "opaque-b".into()],
                    stage,
                    id,
                )
                .unwrap();
            assert_eq!(result.operation_id, id.to_string());
            worker.join().unwrap();
        }
    }
    #[test]
    fn empty_index_group_is_rejected_before_dispatch() {
        let backend = Backend::at_path(PathBuf::from("/nonexistent-newport-test.sock"));
        for entries in [vec![], vec![String::new()]] {
            let error = backend
                .change_index_entries(
                    "server",
                    "repo",
                    "reviewed",
                    &entries,
                    true,
                    uuid::Uuid::new_v4(),
                )
                .unwrap_err();
            assert!(!error.dispatched);
            assert!(error.message.contains("Select a file"));
        }
    }

    #[test]
    fn oversized_index_groups_fail_before_any_transport_or_receipt() {
        let backend = Backend::at_path(PathBuf::from("/nonexistent-newport-test.sock"));
        for entries in [
            vec!["id".to_owned(); 10_001],
            vec!["a".repeat(600_000), "b".repeat(600_000)],
        ] {
            let error = backend
                .change_index_entries(
                    "server",
                    "repo",
                    "reviewed",
                    &entries,
                    true,
                    uuid::Uuid::new_v4(),
                )
                .unwrap_err();
            assert!(!error.dispatched);
            assert!(error.message.contains("Narrow the filter"));
        }
    }

    fn approval(stream: &mut Stream, awaiting_agent: bool) {
        let data = rmp_serde::to_vec_named(&super::protocol::AgentApproval {
            version: VERSION,
            awaiting_agent,
        })
        .unwrap();
        stream
            .write_all(&(data.len() as u32).to_be_bytes())
            .unwrap();
        stream.write_all(&data).unwrap();
    }

    #[test]
    fn consent_wait_outlives_request_deadline_without_replaying() {
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let task = thread::spawn(move || {
            let (mut stream, request) = host.receive();
            assert_eq!(request.method, "test_connection");
            approval(&mut stream, true);
            thread::sleep(Duration::from_millis(350));
            approval(&mut stream, false);
            Host::reply(&mut stream, Value::Bool(true), None);
            assert!(matches!(host.listener.accept(), Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock));
        });
        assert_eq!(
            backend
                .call_with_timeout("test_connection", json!({}), Duration::from_millis(150))
                .unwrap(),
            Value::Bool(true)
        );
        task.join().unwrap();
    }

    #[test]
    fn network_deadline_resumes_when_consent_finishes() {
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let task = thread::spawn(move || {
            let (mut stream, _) = host.receive();
            approval(&mut stream, true);
            thread::sleep(Duration::from_millis(350));
            approval(&mut stream, false);
            thread::sleep(Duration::from_millis(350));
        });
        let started = Instant::now();
        let error = backend
            .call_with_timeout("test_connection", json!({}), Duration::from_millis(150))
            .unwrap_err();
        assert!(error.contains("not retried"));
        assert!(started.elapsed() >= Duration::from_millis(450));
        task.join().unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn closed_peer_keeps_buffered_final_frame_after_timeout_update() {
        let (mut stream, mut peer) = Stream::pair().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        approval(&mut peer, false);
        Host::reply(&mut peer, json!(true), None);
        drop(peer);
        for expected_completion in [false, true] {
            super::update_response_timeout(&stream, Duration::from_millis(150)).unwrap();
            let mut header = [0; 4];
            stream.read_exact(&mut header).unwrap();
            let mut bytes = vec![0; u32::from_be_bytes(header) as usize];
            stream.read_exact(&mut bytes).unwrap();
            let frame: super::ResponseFrame = rmp_serde::from_slice(&bytes).unwrap();
            match frame {
                super::ResponseFrame::Complete(response) => {
                    assert!(expected_completion);
                    assert_eq!(response.result, json!(true));
                }
                super::ResponseFrame::AgentApproval(progress) => {
                    assert!(!expected_completion);
                    assert!(!progress.awaiting_agent);
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn closed_peer_with_partial_reply_remains_an_error() {
        let (mut stream, mut peer) = Stream::pair().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        peer.write_all(&[0, 0]).unwrap();
        drop(peer);
        super::update_response_timeout(&stream, Duration::from_millis(150)).unwrap();
        let mut header = [0; 4];
        assert_eq!(
            stream.read_exact(&mut header).unwrap_err().kind(),
            std::io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn interrupted_metrics_read_has_no_write_warning_and_is_not_replayed() {
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let task = thread::spawn(move || {
            let (stream, request) = host.receive();
            assert_eq!(request.method, "cockpit_history");
            drop(stream);
            thread::sleep(Duration::from_millis(100));
            assert!(matches!(host.listener.accept(), Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock));
        });
        let error = backend
            .call_with_timeout("cockpit_history", json!({}), Duration::from_secs(1))
            .unwrap_err();
        assert!(error.contains("Couldn’t load server metrics"));
        assert!(!error.contains("write"));
        task.join().unwrap();
    }

    #[test]
    fn interrupted_creation_is_not_replayed_and_recovers_by_same_operation_id() {
        let host = Host::new();
        let backend = Backend {
            path: host.path.clone(),
        };
        let id = uuid::Uuid::new_v4();
        let task = thread::spawn(move || {
            let (mut stream, request) = host.receive();
            assert_eq!(request.method, "git_pending_operations");
            Host::reply(&mut stream, json!([]), None);
            drop(stream);
            let (stream, request) = host.receive();
            assert_eq!(request.method, "git_request");
            assert_eq!(request.args["request"]["method"], "repo.clone");
            assert_eq!(
                request.args["request"]["params"]["operationId"],
                id.to_string()
            );
            assert_eq!(request.args["request"]["params"]["url"], "/origin.git");
            drop(stream);
            let (mut stream, request) = host.receive();
            assert_eq!(
                request.args["request"]["method"], "operation.get",
                "Creation must not be replayed"
            );
            assert_eq!(
                request.args["request"]["params"]["operationId"],
                id.to_string()
            );
            Host::reply(&mut stream, success(id), None);
        });
        let error = backend
            .bootstrap(
                "server",
                "repo.clone",
                json!({"url":"/origin.git","operationId":"untrusted-stale-id"}),
                id,
            )
            .unwrap_err();
        assert!(error.dispatched);
        assert!(error.message.contains("not retried"));
        let recovered = backend.git_operation("server", id).unwrap();
        assert_eq!(recovered.operation_id, id.to_string());
        assert_eq!(recovered.state, crate::git_operation::State::Succeeded);
        task.join().unwrap();
    }
    #[test]
    fn failed_preflight_never_dispatches_creation() {
        let host = Host::new();
        let backend = Backend {
            path: host.path.clone(),
        };
        let task = thread::spawn(move || {
            let (mut stream, request) = host.receive();
            assert_eq!(request.method, "git_pending_operations");
            Host::reply(
                &mut stream,
                Value::Null,
                Some(json!({"message":"Storage unavailable"})),
            );
        });
        let error = backend
            .bootstrap("server", "repo.init", json!({}), uuid::Uuid::new_v4())
            .unwrap_err();
        assert!(!error.dispatched);
        assert_eq!(error.message, "Storage unavailable");
        task.join().unwrap();
    }

    #[test]
    fn reviewed_branch_actions_preserve_snapshot_oid_token_and_explicit_force() {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let wire = |text: &str| json!({"display":text,"bytesB64":STANDARD.encode(text)});
        let entry = |name: &str, remote: bool| {
            json!({
                "name":wire(name),
                "reference":wire(&format!("refs/{}/{name}",if remote {"remotes"} else {"heads"})),
                "remote":remote,"current":false,"upstream":null,
                "oid":{"algorithm":"sha1","hex":"a".repeat(40)},
                "tracking":{"editable":true,"token":"reviewed-tracking"}
            })
        };
        let review = crate::git_branch::BranchReview::from_entry(
            "reviewed-repo",
            "reviewed-snapshot",
            &entry("topic", false),
        )
        .unwrap();
        let actions = [
            review.delete_action(false).unwrap(),
            review.delete_action(true).unwrap(),
            review
                .set_upstream_action(Some(&entry("origin/main", true)))
                .unwrap(),
            review
                .set_upstream_action(Some(&entry("origin/main", false)))
                .unwrap(),
            review.set_upstream_action(None).unwrap(),
        ];
        for action in actions {
            let host = Host::new();
            let backend = Backend::at_path(host.path.clone());
            let id = uuid::Uuid::new_v4();
            let expected = action.clone();
            let task = thread::spawn(move || {
                let (mut stream, request) = host.receive();
                assert_eq!(request.method, "git_pending_operations");
                assert_eq!(request.args, json!({"serverId":"reviewed-server"}));
                Host::reply(&mut stream, json!([]), None);
                drop(stream);
                let (mut stream, request) = host.receive();
                assert_eq!(request.method, "git_request");
                assert_eq!(
                    request.args,
                    json!({"serverId":"reviewed-server","request":{
                        "method":"operation.start","params":{
                            "operationId":id,"repoId":"reviewed-repo",
                            "expectedSnapshot":"reviewed-snapshot","action":expected,
                        }
                    }})
                );
                Host::reply(&mut stream, success(id), None);
                assert!(matches!(host.listener.accept(), Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock));
            });
            let outcome = backend
                .apply_reviewed_action(
                    "reviewed-server",
                    review.repo_id(),
                    review.snapshot(),
                    action,
                    id,
                )
                .unwrap();
            assert_eq!(outcome.operation_id, id.to_string());
            assert_eq!(outcome.state, crate::git_operation::State::Succeeded);
            task.join().unwrap();
        }
    }

    #[test]
    fn branch_action_disconnect_is_not_replayed_and_recovers_original_operation_id() {
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let id = uuid::Uuid::new_v4();
        let task = thread::spawn(move || {
            let (mut stream, request) = host.receive();
            assert_eq!(request.method, "git_pending_operations");
            Host::reply(&mut stream, json!([]), None);
            drop(stream);
            let (stream, request) = host.receive();
            assert_eq!(request.args["request"]["method"], "operation.start");
            assert_eq!(
                request.args["request"]["params"]["action"]["kind"],
                "branch.delete"
            );
            assert_eq!(
                request.args["request"]["params"]["operationId"],
                id.to_string()
            );
            drop(stream);
            let (mut stream, request) = host.receive();
            assert_eq!(
                request.args["request"]["method"], "operation.get",
                "Never replay a disconnected branch write"
            );
            assert_eq!(
                request.args["request"]["params"]["operationId"],
                id.to_string()
            );
            Host::reply(&mut stream, success(id), None);
        });
        let failure = backend.apply_reviewed_action(
            "server", "repo", "snapshot",
            json!({"kind":"branch.delete","name":"topic","expectedOid":"a".repeat(40),"force":false}), id,
        ).unwrap_err();
        assert!(failure.dispatched);
        assert!(failure.message.contains("not retried"));
        assert_eq!(
            backend.git_operation("server", id).unwrap().state,
            crate::git_operation::State::Succeeded
        );
        task.join().unwrap();
    }

    #[test]
    fn branch_action_failed_preflight_does_not_dispatch() {
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let task = thread::spawn(move || {
            let (mut stream, request) = host.receive();
            assert_eq!(request.method, "git_pending_operations");
            Host::reply(
                &mut stream,
                Value::Null,
                Some(json!("Receipt storage unavailable")),
            );
        });
        let failure = backend.apply_reviewed_action(
            "server", "repo", "snapshot",
            json!({"kind":"branch.set_upstream","name":"topic","expectedOid":"a".repeat(40),"expectedToken":"tracking","upstream":null}), uuid::Uuid::new_v4(),
        ).unwrap_err();
        assert!(!failure.dispatched);
        assert_eq!(failure.message, "Receipt storage unavailable");
        task.join().unwrap();
    }
    #[test]
    fn malformed_creation_outcome_remains_uncertain() {
        let host = Host::new();
        let backend = Backend {
            path: host.path.clone(),
        };
        let id = uuid::Uuid::new_v4();
        let task = thread::spawn(move || {
            let (mut stream, _) = host.receive();
            Host::reply(&mut stream, json!([]), None);
            drop(stream);
            let (mut stream, _) = host.receive();
            Host::reply(&mut stream, success(uuid::Uuid::new_v4()), None);
        });
        assert!(
            backend
                .bootstrap("server", "repo.init", json!({}), id)
                .unwrap_err()
                .dispatched
        );
        task.join().unwrap();
    }
    #[test]
    fn failed_bookmark_save_releases_open_handle_and_preserves_canonical_byte_path() {
        let host = Host::new();
        let backend = Backend {
            path: host.path.clone(),
        };
        let canonical = json!({"display":"/canonical�","bytesB64":"L2Nhbm9uaWNhbP8="});
        let task = thread::spawn(move || {
            let (mut stream, request) = host.receive();
            assert_eq!(request.args["request"]["method"], "repo.open");
            Host::reply(
                &mut stream,
                json!({"repoId":"new-handle","root":canonical}),
                None,
            );
            drop(stream);
            let (mut stream, request) = host.receive();
            assert_eq!(request.method, "git_projects_list");
            Host::reply(&mut stream, json!([]), None);
            drop(stream);
            let (mut stream, request) = host.receive();
            assert_eq!(request.method, "git_projects_save");
            assert_eq!(request.args["project"]["path"], canonical);
            Host::reply(&mut stream, Value::Null, Some(json!("Disk full")));
            drop(stream);
            let (mut stream, request) = host.receive();
            assert_eq!(request.args["request"]["method"], "repo.close");
            assert_eq!(request.args["request"]["params"]["repoId"], "new-handle");
            Host::reply(&mut stream, Value::Null, None);
        });
        let error=backend.add_project(json!({"serverId":"server","name":"Project","path":{"display":"/alias","bytesB64":"L2FsaWFz"}})).unwrap_err();
        assert_eq!(error, "Disk full");
        task.join().unwrap();
    }
    #[test]
    fn bookmark_retry_after_a_lost_save_reply_reuses_the_saved_project_without_git_creation() {
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let path = json!({"display":"/canonical�","bytesB64":"L2Nhbm9uaWNhbP8="});
        let args = json!({"serverId":"server","name":"Project","path":path});
        let saved = json!({"id":"saved-once","serverId":"server","name":"Project","path":path});
        let expected = saved.clone();
        let task = thread::spawn(move || {
            for attempt in 0..2 {
                let (mut stream, request) = host.receive();
                assert_eq!(request.args["request"]["method"], "repo.open");
                assert_eq!(request.args["request"]["params"]["path"], path);
                Host::reply(&mut stream, json!({"repoId":"handle","root":path}), None);
                drop(stream);
                let (mut stream, request) = host.receive();
                assert_eq!(request.method, "git_projects_list");
                Host::reply(
                    &mut stream,
                    if attempt == 0 {
                        json!([])
                    } else {
                        json!([saved])
                    },
                    None,
                );
                drop(stream);
                if attempt == 0 {
                    let (stream, request) = host.receive();
                    assert_eq!(request.method, "git_projects_save");
                    assert_eq!(request.args["project"]["path"], path);
                    // The store committed, but its response was lost.
                    drop(stream);
                }
                let (mut stream, request) = host.receive();
                assert_eq!(request.args["request"]["method"], "repo.close");
                assert_eq!(request.args["request"]["params"]["repoId"], "handle");
                Host::reply(&mut stream, Value::Null, None);
            }
        });
        assert!(backend.add_project(args.clone()).is_err());
        assert_eq!(backend.add_project(args).unwrap(), expected);
        task.join().unwrap();
    }
    #[test]
    fn tag_push_sends_exact_reviewed_identity_once() {
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let id = uuid::Uuid::new_v4();
        let action = json!({"kind":"tag.push","name":"v1","expectedOid":"b".repeat(40),"remote":"backup","expectedToken":"reviewed-config"});
        let expected = action.clone();
        let task = thread::spawn(move || {
            let (mut stream, request) = host.receive();
            assert_eq!(request.method, "git_pending_operations");
            Host::reply(&mut stream, json!([]), None);
            drop(stream);
            let (mut stream, request) = host.receive();
            assert_eq!(
                request.args,
                json!({"serverId":"server","request":{"method":"operation.start","params":{"operationId":id,"repoId":"repo","expectedSnapshot":"snapshot","action":expected}}})
            );
            Host::reply(&mut stream, success(id), None);
            assert!(
                matches!(host.listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
            );
        });
        assert_eq!(
            backend
                .apply_reviewed_action("server", "repo", "snapshot", action, id)
                .unwrap()
                .state,
            crate::git_operation::State::Succeeded
        );
        task.join().unwrap();
    }
    #[test]
    fn terminal_link_opener_rejects_unsafe_schemes_and_dispatches_exact_url_once() {
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        assert!(backend.open_terminal_link("file:///etc/passwd").is_err());
        assert!(
            matches!(host.listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
        let task = thread::spawn(move || {
            let (mut stream, request) = host.receive();
            assert_eq!(request.method, "open_url");
            assert_eq!(request.args, json!({"url":"https://example.org/服务?q=1"}));
            Host::reply(&mut stream, Value::Null, None);
            assert!(
                matches!(host.listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
            );
        });
        backend
            .open_terminal_link("HTTPS://example.org/服务?q=1")
            .unwrap();
        task.join().unwrap();
    }
    #[test]
    fn git_sync_transfers_dispatch_the_reviewed_snapshot_and_remote_once() {
        use crate::git_sync::{Kind, Target};
        let target = Target {
            snapshot: "reviewed-state".into(),
            remote: "origin".into(),
            destination: "release/next".into(),
            branch: "topic".into(),
            oid: "a".repeat(40),
            ahead: 1,
            behind: 0,
        };
        for kind in [Kind::Fetch, Kind::Pull, Kind::Push] {
            let host = Host::new();
            let backend = Backend::at_path(host.path.clone());
            let id = uuid::Uuid::new_v4();
            let action = target
                .action(kind, &json!({"name":"origin","token":"reviewed-config"}))
                .unwrap();
            let expected = action.clone();
            let task = thread::spawn(move || {
                let (mut stream, request) = host.receive();
                assert_eq!(request.method, "git_pending_operations");
                Host::reply(&mut stream, json!([]), None);
                drop(stream);
                let (mut stream, request) = host.receive();
                assert_eq!(
                    request.args,
                    json!({"serverId":"server","request":{"method":"operation.start","params":{"operationId":id,"repoId":"repo","expectedSnapshot":"reviewed-state","action":expected}}})
                );
                Host::reply(&mut stream, success(id), None);
                assert!(
                    matches!(host.listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
                );
            });
            assert_eq!(
                backend
                    .apply_reviewed_action("server", "repo", &target.snapshot, action, id)
                    .unwrap()
                    .state,
                crate::git_operation::State::Succeeded
            );
            task.join().unwrap();
        }
    }
    #[test]
    fn remote_picker_reads_only_one_names_page_and_selected_configuration() {
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let worker = thread::spawn(move || {
            let (mut stream, request) = host.receive();
            assert_eq!(
                request.args,
                json!({"serverId":"server","request":{"method":"repo.remote_names","params":{"repoId":"repo","filter":"backup","pageSize":20,"cursor":"next"}}})
            );
            Host::reply(
                &mut stream,
                json!({"entries":[{"name":"backup-21"}],"snapshot":"s","nextCursor":null,"metadata":{"totalEntries":21}}),
                None,
            );
            drop(stream);
            let (mut stream, request) = host.receive();
            assert_eq!(
                request.args,
                json!({"serverId":"server","request":{"method":"repo.remote","params":{"repoId":"repo","name":"backup-21"}}})
            );
            Host::reply(
                &mut stream,
                json!({"name":"backup-21","token":"reviewed","url":"/fetch","pushUrl":"/push"}),
                None,
            );
            assert!(
                matches!(host.listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
            );
        });
        let names = backend
            .git_remote_names("server", "repo", "backup", Some("next"))
            .unwrap();
        assert_eq!(names.entries, ["backup-21"]);
        assert_eq!(
            backend.git_remote("server", "repo", "backup-21").unwrap()["pushUrl"],
            "/push"
        );
        worker.join().unwrap();
    }
    #[test]
    fn worktree_creation_preserves_reviewed_identity_mode_and_lock() {
        for new_branch in [false, true] {
            let host = Host::new();
            let backend = Backend::at_path(host.path.clone());
            let id = uuid::Uuid::new_v4();
            let task = thread::spawn(move || {
                let (mut stream, request) = host.receive();
                assert_eq!(request.method, "git_pending_operations");
                assert_eq!(request.args, json!({"serverId":"server"}));
                Host::reply(&mut stream, json!([]), None);
                drop(stream);
                let (mut stream, request) = host.receive();
                assert_eq!(request.method, "git_request");
                assert_eq!(
                    request.args,
                    json!({"serverId":"server","request":{"method":"operation.start","params":{
                        "operationId":id,"repoId":"repo","expectedSnapshot":"reviewed-worktrees",
                        "action":{"kind":"worktree.add","name":"named-checkout","path":{"display":"/srv/separate directory","bytesB64":"L3Nydi9zZXBhcmF0ZSBkaXJlY3Rvcnk="},"branch":"topic","expectedOid":"a".repeat(40),"newBranch":new_branch,"locked":true}
                    }}})
                );
                Host::reply(&mut stream, success(id), None);
                assert!(
                    matches!(host.listener.accept(), Err(error) if error.kind()==std::io::ErrorKind::WouldBlock)
                );
            });
            let result = backend
                .create_worktree(
                    "server",
                    "repo",
                    "reviewed-worktrees",
                    crate::git_worktree::CreateTarget {
                        name: "named-checkout",
                        path: "/srv/separate directory",
                        branch: "topic",
                        oid: &"a".repeat(40),
                        new_branch,
                        locked: true,
                    },
                    id,
                )
                .unwrap();
            assert_eq!(result.operation_id, id.to_string());
            task.join().unwrap();
        }
    }
    #[test]
    fn worktree_branch_reads_are_bounded_filtered_and_ownership_is_exact() {
        use base64::Engine as _;
        let reference = "refs/heads/topic";
        for owner in [None, Some(reference), Some("refs/heads/wrong")] {
            let host = Host::new();
            let backend = Backend::at_path(host.path.clone());
            let task = thread::spawn(move || {
                let (mut stream, request) = host.receive();
                assert_eq!(
                    request.args,
                    json!({"serverId":"server","request":{"method":"repo.branches","params":{"repoId":"repo","branchKind":"local","pageSize":20,"filter":"topic","cursor":"next-page"}}})
                );
                Host::reply(
                    &mut stream,
                    json!({"entries":[],"snapshot":"branches","metadata":{},"nextCursor":null}),
                    None,
                );
                drop(stream);
                let (mut stream, request) = host.receive();
                assert_eq!(
                    request.args,
                    json!({"serverId":"server","request":{"method":"repo.worktrees","params":{"repoId":"repo","branch":reference,"pageSize":1}}})
                );
                let entries:Vec<_>=owner.into_iter().map(|branch|json!({"head":{"name":{"bytesB64":base64::engine::general_purpose::STANDARD.encode(branch)}},"path":{"display":"/srv/occupied"}})).collect();
                Host::reply(
                    &mut stream,
                    json!({"snapshot":"worktrees","metadata":{},"entries":entries,"nextCursor":null}),
                    None,
                );
                assert!(
                    matches!(host.listener.accept(),Err(error) if error.kind()==std::io::ErrorKind::WouldBlock)
                );
            });
            backend
                .worktree_branches("server", "repo", "topic", Some("next-page"))
                .unwrap();
            let result = backend.worktree_branch_owner(
                "server",
                "repo",
                &crate::git_branch::WorktreeBranch {
                    name: "topic".into(),
                    reference: reference.into(),
                    oid: "a".repeat(40),
                },
            );
            match owner {
                None => assert_eq!(result.unwrap(), None),
                Some(value) if value == reference => {
                    assert_eq!(result.unwrap().as_deref(), Some("/srv/occupied"))
                }
                _ => assert!(result.is_err()),
            }
            task.join().unwrap();
        }
    }
    #[test]
    fn worktree_starting_points_request_both_branch_kinds_in_bounded_pages() {
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let task = thread::spawn(move || {
            let (mut stream, request) = host.receive();
            assert_eq!(
                request.args,
                json!({"serverId":"server","request":{"method":"repo.branches","params":{"repoId":"repo","pageSize":20,"filter":"origin/topic","cursor":"next"}}})
            );
            Host::reply(
                &mut stream,
                json!({"entries":[],"snapshot":"branches","metadata":{},"nextCursor":null}),
                None,
            );
        });
        backend
            .worktree_branch_choices(
                "server",
                "repo",
                "origin/topic",
                Some("next"),
                crate::git_branch::WorktreeBranchUse::StartingPoint,
            )
            .unwrap();
        task.join().unwrap();
    }
    #[test]
    fn worktree_branch_precheck_filters_local_refs_with_bounded_pages() {
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let task = thread::spawn(move || {
            let (mut stream, request) = host.receive();
            assert_eq!(
                request.args,
                json!({"serverId":"server","request":{"method":"repo.branches","params":{"repoId":"repo","pageSize":20,"filter":"topic","branchKind":"local"}}})
            );
            Host::reply(
                &mut stream,
                json!({"snapshot":"s", "metadata":{}, "entries":[],"nextCursor":null}),
                None,
            );
        });
        backend
            .check_worktree_branch("server", "repo", "topic")
            .unwrap();
        task.join().unwrap();
    }
    #[test]
    fn worktree_name_precheck_is_an_exact_single_row_read() {
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let task = thread::spawn(move || {
            let (mut stream, request) = host.receive();
            assert_eq!(
                request.args,
                json!({"serverId":"server","request":{"method":"repo.worktrees","params":{"repoId":"repo","pageSize":1,"name":"topic"}}})
            );
            Host::reply(
                &mut stream,
                json!({"snapshot":"worktrees","metadata":{},"entries":[],"nextCursor":null}),
                None,
            );
        });
        backend
            .check_worktree_name("server", "repo", "topic")
            .unwrap();
        task.join().unwrap();
    }
    #[test]
    fn invalid_worktree_management_is_rejected_before_any_backend_call() {
        let backend = Backend::at_path(PathBuf::from("/nonexistent-worktree-validation.sock"));
        let id = uuid::Uuid::new_v4();
        let error = backend
            .repair_worktree("server", "repo", "snapshot", ("topic", "/srv/../other"), id)
            .unwrap_err();
        assert!(!error.dispatched);
        assert!(error.message.contains("absolute path"));
        let error = backend
            .remove_worktree("server", "repo", "snapshot", ("../other", true), id)
            .unwrap_err();
        assert!(!error.dispatched);
        assert!(error.message.contains("exact name"));
        let error = backend
            .lock_worktree(
                "server",
                "repo",
                "snapshot",
                ("topic", &"a".repeat(4097), false),
                id,
            )
            .unwrap_err();
        assert!(!error.dispatched);
        assert!(error.message.contains("4096 bytes"));
    }
}
