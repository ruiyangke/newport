//! Integration-side pump for the shared host router. Native calls, commit
//! receipts and demand completions run beside the agent event stream, never
//! while suspending the stream needed to fulfill a pasteboard callback.
use super::{Endpoint, Proposal, Published, Router};
use crate::{
    agent::Agent,
    agent_runtime::protocol::{Hello, clipboard_export as wire},
    clipboard_export::{
        Commit, CommittedCopy, Connection, Lease, Read,
        source::{Demand, Demands},
    },
    desktop_integration::{ClipboardAccess, Desktop},
};
use tokio::{
    io::AsyncWrite,
    task::{JoinError, JoinSet},
};

#[path = "clipboard_router_route.rs"]
pub(crate) mod route;

#[cfg(unix)]
#[path = "clipboard_router_files.rs"]
pub(crate) mod files;

pub(crate) trait Transport: Send {
    fn lease(&self) -> Option<Lease>;
    fn commit(
        &mut self,
        lease: &Lease,
    ) -> impl std::future::Future<Output = Result<Commit, String>> + Send;
    fn read(
        &mut self,
        copy: &CommittedCopy,
        format: &str,
    ) -> impl std::future::Future<Output = Result<Read, String>> + Send;
}
impl<W: AsyncWrite + Unpin + Send> Transport for Agent<W> {
    fn lease(&self) -> Option<Lease> {
        self.clipboard_export_lease()
    }
    async fn commit(&mut self, lease: &Lease) -> Result<Commit, String> {
        Ok(self.commit_clipboard_copy(lease).await?)
    }
    async fn read(&mut self, copy: &CommittedCopy, format: &str) -> Result<Read, String> {
        Ok(self.read_clipboard_copy(copy, format).await?)
    }
}

