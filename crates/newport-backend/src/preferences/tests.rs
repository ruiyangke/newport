use super::*;
use crate::git::ClientIdentity;
use projects::{Project, git_projects_list, git_projects_remove, git_projects_save};
use uuid::Uuid;

struct NativeHost(Preferences);

fn sidebar_width(state: &Preferences) -> Option<f64> {
    state.get("sidebarWidth").unwrap().and_then(|v| v.as_f64())
}
async fn set_sidebar_width(state: &Preferences, width: f64) -> Result<(), String> {
    set_window_preferences(state, json!({ "sidebarWidth": width })).await
}

#[test]
fn failed_atomic_publication_preserves_previous_file_and_cleans_temporary_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("preferences.json");
    let backup = directory.path().join("previous.json");
    let original = br#"{"sidebarWidth":220,"preserve":"original"}"#;
    fs::write(&path, original).unwrap();
    let state = Preferences::open(directory.path()).unwrap();
    let result = state.update(|values| {
        values.insert("sidebarWidth".into(), json!(250));
        // Make publication fail after the old JSON has been read. This models
        // an OS replace failure without permission assumptions about test users.
        fs::rename(&path, &backup).unwrap();
        fs::create_dir(&path).unwrap();
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(fs::read(&backup).unwrap(), original);
    fs::remove_dir(&path).unwrap();
    fs::rename(&backup, &path).unwrap();
    assert_eq!(sidebar_width(&state), Some(220.));
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}
impl crate::rpc::Host for NativeHost {
    async fn dispatch(&self, method: &str, args: Value) -> Result<Value, Value> {
        crate::rpc::preferences::Command::parse(method)
            .ok_or_else(|| json!("Unknown command"))?
            .dispatch(&self.0, &args)
            .await
    }
    async fn open_terminal(&self, _: crate::rpc::TerminalOpen) -> Result<Box<dyn Send>, String> {
        Err("Not a terminal fixture".into())
    }
}

#[tokio::test]
async fn native_rpc_edits_and_reopens_preferences() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint = directory.path().join("native-client/desktop.sock");
    let state = Preferences::open(directory.path()).unwrap();
    assert_eq!(sidebar_width(&state), None);
    let listener = crate::rpc::Server::bind(&endpoint).unwrap();
    let task = tokio::spawn(listener.run(NativeHost(state)));
    let server = Uuid::new_v4();
    let bookmark = serde_json::to_value(project(server, b"/native/repo\xff")).unwrap();
    let expected = bookmark.clone();
    tokio::task::spawn_blocking(move || {
        let client = newport_client::backend::Backend::at_path(endpoint);
        client
            .call(
                "set_window_preferences",
                json!({"preferences":{"sidebarWidth":260}}),
            )
            .unwrap();
        assert_eq!(
            client
                .call("git_projects_save", json!({"project":bookmark}))
                .unwrap(),
            bookmark
        );
        assert_eq!(
            client
                .call("git_projects_list", json!({"serverId":server}))
                .unwrap(),
            json!([bookmark])
        );
        assert!(
            client
                .call(
                    "set_window_preferences",
                    json!({"preferences":{"sidebarWidth":999}}),
                )
                .is_err()
        );
        client
            .call(
                "git_projects_remove",
                json!({"serverId":Uuid::new_v4(),"projectId":bookmark["id"]}),
            )
            .unwrap();
        assert_eq!(
            client
                .call("git_projects_list", json!({"serverId":server}))
                .unwrap(),
            json!([bookmark])
        );
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let reopened = Preferences::open(directory.path()).unwrap();
    assert_eq!(sidebar_width(&reopened), Some(260.));
    assert_eq!(
        serde_json::to_value(git_projects_list(&reopened, server).await.unwrap()).unwrap(),
        json!([expected])
    );
}

fn project(server: Uuid, path: &[u8]) -> Project {
    serde_json::from_value(json!({
        "id":Uuid::new_v4(),"serverId":server,"name":"Fixture",
        "path":crate::git::protocol::Path::new(path)
    }))
    .unwrap()
}

#[tokio::test]
async fn existing_identity_unknown_preferences_and_binary_project_paths_survive_edits() {
    let directory = tempfile::tempdir().unwrap();
    let id = Uuid::new_v4().to_string();
    let original = json!({"gitClientId":id,"sidebarWidth":222,"futureSetting":{"enabled":true}});
    let path = directory.path().join("preferences.json");
    fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
    let before = fs::read(&path).unwrap();
    let state = Preferences::open(directory.path()).unwrap();
    assert_eq!(sidebar_width(&state), Some(222.));
    assert_eq!(fs::read(&path).unwrap(), before, "Opening is read-only");
    assert_eq!(state.git_client_id().await.unwrap(), id);
    assert_eq!(
        fs::read(&path).unwrap(),
        before,
        "Reading an existing identity is read-only"
    );
    let server = Uuid::new_v4();
    let bookmark = project(server, b"/fixture/project\xff");
    git_projects_save(&state, bookmark.clone()).await.unwrap();
    set_sidebar_width(&state, 245.).await.unwrap();
    assert_eq!(state.git_client_id().await.unwrap(), id);
    let reopened = Preferences::open(directory.path()).unwrap();
    assert_eq!(reopened.git_client_id().await.unwrap(), id);
    assert_eq!(sidebar_width(&reopened), Some(245.));
    assert_eq!(
        serde_json::to_value(git_projects_list(&reopened, server).await.unwrap()).unwrap(),
        json!([bookmark])
    );
    assert!(
        git_projects_list(&reopened, Uuid::new_v4())
            .await
            .unwrap()
            .is_empty()
    );
    let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["futureSetting"], original["futureSetting"]);
}

#[tokio::test]
async fn concurrent_edits_preserve_every_project_and_one_git_identity() {
    let directory = tempfile::tempdir().unwrap();
    let state = Preferences::open(directory.path()).unwrap();
    let server = Uuid::new_v4();
    let mut tasks = Vec::new();
    for i in 0..12 {
        let state = state.clone();
        tasks.push(tokio::spawn(async move {
            git_projects_save(&state, project(server, format!("/fixture/{i}").as_bytes()))
                .await
                .unwrap();
            set_sidebar_width(&state, 176. + i as f64).await.unwrap();
            state.git_client_id().await.unwrap()
        }));
    }
    let mut ids = std::collections::HashSet::new();
    for task in tasks {
        ids.insert(task.await.unwrap());
    }
    assert_eq!(ids.len(), 1);
    assert_eq!(git_projects_list(&state, server).await.unwrap().len(), 12);
    let reopened = Preferences::open(directory.path()).unwrap();
    assert!(ids.contains(&reopened.git_client_id().await.unwrap()));
    assert_eq!(
        git_projects_list(&reopened, server).await.unwrap().len(),
        12
    );
}

#[tokio::test]
async fn corrupt_files_and_identity_are_not_reset_or_overwritten() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("preferences.json");
    let state = Preferences::open(directory.path()).unwrap();
    for malformed in ["{", "[]", "null"] {
        fs::write(&path, malformed).unwrap();
        assert!(Preferences::open(directory.path()).is_err());
        assert!(set_sidebar_width(&state, 220.).await.is_err());
        assert!(state.git_client_id().await.is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), malformed);
    }
    fs::write(&path, r#"{"gitClientId":"invalid","gitProjects":{}}"#).unwrap();
    let before = fs::read(&path).unwrap();
    assert!(
        state
            .git_client_id()
            .await
            .unwrap_err()
            .contains("identity is invalid")
    );
    assert!(
        git_projects_save(&state, project(Uuid::new_v4(), b"/fixture"))
            .await
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[tokio::test]
async fn failed_save_never_exposes_a_new_identity_or_unsaved_preference() {
    let directory = tempfile::tempdir().unwrap();
    let parent = directory.path().join("profile");
    let state = Preferences::open(&parent).unwrap();
    // Parent path occupied by a file: creation cannot publish any preferences.
    fs::write(&parent, "do not replace").unwrap();
    assert!(state.git_client_id().await.is_err());
    assert!(set_sidebar_width(&state, 220.).await.is_err());
    assert!(
        git_projects_save(&state, project(Uuid::new_v4(), b"/fixture"))
            .await
            .is_err()
    );
    assert_eq!(fs::read_to_string(&parent).unwrap(), "do not replace");
    fs::remove_file(&parent).unwrap();
    assert_eq!(sidebar_width(&state), None);
    assert!(
        git_projects_list(&state, Uuid::new_v4())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(state.get("gitClientId").unwrap().is_none());
    assert!(state.git_identity.get().is_none());
    let id = state.git_client_id().await.unwrap();
    assert_eq!(state.get("gitClientId").unwrap(), Some(json!(id)));
}

#[tokio::test]
async fn rejected_changes_preserve_bytes_and_removal_is_server_scoped() {
    let directory = tempfile::tempdir().unwrap();
    let state = Preferences::open(directory.path()).unwrap();
    let server = Uuid::new_v4();
    let bookmark = project(server, b"/fixture");
    let id = serde_json::to_value(&bookmark).unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    git_projects_save(&state, bookmark.clone()).await.unwrap();
    let path = directory.path().join("preferences.json");
    let before = fs::read(&path).unwrap();
    for width in [175., 301., f64::NAN, f64::INFINITY] {
        assert!(set_sidebar_width(&state, width).await.is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    assert!(
        git_projects_save(&state, project(server, b"/fixture"))
            .await
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    git_projects_remove(&state, Uuid::new_v4(), id)
        .await
        .unwrap();
    assert_eq!(git_projects_list(&state, server).await.unwrap().len(), 1);
    git_projects_remove(&state, server, id).await.unwrap();
    assert!(git_projects_list(&state, server).await.unwrap().is_empty());
}

// A competing preference transaction held on a different thread. The timeout
// prevents a broken synchronous implementation from hanging the test runtime.
struct StorageGate {
    release: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl StorageGate {
    async fn hold(state: &Preferences) -> Self {
        let access = state.access.clone();
        let (ready, started) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let _guard = access.lock().unwrap();
            let _ = ready.send(());
            let _ = wait.recv_timeout(std::time::Duration::from_secs(5));
        });
        started.await.unwrap();
        Self {
            release: Some(release),
            thread: Some(thread),
        }
    }
}
impl Drop for StorageGate {
    fn drop(&mut self) {
        let _ = self.release.take().unwrap().send(());
        self.thread.take().unwrap().join().unwrap();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cold_identity_storage_does_not_block_executor_or_another_profile() {
    let directory = tempfile::tempdir().unwrap();
    let state = Preferences::open(&directory.path().join("slow")).unwrap();
    let other = Preferences::open(&directory.path().join("other")).unwrap();
    let gate = StorageGate::hold(&state).await;
    let mut identity = state.git_client_id();
    // Poll the real production lookup while storage is unavailable. Its first
    // poll must yield, leaving the only Tokio thread free for other requests.
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(identity.as_mut().poll(cx).is_pending()))
            .await
    );
    assert!(state.git_identity.get().is_none());
    let other_id = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        set_sidebar_width(&other, 230.).await.unwrap();
        other.git_client_id().await.unwrap()
    })
    .await
    .unwrap();
    assert!(!other_id.is_empty());
    assert!(state.git_identity.get().is_none());
    drop(gate);
    let id = identity.await.unwrap();
    assert_ne!(id, other_id);
    assert_eq!(state.get("gitClientId").unwrap(), Some(json!(id)));
}

#[tokio::test(flavor = "current_thread")]
async fn cached_identity_bypasses_storage_even_during_an_unrelated_preference_write() {
    let directory = tempfile::tempdir().unwrap();
    let state = Preferences::open(directory.path()).unwrap();
    let id = state.git_client_id().await.unwrap();
    let clone = state.clone();
    let _gate = StorageGate::hold(&state).await;
    let mut identity = clone.git_client_id();
    let result =
        std::future::poll_fn(|cx| std::task::Poll::Ready(identity.as_mut().poll(cx))).await;
    assert!(matches!(result, std::task::Poll::Ready(Ok(value)) if value == id));
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_identity_waiter_still_persists_one_identity_before_caching() {
    let directory = tempfile::tempdir().unwrap();
    let state = Preferences::open(directory.path()).unwrap();
    let gate = StorageGate::hold(&state).await;
    let mut identity = state.git_client_id();
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(identity.as_mut().poll(cx).is_pending()))
            .await
    );
    drop(identity);
    assert!(state.git_identity.get().is_none());
    assert!(!state.path.exists());
    drop(gate);
    // The owned persistence worker survives a cancelled Git caller; no second
    // request is needed to finish publishing its durable result.
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while state.git_identity.get().is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let persisted = state.get("gitClientId").unwrap().unwrap();
    assert_eq!(json!(state.git_client_id().await.unwrap()), persisted);
    let reopened = Preferences::open(directory.path()).unwrap();
    assert_eq!(json!(reopened.git_client_id().await.unwrap()), persisted);
}

#[tokio::test]
async fn window_preferences_merge_validate_and_keep_other_keys() {
    let directory = tempfile::tempdir().unwrap();
    let state = Preferences::open(directory.path()).unwrap();
    set_sidebar_width(&state, 245.).await.unwrap();
    crate::preferences::set_window_preferences(
        &state,
        json!({"sidebarCompact": true, "appearance": "dark", "terminalOptionAsMeta": false}),
    )
    .await
    .unwrap();
    let stored: Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("preferences.json")).unwrap())
            .unwrap();
    assert_eq!(
        stored,
        json!({"sidebarWidth": 245.0, "sidebarCompact": true, "appearance": "dark",
            "terminalOptionAsMeta": false})
    );
    for invalid in [
        json!({"appearance": "blue"}),
        json!({"sidebarWidth": 999}),
        json!({"sidebarCompact": "yes"}),
        json!({"terminalOptionAsMeta": "yes"}),
        json!({"unknown": 1}),
        json!([]),
    ] {
        assert!(
            crate::preferences::set_window_preferences(&state, invalid.clone())
                .await
                .is_err(),
            "{invalid}"
        );
    }
}

#[tokio::test]
async fn a_deleted_servers_entries_go_and_other_servers_keep_theirs() {
    let directory = tempfile::tempdir().unwrap();
    let state = Preferences::open(directory.path()).unwrap();
    let (gone, kept) = (Uuid::new_v4(), Uuid::new_v4());
    for server in [gone, kept] {
        let project: Project = serde_json::from_value(json!({
            "id": Uuid::new_v4(),
            "serverId": server,
            "name": "Example",
            "path": {"display": "/home/user/project", "bytesB64": "L2hvbWUvdXNlci9wcm9qZWN0"},
        }))
        .unwrap();
        git_projects_save(&state, project).await.unwrap();
        remote_apps::remote_apps_installed_save(&state, server, true)
            .await
            .unwrap();
    }
    forget_server(&state, gone).await.unwrap();
    assert!(git_projects_list(&state, gone).await.unwrap().is_empty());
    assert_eq!(git_projects_list(&state, kept).await.unwrap().len(), 1);
    assert_eq!(
        remote_apps::remote_apps_installed(&state).await.unwrap(),
        [kept]
    );
}
