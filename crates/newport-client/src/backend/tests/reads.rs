//! Read/lease regressions use typed protobuf peers and retain UI publication checks.
use super::*;
use crate::backend::exchange::{Cause, Phase};
use newport_protocol::desktop::{connection, metrics, stream};

fn connection_test() -> desktop::Operation {
    desktop::Operation::Connection(Box::new(connection::Operation::TestConnection(Box::new(
        connection::TestConnectionArgs { server: server() },
    ))))
}
fn connected() -> desktop::Reply {
    desktop::Reply::Connection(Box::new(connection::Reply::TestConnection(Box::new(
        common::Present {},
    ))))
}
fn git_result(reply: git::Reply) -> desktop::Reply {
    desktop::Reply::Git(Box::new(desktop::git::Reply::Request(Box::new(
        desktop::git::GitResult::Reply(Box::new(reply)),
    ))))
}
fn reply_frame(peer: &Peer, reply: desktop::Reply) -> Vec<u8> {
    wire::encode_server_for(
        &desktop::ServerMessage::Response(Box::new(desktop::Response {
            id: peer.request.id,
            outcome: desktop::Outcome::Completed(Box::new(reply)),
        })),
        &peer.request,
    )
    .unwrap()
}
fn wrong_identity(peer: &mut Peer, reply: desktop::Reply) {
    // Encode a valid message with a deliberately unrelated request identity.
    // The receiving endpoint, rather than the fixture encoder, must reject it.
    let message = desktop::ServerMessage::Response(Box::new(desktop::Response {
        id: desktop::RequestId::new(999).unwrap(),
        outcome: desktop::Outcome::Completed(Box::new(reply)),
    }));
    peer.socket
        .write_all(&wire::encode_server(&message).unwrap())
        .unwrap();
}
fn approval(peer: &mut Peer, awaiting: bool) {
    let message = desktop::ServerMessage::Progress(Box::new(desktop::Progress {
        id: peer.request.id,
        state: desktop::ProgressState::AgentApproval(Box::new(desktop::AgentApproval {
            server: server(),
            awaiting,
        })),
    }));
    peer.socket
        .write_all(&wire::encode_server_for(&message, &peer.request).unwrap())
        .unwrap();
}
fn terminal_open() -> stream::Operation {
    stream::Operation::TerminalOpen(Box::new(stream::TerminalOpenArgs {
        server: server(),
        session: Uuid::from_u128(2),
        columns: 80,
        rows: 24,
        persist: false,
        attach: false,
        colors: None,
    }))
}
fn opened(peer: &mut Peer) {
    assert!(
        matches!(&peer.request.operation, desktop::Operation::Stream(operation)
        if matches!(operation.as_ref(), stream::Operation::TerminalOpen(args)
            if args.server == server() && args.session == Uuid::from_u128(2)))
    );
    peer.outcome(desktop::Outcome::Opened(Box::new(
        stream::Opened::Terminal(Box::new(stream::TerminalOpened {
            version: desktop::VERSION,
            session: Uuid::from_u128(2),
        })),
    )));
}

#[test]
fn profile_discovery_matches_the_host_without_requiring_a_socket_file() {
    let root = std::env::temp_dir().join(format!("np-profile-{}", Uuid::new_v4()));
    assert_eq!(super::super::default_profile(&root), root.join("Newport"));
    assert!(!root.exists());
}

#[test]
fn terminal_lease_child_process() {
    let Some(path) = std::env::var_os("NEWPORT_TEST_TERMINAL_LEASE_SOCKET") else {
        return;
    };
    let _lease = Backend::at_path(path.into())
        .open(terminal_open(), Duration::from_secs(5))
        .unwrap();
    println!("LEASE_READY");
    std::io::stdout().flush().unwrap();
    loop {
        thread::park();
    }
}

#[test]
fn terminal_lease_closes_when_client_process_is_killed_without_destructors() {
    use std::{
        io::{BufRead, BufReader},
        process::{Command, Stdio},
        sync::mpsc,
    };
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let host = Host::new();
    let mut child = Child(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "backend::tests::reads::terminal_lease_child_process",
                "--nocapture",
            ])
            .env("NEWPORT_TEST_TERMINAL_LEASE_SOCKET", &host.path)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    let output = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if line.unwrap().contains("LEASE_READY") {
                sender.send(()).unwrap();
                break;
            }
        }
    });
    let mut peer = host.receive();
    opened(&mut peer);
    receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let mut byte = [0];
    assert_eq!(peer.socket.read(&mut byte).unwrap(), 0);
    output.join().unwrap();
    host.no_request();
}

