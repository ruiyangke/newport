//! A terminal and the account's display on its server. Where this computer
//! may use the server's Remote apps, every terminal's shell names the display
//! (Needle's `terminal display`), and someone using the tab here shows the
//! display here, starting it when it isn't running. The display never delays,
//! blocks or ends the shell.
use crate::{
    account::Account,
    desktop_integration::Desktop,
    graphical_session::{self, Demand},
    preferences::{Preferences, remote_apps},
    server_session::ServerSession,
};
use std::{future::Future, pin::Pin, sync::Arc};

/// Builds the factory for the account's display on demand.
pub(crate) type Source = Arc<dyn Fn() -> graphical_session::FactoryFuture + Send + Sync>;
/// Local checks that this computer may show a server's display now.
pub(crate) type Check =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> + Send + Sync>;

/// The server's display as this computer may use it: only for a server set
/// up for Remote apps. The checks are local and do not open a connection,
/// install an agent, prompt for sign-in or change sharing permissions;
/// refusals leave an ordinary shell.
#[derive(Clone)]
pub(crate) struct Access {
    allowed: Check,
    /// Makes the checks, then the factory.
    source: Source,
    #[cfg(test)]
    shown: Option<View>,
}
impl Access {
    pub(crate) fn new(allowed: Check, factory: Source) -> Self {
        let checks = allowed.clone();
        let source: Source = Arc::new(move || {
            let (checks, factory) = (checks.clone(), factory.clone());
            Box::pin(async move {
                checks().await?;
                factory().await
            })
        });
        Self {
            allowed,
            source,
            #[cfg(test)]
            shown: None,
        }
    }
    /// Access whose display `view` shows.
    #[cfg(test)]
    pub(crate) fn showing(view: View) -> Self {
        Self {
            allowed: Arc::new(|| Box::pin(async { Ok(()) })),
            source: Arc::new(|| Box::pin(std::future::pending())),
            shown: Some(view),
        }
    }
    /// Whether this computer may show the display now.
    pub(crate) async fn allowed(&self) -> Result<(), String> {
        (self.allowed)().await
    }
    pub(crate) fn source(&self) -> Source {
        self.source.clone()
    }
}
/// The display of `server` for a tab opening now, when this computer may show
/// it: the server is set up for Remote apps here and the account is entitled
/// to them. Both are read once from what this computer knows, before the
/// shell starts, so no shell waits on the account or the network; an
/// entitlement not known yet counts as none. Otherwise the shell gets a
/// clipboard display of its own, even while another computer shows the
/// account's display. A license that lapses later leaves names that reach no
/// display, which programs treat as no display at all.
pub(crate) async fn access(
    server: uuid::Uuid,
    account: Account,
    preferences: Preferences,
    desktop: Desktop,
) -> Option<Access> {
    if !desktop.supports_shared_graphical()
        || !account.allows_remote_sessions_now()
        || !remote_apps::remote_apps_installed(&preferences)
            .await
            .is_ok_and(|servers| servers.contains(&server))
    {
        return None;
    }
    let allowed: Check = {
        let (account, preferences) = (account.clone(), preferences.clone());
        Arc::new(move || {
            let (account, preferences) = (account.clone(), preferences.clone());
            Box::pin(async move {
                if !remote_apps::remote_apps_installed(&preferences)
                    .await?
                    .contains(&server)
                {
                    return Err("Remote apps isn’t set up for this server".into());
                }
                if !account.allows_remote_sessions().await {
                    return Err("This computer has no Remote apps access".into());
                }
                Ok(())
            })
        })
    };
    let factory: Source =
        Arc::new(move || shared_factory(account.clone(), preferences.clone(), desktop.clone()));
    Some(Access::new(allowed, factory))
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

/// How showing the account's display here ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shown {
    /// It showed here until it was lost, as when another computer took it.
    Lost,
    /// It couldn't show here, as when the display didn't start.
    Failed,
    /// This computer may not show it now, as without a license.
    Refused,
}

/// Shows the account's display on this computer.
#[derive(Clone)]
pub(crate) struct View {
    /// Acquires the display, starting it when it isn't running, and holds it
    /// until it is lost.
    show: Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Shown> + Send>> + Send + Sync>,
    /// The terminal was used while the display shows here: what the
    /// display's programs copy reaches this computer, rather than another
    /// shown server's.
    stand: Arc<dyn Fn() + Send + Sync>,
}

/// Shows the account's display on this computer from when someone uses the
/// terminal here until another computer takes it, or becomes a held shell's
/// active one. The terminal never waits for
/// it, and its failures only reach the log.
pub(crate) struct Viewing {
    view: Option<View>,
    task: Option<tokio::task::JoinHandle<Shown>>,
}

impl Viewing {
    pub(crate) fn new(view: Option<View>) -> Self {
        Self { view, task: None }
    }

    /// Someone used the terminal on this computer: it shows the display
    /// unless it shows it already or is still asking, and the display takes
    /// copies here.
    pub(crate) fn activated(&mut self) {
        let Some(view) = &self.view else {
            return;
        };
        if self.task.as_ref().is_none_or(|task| task.is_finished()) {
            self.task = Some(tokio::spawn((view.show)()));
        } else {
            (view.stand)();
        }
    }

