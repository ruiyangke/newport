use super::*;
use crate::{
    manager::{Shared, share},
    model::Server,
    profile::Store,
};
use std::{
    sync::{
        Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize},
    },
    time::Duration,
};

struct Release(Arc<(Mutex<bool>, Condvar)>);
impl Release {
    fn new() -> Self {
        Self(Arc::new((Mutex::new(false), Condvar::new())))
    }
    fn release(&self) {
        *self.0.0.lock().unwrap() = true;
        self.0.1.notify_all();
    }
}
impl Drop for Release {
    fn drop(&mut self) {
        self.release();
    }
}
fn wait(gate: &Arc<(Mutex<bool>, Condvar)>) {
    drop(
        gate.1
            .wait_while(gate.0.lock().unwrap(), |released| !*released)
            .unwrap(),
    );
}
struct NoDesktop;
impl crate::desktop_integration::DesktopIntegration for NoDesktop {
    fn on_main_thread(&self, _: Box<dyn FnOnce() + Send>) -> Result<(), String> {
        panic!("no desktop work expected")
    }
    fn clipboard_revision(&self) -> Option<isize> {
        panic!("no desktop work expected")
    }
    fn clipboard_offer(&self, _: Option<isize>) -> Option<(isize, Vec<String>)> {
        panic!("no desktop work expected")
    }
    fn read_clipboard_format(&self, _: isize, _: &str) -> Option<Vec<u8>> {
        panic!("no desktop work expected")
    }
    fn open_url(&self, _: &str) -> Result<(), String> {
        panic!("no desktop work expected")
    }
}
async fn fixture() -> (tempfile::TempDir, Shared, Arc<Store>) {
    let directory = tempfile::tempdir().unwrap();
    let mut manager = Manager::new(Store::for_test(directory.path().into()));
    let servers: Vec<Server> = (0..2).map(|index| serde_json::from_value(serde_json::json!({
        "id":Uuid::new_v4(),"name":format!("Server {index}"),"sshHost":"unconnected.invalid","sshUser":"fixture","sshPort":22
    })).unwrap()).collect();
    let id = servers[0].id;
    manager
        .save_with_password(
            Config {
                servers,
                ..Default::default()
            },
            Some((id, zeroize::Zeroizing::new("fixture-secret".into()))),
        )
        .await
        .unwrap();
    let store = manager.store.clone();
    (directory, share(manager), store)
}

