//! Opt-in replay acceptance through desktop IPC, SSH, and the installed agent.
//! Uses only UUID-named repositories in the disposable loopback Docker fixture.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use newport_client::{
    backend::Backend,
    git_conflict::{recovery_allowed, resolution},
    git_history::{MainlineReview, commit_action},
};
use newport_protocol::{
    desktop,
    git::{self as protocol, Operation, OperationState as State},
};
use serde_json::Value;
use std::{
    process::Command,
    time::{Duration, Instant},
};

const CONTAINER: &str = "newport-gpui-files";

fn docker(args: &[&str]) -> Result<String, String> {
    let output = Command::new("docker")
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "Fixture command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| e.to_string())
}
fn exec(args: &[&str]) -> Result<String, String> {
    let mut command = vec!["exec", "--user", "fixture", CONTAINER];
    command.extend_from_slice(args);
    docker(&command)
}
fn git(path: &str, args: &[&str]) -> Result<String, String> {
    let mut command = vec!["git", "-C", path];
    command.extend_from_slice(args);
    exec(&command).map(|s| s.trim_end().to_owned())
}
struct Fixture(String);
impl Fixture {
    fn new() -> Result<Self, String> {
        let path = format!(
            "/home/fixture/gpui-history-test-{}",
            uuid::Uuid::new_v4().simple()
        );
        exec(&["mkdir", "--", &path])?;
        Ok(Self(path))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = exec(&["rm", "-rf", "--", &self.0]) {
            eprintln!("Cleanup failed for {}: {error}", self.0);
        }
    }
}
/// Sends a Git read and returns its checked reply.
fn reply(
    backend: &Backend,
    server: &str,
    request: protocol::Request,
) -> Result<protocol::Reply, String> {
    backend
        .git(
            server
                .parse::<uuid::Uuid>()
                .map_err(|error| error.to_string())?,
            request,
            false,
        )
        .map_err(|error| error.to_string())
}
/// [`reply`] as JSON, for assertions on its fields.
fn request(backend: &Backend, server: &str, request: protocol::Request) -> Result<Value, String> {
    serde_json::to_value(reply(backend, server, request)?).map_err(|error| error.to_string())
}

fn acknowledge(backend: &Backend, server: &str, operation_id: uuid::Uuid) -> Result<(), String> {
    backend
        .execute(desktop::Operation::Git(Box::new(
            desktop::git::Operation::AcknowledgeOperation(Box::new(
                desktop::git::AcknowledgeOperationArgs {
                    server: server
                        .parse::<uuid::Uuid>()
                        .map_err(|error| error.to_string())?,
                    operation_id,
                },
            )),
        )))
        .map(drop)
        .map_err(|error| error.to_string())
}

fn settled(
    backend: &Backend,
    server: &str,
    mut operation: Operation,
    expected: State,
) -> Result<(), String> {
    let id = operation
        .operation_id
        .parse::<uuid::Uuid>()
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while operation.state == State::Running && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        operation = backend.git_operation(server, id)?;
    }
    assert_eq!(operation.state, expected, "{operation:?}");
    assert_eq!(backend.git_operation(server, id)?.state, expected);
    Ok(())
}

