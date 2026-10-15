//! Read models scoped to a server incarnation and feature. No writes, cursor
//! streams, permission gates or live resource handles belong in this cache.
use crate::{
    query::{Policy, Registry},
    server_session::ServerSession,
    services::Scope,
};
use serde_json::Value;
use std::{sync::Arc, time::Duration};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Read {
    Services,
    UnitFile { unit: String, user: bool },
    Containers,
    Inspect(String),
    Disk,
    Compose(String),
}
impl Read {
    fn services(&self) -> bool {
        matches!(self, Self::Services | Self::UnitFile { .. })
    }
    fn policy(&self) -> Policy {
        let seconds = match self {
            Self::Services | Self::Containers => 5,
            Self::UnitFile { .. } => 10,
            _ => 5,
        };
        Policy {
            stale_after: Duration::from_secs(seconds),
            retry_after: Duration::from_secs(5),
            timeout: Duration::from_secs(40),
            observation_lease: Duration::from_secs(6),
        }
    }
    async fn load(&self, owner: &Arc<ServerSession>) -> Result<Value, String> {
        match self {
            Self::Services => crate::services::snapshot(owner).await,
            Self::UnitFile { unit, user } => {
                crate::services::unit_file(
                    owner,
                    unit,
                    if *user { Scope::User } else { Scope::System },
                )
                .await
            }
            Self::Containers => crate::containers::snapshot(owner).await,
            Self::Inspect(id) => crate::containers::inspect(owner, id).await,
            Self::Disk => crate::containers::disk(owner).await,
            Self::Compose(project) => crate::compose::project(owner, project).await,
        }
    }
}

pub(crate) struct Queries {
    services: Registry<Read, Value>,
    containers: Registry<Read, Value>,
    pub files: Registry<String, Arc<crate::files::Listing>>,
}
impl Default for Queries {
    fn default() -> Self {
        Self {
            services: Registry::new(40),
            containers: Registry::new(40),
            files: Registry::new(16),
        }
    }
}
impl Queries {
    pub fn close(&self) {
        self.services.close();
        self.containers.close();
        self.files.close();
    }
    pub fn invalidate_services(&self) {
        self.services.invalidate(|_| true);
    }
    pub fn invalidate_containers(&self) {
        self.containers.invalidate(|_| true);
    }
}

pub(crate) async fn read(
    owner: &Arc<ServerSession>,
    key: Read,
    force: bool,
) -> Result<Value, String> {
    owner.require_agent_available()?;
    let registry = if key.services() {
        &owner.queries.services
    } else {
        &owner.queries.containers
    };
    let weak = Arc::downgrade(owner);
    let policy = key.policy();
    let load = key.clone();
    let query = registry.get_or_insert(key, policy, move || {
        let weak = weak.clone();
        let load = load.clone();
        async move {
            let owner = weak.upgrade().ok_or("Server session closed.")?;
            let mut cancelled = owner.cancellation();
            if *cancelled.borrow() {
                return Err("Server session closed.".into());
            }
            tokio::select! {
                biased;
                _ = cancelled.changed() => Err("Server session closed.".into()),
                result = load.load(&owner) => result,
            }
        }
    })?;
    if owner.is_revoked() {
        query.close();
    }
    if force {
        query.refresh().await
    } else {
        query.read().await
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
    use serde_json::json;
    use uuid::Uuid;

    #[tokio::test]
    async fn features_and_servers_read_independently_while_profile_writer_is_busy() {
        let directory = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(Store::for_test(directory.path().into()));
        let a: Server = serde_json::from_value(json!({"id":Uuid::new_v4(),
            "name":"A", "sshHost":"unconnected.invalid", "sshUser":"fixture", "sshPort":22}))
        .unwrap();
        let b = Server {
            id: Uuid::new_v4(),
            name: "B".into(),
            ..a.clone()
        };
        manager.config.servers = vec![a.clone(), b.clone()];
        let state = share(manager);
        let owner_a = state.owner(a.id).unwrap();
        let owner_b = state.owner(b.id).unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let loader_entered = entered.clone();
        let slow = owner_a
            .queries
            .services
            .get_or_insert(Read::Services, Read::Services.policy(), move || {
                let entered = loader_entered.clone();
                async move {
                    entered.notify_one();
                    std::future::pending::<Result<Value, String>>().await
                }
            })
            .unwrap();
        owner_a
            .queries
            .containers
            .get_or_insert(Read::Containers, Read::Containers.policy(), || async {
                Ok(json!({"owner":"A", "feature":"containers"}))
            })
            .unwrap();
        owner_b
            .queries
            .services
            .get_or_insert(Read::Services, Read::Services.policy(), || async {
                Ok(json!({"owner":"B", "feature":"services"}))
            })
            .unwrap();
        let mut writer = state.lock().await;
        let work_state = state.clone();
        let pending =
            tokio::spawn(async move { commands::services_snapshot(&work_state, a.id).await });
        tokio::time::timeout(Duration::from_secs(1), entered.notified())
            .await
            .expect("a query must start while the profile writer is occupied");
        let (containers, services) = tokio::time::timeout(Duration::from_secs(1), async {
            tokio::join!(
                commands::containers_snapshot(&state, a.id),
                commands::services_snapshot(&state, b.id)
            )
        })
        .await
        .expect(
            "another server and another feature must not queue behind the slow query or writer",
        );
        assert_eq!(containers.unwrap()["owner"], "A");
        assert_eq!(services.unwrap()["owner"], "B");
        assert!(!pending.is_finished());
        writer.advance_connection_revision(a.id);
        drop(writer);
        assert!(pending.await.unwrap().is_err());
        assert!(slow.snapshot().data.is_none());
        assert!(Arc::ptr_eq(&owner_b, &state.owner(b.id).unwrap()));
        assert_eq!(
            commands::services_snapshot(&state, b.id).await.unwrap()["owner"],
            "B"
        );
        crate::manager::shutdown(&state).await;
    }
}