#[test]
fn an_open_terminal_lease_never_inherits_the_request_deadline() {
    let host = Host::new();
    let backend = Backend::at_path(host.path.clone());
    let task = thread::spawn(move || {
        let mut peer = host.receive();
        opened(&mut peer);
        thread::sleep(Duration::from_millis(350));
        let event = stream::TerminalServer::Event(Box::new(stream::TerminalEvent::Ready(
            Box::new(common::Present {}),
        )));
        peer.socket
            .write_all(&wire::encode_terminal_server(&event).unwrap())
            .unwrap();
        let mut byte = [0];
        assert_eq!(peer.socket.read(&mut byte).unwrap(), 0);
        host.no_request();
    });
    let mut lease = backend
        .open(terminal_open(), Duration::from_millis(150))
        .unwrap();
    let frame = lease
        .reader
        .next_idle(desktop::MAX_RESPONSE, Duration::from_millis(150))
        .unwrap()
        .unwrap();
    assert!(
        matches!(wire::decode_terminal_server(frame.body()).unwrap(), stream::TerminalServer::Event(event)
        if matches!(event.as_ref(), stream::TerminalEvent::Ready(_)))
    );
    drop(lease);
    task.join().unwrap();
}

#[test]
fn terminal_lease_rejects_failed_open_and_does_not_reconnect() {
    let host = Host::new();
    let backend = Backend::at_path(host.path.clone());
    let task = thread::spawn(move || {
        let mut peer = host.receive();
        peer.fail("terminal_unavailable", "Terminal is unavailable");
        drop(peer); // A final refusal is complete only with transport EOF.
        thread::sleep(Duration::from_millis(50));
        host.no_request();
    });
    let error = backend
        .open(terminal_open(), Duration::from_secs(1))
        .err()
        .expect("refused lease");
    assert_eq!(error.code(), "terminal_unavailable");
    assert_eq!(error.retry(), common::Retry::No);
    task.join().unwrap();
}

