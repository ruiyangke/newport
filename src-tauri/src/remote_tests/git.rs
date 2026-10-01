//! Real SSH round trip against the disposable Linux server, never user projects.
use super::*;
use crate::git::{
    client::Client as TransportClient,
    protocol::{Path, Request, Side},
};

/// Optional test-only wire capture for exercising the TypeScript boundary against
/// real agent responses. This wrapper is never part of production RPC traffic.
#[path = "git_metrics.rs"]
mod metrics;
struct Client<S> {
    inner: TransportClient<metrics::Stream<S>>,
    counters: std::sync::Arc<metrics::Counters>,
    trace: Option<std::fs::File>,
}
impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin> Client<S> {
    async fn start(stream: S) -> Result<(Self, serde_json::Value), crate::git::protocol::Error> {
        Self::start_with_identity(stream, Uuid::new_v4().to_string()).await
    }
    async fn start_with_identity(
        stream: S,
        identity: String,
    ) -> Result<(Self, serde_json::Value), crate::git::protocol::Error> {
        let stream = metrics::Stream::new(stream);
        let counters = stream.counters.clone();
        let (inner, info) = TransportClient::start_with_identity(stream, identity).await?;
        let trace = std::env::var_os("NEWPORT_GIT_TRACE_PATH").map(|path| {
            let mut options = std::fs::OpenOptions::new();
            options.create(true).append(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options.open(path).expect("open explicit Git test trace")
        });
        Ok((
            Self {
                inner,
                counters,
                trace,
            },
            info,
        ))
    }
    async fn request(
        &mut self,
        request: Request,
    ) -> Result<serde_json::Value, crate::git::protocol::Error> {
        use std::io::Write;
        let captured = serde_json::to_value(&request).unwrap();
        let before = self.counters.snapshot();
        let started = std::time::Instant::now();
        let result = self.inner.request(request).await;
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        let after = self.counters.snapshot();
        let measurements = serde_json::json!({"elapsedMs":elapsed_ms,"requestBytes":after[0]-before[0],"responseBytes":after[1]-before[1],"requestFrames":after[2]-before[2],"responseFrames":after[3]-before[3]});
        if let Some(trace) = &mut self.trace {
            let mut row = match &result {
                Ok(response) => serde_json::json!({"request":captured,"response":response}),
                Err(error) => serde_json::json!({"request":captured,"error":error}),
            };
            row["measurements"] = measurements;
            serde_json::to_writer(&mut *trace, &row).expect("write Git test trace");
            trace
                .write_all(b"\n")
                .expect("finish Git test trace record");
            trace.flush().expect("flush Git test trace");
        }
        result
    }
    async fn ping(&mut self) -> Result<(), crate::git::protocol::Error> {
        self.inner.ping().await
    }
}

#[tokio::test]
#[ignore = "Requires the disposable OpenSSH fixture"]
async fn git_rpc_roundtrip() {
    let temporary = tempfile::tempdir().unwrap();
    let git = |root: &std::path::Path, args: &[&str]| {
        let output = std::process::Command::new("git")
            .current_dir(root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    git(
        temporary.path(),
        &["init", "--template=", "--initial-branch=main"],
    );
    git(temporary.path(), &["config", "user.name", "Fixture"]);
    git(
        temporary.path(),
        &["config", "user.email", "fixture@example.test"],
    );
    std::fs::write(temporary.path().join("sample.txt"), "before\n").unwrap();
    git(temporary.path(), &["add", "sample.txt"]);
    git(temporary.path(), &["commit", "-m", "Fixture commit"]);
    let changed = (0..5000)
        .map(|i| format!("line {i}: content for streaming\n"))
        .collect::<String>();
    std::fs::write(temporary.path().join("sample.txt"), changed).unwrap();
    let blob_content = "pageable blob content\n".repeat(30_000);
    let blob_file = temporary.path().join(".git/blob-fixture");
    std::fs::write(&blob_file, &blob_content).unwrap();
    let pageable_blob = git(
        temporary.path(),
        &["hash-object", "-w", blob_file.to_str().unwrap()],
    );
    std::fs::remove_file(blob_file).unwrap();
    let mut archive = tar::Builder::new(Vec::new());
    archive.append_dir_all(".", temporary.path()).unwrap();
    let bytes = archive.into_inner().unwrap();
    let bare_temp = tempfile::tempdir().unwrap();
    git(
        bare_temp.path(),
        &["init", "--bare", "--initial-branch=main", "--template="],
    );
    let mut bare_archive = tar::Builder::new(Vec::new());
    bare_archive.append_dir_all(".", bare_temp.path()).unwrap();
    let bare_bytes = bare_archive.into_inner().unwrap();
    let server = server();
    let session = ExecSession::connect(&server).await.unwrap();
    crate::agent::install(&session).await.unwrap();
    let root = format!("/tmp/newport-git-{}", Uuid::new_v4());
    session
        .execute(
            &format!("mkdir {root} && tar -xf - -C {root}"),
            Some(&bytes),
        )
        .await
        .unwrap();
    let remote_root = format!("{root}-remote.git");
    session
        .execute(
            &format!("mkdir {remote_root} && tar -xf - -C {remote_root}"),
            Some(&bare_bytes),
        )
        .await
        .unwrap();
    let stream = session
        .stream("exec \"$HOME/.local/bin/newport-agent\" git-rpc --stdio")
        .await
        .unwrap();
    let client_id = Uuid::new_v4().to_string();
    let (mut client, info) = Client::start_with_identity(stream, client_id.clone())
        .await
        .unwrap();
    assert!(info["capabilities"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == "stage"));
    let opened = client
        .request(Request::Open {
            path: Path::new(root.as_bytes()),
        })
        .await
        .unwrap();
    let repo_id = opened["repoId"].as_str().unwrap().to_owned();
    let mut blob_cursor = None;
    let mut blob_bytes = Vec::new();
    loop {
        let page = client
            .request(Request::BlobPage {
                repo_id: repo_id.clone(),
                oid: pageable_blob.to_string(),
                max_bytes: Some(if blob_cursor.is_none() { 4096 } else { 524288 }),
                cursor: blob_cursor,
            })
            .await
            .unwrap();
        assert_eq!(page["metadata"]["size"], blob_content.len());
        for entry in page["entries"].as_array().unwrap() {
            assert_eq!(entry["offset"], blob_bytes.len());
            blob_bytes.extend(
                base64::Engine::decode(
                    &base64::engine::general_purpose::STANDARD,
                    entry["bytesB64"].as_str().unwrap(),
                )
                .unwrap(),
            );
        }
        blob_cursor = page["nextCursor"].as_str().map(str::to_owned);
        if blob_cursor.is_none() {
            break;
        }
    }
    assert_eq!(blob_bytes, blob_content.as_bytes());
    let status = client
        .request(Request::Status {
            filter: None,
            repo_id: repo_id.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let summary = client
        .request(Request::StatusSummary {
            repo_id: Some(repo_id.clone()),
            path: None,
        })
        .await
        .unwrap();
    assert_eq!(summary["totalEntries"], status["metadata"]["totalEntries"]);
    assert!(summary.get("entries").is_none());
    let by_path = client
        .request(Request::StatusSummary {
            repo_id: None,
            path: Some(serde_json::from_value(opened["root"].clone()).unwrap()),
        })
        .await
        .unwrap();
    assert_eq!(summary, by_path);

    assert_eq!(status["entries"].as_array().unwrap().len(), 1);
    let entry_id = status["entries"][0]["entryId"].as_str().unwrap().to_owned();
    let diff_request = Request::Diff {
        repo_id: repo_id.clone(),
        snapshot: status["snapshot"].as_str().unwrap().into(),
        entry_id: entry_id.clone(),
        side: Side::IndexToWorktree,
        context_lines: 3,
    };
    let diff = client.request(diff_request.clone()).await.unwrap();
    assert_eq!(diff["diff"]["truncated"], false);
    assert!(
        diff["diff"]["files"][0]["hunks"][0]["lines"]
            .as_array()
            .unwrap()
            .len()
            > 5000
    );
    let mut cursor = None;
    let mut paged_lines = Vec::new();
    loop {
        let page = client
            .request(Request::DiffPage {
                line_encoding: Some(crate::git::protocol::DiffLineEncoding::TupleV1),
                repo_id: repo_id.clone(),
                snapshot: status["snapshot"].as_str().unwrap().into(),
                entry_id: entry_id.clone(),
                side: Side::IndexToWorktree,
                context_lines: 3,
                page_size: 5000,
                max_bytes: Some(65536),
                cursor,
            })
            .await
            .unwrap();
        assert_eq!(page["metadata"]["sourceSnapshot"], status["snapshot"]);
        for file in page["entries"].as_array().unwrap() {
            for hunk in file["hunks"].as_array().unwrap() {
                for line in hunk["lines"].as_array().unwrap() {
                    assert_eq!(line[2], true);
                    paged_lines.push((hunk["id"].clone(), line[7].clone(), line[6].clone()));
                }
            }
        }
        cursor = page["nextCursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    let legacy_lines: Vec<_> = diff["diff"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|file| {
            file["hunks"].as_array().unwrap().iter().flat_map(|hunk| {
                hunk["lines"].as_array().unwrap().iter().map(|line| {
                    (
                        hunk["id"].clone(),
                        line["id"].clone(),
                        line["content"]["bytesB64"].clone(),
                    )
                })
            })
        })
        .collect();
    assert_eq!(paged_lines, legacy_lines);
    let history = client
        .request(Request::History {
            message_bytes: None,
            repo_id: repo_id.clone(),
            page_size: 10,
            cursor: None,
            revision: "HEAD".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        history["entries"][0]["message"]["display"],
        "Fixture commit\n"
    );
    let summary = client
        .request(Request::History {
            message_bytes: Some(7),
            repo_id: repo_id.clone(),
            page_size: 10,
            cursor: None,
            revision: "HEAD".into(),
        })
        .await
        .unwrap();
    assert_eq!(summary["entries"][0]["message"]["display"], "Fixture");
    assert_eq!(summary["entries"][0]["messageTruncated"], true);
    assert_eq!(summary["entries"][0]["oid"], history["entries"][0]["oid"]);
    let detail = client
        .request(Request::Commit {
            repo_id: repo_id.clone(),
            commit_oid: history["entries"][0]["oid"]["hex"].as_str().unwrap().into(),
        })
        .await
        .unwrap();
    assert_eq!(detail, history["entries"][0]);
    assert_eq!(detail["messageTruncated"], false);

    let branches = client
        .request(Request::Branches {
            filter: String::new(),
            branch_kind: None,
            repo_id: repo_id.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(branches["entries"].as_array().unwrap().len(), 1);
    session
        .execute(&format!("printf changed >> {root}/sample.txt"), None)
        .await
        .unwrap();
    assert_eq!(
        client.request(diff_request).await.unwrap_err().code,
        "STALE_SNAPSHOT"
    );
    partial_staging_roundtrip(&mut client, &repo_id).await;
    line_selection_roundtrip(&mut client, &repo_id, &session, &root).await;
    sha256_repository(&mut client, &session, &root).await;
    let status = client
        .request(Request::Status {
            filter: None,
            repo_id: repo_id.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let operation_id = uuid::Uuid::new_v4().to_string();
    let operation = Request::Start {
        operation_id: operation_id.clone(),
        repo_id: repo_id.clone(),
        expected_snapshot: status["snapshot"].as_str().unwrap().into(),
        action: crate::git::protocol::Action::Stage {
            hunks: None,
            entry_ids: vec![status["entries"][0]["entryId"].as_str().unwrap().into()],
        },
    };
    let result = client.request(operation.clone()).await.unwrap();
    assert_eq!(result["state"], "succeeded", "{result}");
    assert_eq!(client.request(operation.clone()).await.unwrap(), result);
    assert_eq!(
        client
            .request(Request::Get {
                operation_id: operation_id.clone()
            })
            .await
            .unwrap(),
        result
    );
    // Only the disposable fixture's journal is altered: simulate an interrupted
    // record without killing a process in the middle of a Git write.
    assert_eq!(
        client
            .request(Request::Review {
                operation_id: operation_id.clone()
            })
            .await
            .unwrap_err()
            .code,
        "OPERATION_NOT_REVIEWABLE"
    );
    let before_review = client
        .request(Request::Status {
            filter: None,
            repo_id: repo_id.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let payload =
        serde_json::to_vec(&serde_json::json!({"file":format!("{client_id}-{operation_id}.json")}))
            .unwrap();
    session
        .execute("newport-test-fixture interrupt-journal", Some(&payload))
        .await
        .unwrap();
    assert_eq!(
        client
            .request(Request::Review {
                operation_id: operation_id.clone()
            })
            .await
            .unwrap_err()
            .code,
        "OPERATION_NOT_REVIEWABLE"
    );
    let interrupted = client
        .request(Request::Get {
            operation_id: operation_id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(interrupted["state"], "outcome_unknown");
    let reviewed = client
        .request(Request::Review {
            operation_id: operation_id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(reviewed["state"], "reviewed_unknown");
    assert_eq!(
        reviewed["seq"].as_u64().unwrap(),
        interrupted["seq"].as_u64().unwrap() + 1
    );
    assert_eq!(reviewed["result"], interrupted["result"]);
    assert_eq!(reviewed["error"], interrupted["error"]);
    assert_eq!(
        client
            .request(Request::Review {
                operation_id: operation_id.clone()
            })
            .await
            .unwrap(),
        reviewed
    );
    let (mut stranger, _) = Client::start(
        session
            .stream("exec \"$HOME/.local/bin/newport-agent\" git-rpc --stdio")
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        stranger
            .request(Request::Review {
                operation_id: operation_id.clone()
            })
            .await
            .unwrap_err()
            .code,
        "OPERATION_NOT_FOUND"
    );
    drop(stranger);
    let (mut reconnected, _) = Client::start_with_identity(
        session
            .stream("exec \"$HOME/.local/bin/newport-agent\" git-rpc --stdio")
            .await
            .unwrap(),
        client_id.clone(),
    )
    .await
    .unwrap();
    assert_eq!(
        reconnected
            .request(Request::Review {
                operation_id: operation_id.clone()
            })
            .await
            .unwrap(),
        reviewed
    );
    drop(reconnected);
    assert_eq!(client.request(operation).await.unwrap(), reviewed);
    assert_eq!(
        client
            .request(Request::Status {
                filter: None,
                repo_id: repo_id.clone(),
                page_size: 100,
                cursor: None
            })
            .await
            .unwrap(),
        before_review
    );

    let status = client
        .request(Request::Status {
            filter: None,
            repo_id: repo_id.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let commit_id = Uuid::new_v4().to_string();
    let mut commit_request = Request::Start {
        operation_id: commit_id.clone(),
        repo_id: repo_id.clone(),
        expected_snapshot: status["snapshot"].as_str().unwrap().into(),
        action: crate::git::protocol::Action::Commit {
            message: "Remote commit through Newport".into(),
            author: Some(crate::git::protocol::Author {
                name: "Fixture".into(),
                email: "fixture@example.test".into(),
            }),
        },
    };
    let committed = client.request(commit_request.clone()).await.unwrap();
    assert_eq!(committed["state"], "succeeded", "{committed}");
    drop(client);
    let stream = session
        .stream("exec \"$HOME/.local/bin/newport-agent\" git-rpc --stdio")
        .await
        .unwrap();
    let (mut client, _) = Client::start_with_identity(stream, client_id)
        .await
        .unwrap();
    assert_eq!(
        client
            .request(Request::Get {
                operation_id: commit_id
            })
            .await
            .unwrap(),
        committed
    );
    let reopened = client
        .request(Request::Open {
            path: Path::new(root.as_bytes()),
        })
        .await
        .unwrap();
    let repo_id = reopened["repoId"].as_str().unwrap().to_owned();
    if let Request::Start { repo_id: id, .. } = &mut commit_request {
        *id = repo_id.clone();
    }
    assert_eq!(client.request(commit_request).await.unwrap(), committed);
    let history = client
        .request(Request::History {
            message_bytes: None,
            repo_id: repo_id.clone(),
            revision: "HEAD".into(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(history["entries"].as_array().unwrap().len(), 2);
    let head_oid = committed["result"]["commitOid"]
        .as_str()
        .unwrap()
        .to_owned();
    for action in [
        crate::git::protocol::Action::BranchCreate {
            name: "remote-topic".into(),
            start_oid: head_oid.clone(),
        },
        crate::git::protocol::Action::BranchRename {
            name: "remote-topic".into(),
            new_name: "remote-renamed".into(),
            expected_oid: head_oid.clone(),
        },
        crate::git::protocol::Action::BranchDelete {
            name: "remote-renamed".into(),
            expected_oid: head_oid,
            force: false,
        },
    ] {
        let status = client
            .request(Request::Status {
                filter: None,
                repo_id: repo_id.clone(),
                page_size: 100,
                cursor: None,
            })
            .await
            .unwrap();
        let request = Request::Start {
            operation_id: Uuid::new_v4().to_string(),
            repo_id: repo_id.clone(),
            expected_snapshot: status["snapshot"].as_str().unwrap().into(),
            action,
        };
        let result = client.request(request.clone()).await.unwrap();
        assert_eq!(result["state"], "succeeded", "{result}");
        assert_eq!(client.request(request).await.unwrap(), result);
    }
    let checkout_oid = committed["result"]["commitOid"]
        .as_str()
        .unwrap()
        .to_owned();
    let status = client
        .request(Request::Status {
            filter: None,
            repo_id: repo_id.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let checkout = Request::Start {
        operation_id: Uuid::new_v4().to_string(),
        repo_id: repo_id.clone(),
        expected_snapshot: status["snapshot"].as_str().unwrap().into(),
        action: crate::git::protocol::Action::Checkout {
            target: crate::git::protocol::CheckoutTarget::Detached { oid: checkout_oid },
        },
    };
    let result = client.request(checkout.clone()).await.unwrap();
    assert_eq!(result["state"], "succeeded", "{result}");
    assert_eq!(result["result"]["detached"], true);
    assert_eq!(client.request(checkout).await.unwrap(), result);
    let status = client
        .request(Request::Status {
            filter: None,
            repo_id: repo_id.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let added_remote = client
        .request(Request::Start {
            operation_id: Uuid::new_v4().to_string(),
            repo_id: repo_id.clone(),
            expected_snapshot: status["snapshot"].as_str().unwrap().into(),
            action: crate::git::protocol::Action::RemoteAdd {
                name: "origin".into(),
                url: remote_root.clone(),
            },
        })
        .await
        .unwrap();
    assert_eq!(added_remote["state"], "succeeded", "{added_remote}");
    let remotes = client
        .request(Request::Remotes {
            repo_id: repo_id.clone(),
        })
        .await
        .unwrap();
    let selected_remote = client
        .request(Request::Remote {
            repo_id: repo_id.clone(),
            name: "origin".into(),
        })
        .await
        .unwrap();
    assert_eq!(selected_remote, remotes["entries"][0]);
    let remote_names = client
        .request(Request::RemoteNames {
            repo_id: repo_id.clone(),
            filter: "ORIGIN".into(),
            page_size: 1,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(
        remote_names["entries"],
        serde_json::json!([{"name":"origin"}])
    );
    assert!(remote_names["nextCursor"].is_null());

    let remote_token = remotes["entries"][0]["token"].as_str().unwrap().to_owned();
    for action in [
        crate::git::protocol::Action::Push {
            remote: "origin".into(),
            expected_token: remote_token.clone(),
            branch: branches["entries"][0]["name"]["display"]
                .as_str()
                .unwrap()
                .into(),
            expected_oid: committed["result"]["commitOid"].as_str().unwrap().into(),
            destination_branch: "main".into(),
        },
        crate::git::protocol::Action::Fetch {
            remote: "origin".into(),
            expected_token: remote_token,
            prune: true,
        },
    ] {
        let status = client
            .request(Request::Status {
                filter: None,
                repo_id: repo_id.clone(),
                page_size: 100,
                cursor: None,
            })
            .await
            .unwrap();
        let request = Request::Start {
            operation_id: Uuid::new_v4().to_string(),
            repo_id: repo_id.clone(),
            expected_snapshot: status["snapshot"].as_str().unwrap().into(),
            action,
        };
        let result = client.request(request.clone()).await.unwrap();
        assert_eq!(result["state"], "succeeded", "{result}");
        assert_eq!(client.request(request).await.unwrap(), result);
    }
    let refreshed = client
        .request(Request::Branches {
            filter: String::new(),
            branch_kind: None,
            repo_id: repo_id.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert!(refreshed["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["remote"] == true));
    // Outbound Git SSH uses an agent entirely inside the disposable server.
    // These commands create test credentials; production transfers use the Git CLI.
    let agent_dir = format!("{root}-agent");
    session.execute(&format!("mkdir -m 700 {agent_dir} && ssh-keygen -q -t ed25519 -N '' -f {agent_dir}/key && cat {agent_dir}/key.pub >> ~/.ssh/authorized_keys && ssh-agent -a {agent_dir}/socket > {agent_dir}/env && SSH_AUTH_SOCK={agent_dir}/socket ssh-add {agent_dir}/key"), None).await.unwrap();
    let ssh_stream = session.stream(&format!("SSH_AUTH_SOCK={agent_dir}/socket exec \"$HOME/.local/bin/newport-agent\" git-rpc --stdio")).await.unwrap();
    let (mut ssh_client, _) = Client::start(ssh_stream).await.unwrap();
    let opened = ssh_client
        .request(Request::Open {
            path: Path::new(root.as_bytes()),
        })
        .await
        .unwrap();
    let ssh_repo = opened["repoId"].as_str().unwrap().to_owned();
    let remotes = ssh_client
        .request(Request::Remotes {
            repo_id: ssh_repo.clone(),
        })
        .await
        .unwrap();
    let status = ssh_client
        .request(Request::Status {
            filter: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let changed = ssh_client
        .request(Request::Start {
            operation_id: Uuid::new_v4().to_string(),
            repo_id: ssh_repo.clone(),
            expected_snapshot: status["snapshot"].as_str().unwrap().into(),
            action: crate::git::protocol::Action::RemoteSetUrl {
                name: "origin".into(),
                url: format!("ssh://fixture@127.0.0.1{remote_root}"),
                expected_token: remotes["entries"][0]["token"].as_str().unwrap().into(),
            },
        })
        .await
        .unwrap();
    assert_eq!(changed["state"], "succeeded", "{changed}");
    let remotes = ssh_client
        .request(Request::Remotes {
            repo_id: ssh_repo.clone(),
        })
        .await
        .unwrap();
    let token = remotes["entries"][0]["token"].as_str().unwrap().to_owned();
    let untrusted = ssh_client
        .request(Request::Start {
            operation_id: Uuid::new_v4().to_string(),
            repo_id: ssh_repo.clone(),
            expected_snapshot: status["snapshot"].as_str().unwrap().into(),
            action: crate::git::protocol::Action::Fetch {
                remote: "origin".into(),
                expected_token: token.clone(),
                prune: false,
            },
        })
        .await
        .unwrap();
    assert_eq!(untrusted["state"], "failed", "{untrusted}");
    assert_eq!(
        untrusted["error"]["code"], "CERTIFICATE_REJECTED",
        "{untrusted}"
    );
    session.execute("printf '127.0.0.1 ' >> ~/.ssh/known_hosts && cat /etc/ssh/ssh_host_ed25519_key.pub >> ~/.ssh/known_hosts", None).await.unwrap();
    for action in [
        crate::git::protocol::Action::Fetch {
            remote: "origin".into(),
            expected_token: token.clone(),
            prune: false,
        },
        crate::git::protocol::Action::Push {
            remote: "origin".into(),
            expected_token: token,
            branch: branches["entries"][0]["name"]["display"]
                .as_str()
                .unwrap()
                .into(),
            expected_oid: committed["result"]["commitOid"].as_str().unwrap().into(),
            destination_branch: "ssh-copy".into(),
        },
    ] {
        let status = ssh_client
            .request(Request::Status {
                filter: None,
                repo_id: ssh_repo.clone(),
                page_size: 100,
                cursor: None,
            })
            .await
            .unwrap();
        let result = ssh_client
            .request(Request::Start {
                operation_id: Uuid::new_v4().to_string(),
                repo_id: ssh_repo.clone(),
                expected_snapshot: status["snapshot"].as_str().unwrap().into(),
                action,
            })
            .await
            .unwrap();
        assert_eq!(result["state"], "succeeded", "{result}");
    }
    let initial_oid = git(temporary.path(), &["rev-parse", "HEAD"]);
    let remotes = ssh_client
        .request(Request::Remotes {
            repo_id: ssh_repo.clone(),
        })
        .await
        .unwrap();
    let pull_token = remotes["entries"][0]["token"].as_str().unwrap().to_owned();
    for action in [
        crate::git::protocol::Action::BranchCreate {
            name: "pull-demo".into(),
            start_oid: initial_oid.clone(),
        },
        crate::git::protocol::Action::Checkout {
            target: crate::git::protocol::CheckoutTarget::Branch {
                name: "pull-demo".into(),
                expected_oid: initial_oid,
            },
        },
        crate::git::protocol::Action::PullFastForward {
            remote: "origin".into(),
            expected_token: pull_token,
            remote_branch: "main".into(),
        },
    ] {
        let status = ssh_client
            .request(Request::Status {
                filter: None,
                repo_id: ssh_repo.clone(),
                page_size: 100,
                cursor: None,
            })
            .await
            .unwrap();
        let request = Request::Start {
            operation_id: Uuid::new_v4().to_string(),
            repo_id: ssh_repo.clone(),
            expected_snapshot: status["snapshot"].as_str().unwrap().into(),
            action,
        };
        let result = ssh_client.request(request.clone()).await.unwrap();
        assert_eq!(result["state"], "succeeded", "{result}");
        assert_eq!(ssh_client.request(request).await.unwrap(), result);
    }
    let branches = ssh_client
        .request(Request::Branches {
            filter: String::new(),
            branch_kind: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let pulled = branches["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"]["display"] == "pull-demo")
        .unwrap();
    assert_eq!(pulled["current"], true);
    assert_eq!(pulled["oid"]["hex"], committed["result"]["commitOid"]);
    use crate::git::protocol::{Action, Author, CheckoutTarget};
    let author = Author {
        name: "Fixture".into(),
        email: "fixture@example.test".into(),
    };
    let base_oid = committed["result"]["commitOid"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        run_operation(&mut ssh_client, &ssh_repo, |_| Action::BranchCreate {
            name: "merge-topic".into(),
            start_oid: base_oid.clone()
        })
        .await["state"],
        "succeeded"
    );
    session
        .execute(&format!("printf 'ours\\n' > {root}/sample.txt"), None)
        .await
        .unwrap();
    assert_eq!(
        run_operation(&mut ssh_client, &ssh_repo, stage_all).await["state"],
        "succeeded"
    );
    let ours = run_operation(&mut ssh_client, &ssh_repo, |_| Action::Commit {
        message: "Ours".into(),
        author: Some(author.clone()),
    })
    .await;
    assert_eq!(ours["state"], "succeeded", "{ours}");
    assert_eq!(
        run_operation(&mut ssh_client, &ssh_repo, |_| Action::Checkout {
            target: CheckoutTarget::Branch {
                name: "merge-topic".into(),
                expected_oid: base_oid
            }
        })
        .await["state"],
        "succeeded"
    );
    session
        .execute(&format!("printf 'theirs\\n' > {root}/sample.txt"), None)
        .await
        .unwrap();
    assert_eq!(
        run_operation(&mut ssh_client, &ssh_repo, stage_all).await["state"],
        "succeeded"
    );
    let theirs = run_operation(&mut ssh_client, &ssh_repo, |_| Action::Commit {
        message: "Theirs".into(),
        author: Some(author.clone()),
    })
    .await;
    assert_eq!(theirs["state"], "succeeded", "{theirs}");
    assert_eq!(
        run_operation(&mut ssh_client, &ssh_repo, |_| Action::Checkout {
            target: CheckoutTarget::Branch {
                name: "pull-demo".into(),
                expected_oid: ours["result"]["commitOid"].as_str().unwrap().into()
            }
        })
        .await["state"],
        "succeeded"
    );
    let merged = run_operation(&mut ssh_client, &ssh_repo, |_| Action::Merge {
        target_oid: theirs["result"]["commitOid"].as_str().unwrap().into(),
    })
    .await;
    assert_eq!(merged["state"], "needs_resolution", "{merged}");
    let before_abort = ssh_client
        .request(Request::Status {
            filter: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let abort_request = Request::Start {
        operation_id: Uuid::new_v4().to_string(),
        repo_id: ssh_repo.clone(),
        expected_snapshot: before_abort["snapshot"].as_str().unwrap().into(),
        action: Action::MergeAbort {},
    };
    let aborted = ssh_client.request(abort_request.clone()).await.unwrap();
    assert_eq!(aborted["state"], "succeeded", "{aborted}");
    assert_eq!(aborted["result"]["aborted"], true);
    assert_eq!(ssh_client.request(abort_request).await.unwrap(), aborted);
    let after_abort = ssh_client
        .request(Request::Status {
            filter: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(after_abort["metadata"]["operationState"], "Clean");
    assert!(after_abort["entries"].as_array().unwrap().is_empty());
    let merged_again = run_operation(&mut ssh_client, &ssh_repo, |_| Action::Merge {
        target_oid: theirs["result"]["commitOid"].as_str().unwrap().into(),
    })
    .await;
    assert_eq!(merged_again["state"], "needs_resolution", "{merged_again}");

    let status = ssh_client
        .request(Request::Status {
            filter: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let conflict = status["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["conflicted"] == true)
        .unwrap();
    let blob = ssh_client
        .request(Request::Blob {
            repo_id: ssh_repo.clone(),
            oid: conflict["conflict"]["theirs"]["oid"]["hex"]
                .as_str()
                .unwrap()
                .into(),
        })
        .await
        .unwrap();
    use base64::Engine;
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(blob["bytesB64"].as_str().unwrap())
            .unwrap(),
        b"theirs\n"
    );
    // Resolve by choosing an existing side: no client bytes cross the API.
    let theirs_oid = conflict["conflict"]["theirs"]["oid"]["hex"]
        .as_str()
        .unwrap()
        .to_owned();
    let chosen = run_operation(&mut ssh_client, &ssh_repo, |status| {
        Action::ConflictResolve {
            entry_ids: status["entries"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|e| e["conflicted"] == true)
                .take(1)
                .map(|e| e["entryId"].as_str().unwrap().into())
                .collect(),
            side: crate::git::protocol::ConflictSide::Theirs,
            expected_oid: Some(theirs_oid.clone()),
        }
    })
    .await;
    assert_eq!(chosen["state"], "succeeded", "{chosen}");
    assert_eq!(
        session
            .execute(&format!("cat {root}/sample.txt"), None)
            .await
            .unwrap(),
        "theirs\n"
    );
    let cleared = ssh_client
        .request(Request::Status {
            filter: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert!(cleared["entries"]
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["conflicted"] == false));
    session
        .execute(&format!("printf 'resolved\\n' > {root}/sample.txt"), None)
        .await
        .unwrap();
    assert_eq!(
        run_operation(&mut ssh_client, &ssh_repo, stage_all).await["state"],
        "succeeded"
    );
    let resolved = run_operation(&mut ssh_client, &ssh_repo, |_| Action::Commit {
        message: "Resolve merge".into(),
        author: Some(author),
    })
    .await;
    assert_eq!(resolved["state"], "succeeded", "{resolved}");
    assert_eq!(resolved["result"]["mergeCompleted"], true);
    session
        .execute(
            &format!("printf 'stashed work\\n' > {root}/sample.txt"),
            None,
        )
        .await
        .unwrap();
    let saved = run_operation(&mut ssh_client, &ssh_repo, |_| Action::StashSave {
        message: "Remote stash fixture".into(),
        include_untracked: false,
        keep_index: false,
        author: Some(crate::git::protocol::Author {
            name: "Fixture".into(),
            email: "fixture@example.test".into(),
        }),
    })
    .await;
    assert_eq!(saved["state"], "succeeded", "{saved}");
    let stashes = ssh_client
        .request(Request::Stashes {
            repo_id: ssh_repo.clone(),
            page_size: 1,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(stashes["entries"].as_array().unwrap().len(), 1);
    assert_eq!(stashes["entries"][0]["oid"], saved["result"]["oid"]);
    let popped = run_operation(&mut ssh_client, &ssh_repo, |_| Action::StashPop {
        index: Some(0),
        oid: saved["result"]["oid"].as_str().unwrap().into(),
        expected_token: stashes["metadata"]["listToken"].as_str().unwrap().into(),
        reinstate_index: true,
    })
    .await;
    assert_eq!(popped["state"], "succeeded", "{popped}");
    assert_eq!(popped["result"]["dropped"], true);
    let operation_id = popped["operationId"].as_str().unwrap();
    let recorded = ssh_client
        .request(Request::Get {
            operation_id: operation_id.into(),
        })
        .await
        .unwrap();
    assert_eq!(recorded["result"], popped["result"]);
    let stashes = ssh_client
        .request(Request::Stashes {
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert!(stashes["entries"].as_array().unwrap().is_empty());
    // Exercise apply/drop independently of pop and verify working files survive.
    let saved_again = run_operation(&mut ssh_client, &ssh_repo, |_| Action::StashSave {
        message: "Apply/drop SSH coverage".into(),
        include_untracked: false,
        keep_index: false,
        author: Some(crate::git::protocol::Author {
            name: "Fixture".into(),
            email: "fixture@example.test".into(),
        }),
    })
    .await;
    assert_eq!(saved_again["state"], "succeeded", "{saved_again}");
    let stashes = ssh_client
        .request(Request::Stashes {
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let stash_oid = saved_again["result"]["oid"].as_str().unwrap().to_owned();
    let stash_token = stashes["metadata"]["listToken"]
        .as_str()
        .unwrap()
        .to_owned();
    let applied = run_operation(&mut ssh_client, &ssh_repo, |_| Action::StashApply {
        index: Some(0),
        oid: stash_oid.clone(),
        expected_token: stash_token.clone(),
        reinstate_index: true,
    })
    .await;
    assert_eq!(applied["state"], "succeeded", "{applied}");
    let dropped = run_operation(&mut ssh_client, &ssh_repo, |_| Action::StashDrop {
        index: Some(0),
        oid: stash_oid,
        expected_token: stash_token,
    })
    .await;
    assert_eq!(dropped["state"], "succeeded", "{dropped}");
    assert_eq!(
        session
            .execute(&format!("cat {root}/sample.txt"), None)
            .await
            .unwrap(),
        "stashed work\n"
    );
    assert_eq!(
        run_operation(&mut ssh_client, &ssh_repo, stage_all).await["state"],
        "succeeded"
    );
    let unstaged = run_operation(&mut ssh_client, &ssh_repo, |status| Action::Unstage {
        hunks: None,
        entry_ids: status["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["entryId"].as_str().unwrap().into())
            .collect(),
    })
    .await;
    assert_eq!(unstaged["state"], "succeeded", "{unstaged}");
    let status = ssh_client
        .request(Request::Status {
            filter: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert!(status["entries"]
        .as_array()
        .unwrap()
        .iter()
        .all(|entry| entry["staged"] == false));
    assert!(status["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["unstaged"] == true));
    let tagged = run_operation(&mut ssh_client, &ssh_repo, |status| Action::TagCreate {
        name: "fixture/release".into(),
        target_oid: status["metadata"]["head"]["oid"]["hex"]
            .as_str()
            .unwrap()
            .into(),
        annotation: Some(crate::git::protocol::TagAnnotation {
            message: "Remote release notes".into(),
            author: Some(crate::git::protocol::Author {
                name: "Fixture".into(),
                email: "fixture@example.test".into(),
            }),
        }),
    })
    .await;
    assert_eq!(tagged["state"], "succeeded", "{tagged}");
    let tags = ssh_client
        .request(Request::Tags {
            message_bytes: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(
        tags["entries"][0]["message"]["display"],
        "Remote release notes\n"
    );
    let summary = ssh_client
        .request(Request::Tags {
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
            message_bytes: Some(6),
        })
        .await
        .unwrap();
    assert_eq!(summary["entries"][0]["message"]["display"], "Remote");
    assert_eq!(summary["entries"][0]["messageTruncated"], true);
    let mut detail = ssh_client
        .request(Request::Tag {
            repo_id: ssh_repo.clone(),
            oid: tags["entries"][0]["oid"]["hex"].as_str().unwrap().into(),
        })
        .await
        .unwrap();
    for field in ["name", "reference", "symbolicTarget"] {
        assert!(detail.get(field).is_none());
        detail[field] = tags["entries"][0][field].clone();
    }
    assert_eq!(detail, tags["entries"][0]);

    let remotes = ssh_client
        .request(Request::Remotes {
            repo_id: ssh_repo.clone(),
        })
        .await
        .unwrap();
    let pushed = run_operation(&mut ssh_client, &ssh_repo, |_| Action::TagPush {
        remote: "origin".into(),
        expected_token: remotes["entries"][0]["token"].as_str().unwrap().into(),
        name: "fixture/release".into(),
        expected_oid: tagged["result"]["oid"].as_str().unwrap().into(),
    })
    .await;
    assert_eq!(pushed["state"], "succeeded", "{pushed}");
    for for_push in [false, true] {
        let advertised = ssh_client
            .request(Request::RemoteRefs {
                filter: String::new(),
                repo_id: ssh_repo.clone(),
                remote: "origin".into(),
                expected_token: remotes["entries"][0]["token"].as_str().unwrap().into(),
                for_push,
                page_size: 100,
                cursor: None,
            })
            .await
            .unwrap();
        assert_eq!(advertised["metadata"]["basis"], "remote_advertisement");
        assert_eq!(advertised["metadata"]["forPush"], for_push);
        let tag = advertised["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["reference"]["display"] == "refs/tags/fixture/release")
            .unwrap();
        assert_eq!(tag["kind"], "tag");
        assert_eq!(tag["oid"]["hex"], tagged["result"]["oid"]);
        if !for_push {
            let peeled = advertised["entries"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["reference"]["display"] == "refs/tags/fixture/release^{}")
                .unwrap();
            assert_eq!(peeled["kind"], "peeled_tag");
            assert_eq!(peeled["oid"]["hex"], tagged["result"]["targetOid"]);
        }
    }
    let removed = run_operation(&mut ssh_client, &ssh_repo, |_| Action::TagDelete {
        name: "fixture/release".into(),
        expected_oid: tagged["result"]["oid"].as_str().unwrap().into(),
    })
    .await;
    assert_eq!(removed["state"], "succeeded", "{removed}");
    let local_tags = ssh_client
        .request(Request::Tags {
            message_bytes: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert!(local_tags["entries"].as_array().unwrap().is_empty());
    let bare = ssh_client
        .request(Request::Open {
            path: Path::new(remote_root.as_bytes()),
        })
        .await
        .unwrap();
    let remote_tags = ssh_client
        .request(Request::Tags {
            message_bytes: None,
            repo_id: bare["repoId"].as_str().unwrap().into(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(
        remote_tags["entries"][0]["oid"]["hex"],
        tagged["result"]["oid"]
    );
    assert_eq!(
        remote_tags["entries"][0]["message"]["display"],
        "Remote release notes\n"
    );
    let replay_author = || {
        Some(Author {
            name: "Replay User".into(),
            email: "replay@example.test".into(),
        })
    };
    assert_eq!(
        run_operation(&mut ssh_client, &ssh_repo, stage_all).await["state"],
        "succeeded"
    );
    let source = run_operation(&mut ssh_client, &ssh_repo, |_| Action::Commit {
        message: "Replay fixture".into(),
        author: replay_author(),
    })
    .await;
    assert_eq!(source["state"], "succeeded", "{source}");
    let reverted = run_operation(&mut ssh_client, &ssh_repo, |_| Action::Revert {
        target_oid: source["result"]["commitOid"].as_str().unwrap().into(),
        mainline: 0,
        author: replay_author(),
    })
    .await;
    assert_eq!(reverted["state"], "succeeded", "{reverted}");
    assert_eq!(reverted["result"]["integrationCompleted"], "revert");
    let status = ssh_client
        .request(Request::Status {
            filter: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let pick_request = Request::Start {
        operation_id: Uuid::new_v4().to_string(),
        repo_id: ssh_repo.clone(),
        expected_snapshot: status["snapshot"].as_str().unwrap().into(),
        action: Action::CherryPick {
            target_oid: source["result"]["commitOid"].as_str().unwrap().into(),
            mainline: 0,
            author: replay_author(),
        },
    };
    let picked = ssh_client.request(pick_request.clone()).await.unwrap();
    assert_eq!(picked["state"], "succeeded", "{picked}");
    assert_eq!(picked["result"]["integrationCompleted"], "cherry_pick");
    assert_eq!(ssh_client.request(pick_request).await.unwrap(), picked);
    let conflicted_pick = || Action::CherryPick {
        target_oid: theirs["result"]["commitOid"].as_str().unwrap().into(),
        mainline: 0,
        author: replay_author(),
    };
    let conflicted = run_operation(&mut ssh_client, &ssh_repo, |_| conflicted_pick()).await;
    assert_eq!(conflicted["state"], "needs_resolution", "{conflicted}");
    let status = ssh_client
        .request(Request::Status {
            filter: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(status["metadata"]["integration"]["kind"], "cherry_pick");
    assert_eq!(status["metadata"]["integration"]["canAbort"], true);
    let aborted = run_operation(&mut ssh_client, &ssh_repo, |_| Action::IntegrationAbort {}).await;
    assert_eq!(aborted["state"], "succeeded", "{aborted}");
    let conflicted = run_operation(&mut ssh_client, &ssh_repo, |_| conflicted_pick()).await;
    assert_eq!(conflicted["state"], "needs_resolution", "{conflicted}");
    session
        .execute(
            &format!("printf 'replay resolved\\n' > {root}/sample.txt"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        run_operation(&mut ssh_client, &ssh_repo, stage_all).await["state"],
        "succeeded"
    );
    let resumed = run_operation(&mut ssh_client, &ssh_repo, |_| {
        Action::IntegrationContinue {
            message: None,
            author: replay_author(),
        }
    })
    .await;
    assert_eq!(resumed["state"], "succeeded", "{resumed}");
    assert_eq!(resumed["result"]["integrationCompleted"], "cherry_pick");
    let status = ssh_client
        .request(Request::Status {
            filter: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(status["metadata"]["operationState"], "Clean");
    assert!(status["metadata"]["integration"].is_null());
    assert!(status["entries"].as_array().unwrap().is_empty());
    let original = status["metadata"]["head"]["oid"]["hex"]
        .as_str()
        .unwrap()
        .to_owned();
    let amend_request = Request::Start {
        operation_id: Uuid::new_v4().to_string(),
        repo_id: ssh_repo.clone(),
        expected_snapshot: status["snapshot"].as_str().unwrap().into(),
        action: Action::Amend {
            expected_oid: original.clone(),
            message: "Amended remote replay".into(),
            committer: replay_author(),
            author: None,
        },
    };
    let amended = ssh_client.request(amend_request.clone()).await.unwrap();
    assert_eq!(amended["state"], "succeeded", "{amended}");
    assert_eq!(amended["result"]["amended"], true);
    assert_eq!(amended["result"]["replacedOid"], original);
    assert_eq!(ssh_client.request(amend_request).await.unwrap(), amended);
    let history = ssh_client
        .request(Request::History {
            message_bytes: None,
            repo_id: ssh_repo.clone(),
            revision: "HEAD".into(),
            page_size: 1,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(
        history["entries"][0]["message"]["display"],
        "Amended remote replay\n"
    );
    assert_eq!(
        history["entries"][0]["parents"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        history["entries"][0]["oid"]["hex"],
        amended["result"]["commitOid"]
    );
    let branches = ssh_client
        .request(Request::Branches {
            filter: String::new(),
            branch_kind: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let current = branches["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["current"] == true)
        .unwrap();
    let status = ssh_client
        .request(Request::Status {
            filter: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let upstream_request = Request::Start {
        operation_id: Uuid::new_v4().to_string(),
        repo_id: ssh_repo.clone(),
        expected_snapshot: status["snapshot"].as_str().unwrap().into(),
        action: Action::BranchSetUpstream {
            name: current["name"]["display"].as_str().unwrap().into(),
            expected_oid: current["oid"]["hex"].as_str().unwrap().into(),
            expected_token: current["tracking"]["token"].as_str().unwrap().into(),
            upstream: Some("refs/remotes/origin/main".into()),
        },
    };
    let tracked = ssh_client.request(upstream_request.clone()).await.unwrap();
    assert_eq!(tracked["state"], "succeeded", "{tracked}");
    assert_eq!(ssh_client.request(upstream_request).await.unwrap(), tracked);
    let status = ssh_client
        .request(Request::Status {
            filter: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(
        status["metadata"]["upstreamRef"]["display"],
        "refs/remotes/origin/main"
    );
    assert!(status["metadata"]["ahead"].as_u64().unwrap() > 0);
    let cleared = run_operation(&mut ssh_client, &ssh_repo, |_| Action::BranchSetUpstream {
        name: current["name"]["display"].as_str().unwrap().into(),
        expected_oid: current["oid"]["hex"].as_str().unwrap().into(),
        expected_token: tracked["result"]["tracking"]["token"]
            .as_str()
            .unwrap()
            .into(),
        upstream: None,
    })
    .await;
    assert_eq!(cleared["state"], "succeeded", "{cleared}");
    assert!(cleared["result"]["upstream"].is_null());
    // Rebase through the framed SSH protocol, including durable replay and recovery.
    let rebase_original = ours["result"]["commitOid"].as_str().unwrap().to_owned();
    let rebase_onto = theirs["result"]["commitOid"].as_str().unwrap().to_owned();
    assert_eq!(
        run_operation(&mut ssh_client, &ssh_repo, |_| Action::BranchCreate {
            name: "rebase-demo".into(),
            start_oid: rebase_original.clone()
        })
        .await["state"],
        "succeeded"
    );
    assert_eq!(
        run_operation(&mut ssh_client, &ssh_repo, |_| Action::Checkout {
            target: CheckoutTarget::Branch {
                name: "rebase-demo".into(),
                expected_oid: rebase_original.clone()
            }
        })
        .await["state"],
        "succeeded"
    );
    let status = ssh_client
        .request(Request::Status {
            filter: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let rebase_request = Request::Start {
        operation_id: Uuid::new_v4().to_string(),
        repo_id: ssh_repo.clone(),
        expected_snapshot: status["snapshot"].as_str().unwrap().into(),
        action: Action::Rebase {
            upstream_oid: rebase_onto.clone(),
            onto_oid: None,
            committer: replay_author(),
        },
    };
    let rebased = ssh_client.request(rebase_request.clone()).await.unwrap();
    assert_eq!(rebased["state"], "needs_resolution", "{rebased}");
    assert_eq!(ssh_client.request(rebase_request).await.unwrap(), rebased);
    let status = ssh_client
        .request(Request::Status {
            filter: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(status["metadata"]["integration"]["kind"], "rebase");
    assert_eq!(status["metadata"]["integration"]["managed"], true);
    let aborted = run_operation(&mut ssh_client, &ssh_repo, |_| Action::IntegrationAbort {}).await;
    assert_eq!(aborted["state"], "succeeded", "{aborted}");
    assert_eq!(aborted["result"]["oid"], rebase_original);
    let skip_rebase = run_operation(&mut ssh_client, &ssh_repo, |_| Action::Rebase {
        upstream_oid: rebase_onto.clone(),
        onto_oid: None,
        committer: replay_author(),
    })
    .await;
    assert_eq!(skip_rebase["state"], "needs_resolution", "{skip_rebase}");
    let skipped = run_operation(&mut ssh_client, &ssh_repo, |_| Action::IntegrationSkip {}).await;
    assert_eq!(skipped["state"], "succeeded", "{skipped}");
    let restored = run_operation(&mut ssh_client, &ssh_repo, |status| {
        assert_eq!(status["metadata"]["head"]["oid"]["hex"], rebase_onto);
        assert!(status["metadata"]["integration"].is_null());
        Action::Reset {
            target_oid: rebase_original.clone(),
            expected_oid: rebase_onto.clone(),
            mode: crate::git::protocol::ResetMode::Hard,
        }
    })
    .await;
    assert_eq!(restored["state"], "succeeded", "{restored}");

    let rebased = run_operation(&mut ssh_client, &ssh_repo, |_| Action::Rebase {
        upstream_oid: rebase_onto.clone(),
        onto_oid: None,
        committer: replay_author(),
    })
    .await;
    assert_eq!(rebased["state"], "needs_resolution", "{rebased}");
    session
        .execute(
            &format!("printf 'rebased resolution\\n' > {root}/sample.txt"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        run_operation(&mut ssh_client, &ssh_repo, stage_all).await["state"],
        "succeeded"
    );
    let completed = run_operation(&mut ssh_client, &ssh_repo, |_| {
        Action::IntegrationContinue {
            message: None,
            author: None,
        }
    })
    .await;
    assert_eq!(completed["state"], "succeeded", "{completed}");
    assert_eq!(completed["result"]["integrationCompleted"], "rebase");
    // Explicit reset modes use the same durable operation identity as commits.
    use crate::git::protocol::ResetMode;
    let rebased_oid = completed["result"]["commitOid"]
        .as_str()
        .unwrap()
        .to_owned();
    let soft = run_operation(&mut ssh_client, &ssh_repo, |_| Action::Reset {
        target_oid: rebase_onto.clone(),
        expected_oid: rebased_oid.clone(),
        mode: ResetMode::Soft,
    })
    .await;
    assert_eq!(soft["state"], "succeeded", "{soft}");
    assert_eq!(soft["result"]["previousOid"], rebased_oid);
    let mixed = run_operation(&mut ssh_client, &ssh_repo, |_| Action::Reset {
        target_oid: rebased_oid.clone(),
        expected_oid: rebase_onto.clone(),
        mode: ResetMode::Mixed,
    })
    .await;
    assert_eq!(mixed["state"], "succeeded", "{mixed}");
    let status = ssh_client
        .request(Request::Status {
            filter: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let hard_request = Request::Start {
        operation_id: Uuid::new_v4().to_string(),
        repo_id: ssh_repo.clone(),
        expected_snapshot: status["snapshot"].as_str().unwrap().into(),
        action: Action::Reset {
            target_oid: rebase_onto.clone(),
            expected_oid: rebased_oid,
            mode: ResetMode::Hard,
        },
    };
    let hard = ssh_client.request(hard_request.clone()).await.unwrap();
    assert_eq!(hard["state"], "succeeded", "{hard}");
    assert_eq!(ssh_client.request(hard_request).await.unwrap(), hard);
    use crate::git::protocol::DiscardSource;
    session
        .execute(
            &format!("printf 'staged discard\\n' > {root}/sample.txt"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        run_operation(&mut ssh_client, &ssh_repo, stage_all).await["state"],
        "succeeded"
    );
    session.execute(&format!("printf 'working discard\\n' > {root}/sample.txt && printf 'new\\n' > {root}/discard-new.txt"),None).await.unwrap();
    let status = ssh_client
        .request(Request::Status {
            filter: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let discard_request = Request::Start {
        operation_id: Uuid::new_v4().to_string(),
        repo_id: ssh_repo.clone(),
        expected_snapshot: status["snapshot"].as_str().unwrap().into(),
        action: Action::Discard {
            source: DiscardSource::Index,
            hunks: None,
            entry_ids: status["entries"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|e| {
                    matches!(
                        e["path"]["display"].as_str(),
                        Some("sample.txt" | "discard-new.txt")
                    )
                })
                .map(|e| e["entryId"].as_str().unwrap().into())
                .collect(),
        },
    };
    let discarded = ssh_client.request(discard_request.clone()).await.unwrap();
    assert_eq!(discarded["state"], "succeeded", "{discarded}");
    assert_eq!(discarded["result"]["indexChanged"], false);
    assert_eq!(
        ssh_client.request(discard_request).await.unwrap(),
        discarded
    );
    let status = ssh_client
        .request(Request::Status {
            filter: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert!(!status["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["path"]["display"] == "discard-new.txt"));
    let entry = status["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["path"]["display"] == "sample.txt")
        .unwrap();
    assert_eq!(entry["staged"], true);
    assert_eq!(entry["unstaged"], false);
    let discarded = run_operation(&mut ssh_client, &ssh_repo, |status| Action::Discard {
        source: DiscardSource::Head,
        hunks: None,
        entry_ids: status["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["path"]["display"] == "sample.txt")
            .map(|e| e["entryId"].as_str().unwrap().into())
            .collect(),
    })
    .await;
    assert_eq!(discarded["state"], "succeeded", "{discarded}");
    let status = ssh_client
        .request(Request::Status {
            filter: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert!(!status["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["path"]["display"] == "sample.txt"));
    let history_diff = ssh_client
        .request(Request::CommitDiff {
            path: None,
            repo_id: ssh_repo.clone(),
            commit_oid: completed["result"]["commitOid"].as_str().unwrap().into(),
            parent_index: 0,
            context_lines: 3,
        })
        .await
        .unwrap();
    assert_eq!(
        history_diff["diff"]["commitOid"]["hex"],
        completed["result"]["commitOid"]
    );
    assert_eq!(history_diff["diff"]["parentOid"]["hex"], rebase_onto);
    assert!(!history_diff["diff"]["files"].as_array().unwrap().is_empty());
    let merge_diff = ssh_client
        .request(Request::CommitDiff {
            path: None,
            repo_id: ssh_repo.clone(),
            commit_oid: resolved["result"]["commitOid"].as_str().unwrap().into(),
            parent_index: 1,
            context_lines: 3,
        })
        .await
        .unwrap();
    assert_eq!(merge_diff["diff"]["parents"].as_array().unwrap().len(), 2);
    assert_eq!(merge_diff["diff"]["parentIndex"], 1);
    let root_diff = ssh_client
        .request(Request::CommitDiff {
            path: None,
            repo_id: ssh_repo.clone(),
            commit_oid: git(temporary.path(), &["rev-parse", "HEAD"]),
            parent_index: 0,
            context_lines: 3,
        })
        .await
        .unwrap();
    assert!(root_diff["diff"]["parentOid"].is_null());
    assert_eq!(root_diff["diff"]["files"][0]["status"], "Added");
    let commit_files = ssh_client
        .request(Request::CommitFiles {
            repo_id: ssh_repo.clone(),
            commit_oid: completed["result"]["commitOid"].as_str().unwrap().into(),
            parent_index: 0,
            page_size: 1,
            cursor: None,
        })
        .await
        .unwrap();
    assert!(!commit_files["entries"].as_array().unwrap().is_empty());
    assert!(commit_files["entries"][0].get("hunks").is_none());
    let selected: Path =
        serde_json::from_value(commit_files["entries"][0]["newPath"].clone()).unwrap();
    let selected_diff = ssh_client
        .request(Request::CommitDiff {
            repo_id: ssh_repo.clone(),
            commit_oid: completed["result"]["commitOid"].as_str().unwrap().into(),
            parent_index: 0,
            context_lines: 3,
            path: Some(selected.clone()),
        })
        .await
        .unwrap();
    assert_eq!(selected_diff["diff"]["files"].as_array().unwrap().len(), 1);
    assert_eq!(
        selected_diff["diff"]["files"][0]["newOid"],
        commit_files["entries"][0]["newOid"]
    );
    let mut cursor = None;
    let mut snapshot = None;
    let mut units = 0;
    loop {
        let page = ssh_client
            .request(Request::CommitDiffPage {
                line_encoding: Some(crate::git::protocol::DiffLineEncoding::TupleV1),
                repo_id: ssh_repo.clone(),
                commit_oid: completed["result"]["commitOid"].as_str().unwrap().into(),
                parent_index: 0,
                context_lines: 3,
                path: selected.clone(),
                page_size: 1,
                max_bytes: Some(65536),
                cursor,
            })
            .await
            .unwrap();
        assert_eq!(page["metadata"]["readOnly"], true);
        if let Some(previous) = &snapshot {
            assert_eq!(previous, &page["snapshot"]);
        }
        snapshot = Some(page["snapshot"].clone());
        for file in page["entries"].as_array().unwrap() {
            let hunks = file["hunks"].as_array().unwrap();
            units += if hunks.is_empty() {
                1
            } else {
                hunks
                    .iter()
                    .map(|h| h["lines"].as_array().unwrap().len())
                    .sum::<usize>()
            };
        }
        cursor = page["nextCursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            assert_eq!(
                units as u64,
                page["metadata"]["totalUnits"].as_u64().unwrap()
            );
            break;
        }
    }
    let remotes = ssh_client
        .request(Request::Remotes {
            repo_id: ssh_repo.clone(),
        })
        .await
        .unwrap();
    let token = remotes["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "origin")
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let status = ssh_client
        .request(Request::Status {
            filter: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let rename_request = Request::Start {
        operation_id: Uuid::new_v4().to_string(),
        repo_id: ssh_repo.clone(),
        expected_snapshot: status["snapshot"].as_str().unwrap().into(),
        action: Action::RemoteRename {
            name: "origin".into(),
            new_name: "upstream".into(),
            expected_token: token,
        },
    };
    let renamed = ssh_client.request(rename_request.clone()).await.unwrap();
    assert_eq!(renamed["state"], "succeeded", "{renamed}");
    assert!(renamed["result"]["renamedReferences"].as_u64().unwrap() > 0);
    assert_eq!(ssh_client.request(rename_request).await.unwrap(), renamed);
    let remotes = ssh_client
        .request(Request::Remotes {
            repo_id: ssh_repo.clone(),
        })
        .await
        .unwrap();
    assert!(!remotes["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["name"] == "origin"));
    let fetched = run_operation(&mut ssh_client, &ssh_repo, |_| Action::Fetch {
        remote: "upstream".into(),
        expected_token: renamed["result"]["token"].as_str().unwrap().into(),
        prune: false,
    })
    .await;
    assert_eq!(fetched["state"], "succeeded", "{fetched}");
    let lease_tip = completed["result"]["commitOid"]
        .as_str()
        .unwrap()
        .to_owned();
    let lease_rewrite = ours["result"]["commitOid"].as_str().unwrap().to_owned();
    let lease_token = renamed["result"]["token"].as_str().unwrap().to_owned();
    assert_eq!(
        run_operation(&mut ssh_client, &ssh_repo, |_| Action::BranchCreate {
            name: "lease-source".into(),
            start_oid: lease_tip.clone()
        })
        .await["state"],
        "succeeded"
    );
    assert_eq!(
        run_operation(&mut ssh_client, &ssh_repo, |_| Action::Push {
            remote: "upstream".into(),
            expected_token: lease_token.clone(),
            branch: "lease-source".into(),
            expected_oid: lease_tip.clone(),
            destination_branch: "lease-remote".into()
        })
        .await["state"],
        "succeeded"
    );
    assert_eq!(
        run_operation(&mut ssh_client, &ssh_repo, |_| Action::Checkout {
            target: CheckoutTarget::Branch {
                name: "lease-source".into(),
                expected_oid: lease_tip.clone()
            }
        })
        .await["state"],
        "succeeded"
    );
    assert_eq!(
        run_operation(&mut ssh_client, &ssh_repo, |_| Action::Reset {
            target_oid: lease_rewrite.clone(),
            expected_oid: lease_tip.clone(),
            mode: ResetMode::Hard
        })
        .await["state"],
        "succeeded"
    );
    let lease = |remote_oid: String, destination: &str, local_oid: String| Action::PushWithLease {
        remote: "upstream".into(),
        expected_token: lease_token.clone(),
        branch: "lease-source".into(),
        expected_oid: local_oid,
        destination_branch: destination.into(),
        expected_remote_oid: remote_oid,
    };
    let status = ssh_client
        .request(Request::Status {
            filter: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let lease_request = Request::Start {
        operation_id: Uuid::new_v4().to_string(),
        repo_id: ssh_repo.clone(),
        expected_snapshot: status["snapshot"].as_str().unwrap().into(),
        action: lease(lease_tip.clone(), "lease-remote", lease_rewrite.clone()),
    };
    let pushed = ssh_client.request(lease_request.clone()).await.unwrap();
    assert_eq!(pushed["state"], "succeeded", "{pushed}");
    assert_eq!(pushed["result"]["leaseMatched"], true);
    assert_eq!(ssh_client.request(lease_request).await.unwrap(), pushed);
    let stale = run_operation(&mut ssh_client, &ssh_repo, |_| {
        lease(lease_tip.clone(), "lease-remote", lease_rewrite.clone())
    })
    .await;
    assert_eq!(stale["error"]["code"], "STALE_REMOTE_REFERENCE", "{stale}");
    let absent = run_operation(&mut ssh_client, &ssh_repo, |_| {
        lease(lease_rewrite.clone(), "lease-new", lease_rewrite.clone())
    })
    .await;
    assert_eq!(
        absent["error"]["code"], "STALE_REMOTE_REFERENCE",
        "{absent}"
    );
    let created = run_operation(&mut ssh_client, &ssh_repo, |_| {
        lease("0".repeat(40), "lease-new", lease_rewrite.clone())
    })
    .await;
    assert_eq!(created["state"], "succeeded", "{created}");
    assert_eq!(
        run_operation(&mut ssh_client, &ssh_repo, |_| Action::Reset {
            target_oid: lease_tip.clone(),
            expected_oid: lease_rewrite.clone(),
            mode: ResetMode::Soft
        })
        .await["state"],
        "succeeded"
    );
    // Fixture-only receive hook changes the ref after advertisement and before
    // receive-pack's transaction, exercising the server-side old-OID guard.
    let raced_oid = git(temporary.path(), &["rev-parse", "HEAD"]);
    let hook=format!("#!/bin/sh\nif [ \"$1\" = refs/heads/lease-remote ]; then\n  git update-ref refs/heads/lease-remote {raced_oid}\nfi\nexit 0\n");
    session
        .execute(
            &format!("mkdir -p {remote_root}/hooks && cat > {remote_root}/hooks/update && chmod 700 {remote_root}/hooks/update"),
            Some(hook.as_bytes()),
        )
        .await
        .unwrap();
    let raced = run_operation(&mut ssh_client, &ssh_repo, |_| {
        lease(lease_rewrite.clone(), "lease-remote", lease_tip.clone())
    })
    .await;
    session
        .execute(&format!("rm {remote_root}/hooks/update"), None)
        .await
        .unwrap();
    assert_eq!(raced["error"]["code"], "PUSH_REJECTED", "{raced}");
    assert_eq!(
        run_operation(&mut ssh_client, &ssh_repo, |_| Action::Fetch {
            remote: "upstream".into(),
            expected_token: lease_token.clone(),
            prune: false
        })
        .await["state"],
        "succeeded"
    );
    let branches = ssh_client
        .request(Request::Branches {
            filter: String::new(),
            branch_kind: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let raced_branch = branches["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["reference"]["display"] == "refs/remotes/upstream/lease-remote")
        .unwrap();
    assert_eq!(raced_branch["oid"]["hex"], raced_oid);
    let delete_remote = |oid: String| Action::BranchDeleteRemote {
        remote: "upstream".into(),
        expected_token: lease_token.clone(),
        branch: "lease-new".into(),
        expected_oid: oid,
    };
    let stale_delete = run_operation(&mut ssh_client, &ssh_repo, |_| {
        delete_remote(lease_tip.clone())
    })
    .await;
    assert_eq!(
        stale_delete["error"]["code"], "STALE_REMOTE_REFERENCE",
        "{stale_delete}"
    );
    let zero_delete = run_operation(&mut ssh_client, &ssh_repo, |_| {
        delete_remote("0".repeat(40))
    })
    .await;
    assert_eq!(
        zero_delete["error"]["code"], "INVALID_REQUEST",
        "{zero_delete}"
    );
    let hook=format!("#!/bin/sh\nif [ \"$1\" = refs/heads/lease-new ]; then\n  git update-ref refs/heads/lease-new {raced_oid}\nfi\nexit 0\n");
    session
        .execute(
            &format!("mkdir -p {remote_root}/hooks && cat > {remote_root}/hooks/update && chmod 700 {remote_root}/hooks/update"),
            Some(hook.as_bytes()),
        )
        .await
        .unwrap();
    let raced_delete = run_operation(&mut ssh_client, &ssh_repo, |_| {
        delete_remote(lease_rewrite.clone())
    })
    .await;
    session
        .execute(&format!("rm {remote_root}/hooks/update"), None)
        .await
        .unwrap();
    assert_eq!(
        raced_delete["error"]["code"], "PUSH_REJECTED",
        "{raced_delete}"
    );
    assert_eq!(
        run_operation(&mut ssh_client, &ssh_repo, |_| Action::Fetch {
            remote: "upstream".into(),
            expected_token: lease_token.clone(),
            prune: true
        })
        .await["state"],
        "succeeded"
    );
    let branches = ssh_client
        .request(Request::Branches {
            filter: String::new(),
            branch_kind: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let saved = branches["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["reference"]["display"] == "refs/remotes/upstream/lease-new")
        .unwrap();
    assert_eq!(saved["oid"]["hex"], raced_oid);
    let status = ssh_client
        .request(Request::Status {
            filter: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let delete_request = Request::Start {
        operation_id: Uuid::new_v4().to_string(),
        repo_id: ssh_repo.clone(),
        expected_snapshot: status["snapshot"].as_str().unwrap().into(),
        action: delete_remote(raced_oid.clone()),
    };
    let deleted = ssh_client.request(delete_request.clone()).await.unwrap();
    assert_eq!(deleted["state"], "succeeded", "{deleted}");
    assert_eq!(deleted["result"]["deleted"], true);
    assert_eq!(deleted["result"]["leaseMatched"], true);
    assert!(deleted["result"]["oid"].is_null());
    assert_eq!(ssh_client.request(delete_request).await.unwrap(), deleted);
    assert_eq!(
        run_operation(&mut ssh_client, &ssh_repo, |_| Action::Fetch {
            remote: "upstream".into(),
            expected_token: lease_token.clone(),
            prune: true
        })
        .await["state"],
        "succeeded"
    );
    let branches = ssh_client
        .request(Request::Branches {
            filter: String::new(),
            branch_kind: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert!(!branches["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["reference"]["display"] == "refs/remotes/upstream/lease-new"));
    let local = branches["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["reference"]["display"] == "refs/heads/lease-source")
        .unwrap();
    assert_eq!(local["oid"]["hex"], lease_tip);
    let delete_tag = |oid: String| Action::TagDeleteRemote {
        remote: "upstream".into(),
        expected_token: lease_token.clone(),
        name: "fixture/release".into(),
        expected_oid: oid,
    };
    // A tag lease identifies the annotation itself, never its peeled commit.
    let peeled_delete = run_operation(&mut ssh_client, &ssh_repo, |_| {
        delete_tag(tagged["result"]["targetOid"].as_str().unwrap().into())
    })
    .await;
    assert_eq!(
        peeled_delete["error"]["code"], "STALE_REMOTE_REFERENCE",
        "{peeled_delete}"
    );
    let zero_tag = run_operation(&mut ssh_client, &ssh_repo, |_| delete_tag("0".repeat(40))).await;
    assert_eq!(zero_tag["error"]["code"], "INVALID_REQUEST", "{zero_tag}");
    let local_tags_before = ssh_client
        .request(Request::Tags {
            message_bytes: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    // Fixture-only hook changes the reference after negotiation. The server's
    // final old-OID check must reject deletion and preserve the competing tag.
    let hook = format!("#!/bin/sh\nif [ \"$1\" = refs/tags/fixture/release ]; then\n  git update-ref refs/tags/fixture/release {raced_oid}\nfi\nexit 0\n");
    session
        .execute(
            &format!("mkdir -p {remote_root}/hooks && cat > {remote_root}/hooks/update && chmod 700 {remote_root}/hooks/update"),
            Some(hook.as_bytes()),
        )
        .await
        .unwrap();
    let raced_tag = run_operation(&mut ssh_client, &ssh_repo, |_| {
        delete_tag(tagged["result"]["oid"].as_str().unwrap().into())
    })
    .await;
    session
        .execute(&format!("rm {remote_root}/hooks/update"), None)
        .await
        .unwrap();
    assert_eq!(raced_tag["error"]["code"], "PUSH_REJECTED", "{raced_tag}");
    let remote_tags = ssh_client
        .request(Request::Tags {
            message_bytes: None,
            repo_id: bare["repoId"].as_str().unwrap().into(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let preserved_tag = remote_tags["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"]["display"] == "fixture/release")
        .unwrap();
    assert_eq!(preserved_tag["oid"]["hex"], raced_oid);
    let status = ssh_client
        .request(Request::Status {
            filter: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let delete_request = Request::Start {
        operation_id: Uuid::new_v4().to_string(),
        repo_id: ssh_repo.clone(),
        expected_snapshot: status["snapshot"].as_str().unwrap().into(),
        action: delete_tag(raced_oid.clone()),
    };
    let deleted_tag = ssh_client.request(delete_request.clone()).await.unwrap();
    assert_eq!(deleted_tag["state"], "succeeded", "{deleted_tag}");
    assert_eq!(deleted_tag["result"]["destinationTag"], "fixture/release");
    assert_eq!(deleted_tag["result"]["deleted"], true);
    assert_eq!(deleted_tag["result"]["leaseMatched"], true);
    assert_eq!(
        ssh_client.request(delete_request).await.unwrap(),
        deleted_tag
    );
    let remote_tags = ssh_client
        .request(Request::Tags {
            message_bytes: None,
            repo_id: bare["repoId"].as_str().unwrap().into(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert!(!remote_tags["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["name"]["display"] == "fixture/release"));
    let absent_tag = run_operation(&mut ssh_client, &ssh_repo, |_| {
        delete_tag(raced_oid.clone())
    })
    .await;
    assert_eq!(
        absent_tag["error"]["code"], "STALE_REMOTE_REFERENCE",
        "{absent_tag}"
    );
    let local_tags_after = ssh_client
        .request(Request::Tags {
            message_bytes: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(local_tags_before["entries"], local_tags_after["entries"]);
    let linked_root = format!("{root}-worktree");
    let branch = run_operation(&mut ssh_client, &ssh_repo, |_| Action::BranchCreate {
        name: "fixture-worktree".into(),
        start_oid: lease_tip.clone(),
    })
    .await;
    assert_eq!(branch["state"], "succeeded", "{branch}");
    let before_add = ssh_client
        .request(Request::Worktrees {
            at_snapshot: None,
            filter: String::new(),
            branch: None,
            name: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let add_request = Request::Start {
        operation_id: Uuid::new_v4().to_string(),
        repo_id: ssh_repo.clone(),
        expected_snapshot: before_add["snapshot"].as_str().unwrap().into(),
        action: Action::WorktreeAdd {
            name: "fixture-worktree".into(),
            path: Path::new(linked_root.as_bytes()),
            branch: "fixture-worktree".into(),
            expected_oid: lease_tip.clone(),
            locked: true,
            new_branch: false,
        },
    };
    let added = ssh_client.request(add_request.clone()).await.unwrap();
    assert_eq!(added["state"], "succeeded", "{added}");
    assert_eq!(added["result"]["openRequired"], true);
    assert_eq!(ssh_client.request(add_request).await.unwrap(), added);
    let worktrees = ssh_client
        .request(Request::Worktrees {
            at_snapshot: None,
            filter: String::new(),
            branch: None,
            name: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(worktrees["entries"].as_array().unwrap().len(), 2);
    assert_eq!(worktrees["entries"][0]["kind"], "main");
    assert_eq!(
        worktrees["entries"][1]["head"]["name"]["display"],
        "refs/heads/fixture-worktree"
    );
    assert_eq!(worktrees["entries"][1]["locked"], true);
    assert_eq!(worktrees["entries"][1]["lockReason"]["display"], "");
    let linked = ssh_client
        .request(Request::Open {
            path: Path::new(linked_root.as_bytes()),
        })
        .await
        .unwrap();
    let linked_id = linked["repoId"].as_str().unwrap().to_owned();
    let from_linked = ssh_client
        .request(Request::Worktrees {
            at_snapshot: None,
            filter: String::new(),
            branch: None,
            name: None,
            repo_id: linked_id.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(from_linked["entries"][1]["current"], true);
    ssh_client
        .request(Request::Close { repo_id: linked_id })
        .await
        .unwrap();
    let worktree_name = worktrees["entries"][1]["name"]["display"]
        .as_str()
        .unwrap()
        .to_owned();
    let unlock = Request::Start {
        operation_id: Uuid::new_v4().to_string(),
        repo_id: ssh_repo.clone(),
        expected_snapshot: worktrees["snapshot"].as_str().unwrap().into(),
        action: Action::WorktreeUnlock {
            name: worktree_name.clone(),
        },
    };
    let unlocked = ssh_client.request(unlock.clone()).await.unwrap();
    assert_eq!(unlocked["state"], "succeeded", "{unlocked}");
    assert_eq!(ssh_client.request(unlock).await.unwrap(), unlocked);
    let unlocked_list = ssh_client
        .request(Request::Worktrees {
            at_snapshot: None,
            filter: String::new(),
            branch: None,
            name: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(unlocked_list["entries"][1]["locked"], false);
    let lock = Request::Start {
        operation_id: Uuid::new_v4().to_string(),
        repo_id: ssh_repo.clone(),
        expected_snapshot: unlocked_list["snapshot"].as_str().unwrap().into(),
        action: Action::WorktreeLock {
            name: worktree_name.clone(),
            reason: Some("Keep this checkout".into()),
        },
    };
    let locked = ssh_client.request(lock.clone()).await.unwrap();
    assert_eq!(locked["state"], "succeeded", "{locked}");
    assert_eq!(ssh_client.request(lock).await.unwrap(), locked);
    session
        .execute(&format!("mv {linked_root} {linked_root}-moved"), None)
        .await
        .unwrap();
    let missing = ssh_client
        .request(Request::Worktrees {
            at_snapshot: None,
            filter: String::new(),
            branch: None,
            name: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(missing["entries"][1]["state"], "missing");
    assert_eq!(missing["entries"][1]["locked"], true);
    assert_eq!(
        missing["entries"][1]["lockReason"]["display"],
        "Keep this checkout"
    );
    let unlock_missing = ssh_client
        .request(Request::Start {
            operation_id: Uuid::new_v4().to_string(),
            repo_id: ssh_repo.clone(),
            expected_snapshot: missing["snapshot"].as_str().unwrap().into(),
            action: Action::WorktreeUnlock {
                name: worktree_name,
            },
        })
        .await
        .unwrap();
    assert_eq!(unlock_missing["state"], "succeeded", "{unlock_missing}");
    let repair_listing = ssh_client
        .request(Request::Worktrees {
            at_snapshot: None,
            filter: String::new(),
            branch: None,
            name: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let moved_root = format!("{linked_root}-moved");
    let repair_request = Request::Start {
        operation_id: Uuid::new_v4().to_string(),
        repo_id: ssh_repo.clone(),
        expected_snapshot: repair_listing["snapshot"].as_str().unwrap().into(),
        action: Action::WorktreeRepair {
            name: "fixture-worktree".into(),
            path: Path::new(moved_root.as_bytes()),
        },
    };
    let repaired = ssh_client.request(repair_request.clone()).await.unwrap();
    assert_eq!(repaired["state"], "succeeded", "{repaired}");
    assert_eq!(repaired["result"]["changed"], true);
    assert_eq!(ssh_client.request(repair_request).await.unwrap(), repaired);
    let repaired_listing = ssh_client
        .request(Request::Worktrees {
            at_snapshot: None,
            filter: String::new(),
            branch: None,
            name: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(repaired_listing["entries"][1]["state"], "available");
    assert_eq!(
        repaired_listing["entries"][1]["path"]["display"],
        moved_root
    );
    let repaired_open = ssh_client
        .request(Request::Open {
            path: Path::new(moved_root.as_bytes()),
        })
        .await
        .unwrap();
    ssh_client
        .request(Request::Close {
            repo_id: repaired_open["repoId"].as_str().unwrap().into(),
        })
        .await
        .unwrap();
    session
        .execute(&format!("mv {moved_root} {moved_root}-again"), None)
        .await
        .unwrap();
    let stale_worktrees = ssh_client
        .request(Request::Worktrees {
            at_snapshot: None,
            filter: String::new(),
            branch: None,
            name: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let prune_request = Request::Start {
        operation_id: Uuid::new_v4().to_string(),
        repo_id: ssh_repo.clone(),
        expected_snapshot: stale_worktrees["snapshot"].as_str().unwrap().into(),
        action: Action::WorktreePrune {
            name: "fixture-worktree".into(),
        },
    };
    let pruned = ssh_client.request(prune_request.clone()).await.unwrap();
    assert_eq!(pruned["state"], "succeeded", "{pruned}");
    assert_eq!(pruned["result"]["registrationOnly"], true);
    assert_eq!(ssh_client.request(prune_request).await.unwrap(), pruned);
    session
        .execute(&format!("test -f {linked_root}-moved-again/.git"), None)
        .await
        .unwrap();
    let worktrees = ssh_client
        .request(Request::Worktrees {
            at_snapshot: None,
            filter: String::new(),
            branch: None,
            name: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(worktrees["entries"].as_array().unwrap().len(), 1);
    let remove_root = format!("{root}-remove");
    let recreated = ssh_client
        .request(Request::Start {
            operation_id: Uuid::new_v4().to_string(),
            repo_id: ssh_repo.clone(),
            expected_snapshot: worktrees["snapshot"].as_str().unwrap().into(),
            action: Action::WorktreeAdd {
                name: "remove-fixture".into(),
                path: Path::new(remove_root.as_bytes()),
                branch: "fixture-worktree".into(),
                expected_oid: lease_tip.clone(),
                locked: false,
                new_branch: false,
            },
        })
        .await
        .unwrap();
    assert_eq!(recreated["state"], "succeeded", "{recreated}");
    let worktrees = ssh_client
        .request(Request::Worktrees {
            at_snapshot: None,
            filter: String::new(),
            branch: None,
            name: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let remove_request = Request::Start {
        operation_id: Uuid::new_v4().to_string(),
        repo_id: ssh_repo.clone(),
        expected_snapshot: worktrees["snapshot"].as_str().unwrap().into(),
        action: Action::WorktreeRemove {
            name: "remove-fixture".into(),
        },
    };
    let removed = ssh_client.request(remove_request.clone()).await.unwrap();
    assert_eq!(removed["state"], "succeeded", "{removed}");
    assert_eq!(removed["result"]["registrationOnly"], false);
    assert_eq!(ssh_client.request(remove_request).await.unwrap(), removed);
    session
        .execute(&format!("test ! -e {remove_root}"), None)
        .await
        .unwrap();
    // A worktree on a new branch, as an agent is given one: the branch is
    // created at the start commit and checked out in the new checkout, over
    // real SSH against real Git, and the checkout opens as its own repository.
    let agent_root = format!("{root}-agent-task");
    let worktrees = ssh_client
        .request(Request::Worktrees {
            at_snapshot: None,
            filter: String::new(),
            branch: None,
            name: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    let agent_request = Request::Start {
        operation_id: Uuid::new_v4().to_string(),
        repo_id: ssh_repo.clone(),
        expected_snapshot: worktrees["snapshot"].as_str().unwrap().into(),
        action: Action::WorktreeAdd {
            name: "agent-task".into(),
            path: Path::new(agent_root.as_bytes()),
            branch: "agent/remote-task".into(),
            expected_oid: lease_tip.clone(),
            locked: false,
            new_branch: true,
        },
    };
    let agent = ssh_client.request(agent_request.clone()).await.unwrap();
    assert_eq!(agent["state"], "succeeded", "{agent}");
    assert_eq!(agent["result"]["branchCreated"], true);
    assert_eq!(ssh_client.request(agent_request).await.unwrap(), agent);
    let checked_out = session
        .execute(
            &format!("git -C {agent_root} rev-parse --abbrev-ref HEAD && git -C {agent_root} rev-parse HEAD"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        checked_out.lines().collect::<Vec<_>>(),
        vec!["agent/remote-task", lease_tip.as_str()]
    );
    let agent_repo = ssh_client
        .request(Request::Open {
            path: Path::new(agent_root.as_bytes()),
        })
        .await
        .unwrap();
    assert_ne!(agent_repo["repoId"], ssh_repo.as_str());
    assert_eq!(
        agent_repo["head"]["name"]["display"],
        "refs/heads/agent/remote-task"
    );
    let init_root = format!("{root}-initialized");
    session
        .execute(
            &format!("mkdir {init_root} && printf 'existing project\\n' > {init_root}/README.md"),
            None,
        )
        .await
        .unwrap();
    let init_request = Request::Init {
        operation_id: Uuid::new_v4().to_string(),
        path: Path::new(init_root.as_bytes()),
        initial_branch: "develop".into(),
    };
    let initialized = ssh_client.request(init_request.clone()).await.unwrap();
    assert_eq!(initialized["state"], "succeeded", "{initialized}");
    assert_eq!(initialized["result"]["initialBranch"], "develop");
    assert_eq!(ssh_client.request(init_request).await.unwrap(), initialized);
    let opened = ssh_client
        .request(Request::Open {
            path: Path::new(init_root.as_bytes()),
        })
        .await
        .unwrap();
    let new_repo = opened["repoId"].as_str().unwrap().to_owned();
    let history = ssh_client
        .request(Request::History {
            message_bytes: None,
            repo_id: new_repo.clone(),
            page_size: 100,
            cursor: None,
            revision: "HEAD".into(),
        })
        .await
        .unwrap();
    assert!(history["entries"].as_array().unwrap().is_empty());
    let status = ssh_client
        .request(Request::Status {
            filter: None,
            repo_id: new_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(status["entries"].as_array().unwrap().len(), 1);
    assert_eq!(status["entries"][0]["path"]["display"], "README.md");
    assert_eq!(status["entries"][0]["untracked"], true);
    assert_eq!(
        run_operation(&mut ssh_client, &new_repo, stage_all).await["state"],
        "succeeded"
    );
    let first_commit = run_operation(&mut ssh_client, &new_repo, |_| Action::Commit {
        message: "Initial project commit".into(),
        author: replay_author(),
    })
    .await;
    assert_eq!(first_commit["state"], "succeeded", "{first_commit}");
    let branch = ssh_client
        .request(Request::Branches {
            filter: String::new(),
            branch_kind: None,
            repo_id: new_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert!(branch["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["name"]["display"] == "develop" && b["current"] == true));
    let duplicate = ssh_client
        .request(Request::Init {
            operation_id: Uuid::new_v4().to_string(),
            path: Path::new(init_root.as_bytes()),
            initial_branch: "main".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(duplicate.code, "ALREADY_REPOSITORY");
    ssh_client
        .request(Request::Close { repo_id: new_repo })
        .await
        .unwrap();
    let clone_root = format!("{root}-cloned");
    let clone_request = Request::Clone {
        operation_id: Uuid::new_v4().to_string(),
        url: format!("ssh://fixture@127.0.0.1{remote_root}"),
        path: Path::new(clone_root.as_bytes()),
        branch: Some("lease-remote".into()),
        bare: false,
    };
    let cloned = ssh_client.request(clone_request.clone()).await.unwrap();
    assert_eq!(cloned["state"], "succeeded", "{cloned}");
    assert_eq!(cloned["result"]["oid"], raced_oid);
    assert_eq!(cloned["result"]["openRequired"], true);
    assert_eq!(ssh_client.request(clone_request).await.unwrap(), cloned);
    let cloned_open = ssh_client
        .request(Request::Open {
            path: Path::new(clone_root.as_bytes()),
        })
        .await
        .unwrap();
    let cloned_id = cloned_open["repoId"].as_str().unwrap().to_owned();
    let cloned_status = ssh_client
        .request(Request::Status {
            filter: None,
            repo_id: cloned_id.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert!(
        cloned_status["entries"].as_array().unwrap().is_empty(),
        "{cloned_status}"
    );
    assert_eq!(cloned_status["metadata"]["head"]["oid"]["hex"], raced_oid);
    assert_eq!(
        cloned_status["metadata"]["head"]["name"]["display"],
        "refs/heads/lease-remote"
    );
    ssh_client
        .request(Request::Close { repo_id: cloned_id })
        .await
        .unwrap();
    let bare_clone_root = format!("{root}-cloned.git");
    let bare_clone = ssh_client
        .request(Request::Clone {
            operation_id: Uuid::new_v4().to_string(),
            url: format!("ssh://fixture@127.0.0.1{remote_root}"),
            path: Path::new(bare_clone_root.as_bytes()),
            branch: Some("lease-remote".into()),
            bare: true,
        })
        .await
        .unwrap();
    assert_eq!(bare_clone["state"], "succeeded", "{bare_clone}");
    let bare_clone = ssh_client
        .request(Request::Open {
            path: Path::new(bare_clone_root.as_bytes()),
        })
        .await
        .unwrap();
    assert_eq!(bare_clone["bare"], true);
    ssh_client
        .request(Request::Close {
            repo_id: bare_clone["repoId"].as_str().unwrap().into(),
        })
        .await
        .unwrap();
    // Earlier collision tests intentionally leave edits in this disposable repo.
    // Preserve those edits before testing a clean fast-forward checkout.
    let preserved = run_operation(&mut ssh_client, &ssh_repo, |_| Action::StashSave {
        message: "Preserve earlier fixture edits".into(),
        include_untracked: true,
        keep_index: false,
        author: replay_author(),
    })
    .await;
    assert_eq!(preserved["state"], "succeeded", "{preserved}");
    let base_oid = git(temporary.path(), &["rev-parse", "HEAD"]);
    let created = run_operation(&mut ssh_client, &ssh_repo, |_| Action::BranchCreate {
        name: "ssh-fast-forward".into(),
        start_oid: base_oid.clone(),
    })
    .await;
    assert_eq!(created["state"], "succeeded", "{created}");
    let checked_out = run_operation(&mut ssh_client, &ssh_repo, |_| Action::Checkout {
        target: CheckoutTarget::Branch {
            name: "ssh-fast-forward".into(),
            expected_oid: base_oid,
        },
    })
    .await;
    assert_eq!(checked_out["state"], "succeeded", "{checked_out}");
    let advanced = run_operation(&mut ssh_client, &ssh_repo, |_| Action::FastForward {
        target_oid: lease_tip.clone(),
    })
    .await;
    assert_eq!(advanced["state"], "succeeded", "{advanced}");
    assert_eq!(advanced["result"]["fastForwarded"], true);
    let status = ssh_client
        .request(Request::Status {
            filter: None,
            repo_id: ssh_repo.clone(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(status["metadata"]["head"]["oid"]["hex"], lease_tip);
    let removed = run_operation(&mut ssh_client, &ssh_repo, |_| Action::RemoteRemove {
        name: "upstream".into(),
        expected_token: lease_token.clone(),
    })
    .await;
    assert_eq!(removed["state"], "succeeded", "{removed}");
    let remotes = ssh_client
        .request(Request::Remotes {
            repo_id: ssh_repo.clone(),
        })
        .await
        .unwrap();
    assert!(!remotes["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|remote| remote["name"] == "upstream"));
    drop(ssh_client);
    client.ping().await.unwrap();
    client
        .request(Request::Close {
            repo_id: repo_id.clone(),
        })
        .await
        .unwrap();
    // Stateless: closing released nothing, so the id still names the
    // repository and reading it still works.
    client
        .request(Request::Status {
            filter: None,
            repo_id,
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    drop(client);
    session.close().await;
}

async fn run_operation<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    client: &mut Client<S>,
    repo_id: &str,
    action: impl FnOnce(&serde_json::Value) -> crate::git::protocol::Action,
) -> serde_json::Value {
    let status = client
        .request(Request::Status {
            filter: None,
            repo_id: repo_id.into(),
            page_size: 100,
            cursor: None,
        })
        .await
        .unwrap();
    client
        .request(Request::Start {
            operation_id: Uuid::new_v4().to_string(),
            repo_id: repo_id.into(),
            expected_snapshot: status["snapshot"].as_str().unwrap().into(),
            action: action(&status),
        })
        .await
        .unwrap()
}
fn stage_all(status: &serde_json::Value) -> crate::git::protocol::Action {
    crate::git::protocol::Action::Stage {
        hunks: None,
        entry_ids: status["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["entryId"].as_str().unwrap().into())
            .collect(),
    }
}

/// A repository using the SHA-256 object format must be readable over SSH with
/// every identifier reported at its own width, not assumed to be SHA-1.
async fn sha256_repository<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    client: &mut Client<S>,
    session: &ExecSession,
    root: &str,
) {
    let path = format!("{root}-sha256");
    let script = format!(
        "mkdir -p {path} && cd {path} && git init -q --object-format=sha256 . \
         && git config user.email t@example.test && git config user.name Test \
         && printf 'one\\n' > file && git add file && git commit -q -m initial \
         && git rev-parse HEAD"
    );
    let head = session.execute(&script, None).await.unwrap();
    let head = head.trim().to_owned();
    assert_eq!(head.len(), 64, "expected a SHA-256 commit id, got {head:?}");

    let opened = client
        .request(Request::Open {
            path: Path::new(path.as_bytes()),
        })
        .await
        .unwrap();
    assert_eq!(opened["objectFormat"], "sha256", "{opened}");
    assert_eq!(opened["head"]["oid"]["algorithm"], "sha256");
    assert_eq!(opened["head"]["oid"]["hex"], head);
    let repo_id = opened["repoId"].as_str().unwrap().to_owned();

    let history = client
        .request(Request::History {
            message_bytes: None,
            repo_id: repo_id.clone(),
            revision: "HEAD".into(),
            page_size: 10,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(history["entries"][0]["oid"]["algorithm"], "sha256");
    assert_eq!(history["entries"][0]["oid"]["hex"], head);

    // A working change is readable, so identifiers flow through status too.
    session
        .execute(&format!("printf 'two\\n' >> {path}/file"), None)
        .await
        .unwrap();
    let status = client
        .request(Request::Status {
            filter: None,
            repo_id: repo_id.clone(),
            page_size: 10,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(status["metadata"]["head"]["oid"]["algorithm"], "sha256");
    assert_eq!(status["entries"].as_array().unwrap().len(), 1, "{status}");
    client.request(Request::Close { repo_id }).await.unwrap();
}

/// Stage individual lines of a file that is not tracked yet, proving over real SSH
/// that only the selected lines reach the index and the rest stay in the worktree.
async fn line_selection_roundtrip<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    client: &mut Client<S>,
    repo_id: &str,
    session: &ExecSession,
    root: &str,
) {
    use crate::git::protocol::{Action, HunkSelection};
    session
        .execute(
            &format!("printf 'one\\ntwo\\nthree\\n' > {root}/lines.txt"),
            None,
        )
        .await
        .unwrap();
    async fn addressable<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
        client: &mut Client<S>,
        repo_id: &str,
        side: Side,
    ) -> (String, String, serde_json::Value) {
        let status = client
            .request(Request::Status {
                filter: None,
                repo_id: repo_id.into(),
                page_size: 100,
                cursor: None,
            })
            .await
            .unwrap();
        let snapshot = status["snapshot"].as_str().unwrap().to_owned();
        let entry_id = status["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["path"]["display"] == "lines.txt")
            .expect("lines.txt listed")["entryId"]
            .as_str()
            .unwrap()
            .to_owned();
        let diff = client
            .request(Request::Diff {
                repo_id: repo_id.into(),
                snapshot: snapshot.clone(),
                entry_id: entry_id.clone(),
                side,
                context_lines: 3,
            })
            .await
            .unwrap();
        (snapshot, entry_id, diff)
    }
    let (snapshot, entry_id, diff) = addressable(client, repo_id, Side::IndexToWorktree).await;
    let hunk = &diff["diff"]["files"][0]["hunks"][0];
    let hunk_id = hunk["id"].as_str().unwrap().to_owned();
    let selected: Vec<String> = hunk["lines"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|line| line["content"]["display"] != "two\n")
        .filter_map(|line| line["id"].as_str().map(String::from))
        .collect();
    assert_eq!(selected.len(), 2, "{hunk}");
    let request = Request::Start {
        operation_id: Uuid::new_v4().to_string(),
        repo_id: repo_id.into(),
        expected_snapshot: snapshot,
        action: Action::Stage {
            entry_ids: vec![entry_id],
            hunks: Some(HunkSelection {
                ids: vec![hunk_id],
                lines: Some(selected),
                context_lines: 3,
            }),
        },
    };
    let result = client.request(request.clone()).await.unwrap();
    assert_eq!(result["state"], "succeeded", "{result}");
    assert_eq!(client.request(request).await.unwrap(), result);
    let contents = |diff: &serde_json::Value, origin: &str| -> Vec<String> {
        diff["diff"]["files"][0]["hunks"][0]["lines"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|line| line["origin"] == origin)
            .map(|line| line["content"]["display"].as_str().unwrap().to_owned())
            .collect()
    };
    let (_, _, staged) = addressable(client, repo_id, Side::HeadToIndex).await;
    assert_eq!(contents(&staged, "+"), ["one\n", "three\n"]);
    let (snapshot, entry_id, remaining) = addressable(client, repo_id, Side::IndexToWorktree).await;
    assert_eq!(contents(&remaining, "+"), ["two\n"]);
    // The unselected line never reached the index and the file is unchanged.
    let worktree = session
        .execute(&format!("cat {root}/lines.txt"), None)
        .await
        .unwrap();
    assert_eq!(worktree, "one\ntwo\nthree\n");
    // Discarding the remaining hunk rewrites only that working file.
    let hunk_id = remaining["diff"]["files"][0]["hunks"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let discard = Request::Start {
        operation_id: Uuid::new_v4().to_string(),
        repo_id: repo_id.into(),
        expected_snapshot: snapshot,
        action: Action::Discard {
            entry_ids: vec![entry_id],
            source: crate::git::protocol::DiscardSource::Index,
            hunks: Some(HunkSelection {
                ids: vec![hunk_id],
                lines: None,
                context_lines: 3,
            }),
        },
    };
    let discarded = client.request(discard.clone()).await.unwrap();
    assert_eq!(discarded["state"], "succeeded", "{discarded}");
    assert_eq!(client.request(discard).await.unwrap(), discarded);
    let worktree = session
        .execute(&format!("cat {root}/lines.txt"), None)
        .await
        .unwrap();
    assert_eq!(worktree, "one\nthree\n");
}

/// Read identifiers over SSH, then replay the exact operation receipt. Returning
/// the hunk to the worktree leaves the surrounding fixture's edits unchanged.
async fn partial_staging_roundtrip<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    client: &mut Client<S>,
    repo_id: &str,
) {
    use crate::git::protocol::{Action, HunkSelection};
    for unstage in [false, true] {
        let status = client
            .request(Request::Status {
                filter: None,
                repo_id: repo_id.into(),
                page_size: 100,
                cursor: None,
            })
            .await
            .unwrap();
        let entry_id = status["entries"][0]["entryId"].as_str().unwrap().to_owned();
        let snapshot = status["snapshot"].as_str().unwrap().to_owned();
        let diff = client
            .request(Request::Diff {
                repo_id: repo_id.into(),
                snapshot: snapshot.clone(),
                entry_id: entry_id.clone(),
                side: if unstage {
                    Side::HeadToIndex
                } else {
                    Side::IndexToWorktree
                },
                context_lines: 3,
            })
            .await
            .unwrap();
        assert_eq!(diff["diff"]["truncated"], false);
        let id = diff["diff"]["files"][0]["hunks"][0]["id"].as_str().unwrap();
        assert_eq!(id.len(), 64);
        let hunks = Some(HunkSelection {
            ids: vec![id.into()],
            lines: None,
            context_lines: 3,
        });
        let entry_ids = vec![entry_id];
        let action = if unstage {
            Action::Unstage { entry_ids, hunks }
        } else {
            Action::Stage { entry_ids, hunks }
        };
        let request = Request::Start {
            operation_id: Uuid::new_v4().to_string(),
            repo_id: repo_id.into(),
            expected_snapshot: snapshot,
            action,
        };
        let result = client.request(request.clone()).await.unwrap();
        assert_eq!(result["state"], "succeeded", "{result}");
        assert_eq!(client.request(request).await.unwrap(), result);
    }
}
