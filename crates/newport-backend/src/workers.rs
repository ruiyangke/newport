//! Lifetime ownership for background services shared by desktop hosts.
use crate::{
    account::Account,
    connectivity,
    desktop_integration::Desktop,
    manager::{self, Shared},
    metrics_db::MetricsDb,
};
use tokio::{sync::Mutex, task::JoinSet};

/// Dropping the owner aborts its workers. Normal shutdown additionally waits
/// for cancellation before closing SSH sessions and the metrics database.
pub struct Workers(Mutex<JoinSet<()>>);

impl Workers {
    /// Call inside the application's Tokio runtime, once per owned profile.
    pub fn start(manager: Shared, metrics: MetricsDb, desktop: Desktop, account: Account) -> Self {
        let mut tasks = JoinSet::new();
        tasks.spawn(account.maintain());
        tasks.spawn(manager.sessions.connections.clone().observe_approvals());
        tasks.spawn(manager::connection_events(
            manager.clone(),
            connectivity::subscribe(),
        ));
        tasks.spawn(manager::background(manager.clone(), desktop));
        tasks.spawn(crate::agent_lifecycle::run(manager.clone()));
        tasks.spawn(crate::metrics_sampler::run(manager, metrics.clone()));
        tasks.spawn(metrics.maintain());
        Self(Mutex::new(tasks))
    }

    /// Idempotent and serialized across shutdown callers. The tasks remain in
    /// the owned set if this waiter is cancelled, so a later call can finish.
    pub async fn stop(&self) {
        self.0.lock().await.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Duration;

    struct Active(Arc<AtomicUsize>);
    impl Drop for Active {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    async fn fixture() -> (Workers, Arc<AtomicUsize>) {
        let active = Arc::new(AtomicUsize::new(0));
        let mut tasks = JoinSet::new();
        for _ in 0..4 {
            let active = active.clone();
            let (ready, wait) = tokio::sync::oneshot::channel();
            tasks.spawn(async move {
                active.fetch_add(1, Ordering::SeqCst);
                let _guard = Active(active);
                ready.send(()).unwrap();
                std::future::pending::<()>().await;
            });
            wait.await.unwrap();
        }
        (Workers(Mutex::new(tasks)), active)
    }

    #[tokio::test]
    async fn stop_waits_for_every_worker_and_accepts_concurrent_repeated_callers() {
        let (workers, active) = fixture().await;
        tokio::join!(workers.stop(), workers.stop());
        assert_eq!(active.load(Ordering::SeqCst), 0);
        workers.stop().await;
    }

    #[tokio::test]
    async fn dropping_owner_does_not_detach_workers() {
        let (workers, active) = fixture().await;
        drop(workers);
        tokio::time::timeout(Duration::from_secs(1), async {
            while active.load(Ordering::SeqCst) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}
