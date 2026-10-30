use super::*;
use crate::{
    commands,
    manager::{Manager, share},
    profile::Store,
};
use serde_json::json;
use tokio::sync::{mpsc, oneshot};

type Calls = mpsc::UnboundedReceiver<oneshot::Sender<Result<Arc<Value>, String>>>;
struct Fixture {
    _directory: tempfile::TempDir,
    state: Shared,
    db: MetricsDb,
    servers: [Server; 2],
}
impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(Store::for_test(directory.path().into()));
        let servers = ["A", "B"].map(|name| {
            serde_json::from_value(json!({
                "id":Uuid::new_v4(), "name":name, "sshHost":"unconnected.invalid",
                "sshPort":22, "sshUser":"fixture"
            }))
            .unwrap()
        });
        manager.config.servers = servers.to_vec();
        let db = MetricsDb::open(&directory.path().join("metrics.db"), Ok(vec![7; 32]))
            .await
            .unwrap();
        Self {
            _directory: directory,
            state: share(manager),
            db,
            servers,
        }
    }
    fn install(&self, index: usize) -> (Arc<Reading>, Calls) {
        let profile = &self.servers[index];
        let owner = self.state.owner(profile.id).unwrap();
        let (calls, incoming) = mpsc::unbounded_channel();
        let reading = owner.metrics.get_or_insert(profile, || Reading {
            profile: profile.clone(),
            cancelled: Arc::new(AtomicBool::new(false)),
            retry: Arc::new(AtomicBool::new(false)),
            query: Query::new(policy(), move || {
                let calls = calls.clone();
                async move {
                    let (reply, wait) = oneshot::channel();
                    calls.send(reply).map_err(|_| "Fixture closed")?;
                    wait.await.map_err(|_| "Fixture reply dropped")?
                }
            }),
        });
        (reading, incoming)
    }
    fn request(&self, index: usize, force: bool) -> tokio::task::JoinHandle<Result<Value, String>> {
        let (state, db, id) = (self.state.clone(), self.db.clone(), self.servers[index].id);
        tokio::spawn(async move {
            commands::cockpit_collect(&state, &db, Some(force), id, cockpit::Section::Overview)
                .await
        })
    }
    async fn stop(self) {
        crate::manager::shutdown(&self.state).await;
        self.db.close().await;
    }
}
async fn call(calls: &mut Calls) -> oneshot::Sender<Result<Arc<Value>, String>> {
    tokio::time::timeout(Duration::from_secs(1), calls.recv())
        .await
        .unwrap()
        .unwrap()
}
fn answer(reply: oneshot::Sender<Result<Arc<Value>, String>>, data: Value) {
    reply.send(Ok(Arc::new(data))).unwrap();
}
async fn settle(reading: &Reading) {
    let mut changes = reading.query.subscribe();
    tokio::time::timeout(Duration::from_secs(1), async {
        while reading.query.snapshot().fetching {
            changes.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn first_read_manual_refresh_and_background_share_work_without_blocking_other_server_or_writer()
 {
    let f = Fixture::new().await;
    let (a, mut a_calls) = f.install(0);
    let (_b, mut b_calls) = f.install(1);
    let writer = f.state.lock().await;
    let background = tokio::spawn(run(f.state.clone(), f.db.clone()));
    let a_reply = call(&mut a_calls).await;
    let b_reply = call(&mut b_calls).await;
    let abandoned = f.request(0, false);
    let refreshed = f.request(0, true);
    let independent = f.request(1, false);
    tokio::task::yield_now().await;
    assert!(!refreshed.is_finished());
    abandoned.abort();
    assert!(abandoned.await.unwrap_err().is_cancelled());
    answer(b_reply, json!({"cpu":2}));
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), independent)
            .await
            .unwrap()
            .unwrap()
            .unwrap()["cpu"],
        2
    );
    answer(a_reply, json!({"cpu":1}));
    assert_eq!(refreshed.await.unwrap().unwrap()["cpu"], 1);
    assert_eq!(f.request(0, false).await.unwrap().unwrap()["cpu"], 1);
    assert!(a_calls.try_recv().is_err());
    background.abort();
    assert!(background.await.unwrap_err().is_cancelled());
    assert!(a.cancelled.load(Ordering::Acquire));
    assert!(a.query.snapshot().data.is_none());
    drop(writer);
    f.stop().await;
}

#[tokio::test]
async fn last_good_reading_retains_timestamp_and_exposes_error_until_recovery() {
    let f = Fixture::new().await;
    let (reading, mut calls) = f.install(0);
    let first = f.request(0, false);
    answer(call(&mut calls).await, json!({"cpu":42,"sampledAt":123}));
    assert_eq!(first.await.unwrap().unwrap()["sampledAt"], 123);
    let good = reading.query.snapshot();
    let failed = f.request(0, true);
    call(&mut calls).await.send(Err("Offline".into())).unwrap();
    assert_eq!(failed.await.unwrap().unwrap_err(), "Offline");
    let retained = f.request(0, false).await.unwrap().unwrap();
    assert_eq!(retained["sampledAt"], 123);
    assert_eq!(retained["collectionError"], "Offline");
    assert_eq!(reading.query.snapshot().updated_at, good.updated_at);
    assert_eq!(reading.query.snapshot().revision, good.revision);
    let recovered = f.request(0, true);
    answer(call(&mut calls).await, json!({"cpu":12,"sampledAt":456}));
    assert!(
        recovered
            .await
            .unwrap()
            .unwrap()
            .get("collectionError")
            .is_none()
    );
    assert_eq!(
        f.request(0, false).await.unwrap().unwrap()["sampledAt"],
        456
    );
    f.stop().await;
}

#[tokio::test]
async fn metadata_preserves_cache_but_setup_credentials_and_deletion_retire_it() {
    let f = Fixture::new().await;
    let (original, mut calls) = f.install(0);
    let first = f.request(0, false);
    answer(call(&mut calls).await, json!({"cpu":1}));
    first.await.unwrap().unwrap();
    let id = f.servers[0].id;
    let owner = f.state.owner(id).unwrap();
    {
        let mut writer = f.state.lock().await;
        writer.config.servers[0].name = "renamed".into();
        writer.config.servers[0].clipboard_enabled = true;
    }
    assert!(Arc::ptr_eq(&original, &query(&f.state, &f.db, id).unwrap()));
    for flag in ["approval", "setup"] {
        let previous = query(&f.state, &f.db, id).unwrap();
        {
            let mut writer = f.state.lock().await;
            if flag == "approval" {
                writer.config.servers[0].agent_install_approved = true;
            } else {
                writer.config.servers[0].setup_complete = true;
            }
        }
        assert!(previous.cancelled.load(Ordering::Acquire));
        assert!(previous.query.snapshot().data.is_none());
        assert!(Arc::ptr_eq(&owner, &f.state.owner(id).unwrap()));
        assert!(!Arc::ptr_eq(
            &previous,
            &query(&f.state, &f.db, id).unwrap()
        ));
    }
    let previous = query(&f.state, &f.db, id).unwrap();
    f.state.lock().await.advance_connection_revision(id);
    assert!(previous.cancelled.load(Ordering::Acquire));
    assert!(!Arc::ptr_eq(&owner, &f.state.owner(id).unwrap()));
    let replacement = query(&f.state, &f.db, id).unwrap();
    f.state.lock().await.config.servers.remove(0);
    assert!(replacement.cancelled.load(Ordering::Acquire));
    assert!(query(&f.state, &f.db, id).is_err());
    f.stop().await;
}

#[tokio::test]
async fn background_queries_refresh_every_three_seconds_without_page_demand() {
    let f = Fixture::new().await;
    let (a, mut calls) = f.install(0);
    let (b, mut other) = f.install(1);
    tokio::time::pause();
    let background = tokio::spawn(run(f.state.clone(), f.db.clone()));
    answer(call(&mut calls).await, json!({"cpu":1}));
    answer(call(&mut other).await, json!({"cpu":2}));
    settle(&a).await;
    settle(&b).await;
    tokio::time::advance(Duration::from_secs(2)).await;
    tokio::task::yield_now().await;
    assert!(calls.try_recv().is_err());
    tokio::time::advance(Duration::from_secs(1)).await;
    answer(call(&mut calls).await, json!({"cpu":3}));
    answer(call(&mut other).await, json!({"cpu":4}));
    settle(&a).await;
    assert_eq!(cached(&a).unwrap().unwrap()["cpu"], 3);
    background.abort();
    assert!(background.await.unwrap_err().is_cancelled());
    tokio::time::resume();
    f.stop().await;
}

#[tokio::test]
async fn clear_fences_samples_waiting_to_write_history_and_other_servers_remain_independent() {
    let f = Fixture::new().await;
    let (a, _calls) = f.install(0);
    let (b, _calls_b) = f.install(1);
    let owner = f.state.owner(a.profile.id).unwrap();
    let held = crate::manager::server_history_guard(&f.state, a.profile.id).await;
    let pending = tokio::spawn({
        let (state, db, reading, owner) = (f.state.clone(), f.db.clone(), a.clone(), owner.clone());
        async move {
            record(
                &state,
                &db,
                &owner,
                &reading.profile,
                &reading.cancelled,
                json!({"cpu":11,"network":[]}),
            )
            .await
        }
    });
    tokio::task::yield_now().await;
    assert!(!pending.is_finished());
    let other_owner = f.state.owner(b.profile.id).unwrap();
    let saved = tokio::time::timeout(
        Duration::from_secs(1),
        record(
            &f.state,
            &f.db,
            &other_owner,
            &b.profile,
            &b.cancelled,
            json!({"cpu":22,"network":[]}),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(saved.get("historyError").is_none());
    assert_eq!(f.db.cache_info().await.unwrap().samples, 1);
    owner.metrics.clear();
    drop(held);
    assert_eq!(pending.await.unwrap().unwrap_err(), CHANGED);
    assert_eq!(f.db.cache_info().await.unwrap().samples, 1);
    // Real clear command closes every cached generation after deleting history.
    commands::clear_metrics_cache(&f.state, &f.db)
        .await
        .unwrap();
    assert!(b.cancelled.load(Ordering::Acquire));
    assert_eq!(f.db.cache_info().await.unwrap().samples, 0);
    assert!(
        record(
            &f.state,
            &f.db,
            &other_owner,
            &b.profile,
            &b.cancelled,
            json!({"cpu":33,"network":[]})
        )
        .await
        .is_err()
    );
    assert_eq!(f.db.cache_info().await.unwrap().samples, 0);
    f.stop().await;
}