pub(crate) struct Driver {
    router: Router,
    pub(crate) endpoint: Endpoint,
    demands: Demands,
    commits: JoinSet<Receipt>,
    publications: JoinSet<Option<Published>>,
    reads: JoinSet<()>,
    metadata: JoinSet<()>,
    routes: Option<route::Routes>,
    /// Downloads the files of this server's copies once they are published.
    #[cfg(unix)]
    files: Option<std::sync::Arc<dyn files::Download>>,
    downloads: JoinSet<()>,
}
pub(crate) struct Receipt {
    proposal: Proposal,
    copy: Result<CommittedCopy, String>,
}
pub(crate) enum Work {
    Commit(Box<Result<Receipt, JoinError>>),
    Publication(Result<Option<Published>, JoinError>),
    Download(Result<(), JoinError>),
    Read(Result<(), JoinError>),
    Metadata(Result<(), JoinError>),
    Demand(Demand),
    Route(route::Work),
}
impl Driver {
    pub(crate) async fn attach<W: AsyncWrite + Unpin + Send>(
        app: &Desktop,
        agent: &mut Agent<W>,
        hello: &Hello,
        owner: &std::sync::Arc<crate::server_session::ServerSession>,
        server: &str,
    ) -> Result<Option<Self>, String> {
        let Some(router) = app.clipboard_router() else {
            return Ok(None);
        };
        if hello.require(wire::CAPABILITY, wire::COPY_VERSION).is_err() {
            return Ok(None);
        }
        agent.enable_clipboard_copies(hello).await?;
        let connection = agent
            .clipboard_export_connection()
            .ok_or("Clipboard integration is not negotiated")?;
        let mut driver = Self::new(router, connection)?;
        driver.routes = Some(route::Routes::new(
            &driver.router,
            route::Backend::bind(owner),
        ));
        #[cfg(unix)]
        {
            driver.files = Some(files::Files::new(
                crate::clipboard_files::Shared(owner.clone()),
                server.to_owned(),
                app.clone(),
            ));
        }
        #[cfg(not(unix))]
        let _ = server;
        Ok(Some(driver))
    }
    pub(crate) fn new(router: Router, connection: Connection) -> Result<Self, String> {
        let endpoint = router
            .bind_connection(connection)
            .ok_or("Clipboard integration is already registered or retired")?;
        Ok(Self {
            router,
            endpoint,
            demands: Demands::default(),
            commits: JoinSet::new(),
            publications: JoinSet::new(),
            reads: JoinSet::new(),
            metadata: JoinSet::new(),
            routes: None,
            #[cfg(unix)]
            files: None,
            downloads: JoinSet::new(),
        })
    }
    #[cfg(all(test, unix))]
    pub(crate) fn with_files(mut self, files: std::sync::Arc<dyn files::Download>) -> Self {
        self.files = Some(files);
        self
    }
    #[cfg(test)]
    pub(crate) fn with_route_control(
        router: Router,
        connection: Connection,
        control: std::sync::Arc<dyn route::Control>,
    ) -> Result<Self, String> {
        let mut driver = Self::new(router, connection)?;
        driver.routes = Some(route::Routes::new(&driver.router, control));
        Ok(driver)
    }
    /// Called on the existing integration timer; construction starts no native
    /// owner and an idle, unfocused connection performs no clipboard polling.
    pub(crate) fn tick(&mut self) {
        if let Some(routes) = &mut self.routes {
            routes.refresh(&self.endpoint);
        }
        self.router.tick_downloads();
        if self.metadata.is_empty() && self.endpoint.observes_clipboard() {
            let router = self.router.clone();
            self.metadata.spawn(async move {
                let _ = router.offer(None).await;
            });
        }
    }
    /// Retry the latest receiver offer after pending work settles. The endpoint
    /// remembers already proposed source revisions across focus changes.
    pub(crate) async fn propose(&mut self, agent: &mut impl Transport) -> Result<(), String> {
        if !self.commits.is_empty() || !self.publications.is_empty() {
            return Ok(());
        }
        let Some(proposal) = agent.lease().and_then(|offer| self.endpoint.propose(offer)) else {
            return Ok(());
        };
        let commit = match agent.commit(proposal.lease()).await {
            Ok(commit) => commit,
            // The maintenance write preceding Commit can receive a newer
            // source or focus withdrawal. Refuse this proposal without
            // restarting an otherwise healthy integration connection.
            Err(_) if !proposal.is_current() => return Ok(()),
            Err(error) => return Err(error),
        };
        self.commits.spawn(async move {
            Receipt {
                proposal,
                copy: commit.receive().await,
            }
        });
        Ok(())
    }
    pub(crate) async fn next(&mut self) -> Work {
        tokio::select! {
            work = async { self.routes.as_mut().unwrap().next().await }, if self.routes.is_some() => Work::Route(work),
            result = self.commits.join_next(), if !self.commits.is_empty() => Work::Commit(Box::new(result.unwrap())),
            result = self.publications.join_next(), if !self.publications.is_empty() => Work::Publication(result.unwrap()),
            result = self.downloads.join_next(), if !self.downloads.is_empty() => Work::Download(result.unwrap()),
            result = self.reads.join_next(), if !self.reads.is_empty() => Work::Read(result.unwrap()),
            result = self.metadata.join_next(), if !self.metadata.is_empty() => Work::Metadata(result.unwrap()),
            Some(demand) = self.demands.receive() => Work::Demand(demand),
        }
    }
    pub(crate) async fn complete(
        &mut self,
        work: Work,
        agent: &mut impl Transport,
    ) -> Result<(), String> {
        match work {
            Work::Route(work) => {
                self.routes
                    .as_mut()
                    .unwrap()
                    .complete(work, &self.endpoint)?;
            }
            Work::Commit(result) => {
                let Receipt { proposal, copy } = (*result).map_err(|e| e.to_string())?;
                if let Ok(copy) = copy {
                    self.publications
                        .spawn(self.router.publish(proposal, copy, &self.demands));
                }
            }
            Work::Demand(demand) => {
                if demand.is_current()
                    && let Some(copy) = demand.copy()
                {
                    // The bounded demand queue may race source revocation or
                    // receiver cancellation credit. Refuse this paste demand;
                    // transport failures also fence Agent's event receiver.
                    if let Ok(read) = agent.read(copy, demand.format()).await {
                        self.reads.spawn(demand.complete(read));
                    }
                }
            }
            Work::Publication(result) => {
                let published = result.map_err(|e| e.to_string())?;
                #[cfg(unix)]
                if let (Some(published), Some(files)) = (published, &self.files) {
                    // A newer copy supersedes a running download, which then
                    // ends on its own.
                    self.downloads
                        .spawn(files.clone().run(self.router.clone(), published));
                }
                #[cfg(not(unix))]
                let _ = published;
            }
            Work::Read(result) | Work::Metadata(result) | Work::Download(result) => {
                result.map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
}
impl Drop for Driver {
    fn drop(&mut self) {
        // Revoke before aborting tasks: externally held sources cannot outlive
        // teardown while native publication futures are still being dropped.
        self.endpoint.revoke();
        self.commits.abort_all();
        self.publications.abort_all();
        self.reads.abort_all();
        self.metadata.abort_all();
        self.downloads.abort_all();
    }
}
