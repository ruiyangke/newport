use crate::page_perf_tests::{PageHost, frame, owner};
use crate::{Newport, Page, theme};
use gpui::{AppContext, Entity, TestAppContext, VisualTestContext, px, size};
use gpui_kit as gpui;
use serde_json::{Value, json};

fn fixture(cx: &mut TestAppContext) -> (Entity<Newport>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        theme::install(false, cx);
        cx.set_reduce_motion(true);
    });
    let mut view = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let entity = cx.new(|cx| {
            let mut v = owner(window, cx);
            v.page = Page::Integration;

            v.snapshot["runtime"]["agentStatus"]["fixture"] =
                json!({"installed":true,"installedHash":"a".repeat(64)});
            v
        });
        view = Some(entity.clone());
        PageHost(entity)
    });
    visual.simulate_resize(size(px(550.), px(1500.)));
    frame(visual);
    (view.unwrap(), visual)
}

fn has(visual: &mut VisualTestContext, label: &str) -> bool {
    visual.update(|window, _| {
        gpui_kit::base::test_support::snapshots(window)
            .iter()
            .any(|n| n.label() == Some(label))
    })
}

fn click(visual: &mut VisualTestContext, label: &str) {
    let bounds = visual.update(|window, _| {
        gpui_kit::base::test_support::snapshots(window)
            .iter()
            .find(|n| n.label() == Some(label))
            .unwrap_or_else(|| panic!("Missing {label}"))
            .bounds()
    });
    visual.simulate_click(bounds.center(), Default::default());
    frame(visual);
}

#[gpui::test]
fn integration_revocations_remain_independent_during_lifecycle_work(cx: &mut TestAppContext) {
    let (view, visual) = fixture(cx);
    let (work, requests) = std::sync::mpsc::sync_channel(16);
    view.update(visual, |v, _| v.work = work.into());
    for busy in [
        "reinstall",
        "feature",
        "recovery",
        "review",
        "removal",
        "helper-restore",
        "helper-inspect",
        "snapshot",
        "installation",
    ] {
        view.update(visual, |v, cx| {
            v.resources.clear();
            v.integration_refresh_pending.clear();
            v.snapshot["runtime"]["agentInstallation"]["fixture"] = Value::Null;
            v.snapshot["runtime"]["agentOperations"]["fixture"] = Value::Null;
            if busy == "snapshot" {
                v.integration_refresh_pending.insert("fixture".into());
            } else if busy == "installation" {
                v.snapshot["runtime"]["agentInstallation"]["fixture"] = json!({"stage":"uploading"});
            } else {
                v.resources.begin(&format!("integration:{busy}:fixture")).unwrap();
                if busy == "removal" {
                    v.snapshot["runtime"]["agentOperations"]["fixture"] = json!({"removals":[{"operationId":uuid::Uuid::new_v4().to_string(),"phase":"restoring"}]});
                }
            }
            v.snapshot["config"]["servers"][0]["clipboardEnabled"] = json!(true);
            v.snapshot["config"]["servers"][0]["browserEnabled"] = json!(true);
            cx.notify();
        });
        frame(visual);
        let mut pending = Vec::new();
        for (label, feature) in [
            ("Clipboard access", "clipboard"),
            ("Browser links", "browser"),
        ] {
            let point = visual.update(|window, _| {
                gpui_kit::base::test_support::snapshots(window)
                    .into_iter()
                    .find(|node| {
                        node.label() == Some(label) && node.role() == Some(gpui::Role::Label)
                    })
                    .unwrap()
                    .bounds()
                    .center()
            });
            visual.simulate_click(point, Default::default());
            frame(visual);
            let request = requests
                .try_recv()
                .unwrap_or_else(|_| panic!("{feature} must remain revocable during {busy}"));
            assert_eq!(
                request.key,
                format!("integration:disable:{feature}:fixture")
            );
            assert_eq!(request.method, "set_integration_enabled");
            assert_eq!(
                request.args,
                json!({"id":"fixture","feature":feature,"enabled":false,
                    "expectedConnection":view.read_with(visual, |v, _| crate::terminal_scope::connection(&v.selected_server().unwrap(), &v.snapshot))})
            );
            visual.simulate_click(point, Default::default());
            frame(visual);
            assert!(requests.try_recv().is_err(), "duplicate pending revocation");
            pending.push(request);
        }
        assert!(has(visual, "Clipboard access: Turning off…"));
        view.update(visual, |v, cx| {
            for request in &pending {
                v.resources
                    .complete(&request.key, request.generation, Ok(Value::Null));
            }
            v.integration_command("fixture", Some(("browser", true)), cx);
        });
        assert!(
            requests.try_recv().is_err(),
            "enabling must still wait during {busy}"
        );
    }
}

