//! Exercise actual snapshot completion/reset/reopen ordering without a backend.
use crate::connections::fixtures::{self as discovery_fixture, Listener};
use crate::project_library_state::typed_fixture::*;
use crate::snapshot_fixture::{self as fixture, NEXT_INSTANCE, SERVER};
use crate::{Completion, Page};
use gpui_kit::TestAppContext;

#[gpui_kit::test]
fn connection_project_metadata_refreshes_independently_and_respects_retry_interval(
    cx: &mut TestAppContext,
) {
    use std::time::{Duration, Instant};
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        crate::theme::install(false, cx);
    });
    let (owner, cx) = cx.add_window_view(crate::project_library_state::typed_test_owner);
    owner.update(cx, |this, _| {
        let (work, requests) = std::sync::mpsc::sync_channel(32);
        this.work = work.into();
        this.page = Page::Ports;
        discovery_fixture::put(this, discovery_fixture::observation([]));
        let data = vec![bookmark(1, TYPED_SERVER, "Original", "/srv/app")];
        let due = Instant::now() + Duration::from_secs(61);
        this.refresh_connection_projects(Instant::now());
        let initial = requests.try_recv().expect("initial project metadata read");
        assert_eq!(initial.key, "connection-projects");
        this.complete_connection_projects(initial.generation, Ok(projects_reply(data.clone())));
        // Port discovery can be slow; it must not starve the local metadata read.
        this.resources.begin("ports").unwrap();
        this.window_visible = false;
        this.poll_connections(due);
        assert!(requests.try_recv().is_err());
        this.window_visible = true;
        this.poll_connections(due);
        let read = requests
            .try_recv()
            .expect("visible metadata refresh is independent of port polling");
        assert_eq!(read.key, "connection-projects");
        assert_eq!(
            server(&read),
            Some(uuid::Uuid::parse_str(TYPED_SERVER).unwrap())
        );
        assert_eq!(this.connection_bookmarks(), data);
        assert!(
            !this
                .resources
                .get("connection-projects")
                .unwrap()
                .display_pending()
        );
        this.poll_connections(Instant::now());
        assert!(requests.try_recv().is_err(), "pending reads coalesce");
        this.resources.complete(
            "connection-projects",
            read.generation,
            Err("Unavailable".into()),
        );
        this.refresh();
        this.poll_connections(Instant::now());
        assert!(
            requests.try_recv().is_err(),
            "failed refreshes use completion time for backoff"
        );
        assert_eq!(this.connection_bookmarks(), data);
    });
}

#[gpui_kit::test]
fn bookmark_changes_invalidate_only_the_owning_connections_metadata(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        crate::theme::install(false, cx);
    });
    let (owner, cx) = cx.add_window_view(crate::project_library_state::typed_test_owner);
    owner.update(cx, |this, cx| {
        let (work, requests) = std::sync::mpsc::sync_channel(32);
        this.work = work.into();
        this.page = Page::Ports;
        discovery_fixture::put(this, discovery_fixture::observation([]));
        let data = vec![bookmark(1, TYPED_SERVER, "Original", "/srv/app")];
        this.refresh_connection_projects(std::time::Instant::now());
        let initial = requests.try_recv().expect("initial project metadata read");
        assert_eq!(initial.key, "connection-projects");
        this.complete_connection_projects(initial.generation, Ok(projects_reply(data.clone())));
        this.refresh();
        requests.try_iter().for_each(drop);
        this.project_bookmarks_changed("another-server");
        this.poll_connections(std::time::Instant::now());
        assert!(requests.try_recv().is_err());
        let old = this.resources.begin("connection-projects").unwrap();
        this.project_bookmarks_changed(TYPED_SERVER);
        this.poll_connections(std::time::Instant::now());
        let fresh = requests
            .try_recv()
            .expect("bookmark write must invalidate the earlier metadata read");
        assert_eq!(fresh.key, "connection-projects");
        assert!(fresh.generation > old);
        assert!(!this.complete_connection_projects(old, Ok(projects_reply(data.clone()))));
        assert_eq!(this.connection_bookmarks(), data);
        this.complete_connection_projects(fresh.generation, Ok(projects_reply(data.clone())));
        this.navigate(Page::Integration, cx);
        requests.try_iter().for_each(drop);
        this.project_bookmarks_changed(TYPED_SERVER);
        this.poll_connections(std::time::Instant::now());
        assert!(
            requests.try_recv().is_err(),
            "hidden invalidation is deferred"
        );
        this.navigate(Page::Ports, cx);
        assert_eq!(this.connection_bookmarks(), data);
        let reads: Vec<_> = requests
            .try_iter()
            .filter(|work| work.key == "connection-projects")
            .collect();
        assert_eq!(
            reads.len(),
            1,
            "reopening must refresh even a recently cached bookmark list"
        );
    });
}