fn oid() -> git::ObjectId {
    git::ObjectId {
        algorithm: "sha1".into(),
        hex: "a".repeat(40),
    }
}
fn head() -> git::Head {
    git::Head {
        oid: Some(oid()),
        name: Some(git::Path::new(b"refs/heads/main")),
        detached: false,
        unborn: false,
    }
}
fn summary() -> git::StatusSummary {
    git::StatusSummary {
        head: head(),
        operation_state: "none".into(),
        integration: None,
        ahead: None,
        behind: None,
        basis: "local".into(),
        upstream_ref: None,
        upstream: None,
        truncated: false,
        total_entries: 0,
        group_counts: git::GroupCounts {
            staged: 0,
            unstaged: 0,
            untracked: 0,
            conflicted: 0,
        },
        matched_entries: None,
    }
}
fn page<T, M>(metadata: M) -> git::Page<T, M> {
    git::Page {
        snapshot: "snapshot".into(),
        entries: vec![],
        next_cursor: None,
        metadata,
    }
}
fn worktree_metadata(count: u64) -> git::WorktreeMetadata {
    git::WorktreeMetadata {
        list_token: "list-token".into(),
        total_entries: count,
        matching_entries: count,
        current: None,
        main: None,
    }
}
fn diff_request(working: bool) -> git::Request {
    if working {
        git::Request::DiffPage {
            repo_id: "repo".into(),
            snapshot: "status".into(),
            entry_id: "file-id".into(),
            side: git::Side::IndexToWorktree,
            context_lines: 3,
            page_size: 20,
            max_bytes: None,
            cursor: None,
            line_encoding: None,
        }
    } else {
        git::Request::CommitDiffPage {
            repo_id: "repo".into(),
            commit_oid: "a".repeat(40),
            path: git::Path::new(b"file"),
            parent_index: 0,
            context_lines: 3,
            page_size: 20,
            max_bytes: None,
            cursor: None,
            line_encoding: None,
        }
    }
}
fn diff_page(working: bool) -> git::Page<git::FileFragment, git::DiffMetadata> {
    // Reuse the existing UI diff fixture, lowered once to the actual shared DTO.
    // Only the protobuf encoder sends it; JSON is not a transport fixture.
    serde_json::from_value(crate::git_diff_page::tests::page(working)).unwrap()
}
fn page_cases() -> Vec<(git::Request, git::Reply)> {
    let mut cases = vec![
        (
            git::Request::Status {
                repo_id: "repo".into(),
                filter: None,
                page_size: 20,
                cursor: None,
            },
            git::Reply::Status(Box::new(page(summary()))),
        ),
        (
            git::Request::History {
                repo_id: "repo".into(),
                page_size: 20,
                cursor: None,
                revision: "HEAD".into(),
                message_bytes: None,
            },
            git::Reply::History(Box::new(page(git::HistoryMetadata {
                resolved_revision: Some(oid()),
                truncated: Some(false),
            }))),
        ),
        (
            git::Request::CommitFiles {
                repo_id: "repo".into(),
                commit_oid: "a".repeat(40),
                parent_index: 0,
                page_size: 20,
                cursor: None,
            },
            git::Reply::CommitFiles(Box::new(page(diff_page(false).metadata))),
        ),
        (
            git::Request::Branches {
                repo_id: "repo".into(),
                page_size: 20,
                cursor: None,
                filter: String::new(),
                branch_kind: None,
            },
            git::Reply::Branches(Box::new(page(git::TotalEntries { total_entries: 0 }))),
        ),
        (
            git::Request::RemoteNames {
                repo_id: "repo".into(),
                filter: String::new(),
                page_size: 20,
                cursor: None,
            },
            git::Reply::RemoteNames(Box::new(page(git::TotalEntries { total_entries: 0 }))),
        ),
        (
            git::Request::RemoteRefs {
                repo_id: "repo".into(),
                remote: "origin".into(),
                expected_token: "remote-token".into(),
                for_push: false,
                filter: String::new(),
                page_size: 20,
                cursor: None,
            },
            git::Reply::RemoteRefs(Box::new(page(git::RemoteReferenceMetadata {
                remote: "origin".into(),
                remote_token: "remote-token".into(),
                for_push: false,
                basis: "remote".into(),
                truncated: false,
                total_entries: 0,
            }))),
        ),
        (
            git::Request::Stashes {
                repo_id: "repo".into(),
                page_size: 20,
                cursor: None,
            },
            git::Reply::Stashes(Box::new(page(git::StashMetadata {
                list_token: "list-token".into(),
                total_entries: 0,
            }))),
        ),
        (
            git::Request::Tags {
                repo_id: "repo".into(),
                page_size: 20,
                cursor: None,
                message_bytes: None,
            },
            git::Reply::Tags(Box::new(page(git::TotalEntries { total_entries: 0 }))),
        ),
        (
            git::Request::Worktrees {
                repo_id: "repo".into(),
                page_size: 20,
                cursor: None,
                filter: String::new(),
                branch: None,
                name: None,
                at_snapshot: None,
            },
            git::Reply::Worktrees(Box::new(page(worktree_metadata(0)))),
        ),
        (
            git::Request::BlobPage {
                repo_id: "repo".into(),
                oid: "a".repeat(40),
                max_bytes: None,
                cursor: None,
            },
            git::Reply::BlobPage(Box::new(page(git::BlobMetadata {
                oid: oid(),
                size: 0,
            }))),
        ),
        (
            diff_request(true),
            git::Reply::DiffPage(Box::new(diff_page(true))),
        ),
    ];
    for parented in [false, true] {
        for encoding in [None, Some(git::DiffLineEncoding::TupleV1)] {
            let mut request = diff_request(false);
            let git::Request::CommitDiffPage { line_encoding, .. } = &mut request else {
                panic!("Expected commit diff request");
            };
            *line_encoding = encoding;
            let mut page = diff_page(false);
            if parented {
                let parent = git::ObjectId {
                    algorithm: "sha1".into(),
                    hex: "b".repeat(40),
                };
                page.metadata.parent_oid = Some(parent.clone());
                page.metadata.parent_index = Some(0);
                page.metadata.parents = Some(vec![parent]);
            }
            cases.push((request, git::Reply::CommitDiffPage(Box::new(page))));
        }
    }
    cases
}