#[gpui::test]
fn confirmation_requires_current_review_and_retries_the_same_operation(cx: &mut TestAppContext) {
    let (view, visual) = fixture(cx);
    let (work, requests) = std::sync::mpsc::sync_channel(8);
    view.update(visual, |v, _| v.work = work.into());
    click(visual, "Remove Needle…");
    assert!(requests.try_recv().is_err(), "review must be read-only");
    let cancelled = view.read_with(visual, |v, _| v.needle_removal_reviews["fixture"].clone());
    click(visual, "Cancel");
    view.update(visual, |v, cx| {
        v.request_needle_removal(cancelled.clone(), false, cx)
    });
    assert!(
        requests.try_recv().is_err(),
        "cancelled callback must be fenced"
    );
    click(visual, "Remove Needle…");
    let review = view.read_with(visual, |v, _| v.needle_removal_reviews["fixture"].clone());
    assert_ne!(review.id, cancelled.id);
    view.update(visual, |v, cx| {
        v.request_needle_removal(cancelled, false, cx)
    });
    assert!(
        requests.try_recv().is_err(),
        "replaced review must be fenced"
    );
    click(visual, "Remove Needle");
    let first = requests.try_recv().unwrap();
    assert_eq!(first.method, "remove_agent");
    assert_eq!(
        first.args,
        json!({"expectedConnection":review.server,"operationId":review.id,"build":review.build})
    );
    assert!(uuid::Uuid::parse_str(&review.id).is_ok());
    click(visual, "Retry removal");
    assert!(
        requests.try_recv().is_err(),
        "pending action must be disabled"
    );
    view.update(visual, |v, cx| {
        v.resources
            .complete(&first.key, first.generation, Err("Reply lost".into()));
        cx.notify();
    });
    frame(visual);
    assert!(has(visual, "Reply lost"));
    click(visual, "Retry removal");
    let retry = requests.try_recv().unwrap();
    assert_eq!(
        retry.args, first.args,
        "uncertain dispatch must retain identity"
    );
}

#[gpui::test]
fn confirmation_rejects_changed_connection_build_and_unverified_installation(
    cx: &mut TestAppContext,
) {
    let (view, visual) = fixture(cx);
    let (work, requests) = std::sync::mpsc::sync_channel(8);
    view.update(visual, |v, _| v.work = work.into());
    for change in ["host", "build", "error", "missing", "journal", "deployment"] {
        view.update(visual, |v, cx| {
            v.snapshot["config"]["servers"][0]["sshHost"] = json!("localhost");
            v.snapshot["runtime"]["agentStatus"]["fixture"] =
                json!({"installed":true,"installedHash":"a".repeat(64)});
            v.snapshot["runtime"]["agentOperations"]["fixture"] = json!({});
            v.review_needle_removal(cx);
            let review = v.needle_removal_reviews["fixture"].clone();
            match change {
                "host" => v.snapshot["config"]["servers"][0]["sshHost"] = json!("replacement"),
                "build" => {
                    v.snapshot["runtime"]["agentStatus"]["fixture"]["installedHash"] =
                        json!("b".repeat(64))
                }
                "error" => {
                    v.snapshot["runtime"]["agentStatus"]["fixture"]["error"] =
                        json!("Discovery failed")
                }
                "missing" => {
                    v.snapshot["runtime"]["agentStatus"]["fixture"]["installed"] = json!(false)
                }
                "journal" => {
                    v.snapshot["runtime"]["agentOperations"]["fixture"]["error"] =
                        json!("Journal unreadable")
                }
                "deployment" => {
                    v.snapshot["runtime"]["agentOperations"]["fixture"]["operations"] =
                        json!([{"phase":"needsResolution","intent":{"operationId":"unfinished"}}])
                }
                _ => unreachable!(),
            }
            v.request_needle_removal(review, false, cx);
        });
        assert!(
            requests.try_recv().is_err(),
            "accepted stale {change} confirmation"
        );
        if matches!(change, "host" | "build" | "error" | "missing") {
            frame(visual);
            assert!(has(
                visual,
                "The server or installation state changed. Cancel and review removal again."
            ));
            // Kit's test snapshots observe the inner button; its native disabled
            // flag lives on an outer semantics wrapper. Exercise the rendered action.
            click(visual, "Remove Needle");
            assert!(requests.try_recv().is_err(), "invalid {change} action");
        }
    }
}

