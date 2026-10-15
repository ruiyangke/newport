use super::*;
use crate::desktop_integration::DesktopIntegration;
use serde_json::json;
use std::time::Duration;
use uuid::Uuid;

// These tests each create the process's sole SSH credential owner. Isolated
// profile paths do not make that process-wide registration independent.
use crate::credentials::TEST_OWNER as BACKEND_OWNER;

struct DesktopFixture;
impl DesktopIntegration for DesktopFixture {
    fn on_main_thread(&self, _: Box<dyn FnOnce() + Send>) -> Result<(), String> {
        Err("No desktop in this fixture".into())
    }
    fn clipboard_revision(&self) -> Option<isize> {
        None
    }
    fn clipboard_offer(&self, _: Option<isize>) -> Option<(isize, Vec<String>)> {
        None
    }
    fn read_clipboard_format(&self, _: isize, _: &str) -> Option<Vec<u8>> {
        None
    }
    fn open_url(&self, _: &str) -> Result<(), String> {
        Err("No browser in this fixture".into())
    }
}
impl DesktopCommands for DesktopFixture {
    async fn dispatch(&self, method: &str, _: Value) -> Result<Value, Value> {
        if method == "get_startup_settings" {
            Ok(json!({"enabled":false,"available":false}))
        } else {
            Err(json!("Unknown desktop command"))
        }
    }
}