#[gpui_kit::test]
fn configuration_outage_keeps_connections_cache_and_recovery_revalidates_it(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        crate::theme::install(false, cx);
    });
    let (owner, cx) = cx.add_window_view(crate::project_library_state::typed_test_owner);
    owner.update(cx, |this, cx| {
        let (work, requests) = std::sync::mpsc::sync_channel(32);
        let (replies, incoming) = std::sync::mpsc::channel();
        this.work = work.into();
        this.replies = incoming;
        this.page = Page::Ports;
        let data = discovery_fixture::observation([Listener::new(8080).process(
            "cached application",
            "app",
            &[],
            None,
        )]);
        discovery_fixture::put(this, data.clone());
        this.connections_state
            .entry(TYPED_SERVER.into())
            .or_default()
            .filter = 1;
        this.suspend_connections();
        this.page = Page::Integration;
        let original = this.snapshot.clone().unwrap();
        let mut unavailable = original.clone();
        unavailable.load_error = Some("Vault temporarily unavailable".into());
        unavailable.config.servers.clear();
        let generation = this.resources.begin("snapshot").unwrap();
        replies
            .send(fixture::completion(generation, unavailable))
            .unwrap();
        this.tick(cx);
        assert!(
            this.connections_state.contains_key(TYPED_SERVER),
            "an unavailable profile is not server deletion"
        );
        // Controller recovery test: visit the unavailable route before the
        // profile recovers. It must not consume or overwrite suspended data.
        requests.try_iter().for_each(drop);
        this.navigate(Page::Ports, cx);
        assert!(
            !requests.try_iter().any(|request| matches!(
                kind(&request),
                Some(Kind::ConnectionConnectionsQuery | Kind::PreferencesProjectsList)
            )),
            "unavailable configuration must not start Connections reads"
        );
        this.navigate(Page::Integration, cx);
        let generation = requests
            .try_iter()
            .find(|request| request.key == "snapshot")
            .expect("Integration requests configuration recovery")
            .generation;
        replies
            .send(fixture::completion(generation, original.clone()))
            .unwrap();
        this.tick(cx);
        assert_eq!(this.connections_state[TYPED_SERVER].filter, 1);
        this.page = Page::Ports;
        this.restore_connections();
        assert_eq!(
            this.connections_state[TYPED_SERVER]
                .reading
                .as_ref()
                .unwrap()
                .view()
                .unwrap()
                .evidence(),
            data.data.as_ref().unwrap(),
            "same-owner recovery restores cached ports"
        );
        this.suspend_connections();
        this.page = Page::Integration;
        let mut replacement = original;
        fixture::set(&mut replacement.runtime.connection_revisions, SERVER, 2);
        // Recovery requested another snapshot while reopening the same route.
        let generation = requests
            .try_iter()
            .find(|request| request.key == "snapshot")
            .expect("recovery revalidates the snapshot")
            .generation;
        replies
            .send(fixture::completion(generation, replacement))
            .unwrap();
        this.tick(cx);
        this.page = Page::Ports;
        this.restore_connections();
        assert!(
            this.connections_state[TYPED_SERVER].reading.is_none(),
            "replacement owner cannot reuse old port readings"
        );
    });
}