#[gpui::test]
fn removal_recovery_uses_durable_state_and_preserves_preferences(cx: &mut TestAppContext) {
    let (view, visual) = fixture(cx);
    let (work, requests) = std::sync::mpsc::sync_channel(8);
    let id = uuid::Uuid::new_v4().to_string();
    let build = "a".repeat(64);
    view.update(visual, |v, cx| {
        v.work = work.into();
        v.snapshot["runtime"]["agentOperations"]["fixture"] = json!({"activeOperationId":id,"removals":[{"operationId":id,"build":build,"phase":"removing"}]});
        // Navigation discards page resources; backend ownership remains authoritative.
        v.reset_view(true);
        cx.notify();
    });
    frame(visual);
    assert!(has(visual, "Removing Needle…"));
    assert!(has(visual, "Sharing connection: Paused"));
    assert!(!has(visual, "Sharing connection: Connected"));
    assert!(!has(visual, "Reinstall Needle"));
    click(visual, "Resume removal");
    click(visual, "Restore or cancel removal");
    assert!(requests.try_recv().is_err());
    view.update(visual, |v, cx| {
        v.snapshot["runtime"]["agentOperations"]["fixture"]["activeOperationId"] = Value::Null;
        v.snapshot["runtime"]["agentOperations"]["fixture"]["removals"][0]["error"] =
            json!("Another client is using Needle. Disconnect it, then resume removal.");
        cx.notify();
    });
    frame(visual);
    assert!(has(visual, "Removal unfinished"));
    click(visual, "Resume removal");
    let resume = requests.try_recv().unwrap();
    assert_eq!(resume.method, "remove_agent");
    assert_eq!(resume.args["operationId"], id);
    assert_eq!(resume.args["build"], build);
    view.update(visual, |v, cx| {
        v.resources
            .complete(&resume.key, resume.generation, Ok(json!("Needle removed")));
        v.resources.clear();
        v.snapshot["runtime"]["agentOperations"]["fixture"]["removals"][0] =
            json!({"operationId":id,"build":build,"phase":"removed"});
        cx.notify();
    });
    frame(visual);
    assert!(
        has(visual, "Needle removed"),
        "{:?}",
        visual.update(|window, _| gpui_kit::base::test_support::snapshots(window)
            .iter()
            .filter_map(|n| n.label().map(str::to_owned))
            .collect::<Vec<_>>())
    );
    assert!(!has(visual, "Resume removal"));
    click(visual, "Restore Needle");
    let restore = requests.try_recv().unwrap();
    assert_eq!(restore.method, "restore_removed_agent");
    assert_eq!(restore.args["operationId"], id);
    assert_eq!(
        restore.args["expectedConnection"],
        resume.args["expectedConnection"]
    );
    assert!(restore.args.get("build").is_none());
    assert!(has(visual, "Restoring Needle…"));
    view.update(visual, |v, cx| {
        v.resources
            .complete(&restore.key, restore.generation, Err("Reply lost".into()));
        v.resources.clear();
        v.snapshot["runtime"]["agentOperations"]["fixture"]["removals"][0]["phase"] =
            json!("restoring");
        cx.notify();
    });
    frame(visual);
    assert!(has(visual, "Restore unfinished"));
    assert!(!has(visual, "Resume removal"));
    click(visual, "Retry restore");
    let retry = requests.try_recv().unwrap();
    assert_eq!(retry.args, restore.args);
    view.update(visual, |v, cx| {
        v.resources
            .complete(&retry.key, retry.generation, Ok(json!("Needle restored")));
        v.resources.clear();
        v.snapshot["runtime"]["agentOperations"]["fixture"]["removals"][0]["phase"] =
            json!("restored");
        assert_eq!(v.snapshot["config"]["servers"][0]["clipboardEnabled"], true);
        assert_eq!(v.snapshot["config"]["servers"][0]["browserEnabled"], true);
        cx.notify();
    });
    frame(visual);
    assert!(has(visual, "Reinstall Needle"));
    assert!(has(visual, "Remove Needle…"));
    assert!(!has(visual, "Needle removal requested"));
    assert!(!has(visual, "Sharing connection: Paused"));
}

