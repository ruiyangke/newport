//! Overview metrics through the agent on a real Linux server.
use super::server;
use crate::{cockpit, ssh};
use tokio::time::{sleep, Duration};

#[tokio::test]
#[ignore = "Requires npm run test:remote and real OpenSSH"]
async fn overview_comes_from_the_agent_with_recent_process_cpu() {
    let server = server();
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
        reading = cockpit::collect(&server, cockpit::Section::Overview)
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
    assert!(cores.iter().all(
        |core| core["history"].as_array().is_some_and(|h| !h.is_empty()
            && h.len() <= 60
            && h.iter()
                .all(|v| v.as_f64().is_some_and(|u| (0.0..=100.0).contains(&u))))
    ));
    assert_eq!(cores.len() as u64, reading["cores"].as_u64().unwrap());
    assert!(cores.iter().all(|core| core["usage"]
        .as_f64()
        .is_some_and(|u| (0.0..=100.0).contains(&u))));
    // A spinning process is near the top with most of a core.
    let position = busy.unwrap_or_else(|| panic!("no busy process in {:#}", reading["processes"]));
    assert!(position < 5, "{position}");
    assert!(reading["processes"][position]["user"] == "fixture");
    assert!(reading["memoryTotal"].as_u64().unwrap() > reading["memoryUsed"].as_u64().unwrap());
    assert!(reading["disks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["mount"] == "/"));
}

#[tokio::test]
#[ignore = "Requires npm run test:remote and real OpenSSH"]
async fn monitoring_deploys_only_the_agent_binary() {
    let server = server();
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
    let reading = cockpit::collect(&server, cockpit::Section::Overview)
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
