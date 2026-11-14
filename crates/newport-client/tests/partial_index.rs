//! Opt-in partial-index acceptance through desktop IPC, SSH and the installed agent.
//! All mutations are confined to a UUID-owned repository in the loopback fixture.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use newport_client::{backend::Backend, git_discard, git_selection::Review};
use newport_protocol::{
    desktop,
    git::{self as protocol, Operation, OperationState as State},
};
use serde_json::Value;
use std::{
    io::Write,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const CONTAINER: &str = "newport-gpui-files";
const FILE: &str = "range ü.txt";

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
struct Fixture(String);
impl Fixture {
    fn new() -> Result<Self, String> {
        let path = format!(
            "/home/fixture/gpui-partial-test-{}",
            uuid::Uuid::new_v4().simple()
        );
        exec(&["mkdir", "--", &path])?;
        Ok(Self(path))
    }
    fn git(&self, args: &[&str]) -> Result<String, String> {
        let mut command = vec!["git", "-C", &self.0];
        command.extend_from_slice(args);
        exec(&command)
    }
    fn write(&self, name: &str, content: &str) -> Result<(), String> {
        // Positional arguments keep both paths and contents out of shell source.
        let mut child = Command::new("docker")
            .args([
                "exec",
                "-i",
                "--user",
                "fixture",
                CONTAINER,
                "sh",
                "-c",
                "cat > \"$1\"",
                "sh",
                &format!("{}/{name}", self.0),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        let written = child
            .stdin
            .take()
            .ok_or("Missing fixture stdin")?
            .write_all(content.as_bytes());
        let output = child.wait_with_output().map_err(|e| e.to_string())?;
        written.map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned());
        }
        Ok(())
    }
    fn read(&self, name: &str) -> Result<String, String> {
        exec(&["cat", "--", &format!("{}/{name}", self.0)])
    }
    fn assert_contents(&self, index: &str, worktree: &str, head: &str) -> Result<(), String> {
        assert_eq!(
            self.git(&["show", &format!(":{FILE}")])?,
            index,
            "Index bytes"
        );
        assert_eq!(self.read(FILE)?, worktree, "Worktree bytes");
        assert_eq!(
            self.git(&["rev-parse", "HEAD"])?,
            head,
            "HEAD must not move"
        );
        assert_eq!(self.git(&["show", ":unrelated.txt"])?, "original\n");
        assert_eq!(self.read("unrelated.txt")?, "unrelated edit\n");
        Ok(())
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
fn read(
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

fn settled(backend: &Backend, server: &str, mut operation: Operation) -> Result<(), String> {
    let id = operation
        .operation_id
        .parse::<uuid::Uuid>()
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while operation.state == State::Running && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        operation = backend.git_operation(server, id)?;
    }
    assert_eq!(operation.state, State::Succeeded, "{operation:?}");
    assert_eq!(backend.git_operation(server, id)?.state, State::Succeeded);
    acknowledge(backend, server, id)?;
    Ok(())
}
fn selection(
    backend: &Backend,
    server: &str,
    repo: &str,
    staged: bool,
    wanted: &[&str],
) -> Result<(Review, Vec<String>, Vec<String>), String> {
    let protocol::Reply::Status(status) = read(
        backend,
        server,
        protocol::Request::Status {
            repo_id: repo.into(),
            filter: None,
            page_size: 20,
            cursor: None,
        },
    )?
    else {
        return Err("Expected a status page".into());
    };
    assert!(status.next_cursor.is_none());
    let path = protocol::Path::new(FILE.as_bytes());
    let entry = status
        .entries
        .iter()
        .find(|entry| entry.path.bytes_b64 == path.bytes_b64)
        .ok_or("Missing fixture file")?;
    let protocol::Reply::DiffPage(page) = read(
        backend,
        server,
        protocol::Request::DiffPage {
            repo_id: repo.into(),
            snapshot: status.snapshot.clone(),
            entry_id: entry.entry_id.clone(),
            side: if staged {
                protocol::Side::HeadToIndex
            } else {
                protocol::Side::IndexToWorktree
            },
            context_lines: 3,
            page_size: 200,
            max_bytes: Some(65536),
            cursor: None,
            line_encoding: None,
        },
    )?
    else {
        return Err("Expected a diff page".into());
    };
    let review = Review::from_page(&page)?;
    let mut lines = vec![];
    for file in &page.entries {
        for hunk in &file.hunks {
            for line in &hunk.lines {
                if !matches!(line.origin.as_str(), "+" | "-") {
                    continue;
                }
                if wanted
                    .iter()
                    .any(|content| content.as_bytes() == line.content_bytes_b64.0)
                {
                    lines.push(line.id.clone().ok_or("Missing line ID")?);
                }
            }
        }
    }
    assert_eq!(
        lines.len(),
        wanted.len(),
        "Every requested changed line must be present exactly once"
    );
    let hunks = review.hunks_for_lines(&lines)?;
    Ok((review, hunks, lines))
}

#[test]
#[ignore = "requires isolated desktop backend and disposable loopback Docker SSH fixture"]
fn partial_index_changes_preserve_unselected_bytes_and_reject_stale_reviews() -> Result<(), String>
{
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
    fixture.git(&["init", "--initial-branch=main"])?;
    fixture.git(&["config", "user.name", "Native Test"])?;
    fixture.git(&["config", "user.email", "fixture@localhost"])?;
    fixture.git(&["config", "commit.gpgSign", "false"])?;
    let base = (1..=30).map(|n| format!("line {n}\n")).collect::<String>();
    let first = base.replace("line 2\n", "changed α\n");
    let changed = first.replace("line 25\n", "changed β\n");
    fixture.write(FILE, &base)?;
    fixture.write("unrelated.txt", "original\n")?;
    fixture.git(&["add", "--", FILE, "unrelated.txt"])?;
    fixture.git(&["commit", "-m", "Base"])?;
    let head = fixture.git(&["rev-parse", "HEAD"])?;
    fixture.write(FILE, &changed)?;
    fixture.write("unrelated.txt", "unrelated edit\n")?;
    let opened = request(
        &backend,
        server,
        protocol::Request::Open {
            path: protocol::Path::new(fixture.0.as_bytes()),
        },
    )?;
    assert_eq!(opened["root"]["bytesB64"], STANDARD.encode(&fixture.0));
    let repo = opened["repoId"].as_str().ok_or("Missing repository ID")?;

    // A replacement range changes just that region, leaving the second hunk unstaged.
    let (review, hunks, lines) =
        selection(&backend, server, repo, false, &["line 2\n", "changed α\n"])?;
    settled(
        &backend,
        server,
        backend
            .change_index_selection(
                server,
                repo,
                &review,
                &hunks,
                Some(&lines),
                uuid::Uuid::new_v4(),
            )
            .map_err(|e| e.message)?,
    )?;
    fixture.assert_contents(&first, &changed, &head)?;
    eprintln!("Verified selected replacement staged, distant hunk and unrelated file untouched");

    let (review, hunks, lines) =
        selection(&backend, server, repo, true, &["line 2\n", "changed α\n"])?;
    settled(
        &backend,
        server,
        backend
            .change_index_selection(
                server,
                repo,
                &review,
                &hunks,
                Some(&lines),
                uuid::Uuid::new_v4(),
            )
            .map_err(|e| e.message)?,
    )?;
    fixture.assert_contents(&base, &changed, &head)?;
    eprintln!("Verified selected replacement unstaged without modifying working files");

    // An addition-only selection must retain its unselected deletion in the index.
    let (review, hunks, lines) = selection(&backend, server, repo, false, &["changed α\n"])?;
    settled(
        &backend,
        server,
        backend
            .change_index_selection(
                server,
                repo,
                &review,
                &hunks,
                Some(&lines),
                uuid::Uuid::new_v4(),
            )
            .map_err(|e| e.message)?,
    )?;
    fixture.assert_contents(
        &base.replace("line 2\n", "line 2\nchanged α\n"),
        &changed,
        &head,
    )?;
    let (review, hunks, lines) = selection(&backend, server, repo, true, &["changed α\n"])?;
    settled(
        &backend,
        server,
        backend
            .change_index_selection(
                server,
                repo,
                &review,
                &hunks,
                Some(&lines),
                uuid::Uuid::new_v4(),
            )
            .map_err(|e| e.message)?,
    )?;
    fixture.assert_contents(&base, &changed, &head)?;
    eprintln!("Verified addition-only selection preserves the unselected original line");

    let (review, hunks, lines) =
        selection(&backend, server, repo, false, &["line 25\n", "changed β\n"])?;
    let discard = git_discard::Review::selection(&review, &hunks, Some(&lines))?;
    settled(
        &backend,
        server,
        backend
            .discard_changes(
                server,
                repo,
                &discard,
                git_discard::Source::Index,
                uuid::Uuid::new_v4(),
            )
            .map_err(|e| e.message)?,
    )?;
    fixture.assert_contents(&base, &first, &head)?;
    eprintln!("Verified partial discard restores only the selected range");

    let (review, hunks, lines) =
        selection(&backend, server, repo, false, &["line 2\n", "changed α\n"])?;
    let external = first.replace("line 15\n", "external edit\n");
    fixture.write(FILE, &external)?;
    let error = backend
        .change_index_selection(
            server,
            repo,
            &review,
            &hunks,
            Some(&lines),
            uuid::Uuid::new_v4(),
        )
        .expect_err("Stale review must not dispatch a new patch");
    assert!(
        error.message.contains("STALE_SNAPSHOT") || error.message.contains("repository changed"),
        "Unexpected rejection: {error:?}"
    );
    fixture.assert_contents(&base, &external, &head)?;
    eprintln!("Verified stale review rejected with unchanged index and working files");
    Ok(())
}