#[gpui::test]
fn removal_review_and_errors_fit_narrow_and_wide_pages(cx: &mut TestAppContext) {
    let (view, visual) = fixture(cx);
    let name = "Long production server name ".repeat(10);
    let error = "Removal is pending because another client is using Needle. ".repeat(8);
    view.update(visual, |v, cx| {
        v.snapshot["config"]["servers"][0]["name"] = json!(name);
        v.review_needle_removal(cx);
    });
    let target = format!("{name} · fixture@localhost");
    for width in [550., 1000.] {
        visual.simulate_resize(size(px(width), px(1500.)));
        frame(visual);
        visual.update(|window, _| {
            let nodes = gpui_kit::base::test_support::snapshots(window);
            for label in [&target, "Remove Needle", "Cancel"] {
                let node = nodes.iter().find(|n| n.label() == Some(label)).unwrap();
                assert!(node.bounds().left() >= px(0.));
                assert!(
                    node.bounds().right() <= px(width),
                    "{label} exceeds {width}"
                );
            }
        });
    }
    view.update(visual, |v, cx| {
        v.snapshot["runtime"]["agentOperations"]["fixture"] = json!({"removals":[{"operationId":uuid::Uuid::new_v4().to_string(),"build":"a".repeat(64),"phase":"removing","error":error}]});
        cx.notify();
    });
    for width in [550., 1000.] {
        visual.simulate_resize(size(px(width), px(1500.)));
        frame(visual);
        visual.update(|window, _| {
            let nodes = gpui_kit::base::test_support::snapshots(window);
            for label in [&error, "Resume removal", "Restore or cancel removal"] {
                let node = nodes.iter().find(|n| n.label() == Some(label)).unwrap();
                assert!(
                    node.bounds().right() <= px(width),
                    "{label} exceeds {width}"
                );
            }
        });
    }
}

