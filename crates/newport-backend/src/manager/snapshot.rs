//! Read projection of committed profile data and its captured session owners.
//! Snapshot readers never acquire configuration/tunnel mutation admission.
use super::{Runtime, Snapshot};
use crate::{model::Config, server_session::ServerSession};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

struct Published {
    config: Config,
    load_error: Option<String>,
    owners: Vec<Arc<ServerSession>>,
}

struct State {
    published: Arc<Published>,
    revision: u64,
    payload: serde_json::Value,
}

pub(super) struct Source {
    instance_id: Uuid,
    runtime: Arc<Mutex<Runtime>>,
    state: Mutex<State>,
}

impl Source {
    pub(super) fn new(instance_id: Uuid, runtime: Arc<Mutex<Runtime>>) -> Self {
        Self {
            instance_id,
            runtime,
            state: Mutex::new(State {
                published: Arc::new(Published {
                    config: Config::default(),
                    load_error: None,
                    owners: vec![],
                }),
                revision: 0,
                payload: serde_json::Value::Null,
            }),
        }
    }

    pub(super) fn publish(
        &self,
        config: Config,
        load_error: Option<String>,
        owners: Vec<Arc<ServerSession>>,
    ) {
        let mut state = self.state.lock().unwrap();
        let previous = &state.published;
        if previous.config == config
            && previous.load_error == load_error
            && previous.owners.len() == owners.len()
            && previous
                .owners
                .iter()
                .zip(&owners)
                .all(|(a, b)| Arc::ptr_eq(a, b))
        {
            return;
        }
        state.published = Arc::new(Published {
            config,
            load_error,
            owners,
        });
    }

    pub(super) fn read(&self) -> Snapshot {
        loop {
            let (published, revision) = {
                let state = self.state.lock().unwrap();
                (state.published.clone(), state.revision)
            };
            // No publication guard while reading any feature owner. A slow
            // lifecycle journal reader must not hold up profile publication.
            let snapshot = self.collect(&published);
            if let Some(snapshot) = self.finish(&published, revision, snapshot) {
                return snapshot;
            }
        }
    }

    fn finish(
        &self,
        published: &Arc<Published>,
        revision: u64,
        mut snapshot: Snapshot,
    ) -> Option<Snapshot> {
        let payload = serde_json::to_value(&snapshot).expect("snapshot serializes");
        let mut state = self.state.lock().unwrap();
        // Discard a projection that raced a profile publication, rather than
        // assign a newer UI revision to an older connection/configuration.
        if !Arc::ptr_eq(published, &state.published) || state.revision != revision {
            return None;
        }
        if state.payload != payload {
            state.revision += 1;
            state.payload = payload;
        }
        snapshot.revision = state.revision;
        Some(snapshot)
    }

