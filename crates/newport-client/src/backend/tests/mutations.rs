use super::*;

fn asserted_action(peer: &Peer, id: Uuid, snapshot: &str) -> git::Action {
    let git::Request::Start {
        operation_id,
        repo_id,
        expected_snapshot,
        action,
    } = git_request(peer)
    else {
        panic!("expected one mutation");
    };
    assert_eq!(operation_id, &id.to_string());
    assert_eq!(repo_id, "repo");
    assert_eq!(expected_snapshot, snapshot);
    action.clone()
}
fn apply_once(action: git::Action, snapshot: &str) {
    let host = Host::new();
    let backend = Backend::at_path(host.path.clone());
    let id = Uuid::new_v4();
    let expected = action.clone();
    let captured = snapshot.to_owned();
    let task = thread::spawn(move || {
        pending(&host);
        let mut peer = host.receive();
        assert_eq!(asserted_action(&peer, id, &captured), expected);
        peer.git_reply(success(id));
        drop(peer);
        host.no_request();
    });
    assert_eq!(
        backend
            .apply_reviewed_action(SERVER, "repo", snapshot, action, id)
            .unwrap()
            .state,
        git::OperationState::Succeeded
    );
    task.join().unwrap();
}
#[test]
fn partial_index_write_keeps_the_reviewed_ids_and_snapshot() {
    let mut page = crate::git_diff_page::tests::page(true);
    page.next_cursor = Some("more".into());
    page.metadata.source_snapshot = Some("reviewed".into());
    page.metadata.entry_id = Some("file".into());
    let hunk = &mut page.entries[0].hunks[0];
    hunk.id = Some("hunk".into());
    hunk.total_lines = 2;
    hunk.lines[0].id = Some("line".into());
    let review = crate::git_selection::Review::from_page(&page).unwrap();
    let host = Host::new();
    let backend = Backend::at_path(host.path.clone());
    let id = Uuid::new_v4();
    let task = thread::spawn(move || {
        pending(&host);
        let mut peer = host.receive();
        assert_eq!(
            asserted_action(&peer, id, "reviewed"),
            git::Action::Stage {
                entry_ids: vec!["file".into()],
                hunks: Some(git::HunkSelection {
                    ids: vec!["hunk".into()],
                    lines: Some(vec!["line".into()]),
                    context_lines: 3
                })
            }
        );
        peer.git_reply(success(id));
    });
    backend
        .change_index_selection(
            SERVER,
            "repo",
            &review,
            &["hunk".into()],
            Some(&["line".into()]),
            id,
        )
        .unwrap();
    task.join().unwrap();
    let host = Host::new();
    let backend = Backend::at_path(host.path.clone());
    assert!(
        !backend
            .change_index_selection(SERVER, "repo", &review, &["stale-hunk".into()], None, id)
            .unwrap_err()
            .dispatched
    );
    host.no_request();
}
#[test]
fn discard_dispatches_once_with_the_reviewed_snapshot_and_restore_source() {
    for source in [
        crate::git_discard::Source::Index,
        crate::git_discard::Source::Head,
    ] {
        let entry = git::StatusEntry {
            entry_id: "e".into(),
            path: git::Path::new(b"file"),
            old_path: None,
            flags: 0,
            staged: false,
            unstaged: true,
            untracked: false,
            conflicted: false,
            conflict: None,
        };
        let status = git::Page {
            snapshot: "reviewed".into(),
            entries: vec![entry.clone()],
            next_cursor: None,
            metadata: git::StatusSummary {
                head: git::Head {
                    oid: None,
                    name: None,
                    detached: false,
                    unborn: true,
                },
                operation_state: "none".into(),
                integration: None,
                ahead: None,
                behind: None,
                basis: "local".into(),
                upstream_ref: None,
                upstream: None,
                truncated: false,
                total_entries: 1,
                group_counts: git::GroupCounts {
                    staged: 0,
                    unstaged: 1,
                    untracked: 0,
                    conflicted: 0,
                },
                matched_entries: None,
            },
        };
        let review = crate::git_discard::Review::file(Some(&status), Some(&entry)).unwrap();
        let expected = review.action(source).unwrap();
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let id = Uuid::new_v4();
        let task = thread::spawn(move || {
            pending(&host);
            let mut peer = host.receive();
            assert_eq!(asserted_action(&peer, id, "reviewed"), expected);
            peer.git_reply(success(id));
            drop(peer);
            host.no_request();
        });
        backend
            .discard_changes(SERVER, "repo", &review, source, id)
            .unwrap();
        task.join().unwrap();
    }
}
#[test]
fn index_group_uses_one_operation_and_the_reviewed_snapshot() {
    for stage in [true, false] {
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let id = Uuid::new_v4();
        let task = thread::spawn(move || {
            pending(&host);
            let mut peer = host.receive();
            let ids = vec!["opaque-a".into(), "opaque-b".into()];
            assert_eq!(
                asserted_action(&peer, id, "reviewed"),
                if stage {
                    git::Action::Stage {
                        entry_ids: ids,
                        hunks: None,
                    }
                } else {
                    git::Action::Unstage {
                        entry_ids: ids,
                        hunks: None,
                    }
                }
            );
            peer.git_reply(success(id));
        });
        assert_eq!(
            backend
                .change_index_entries(
                    SERVER,
                    "repo",
                    "reviewed",
                    &["opaque-a".into(), "opaque-b".into()],
                    stage,
                    id
                )
                .unwrap()
                .operation_id,
            id.to_string()
        );
        task.join().unwrap();
    }
}
#[test]
fn empty_and_oversized_index_groups_fail_before_any_transport_or_receipt() {
    let host = Host::new();
    let backend = Backend::at_path(host.path.clone());
    for entries in [
        vec![],
        vec![String::new()],
        vec!["id".to_owned(); 10_001],
        vec!["a".repeat(600_000), "b".repeat(600_000)],
        vec!["a".repeat(git::MAX_TEXT + 1)],
    ] {
        assert!(
            !backend
                .change_index_entries(SERVER, "repo", "reviewed", &entries, true, Uuid::new_v4())
                .unwrap_err()
                .dispatched
        );
        host.no_request();
    }
}
#[test]
fn interrupted_creation_is_not_replayed_and_recovers_by_same_operation_id() {
    let host = Host::new();
    let backend = Backend::at_path(host.path.clone());
    let id = Uuid::new_v4();
    let task = thread::spawn(move || {
        pending(&host);
        let peer = host.receive();
        assert_eq!(
            git_request(&peer),
            &git::Request::Clone {
                operation_id: id.to_string(),
                url: "/origin.git".into(),
                path: git::Path::new(b"/new-repo"),
                branch: None,
                bare: false
            }
        );
        drop(peer);
        let mut peer = host.receive();
        assert_eq!(
            git_request(&peer),
            &git::Request::Get {
                operation_id: id.to_string()
            }
        );
        peer.git_reply(success(id));
    });
    let error = backend
        .bootstrap(
            SERVER,
            RepositoryCreation::Clone {
                url: "/origin.git".into(),
                path: git::Path::new(b"/new-repo"),
                branch: None,
                bare: false,
            },
            id,
        )
        .unwrap_err();
    assert!(error.dispatched);
    assert!(error.message.contains("not retried"));
    assert_eq!(
        backend.git_operation(SERVER, id).unwrap().operation_id,
        id.to_string()
    );
    task.join().unwrap();
}
#[test]
fn failed_preflight_never_dispatches_creation_or_reviewed_action() {
    for creation in [true, false] {
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let task = thread::spawn(move || {
            let mut peer = host.receive();
            assert!(
                matches!(&peer.request.operation,desktop::Operation::Git(value) if matches!(value.as_ref(),desktop::git::Operation::PendingOperations(_)))
            );
            peer.fail("STORAGE_UNAVAILABLE", "Receipt storage unavailable");
            drop(peer);
            thread::sleep(Duration::from_millis(20));
            host.no_request();
        });
        let id = Uuid::new_v4();
        let error = if creation {
            backend.bootstrap(
                SERVER,
                RepositoryCreation::Init {
                    path: git::Path::new(b"/repo"),
                    initial_branch: "main".into(),
                },
                id,
            )
        } else {
            backend.apply_reviewed_action(
                SERVER,
                "repo",
                "snapshot",
                git::Action::BranchSetUpstream {
                    name: "topic".into(),
                    expected_oid: "a".repeat(40),
                    expected_token: "tracking".into(),
                    upstream: None,
                },
                id,
            )
        }
        .unwrap_err();
        assert!(!error.dispatched);
        assert!(error.message.contains("Receipt storage unavailable"));
        task.join().unwrap();
    }
}
#[test]
fn branch_action_disconnect_is_not_replayed_and_recovers_original_operation_id() {
    let host = Host::new();
    let backend = Backend::at_path(host.path.clone());
    let id = Uuid::new_v4();
    let action = git::Action::BranchDelete {
        name: "topic".into(),
        expected_oid: "a".repeat(40),
        force: false,
    };
    let expected = action.clone();
    let task = thread::spawn(move || {
        pending(&host);
        let peer = host.receive();
        assert_eq!(asserted_action(&peer, id, "snapshot"), expected);
        drop(peer);
        let mut peer = host.receive();
        assert_eq!(
            git_request(&peer),
            &git::Request::Get {
                operation_id: id.to_string()
            }
        );
        peer.git_reply(success(id));
    });
    let error = backend
        .apply_reviewed_action(SERVER, "repo", "snapshot", action, id)
        .unwrap_err();
    assert!(error.dispatched);
    assert!(error.message.contains("not retried"));
    assert_eq!(
        backend.git_operation(SERVER, id).unwrap().state,
        git::OperationState::Succeeded
    );
    task.join().unwrap();
}
#[test]
fn malformed_creation_outcome_remains_uncertain() {
    let host = Host::new();
    let backend = Backend::at_path(host.path.clone());
    let task = thread::spawn(move || {
        pending(&host);
        let mut peer = host.receive();
        // Bypass the encoder's request correlation deliberately to exercise the receiver.
        let reply = desktop::Reply::Git(Box::new(desktop::git::Reply::Request(Box::new(
            desktop::git::GitResult::Reply(Box::new(success(Uuid::new_v4()))),
        ))));
        let message = desktop::ServerMessage::Response(Box::new(desktop::Response {
            id: peer.request.id,
            outcome: desktop::Outcome::Completed(Box::new(reply)),
        }));
        peer.socket
            .write_all(&wire::encode_server(&message).unwrap())
            .unwrap();
    });
    assert!(
        backend
            .bootstrap(
                SERVER,
                RepositoryCreation::Init {
                    path: git::Path::new(b"/repo"),
                    initial_branch: "main".into()
                },
                Uuid::new_v4()
            )
            .unwrap_err()
            .dispatched
    );
    task.join().unwrap();
}
#[test]
fn reviewed_branch_actions_preserve_snapshot_oid_token_and_explicit_force() {
    let entry = |name: &str, remote: bool| git::Branch {
        name: git::Path::new(name.as_bytes()),
        reference: git::Path::new(
            format!("refs/{}/{name}", if remote { "remotes" } else { "heads" }).as_bytes(),
        ),
        oid: git::ObjectId {
            algorithm: "sha1".into(),
            hex: "a".repeat(40),
        },
        remote,
        current: false,
        upstream: None,
        tracking: Some(git::Tracking {
            token: "reviewed-tracking".into(),
            editable: true,
            configuration: git::TrackingConfiguration {
                remote: vec![],
                merge: vec![],
            },
        }),
    };
    let review = crate::git_branch::BranchReview::from_entry(
        "repo",
        "reviewed-snapshot",
        &entry("topic", false),
    )
    .unwrap();
    for action in [
        review.delete_action(false).unwrap(),
        review.delete_action(true).unwrap(),
        review
            .set_upstream_action(Some(&entry("origin/main", true)))
            .unwrap(),
        review
            .set_upstream_action(Some(&entry("origin/main", false)))
            .unwrap(),
        review.set_upstream_action(None).unwrap(),
    ] {
        apply_once(action, "reviewed-snapshot");
    }
}
#[test]
fn tag_push_sends_exact_reviewed_identity_once() {
    apply_once(
        git::Action::TagPush {
            name: "v1".into(),
            expected_oid: "b".repeat(40),
            remote: "backup".into(),
            expected_token: "reviewed-config".into(),
        },
        "snapshot",
    );
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
        apply_once(
            target
                .action(
                    kind,
                    &git::Remote {
                        name: "origin".into(),
                        url: None,
                        push_url: None,
                        token: "reviewed-config".into(),
                    },
                )
                .unwrap(),
            &target.snapshot,
        );
    }
}
#[test]
fn worktree_creation_preserves_reviewed_identity_mode_and_lock() {
    for new_branch in [false, true] {
        let host = Host::new();
        let backend = Backend::at_path(host.path.clone());
        let id = Uuid::new_v4();
        let task = thread::spawn(move || {
            pending(&host);
            let mut peer = host.receive();
            assert_eq!(
                asserted_action(&peer, id, "reviewed-worktrees"),
                git::Action::WorktreeAdd {
                    name: "named-checkout".into(),
                    path: git::Path::new(b"/srv/separate directory"),
                    branch: "topic".into(),
                    expected_oid: "a".repeat(40),
                    new_branch,
                    locked: true
                }
            );
            peer.git_reply(success(id));
            drop(peer);
            host.no_request();
        });
        assert_eq!(
            backend
                .create_worktree(
                    SERVER,
                    "repo",
                    "reviewed-worktrees",
                    crate::git_worktree::CreateTarget {
                        name: "named-checkout",
                        path: "/srv/separate directory",
                        branch: "topic",
                        oid: &"a".repeat(40),
                        new_branch,
                        locked: true
                    },
                    id
                )
                .unwrap()
                .operation_id,
            id.to_string()
        );
        task.join().unwrap();
    }
}
#[test]
fn invalid_worktree_management_is_rejected_before_any_backend_call() {
    let host = Host::new();
    let backend = Backend::at_path(host.path.clone());
    let id = Uuid::new_v4();
    let error = backend
        .repair_worktree(SERVER, "repo", "snapshot", ("topic", "/srv/../other"), id)
        .unwrap_err();
    assert!(!error.dispatched);
    assert!(error.message.contains("absolute path"));
    let error = backend
        .remove_worktree(SERVER, "repo", "snapshot", ("../other", true), id)
        .unwrap_err();
    assert!(!error.dispatched);
    assert!(error.message.contains("exact name"));
    let error = backend
        .lock_worktree(
            SERVER,
            "repo",
            "snapshot",
            ("topic", &"a".repeat(4097), false),
            id,
        )
        .unwrap_err();
    assert!(!error.dispatched);
    assert!(error.message.contains("4096 bytes"));
    host.no_request();
}
fn close_handle(host: &Host, repo: &str) {
    let mut peer = host.receive();
    assert_eq!(
        git_request(&peer),
        &git::Request::Close {
            repo_id: repo.into()
        }
    );
    peer.git_reply(git::Reply::Close(Box::new(git::Closed { closed: true })));
}
#[test]
fn failed_bookmark_save_releases_open_handle_and_preserves_canonical_byte_path() {
    use desktop::preferences as prefs;
    let host = Host::new();
    let backend = Backend::at_path(host.path.clone());
    let canonical = git::Path::new(b"/canonical\xff");
    let task = thread::spawn(move || {
        let mut peer = host.receive();
        assert_eq!(
            git_request(&peer),
            &git::Request::Open {
                path: git::Path::new(b"/alias")
            }
        );
        peer.git_reply(repository("new-handle", canonical.clone()));
        drop(peer);
        let mut peer = host.receive();
        assert!(
            matches!(&peer.request.operation,desktop::Operation::Preferences(op) if matches!(op.as_ref(),prefs::Operation::ProjectsList(args) if args.server==server()))
        );
        peer.reply(desktop::Reply::Preferences(Box::new(
            prefs::Reply::ProjectsList(Box::new(prefs::Projects { projects: vec![] })),
        )));
        drop(peer);
        let mut peer = host.receive();
        assert!(
            matches!(&peer.request.operation,desktop::Operation::Preferences(op) if matches!(op.as_ref(),prefs::Operation::ProjectsSave(args) if args.project.path==canonical))
        );
        peer.fail("STORAGE_UNAVAILABLE", "Disk full");
        drop(peer);
        close_handle(&host, "new-handle");
    });
    assert_eq!(
        backend
            .add_project(server(), "Project", git::Path::new(b"/alias"))
            .unwrap_err(),
        "Disk full"
    );
    task.join().unwrap();
}
#[test]
fn bookmark_retry_after_a_lost_save_reply_reuses_the_saved_project_without_git_creation() {
    use desktop::preferences as prefs;
    let host = Host::new();
    let backend = Backend::at_path(host.path.clone());
    let canonical = git::Path::new(b"/canonical\xff");
    let path = canonical.clone();
    let saved = prefs::Project {
        id: Uuid::new_v4(),
        server: server(),
        name: "Project".into(),
        path: canonical.clone(),
    };
    let expected = saved.clone();
    let task = thread::spawn(move || {
        for attempt in 0..2 {
            let mut peer = host.receive();
            assert_eq!(
                git_request(&peer),
                &git::Request::Open {
                    path: canonical.clone()
                }
            );
            peer.git_reply(repository("handle", canonical.clone()));
            drop(peer);
            let mut peer = host.receive();
            assert!(
                matches!(&peer.request.operation,desktop::Operation::Preferences(op) if matches!(op.as_ref(),prefs::Operation::ProjectsList(_)))
            );
            peer.reply(desktop::Reply::Preferences(Box::new(
                prefs::Reply::ProjectsList(Box::new(prefs::Projects {
                    projects: if attempt == 0 {
                        vec![]
                    } else {
                        vec![saved.clone()]
                    },
                })),
            )));
            drop(peer);
            if attempt == 0 {
                let peer = host.receive();
                assert!(
                    matches!(&peer.request.operation,desktop::Operation::Preferences(op) if matches!(op.as_ref(),prefs::Operation::ProjectsSave(args) if args.project.path==canonical))
                );
                drop(peer);
            }
            close_handle(&host, "handle");
        }
    });
    assert!(
        backend
            .add_project(server(), "Project", path.clone())
            .is_err()
    );
    assert_eq!(
        backend.add_project(server(), "Project", path).unwrap(),
        expected
    );
    task.join().unwrap();
}
#[test]
fn terminal_link_opener_rejects_unsafe_schemes_and_dispatches_exact_url_once() {
    let host = Host::new();
    let backend = Backend::at_path(host.path.clone());
    let terminal = Uuid::new_v4();
    assert!(
        backend
            .open_terminal_link(terminal, "file:///etc/passwd")
            .is_err()
    );
    host.no_request();
    let task = thread::spawn(move || {
        let mut peer = host.receive();
        assert!(
            matches!(&peer.request.operation,desktop::Operation::Configuration(op) if matches!(op.as_ref(),desktop::configuration::Operation::OpenTerminalLink(args) if args.terminal==terminal && args.url=="https://example.org/服务?q=1"))
        );
        peer.reply(desktop::Reply::Configuration(Box::new(
            desktop::configuration::Reply::OpenTerminalLink(Box::new(common::Present {})),
        )));
        drop(peer);
        host.no_request();
    });
    backend
        .open_terminal_link(terminal, "HTTPS://example.org/服务?q=1")
        .unwrap();
    task.join().unwrap();
}
