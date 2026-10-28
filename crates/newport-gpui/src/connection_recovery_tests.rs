//! Exercise actual snapshot completion/reset/reopen ordering without a backend.
use crate::{Completion, Page};
use gpui_kit::TestAppContext;
use serde_json::{Value, json};

#[gpui_kit::test]
fn connection_project_metadata_refreshes_independently_and_respects_retry_interval(
    cx: &mut TestAppContext,
) {
    use std::time::{Duration, Instant};
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        crate::theme::install(false, cx);
    });
    let (owner, cx) = cx.add_window_view(crate::page_perf_tests::owner);
    owner.update(cx, |this, _| {
        let (work, requests) = std::sync::mpsc::sync_channel(32);
        this.work = work.into();
        this.page = Page::Ports;
        let data = json!([{"name":"Original", "path":"/srv/app"}]);
        let due = Instant::now() + Duration::from_secs(61);
        let generation = this.resources.begin("connection-projects").unwrap();
        this.resources
            .complete("connection-projects", generation, Ok(data.clone()));
        // Port discovery can be slow; it must not starve the local metadata read.
        this.resources.begin("page").unwrap();
        this.window_visible = false;
        this.poll_connections(due);
        assert!(requests.try_recv().is_err());
        this.window_visible = true;
        this.poll_connections(due);
        let read = requests
            .try_recv()
            .expect("visible metadata refresh is independent of port polling");
        assert_eq!(read.key, "connection-projects");
        assert_eq!(read.args["serverId"], "fixture");
        assert_eq!(this.value("connection-projects"), data);
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
        assert_eq!(this.value("connection-projects"), data);
    });
}

