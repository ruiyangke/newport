//! Native host update ownership. Views only consume the existing Settings RPC
//! contract; verified packages and download workers never belong to UI entities.
use newport_platform::updates::{Progress, Target, Updater, VerifiedUpdate};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const UNAVAILABLE: &str = "Updates are unavailable in this build.";

#[cfg(target_os = "macos")]
pub mod install;
#[cfg(windows)]
#[path = "updates/install_windows.rs"]
pub mod install;
#[cfg(target_os = "macos")]
pub mod recovery;
#[cfg(all(test, not(windows)))]
#[path = "updates/install_windows.rs"]
mod windows_install_tests;

pub(crate) trait Package: Send + Sync + 'static {
    fn version(&self) -> String;
}
impl Package for VerifiedUpdate {
    fn version(&self) -> String {
        self.version().to_string()
    }
}
trait Download<P>: Send + Sync {
    fn check(
        &self,
        cancelled: &dyn Fn() -> bool,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<Option<P>, String>;
}
impl Download<VerifiedUpdate> for Updater {
    fn check(
        &self,
        cancelled: &dyn Fn() -> bool,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<Option<VerifiedUpdate>, String> {
        self.check(cancelled, progress)
    }
}

pub(crate) struct Service<P = VerifiedUpdate> {
    source: Option<Arc<dyn Download<P>>>,
    shared: Arc<Shared<P>>,
}
impl<P> Clone for Service<P> {
    fn clone(&self) -> Self {
        Self {
            source: self.source.clone(),
            shared: self.shared.clone(),
        }
    }
}
struct Shared<P> {
    closed: AtomicBool,
    preparation_finished: tokio::sync::Notify,
    state: Mutex<State<P>>,
}
struct State<P> {
    phase: &'static str,
    version: Option<String>,
    downloaded: u64,
    total: Option<u64>,
    error: Option<String>,
    failure: Option<String>,
    busy: bool,
    preparing: bool,
    pending: Option<Arc<P>>,
}

fn available(debug: bool, isolated: bool, app_store: bool) -> bool {
    !debug && !isolated && !app_store
}

impl Service {
    #[cfg(any(target_os = "macos", windows))]
    pub fn enabled(&self) -> bool {
        self.source.is_some()
    }
    pub async fn install(
        &self,
        version: &str,
        desktop: crate::desktop::Integration,
    ) -> Result<(), String> {
        #[cfg(any(target_os = "macos", windows))]
        {
            self.prepare_install(version, install::Request::prepare, move |request| {
                desktop.install(request)
            })
            .await
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = (version, desktop);
            Err("Installing updates isn’t available on this platform yet.".into())
        }
    }

    pub fn disabled() -> Self {
        Self::new(None)
    }

    pub fn for_host() -> Result<Self, String> {
        if !available(
            cfg!(debug_assertions),
            newport_backend::profile::data_directory_override().is_some(),
            option_env!("NEWPORT_APP_STORE").is_some(),
        ) {
            return Ok(Self::disabled());
        }
        let target = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            Target::MacArm64
        } else if cfg!(all(windows, target_arch = "x86_64")) {
            Target::WindowsX64
        } else if cfg!(all(windows, target_arch = "aarch64")) {
            Target::WindowsArm64
        } else {
            return Ok(Self::disabled());
        };
        Ok(Self::new(Some(Arc::new(Updater::production(
            env!("CARGO_PKG_VERSION"),
            target,
        )?))))
    }
}

impl<P: Package> Service<P> {
    #[cfg(any(target_os = "macos", windows, test))]
    async fn prepare_install<I: Send + 'static>(
        &self,
        version: &str,
        prepare: impl FnOnce(&P) -> Result<I, String> + Send + 'static,
        enqueue: impl FnOnce(I) -> Result<(), String> + Send + 'static,
    ) -> Result<(), String> {
        let package = {
            let mut state = self.state();
            if self.source.is_none() {
                return Err(UNAVAILABLE.into());
            }
            if self.closed() {
                return Err("The application is closing.".into());
            }
            if state.busy {
                return Err("An update is already in progress.".into());
            }
            if state.phase != "ready" || state.version.as_deref() != Some(version) {
                return Err("The ready update changed. Review it before restarting.".into());
            }
            let package = state
                .pending
                .clone()
                .ok_or("No verified update is ready.")?;
            state.busy = true;
            state.preparing = true;
            state.phase = "installing";
            state.error = None;
            state.failure = None;
            package
        };
        let job = InstallJob {
            service: self.clone(),
            handed_off: false,
        };
        tokio::task::spawn_blocking(move || {
            let mut job = job;
            let prepared = prepare(&package)?;
            // Serialize the final handoff with shutdown. Once enqueue succeeds,
            // the host owns the preparation and no RPC cancellation can replay it.
            let mut state = job.service.state();
            if job.service.closed() {
                return Err("Installation cancelled during shutdown.".into());
            }
            enqueue(prepared)?;
            state.pending = None;
            job.handed_off = true;
            // Keep busy/installing until the app exits; a duplicate cannot stage
            // or enqueue another installation after ownership transferred.
            Ok(())
        })
        .await
        .map_err(|error| format!("Update preparation failed: {error}"))?
    }

