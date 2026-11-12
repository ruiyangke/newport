//! Read models scoped to a server incarnation and feature. No writes, cursor
//! streams, permission gates or live resource handles belong in this cache.
use crate::{
    query::{Policy, Registry},
    server_session::ServerSession,
};
use newport_protocol::monitor::services::Scope;
use serde_json::Value;
use std::{sync::Arc, time::Duration};

/// Which part of Docker's disk usage to read. Volume sizes take seconds to
/// measure, so the page reads them apart from everything else.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum DiskPart {
    Full,
    Quick,
    Volumes,
}
impl DiskPart {
    pub fn parse(part: Option<&str>) -> Result<Self, String> {
        match part {
            None => Ok(Self::Full),
            Some("quick") => Ok(Self::Quick),
            Some("volumes") => Ok(Self::Volumes),
            Some(other) => Err(format!("Unknown disk part: {other}")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Read {
    Services,
    UnitFile { unit: String, user: bool },
    Containers,
    Inspect(String),
    Disk(DiskPart),
    Compose(String),
    RemoteAppsStatus,
    RemoteAppsCatalogue,
}
impl Read {
    fn services(&self) -> bool {
        matches!(self, Self::Services | Self::UnitFile { .. })
    }
    /// The Remote apps agent and distribution, and the agent's applications.
    fn remote_apps(&self) -> bool {
        matches!(self, Self::RemoteAppsStatus | Self::RemoteAppsCatalogue)
    }
    fn policy(&self) -> Policy {
        let seconds = match self {
            Self::Services | Self::Containers => 5,
            Self::UnitFile { .. } => 10,
            // Measuring volumes walks them on disk; reuse a recent reading.
            Self::Disk(DiskPart::Volumes) => 60,
            Self::RemoteAppsStatus => 30,
            // Installed applications change rarely; Refresh reads them again.
            Self::RemoteAppsCatalogue => 300,
            _ => 5,
        };
        Policy {
            stale_after: Duration::from_secs(seconds),
            retry_after: Duration::from_secs(5),
            timeout: Duration::from_secs(40),
            observation_lease: Duration::from_secs(6),
        }
    }
    async fn load(&self, owner: &Arc<ServerSession>) -> Result<Cached, String> {
        use newport_protocol::monitor::{compose, containers, services};
        Ok(match self {
            Self::Services => Cached::Services(
                crate::services::request(owner, services::Request::Snapshot).await?,
            ),
            Self::UnitFile { unit, user } => Cached::Services(
                crate::services::request(
                    owner,
                    services::Request::Unit {
                        unit: unit.clone(),
                        scope: if *user { Scope::User } else { Scope::System },
                    },
                )
                .await?,
            ),
            Self::Containers => Cached::Containers(
                crate::containers::request(owner, containers::Request::Snapshot).await?,
            ),
            Self::Inspect(id) => Cached::Containers(
                crate::containers::request(
                    owner,
                    containers::Request::Inspect {
                        container: id.clone(),
                    },
                )
                .await?,
            ),
            Self::Disk(part) => Cached::Containers(
                crate::containers::request(
                    owner,
                    containers::Request::Disk {
                        part: match part {
                            DiskPart::Full => None,
                            DiskPart::Quick => Some(containers::DiskPart::Quick),
                            DiskPart::Volumes => Some(containers::DiskPart::Volumes),
                        },
                    },
                )
                .await?,
            ),
            Self::Compose(project) => Cached::Compose(
                crate::compose::request(
                    owner,
                    compose::Request::Project {
                        project: project.clone(),
                    },
                )
                .await?,
            ),
            Self::RemoteAppsStatus => Cached::Projection(crate::remote_apps::status(owner).await?),
            Self::RemoteAppsCatalogue => {
                Cached::Projection(crate::remote_apps::catalogue(owner).await?)
            }
        })
    }
}

/// Typed cache entries retain service ownership without a JSON round trip.
#[derive(Clone)]
pub(crate) enum Cached {
    Services(newport_protocol::monitor::services::Reply),
    Containers(newport_protocol::monitor::containers::Reply),
    Compose(newport_protocol::monitor::compose::Reply),
    Projection(Value),
}
impl Cached {
    fn into_value(self) -> Result<Value, String> {
        use newport_protocol::monitor::services::Reply;
        match self {
            Self::Services(value) => match value {
                Reply::Snapshot(value) => serde_json::to_value(value),
                Reply::Logs(value) => serde_json::to_value(value),
                Reply::Unit(value) => serde_json::to_value(value),
                Reply::Action(value) => serde_json::to_value(value),
            },
            Self::Containers(value) => serde_json::to_value(value),
            Self::Compose(value) => serde_json::to_value(value),
            Self::Projection(value) => return Ok(value),
        }
        .map_err(|error| error.to_string())
    }
}

pub(crate) struct Queries {
    services: Registry<Read, Cached>,
    containers: Registry<Read, Cached>,
    remote_apps: Registry<Read, Cached>,
}
impl Default for Queries {
    fn default() -> Self {
        Self {
            services: Registry::new(40),
            containers: Registry::new(40),
            remote_apps: Registry::new(3),
        }
    }
}
impl Queries {
    pub fn close(&self) {
        self.services.close();
        self.containers.close();
        self.remote_apps.close();
    }
    pub fn invalidate_services(&self) {
        self.services.invalidate(|_| true);
    }
    pub fn invalidate_containers(&self) {
        self.containers.invalidate(|_| true);
    }
}

pub(crate) async fn read_typed(
    owner: &Arc<ServerSession>,
    key: Read,
    force: bool,
) -> Result<Cached, String> {
    // Remote apps use their own agent, not Needle.
    let registry = if key.remote_apps() {
        &owner.queries.remote_apps
    } else {
        if key.services() {
            &owner.queries.services
        } else {
            &owner.queries.containers
        }
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

/// Existing internal UI projections keep their established JSON shape.
pub(crate) async fn read(
    owner: &Arc<ServerSession>,
    key: Read,
    force: bool,
) -> Result<Value, String> {
    read_typed(owner, key, force).await?.into_value()
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

    #[test]
    fn disk_parts_parse_and_volume_sizes_stay_fresh_longer() {
        assert_eq!(DiskPart::parse(None), Ok(DiskPart::Full));
        assert_eq!(DiskPart::parse(Some("quick")), Ok(DiskPart::Quick));
        assert_eq!(DiskPart::parse(Some("volumes")), Ok(DiskPart::Volumes));
        assert!(DiskPart::parse(Some("images")).is_err());
        assert!(
            Read::Disk(DiskPart::Volumes).policy().stale_after
                > Read::Disk(DiskPart::Quick).policy().stale_after
        );
    }

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
                    std::future::pending::<Result<Cached, String>>().await
                }
            })
            .unwrap();
        owner_a
            .queries
            .containers
            .get_or_insert(Read::Containers, Read::Containers.policy(), || async {
                Ok(Cached::Projection(
                    json!({"owner":"A", "feature":"containers"}),
                ))
            })
            .unwrap();
        owner_b
            .queries
            .services
            .get_or_insert(Read::Services, Read::Services.policy(), || async {
                Ok(Cached::Projection(
                    json!({"owner":"B", "feature":"services"}),
                ))
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