#[gpui_kit::test]
fn bookmark_changes_invalidate_only_the_owning_connections_metadata(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        crate::theme::install(false, cx);
    });
    let (owner, cx) = cx.add_window_view(crate::page_perf_tests::owner);
    owner.update(cx, |this, cx| {
        let (work, requests) = std::sync::mpsc::sync_channel(32);
        this.work = work.into();
        this.page = Page::Ports;
        let data = json!([{"name":"Original", "path":"/srv/app"}]);
        let generation = this.resources.begin("connection-projects").unwrap();
        this.resources
            .complete("connection-projects", generation, Ok(data.clone()));
        this.refresh();
        requests.try_iter().for_each(drop);
        this.project_bookmarks_changed("another-server");
        this.poll_connections(std::time::Instant::now());
        assert!(requests.try_recv().is_err());
        let old = this.resources.begin("connection-projects").unwrap();
        this.project_bookmarks_changed("fixture");
        this.poll_connections(std::time::Instant::now());
        let fresh = requests
            .try_recv()
            .expect("bookmark write must invalidate the earlier metadata read");
        assert_eq!(fresh.key, "connection-projects");
        assert!(fresh.generation > old);
        assert!(
            !this
                .resources
                .complete("connection-projects", old, Ok(data.clone()))
        );
        assert_eq!(this.value("connection-projects"), data);
        this.resources
            .complete("connection-projects", fresh.generation, Ok(data.clone()));
        this.navigate(Page::Integration, cx);
        requests.try_iter().for_each(drop);
        this.project_bookmarks_changed("fixture");
        this.poll_connections(std::time::Instant::now());
        assert!(
            requests.try_recv().is_err(),
            "hidden invalidation is deferred"
        );
        this.navigate(Page::Ports, cx);
        assert_eq!(this.value("connection-projects"), data);
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

fn completion(key: &str, generation: u64, value: Value) -> Completion {
    Completion {
        pdf: None,
        pdf_text: None,
        image: None,
        server: "fixture".into(),
        key: key.into(),
        generation,
        result: Ok(value),
    }
}

#[gpui_kit::test]
fn configuration_outage_keeps_connections_cache_and_recovery_revalidates_it(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        crate::theme::install(false, cx);
    });
    let (owner, cx) = cx.add_window_view(crate::page_perf_tests::owner);
    owner.update(cx, |this, cx| {
        let (work, requests) = std::sync::mpsc::sync_channel(32);
        let (replies, incoming) = std::sync::mpsc::channel();
        this.work = work.into();
        this.replies = incoming;
        this.page = Page::Ports;
        let data = json!([{"port":8080,"process":"cached application"}]);
        let generation = this.resources.begin("page").unwrap();
        this.resources
            .complete("page", generation, Ok(data.clone()));
        this.connections_state
            .entry("fixture".into())
            .or_default()
            .filter = 1;
        this.suspend_connections();
        this.page = Page::Integration;
        let original = this.snapshot.clone();
        let mut unavailable = original.clone();
        unavailable["loadError"] = json!("Vault temporarily unavailable");
        unavailable["config"]["servers"] = json!([]);
        let generation = this.resources.begin("snapshot").unwrap();
        replies
            .send(completion("snapshot", generation, unavailable))
            .unwrap();
        this.tick(cx);
        assert!(
            this.connections_state.contains_key("fixture"),
            "an unavailable profile is not server deletion"
        );
        // Controller recovery test: visit the unavailable route before the
        // profile recovers. It must not consume or overwrite suspended data.
        requests.try_iter().for_each(drop);
        this.navigate(Page::Ports, cx);
        assert!(
            !requests.try_iter().any(|request| matches!(
                request.method.as_str(),
                "connections_query" | "git_projects_list"
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
            .send(completion("snapshot", generation, original.clone()))
            .unwrap();
        this.tick(cx);
        assert_eq!(this.connections_state["fixture"].filter, 1);
        this.page = Page::Ports;
        this.restore_connections();
        assert_eq!(
            this.value("page"),
            data,
            "same-owner recovery restores cached ports"
        );
        this.suspend_connections();
        this.page = Page::Integration;
        let mut replacement = original;
        replacement["runtime"]["connectionRevisions"]["fixture"] = json!(2);
        // Recovery requested another snapshot while reopening the same route.
        let generation = requests
            .try_iter()
            .find(|request| request.key == "snapshot")
            .expect("recovery revalidates the snapshot")
            .generation;
        replies
            .send(completion("snapshot", generation, replacement))
            .unwrap();
        this.tick(cx);
        this.page = Page::Ports;
        this.restore_connections();
        assert!(
            this.value("page").is_null(),
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
    let (owner, cx) = cx.add_window_view(crate::page_perf_tests::owner);
    owner.update(cx, |this, cx| {
        let (work, requests) = std::sync::mpsc::sync_channel(32);
        let (replies, incoming) = std::sync::mpsc::channel();
        this.work = work.into();
        this.replies = incoming;
        this.page = Page::Ports;
        let original = this.snapshot.clone();
        let data = json!([{"port":8080}]);
        let generation = this.resources.begin("page").unwrap();
        this.resources
            .complete("page", generation, Ok(data.clone()));
        this.navigate(Page::Integration, cx);
        let request = requests.try_recv().unwrap();
        assert_eq!(request.key, "snapshot");
        let mut unavailable = original.clone();
        unavailable["loadError"] = json!("Vault temporarily unavailable");
        unavailable["config"]["servers"] = json!([]);
        replies
            .send(completion("snapshot", request.generation, unavailable))
            .unwrap();
        this.tick(cx);
        this.navigate(Page::Ports, cx);
        assert!(requests.try_recv().is_err());
        let generation = this.resources.begin("snapshot").unwrap();
        replies
            .send(completion("snapshot", generation, original))
            .unwrap();
        this.tick(cx);
        assert_eq!(
            this.value("page"),
            data,
            "recovery restores before revalidation completes"
        );
        let queued: Vec<_> = requests.try_iter().collect();
        assert_eq!(
            queued
                .iter()
                .filter(|r| r.method == "connections_query")
                .count(),
            1
        );
        assert!(this.resources.get("page").unwrap().pending);
        assert!(!this.resources.get("page").unwrap().display_pending());
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
    let (owner, cx) = cx.add_window_view(crate::page_perf_tests::owner);
    owner.update(cx, |this, _| {
        let (work, requests) = std::sync::mpsc::sync_channel(32);
        this.work = work.into();
        this.page = Page::Ports;
        this.window_visible = true;
        let generation = this.resources.begin("page").unwrap();
        this.discover_connections(true); // Coalesced behind the pending read.
        this.resources.complete("page", generation, Ok(json!([])));
        this.snapshot["loadError"] = json!("Profile temporarily unavailable");
        assert!(
            !this.poll_connections(std::time::Instant::now() + std::time::Duration::from_secs(2))
        );
        assert!(
            requests.try_recv().is_err(),
            "an outage must not dispatch polls"
        );
        this.snapshot["loadError"] = Value::Null;
        this.poll_connections(std::time::Instant::now() + std::time::Duration::from_secs(3));
        let resumed: Vec<_> = requests.try_iter().collect();
        assert_eq!(resumed.len(), 2);
        assert!(
            resumed
                .iter()
                .any(|request| request.method == "git_projects_list")
        );
        let request = resumed
            .iter()
            .find(|request| request.method == "connections_query")
            .expect("resume after configuration recovery");
        assert_eq!(request.method, "connections_query");
        assert_eq!(
            request.args["refresh"], true,
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
        let (owner, cx) = cx.add_window_view(crate::page_perf_tests::owner);
        owner.update(cx, |this, cx| {
            let (work, requests) = std::sync::mpsc::sync_channel(32);
            let (replies, incoming) = std::sync::mpsc::channel();
            this.work = work.into();
            this.replies = incoming;
            this.page = Page::Projects;
            this.snapshot["instanceId"] = json!("backend-before");
            this.snapshot["runtime"]["connectionRevisions"] = json!({"fixture":1});
            let project = json!({"id":"bookmark", "serverId":"fixture", "name":"Project",
                "path":{"display":"/repo","bytesB64":"L3JlcG8="}});
            let worktree = json!({"display":"/linked","bytesB64":"L2xpbmtlZA=="});
            this.selection = Some(project.clone());
            // A repository is still opening. No real descriptor/transport is
            // installed, so this fixture cannot send a remote release request.
            this.git.opening_path = worktree.clone();
            this.git.tab = 1;
            this.git.page = json!({"snapshot":"old", "entries":[]});
            let old_read = this.resources.begin("git-list").unwrap();
            let snapshot_read = this.resources.begin("snapshot").unwrap();
            let mut fresh = this.snapshot.clone();
            if restart {
                fresh["instanceId"] = json!("backend-after");
            } else {
                fresh["runtime"]["connectionRevisions"]["fixture"] = json!(2);
            }
            replies
                .send(completion("snapshot", snapshot_read, fresh))
                .unwrap();
            replies
                .send(completion(
                    "git-list",
                    old_read,
                    json!({"snapshot":"obsolete"}),
                ))
                .unwrap();
            this.tick(cx);
            assert!(this.selection.is_none());
            assert!(this.git.page.is_null(), "old comparison must be retired");
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
            assert_eq!(list.method, "git_projects_list");
            replies
                .send(completion(
                    "page",
                    list.generation,
                    if removed {
                        json!([])
                    } else {
                        json!([project.clone()])
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
            assert_eq!(pending[0].method, "git_request");
            assert_eq!(pending[0].args["request"]["method"], "repo.open");
            assert_eq!(pending[0].args["request"]["params"]["path"], worktree);
        });
    }
}

#[gpui_kit::test]
fn ordinary_snapshot_and_configuration_outage_do_not_reset_repository(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        crate::theme::install(false, cx);
    });
    let (owner, cx) = cx.add_window_view(crate::page_perf_tests::owner);
    owner.update(cx, |this, cx| {
        let (work, requests) = std::sync::mpsc::sync_channel(32);
        let (replies, incoming) = std::sync::mpsc::channel();
        this.work = work.into();
        this.replies = incoming;
        this.page = Page::Projects;
        this.snapshot["instanceId"] = json!("backend");
        let selection = json!({"id":"bookmark","serverId":"fixture"});
        this.selection = Some(selection.clone());
        let cached = json!({"entries":["cached"],"snapshot":"current"});
        this.git.page = cached.clone();
        for unavailable in [false, true] {
            let generation = this.resources.begin("snapshot").unwrap();
            let mut fresh = this.snapshot.clone();
            if unavailable {
                fresh["loadError"] = json!("Vault temporarily unavailable");
                fresh["config"]["servers"] = json!([]);
            }
            replies
                .send(completion("snapshot", generation, fresh))
                .unwrap();
            this.tick(cx);
            assert_eq!(this.selection, Some(selection.clone()));
            assert_eq!(this.git.page, cached);
            assert!(requests.try_recv().is_err());
        }
    });
}