#[test]
fn all_git_page_reads_validate_the_wire_envelope_before_publishing() {
    for (request, reply) in page_cases() {
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let expected_request = request.clone();
        let task = thread::spawn(move || {
            let mut peer = host.receive();
            assert_eq!(git_request(&peer), &expected_request);
            peer.git_reply(reply.clone());
            drop(peer);
            let mut peer = host.receive();
            assert_eq!(git_request(&peer), &expected_request);
            wrong_identity(&mut peer, git_result(reply));
        });
        let mut resources = crate::resources::Resources::default();
        let generation = resources.begin("page").unwrap();
        assert!(resources.complete_detailed(
            "page",
            generation,
            backend.request_git(server(), request.clone(), false)
        ));
        let accepted = resources.get("page").unwrap().value.clone();
        assert!(
            resources.get("page").unwrap().error.is_none(),
            "{}: {:?}",
            request.method(),
            resources.get("page").unwrap().error
        );
        let generation = resources.begin("page").unwrap();
        assert!(resources.complete_detailed(
            "page",
            generation,
            backend.request_git(server(), request.clone(), false)
        ));
        let failed = resources.get("page").unwrap();
        assert_eq!(failed.value, accepted, "{}", request.method());
        assert!(!failed.pending);
        assert_eq!(failed.error_code.as_deref(), Some("PROTOCOL_ERROR"));
        task.join().unwrap();
    }
}

#[test]
fn malformed_diff_content_keeps_the_accepted_page_and_ends_loading() {
    for working in [true, false] {
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let task = thread::spawn(move || {
            let mut peer = host.receive();
            let mut malformed = diff_page(working);
            // A typed but semantically impossible diff line must not become UI data.
            malformed.entries[0].hunks[0].lines[0].origin = "invalid-origin".into();
            peer.git_reply(if working {
                git::Reply::DiffPage(Box::new(malformed))
            } else {
                git::Reply::CommitDiffPage(Box::new(malformed))
            });
        });
        let mut resources = crate::resources::Resources::default();
        let generation = resources.begin("diff").unwrap();
        let accepted = crate::git_diff_page::tests::page(working);
        assert!(resources.complete_detailed("diff", generation, Ok(accepted.clone())));
        let generation = resources.begin("diff").unwrap();
        assert!(resources.complete_detailed(
            "diff",
            generation,
            backend.request_git(server(), diff_request(working), false)
        ));
        let failed = resources.get("diff").unwrap();
        assert_eq!(failed.value, accepted);
        assert!(!failed.pending);
        assert_eq!(failed.error_code.as_deref(), Some("PROTOCOL_ERROR"));
        task.join().unwrap();
    }
}

fn remotes() -> git::Remotes {
    git::Remotes {
        entries: vec![],
        authentication: git::RemoteAuthentication {
            ssh: "agent".into(),
            https: "none".into(),
        },
        configuration: git::RepositoryConfiguration {
            token: "configuration-token".into(),
            fetch_prune: false,
            user_name: None,
            user_email: None,
        },
    }
}

#[test]
fn git_envelope_validation_preserves_nonpage_replies_and_typed_errors() {
    let id = Uuid::new_v4();
    let cases = vec![
        (
            git::Request::Open {
                path: git::Path::new(b"/srv/repo"),
            },
            repository("repo", git::Path::new(b"/srv/repo")),
        ),
        (
            git::Request::StatusSummary {
                repo_id: Some("repo".into()),
                path: None,
            },
            git::Reply::StatusSummary(Box::new(summary())),
        ),
        (
            git::Request::Remotes {
                repo_id: "repo".into(),
            },
            git::Reply::Remotes(Box::new(remotes())),
        ),
        (
            git::Request::Get {
                operation_id: id.to_string(),
            },
            success(id),
        ),
    ];
    for (request, reply) in cases {
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let expected = serde_json::to_value(reply.projection(&request)).unwrap();
        let task = thread::spawn(move || {
            let mut peer = host.receive();
            peer.git_reply(reply);
        });
        assert_eq!(
            backend.request_git(server(), request, false).unwrap(),
            expected
        );
        task.join().unwrap();
    }
    // Collected diff is its own typed result, not a page or an arbitrary map.
    for working in [true, false] {
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let request = if working {
            git::Request::Diff {
                repo_id: "repo".into(),
                snapshot: "status".into(),
                entry_id: "file-id".into(),
                side: git::Side::IndexToWorktree,
                context_lines: 3,
            }
        } else {
            git::Request::CommitDiff {
                repo_id: "repo".into(),
                commit_oid: "a".repeat(40),
                path: Some(git::Path::new(b"file")),
                parent_index: 0,
                context_lines: 3,
            }
        };
        let metadata = diff_page(working).metadata;
        let diff = git::Diff {
            files: vec![],
            read_only: metadata.read_only,
            commit_oid: metadata.commit_oid,
            parent_oid: metadata.parent_oid,
            parent_index: metadata.parent_index,
            parents: metadata.parents,
            source_snapshot: metadata.source_snapshot,
            entry_id: metadata.entry_id,
            side: metadata.side,
            context_lines: metadata.context_lines,
            selected_path: metadata.selected_path,
            truncated: Some(false),
            total_files: Some(0),
            total_units: Some(0),
            has_omissions: Some(false),
        };
        let expected = json!({"snapshot":"diff-snapshot", "diff":diff});
        let task = thread::spawn(move || {
            let mut peer = host.receive();
            peer.reply(desktop::Reply::Git(Box::new(desktop::git::Reply::Request(
                Box::new(desktop::git::GitResult::Diff(Box::new(
                    desktop::git::CollectedDiff {
                        snapshot: "diff-snapshot".into(),
                        diff,
                    },
                ))),
            ))));
        });
        assert_eq!(
            backend.request_git(server(), request, false).unwrap(),
            expected
        );
        task.join().unwrap();
    }
    let host = Host::new();
    let backend = Backend::at_path(host.path.clone());
    let task = thread::spawn(move || {
        let mut peer = host.receive();
        peer.fail("STALE_SNAPSHOT", "Refresh the repository");
    });
    let error = backend
        .request_git(server(), diff_request(true), false)
        .unwrap_err();
    assert_eq!(error.code.as_deref(), Some("STALE_SNAPSHOT"));
    assert_eq!(error.message, "Refresh the repository");
    task.join().unwrap();
}