async fn start(directory: &Path) -> Application {
    Application::start(
        Store::for_test(directory.into()),
        true,
        Arc::new(DesktopFixture),
        DesktopFixture,
    )
    .await
    .unwrap()
    .unwrap()
}
async fn call(endpoint: &Path, method: &'static str, args: Value) -> Result<Value, String> {
    let endpoint = endpoint.to_owned();
    tokio::task::spawn_blocking(move || {
        newport_client::backend::Backend::at_path(endpoint).call_with_timeout(
            method,
            args,
            // This is a lifecycle/data-retention check, not a latency budget.
            // Parallel suites perform real Stronghold encryption and fsyncs;
            // allow bounded scheduling/IO contention without replaying writes.
            Duration::from_secs(30),
        )
    })
    .await
    .unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn long_profile_starts_restarts_and_preserves_single_instance_ownership() {
    let _owner = BACKEND_OWNER.lock().await;
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("long-profile-".repeat(12));
    std::fs::create_dir_all(&directory).unwrap();
    let endpoint = newport_platform::ipc::profile_endpoint(&directory);
    for _ in 0..2 {
        let application = start(&directory).await;
        assert_eq!(application.endpoint(), endpoint);
        let snapshot = call(&endpoint, "snapshot", json!({})).await.unwrap();
        assert_eq!(snapshot["loadError"], Value::Null);
        assert!(snapshot["config"]["servers"].as_array().unwrap().is_empty());
        assert!(Instance::acquire(&directory, true).unwrap().is_none());
        application.stop().await.unwrap();
        assert!(!endpoint.exists());
        assert!(Instance::acquire(&directory, true).unwrap().is_some());
    }
    std::fs::remove_dir(endpoint.parent().unwrap()).unwrap();
}

#[tokio::test]
async fn reservation_excludes_other_owners_before_any_service_starts_and_transfers_without_unlocking()
 {
    let _owner = BACKEND_OWNER.lock().await;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path();
    let key_reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let reserve = || {
        let reads = key_reads.clone();
        let store = Store::open(path.into(), move |_| {
            reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(zeroize::Zeroizing::new(vec![7; 32]))
        })
        .unwrap();
        Application::reserve(store, true).unwrap()
    };
    let reservation = reserve().unwrap();
    assert!(reserve().is_none());
    for file in [
        "profiles.stronghold",
        "preferences.json",
        "metrics.sqlite3",
        "native-client/desktop.sock",
    ] {
        assert!(!path.join(file).exists(), "Reservation initialized {file}");
    }
    drop(reservation);
    let reservation = reserve().unwrap();
    assert_eq!(key_reads.load(std::sync::atomic::Ordering::SeqCst), 0);
    let application = reservation
        .start(Arc::new(DesktopFixture), DesktopFixture)
        .await
        .unwrap();
    assert_eq!(key_reads.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(reserve().is_none());
    let snapshot = call(application.endpoint(), "snapshot", json!({}))
        .await
        .unwrap();
    assert_eq!(snapshot["loadError"], Value::Null);
    application.stop().await.unwrap();
    assert!(reserve().is_some());
}

#[tokio::test]
async fn standalone_start_dispatch_shutdown_and_restart_preserve_profile_and_release_ownership() {
    let _owner = BACKEND_OWNER.lock().await;
    let directory = tempfile::tempdir().unwrap();
    let id = Uuid::new_v4();
    let server = json!({"id":id,"name":"Standalone fixture","sshHost":"127.0.0.1","sshPort":9,"sshUser":"fixture"});
    let mut application = start(directory.path()).await;
    let endpoint = application.endpoint().to_owned();
    let first = call(&endpoint, "snapshot", json!({})).await.unwrap();
    assert!(first["config"]["servers"].as_array().unwrap().is_empty());
    assert!(
        Application::start(
            Store::for_test(directory.path().into()),
            true,
            Arc::new(DesktopFixture),
            DesktopFixture
        )
        .await
        .unwrap()
        .is_none()
    );
    call(&endpoint, "set_sidebar_width", json!({"width":260.0}))
        .await
        .unwrap();
    call(
        &endpoint,
        "save_server",
        json!({"server":server,"password":"fixture secret"}),
    )
    .await
    .unwrap();
    assert_eq!(&*crate::credentials::get(id).unwrap(), "fixture secret");
    assert!(
        crate::credentials::register(Arc::new(Store::for_test(directory.path().into()))).is_err()
    );
    let snapshot = call(&endpoint, "snapshot", json!({})).await.unwrap();
    assert_eq!(snapshot["config"]["servers"][0]["id"], id.to_string());
    assert!(!snapshot.to_string().contains("fixture secret"));
    assert_eq!(
        call(&endpoint, "get_sidebar_width", json!({}))
            .await
            .unwrap(),
        260.0
    );
    assert_eq!(
        call(&endpoint, "git_pending_operations", json!({"serverId":id}))
            .await
            .unwrap(),
        json!([])
    );
    assert_eq!(
        call(&endpoint, "get_startup_settings", json!({}))
            .await
            .unwrap()["available"],
        false
    );
    assert!(
        call(&endpoint, "unknown", json!({}))
            .await
            .unwrap_err()
            .contains("Unknown desktop command")
    );
    let vault = std::fs::read(directory.path().join("profiles.stronghold")).unwrap();
    let preferences = std::fs::read(directory.path().join("preferences.json")).unwrap();

    application.shutdown.take().unwrap().send(()).unwrap();
    application.wait().await.unwrap();
    application.wait().await.unwrap();
    application.stop().await.unwrap();
    assert!(crate::credentials::get(id).is_err());
    assert!(call(&endpoint, "snapshot", json!({})).await.is_err());
    let restarted = start(directory.path()).await;
    let second = call(restarted.endpoint(), "snapshot", json!({}))
        .await
        .unwrap();
    assert_ne!(first["instanceId"], second["instanceId"]);
    assert_eq!(second["config"], snapshot["config"]);
    assert_eq!(&*crate::credentials::get(id).unwrap(), "fixture secret");
    assert_eq!(
        std::fs::read(directory.path().join("profiles.stronghold")).unwrap(),
        vault
    );
    assert_eq!(
        std::fs::read(directory.path().join("preferences.json")).unwrap(),
        preferences
    );

    // Dropping the UI-facing handle still drains the backend before unlocking.
    drop(restarted);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(owner) = Instance::acquire(directory.path(), true).unwrap() {
                drop(owner);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(crate::credentials::get(id).is_err());

    // Unavailable credentials expose the existing recoverable load error, without
    // replacing the vault or silently falling back to an empty writable profile.
    let store = Store::open(directory.path().into(), |_| Err("Vault unavailable".into())).unwrap();
    let locked = Application::start(store, true, Arc::new(DesktopFixture), DesktopFixture)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        call(locked.endpoint(), "snapshot", json!({}))
            .await
            .unwrap()["loadError"],
        "Vault unavailable"
    );
    assert!(
        call(locked.endpoint(), "save_server", json!({"server":server}))
            .await
            .unwrap_err()
            .contains("Vault unavailable")
    );
    locked.stop().await.unwrap();
    assert_eq!(
        std::fs::read(directory.path().join("profiles.stronghold")).unwrap(),
        vault
    );
}

#[tokio::test]
async fn invalid_preferences_abort_startup_without_claiming_credentials_or_replacing_data() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("preferences.json");
    std::fs::write(&path, "invalid").unwrap();
    let result = Application::start(
        Store::for_test(directory.path().into()),
        true,
        Arc::new(DesktopFixture),
        DesktopFixture,
    )
    .await;
    assert!(matches!(result, Err(error) if error.contains("Preferences could not be read")));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "invalid");
    assert!(Instance::acquire(directory.path(), true).unwrap().is_some());
    assert!(!directory.path().join("native-client/desktop.sock").exists());
}