    /// The display stopped showing here by itself, or couldn't show: the
    /// tab's next use asks again. Not while it shows, nor when this computer
    /// may not show it, which no keystroke would change.
    pub(crate) async fn lost(&mut self) {
        if let Some(task) = &mut self.task {
            let shown = task.await.unwrap_or(Shown::Refused);
            self.task = None;
            if shown != Shown::Refused {
                return;
            }
        }
        std::future::pending().await
    }

    /// Another computer became a held shell's active one.
    pub(crate) fn inactive(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }

    #[cfg(all(test, unix))]
    pub(crate) fn asking(&self) -> bool {
        self.task.is_some()
    }
}

impl Drop for Viewing {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Shows the account's display through this server's Remote apps, where the
/// account may use them.
pub(crate) fn view(owner: &Arc<ServerSession>, access: Access) -> View {
    #[cfg(test)]
    if let Some(view) = &access.shown {
        return view.clone();
    }
    let stand: Arc<dyn Fn() + Send + Sync> = {
        let owner = Arc::downgrade(owner);
        Arc::new(move || {
            if let Some(owner) = owner.upgrade() {
                owner.graphical.stand();
            }
        })
    };
    let owner = owner.clone();
    let show = Arc::new(move || {
        let (owner, access) = (owner.clone(), access.clone());
        Box::pin(async move {
            if let Err(error) = access.allowed().await {
                log::debug!("The terminal in use doesn't show the server's apps here: {error}");
                return Shown::Refused;
            }
            let acquired = async {
                let factory = access.source()().await?;
                owner
                    .acquire_graphical(Demand::ActiveTerminal, factory)
                    .await
            }
            .await;
            match acquired {
                Ok(lease) => {
                    // A display already running, shown for another tab, may
                    // not be the one taking copies.
                    lease.stand();
                    let reason = lease.unavailable().await;
                    log::info!(
                        "The server's apps stopped showing for the terminal in use: {reason}"
                    );
                    Shown::Lost
                }
                Err(error) => {
                    log::info!("The terminal in use doesn't show the server's apps here: {error}");
                    Shown::Failed
                }
            }
        }) as Pin<Box<dyn Future<Output = Shown> + Send>>
    });
    View { show, stand }
}

#[cfg(test)]
pub(crate) use fake::FakeView;
#[cfg(test)]
mod fake {
    use super::{Shown, View};
    use std::{
        future::Future,
        pin::Pin,
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::Duration,
    };

    /// A display view that counts its starts, the views it holds and the
    /// uses while it shows. It fails at once while `failing` is set, is
    /// refused while `refused` is, and loses the display, as to a takeover,
    /// when `taken` is notified.
    pub(crate) struct FakeView {
        pub stood: Arc<AtomicUsize>,
        pub started: Arc<AtomicUsize>,
        pub holding: Arc<AtomicUsize>,
        pub failing: Arc<AtomicBool>,
        pub refused: Arc<AtomicBool>,
        pub taken: Arc<tokio::sync::Notify>,
    }

    struct Held(Arc<AtomicUsize>);
    impl Drop for Held {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::AcqRel);
        }
    }

    impl FakeView {
        pub fn new() -> Self {
            Self {
                stood: Default::default(),
                started: Default::default(),
                holding: Default::default(),
                failing: Default::default(),
                refused: Default::default(),
                taken: Default::default(),
            }
        }
        pub fn view(&self) -> View {
            let (started, holding, failing, refused, taken) = (
                self.started.clone(),
                self.holding.clone(),
                self.failing.clone(),
                self.refused.clone(),
                self.taken.clone(),
            );
            let stood = self.stood.clone();
            let show = Arc::new(move || {
                let (started, holding, failing, refused, taken) = (
                    started.clone(),
                    holding.clone(),
                    failing.clone(),
                    refused.clone(),
                    taken.clone(),
                );
                Box::pin(async move {
                    started.fetch_add(1, Ordering::AcqRel);
                    if refused.load(Ordering::Acquire) {
                        return Shown::Refused;
                    }
                    if failing.load(Ordering::Acquire) {
                        return Shown::Failed;
                    }
                    holding.fetch_add(1, Ordering::AcqRel);
                    let _held = Held(holding);
                    taken.notified().await;
                    Shown::Lost
                }) as Pin<Box<dyn Future<Output = Shown> + Send>>
            });
            View {
                show,
                stand: Arc::new(move || {
                    stood.fetch_add(1, Ordering::AcqRel);
                }),
            }
        }
        pub fn counts(&self) -> (usize, usize) {
            (
                self.started.load(Ordering::Acquire),
                self.holding.load(Ordering::Acquire),
            )
        }
        pub async fn until(&self, counts: (usize, usize)) {
            let reached = async {
                while self.counts() != counts {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            };
            if tokio::time::timeout(Duration::from_secs(5), reached)
                .await
                .is_err()
            {
                panic!("views {:?}, expected {counts:?}", self.counts());
            }
        }
        /// The counts stay as they are.
        pub async fn stays(&self, counts: (usize, usize)) {
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert_eq!(self.counts(), counts);
        }
    }
}