    fn new(source: Option<Arc<dyn Download<P>>>) -> Self {
        Self {
            source,
            shared: Arc::new(Shared {
                closed: AtomicBool::new(false),
                preparation_finished: tokio::sync::Notify::new(),
                state: Mutex::new(State {
                    phase: "idle",
                    version: None,
                    downloaded: 0,
                    total: None,
                    error: None,
                    failure: None,
                    busy: false,
                    preparing: false,
                    pending: None,
                }),
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, State<P>> {
        // State mutations are small and contain no external callbacks. A worker
        // panic must leave status readable and release its operation reservation.
        self.shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    fn closed(&self) -> bool {
        self.shared.closed.load(Ordering::Acquire)
    }

    pub fn status(&self) -> Value {
        let state = self.state();
        let error = state.error.as_ref().or(state.failure.as_ref());
        json!({"enabled":self.source.is_some() && !self.closed(),"currentVersion":env!("CARGO_PKG_VERSION"),"build":crate::build_stamp::status(),"phase":state.phase,"version":state.version,"downloaded":state.downloaded,"total":state.total,"error":error})
    }

    pub async fn check(&self) -> Result<(), String> {
        let source = self.source.clone().ok_or(UNAVAILABLE)?;
        {
            let mut state = self.state();
            if self.closed() {
                return Err("The application is closing.".into());
            }
            if state.busy {
                return Err("An update is already in progress.".into());
            }
            // Do not replace a verified package while its restart confirmation
            // may be open. Only installation/explicit discard can consume it.
            if state.pending.is_some() {
                return Ok(());
            }
            state.busy = true;
            state.phase = "checking";
            state.version = None;
            state.downloaded = 0;
            state.total = None;
            state.error = None;
        }
        // Created before spawning so cancelling a queued worker also releases
        // ownership. Cancelling the RPC waiter does NOT cancel a running check.
        let job = CheckJob {
            service: self.clone(),
            completed: false,
        };
        tokio::task::spawn_blocking(move || {
            let result = source.check(&|| job.service.closed(), &mut |progress| {
                let mut state = job.service.state();
                if !job.service.closed() {
                    state.phase = "downloading";
                    state.version = Some(progress.version.to_string());
                    state.downloaded = progress.downloaded;
                    state.total = progress.total;
                }
            });
            job.finish(result)
        })
        .await
        .map_err(|error| format!("Update worker failed: {error}"))?
    }

    pub async fn background(self) {
        if self.source.is_none() || self.closed() {
            return;
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
        while !self.closed() {
            let _ = self.check().await;
            tokio::time::sleep(Duration::from_secs(6 * 60 * 60)).await;
        }
    }

    /// A previous launch failed to install and was restored. Keep the reason
    /// visible, even once the version is offered again, until the next attempt.
    #[cfg(any(target_os = "macos", test))]
    pub fn report_failure(&self, message: String) {
        self.state().failure = Some(message);
    }

    pub fn shutdown(&self) {
        self.shared.closed.store(true, Ordering::Release);
        let mut state = self.state();
        state.pending = None;
        state.phase = "idle";
        state.version = None;
        state.downloaded = 0;
        state.total = None;
        state.error = None;
    }

    /// Model a preparation that never finishes, for shutdown tests.
    #[cfg(test)]
    pub(crate) fn hold_preparation(&self) {
        self.state().preparing = true;
    }

    /// Unlike network-only checks, staging owns temporary files. Let cancelled
    /// preparation finish and clean them before tearing down the runtime.
    pub async fn finish_preparation(&self) {
        loop {
            let notified = self.shared.preparation_finished.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.state().preparing {
                return;
            }
            notified.await;
        }
    }
}

#[cfg(any(target_os = "macos", windows, test))]
struct InstallJob<P: Package> {
    service: Service<P>,
    handed_off: bool,
}
#[cfg(any(target_os = "macos", windows, test))]
impl<P: Package> Drop for InstallJob<P> {
    fn drop(&mut self) {
        let mut state = self.service.state();
        state.preparing = false;
        if !self.handed_off {
            state.busy = false;
            if !self.service.closed() {
                state.phase = "ready";
                state.error = Some("Could not prepare the update. Try again.".into());
            }
        }
        drop(state);
        self.service.shared.preparation_finished.notify_waiters();
    }
}

struct CheckJob<P: Package> {
    service: Service<P>,
    completed: bool,
}
impl<P: Package> CheckJob<P> {
    fn finish(mut self, result: Result<Option<P>, String>) -> Result<(), String> {
        // Resolve package metadata before taking the state lock. If a worker
        // fails here, the reservation guard must still publish a retryable error.
        let result = result.map(|package| package.map(|package| (package.version(), package)));
        if let Err(error) = &result {
            eprintln!("Update check failed: {error}");
        }
        let mut state = self.service.state();
        self.completed = true;
        state.busy = false;
        if self.service.closed() {
            return Err("Update check cancelled during shutdown.".into());
        }
        match result {
            Ok(Some((version, package))) => {
                state.version = Some(version);
                state.pending = Some(Arc::new(package));
                state.phase = "ready";
                Ok(())
            }
            Ok(None) => {
                state.phase = "current";
                state.version = None;
                state.downloaded = 0;
                state.total = None;
                Ok(())
            }
            Err(_) => {
                state.phase = "error";
                state.error = Some("Could not check or download the update. Try again.".into());
                Err("Could not check or download the update. Try again.".into())
            }
        }
    }
}
impl<P: Package> Drop for CheckJob<P> {
    fn drop(&mut self) {
        if !self.completed {
            let mut state = self.service.state();
            state.busy = false;
            if !self.service.closed() {
                state.phase = "error";
                state.error = Some("The update worker stopped. Try again.".into());
            }
        }
    }
}

#[cfg(test)]
mod tests;