#[gpui_kit::test]
fn connections_opened_during_outage_restore_on_snapshot_recovery(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        crate::theme::install(false, cx);
    });
    let (owner, cx) = cx.add_window_view(crate::project_library_state::typed_test_owner);
    owner.update(cx, |this, cx| {
        let (work, requests) = std::sync::mpsc::sync_channel(32);
        let (replies, incoming) = std::sync::mpsc::channel();
        this.work = work.into();
        this.replies = incoming;
        this.page = Page::Ports;
        let original = this.snapshot.clone().unwrap();
        let data = discovery_fixture::observation([Listener::new(8080)]);
        discovery_fixture::put(this, data.clone());
        this.navigate(Page::Integration, cx);
        let request = requests.try_recv().unwrap();
        assert_eq!(request.key, "snapshot");
        let mut unavailable = original.clone();
        unavailable.load_error = Some("Vault temporarily unavailable".into());
        unavailable.config.servers.clear();
        replies
            .send(fixture::completion(request.generation, unavailable))
            .unwrap();
        this.tick(cx);
        this.navigate(Page::Ports, cx);
        assert!(requests.try_recv().is_err());
        let generation = this.resources.begin("snapshot").unwrap();
        replies
            .send(fixture::completion(generation, original))
            .unwrap();
        this.tick(cx);
        assert_eq!(
            this.connections_state[TYPED_SERVER]
                .reading
                .as_ref()
                .unwrap()
                .view()
                .unwrap()
                .evidence(),
            data.data.as_ref().unwrap(),
            "recovery restores before revalidation completes"
        );
        let queued: Vec<_> = requests.try_iter().collect();
        assert_eq!(
            queued
                .iter()
                .filter(|r| kind(r) == Some(Kind::ConnectionConnectionsQuery))
                .count(),
            1
        );
        assert!(this.resources.get("ports").unwrap().pending);
        assert!(!this.resources.get("ports").unwrap().display_pending());
    });
}

#[gpui_kit::test]
fn connections_poll_waits_for_valid_configuration_without_losing_manual_intent(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        crate::theme::install(false, cx);
    });
    let (owner, cx) = cx.add_window_view(crate::project_library_state::typed_test_owner);
    owner.update(cx, |this, _| {
        let (work, requests) = std::sync::mpsc::sync_channel(32);
        this.work = work.into();
        this.page = Page::Ports;
        this.window_visible = true;
        discovery_fixture::put(this, discovery_fixture::observation([]));
        let generation = this.resources.begin("ports").unwrap();
        this.discover_connections(true); // Coalesced behind the pending read.
        let completion = crate::connections::DiscoveryCompletion {
            scope: this.selected_connection().unwrap(),
            reading: this.connections_state[TYPED_SERVER]
                .reading
                .clone()
                .unwrap(),
        };
        assert!(this.complete_connections_reading(TYPED_SERVER, generation, completion));
        this.snapshot_mut().load_error = Some("Profile temporarily unavailable".into());
        assert!(
            !this.poll_connections(std::time::Instant::now() + std::time::Duration::from_secs(2))
        );
        assert!(
            requests.try_recv().is_err(),
            "an outage must not dispatch polls"
        );
        this.snapshot_mut().load_error = None;
        this.poll_connections(std::time::Instant::now() + std::time::Duration::from_secs(3));
        let resumed: Vec<_> = requests.try_iter().collect();
        assert_eq!(resumed.len(), 2);
        assert!(
            resumed
                .iter()
                .any(|request| kind(request) == Some(Kind::PreferencesProjectsList))
        );
        let request = resumed
            .iter()
            .find(|request| kind(request) == Some(Kind::ConnectionConnectionsQuery))
            .expect("resume after configuration recovery");
        assert_eq!(kind(request), Some(Kind::ConnectionConnectionsQuery));
        assert!(
            matches!(
                &request.request,
                crate::request_dispatch::Request::Discovery { refresh: true, .. }
            ),
            "retain the user's queued refresh"
        );
        assert!(requests.try_recv().is_err());
    });
}

