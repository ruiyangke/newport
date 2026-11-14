//! One connection per call allows independent reads; no request is replayed.
//! Terminal opens retain their connection as an explicit lifetime lease.
pub mod exchange;
mod git;
use newport_protocol::{
    common::Present,
    desktop,
    git::{Action, Operation, Request as GitRequest},
};
use std::{path::PathBuf, time::Duration};

/// The backend is simply not running for these; any other error says what is
/// wrong with the endpoint.
fn unreachable_backend(path: &std::path::Path, error: &std::io::Error) -> String {
    let mut message = format!(
        "Cannot reach Newport's backend. Open Newport (with the same NEWPORT_DATA_DIR for a development build). Socket: {}",
        path.display()
    );
    if !matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
    ) {
        message.push_str(&format!(". Cause: {error}"));
    }
    message
}

#[derive(Clone)]
pub struct Backend {
    path: PathBuf,
}
#[derive(Clone, Debug)]
pub enum RepositoryCreation {
    Init {
        path: newport_protocol::git::Path,
        initial_branch: String,
    },
    Clone {
        url: String,
        path: newport_protocol::git::Path,
        branch: Option<String>,
        bare: bool,
    },
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
}
pub struct StashTarget<'a> {
    pub mode: StashMode,

    pub oid: &'a str,
    pub index: u64,
    pub token: &'a str,
    pub reinstate_index: bool,
}
/// The snapshot a configuration read replied with.
pub fn snapshot_reply(
    reply: desktop::Reply,
) -> Result<desktop::configuration::Snapshot, crate::request_error::RequestError> {
    match reply {
        desktop::Reply::Configuration(reply) => match *reply {
            desktop::configuration::Reply::Snapshot(snapshot) => Ok(*snapshot),
            _ => Err("The backend answered a configuration read with another reply.".into()),
        },
        _ => Err("The backend answered a configuration read with another reply.".into()),
    }
}
/// The batch an event poll replied with.
pub fn events_reply(
    reply: desktop::Reply,
) -> Result<desktop::events::Batch, crate::request_error::RequestError> {
    match reply {
        desktop::Reply::Events(reply) => match *reply {
            desktop::events::Reply::Poll(batch) => Ok(*batch),
        },
        _ => Err("The backend answered an event poll with another reply.".into()),
    }
}
/// The availability a Remote apps status read replied with.
pub fn availability_reply(
    reply: desktop::Reply,
) -> Result<desktop::applications::Availability, crate::request_error::RequestError> {
    match reply {
        desktop::Reply::Applications(reply) => match *reply {
            desktop::applications::Reply::Status(status) => Ok(*status),
            _ => Err("The backend answered a Remote apps status read with another reply.".into()),
        },
        _ => Err("The backend answered a Remote apps status read with another reply.".into()),
    }
}
/// The catalogue a Remote apps catalogue read replied with.
pub fn catalogue_reply(
    reply: desktop::Reply,
) -> Result<newport_protocol::remote_apps::apps::Catalogue, crate::request_error::RequestError> {
    match reply {
        desktop::Reply::Applications(reply) => match *reply {
            desktop::applications::Reply::Catalogue(catalogue) => Ok(*catalogue),
            _ => {
                Err("The backend answered a Remote apps catalogue read with another reply.".into())
            }
        },
        _ => Err("The backend answered a Remote apps catalogue read with another reply.".into()),
    }
}
impl Backend {
    /// One bounded request; uncertain responses are never replayed.
    pub fn activate_window(&self) -> Result<(), String> {
        self.execute_with_timeout(
            desktop::Operation::Host(Box::new(desktop::host::Operation::Activate(Box::new(
                Present {},
            )))),
            Duration::from_secs(8),
        )
        .map(drop)
        .map_err(|error| error.to_string())
    }
    /// Hands a remote app's ID, from a taskbar pin or shortcut, to the
    /// process that owns the profile. One bounded request, never replayed.
    pub fn open_remote_app(&self, id: &str, action: Option<&str>) -> Result<(), String> {
        self.execute_with_timeout(
            desktop::Operation::Host(Box::new(desktop::host::Operation::OpenRemoteApp(Box::new(
                desktop::host::OpenRemoteAppArgs {
                    id: id.into(),
                    action: action.map(str::to_owned),
                },
            )))),
            Duration::from_secs(8),
        )
        .map(drop)
        .map_err(|error| error.to_string())
    }
    /// Called only for an explicit user gesture; terminal escape parsing has
    /// no connection to the platform opener.
    pub fn open_terminal_link(&self, terminal: uuid::Uuid, url: &str) -> Result<(), String> {
        let url = newport_domain::external_link::http_url(url)
            .ok_or("Only http and https links can be opened.")?;
        self.execute_with_timeout(
            desktop::Operation::Configuration(Box::new(
                desktop::configuration::Operation::OpenTerminalLink(Box::new(
                    desktop::configuration::OpenTerminalLinkArgs { terminal, url },
                )),
            )),
            Duration::from_secs(60),
        )
        .map(drop)
        .map_err(|error| error.to_string())
    }
    /// The backend's configuration and runtime state, read typed.
    pub fn configuration_snapshot(
        &self,
    ) -> Result<desktop::configuration::Snapshot, crate::request_error::RequestError> {
        snapshot_reply(self.execute(desktop::Operation::Configuration(Box::new(
            desktop::configuration::Operation::Snapshot(Box::new(Present {})),
        )))?)
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
        action: Action,
        operation: uuid::Uuid,
    ) -> Result<Operation, crate::git_operation::WriteError> {
        self.start_git_action(server, repo, snapshot, action, operation)
    }
    pub fn remove_worktree(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        target: (&str, bool),
        operation: uuid::Uuid,
    ) -> Result<Operation, crate::git_operation::WriteError> {
        let (name, missing) = target;
        self.start_git_action(
            server,
            repo,
            snapshot,
            crate::git_worktree::remove_action(name, missing)?,
            operation,
        )
    }
    pub fn repair_worktree(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        target: (&str, &str),
        operation: uuid::Uuid,
    ) -> Result<Operation, crate::git_operation::WriteError> {
        let (name, path) = target;
        self.start_git_action(
            server,
            repo,
            snapshot,
            crate::git_worktree::repair_action(name, path)?,
            operation,
        )
    }
    pub fn create_worktree(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        target: crate::git_worktree::CreateTarget<'_>,
        operation: uuid::Uuid,
    ) -> Result<Operation, crate::git_operation::WriteError> {
        self.start_git_action(server, repo, snapshot, target.action()?, operation)
    }
    pub fn lock_worktree(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        target: (&str, &str, bool),
        operation: uuid::Uuid,
    ) -> Result<Operation, crate::git_operation::WriteError> {
        let (name, reason, unlock) = target;
        self.start_git_action(
            server,
            repo,
            snapshot,
            crate::git_worktree::lock_action(name, reason, unlock)?,
            operation,
        )
    }
    pub fn delete_tag(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        target: (&str, &str),
        operation: uuid::Uuid,
    ) -> Result<Operation, crate::git_operation::WriteError> {
        let (name, oid) = target;
        self.start_git_action(
            server,
            repo,
            snapshot,
            Action::TagDelete {
                name: name.into(),
                expected_oid: oid.into(),
            },
            operation,
        )
    }
    pub fn create_tag(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        target: (&str, &str, &str),
        operation: uuid::Uuid,
    ) -> Result<Operation, crate::git_operation::WriteError> {
        let (name, oid, message) = target;
        let annotation = (!message.is_empty()).then(|| newport_protocol::git::TagAnnotation {
            message: message.into(),
            author: None,
        });
        self.start_git_action(
            server,
            repo,
            snapshot,
            Action::TagCreate {
                name: name.into(),
                target_oid: oid.into(),
                annotation,
            },
            operation,
        )
    }
    pub fn save_stash(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        options: (String, bool, bool),
        operation: uuid::Uuid,
    ) -> Result<Operation, crate::git_operation::WriteError> {
        let (message, include_untracked, keep_index) = options;
        self.start_git_action(
            server,
            repo,
            snapshot,
            Action::StashSave {
                message,
                include_untracked,
                keep_index,
                author: None,
            },
            operation,
        )
    }
    pub fn modify_stash(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        target: StashTarget<'_>,
        operation: uuid::Uuid,
    ) -> Result<Operation, crate::git_operation::WriteError> {
        let index = Some(usize::try_from(target.index).map_err(|_| "Invalid stash index")?);
        let oid = target.oid.into();
        let expected_token = target.token.into();
        let action = match target.mode {
            StashMode::Apply => Action::StashApply {
                oid,
                index,
                expected_token,
                reinstate_index: target.reinstate_index,
            },
            StashMode::Pop => Action::StashPop {
                oid,
                index,
                expected_token,
                reinstate_index: target.reinstate_index,
            },
            StashMode::Drop => Action::StashDrop {
                oid,
                index,
                expected_token,
            },
        };
        self.start_git_action(server, repo, snapshot, action, operation)
    }
    pub fn push(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        target: PushTarget<'_>,
        operation: uuid::Uuid,
    ) -> Result<Operation, crate::git_operation::WriteError> {
        self.start_git_action(
            server,
            repo,
            snapshot,
            Action::Push {
                remote: target.remote.into(),
                expected_token: target.token.into(),
                branch: target.branch.into(),
                expected_oid: target.oid.into(),
                destination_branch: target.destination.into(),
            },
            operation,
        )
    }
    pub fn pull_fast_forward(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        target: (&str, &str, &str),
        operation: uuid::Uuid,
    ) -> Result<Operation, crate::git_operation::WriteError> {
        let (remote, token, branch) = target;
        self.start_git_action(
            server,
            repo,
            snapshot,
            Action::PullFastForward {
                remote: remote.into(),
                expected_token: token.into(),
                remote_branch: branch.into(),
            },
            operation,
        )
    }
    pub fn rename_branch(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        target: (&str, &str, &str),
        operation: uuid::Uuid,
    ) -> Result<Operation, crate::git_operation::WriteError> {
        let (name, new_name, expected_oid) = target;
        self.start_git_action(
            server,
            repo,
            snapshot,
            Action::BranchRename {
                name: name.into(),
                new_name: new_name.into(),
                expected_oid: expected_oid.into(),
            },
            operation,
        )
    }
    pub fn create_branch(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        target: (&str, &str),
        operation: uuid::Uuid,
    ) -> Result<Operation, crate::git_operation::WriteError> {
        let (name, start_oid) = target;
        self.start_git_action(
            server,
            repo,
            snapshot,
            Action::BranchCreate {
                name: name.into(),
                start_oid: start_oid.into(),
            },
            operation,
        )
    }
    pub fn checkout(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        target: (&str, &str),
        operation: uuid::Uuid,
    ) -> Result<Operation, crate::git_operation::WriteError> {
        let (branch, expected_oid) = target;
        self.start_git_action(
            server,
            repo,
            snapshot,
            Action::Checkout {
                target: newport_protocol::git::CheckoutTarget::Branch {
                    name: branch.into(),
                    expected_oid: expected_oid.into(),
                },
            },
            operation,
        )
    }
    pub fn commit(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        message: &str,
        operation: uuid::Uuid,
    ) -> Result<Operation, crate::git_operation::WriteError> {
        self.start_git_action(
            server,
            repo,
            snapshot,
            Action::Commit {
                message: message.into(),
                author: None,
            },
            operation,
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
    ) -> Result<Operation, crate::git_operation::WriteError> {
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
    ) -> Result<Operation, crate::git_operation::WriteError> {
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
    ) -> Result<Operation, crate::git_operation::WriteError> {
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
    ) -> Result<Operation, crate::git_operation::WriteError> {
        if repo.is_empty()
            || snapshot.is_empty()
            || entries.is_empty()
            || entries.iter().any(String::is_empty)
        {
            return Err("Select a file from a current repository status first".into());
        }
        // Agent paths() accepts at most `MAX_INDEX_ENTRIES` identities. Bound the request
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
            if stage {
                Action::Stage {
                    entry_ids: entries.to_vec(),
                    hunks: None,
                }
            } else {
                Action::Unstage {
                    entry_ids: entries.to_vec(),
                    hunks: None,
                }
            },
            operation,
        )
    }
    fn dispatch_index_action(
        &self,
        server: &str,
        repo: &str,
        snapshot: &str,
        action: Action,
        operation: uuid::Uuid,
    ) -> Result<Operation, crate::git_operation::WriteError> {
        self.start_git_action(server, repo, snapshot, action, operation)
    }
    /// Dispatch exactly once. The caller retains `operation` for explicit recovery
    /// even if any subsequent response is lost or malformed.
    pub fn fetch(
        &self,
        server: &str,
        repo: &str,
        remote: &str,
        operation: uuid::Uuid,
    ) -> Result<Operation, crate::git_operation::WriteError> {
        let server_id = uuid::Uuid::parse_str(server).map_err(|error| error.to_string())?;
        let detail = self
            .git(
                server_id,
                GitRequest::Remote {
                    repo_id: repo.into(),
                    name: remote.into(),
                },
                false,
            )
            .map_err(|error| error.message)?;
        let newport_protocol::git::Reply::Remote(detail) = detail else {
            return Err("Remote identity changed; refresh before fetching".into());
        };
        if detail.name != remote {
            return Err("Remote identity changed; refresh before fetching".into());
        }
        let token = Some(detail.token.as_str())
            .filter(|value| !value.is_empty())
            .ok_or("Remote configuration token is missing")?;
        let status = self
            .git(
                server_id,
                GitRequest::Status {
                    repo_id: repo.into(),
                    filter: None,
                    page_size: 1,
                    cursor: None,
                },
                false,
            )
            .map_err(|error| error.message)?;
        let newport_protocol::git::Reply::Status(status) = status else {
            return Err("Repository snapshot is missing".into());
        };
        let snapshot = Some(status.snapshot.as_str())
            .filter(|value| !value.is_empty())
            .ok_or("Repository snapshot is missing")?;
        self.start_git_action(
            server,
            repo,
            snapshot,
            Action::Fetch {
                remote: remote.into(),
                expected_token: token.into(),
                prune: false,
            },
            operation,
        )
    }
    pub fn git_operation(&self, server: &str, operation: uuid::Uuid) -> Result<Operation, String> {
        let server = uuid::Uuid::parse_str(server).map_err(|error| error.to_string())?;
        let reply = self
            .git(
                server,
                GitRequest::Get {
                    operation_id: operation.to_string(),
                },
                false,
            )
            .map_err(|error| error.message)?;
        crate::git_operation::confirmed(reply, &operation.to_string())
    }
    pub fn bootstrap(
        &self,
        server: &str,
        creation: RepositoryCreation,
        operation: uuid::Uuid,
    ) -> Result<Operation, crate::git_operation::WriteError> {
        let request = match creation {
            RepositoryCreation::Init {
                path,
                initial_branch,
            } => GitRequest::Init {
                operation_id: operation.to_string(),
                path,
                initial_branch,
            },
            RepositoryCreation::Clone {
                url,
                path,
                branch,
                bare,
            } => GitRequest::Clone {
                operation_id: operation.to_string(),
                url,
                path,
                branch,
                bare,
            },
        };
        self.dispatch_git_write(server, request, operation)
    }
    pub fn add_project(
        &self,
        server: uuid::Uuid,
        name: &str,
        path: newport_protocol::git::Path,
    ) -> Result<desktop::preferences::Project, String> {
        use desktop::{git, preferences};
        let name = name.trim();
        if name.is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
            return Err(
                "Enter a project name of up to 256 bytes without control characters.".into(),
            );
        }
        let reply = self
            .execute(desktop::Operation::Git(Box::new(git::Operation::Request(
                Box::new(git::RequestArgs {
                    server,
                    request: GitRequest::Open { path },
                    retry_connection: false,
                }),
            ))))
            .map_err(|error| error.to_string())?;
        let desktop::Reply::Git(reply) = reply else {
            return Err("Invalid repository reply".into());
        };
        let git::Reply::Request(reply) = *reply else {
            return Err("Invalid repository reply".into());
        };
        let git::GitResult::Reply(reply) = *reply else {
            return Err("Invalid repository reply".into());
        };
        let newport_protocol::git::Reply::Open(repo) = *reply else {
            return Err("Invalid repository reply".into());
        };
        let save = (|| {
            let reply = self
                .execute(desktop::Operation::Preferences(Box::new(
                    preferences::Operation::ProjectsList(Box::new(preferences::ProjectsListArgs {
                        server,
                    })),
                )))
                .map_err(|error| error.to_string())?;
            let desktop::Reply::Preferences(reply) = reply else {
                return Err("Invalid project listing".into());
            };
            let preferences::Reply::ProjectsList(projects) = *reply else {
                return Err("Invalid project listing".into());
            };
            if let Some(project) = projects
                .projects
                .into_iter()
                .find(|project| project.path.bytes_b64 == repo.root.bytes_b64)
            {
                return Ok(project);
            }
            let reply = self
                .execute(desktop::Operation::Preferences(Box::new(
                    preferences::Operation::ProjectsSave(Box::new(preferences::ProjectsSaveArgs {
                        project: preferences::Project {
                            id: uuid::Uuid::new_v4(),
                            server,
                            name: name.into(),
                            path: repo.root.clone(),
                        },
                    })),
                )))
                .map_err(|error| error.to_string())?;
            match reply {
                desktop::Reply::Preferences(reply) => match *reply {
                    preferences::Reply::ProjectsSave(project) => Ok(*project),
                    _ => Err("Invalid project save reply".into()),
                },
                _ => Err("Invalid project save reply".into()),
            }
        })();
        let _ = self.git(
            server,
            GitRequest::Close {
                repo_id: repo.repo_id.clone(),
            },
            false,
        );
        save
    }
    pub fn release_repository(&self, server: &str, repo_id: &str) {
        if let Ok(server) = uuid::Uuid::parse_str(server) {
            let _ = self.git(
                server,
                GitRequest::Close {
                    repo_id: repo_id.into(),
                },
                false,
            );
        }
    }
    pub fn discover() -> Self {
        // The profile this process opened, or a non-empty NEWPORT_DATA_DIR,
        // the same rule the window's own preferences follow.
        let base = crate::profile::directory().unwrap_or_else(|| {
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
    /// Admit the language server stream and retain its unread frame state.
    pub fn open_lsp(&self, open: crate::lsp::Open) -> Result<exchange::Opened, String> {
        let server = crate::desktop_inputs::identity(open.server_id, "server identity")
            .map_err(|error| error.to_string())?;
        let expected = crate::desktop_inputs::captured(open.expected_connection)
            .map_err(|error| error.to_string())?;
        self.open(
            desktop::stream::Operation::LspOpen(Box::new(desktop::stream::LspOpenArgs {
                server,
                expected,
                root: open.root.into(),
                server_name: open.server.into(),
                envrc: open.envrc,
            })),
            Duration::from_secs(45),
        )
        .map_err(|error| error.to_string())
    }
}

fn default_profile(root: &std::path::Path) -> PathBuf {
    // Match the host's profile directory. A named pipe has no filesystem
    // socket entry; endpoint existence cannot select the profile.
    root.join("Newport")
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod unreachable_tests {
    use super::unreachable_backend;
    use std::{io, path::Path};

    #[test]
    fn only_unexpected_errors_name_their_cause() {
        let path = Path::new("/profile/backend.sock");
        for kind in [io::ErrorKind::NotFound, io::ErrorKind::ConnectionRefused] {
            let message = unreachable_backend(path, &io::Error::from(kind));
            assert!(message.starts_with("Cannot reach Newport's backend."));
            assert!(!message.contains("Cause"));
        }
        let message = unreachable_backend(path, &io::Error::from(io::ErrorKind::PermissionDenied));
        assert!(message.contains("/profile/backend.sock"));
        assert!(message.contains("Cause: permission denied"));
    }
}