mod remote_tests {
    use super::*;
    use crate::git::protocol::{Path as GitPath, Request};

    #[tokio::test]
    #[ignore = "Requires npm run test:remote and disposable OpenSSH"]
    async fn standalone_services_reach_real_ssh_git_files_and_terminal_then_restart() {
        let _owner = BACKEND_OWNER.lock().await;
        let directory = tempfile::tempdir().unwrap();
        let id = Uuid::new_v4();
        let server = json!({
            "id":id,"name":"Standalone Linux fixture","sshHost":"127.0.0.1","sshUser":"fixture",
            "sshPort":std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
            "identityFile":std::env::var("NEWPORT_TEST_IDENTITY").unwrap()
        });
        let application = start(directory.path()).await;
        let endpoint = application.endpoint().to_owned();
        call(&endpoint, "save_server", json!({"server":server}))
            .await
            .unwrap();
        // Set up through the same explicit native routes as onboarding. Do not
        // rely on another test having deployed the agent into this container.
        let inspected = call(
            &endpoint,
            "setup_inspect",
            json!({"expectedConnection":server}),
        )
        .await
        .unwrap();
        assert!(inspected["total"].as_u64().is_some_and(|size| size > 0));
        call(
            &endpoint,
            "setup_install",
            json!({"expectedConnection":server}),
        )
        .await
        .unwrap();
        call(
            &endpoint,
            "setup_finish",
            json!({"expectedConnection":server,"browser":false,"clipboard":false}),
        )
        .await
        .unwrap();
        let listing = call(
            &endpoint,
            "files_list",
            json!({"id":id,"operation":Uuid::new_v4(),"path":"/home/fixture"}),
        )
        .await
        .unwrap();
        assert_eq!(listing["path"], "/home/fixture");
        let setup = crate::ssh::ExecSession::connect(&serde_json::from_value(server).unwrap())
            .await
            .unwrap();
        let root = format!("/home/fixture/standalone-{}", Uuid::new_v4());
        setup
            .execute(
                &format!(
                    "git init -b main {root} && printf 'standalone fixture' > {root}/sample.txt"
                ),
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            call(&endpoint, "git_connect", json!({"serverId":id}))
                .await
                .unwrap()["info"]["capabilities"]["backend"],
            "cli"
        );
        let opened = call(
            &endpoint,
            "git_request",
            json!({"serverId":id,"request":Request::Open { path: GitPath::new(root.as_bytes()) }}),
        )
        .await
        .unwrap();
        let status = call(&endpoint, "git_request", json!({"serverId":id,"request":Request::Status {
            repo_id: opened["repoId"].as_str().unwrap().into(), filter: None, page_size:100,cursor:None,
        }})).await.unwrap();
        assert!(status.to_string().contains("sample.txt"), "{status}");

        let session = Uuid::new_v4();
        call(
            &endpoint,
            "terminal_open",
            json!({"id":id,"session":session,"cols":80,"rows":24}),
        )
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let event = call(&endpoint, "terminal_read", json!({"session":session}))
                    .await
                    .unwrap();
                assert_ne!(event["type"], "error", "{event}");
                if event["type"] == "ready" {
                    break;
                }
            }
        })
        .await
        .unwrap();
        call(
            &endpoint,
            "terminal_write",
            json!({"session":session,"data":b"printf 'STANDALONE_%s\\n' READY\n".to_vec()}),
        )
        .await
        .unwrap();
        let mut output = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let event = call(&endpoint, "terminal_read", json!({"session":session}))
                    .await
                    .unwrap();
                assert_ne!(event["type"], "error", "{event}");
                if event["type"] == "data" {
                    output
                        .extend(serde_json::from_value::<Vec<u8>>(event["data"].clone()).unwrap());
                    if String::from_utf8_lossy(&output).contains("STANDALONE_READY") {
                        break;
                    }
                }
            }
        })
        .await
        .unwrap();
        // Leave terminal and Git sessions active: the application must close them.
        application.stop().await.unwrap();
        let restarted = start(directory.path()).await;
        assert!(
            call(
                restarted.endpoint(),
                "terminal_read",
                json!({"session":session})
            )
            .await
            .unwrap_err()
            .contains("Terminal is closed")
        );
        assert_eq!(
            call(restarted.endpoint(), "git_connect", json!({"serverId":id}))
                .await
                .unwrap()["info"]["capabilities"]["backend"],
            "cli"
        );
        restarted.stop().await.unwrap();
        setup
            .execute(&format!("rm -rf {root}"), None)
            .await
            .unwrap();
        setup.close().await;
    }
}