#[tokio::test]
async fn server_editor_rebases_without_blocking_another_server_or_overwriting_feature_consent() {
    let (_directory, state, store) = fixture().await;
    let mut server = state.snapshot().config.servers[0].clone();
    let owner = state.owner(server.id).unwrap();
    let release = Release::new();
    let gate = release.0.clone();
    let once = AtomicBool::new(false);
    let (started, entered) = tokio::sync::oneshot::channel();
    let started = Mutex::new(Some(started));
    *store.preparation_hook.lock().unwrap() = Some(Arc::new(move |config| {
        if config.servers[0].name == "Edited" && !once.swap(true, Ordering::AcqRel) {
            started.lock().unwrap().take().unwrap().send(()).unwrap();
            wait(&gate);
        }
    }));
    server.name = "Edited".into();
    let first = state.clone();
    let task = tokio::spawn(async move {
        crate::commands::save_server(
            &Default::default(),
            &Default::default(),
            &first,
            Arc::new(NoDesktop),
            server,
            None,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .unwrap()
        .unwrap();
    let history = tokio::time::timeout(
        Duration::from_millis(200),
        crate::manager::server_history_guard(&state, owner.server().id),
    )
    .await
    .expect("metadata encryption must not block this server's metrics history");
    drop(history);
    let mut other = state.snapshot().config.servers[1].clone();
    other.name = "Other edited".into();
    crate::commands::save_server(
        &Default::default(),
        &Default::default(),
        &state,
        Arc::new(NoDesktop),
        other,
        None,
    )
    .await
    .unwrap();
    state
        .edit_configuration(None, |manager| {
            let mut config = manager.config.clone();
            config.servers[0].browser_enabled = true;
            config.servers[0].agent_install_approved = true;
            config.servers[0].setup_complete = true;
            Ok((config, ()))
        })
        .await
        .unwrap();
    assert!(!task.is_finished());
    assert!(state.try_lock().is_ok());
    release.release();
    task.await.unwrap().unwrap();
    let saved = store.load().unwrap();
    assert_eq!(saved.servers[0].name, "Edited");
    assert_eq!(saved.servers[1].name, "Other edited");
    assert!(saved.servers[0].browser_enabled);
    assert!(saved.servers[0].agent_install_approved);
    assert!(saved.servers[0].setup_complete);
    assert!(Arc::ptr_eq(
        &owner,
        &state.owner(saved.servers[0].id).unwrap()
    ));
    assert_eq!(
        store.password(saved.servers[0].id).unwrap().as_str(),
        "fixture-secret"
    );
    assert_eq!(saved, state.snapshot().config);
    let unaffected = state.owner(saved.servers[1].id).unwrap();
    crate::commands::save_server(
        &Default::default(),
        &Default::default(),
        &state,
        Arc::new(NoDesktop),
        saved.servers[0].clone(),
        Some("replacement-secret".into()),
    )
    .await
    .unwrap();
    assert!(owner.is_revoked());
    assert!(Arc::ptr_eq(
        &unaffected,
        &state.owner(saved.servers[1].id).unwrap()
    ));
    let replaced = store.load().unwrap();
    assert!(!replaced.servers[0].agent_install_approved);
    assert!(!replaced.servers[0].setup_complete);
    assert!(!replaced.servers[0].browser_enabled);
    assert_eq!(
        store.password(saved.servers[0].id).unwrap().as_str(),
        "replacement-secret"
    );
    crate::manager::shutdown(&state).await;
}

#[tokio::test]
async fn deleting_a_server_during_editor_preparation_does_not_resurrect_it() {
    let (_directory, state, store) = fixture().await;
    let mut server = state.snapshot().config.servers[0].clone();
    let id = server.id;
    server.name = "Obsolete editor".into();
    let release = Release::new();
    let gate = release.0.clone();
    let (started, entered) = tokio::sync::oneshot::channel();
    let started = Mutex::new(Some(started));
    *store.preparation_hook.lock().unwrap() = Some(Arc::new(move |config| {
        if config
            .servers
            .iter()
            .any(|server| server.name == "Obsolete editor")
        {
            started.lock().unwrap().take().unwrap().send(()).unwrap();
            wait(&gate);
        }
    }));
    let editing = state.clone();
    let task = tokio::spawn(async move {
        crate::commands::save_server(
            &Default::default(),
            &Default::default(),
            &editing,
            Arc::new(NoDesktop),
            server,
            None,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .unwrap()
        .unwrap();
    let db = crate::metrics_db::MetricsDb::open(
        &_directory.path().join("metrics.sqlite"),
        Ok(vec![7; 32]),
    )
    .await
    .unwrap();
    let preferences = crate::preferences::Preferences::open(_directory.path()).unwrap();
    crate::commands::delete_server(
        &Default::default(),
        &Default::default(),
        &state,
        &db,
        &preferences,
        id,
    )
    .await
    .unwrap();
    assert!(!task.is_finished());
    release.release();
    assert!(
        task.await
            .unwrap()
            .unwrap_err()
            .contains("Server connection changed")
    );
    assert!(state.owner(id).is_err());
    assert!(
        !store
            .load()
            .unwrap()
            .servers
            .iter()
            .any(|server| server.id == id)
    );
    db.close().await;
    crate::manager::shutdown(&state).await;
}

#[tokio::test]
async fn encryption_does_not_queue_other_servers_or_features_and_rebase_preserves_both_edits() {
    for same_server in [false, true] {
        let (_directory, state, store) = fixture().await;
        let release = Release::new();
        let gate = release.0.clone();
        let (started, entered) = tokio::sync::oneshot::channel();
        let started = Mutex::new(Some(started));
        let once = AtomicBool::new(false);
        *store.preparation_hook.lock().unwrap() = Some(Arc::new(move |config| {
            if config.servers[0].browser_enabled && !once.swap(true, Ordering::AcqRel) {
                started.lock().unwrap().take().unwrap().send(()).unwrap();
                wait(&gate);
            }
        }));
        let first = state.clone();
        let task = tokio::spawn(async move {
            first
                .edit_configuration(None, |manager| {
                    let mut config = manager.config.clone();
                    config.servers[0].browser_enabled = true;
                    Ok((config, ()))
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), entered)
            .await
            .unwrap()
            .unwrap();
        assert!(
            state.try_lock().is_ok(),
            "encryption must not own configuration control"
        );
        let before = state.snapshot();
        assert!(!before.config.servers[0].browser_enabled);
        let other = usize::from(!same_server);
        state
            .edit_configuration(None, move |manager| {
                let mut config = manager.config.clone();
                config.servers[other].clipboard_enabled = true;
                Ok((config, ()))
            })
            .await
            .unwrap();
        assert!(!task.is_finished(), "first preparation remains held");
        assert!(state.snapshot().config.servers[other].clipboard_enabled);
        release.release();
        task.await.unwrap().unwrap();
        let saved = store.load().unwrap();
        assert!(saved.servers[0].browser_enabled);
        assert!(saved.servers[other].clipboard_enabled);
        assert_eq!(saved, state.snapshot().config);
        assert_eq!(
            store.password(saved.servers[0].id).unwrap().as_str(),
            "fixture-secret"
        );
        crate::manager::shutdown(&state).await;
    }
}

#[tokio::test]
async fn prepared_edits_cannot_commit_after_owner_retirement_or_shutdown() {
    for shutdown in [false, true] {
        let (_directory, state, store) = fixture().await;
        let original = state.snapshot().config;
        let owner = state.owner(original.servers[0].id).unwrap();
        let release = Release::new();
        let gate = release.0.clone();
        let (started, entered) = tokio::sync::oneshot::channel();
        let started = Mutex::new(Some(started));
        *store.preparation_hook.lock().unwrap() = Some(Arc::new(move |_| {
            if let Some(started) = started.lock().unwrap().take() {
                started.send(()).unwrap();
                wait(&gate);
            }
        }));
        let first = state.clone();
        let task = tokio::spawn(async move {
            first
                .edit_configuration(None, move |manager| {
                    if owner.is_revoked() {
                        return Err("captured connection retired".into());
                    }
                    let mut config = manager.config.clone();
                    config.servers[0].browser_enabled = true;
                    Ok((config, ()))
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), entered)
            .await
            .unwrap()
            .unwrap();
        if shutdown {
            crate::manager::shutdown(&state).await;
        } else {
            state
                .lock()
                .await
                .advance_connection_revision(original.servers[0].id);
        }
        release.release();
        assert!(task.await.unwrap().is_err());
        assert_eq!(store.load().unwrap(), original);
        assert_eq!(state.snapshot().config, original);
        if !shutdown {
            crate::manager::shutdown(&state).await;
        }
    }
}

#[tokio::test]
async fn cancelling_the_response_during_commit_cannot_leave_memory_behind_disk() {
    let (_directory, state, store) = fixture().await;
    let release = Release::new();
    let gate = release.0.clone();
    let calls = AtomicUsize::new(0);
    let (started, entered) = tokio::sync::oneshot::channel();
    let started = Mutex::new(Some(started));
    let first = state.clone();
    let (cleanup, registered) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        first
            .edit_configuration_with_commit(
                None,
                move |manager| {
                    if calls.fetch_add(1, Ordering::AcqRel) == 1 {
                        started.lock().unwrap().take().unwrap().send(()).unwrap();
                        wait(&gate);
                    }
                    let mut config = manager.config.clone();
                    config.servers[0].name = "Committed after caller closed".into();
                    Ok((config, ()))
                },
                move |_, ()| {
                    cleanup.send(()).unwrap();
                },
            )
            .await
    });
    entered.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    release.release();
    tokio::time::timeout(Duration::from_secs(5), registered)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if state.snapshot().config.servers[0].name == "Committed after caller closed" {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(store.load().unwrap(), state.snapshot().config);
    crate::manager::shutdown(&state).await;
}