    fn collect(&self, published: &Published) -> Snapshot {
        // Forwarding workers still publish through Runtime during their owner
        // migration. All server feature state comes from captured owners below.
        let (tunnels, tunnel_health) = {
            let runtime = self.runtime.lock().unwrap();
            (runtime.tunnels.clone(), runtime.tunnel_health.clone())
        };
        let mut runtime = Runtime {
            tunnels,
            tunnel_health,
            ..Runtime::default()
        };
        runtime
            .tunnels
            .retain(|id, _| published.config.tunnels.iter().any(|t| t.id == *id));
        runtime
            .tunnel_health
            .retain(|id, _| published.config.tunnels.iter().any(|t| t.id == *id));
        for owner in &published.owners {
            let id = owner.server().id;
            runtime.connection_revisions.insert(id, owner.revision());
            runtime
                .agent_transport
                .insert(id, owner.transport_snapshot());
            runtime
                .agent_runtime
                .insert(id, owner.agent_runtime.snapshot());
            runtime
                .agent_permissions
                .insert(id, owner.permissions.snapshot());
            runtime.agent_status.insert(id, owner.lifecycle.snapshot());
            if let Some(progress) = owner.lifecycle.progress.snapshot() {
                runtime.agent_installation.insert(id, progress);
            }
            if let Some(integration) = owner.integration.snapshot() {
                runtime.clipboard.insert(id, integration.connection);
                runtime
                    .clipboard_path_needed
                    .insert(id, integration.path_needed);
                if let Some(message) = integration.message {
                    runtime.clipboard_messages.insert(id, message);
                }
            }
            let health = owner
                .connectivity
                .lock()
                .unwrap()
                .health(std::time::Instant::now());
            runtime.health.insert(id, health.status.into());
            runtime.connectivity.insert(id, health);
        }
        let approvals = crate::agent_approval::subscribe();
        let pending = approvals.borrow();
        runtime.awaiting_agent = published
            .config
            .servers
            .iter()
            .filter(|server| pending.contains_key(&server.id))
            .map(|server| server.id)
            .collect();
        Snapshot {
            instance_id: self.instance_id,
            revision: 0,
            config: published.config.clone(),
            runtime,
            load_error: published.load_error.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        commands,
        manager::{Manager, share},
        model::Server,
        profile::Store,
    };
    use std::time::Duration;

    fn fixture() -> (tempfile::TempDir, crate::manager::Shared, Server) {
        let directory = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(Store::for_test(directory.path().into()));
        let server: Server = serde_json::from_value(serde_json::json!({
            "id": Uuid::new_v4(), "name":"A", "sshHost":"unconnected.invalid",
            "sshPort":22, "sshUser":"fixture"
        }))
        .unwrap();
        manager.config.servers.push(server.clone());
        (directory, share(manager), server)
    }

    #[tokio::test]
    async fn shell_reads_committed_configuration_while_writer_is_busy() {
        let (_directory, state, server) = fixture();
        let before = commands::snapshot(&state).await.unwrap();
        let mut writer = state.lock().await;
        writer.config.servers[0].name = "unpublished edit".into();
        state.observe_connection(&server, 0, std::time::Instant::now(), Ok(()));
        let during = tokio::time::timeout(Duration::from_millis(200), commands::snapshot(&state))
            .await
            .expect("shell must not wait for profile storage")
            .unwrap();
        assert_eq!(during.config.servers[0].name, "A");
        assert_eq!(during.runtime.health[&server.id], "reachable");
        assert!(during.revision > before.revision);
        assert_eq!(
            commands::snapshot(&state).await.unwrap().revision,
            during.revision
        );
        drop(writer);
        let after = commands::snapshot(&state).await.unwrap();
        assert_eq!(after.config.servers[0].name, "unpublished edit");
        assert!(after.revision > during.revision);
        crate::manager::shutdown(&state).await;
    }

    #[tokio::test]
    async fn profile_and_connection_revision_publish_as_one_generation() {
        let (_directory, state, server) = fixture();
        let old = state.owner(server.id).unwrap();
        let mut writer = state.lock().await;
        writer.config.servers[0].ssh_host = "replacement.invalid".into();
        writer.advance_connection_revision(server.id);
        let during = commands::snapshot(&state).await.unwrap();
        assert_eq!(during.config.servers[0].ssh_host, server.ssh_host);
        assert_eq!(
            during.runtime.connection_revisions[&server.id],
            old.revision()
        );
        drop(writer);
        let after = commands::snapshot(&state).await.unwrap();
        assert_eq!(after.config.servers[0].ssh_host, "replacement.invalid");
        assert!(after.runtime.connection_revisions[&server.id] > old.revision());
        assert!(after.revision > during.revision);
        crate::manager::shutdown(&state).await;
    }

    #[tokio::test]
    async fn publication_discards_in_flight_old_snapshot_and_removes_deleted_runtime() {
        let (_directory, state, server) = fixture();
        let source = state.lock().await.snapshots.clone();
        let published = source.state.lock().unwrap().published.clone();
        let stale = source.collect(&published);
        state.lock().await.config.servers.clear();
        let current = source.read();
        assert!(source.finish(&published, current.revision, stale).is_none());
        assert!(current.config.servers.is_empty());
        assert!(!current.runtime.agent_transport.contains_key(&server.id));
        assert!(
            !current
                .runtime
                .connection_revisions
                .contains_key(&server.id)
        );
        assert_eq!(source.read().revision, current.revision);
        crate::manager::shutdown(&state).await;
    }

    #[tokio::test]
    async fn older_parallel_collection_cannot_overwrite_newer_health() {
        let (_directory, state, server) = fixture();
        let source = state.lock().await.snapshots.clone();
        let before = source.read();
        let published = source.state.lock().unwrap().published.clone();
        let stale = source.collect(&published);
        state.observe_connection(&server, 0, std::time::Instant::now(), Ok(()));
        let current = source.read();
        assert_eq!(current.runtime.health[&server.id], "reachable");
        assert!(source.finish(&published, before.revision, stale).is_none());
        assert_eq!(source.read().revision, current.revision);
        crate::manager::shutdown(&state).await;
    }
}