#[gpui_kit::test]
fn connection_epoch_reloads_bookmarks_then_reopens_worktree_and_rejects_old_reads(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        crate::theme::install(false, cx);
    });
    for (restart, removed) in [(true, false), (false, false), (true, true)] {
        let (owner, cx) = cx.add_window_view(crate::project_library_state::typed_test_owner);
        owner.update(cx, |this, cx| {
            let (work, requests) = std::sync::mpsc::sync_channel(32);
            let (replies, incoming) = std::sync::mpsc::channel();
            this.work = work.into();
            this.replies = incoming;
            this.page = Page::Projects;
            let project = bookmark(1, TYPED_SERVER, "Project", "/repo");
            let worktree = newport_protocol::git::Path::new(b"/linked");
            this.selection = Some(project.clone());
            // A repository is still opening. No real descriptor/transport is
            // installed, so this fixture cannot send a remote release request.
            this.git.opening_path = Some(worktree.clone());
            this.git.tab = 1;
            this.git.page = Some(crate::git_fixture::history_page("old", vec![], None));
            let old_read = this.resources.begin("git-list").unwrap();
            let snapshot_read = this.resources.begin("snapshot").unwrap();
            let mut fresh = this.snapshot.clone().unwrap();
            if restart {
                fresh.instance_id = NEXT_INSTANCE;
            } else {
                fixture::set(&mut fresh.runtime.connection_revisions, SERVER, 2);
            }
            replies
                .send(fixture::completion(snapshot_read, fresh))
                .unwrap();
            replies
                .send(Completion {
                    server: TYPED_SERVER.into(),
                    key: "git-list".into(),
                    generation: old_read,
                    result: crate::Outcome::Git(Ok(crate::git_fixture::history_page(
                        "obsolete",
                        vec![],
                        None,
                    )
                    .into_reply())),
                })
                .unwrap();
            this.tick(cx);
            assert!(this.selection.is_none());
            assert!(this.git.page.is_none(), "old comparison must be retired");
            assert!(
                this.resources.get("git-list").is_none(),
                "late reply must be rejected"
            );
            let saved = this.restore_git.as_ref().unwrap();
            assert_eq!(saved.root, worktree);
            assert_eq!(saved.tab, 1);
            let pending: Vec<_> = requests.try_iter().collect();
            assert_eq!(pending.len(), 1, "recovery must not replay any write");
            let list = &pending[0];
            assert_eq!(kind(list), Some(Kind::PreferencesProjectsList));
            replies
                .send(projects_completion(
                    "projects",
                    list.generation,
                    if removed {
                        Vec::new()
                    } else {
                        vec![project.clone()]
                    },
                ))
                .unwrap();
            this.tick(cx);
            if removed {
                assert!(this.selection.is_none());
                assert!(this.restore_git.is_none());
                assert!(
                    requests.try_recv().is_err(),
                    "deleted bookmark must not reopen"
                );
                return;
            }
            assert_eq!(this.selection, Some(project));
            assert_eq!(this.git.tab, 1);
            assert!(this.restore_git.is_none());
            let pending: Vec<_> = requests.try_iter().collect();
            assert_eq!(pending.len(), 1);
            assert_eq!(kind(&pending[0]), Some(Kind::GitRequest));
            assert_eq!(
                git_request(&pending[0]),
                &newport_protocol::git::Request::Open {
                    path: worktree.clone()
                }
            );
        });
    }
}

#[gpui_kit::test]
fn ordinary_snapshot_and_configuration_outage_do_not_reset_repository(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        crate::theme::install(false, cx);
    });
    let (owner, cx) = cx.add_window_view(crate::project_library_state::typed_test_owner);
    owner.update(cx, |this, cx| {
        let (work, requests) = std::sync::mpsc::sync_channel(32);
        let (replies, incoming) = std::sync::mpsc::channel();
        this.work = work.into();
        this.replies = incoming;
        this.page = Page::Projects;
        let selection = bookmark(1, TYPED_SERVER, "Project", "/repo");
        this.selection = Some(selection.clone());
        let cached = Some(crate::git_fixture::history_page(
            "current",
            vec![crate::git_fixture::commit("cached", "Cached")],
            None,
        ));
        this.git.page = cached.clone();
        for unavailable in [false, true] {
            let generation = this.resources.begin("snapshot").unwrap();
            let mut fresh = this.snapshot.clone().unwrap();
            if unavailable {
                fresh.load_error = Some("Vault temporarily unavailable".into());
                fresh.config.servers.clear();
            }
            replies
                .send(fixture::completion(generation, fresh))
                .unwrap();
            this.tick(cx);
            assert_eq!(this.selection, Some(selection.clone()));
            assert_eq!(this.git.page, cached);
            assert!(requests.try_recv().is_err());
        }
    });
}
