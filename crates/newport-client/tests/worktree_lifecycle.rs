//! Opt-in end-to-end test through the running desktop backend and its SSH agent.
//! Requires the disposable newport-gpui-files Docker fixture and isolated profile.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use newport_client::{
    backend::Backend,
    git_operation::{Operation, State},
    git_worktree::CreateTarget,
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

const CONTAINER: &str = "newport-gpui-files";
struct Fixture {
    root: String,
}
impl Fixture {
    fn docker(args: &[&str]) -> Result<String, String> {
        let output = Command::new("docker")
            .args(["exec", "--user", "fixture", CONTAINER])
            .args(args)
            .output()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(format!(
                "Fixture command {args:?} failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
    fn git(&self, args: &[&str]) -> Result<String, String> {
        let mut command = vec!["git", "-C", self.root.as_str()];
        command.extend_from_slice(args);
        Self::docker(&command)
    }
    fn new() -> Result<Self, String> {
        let root = format!(
            "/home/fixture/gpui-worktree-test-{}",
            uuid::Uuid::new_v4().simple()
        );
        // mkdir must succeed before this guard owns cleanup of the unique path.
        Self::docker(&["mkdir", "--", &root])?;
        let fixture = Self { root };
        fixture.git(&["init", "--initial-branch=main"])?;
        fixture.git(&[
            "-c",
            "user.name=Native Test",
            "-c",
            "user.email=fixture@localhost",
            "commit",
            "--allow-empty",
            "-m",
            "Worktree lifecycle fixture",
        ])?;
        Ok(fixture)
    }
    fn checkout(&self, name: &str) -> String {
        format!("{}/{}", self.root, name)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Only our freshly created, UUID-named disposable fixture is removed.
        if let Err(error) = Self::docker(&["rm", "-rf", "--", &self.root]) {
            eprintln!("Fixture cleanup failed for {}: {error}", self.root);
        }
    }
}
fn require(ok: bool, message: &str) -> Result<(), String> {
    ok.then_some(()).ok_or_else(|| message.into())
}
fn rpc(backend: &Backend, server: &str, method: &str, params: Value) -> Result<Value, String> {
    backend.call(
        "git_request",
        json!({"serverId":server,"request":{"method":method,"params":params}}),
    )
}
fn page(backend: &Backend, server: &str, repo: &str) -> Result<Value, String> {
    rpc(
        backend,
        server,
        "repo.worktrees",
        json!({"repoId":repo,"pageSize":20}),
    )
}
fn finished(
    backend: &Backend,
    server: &str,
    mut operation: Operation,
    expected: State,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    while operation.state == State::Running && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        operation = backend.git_operation(
            server,
            operation
                .operation_id
                .parse()
                .map_err(|_| "Invalid operation id")?,
        )?;
    }
    require(
        operation.state == expected,
        &format!("Expected {expected:?}, got {operation:?}"),
    )
}

#[test]
#[ignore = "requires isolated desktop backend and disposable loopback Docker SSH fixture"]
fn remote_worktree_removal_and_recovery_preserve_files_branches_and_other_registrations()
-> Result<(), String> {
    let profile = PathBuf::from(std::env::var("NEWPORT_DATA_DIR").map_err(|e| e.to_string())?);
    require(
        std::fs::canonicalize(&profile)
            .map_err(|e| e.to_string())?
            .starts_with(std::fs::canonicalize("/tmp").map_err(|e| e.to_string())?),
        "Use an isolated profile under /tmp",
    )?;
    let port: u16 = std::env::var("NEWPORT_TEST_SSH_PORT")
        .map_err(|e| e.to_string())?
        .parse::<u16>()
        .map_err(|e| e.to_string())?;
    let mapped = Fixture::docker(&["id", "-un"])?;
    require(
        mapped.trim() == "fixture",
        "Expected disposable fixture user",
    )?;
    let backend = Backend::discover();
    let snapshot = backend.call("snapshot", json!({}))?;
    let servers: Vec<_> = snapshot["config"]["servers"]
        .as_array()
        .ok_or("Missing servers")?
        .iter()
        .filter(|s| {
            s["sshHost"] == "127.0.0.1" && s["sshPort"] == port && s["sshUser"] == "fixture"
        })
        .collect();
    require(servers.len() == 1, "Expected one loopback fixture server")?;
    let server = servers[0]["id"].as_str().ok_or("Missing server ID")?;
    let fixture = Fixture::new()?;
    let opened = rpc(
        &backend,
        server,
        "repo.open",
        json!({"path":{"display":fixture.root,"bytesB64":STANDARD.encode(&fixture.root)}}),
    )?;
    let repo = opened["repoId"].as_str().ok_or("Missing repo ID")?;
    require(
        opened["root"]["bytesB64"] == STANDARD.encode(&fixture.root),
        "Opened repository must be the freshly created disposable fixture",
    )?;
    let oid = fixture.git(&["rev-parse", "HEAD"])?.trim().to_owned();
    // A remote starting point can share its short name with a local branch.
    // Keep different commits so choosing by display name cannot pass this test.
    fixture.git(&["update-ref", "refs/remotes/origin/base", &oid])?;
    fixture.git(&[
        "-c",
        "user.name=Native Test",
        "-c",
        "user.email=fixture@localhost",
        "commit",
        "--allow-empty",
        "-m",
        "Newer local head",
    ])?;
    fixture.git(&["branch", "origin/base"])?;
    let choices = backend.worktree_branch_choices(
        server,
        repo,
        "origin/base",
        None,
        newport_client::git_branch::WorktreeBranchUse::StartingPoint,
    )?;
    let candidates: Vec<_> = choices["entries"]
        .as_array()
        .ok_or("Missing branch choices")?
        .iter()
        .filter_map(|entry| {
            newport_client::git_branch::WorktreeBranchUse::StartingPoint.candidate(entry, false)
        })
        .collect();
    require(
        candidates.len() == 2,
        "Expected distinct local and remote starting points",
    )?;
    let base = candidates
        .iter()
        .find(|branch| branch.reference == "refs/remotes/origin/base")
        .ok_or("Missing remote starting point")?;
    require(
        base.oid == oid,
        "Remote starting point must retain its reviewed commit",
    )?;
    let destination = fixture.checkout("remote-start");
    backend.check_worktree_name(server, repo, "remote-start")?;
    backend.check_worktree_branch(server, repo, "new-from-remote")?;
    let listing = page(&backend, server, repo)?;
    let operation = backend
        .create_worktree(
            server,
            repo,
            listing["snapshot"]
                .as_str()
                .ok_or("Missing worktree snapshot")?,
            CreateTarget {
                name: "remote-start",
                path: &destination,
                branch: "new-from-remote",
                oid: &base.oid,
                new_branch: true,
                locked: false,
            },
            uuid::Uuid::new_v4(),
        )
        .map_err(|error| error.message)?;
    finished(&backend, server, operation, State::Succeeded)?;
    require(
        backend
            .check_worktree_branch(server, repo, "new-from-remote")
            .is_err(),
        "Existing local branch must be rejected before worktree submission",
    )?;
    require(
        backend
            .check_worktree_name(server, repo, "remote-start")
            .is_err(),
        "Existing registration must be rejected by the exact-name precheck",
    )?;
    require(
        Fixture::docker(&["git", "-C", &destination, "rev-parse", "HEAD"])?.trim() == oid,
        "Created worktree must use the remote commit, not the newer local branch",
    )?;
    for name in ["dirty", "moved", "missing", "other-missing"] {
        let listing = page(&backend, server, repo)?;
        let path = fixture.checkout(name);
        let result = backend
            .create_worktree(
                server,
                repo,
                listing["snapshot"].as_str().ok_or("Missing snapshot")?,
                CreateTarget {
                    name,
                    path: &path,
                    branch: name,
                    oid: &oid,
                    new_branch: true,
                    locked: false,
                },
                uuid::Uuid::new_v4(),
            )
            .map_err(|e| e.message)?;
        finished(&backend, server, result, State::Succeeded)?;
    }
    let snapshot = || -> Result<String, String> {
        Ok(page(&backend, server, repo)?["snapshot"]
            .as_str()
            .ok_or("Missing snapshot")?
            .to_owned())
    };
    let remove = |name: &str, missing: bool, expected: State| -> Result<(), String> {
        let result = backend
            .remove_worktree(
                server,
                repo,
                &snapshot()?,
                (name, missing),
                uuid::Uuid::new_v4(),
            )
            .map_err(|e| e.message)?;
        finished(&backend, server, result, expected)
    };
    let dirty = fixture.checkout("dirty");
    let marker = format!("{dirty}/untracked.txt");
    Fixture::docker(&["touch", "--", &marker])?;
    remove("dirty", false, State::Failed)?;
    Fixture::docker(&["test", "-f", &marker])?;
    Fixture::docker(&["rm", "--", &marker])?;
    let locked = backend
        .lock_worktree(
            server,
            repo,
            &snapshot()?,
            ("dirty", "Preserve fixture", false),
            uuid::Uuid::new_v4(),
        )
        .map_err(|e| e.message)?;
    finished(&backend, server, locked, State::Succeeded)?;
    remove("dirty", false, State::Failed)?;
    Fixture::docker(&["test", "-d", &dirty])?;
    let unlocked = backend
        .lock_worktree(
            server,
            repo,
            &snapshot()?,
            ("dirty", "", true),
            uuid::Uuid::new_v4(),
        )
        .map_err(|e| e.message)?;
    finished(&backend, server, unlocked, State::Succeeded)?;
    remove("dirty", false, State::Succeeded)?;
    Fixture::docker(&["test", "!", "-e", &dirty])?;
    require(
        fixture.git(&["rev-parse", "refs/heads/dirty"])?.trim() == oid,
        "Removal must preserve branch",
    )?;

    let old = fixture.checkout("moved");
    let moved = fixture.checkout("new-location");
    Fixture::docker(&["mv", "--", &old, &moved])?;
    let repair = backend
        .repair_worktree(
            server,
            repo,
            &snapshot()?,
            ("moved", &moved),
            uuid::Uuid::new_v4(),
        )
        .map_err(|e| e.message)?;
    finished(&backend, server, repair, State::Succeeded)?;
    let rows = page(&backend, server, repo)?;
    require(
        rows["entries"].as_array().unwrap().iter().any(|row| {
            row["name"]["display"] == "moved"
                && row["path"]["display"] == moved
                && row["state"] == "available"
        }),
        "Repair must update only the moved registration",
    )?;

    // Preserve checkout files elsewhere while exercising registration-only removal.
    for name in ["missing", "other-missing"] {
        Fixture::docker(&[
            "mv",
            "--",
            &fixture.checkout(name),
            &fixture.checkout(&format!("saved-{name}")),
        ])?;
    }
    remove("missing", true, State::Succeeded)?;
    let rows = page(&backend, server, repo)?;
    let entries = rows["entries"]
        .as_array()
        .ok_or("Missing worktree entries")?;
    require(
        !entries
            .iter()
            .any(|row| row["name"]["display"] == "missing"),
        "Selected registration should be removed",
    )?;
    require(
        entries
            .iter()
            .any(|row| row["name"]["display"] == "other-missing"),
        "Other missing registration must remain",
    )?;
    Fixture::docker(&[
        "test",
        "-f",
        &format!("{}/.git", fixture.checkout("saved-missing")),
    ])?;
    require(
        fixture.git(&["rev-parse", "refs/heads/missing"])?.trim() == oid,
        "Pruning must preserve branch",
    )?;
    Ok(())
}
