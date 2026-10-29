//! A terminal owns display demand below the UI and never accepts shared socket
//! paths from IPC. Every terminal joins its server's shared display when it
//! can; the display never delays, blocks or ends the shell.
use crate::{
    account::Account,
    desktop_integration::Desktop,
    graphical_session,
    preferences::{Preferences, remote_apps},
    server_session::ServerSession,
};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

/// How long a new shell waits to join a shared display that is already running.
/// A display that is still starting, or not running, is never waited for.
pub(crate) const JOIN_WAIT: Duration = Duration::from_millis(500);

/// Captures local account/settings access only. Calling it is deferred until an
/// explicit GUI request has completed the desktop's setup guidance.
pub(crate) type Source = Arc<dyn Fn() -> graphical_session::FactoryFuture + Send + Sync>;
pub(crate) fn handoff_source(
    account: Account,
    preferences: Preferences,
    desktop: Desktop,
) -> Source {
    Arc::new(move || shared_factory(account.clone(), preferences.clone(), desktop.clone()))
}
/// The display a new shell may join: only for a server set up for Remote apps
/// and an eligible account. These checks are local and do not open a
/// connection, install an agent, prompt for sign-in or change sharing
/// permissions; refusals leave an ordinary shell.
pub(crate) fn join_source(
    server: uuid::Uuid,
    account: Account,
    preferences: Preferences,
    desktop: Desktop,
) -> Source {
    Arc::new(move || {
        let (account, preferences, desktop) =
            (account.clone(), preferences.clone(), desktop.clone());
        Box::pin(async move {
            if !desktop.supports_shared_graphical()
                || !remote_apps::remote_apps_installed(&preferences)
                    .await?
                    .contains(&server)
            {
                return Err("Remote apps isn’t set up for this server".into());
            }
            if !account.allows_remote_sessions().await {
                return Err("This computer has no Remote apps access".into());
            }
            shared_factory(account, preferences, desktop).await
        })
    })
}

#[derive(Default)]
pub(crate) enum Display {
    #[default]
    None,
    /// Join the server's shared display if one is already running.
    Implicit(Source),
    /// A display already prepared for this shell.
    #[cfg(all(test, unix))]
    Ready(Prepared),
}
pub(crate) enum Prepared {
    None,
    Shared(graphical_session::Lease),
}
impl Display {
    pub async fn prepare(self, owner: &Arc<ServerSession>) -> anyhow::Result<Prepared> {
        match self {
            Self::None => Ok(Prepared::None),
            Self::Implicit(source) => Ok(join(owner, source, JOIN_WAIT).await),
            #[cfg(all(test, unix))]
            Self::Ready(prepared) => Ok(prepared),
        }
    }
}
/// The shell starts without a display unless one is ready within `wait`.
/// Every refusal or failure is logged and leaves an ordinary shell.
pub(crate) async fn join(owner: &Arc<ServerSession>, source: Source, wait: Duration) -> Prepared {
    let running = owner
        .graphical_sessions()
        .iter()
        .any(|display| display.phase == graphical_session::Phase::Ready);
    if !running {
        return Prepared::None;
    }
    let acquire = async {
        let factory = source().await?;
        owner
            .acquire_graphical(graphical_session::Demand::Terminal, factory)
            .await
    };
    match tokio::time::timeout(wait, acquire).await {
        Ok(Ok(lease)) => Prepared::Shared(lease),
        Ok(Err(error)) => {
            log::info!("Terminal starts without the shared display: {error}");
            Prepared::None
        }
        Err(_) => {
            log::info!("Terminal starts without the shared display: it was not ready in time");
            Prepared::None
        }
    }
}
pub(crate) fn shared_factory(
    account: Account,
    preferences: Preferences,
    desktop: Desktop,
) -> graphical_session::FactoryFuture {
    Box::pin(async move {
        let license = account
            .remote_license()
            .await
            .map_err(|error| error.message)?;
        let settings = remote_apps::remote_apps_settings(&preferences).await?;
        desktop.graphical_factory(license, settings).await
    })
}
/// A shared display failed while its shell started. The shell starts again
/// without it; this never reaches the terminal as an error.
#[derive(Debug)]
pub(crate) struct Unavailable(pub String);
impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for Unavailable {}

impl Prepared {
    pub fn failure(&self, error: String) -> anyhow::Error {
        match self {
            Self::Shared(_) => Unavailable(error).into(),
            _ => anyhow::Error::msg(error),
        }
    }
    /// Revalidate the owned generation for every shell, including reconnects.
    /// Never silently give a replacement shell a retired display's endpoints.
    pub fn assignments(
        &self,
        session: uuid::Uuid,
    ) -> Result<BTreeMap<String, Option<String>>, String> {
        match self {
            Self::None => Ok(BTreeMap::new()),
            Self::Shared(lease) => {
                let environment = lease.environment()?;
                if environment.session != session {
                    return Err("The graphical display belongs to another server session".into());
                }
                environment.shell_assignments()
            }
        }
    }
}
