//! A consumer's use of Needle on a server, over the server's link. It is
//! fenced once its server session is retired, or once the runtime it
//! launched on ends, which it does when its channel's connection is lost.
use crate::{
    agent,
    agent_runtime::{self, Error},
    server_session::ServerSession,
};
use std::sync::{Arc, Mutex, Weak};
use uuid::Uuid;

pub struct Needle {
    owner: Weak<ServerSession>,
    runtime: Mutex<Option<agent_runtime::Handle>>,
    worker_id: Mutex<Option<Uuid>>,
}

impl Needle {
    pub(crate) fn new(owner: &Arc<ServerSession>) -> Self {
        Self {
            owner: Arc::downgrade(owner),
            runtime: Mutex::new(None),
            worker_id: Mutex::new(None),
        }
    }
    fn owner(&self) -> Result<Arc<ServerSession>, Error> {
        self.owner.upgrade().ok_or(Error::Stale)
    }
    /// The status of the link Needle runs over.
    pub(crate) fn link_status(
        &self,
    ) -> Result<tokio::sync::watch::Receiver<newport_ssh::Status>, Error> {
        Ok(self.owner()?.link().status())
    }
    pub fn is_closed(&self) -> bool {
        self.validate().is_err()
    }
    pub fn validate(&self) -> Result<(), Error> {
        if self.owner.upgrade().is_none_or(|owner| owner.is_revoked()) {
            return Err(Error::Stale);
        }
        if self
            .runtime
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|runtime| runtime.is_closed())
        {
            return Err(Error::Interrupted("Needle runtime disconnected".into()));
        }
        Ok(())
    }
    pub(crate) fn is_draining(&self) -> bool {
        self.runtime
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|r| r.is_draining())
    }
    /// Used only between requests, after the caller releases its old channel.
    pub(crate) fn release_draining_runtime(&self) {
        let mut runtime = self.runtime.lock().unwrap();
        if runtime.as_ref().is_some_and(|r| r.is_draining()) {
            *runtime = None;
        }
    }
    pub(crate) async fn wait_for_upgrade(&self) {
        let runtime = self.runtime.lock().unwrap().clone();
        if let Some(runtime) = runtime {
            runtime.draining().await;
        } else {
            std::future::pending::<()>().await;
        }
    }
    pub async fn launch(
        &self,
        service: agent::Service,
    ) -> Result<(agent_runtime::WorkerStream, String), Error> {
        let (channel, worker) = self.launch_channel(service).await?;
        Ok((worker.stream(channel.into_stream()), String::new()))
    }
    /// Preserve SSH stderr and exit status for adapters with their own framing.
    /// The returned worker must stay alive for the entire channel lifetime.
    pub(crate) async fn launch_channel(
        &self,
        service: agent::Service,
    ) -> Result<(crate::service_channel::Channel, agent_runtime::Worker), Error> {
        self.validate()?;
        let owner = self.owner()?;
        let (name, version) = service.capability();
        let worker = owner
            .agent_runtime
            .worker(
                owner.link(),
                owner.integration_session_id(),
                name,
                version,
                || self.validate(),
            )
            .await
            .inspect_err(|error| {
                if *error == Error::NotInstalled {
                    owner.lifecycle.found_missing(owner.reached());
                }
            })?;
        self.validate()?;
        *self.runtime.lock().unwrap() = Some(worker.runtime.clone());
        *self.worker_id.lock().unwrap() = Some(worker.id);
        self.validate()?;
        let result = agent::launch_worker_channel(
            owner.link(),
            service,
            worker.runtime.hello(),
            worker.id,
            worker.endpoint.as_ref(),
        )
        .await
        .map(|channel| (channel, worker));
        self.validate()?;
        result
    }
    pub(crate) fn supports_gui_notices(&self) -> bool {
        self.runtime
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|runtime| runtime.hello().require("gui-notices", 1).is_ok())
    }
    pub(crate) fn runtime_hello(&self) -> Result<agent_runtime::protocol::Hello, String> {
        self.validate()?;
        self.runtime
            .lock()
            .unwrap()
            .as_ref()
            .map(|runtime| runtime.hello().clone())
            .ok_or_else(|| "Needle worker is not registered".into())
    }
    pub(crate) fn supports_gui_launch(&self, version: u32) -> bool {
        self.runtime
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|runtime| runtime.hello().require("gui-launch", version).is_ok())
    }
    pub async fn apply_permissions(
        &self,
        epoch: u64,
        desired: agent::control::Permissions,
    ) -> Result<(), Error> {
        self.validate()?;
        let owner = self.owner()?;
        let unregistered = || Error::Unavailable("Needle worker is not registered".into());
        let runtime = self
            .runtime
            .lock()
            .unwrap()
            .as_ref()
            .map(|runtime| runtime.hello().instance_id.clone())
            .ok_or_else(unregistered)?;
        let worker = self.worker_id.lock().unwrap().ok_or_else(unregistered)?;
        let applied = owner
            .agent_runtime
            .permissions(&runtime, worker, epoch, desired)
            .await?;
        self.validate()?;
        if applied.permissions.revision != desired.revision {
            return Err(Error::Unavailable(
                "Needle acknowledged a different permission revision".into(),
            ));
        }
        if !owner.permissions.acknowledge_for(epoch, applied)
            && owner.permissions.snapshot().desired.revision == desired.revision
        {
            return Err(Error::Unavailable(
                "Needle acknowledged permissions that were not requested".into(),
            ));
        }
        Ok(())
    }
    /// Runs beside feature traffic, never behind its writes or acknowledgements.
    pub async fn watch_permissions(&self, epoch: u64, applied: impl Fn()) -> Result<(), Error> {
        let owner = self.owner()?;
        let mut changes = owner.permissions.subscribe();
        loop {
            let desired = *changes.borrow_and_update();
            self.apply_permissions(epoch, desired).await?;
            applied();
            changes
                .changed()
                .await
                .map_err(|_| Error::Interrupted("Permission state closed".into()))?;
        }
    }
}