#[test]
fn repository_settings_invalid_response_is_retryable_and_keeps_protocol_errors() {
    let host = Host::new();
    let backend = Backend::at_path(host.path.clone());
    let valid = git::Reply::Remotes(Box::new(remotes()));
    let expected = serde_json::to_value(valid.projection(&git::Request::Remotes {
        repo_id: "repo".into(),
    }))
    .unwrap();
    let task = thread::spawn(move || {
        for step in 0..3 {
            let mut peer = host.receive();
            assert_eq!(
                git_request(&peer),
                &git::Request::Remotes {
                    repo_id: "repo".into()
                }
            );
            match step {
                0 => {
                    let mut invalid = remotes();
                    invalid.configuration.token.clear();
                    peer.git_reply(git::Reply::Remotes(Box::new(invalid)));
                }
                1 => peer.git_reply(valid.clone()),
                _ => peer.fail("REPOSITORY_CLOSED", "Open the repository again."),
            }
        }
    });
    let mut resources = crate::resources::Resources::default();
    let generation = resources.begin("settings").unwrap();
    assert!(resources.complete_detailed(
        "settings",
        generation,
        backend.repository_settings(SERVER, "repo")
    ));
    let failed = resources.get("settings").unwrap();
    assert!(!failed.pending);
    assert!(failed.value.is_null());
    assert!(failed.error.as_ref().unwrap().contains("Retry settings"));
    let generation = resources.begin("settings").unwrap();
    assert!(resources.complete_detailed(
        "settings",
        generation,
        backend.repository_settings(SERVER, "repo")
    ));
    let ready = resources.get("settings").unwrap();
    assert_eq!(ready.value, expected);
    assert!(ready.error.is_none());
    let error = backend.repository_settings(SERVER, "repo").unwrap_err();
    assert_eq!(error.code.as_deref(), Some("REPOSITORY_CLOSED"));
    task.join().unwrap();
}

#[test]
fn detailed_reads_preserve_protocol_codes_without_parsing_messages() {
    for (code, message) in [
        ("STALE_SNAPSHOT", "The repository changed"),
        ("operation", "STALE_SNAPSHOT: ordinary error text"),
    ] {
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let worker = thread::spawn(move || {
            let mut peer = host.receive();
            assert_eq!(git_request(&peer), &diff_request(true));
            peer.fail(code, message);
            drop(peer);
            thread::sleep(Duration::from_millis(50));
            host.no_request();
        });
        let error = backend
            .request_git(server(), diff_request(true), false)
            .unwrap_err();
        assert_eq!(error.code.as_deref(), Some(code));
        assert_eq!(error.message, message);
        worker.join().unwrap();
    }
}

