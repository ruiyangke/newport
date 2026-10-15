use super::*;
use std::sync::{atomic::AtomicUsize, mpsc};

struct Candidate(Arc<AtomicUsize>);
impl Package for Candidate {
    fn version(&self) -> String {
        "2.0.0".into()
    }
}
impl Drop for Candidate {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
enum Outcome {
    Update,
    Current,
    Error,
    Panic,
}
struct Controlled {
    started: mpsc::Sender<()>,
    released: Mutex<mpsc::Receiver<Outcome>>,
    calls: AtomicUsize,
    drops: Arc<AtomicUsize>,
    saw_cancel: AtomicBool,
}
impl Download<Candidate> for Controlled {
    fn check(
        &self,
        cancelled: &dyn Fn() -> bool,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<Option<Candidate>, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        progress(Progress {
            version: "3.0.0".parse().unwrap(),
            downloaded: 4,
            total: Some(10),
        });
        self.started.send(()).unwrap();
        let outcome = self
            .released
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        self.saw_cancel.store(cancelled(), Ordering::SeqCst);
        // Model a late callback/result even if cancellation was requested.
        progress(Progress {
            version: "3.0.0".parse().unwrap(),
            downloaded: 10,
            total: Some(10),
        });
        match outcome {
            Outcome::Update => Ok(Some(Candidate(self.drops.clone()))),
            Outcome::Current => Ok(None),
            Outcome::Error => Err("fixture signature rejection".into()),
            Outcome::Panic => panic!("fixture worker panic"),
        }
    }
}
struct Fixture {
    service: Service<Candidate>,
    source: Arc<Controlled>,
    started: mpsc::Receiver<()>,
    release: mpsc::Sender<Outcome>,
}
fn fixture() -> Fixture {
    let (started, receive) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let source = Arc::new(Controlled {
        started,
        released: Mutex::new(released),
        calls: AtomicUsize::new(0),
        drops: Arc::new(AtomicUsize::new(0)),
        saw_cancel: AtomicBool::new(false),
    });
    Fixture {
        service: Service::new(Some(source.clone())),
        source,
        started: receive,
        release,
    }
}
async fn start(
    service: &Service<Candidate>,
    started: &mpsc::Receiver<()>,
) -> tokio::task::JoinHandle<Result<(), String>> {
    let service = service.clone();
    let worker = tokio::spawn(async move { service.check().await });
    // Let the RPC task enqueue its blocking job before waiting for the fixture.
    tokio::task::yield_now().await;
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    worker
}
async fn settled(service: &Service<Candidate>) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while service.state().busy {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn concurrent_checks_share_one_worker_and_never_replace_a_ready_package() {
    let f = fixture();
    let check = start(&f.service, &f.started).await;
    assert_eq!(f.service.status()["phase"], "downloading");
    assert_eq!(f.service.status()["downloaded"], 4);
    assert_eq!(f.service.status()["total"], 10);
    assert!(
        f.service.state().pending.is_none(),
        "Progress metadata is not a verified package"
    );
    assert!(
        f.service
            .check()
            .await
            .unwrap_err()
            .contains("already in progress")
    );
    f.release.send(Outcome::Update).unwrap();
    check.await.unwrap().unwrap();
    assert_eq!(f.service.status()["phase"], "ready");
    assert_eq!(
        f.service.status()["version"],
        "2.0.0",
        "The verified package wins over progress metadata"
    );
    f.service.check().await.unwrap();
    assert_eq!(f.source.calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.source.drops.load(Ordering::SeqCst), 0);
    f.service.shutdown();
    assert_eq!(f.source.drops.load(Ordering::SeqCst), 1);
    assert_eq!(f.service.status()["enabled"], false);
}

#[tokio::test]
async fn losing_the_rpc_waiter_does_not_lose_worker_ownership_or_its_result() {
    let f = fixture();
    let check = start(&f.service, &f.started).await;
    check.abort();
    assert!(check.await.unwrap_err().is_cancelled());
    assert!(f.service.check().await.is_err());
    f.release.send(Outcome::Update).unwrap();
    settled(&f.service).await;
    assert_eq!(f.service.status()["phase"], "ready");
    assert!(!f.source.saw_cancel.load(Ordering::SeqCst));
    assert_eq!(f.source.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn shutdown_discards_late_callbacks_and_verified_results() {
    let f = fixture();
    let check = start(&f.service, &f.started).await;
    f.service.shutdown();
    f.release.send(Outcome::Update).unwrap();
    assert!(check.await.unwrap().is_err());
    assert!(f.source.saw_cancel.load(Ordering::SeqCst));
    assert_eq!(f.source.drops.load(Ordering::SeqCst), 1);
    let status = f.service.status();
    assert_eq!(status["phase"], "idle");
    assert_eq!(status["enabled"], false);
    assert!(status["version"].is_null());
    assert_eq!(status["downloaded"], 0);
    assert!(f.service.check().await.is_err());
}

#[tokio::test]
async fn failed_or_panicking_workers_release_ownership_for_an_explicit_retry() {
    for outcome in [Outcome::Error, Outcome::Panic] {
        let f = fixture();
        let check = start(&f.service, &f.started).await;
        f.release.send(outcome).unwrap();
        assert!(check.await.unwrap().is_err());
        assert_eq!(f.service.status()["phase"], "error");
        assert!(f.service.state().pending.is_none());
        let retry = start(&f.service, &f.started).await;
        f.release.send(Outcome::Current).unwrap();
        retry.await.unwrap().unwrap();
        let status = f.service.status();
        assert_eq!(status["phase"], "current");
        assert!(status["error"].is_null());
        assert!(status["version"].is_null());
        assert_eq!(status["downloaded"], 0);
        assert_eq!(f.source.calls.load(Ordering::SeqCst), 2);
    }
}

#[test]
fn only_accepted_installed_release_builds_may_check_or_install() {
    for accepted in [false, true] {
        for debug in [false, true] {
            for isolated in [false, true] {
                for app_store in [false, true] {
                    assert_eq!(
                        available(accepted, debug, isolated, app_store),
                        accepted && !debug && !isolated && !app_store
                    );
                }
            }
        }
    }
    assert_eq!(Service::for_host().unwrap().status()["enabled"], false);
}

#[tokio::test]
async fn desktop_commands_use_the_host_controller_and_keep_disabled_updates_readable() {
    use newport_backend::application::DesktopCommands;
    struct Current(AtomicUsize);
    impl Download<VerifiedUpdate> for Current {
        fn check(
            &self,
            _: &dyn Fn() -> bool,
            _: &mut dyn FnMut(Progress),
        ) -> Result<Option<VerifiedUpdate>, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }
    }
    let (desktop, _requests) = crate::desktop::Integration::new();
    let source = Arc::new(Current(AtomicUsize::new(0)));
    let service = Service::new(Some(source.clone()));
    let commands =
        crate::desktop::Commands::new(desktop, newport_platform::startup::Startup::disabled())
            .with_updates(service.clone());
    assert_eq!(
        commands
            .dispatch("update_status", Value::Null)
            .await
            .unwrap()["phase"],
        "idle"
    );
    assert_eq!(
        commands
            .dispatch("check_for_updates", Value::Null)
            .await
            .unwrap(),
        Value::Null
    );
    assert_eq!(
        commands
            .dispatch("update_status", Value::Null)
            .await
            .unwrap()["phase"],
        "current"
    );
    assert_eq!(source.0.load(Ordering::SeqCst), 1);
    service.shutdown();
    assert!(
        commands
            .dispatch("check_for_updates", Value::Null)
            .await
            .is_err()
    );
    assert_eq!(
        commands
            .dispatch("update_status", Value::Null)
            .await
            .unwrap()["enabled"],
        false
    );
    let disabled = Service::disabled();
    assert!(disabled.check().await.is_err());
    assert_eq!(
        disabled.status(),
        json!({"enabled":false,"currentVersion":env!("CARGO_PKG_VERSION"),"phase":"idle","version":null,"downloaded":0,"total":null,"error":null})
    );
}

async fn ready() -> Fixture {
    let f = fixture();
    let check = start(&f.service, &f.started).await;
    f.release.send(Outcome::Update).unwrap();
    check.await.unwrap().unwrap();
    f
}

struct Preparation(Arc<AtomicUsize>);
impl Drop for Preparation {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn installation_requires_the_confirmed_version_and_retains_ready_bytes_after_failure() {
    let f = ready().await;
    let prepared = Arc::new(AtomicUsize::new(0));
    let observed = prepared.clone();
    assert!(
        f.service
            .prepare_install(
                "3.0.0",
                move |_| {
                    observed.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                },
                |_| Ok(())
            )
            .await
            .unwrap_err()
            .contains("ready update changed")
    );
    assert_eq!(prepared.load(Ordering::SeqCst), 0);
    assert_eq!(f.service.status()["phase"], "ready");
    assert!(
        f.service
            .prepare_install::<()>(
                "2.0.0",
                |_| Err("disk full".into()),
                |_| panic!("failed staging must never queue restart")
            )
            .await
            .is_err()
    );
    assert_eq!(f.service.status()["phase"], "ready");
    assert!(f.service.state().pending.is_some());
    assert_eq!(f.source.drops.load(Ordering::SeqCst), 0);
    let dropped = Arc::new(AtomicUsize::new(0));
    let owned = dropped.clone();
    assert!(
        f.service
            .prepare_install(
                "2.0.0",
                move |_| Ok(Preparation(owned)),
                |_| Err("desktop closed".into())
            )
            .await
            .is_err()
    );
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert_eq!(f.service.status()["phase"], "ready");
    assert!(f.service.state().pending.is_some());
}

#[tokio::test]
async fn successful_handoff_transfers_preparation_once_and_blocks_duplicates() {
    let f = ready().await;
    let (send, receive) = mpsc::channel();
    let dropped = Arc::new(AtomicUsize::new(0));
    let owned = dropped.clone();
    f.service
        .prepare_install(
            "2.0.0",
            move |_| Ok(Preparation(owned)),
            move |plan| send.send(plan).map_err(|error| error.to_string()),
        )
        .await
        .unwrap();
    let preparation = receive.recv().unwrap();
    assert_eq!(f.service.status()["phase"], "installing");
    assert!(f.service.state().pending.is_none());
    assert!(
        f.service
            .prepare_install("2.0.0", |_| Ok(()), |_| panic!("duplicate restart"))
            .await
            .is_err()
    );
    assert!(f.service.check().await.is_err());
    f.service.shutdown();
    assert_eq!(
        dropped.load(Ordering::SeqCst),
        0,
        "The app owns the preparation after handoff"
    );
    drop(preparation);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert_eq!(f.source.drops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn shutdown_during_staging_drops_preparation_without_requesting_restart() {
    let f = ready().await;
    let (started, receive_started) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let dropped = Arc::new(AtomicUsize::new(0));
    let owned = dropped.clone();
    let service = f.service.clone();
    let operation = tokio::spawn(async move {
        service
            .prepare_install(
                "2.0.0",
                move |_| {
                    started.send(()).unwrap();
                    wait.recv_timeout(Duration::from_secs(5)).unwrap();
                    Ok(Preparation(owned))
                },
                |_| panic!("shutdown must prevent handoff"),
            )
            .await
    });
    tokio::task::yield_now().await;
    receive_started
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    f.service.shutdown();
    let waiting = f.service.clone();
    let cleanup = tokio::spawn(async move { waiting.finish_preparation().await });
    tokio::task::yield_now().await;
    assert!(
        !cleanup.is_finished(),
        "Staging cleanup must finish before runtime shutdown"
    );
    release.send(()).unwrap();
    assert!(operation.await.unwrap().is_err());
    cleanup.await.unwrap();
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert_eq!(f.source.drops.load(Ordering::SeqCst), 1);
    assert_eq!(f.service.status()["phase"], "idle");
}

#[tokio::test]
async fn cancelling_the_install_waiter_does_not_replay_or_lose_the_authorized_handoff() {
    let f = ready().await;
    let (started, receive_started) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let (handed_off, received) = mpsc::channel();
    let service = f.service.clone();
    let operation = tokio::spawn(async move {
        service
            .prepare_install(
                "2.0.0",
                move |_| {
                    started.send(()).unwrap();
                    wait.recv_timeout(Duration::from_secs(5)).unwrap();
                    Ok(())
                },
                move |_| handed_off.send(()).map_err(|error| error.to_string()),
            )
            .await
    });
    tokio::task::yield_now().await;
    receive_started
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());
    assert!(
        f.service
            .prepare_install("2.0.0", |_| Ok(()), |_| Ok(()))
            .await
            .is_err()
    );
    release.send(()).unwrap();
    received.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(f.service.status()["phase"], "installing");
    assert!(
        f.service
            .prepare_install("2.0.0", |_| Ok(()), |_| Ok(()))
            .await
            .is_err()
    );
}
