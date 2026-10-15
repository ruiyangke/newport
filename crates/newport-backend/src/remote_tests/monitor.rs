//! Overview metrics through the agent on a real Linux server.
use super::server;
use crate::{cockpit, ssh};
use tokio::time::{Duration, sleep};

#[tokio::test]
#[ignore = "Requires npm run test:remote and disposable Linux OpenSSH"]
async fn overview_query_records_in_background_without_profile_or_service_lock_and_clear_restarts_cleanly()
 {
    use crate::{commands, manager, metrics_db::MetricsDb, metrics_sampler, profile::Store};
    let server = super::installed_server("NEWPORT_TEST_SSH_PORT", "Query metrics", "fixture").await;
    let directory = tempfile::tempdir().unwrap();
    let mut manager = manager::Manager::new(Store::for_test(directory.path().into()));
    manager.config.servers.push(server.clone());
    let state = manager::share(manager);
    let db = MetricsDb::open(&directory.path().join("metrics.db"), Ok(vec![7; 32]))
        .await
        .unwrap();
    let owner = state.owner(server.id).unwrap();
    let service_channel = owner
        .monitor
        .for_request(&serde_json::json!({"op":"action"}));
    let service_busy = service_channel.lock().await;
    let writer = state.lock().await;
    let worker = tokio::spawn(metrics_sampler::run(state.clone(), db.clone()));
    let first = tokio::time::timeout(
        Duration::from_secs(30),
        commands::cockpit_collect(&state, &db, None, server.id, cockpit::Section::Overview),
    )
    .await
    .expect("metrics must not wait for another feature or profile writer")
    .unwrap();
    assert!(first["cpu"].is_number());
    assert!(first.get("historyError").is_none());
    let next = tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            sleep(Duration::from_millis(100)).await;
            let reading = metrics_sampler::read(&state, &db, server.id, false)
                .await
                .unwrap();
            if reading["sampledAt"] != first["sampledAt"] {
                break reading;
            }
        }
    })
    .await
    .expect("background sampling continues at three seconds");
    assert!(next["sampledAt"].as_i64().unwrap() > first["sampledAt"].as_i64().unwrap());
    let count = db.cache_info().await.unwrap().samples;
    assert!(count >= 2);
    for _ in 0..12 {
        assert_eq!(
            metrics_sampler::read(&state, &db, server.id, false)
                .await
                .unwrap()["sampledAt"],
            next["sampledAt"]
        );
    }
    assert_eq!(
        db.cache_info().await.unwrap().samples,
        count,
        "page reads must not record duplicate samples"
    );
    drop((writer, service_busy));
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    commands::clear_metrics_cache(&state, &db).await.unwrap();
    assert_eq!(db.cache_info().await.unwrap().samples, 0);
    let cleared_at = crate::metrics_db::now_ms();
    let fresh = metrics_sampler::read(&state, &db, server.id, false)
        .await
        .unwrap();
    assert!(fresh["sampledAt"].as_i64().unwrap() >= cleared_at);
    assert_eq!(db.cache_info().await.unwrap().samples, 1);
    manager::shutdown(&state).await;
    db.close().await;
}