#[test]
fn consent_wait_outlives_request_deadline_without_replaying() {
    let host = Host::new();
    let backend = Backend::at_path(host.path.clone());
    let task = thread::spawn(move || {
        let mut peer = host.receive();
        assert!(
            matches!(&peer.request.operation, desktop::Operation::Connection(operation)
            if matches!(operation.as_ref(), connection::Operation::TestConnection(args) if args.server == server()))
        );
        approval(&mut peer, true);
        thread::sleep(Duration::from_millis(350));
        approval(&mut peer, false);
        peer.reply(connected());
        drop(peer);
        thread::sleep(Duration::from_millis(50));
        host.no_request();
    });
    assert!(
        matches!(backend.execute_with_timeout(connection_test(), Duration::from_millis(150)).unwrap(), desktop::Reply::Connection(reply)
        if matches!(reply.as_ref(), connection::Reply::TestConnection(_)))
    );
    task.join().unwrap();
}

#[test]
fn network_deadline_resumes_when_consent_finishes() {
    let host = Host::new();
    let backend = Backend::at_path(host.path.clone());
    let task = thread::spawn(move || {
        let mut peer = host.receive();
        approval(&mut peer, true);
        thread::sleep(Duration::from_millis(350));
        approval(&mut peer, false);
        thread::sleep(Duration::from_millis(350));
        drop(peer);
        host.no_request();
    });
    let started = Instant::now();
    let error = backend
        .execute_with_timeout(connection_test(), Duration::from_millis(150))
        .unwrap_err();
    assert_eq!(error.phase, Phase::Response);
    assert!(error.dispatched);
    assert!(matches!(error.cause, Cause::Io(ref error)
        if matches!(error.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock)));
    assert!(started.elapsed() >= Duration::from_millis(450));
    task.join().unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn closed_peer_keeps_buffered_final_frame_after_timeout_update() {
    let host = Host::new();
    let backend = Backend::at_path(host.path.clone());
    let task = thread::spawn(move || {
        let mut peer = host.receive();
        approval(&mut peer, true);
        let progress = desktop::ServerMessage::Progress(Box::new(desktop::Progress {
            id: peer.request.id,
            state: desktop::ProgressState::AgentApproval(Box::new(desktop::AgentApproval {
                server: server(),
                awaiting: false,
            })),
        }));
        let mut frames = wire::encode_server_for(&progress, &peer.request).unwrap();
        frames.extend(reply_frame(&peer, connected()));
        // One write can place both frames in the same Darwin read buffer, even
        // though the resumed operation deadline is installed between frames.
        peer.socket.write_all(&frames).unwrap();
    });
    assert!(
        backend
            .execute_with_timeout(connection_test(), Duration::from_secs(1))
            .is_ok()
    );
    task.join().unwrap();
}

#[test]
fn opened_stream_retains_a_coalesced_first_terminal_frame() {
    let host = Host::new();
    let backend = Backend::at_path(host.path.clone());
    let task = thread::spawn(move || {
        let mut peer = host.receive();
        let message = desktop::ServerMessage::Response(Box::new(desktop::Response {
            id: peer.request.id,
            outcome: desktop::Outcome::Opened(Box::new(stream::Opened::Terminal(Box::new(
                stream::TerminalOpened {
                    version: desktop::VERSION,
                    session: Uuid::from_u128(2),
                },
            )))),
        }));
        let mut bytes = wire::encode_server_for(&message, &peer.request).unwrap();
        bytes.extend(
            wire::encode_terminal_server(&stream::TerminalServer::Event(Box::new(
                stream::TerminalEvent::Ready(Box::new(common::Present {})),
            )))
            .unwrap(),
        );
        peer.socket.write_all(&bytes).unwrap();
    });
    let mut lease = backend
        .open(terminal_open(), Duration::from_secs(1))
        .unwrap();
    let frame = lease
        .reader
        .next_until(
            desktop::TERMINAL_SERVER_LIMITS.message,
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap()
        .unwrap();
    assert!(
        matches!(wire::decode_terminal_server(frame.body()).unwrap(), stream::TerminalServer::Event(event)
        if matches!(event.as_ref(), stream::TerminalEvent::Ready(_)))
    );
    task.join().unwrap();
}

#[test]
fn reply_failures_preserve_closed_or_timed_out_transport_causes() {
    for closed in [true, false] {
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let task = thread::spawn(move || {
            let peer = host.receive();
            if !closed {
                thread::sleep(Duration::from_millis(350));
            }
            drop(peer);
            thread::sleep(Duration::from_millis(50));
            host.no_request();
        });
        let error = backend
            .execute_with_timeout(connection_test(), Duration::from_millis(150))
            .unwrap_err();
        assert_eq!(error.phase, Phase::Response);
        assert!(error.dispatched);
        if closed {
            assert!(matches!(error.cause, Cause::Wire(wire::Error::Ended)));
        } else {
            assert!(matches!(error.cause, Cause::Io(ref error)
            if matches!(error.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock)));
        }
        task.join().unwrap();
    }
}

#[test]
fn closed_peer_with_partial_reply_remains_an_error() {
    for partial_prefix in [true, false] {
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let task = thread::spawn(move || {
            let mut peer = host.receive();
            if partial_prefix {
                peer.socket.write_all(&[0x80]).unwrap();
            } else {
                let frame = reply_frame(&peer, connected());
                peer.socket.write_all(&frame[..frame.len() - 1]).unwrap();
            }
        });
        let error = backend
            .execute_with_timeout(connection_test(), Duration::from_secs(1))
            .unwrap_err();
        assert_eq!(error.phase, Phase::Response);
        assert!(
            matches!(error.cause, Cause::Io(ref error) if error.kind() == std::io::ErrorKind::UnexpectedEof)
        );
        task.join().unwrap();
    }
}

#[test]
fn a_complete_reply_requires_eof_and_rejects_trailing_frames() {
    let host = Host::new();
    let backend = Backend::at_path(host.path.clone());
    let task = thread::spawn(move || {
        let mut peer = host.receive();
        let frame = reply_frame(&peer, connected());
        peer.socket.write_all(&frame).unwrap();
        peer.socket.write_all(&frame).unwrap();
    });
    let error = backend
        .execute_with_timeout(connection_test(), Duration::from_secs(1))
        .unwrap_err();
    assert_eq!(error.phase, Phase::Closure);
    assert_eq!(error.code(), "PROTOCOL_ERROR");
    task.join().unwrap();
}

#[test]
fn a_complete_reply_without_transport_eof_times_out_without_publication() {
    let host = Host::new();
    let backend = Backend::at_path(host.path.clone());
    let task = thread::spawn(move || {
        let mut peer = host.receive();
        peer.reply(connected());
        thread::sleep(Duration::from_millis(350));
        drop(peer);
        host.no_request();
    });
    let error = backend
        .execute_with_timeout(connection_test(), Duration::from_millis(150))
        .unwrap_err();
    assert_eq!(error.phase, Phase::Closure);
    assert!(
        matches!(error.cause, Cause::Io(ref error) if error.kind() == std::io::ErrorKind::TimedOut)
    );
    task.join().unwrap();
}

#[test]
fn interrupted_metrics_read_has_no_write_warning_and_is_not_replayed() {
    let host = Host::new();
    let backend = Backend::at_path(host.path.clone());
    let task = thread::spawn(move || {
        let peer = host.receive();
        assert!(
            matches!(&peer.request.operation, desktop::Operation::Metrics(operation)
            if matches!(operation.as_ref(), metrics::Operation::History(args) if args.server == server() && args.minutes == Some(60)))
        );
        drop(peer);
        thread::sleep(Duration::from_millis(100));
        host.no_request();
    });
    let operation = desktop::Operation::Metrics(Box::new(metrics::Operation::History(Box::new(
        metrics::HistoryArgs {
            server: server(),
            minutes: Some(60),
        },
    ))));
    let error = backend
        .execute_with_timeout(operation, Duration::from_secs(1))
        .unwrap_err();
    assert_eq!(error.phase, Phase::Response);
    assert!(error.dispatched);
    assert!(!error.to_string().contains("write"));
    assert!(!error.to_string().contains("outcome"));
    task.join().unwrap();
}

#[test]
fn remote_picker_reads_only_one_names_page_and_selected_configuration() {
    let host = Host::new();
    let backend = Backend::at_path(host.path.clone());
    let worker = thread::spawn(move || {
        let mut peer = host.receive();
        assert_eq!(
            git_request(&peer),
            &git::Request::RemoteNames {
                repo_id: "repo".into(),
                filter: "backup".into(),
                page_size: 20,
                cursor: Some("next".into())
            }
        );
        let mut names = page(git::TotalEntries { total_entries: 21 });
        names.entries.push(git::RemoteName {
            name: "backup-21".into(),
        });
        peer.git_reply(git::Reply::RemoteNames(Box::new(names)));
        drop(peer);
        let mut peer = host.receive();
        assert_eq!(
            git_request(&peer),
            &git::Request::Remote {
                repo_id: "repo".into(),
                name: "backup-21".into()
            }
        );
        peer.git_reply(git::Reply::Remote(Box::new(git::Remote {
            name: "backup-21".into(),
            token: "reviewed".into(),
            url: Some("/fetch".into()),
            push_url: Some("/push".into()),
        })));
        drop(peer);
        thread::sleep(Duration::from_millis(50));
        host.no_request();
    });
    let names = backend
        .git_remote_names(SERVER, "repo", "backup", Some("next"))
        .unwrap();
    assert_eq!(names.entries, ["backup-21"]);
    assert_eq!(
        backend.git_remote(SERVER, "repo", "backup-21").unwrap()["pushUrl"],
        "/push"
    );
    worker.join().unwrap();
}

#[test]
fn worktree_branch_reads_are_bounded_filtered_and_ownership_is_exact() {
    let reference = "refs/heads/topic";
    for owner in [None, Some(reference), Some("refs/heads/wrong")] {
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let task = thread::spawn(move || {
            let mut peer = host.receive();
            assert_eq!(
                git_request(&peer),
                &git::Request::Branches {
                    repo_id: "repo".into(),
                    branch_kind: Some("local".into()),
                    page_size: 20,
                    filter: "topic".into(),
                    cursor: Some("next-page".into())
                }
            );
            peer.git_reply(git::Reply::Branches(Box::new(page(git::TotalEntries {
                total_entries: 0,
            }))));
            drop(peer);
            let mut peer = host.receive();
            assert_eq!(
                git_request(&peer),
                &git::Request::Worktrees {
                    repo_id: "repo".into(),
                    branch: Some(reference.into()),
                    page_size: 1,
                    cursor: None,
                    filter: String::new(),
                    name: None,
                    at_snapshot: None
                }
            );
            let mut value = page(worktree_metadata(u64::from(owner.is_some())));
            if let Some(branch) = owner {
                let mut head = head();
                head.name = Some(git::Path::new(branch.as_bytes()));
                value.entries.push(git::Worktree {
                    name: Some(git::Path::new(b"topic")),
                    kind: "linked".into(),
                    path: git::Path::new(b"/srv/occupied"),
                    git_dir: git::Path::new(b"/srv/repo/.git/worktrees/topic"),
                    current: false,
                    state: "available".into(),
                    head,
                    locked: false,
                    lock_reason: None,
                    prunable: false,
                });
            }
            peer.git_reply(git::Reply::Worktrees(Box::new(value)));
            drop(peer);
            thread::sleep(Duration::from_millis(50));
            host.no_request();
        });
        backend
            .worktree_branches(SERVER, "repo", "topic", Some("next-page"))
            .unwrap();
        let result = backend.worktree_branch_owner(
            SERVER,
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
        let mut peer = host.receive();
        assert_eq!(
            git_request(&peer),
            &git::Request::Branches {
                repo_id: "repo".into(),
                page_size: 20,
                filter: "origin/topic".into(),
                cursor: Some("next".into()),
                branch_kind: None
            }
        );
        peer.git_reply(git::Reply::Branches(Box::new(page(git::TotalEntries {
            total_entries: 0,
        }))));
    });
    backend
        .worktree_branch_choices(
            SERVER,
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
        let mut peer = host.receive();
        assert_eq!(
            git_request(&peer),
            &git::Request::Branches {
                repo_id: "repo".into(),
                page_size: 20,
                filter: "topic".into(),
                branch_kind: Some("local".into()),
                cursor: None
            }
        );
        peer.git_reply(git::Reply::Branches(Box::new(page(git::TotalEntries {
            total_entries: 0,
        }))));
    });
    backend
        .check_worktree_branch(SERVER, "repo", "topic")
        .unwrap();
    task.join().unwrap();
}

#[test]
fn worktree_name_precheck_is_an_exact_single_row_read() {
    let host = Host::new();
    let backend = Backend::at_path(host.path.clone());
    let task = thread::spawn(move || {
        let mut peer = host.receive();
        assert_eq!(
            git_request(&peer),
            &git::Request::Worktrees {
                repo_id: "repo".into(),
                page_size: 1,
                name: Some("topic".into()),
                cursor: None,
                filter: String::new(),
                branch: None,
                at_snapshot: None
            }
        );
        peer.git_reply(git::Reply::Worktrees(Box::new(page(worktree_metadata(0)))));
    });
    backend
        .check_worktree_name(SERVER, "repo", "topic")
        .unwrap();
    task.join().unwrap();
}