#[gpui::test]
fn restoring_removal_reviews_conflicts_and_fences_obsolete_keep_choices(cx: &mut TestAppContext) {
    let (view, visual) = fixture(cx);
    let (work, requests) = std::sync::mpsc::sync_channel(8);
    let id = uuid::Uuid::new_v4().to_string();
    let fingerprint = "a".repeat(64);
    let path = format!("/home/fixture/{}/.bashrc", "long-folder/".repeat(15));
    view.update(visual, |v, cx| {
        v.work = work.into();
        v.snapshot["runtime"]["agentOperations"]["fixture"] = json!({"removals":[{"operationId":id,"build":"b".repeat(64),"phase":"restoring","error":"A file changed after removal."}]});
        cx.notify();
    });
    frame(visual);
    click(visual, "Review files");
    let review = requests.try_recv().unwrap();
    assert_eq!(review.method, "review_agent");
    assert_eq!(review.args, json!({"id":"fixture","operationId":id}));
    click(visual, "Review files");
    assert!(requests.try_recv().is_err());
    assert!(has(visual, "Reviewing file changes…"));
    view.update(visual, |v, cx| {
        v.resources.complete(&review.key, review.generation, Ok(json!({"operationId":id,"files":[{"index":2,"path":path,"status":"conflict","message":"This file has changed.","fingerprint":fingerprint}]})));
        cx.notify();
    });
    for width in [550., 1000.] {
        visual.simulate_resize(size(px(width), px(1500.)));
        frame(visual);
        visual.update(|window, _| {
            let nodes = gpui_kit::base::test_support::snapshots(window);
            for label in [&path, "Keep current file", "Retry restore", "Review files"] {
                let node = nodes.iter().find(|n| n.label() == Some(label)).unwrap();
                assert!(
                    node.bounds().right() <= px(width),
                    "{label} exceeds {width}"
                );
            }
        });
    }
    click(visual, "Keep current file");
    let keep = requests.try_recv().unwrap();
    assert_eq!(keep.method, "keep_agent_file");
    assert_eq!(
        keep.args,
        json!({"id":"fixture","operationId":id,"index":2,"fingerprint":fingerprint})
    );
    view.update(visual, |v, cx| {
        v.resources.complete(
            &keep.key,
            keep.generation,
            Err("The file changed since review. Refresh before choosing to keep it.".into()),
        );
        cx.notify();
    });
    frame(visual);
    assert!(!has(visual, "Keep current file"));
    view.update(visual, |v, cx| {
        v.integration_review("fixture", &id, Some((2, fingerprint.clone())), cx)
    });
    assert!(
        requests.try_recv().is_err(),
        "failed review cannot authorize a cached keep callback"
    );
    click(visual, "Review files");
    let fresh = requests.try_recv().unwrap();
    view.update(visual, |v, cx| {
        v.resources.complete(&fresh.key, fresh.generation, Ok(json!({"operationId":id,"files":[{"index":2,"path":path,"status":"conflict","message":"This file has changed.","fingerprint":fingerprint}]})));
        v.snapshot["runtime"]["agentOperations"]["fixture"]["removals"][0]["phase"] = json!("restored");
        v.integration_review("fixture", &id, Some((2, fingerprint)), cx);
        cx.notify();
    });
    assert!(
        requests.try_recv().is_err(),
        "restored operation cannot accept delayed file decisions"
    );
    frame(visual);
    assert!(!has(visual, "Keep current file"));
    assert!(!has(visual, "Review files"));
}

#[gpui::test]
fn integration_callbacks_and_queued_requests_retain_the_displayed_connection(
    cx: &mut TestAppContext,
) {
    let (view, visual) = fixture(cx);
    let (work, requests) = std::sync::mpsc::sync_channel(8);
    let original = view.update(visual, |v, _| {
        v.work = work.into();
        crate::terminal_scope::connection(&v.selected_server().unwrap(), &v.snapshot)
    });
    view.update(visual, |v, cx| {
        v.integration_scoped_command("fixture", &original, ("browser", false), cx)
    });
    let request = requests.try_recv().unwrap();
    assert_eq!(request.args["expectedConnection"], original);
    view.update(visual, |v, cx| {
        v.resources
            .complete(&request.key, request.generation, Ok(Value::Null));
        v.snapshot["runtime"]["connectionRevisions"] = json!({"fixture":1});
        for enabled in [false, true] {
            v.integration_scoped_command("fixture", &original, ("browser", enabled), cx);
        }
    });
    assert!(
        requests.try_recv().is_err(),
        "old rendered callbacks cannot follow new credentials"
    );
    assert_eq!(
        request.args["expectedConnection"], original,
        "queued intent is immutable"
    );
    view.update(visual, |v, cx| {
        let current = crate::terminal_scope::connection(&v.selected_server().unwrap(), &v.snapshot);
        v.integration_scoped_command("fixture", &current, ("browser", false), cx);
    });
    let fresh = requests.try_recv().unwrap();
    assert_ne!(fresh.args["expectedConnection"], original);
    assert_eq!(fresh.args["expectedConnection"][8], 1);
}
