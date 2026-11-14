use super::*;
use crate::desktop_integration::DesktopIntegration;
use serde_json::{Value, json};
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
    async fn dispatch(
        &self,
        operation: &desktop::host::Operation,
    ) -> Result<desktop::host::Reply, Failure> {
        match operation {
            desktop::host::Operation::StartupSettings(_) => Ok(
                desktop::host::Reply::StartupSettings(Box::new(desktop::host::Startup {
                    enabled: false,
                    available: false,
                })),
            ),
            _ => Err(rpc::failure::operation("Unknown desktop command")),
        }
    }
}

async fn start(directory: &Path) -> Application {
    Application::start(
        Store::for_test(directory.into()),
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
        rpc::test_fixture::call(endpoint, method, args, Duration::from_secs(30))
    })
    .await
    .unwrap()
}

async fn snapshot(
    endpoint: &Path,
) -> Result<newport_protocol::desktop::configuration::Snapshot, String> {
    let endpoint = endpoint.to_owned();
    tokio::task::spawn_blocking(move || {
        rpc::test_fixture::snapshot(endpoint, Duration::from_secs(30))
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn terminal_rpc_dials_ssh_without_account_or_remote_apps_setup() {
    use newport_client::terminal_transport::Session;
    let _owner = BACKEND_OWNER.lock().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let directory = tempfile::tempdir().unwrap();
    let application = start(directory.path()).await;
    let id = Uuid::new_v4();
    call(
        application.endpoint(),
        "save_server",
        json!({"server":{
            "id":id,"name":"Implicit terminal RPC","sshHost":"127.0.0.1",
            "sshPort":listener.local_addr().unwrap().port(),"sshUser":"fixture"
        }}),
    )
    .await
    .unwrap();
    // Signed out, with no Remote apps setup: the terminal opens as ordinary
    // SSH, without a GUI error and without waiting for display preparation.
    let backend = newport_client::backend::Backend::at_path(application.endpoint().into());
    let session = Session::open_with_backend(backend, &id.to_string(), 80, 24).unwrap();
    tokio::time::timeout(Duration::from_secs(5), listener.accept())
        .await
        .expect("the terminal must dial SSH at once")
        .unwrap();
    assert!(
        session.try_read().is_err(),
        "no event before the shell starts"
    );
    assert!(!session.is_closed());
    drop(session);
    application.stop().await.unwrap();
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
        let snapshot = snapshot(&endpoint).await.unwrap();
        assert_eq!(snapshot.load_error, None);
        assert!(snapshot.config.servers.is_empty());
        assert!(Instance::acquire(&directory).unwrap().is_none());
        application.stop().await.unwrap();
        assert!(!endpoint.exists());
        assert!(Instance::acquire(&directory).unwrap().is_some());
    }
    std::fs::remove_dir(endpoint.parent().unwrap()).unwrap();
}

#[tokio::test]
async fn reservation_excludes_other_owners_before_any_service_starts_and_transfers_without_unlocking()
 {
    let _owner = BACKEND_OWNER.lock().await;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path();
    let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let reserve = || {
        let key = crate::profile::MasterKey::fixture(Counting(reads.clone()));
        let store = Store::open(path.into(), move |_| Ok(key)).unwrap();
        Application::reserve(store).unwrap()
    };
    let reservation = reserve().unwrap();
    assert!(reserve().is_none());
    for file in [
        "profiles.json",
        "preferences.json",
        "metrics.sqlite3",
        "native-client/desktop.sock",
    ] {
        assert!(!path.join(file).exists(), "Reservation initialized {file}");
    }
    drop(reservation);
    let reservation = reserve().unwrap();
    let application = reservation
        .start(Arc::new(DesktopFixture), DesktopFixture)
        .await
        .unwrap();
    // A profile with no passwords and no sign-in never reads credential storage.
    assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(reserve().is_none());
    let snapshot = snapshot(application.endpoint()).await.unwrap();
    assert_eq!(snapshot.load_error, None);
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
    let first = snapshot(&endpoint).await.unwrap();
    assert!(first.config.servers.is_empty());
    assert!(
        Application::start(
            Store::for_test(directory.path().into()),
            Arc::new(DesktopFixture),
            DesktopFixture
        )
        .await
        .unwrap()
        .is_none()
    );
    call(
        &endpoint,
        "set_window_preferences",
        json!({"preferences":{"sidebarWidth":260.0}}),
    )
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
    let saved = snapshot(&endpoint).await.unwrap();
    assert_eq!(saved.config.servers[0].id, id);
    assert!(!format!("{saved:?}").contains("fixture secret"));
    let stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("preferences.json")).unwrap())
            .unwrap();
    assert_eq!(stored["sidebarWidth"], 260.0);
    let receipts_endpoint = endpoint.clone();
    let receipts = tokio::task::spawn_blocking(move || {
        rpc::test_fixture::execute(
            receipts_endpoint,
            "git_pending_operations",
            json!({"serverId":id}),
            Duration::from_secs(30),
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(
        receipts,
        newport_protocol::desktop::Reply::Git(reply)
            if matches!(*reply, newport_protocol::desktop::git::Reply::PendingOperations(ref receipts) if receipts.receipts.is_empty())
    ));
    let startup_endpoint = endpoint.clone();
    let startup = tokio::task::spawn_blocking(move || {
        newport_client::backend::Backend::at_path(startup_endpoint).execute(
            newport_protocol::desktop::Operation::Host(Box::new(
                newport_protocol::desktop::host::Operation::StartupSettings(Box::new(
                    newport_protocol::common::Present {},
                )),
            )),
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(
        startup,
        newport_protocol::desktop::Reply::Host(reply)
            if matches!(&*reply, newport_protocol::desktop::host::Reply::StartupSettings(startup)
                if !startup.available)
    ));
    assert!(
        call(&endpoint, "activate_window", json!({}))
            .await
            .unwrap_err()
            .contains("Unknown desktop command")
    );
    let vault = std::fs::read(directory.path().join("profiles.json")).unwrap();
    let preferences = std::fs::read(directory.path().join("preferences.json")).unwrap();

    application.shutdown.take().unwrap().send(()).unwrap();
    application.wait().await.unwrap();
    application.wait().await.unwrap();
    application.stop().await.unwrap();
    assert!(crate::credentials::get(id).is_err());
    assert!(snapshot(&endpoint).await.is_err());
    let restarted = start(directory.path()).await;
    let second = snapshot(restarted.endpoint()).await.unwrap();
    assert_ne!(first.instance_id, second.instance_id);
    assert_eq!(second.config, saved.config);
    assert_eq!(&*crate::credentials::get(id).unwrap(), "fixture secret");
    assert_eq!(
        std::fs::read(directory.path().join("profiles.json")).unwrap(),
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
            if let Some(owner) = Instance::acquire(directory.path()).unwrap() {
                drop(owner);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(crate::credentials::get(id).is_err());

    // Profiles an earlier version saved, without their key, expose a
    // recoverable load error, never an empty writable profile.
    let earlier = tempfile::tempdir().unwrap();
    let path = earlier.path().join("profiles.stronghold");
    crate::profile::import::write_profiles_fixture(
        &path,
        &[9; 32],
        &json!({"schemaVersion":3,"config":{"servers":[server.clone()],"tunnels":[]}}),
    );
    let original = std::fs::read(&path).unwrap();
    let key = crate::profile::MasterKey::fixture(crate::profile::secrets::Memory::default());
    let store = Store::open(earlier.path().into(), |_| Ok(key)).unwrap();
    let locked = Application::start(store, Arc::new(DesktopFixture), DesktopFixture)
        .await
        .unwrap()
        .unwrap();
    let error = snapshot(locked.endpoint())
        .await
        .unwrap()
        .load_error
        .unwrap();
    assert!(error.contains("Cannot unlock saved profiles"), "{error}");
    assert!(
        call(locked.endpoint(), "save_server", json!({"server":server}))
            .await
            .unwrap_err()
            .contains("Cannot unlock saved profiles")
    );
    locked.stop().await.unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(!earlier.path().join("profiles.json").exists());
}

/// Credential storage that has nothing and counts every use.
struct Counting(Arc<std::sync::atomic::AtomicUsize>);
impl crate::profile::secrets::Secrets for Counting {
    fn read(&self, _: &str) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, String> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(None)
    }
    fn write(&self, _: &str, _: &[u8]) -> Result<(), String> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn delete(&self, _: &str) -> Result<(), String> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn invalid_preferences_abort_startup_without_claiming_credentials_or_replacing_data() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("preferences.json");
    std::fs::write(&path, "invalid").unwrap();
    let result = Application::start(
        Store::for_test(directory.path().into()),
        Arc::new(DesktopFixture),
        DesktopFixture,
    )
    .await;
    assert!(matches!(result, Err(error) if error.contains("Preferences could not be read")));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "invalid");
    assert!(Instance::acquire(directory.path()).unwrap().is_some());
    assert!(!directory.path().join("native-client/desktop.sock").exists());
}

mod remote_tests {
    use super::*;
    use newport_client::terminal_transport::{Event, Session};
    use newport_protocol::git::{Path as GitPath, Request};

    #[tokio::test]
    #[ignore = "Requires cargo xtask test remote and disposable OpenSSH"]
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
        let setup = crate::remote::Shell::new(&serde_json::from_value(server).unwrap());
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

        // Held for the rest of the test, as the app holds its terminals.
        let backend = newport_client::backend::Backend::at_path(endpoint.to_owned());
        let terminal = Session::open_with_backend(backend, &id.to_string(), 80, 24).unwrap();
        let mut output = Vec::new();
        let mut typed = false;
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                match terminal.try_read() {
                    Ok(Event::Data(bytes)) => output.extend(bytes),
                    // A plain terminal is its one computer's active tab.
                    Ok(Event::Ready | Event::Active(true)) => {}
                    Ok(event) => panic!("{event:?}"),
                    Err(_) => tokio::time::sleep(Duration::from_millis(5)).await,
                }
                if !typed && terminal.ready() {
                    terminal
                        .send(b"printf 'STANDALONE_%s\\n' READY\n".to_vec())
                        .unwrap();
                    typed = true;
                }
                if String::from_utf8_lossy(&output).contains("STANDALONE_READY") {
                    break;
                }
            }
        })
        .await
        .unwrap();
        // Leave terminal and Git sessions active: the application must close them.
        application.stop().await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !terminal.is_closed() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let restarted = start(directory.path()).await;
        let reopened = call(
            restarted.endpoint(),
            "git_request",
            json!({"serverId":id,"request":Request::Open { path: GitPath::new(root.as_bytes()) }}),
        )
        .await
        .unwrap();
        assert!(reopened["repoId"].is_string(), "{reopened}");
        restarted.stop().await.unwrap();
        setup
            .execute(&format!("rm -rf {root}"), None)
            .await
            .unwrap();
    }
}