#[tokio::test]
#[ignore = "Requires npm run test:remote and real OpenSSH"]
async fn overview_comes_from_the_agent_with_recent_process_cpu() {
    let server =
        super::installed_server("NEWPORT_TEST_SSH_PORT", "Disposable metrics", "fixture").await;
    // A busy loop under its own name: its comm is the link's name, so
    // `pkill -x` cleanup cannot match the shell running it.
    ssh::execute(
        &server,
        "ln -sf /bin/sh ~/newport-spin && setsid -f ~/newport-spin -c 'while :; do :; done' </dev/null >/dev/null 2>&1",
        None,
    )
    .await
    .unwrap();
    let mut busy = None;
    let mut reading = serde_json::Value::Null;
    // The agent samples every second; a few readings give it a baseline.
    for _ in 0..10 {
        reading = cockpit::collect(
            &crate::remote_tests::owner(&server),
            cockpit::Section::Overview,
        )
        .await
        .unwrap();
        busy = reading["processes"]
            .as_array()
            .unwrap()
            .iter()
            .position(|p| p["name"] == "newport-spin" && p["cpu"].as_f64().unwrap() > 50.0);
        if busy.is_some() {
            break;
        }
        sleep(Duration::from_millis(1200)).await;
    }
    let _ = ssh::execute(&server, "pkill -x newport-spin; rm -f ~/newport-spin", None).await;
    let cores = reading["cpus"].as_array().unwrap();
    // Each core carries its last seconds, one value per second, for the grid.
    assert!(
        cores.iter().all(
            |core| core["history"].as_array().is_some_and(|h| !h.is_empty()
                && h.len() <= 60
                && h.iter()
                    .all(|v| v.as_f64().is_some_and(|u| (0.0..=100.0).contains(&u))))
        )
    );
    assert_eq!(cores.len() as u64, reading["cores"].as_u64().unwrap());
    assert!(cores.iter().all(|core| {
        core["usage"]
            .as_f64()
            .is_some_and(|u| (0.0..=100.0).contains(&u))
    }));
    // A spinning process is near the top with most of a core.
    let position = busy.unwrap_or_else(|| panic!("no busy process in {:#}", reading["processes"]));
    assert!(position < 5, "{position}");
    assert!(reading["processes"][position]["user"] == "fixture");
    assert!(reading["memoryTotal"].as_u64().unwrap() > reading["memoryUsed"].as_u64().unwrap());
    assert!(
        reading["disks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["mount"] == "/")
    );
}

#[tokio::test]
#[ignore = "Requires npm run test:remote and real OpenSSH"]
async fn monitoring_requires_explicit_install_without_changing_shell_integration() {
    let mut server = server();
    server.agent_install_approved = false;
    // Start from a server without the agent or its integration shims.
    let state = "cd ~ && for f in .bashrc .profile .zshrc; do [ -e \"$f\" ] && sha256sum \"$f\"; done; \
                 for n in xclip wl-paste xdg-open newport-browser; do [ -L \".local/bin/$n\" ] && echo \"shim $n\"; done; true";
    ssh::execute(
        &server,
        "rm -f ~/.local/bin/newport-agent ~/.local/bin/xclip ~/.local/bin/wl-paste ~/.local/bin/xdg-open ~/.local/bin/newport-browser",
        None,
    )
    .await
    .unwrap();
    let before = ssh::execute(&server, state, None).await.unwrap();
    let error = cockpit::collect(
        &crate::remote_tests::owner(&server),
        cockpit::Section::Overview,
    )
    .await
    .unwrap_err();
    assert!(error.contains("not installed"), "{error}");
    ssh::execute(&server, "test ! -e ~/.local/bin/newport-agent", None)
        .await
        .unwrap();
    assert_eq!(ssh::execute(&server, state, None).await.unwrap(), before);

    // Onboarding grants consent explicitly. Monitoring itself must not deploy
    // binaries or rewrite shell profiles when merely opening the page.
    server.agent_install_approved = true;
    let session = ssh::ExecSession::connect(&server).await.unwrap();
    let installed = crate::agent_lifecycle::manage_test(&session, false, false).await;
    session.close().await;
    installed.unwrap();
    crate::monitor::retry_installation(&crate::remote_tests::owner(&server)).await;
    let reading = cockpit::collect(
        &crate::remote_tests::owner(&server),
        cockpit::Section::Overview,
    )
    .await
    .unwrap();
    assert!(reading["cpus"].as_array().is_some_and(|c| !c.is_empty()));
    // Sampling a server must never rewrite its shell profile or shadow its
    // xclip and xdg-open; those belong to the opt-in integration.
    let after = ssh::execute(&server, state, None).await.unwrap();
    assert_eq!(after, before);
    assert!(!after.contains("shim"), "{after}");
    assert_eq!(
        ssh::execute(
            &server,
            "test -x ~/.local/bin/newport-agent && echo deployed",
            None
        )
        .await
        .unwrap()
        .trim(),
        "deployed"
    );
}

#[tokio::test]
#[ignore = "Requires the disposable Linux OpenSSH fixture"]
async fn runtime_loss_recovers_monitor_without_caching_ten_minute_unavailability() {
    let registry = crate::server_session::Registry::default();
    let server = super::server();
    let owner = registry.session(&server, 0);
    owner
        .acquire(&server)
        .await
        .unwrap()
        .install(false, false)
        .await
        .unwrap();
    crate::monitor::snapshot(&owner).await.unwrap();
    let old_runtime = owner.agent_runtime.snapshot().hello.unwrap().instance_id;
    owner.agent_runtime.stop_remote_for_test().await;
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            if crate::monitor::snapshot(&owner).await.is_ok()
                && owner
                    .agent_runtime
                    .snapshot()
                    .hello
                    .as_ref()
                    .is_some_and(|hello| hello.instance_id != old_runtime)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("monitor must recover promptly instead of caching runtime loss as unavailable");
    assert_ne!(
        owner.agent_runtime.snapshot().hello.unwrap().instance_id,
        old_runtime
    );
    assert_eq!(owner.transport_snapshot().generation, 1);
    registry.shutdown().await;
}