#[test]
#[ignore = "requires isolated desktop backend and disposable loopback Docker SSH fixture"]
fn replay_mainline_selection_changes_the_actual_patch() -> Result<(), String> {
    let profile =
        std::fs::canonicalize(std::env::var("NEWPORT_DATA_DIR").map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    assert!(
        profile.starts_with(std::fs::canonicalize("/tmp").map_err(|e| e.to_string())?),
        "Use an isolated profile under /tmp"
    );
    let port = std::env::var("NEWPORT_TEST_SSH_PORT")
        .map_err(|e| e.to_string())?
        .parse::<u16>()
        .map_err(|e| e.to_string())?;
    assert_eq!(
        docker(&["port", CONTAINER, "22/tcp"])?.trim(),
        format!("127.0.0.1:{port}")
    );
    assert_eq!(exec(&["id", "-un"])?.trim(), "fixture");
    let backend = Backend::discover();
    let state = backend
        .configuration_snapshot()
        .map_err(|error| error.to_string())?;
    let servers: Vec<_> = state
        .config
        .servers
        .iter()
        .filter(|s| s.ssh_host == "127.0.0.1" && s.ssh_port == port && s.ssh_user == "fixture")
        .collect();
    assert_eq!(
        servers.len(),
        1,
        "Expected one disposable loopback fixture server"
    );
    let server = servers[0].id.to_string();
    let server = server.as_str();
    let fixture = Fixture::new()?;
    let source = format!("{}/source", fixture.0);
    exec(&["mkdir", "--", &source])?;
    git(&source, &["init", "--initial-branch=main"])?;
    git(&source, &["config", "user.name", "Native Test"])?;
    git(&source, &["config", "user.email", "fixture@localhost"])?;
    git(&source, &["config", "commit.gpgSign", "false"])?;
    git(&source, &["commit", "--allow-empty", "-m", "Base"])?;
    let base = git(&source, &["rev-parse", "HEAD"])?;
    for (branch, file) in [("left", "left.txt"), ("right", "right.txt")] {
        git(&source, &["checkout", "-b", branch, &base])?;
        exec(&["touch", "--", &format!("{source}/{file}")])?;
        git(&source, &["add", "--", file])?;
        git(&source, &["commit", "-m", branch])?;
    }
    let right = git(&source, &["rev-parse", "HEAD"])?;
    git(&source, &["checkout", "left"])?;
    let left = git(&source, &["rev-parse", "HEAD"])?;
    git(
        &source,
        &["merge", "--no-ff", "right", "-m", "Two-parent merge"],
    )?;
    let merged = git(&source, &["rev-parse", "HEAD"])?;

    for kind in ["cherry_pick", "revert"] {
        for parent in 0..2 {
            let path = format!("{}/{}-{}", fixture.0, kind, parent + 1);
            let start = if kind == "cherry_pick" {
                &base
            } else {
                &merged
            };
            git(&source, &["worktree", "add", "--detach", &path, start])?;
            let opened = request(
                &backend,
                server,
                protocol::Request::Open {
                    path: protocol::Path::new(path.as_bytes()),
                },
            )?;
            assert_eq!(opened["root"]["bytesB64"], STANDARD.encode(&path));
            let repo = opened["repoId"].as_str().ok_or("Missing repository ID")?;
            let protocol::Reply::Commit(detail) = reply(
                &backend,
                server,
                protocol::Request::Commit {
                    repo_id: repo.into(),
                    commit_oid: merged.clone(),
                },
            )?
            else {
                return Err("Invalid commit detail".into());
            };
            let review =
                MainlineReview::from_commit(&detail, &merged).ok_or("Invalid merge detail")?;
            assert_eq!(review.parents(), &[left.clone(), right.clone()]);
            let mut action =
                commit_action(kind, &detail, &merged, 0).ok_or("Invalid replay action")?;
            assert!(review.select_parent(&mut action, parent));
            let before = request(
                &backend,
                server,
                protocol::Request::Status {
                    repo_id: repo.into(),
                    filter: None,
                    page_size: 10,
                    cursor: None,
                },
            )?;
            let id = uuid::Uuid::new_v4();
            let operation = backend
                .apply_reviewed_action(
                    server,
                    repo,
                    before["snapshot"].as_str().ok_or("Missing snapshot")?,
                    action,
                    id,
                )
                .map_err(|e| e.message)?;
            // Observe a persisted result as the UI does after reconnecting.
            settled(&backend, server, operation, State::Succeeded)?;
            let expected = match (kind, parent) {
                ("cherry_pick", 0) | ("revert", 1) => "right.txt",
                _ => "left.txt",
            };
            assert_eq!(
                git(&path, &["ls-tree", "--name-only", "HEAD"])?,
                expected,
                "{kind} must apply the patch relative to parent {}",
                parent + 1
            );
            assert_eq!(git(&path, &["rev-parse", "HEAD^"])?, *start);
            assert!(git(&path, &["status", "--porcelain=v1"])?.is_empty());
            let after = request(
                &backend,
                server,
                protocol::Request::Status {
                    repo_id: repo.into(),
                    filter: None,
                    page_size: 10,
                    cursor: None,
                },
            )?;
            assert_ne!(after["snapshot"], before["snapshot"]);
            assert!(
                after["entries"]
                    .as_array()
                    .ok_or("Missing status entries")?
                    .is_empty()
            );
            acknowledge(&backend, server, id)?;
            eprintln!(
                "Verified {kind} parent {}: {expected}, clean worktree, persisted success",
                parent + 1
            );
        }
    }
    // Reverting an added file after it was modified must surface a conflict,
    // and abort must restore the exact pre-operation commit and working files.
    let path = format!("{}/conflict", fixture.0);
    git(&source, &["worktree", "add", "--detach", &path, &merged])?;
    exec(&["cp", "/etc/hostname", &format!("{path}/right.txt")])?;
    git(&path, &["commit", "-am", "Modify the merged file"])?;
    let head = git(&path, &["rev-parse", "HEAD"])?;
    let contents = exec(&["cat", &format!("{path}/right.txt")])?;
    let opened = request(
        &backend,
        server,
        protocol::Request::Open {
            path: protocol::Path::new(path.as_bytes()),
        },
    )?;
    assert_eq!(opened["root"]["bytesB64"], STANDARD.encode(&path));
    let repo = opened["repoId"].as_str().ok_or("Missing repository ID")?;
    let protocol::Reply::Commit(detail) = reply(
        &backend,
        server,
        protocol::Request::Commit {
            repo_id: repo.into(),
            commit_oid: merged.clone(),
        },
    )?
    else {
        return Err("Invalid commit detail".into());
    };
    let action = commit_action("revert", &detail, &merged, 0).ok_or("Invalid replay action")?;
    let before = request(
        &backend,
        server,
        protocol::Request::Status {
            repo_id: repo.into(),
            filter: None,
            page_size: 10,
            cursor: None,
        },
    )?;
    let replay_id = uuid::Uuid::new_v4();
    let operation = backend
        .apply_reviewed_action(
            server,
            repo,
            before["snapshot"].as_str().ok_or("Missing snapshot")?,
            action,
            replay_id,
        )
        .map_err(|e| e.message)?;
    settled(&backend, server, operation, State::NeedsResolution)?;
    let conflict = request(
        &backend,
        server,
        protocol::Request::Status {
            repo_id: repo.into(),
            filter: None,
            page_size: 10,
            cursor: None,
        },
    )?;
    assert_eq!(conflict["metadata"]["integration"]["managed"], true);
    assert_eq!(conflict["metadata"]["integration"]["canAbort"], true);
    assert!(
        conflict["entries"]
            .as_array()
            .ok_or("Missing conflicts")?
            .iter()
            .any(|e| e["conflicted"] == true)
    );
    let abort_id = uuid::Uuid::new_v4();
    let operation = backend
        .apply_reviewed_action(
            server,
            repo,
            conflict["snapshot"]
                .as_str()
                .ok_or("Missing conflict snapshot")?,
            protocol::Action::IntegrationAbort {},
            abort_id,
        )
        .map_err(|e| e.message)?;
    settled(&backend, server, operation, State::Succeeded)?;
    assert_eq!(git(&path, &["rev-parse", "HEAD"])?, head);
    assert_eq!(exec(&["cat", &format!("{path}/right.txt")])?, contents);
    assert!(git(&path, &["status", "--porcelain=v1"])?.is_empty());
    let after = request(
        &backend,
        server,
        protocol::Request::Status {
            repo_id: repo.into(),
            filter: None,
            page_size: 10,
            cursor: None,
        },
    )?;
    assert_ne!(after["metadata"]["integration"]["managed"], true);
    assert!(
        after["entries"]
            .as_array()
            .ok_or("Missing status entries")?
            .is_empty()
    );
    for id in [replay_id, abort_id] {
        acknowledge(&backend, server, id)?;
    }
    eprintln!("Verified revert conflict and abort: original HEAD and file contents restored");
    // Exercise the other route the native inspector exposes: choose the
    // recorded deletion, stage it through the agent, then continue the revert.
    let action = commit_action("revert", &detail, &merged, 0).ok_or("Invalid replay action")?;
    let replay_id = uuid::Uuid::new_v4();
    let operation = backend
        .apply_reviewed_action(
            server,
            repo,
            after["snapshot"].as_str().ok_or("Missing snapshot")?,
            action,
            replay_id,
        )
        .map_err(|e| e.message)?;
    settled(&backend, server, operation, State::NeedsResolution)?;
    let protocol::Reply::Status(conflict) = reply(
        &backend,
        server,
        protocol::Request::Status {
            repo_id: repo.into(),
            filter: None,
            page_size: 10,
            cursor: None,
        },
    )?
    else {
        return Err("Invalid status".into());
    };
    assert!(!recovery_allowed(&conflict, "integration.continue"));
    let entry = conflict
        .entries
        .iter()
        .find(|entry| entry.conflicted)
        .ok_or("Missing conflict")?;
    let action = resolution(entry, "theirs").ok_or("Missing recorded deletion")?;
    assert!(matches!(
        &action,
        protocol::Action::ConflictResolve {
            expected_oid: None,
            ..
        }
    ));
    let resolve_id = uuid::Uuid::new_v4();
    let operation = backend
        .apply_reviewed_action(server, repo, &conflict.snapshot, action, resolve_id)
        .map_err(|e| e.message)?;
    settled(&backend, server, operation, State::Succeeded)?;
    let protocol::Reply::Status(resolved) = reply(
        &backend,
        server,
        protocol::Request::Status {
            repo_id: repo.into(),
            filter: None,
            page_size: 10,
            cursor: None,
        },
    )?
    else {
        return Err("Invalid status".into());
    };
    assert!(recovery_allowed(&resolved, "integration.continue"));
    let continue_id = uuid::Uuid::new_v4();
    let message = "Resolve recorded deletion through Newport";
    let operation = backend
        .apply_reviewed_action(
            server,
            repo,
            &resolved.snapshot,
            protocol::Action::IntegrationContinue {
                message: Some(message.into()),
                author: None,
            },
            continue_id,
        )
        .map_err(|e| e.message)?;
    settled(&backend, server, operation, State::Succeeded)?;
    assert_eq!(git(&path, &["ls-tree", "--name-only", "HEAD"])?, "left.txt");
    assert_eq!(git(&path, &["rev-parse", "HEAD^"])?, head);
    assert_eq!(git(&path, &["log", "-1", "--format=%B"])?, message);
    assert!(git(&path, &["status", "--porcelain=v1"])?.is_empty());
    let completed = request(
        &backend,
        server,
        protocol::Request::Status {
            repo_id: repo.into(),
            filter: None,
            page_size: 10,
            cursor: None,
        },
    )?;
    assert!(completed["metadata"]["integration"].is_null());
    for id in [replay_id, resolve_id, continue_id] {
        acknowledge(&backend, server, id)?;
    }
    eprintln!(
        "Verified recorded-side resolution and continue: deletion committed with the supplied message"
    );
    Ok(())
}
