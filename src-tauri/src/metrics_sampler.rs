use crate::{
    cockpit,
    manager::Shared,
    metrics_db::{self, MetricsDb},
    model::Server,
};
use serde_json::Value;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{
    sync::{Mutex, Notify},
    task::{AbortHandle, JoinSet},
};
use uuid::Uuid;

type CachedReadings = HashMap<Uuid, (Server, u64, Result<Value, String>)>;

#[derive(Clone, Default)]
pub struct Sampler {
    latest: Arc<Mutex<CachedReadings>>,
    changed: Arc<Notify>,
}
impl Sampler {
    pub async fn clear_cache(&self) {
        self.latest.lock().await.clear();
    }

    /// Wait for the already-running first sample instead of making the UI poll.
    pub async fn first_reading(&self, server: &Server, revision: u64) -> Result<Value, String> {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                // Register before inspecting the cache so a completed sample cannot be missed.
                let changed = self.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                {
                    let readings = self.latest.lock().await;
                    if let Some((profile, generation, result)) = readings.get(&server.id) {
                        if profile.same_connection(server) && *generation == revision {
                            return result.clone();
                        }
                    }
                }
                changed.await;
            }
        })
        .await
        .unwrap_or_else(|_| Err("The server is taking too long to respond. Try refreshing.".into()))
    }
    #[cfg(test)]
    pub async fn latest(&self, server: &Server, revision: u64) -> Result<Value, String> {
        match self.latest.lock().await.get(&server.id) {
            Some((profile, generation, result))
                if profile.same_connection(server) && *generation == revision =>
            {
                result.clone()
            }
            _ => Err("Waiting for the background sampler’s first reading. Refresh shortly.".into()),
        }
    }
    async fn sample(&self, state: &Shared, db: &MetricsDb, server: &Server, revision: u64) {
        let mut result = cockpit::collect(server, cockpit::Section::Overview).await;
        // Hold the profile lock through persistence: removed/edited profiles cannot receive late writes.
        let manager = state.lock().await;
        if !manager.matches_connection(server, revision) {
            return;
        }
        if let Ok(data) = &mut result {
            let at = metrics_db::now_ms();
            data["sampledAt"] = at.into();
            let saved = db.record(server, at, data).await;
            if let Err(error) = saved {
                data["historyError"] =
                    format!("Live metrics loaded, but local history could not be saved: {error}")
                        .into();
            }
        }
        self.latest
            .lock()
            .await
            .insert(server.id, (server.clone(), revision, result));
        self.changed.notify_waiters();
    }
    pub async fn run(self, state: Shared, db: MetricsDb) {
        let mut jobs = JoinSet::new();
        let mut profiles: HashMap<Uuid, (Server, u64, AbortHandle)> = HashMap::new();
        let mut reconcile = tokio::time::interval(Duration::from_secs(1));
        loop {
            reconcile.tick().await;
            let servers: Vec<_> = {
                let manager = state.lock().await;
                manager
                    .config
                    .servers
                    .iter()
                    .map(|server| (server.clone(), manager.connection_revision(server.id)))
                    .collect()
            };
            // Taking the manager lock above waits out any in-flight persistence.
            // Metadata-only edits keep workers; connection generations replace them.
            profiles.retain(|id, (profile, revision, task)| {
                let current = servers.iter().any(|(server, generation)| {
                    server.id == *id && profile.same_connection(server) && revision == generation
                });
                if !current {
                    task.abort();
                }
                current && !task.is_finished()
            });
            self.latest
                .lock()
                .await
                .retain(|id, (profile, revision, _)| {
                    servers.iter().any(|(server, generation)| {
                        server.id == *id
                            && profile.same_connection(server)
                            && revision == generation
                    })
                });
            while jobs.try_join_next().is_some() {}
            for (server, revision) in servers {
                if profiles.contains_key(&server.id) {
                    continue;
                }
                let (sampler, state, db, profile) =
                    (self.clone(), state.clone(), db.clone(), server.clone());
                let task = jobs.spawn(async move {
                    let mut timer = tokio::time::interval(Duration::from_secs(10));
                    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    loop {
                        timer.tick().await;
                        if !state.lock().await.matches_connection(&profile, revision) {
                            break;
                        }
                        sampler.sample(&state, &db, &profile, revision).await;
                    }
                });
                profiles.insert(server.id, (server, revision, task));
            }
        }
        // Dropping JoinSet on shutdown also cancels every per-server worker.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn first_reading_arrives_without_waiting_for_the_next_poll() {
        let sampler = Sampler::default();
        let server: Server = serde_json::from_value(serde_json::json!({"id":Uuid::new_v4(),"name":"Test","sshHost":"host","sshPort":22,"sshUser":"user","identityFile":null,"authMethod":"publicKey"})).unwrap();
        let request = sampler.first_reading(&server, 0);
        let publish = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            sampler.latest.lock().await.insert(
                server.id,
                (server.clone(), 0, Ok(serde_json::json!({"cpu":42}))),
            );
            sampler.changed.notify_waiters();
        };
        let (result, _) = tokio::time::timeout(Duration::from_secs(1), async {
            tokio::join!(request, publish)
        })
        .await
        .expect("The first reading must not wait for the 10-second poll");
        assert_eq!(result.unwrap()["cpu"], 42);
        assert_eq!(sampler.first_reading(&server, 0).await.unwrap()["cpu"], 42);
        sampler
            .latest
            .lock()
            .await
            .insert(server.id, (server.clone(), 0, Err("Offline".into())));
        assert_eq!(
            sampler.first_reading(&server, 0).await.unwrap_err(),
            "Offline"
        );
    }
    #[tokio::test]
    async fn cached_results_are_isolated_and_errors_are_not_stale_successes() {
        let sampler = Sampler::default();
        let server: Server = serde_json::from_value(serde_json::json!({"id":Uuid::new_v4(),"name":"Test","sshHost":"host","sshPort":22,"sshUser":"user","identityFile":null,"authMethod":"publicKey"})).unwrap();
        assert!(sampler.latest(&server, 0).await.is_err());
        sampler.latest.lock().await.insert(
            server.id,
            (
                server.clone(),
                0,
                Ok(serde_json::json!({"sampledAt":123,"cpu":42})),
            ),
        );
        assert_eq!(sampler.latest(&server, 0).await.unwrap()["sampledAt"], 123);
        let mut renamed = server.clone();
        renamed.name = "Renamed".into();
        renamed.clipboard_enabled = !renamed.clipboard_enabled;
        assert_eq!(sampler.latest(&renamed, 0).await.unwrap()["sampledAt"], 123);
        assert!(sampler.latest(&server, 1).await.is_err());
        let mut changed = server.clone();
        changed.ssh_host = "other-host".into();
        assert!(sampler.latest(&changed, 0).await.is_err());
        sampler
            .latest
            .lock()
            .await
            .insert(server.id, (server.clone(), 0, Err("Offline".into())));
        assert_eq!(sampler.latest(&server, 0).await.unwrap_err(), "Offline");
    }
}
