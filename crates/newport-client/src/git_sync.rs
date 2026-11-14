//! Reviewed upstream transfers.
use base64::Engine as _;
use newport_protocol::git::{Action, Page, Path, Remote, StatusEntry, StatusSummary};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Fetch,
    Pull,
    Push,
}
impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Fetch => "Fetch",
            Self::Pull => "Pull",
            Self::Push => "Push",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub snapshot: String,
    pub remote: String,
    pub destination: String,
    pub branch: String,
    pub oid: String,
    pub ahead: u64,
    pub behind: u64,
}
/// The text of a reference whose bytes are exactly that UTF-8 text.
fn reference(path: &Path) -> Option<&str> {
    (path.bytes_b64 == base64::engine::general_purpose::STANDARD.encode(&path.display))
        .then_some(path.display.as_str())
}
impl Target {
    pub fn from_status(
        repo: Option<&newport_protocol::git::Repository>,
        status: &Page<StatusEntry, StatusSummary>,
    ) -> Result<Self, String> {
        if !repo.is_some_and(|repo| {
            !repo.capabilities.read_only && repo.capabilities.working_tree && !repo.bare
        }) {
            return Err("This repository cannot be written from here.".into());
        }
        let metadata = &status.metadata;
        let head = &metadata.head;
        if head.detached || head.unborn || metadata.integration.is_some() {
            return Err(
                "Finish the current integration and select an existing local branch.".into(),
            );
        }
        let branch = head
            .name
            .as_ref()
            .and_then(reference)
            .and_then(|name| name.strip_prefix("refs/heads/"))
            .filter(|name| !name.is_empty())
            .ok_or("A valid local branch is required.")?;
        let upstream = metadata.upstream.as_ref();
        let remote = upstream
            .and_then(|upstream| reference(&upstream.remote))
            .filter(|name| !name.is_empty() && *name != ".")
            .ok_or("This branch has no remote upstream. Configure one in the branch menu, or use Remotes.")?;
        let destination = upstream
            .and_then(|upstream| reference(&upstream.reference))
            .and_then(|name| name.strip_prefix("refs/heads/"))
            .filter(|name| !name.is_empty())
            .ok_or("The upstream destination is not a branch.")?;
        let oid = head
            .oid
            .as_ref()
            .map(|oid| oid.hex.as_str())
            .filter(|oid| {
                matches!(oid.len(), 40 | 64)
                    && oid.bytes().all(|c| c.is_ascii_hexdigit())
                    && oid.bytes().any(|c| c != b'0')
            })
            .ok_or("The current branch tip is unavailable.")?;
        if status.snapshot.is_empty() {
            return Err("Refresh repository status first.".into());
        }
        Ok(Self {
            snapshot: status.snapshot.clone(),
            remote: remote.into(),
            destination: destination.into(),
            branch: branch.into(),
            oid: oid.into(),
            ahead: metadata.ahead.unwrap_or(0),
            behind: metadata.behind.unwrap_or(0),
        })
    }
    pub fn preferred(&self) -> Kind {
        if self.behind > 0 {
            Kind::Pull
        } else if self.ahead > 0 {
            Kind::Push
        } else {
            Kind::Fetch
        }
    }
    pub fn action(&self, kind: Kind, remote: &Remote) -> Result<Action, String> {
        if remote.name != self.remote {
            return Err("Remote identity changed. Review the transfer again.".into());
        }
        let token = Some(remote.token.as_str())
            .filter(|s| !s.is_empty())
            .ok_or("Remote configuration is unavailable.")?;
        Ok(match kind {
            Kind::Fetch => Action::Fetch {
                remote: self.remote.clone(),
                expected_token: token.into(),
                prune: false,
            },
            Kind::Pull => Action::PullFastForward {
                remote: self.remote.clone(),
                expected_token: token.into(),
                remote_branch: self.destination.clone(),
            },
            Kind::Push => Action::Push {
                remote: self.remote.clone(),
                expected_token: token.into(),
                branch: self.branch.clone(),
                expected_oid: self.oid.clone(),
                destination_branch: self.destination.clone(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_protocol::git;
    fn reference(name: &str) -> Path {
        Path::new(name.as_bytes())
    }
    fn remote(name: &str, token: &str) -> Remote {
        Remote {
            name: name.into(),
            url: None,
            push_url: None,
            token: token.into(),
        }
    }
    fn fixture() -> (Option<git::Repository>, Page<StatusEntry, StatusSummary>) {
        (
            Some(git::Repository {
                repo_id: "repo".into(),
                common_repo_id: "repo".into(),
                root: git::Path::new(b"/repo"),
                bare: false,
                object_format: "sha1".into(),
                head: git::Head {
                    oid: None,
                    name: None,
                    detached: false,
                    unborn: false,
                },
                operation_state: "none".into(),
                integration: None,
                capabilities: git::RepositoryCapabilities {
                    read_only: false,
                    working_tree: true,
                },
            }),
            Page {
                snapshot: "snapshot".into(),
                entries: vec![],
                next_cursor: None,
                metadata: StatusSummary {
                    head: git::Head {
                        oid: Some(git::ObjectId {
                            algorithm: "sha1".into(),
                            hex: "a".repeat(40),
                        }),
                        name: Some(reference("refs/heads/topic")),
                        detached: false,
                        unborn: false,
                    },
                    operation_state: "none".into(),
                    integration: None,
                    ahead: Some(2),
                    behind: Some(3),
                    basis: "local".into(),
                    upstream_ref: Some(reference("refs/remotes/origin/release/next")),
                    upstream: Some(git::Upstream {
                        remote: reference("origin"),
                        reference: reference("refs/heads/release/next"),
                    }),
                    truncated: false,
                    total_entries: 0,
                    group_counts: git::GroupCounts {
                        staged: 0,
                        unstaged: 0,
                        untracked: 0,
                        conflicted: 0,
                    },
                    matched_entries: None,
                },
            },
        )
    }
    #[test]
    fn transfers_use_upstream_destination_and_exact_reviewed_identities() {
        let (repo, mut status) = fixture();
        let target = Target::from_status(repo.as_ref(), &status).unwrap();
        assert_eq!(target.preferred(), Kind::Pull);
        let origin = remote("origin", "config-token");
        let push = serde_json::to_value(target.action(Kind::Push, &origin).unwrap()).unwrap();
        assert_eq!(push["branch"], "topic");
        assert_eq!(push["destinationBranch"], "release/next");
        assert_eq!(push["expectedOid"], "a".repeat(40));
        assert_eq!(
            serde_json::to_value(target.action(Kind::Pull, &origin).unwrap()).unwrap()["remoteBranch"],
            "release/next"
        );
        assert_eq!(
            serde_json::to_value(target.action(Kind::Fetch, &origin).unwrap()).unwrap()["expectedToken"],
            "config-token"
        );
        assert!(
            target
                .action(Kind::Push, &remote("elsewhere", "token"))
                .is_err()
        );
        assert!(target.action(Kind::Push, &remote("origin", "")).is_err());
        status.metadata.behind = Some(0);
        assert_eq!(
            Target::from_status(repo.as_ref(), &status)
                .unwrap()
                .preferred(),
            Kind::Push
        );
        status.metadata.ahead = Some(0);
        assert_eq!(
            Target::from_status(repo.as_ref(), &status)
                .unwrap()
                .preferred(),
            Kind::Fetch
        );
    }
    #[test]
    fn custom_refspec_and_remote_name_use_git_resolved_destination() {
        let (repo, mut status) = fixture();
        status.metadata.upstream_ref = Some(reference("refs/remotes/cache/local-alias"));
        status.metadata.upstream.as_mut().unwrap().remote = reference("team/upstream");
        let target = Target::from_status(repo.as_ref(), &status).unwrap();
        assert_eq!(target.remote, "team/upstream");
        assert_eq!(target.destination, "release/next");
        for kind in [Kind::Fetch, Kind::Pull, Kind::Push] {
            let action = target
                .action(kind, &remote("team/upstream", "reviewed"))
                .unwrap();
            assert_eq!(
                serde_json::to_value(action).unwrap()["remote"],
                "team/upstream"
            );
        }
        // Without the resolved upstream the UI never guesses from a tracking path.
        status.metadata.upstream = None;
        assert!(Target::from_status(repo.as_ref(), &status).is_err());
    }
    #[test]
    fn read_only_unborn_detached_integration_and_lossy_refs_are_rejected() {
        let (mut repo, status) = fixture();
        repo.as_mut().unwrap().capabilities.read_only = true;
        assert!(Target::from_status(repo.as_ref(), &status).is_err());
        repo.as_mut().unwrap().capabilities.read_only = false;
        let lossy = |path: &mut Path| path.bytes_b64 = "/w==".into();
        type Status = Page<StatusEntry, StatusSummary>;
        let changes: [&dyn Fn(&mut Status); 8] = [
            &|status| status.metadata.head.detached = true,
            &|status| status.metadata.head.unborn = true,
            &|status| lossy(status.metadata.head.name.as_mut().unwrap()),
            &|status| lossy(&mut status.metadata.upstream.as_mut().unwrap().remote),
            &|status| lossy(&mut status.metadata.upstream.as_mut().unwrap().reference),
            &|status| status.metadata.upstream.as_mut().unwrap().remote = reference("."),
            &|status| status.snapshot.clear(),
            &|status| {
                status.metadata.integration = Some(git::IntegrationState {
                    kind: "merge".into(),
                    managed: true,
                    can_continue: false,
                    can_abort: true,
                    can_skip: false,
                })
            },
        ];
        for (index, change) in changes.iter().enumerate() {
            let mut invalid = status.clone();
            change(&mut invalid);
            assert!(
                Target::from_status(repo.as_ref(), &invalid).is_err(),
                "{index}"
            );
        }
    }
}
