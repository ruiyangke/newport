//! Host teardown once the UI stops. It runs from a quit observer, bounded so
//! Quit cannot hang: AppKit exits inside terminate:, and Windows exits when the
//! session ends, so the event loop does not always return.
use tokio::{sync::oneshot, task::JoinHandle};

/// Room for file-transfer cleanup (which has its own cap) and SSH disconnects.
/// Past this Quit exits anyway: transfers are journaled for the next launch,
/// and the OS and sshd release sockets, forwards and shells.
pub const LIMIT: std::time::Duration = std::time::Duration::from_secs(10);

pub struct Teardown {
    #[cfg(target_os = "macos")]
    pub acceptance: Option<crate::updates::recovery::Acceptance>,
    #[cfg(any(target_os = "macos", windows))]
    pub tray_monitor: JoinHandle<()>,
    pub signal: JoinHandle<()>,
    pub updates: crate::updates::Service,
    pub update_background: JoinHandle<()>,
    pub desktop: crate::desktop::Integration,
    pub backend: Backend,
}
impl Teardown {
    /// Stop host work. Update staging may still be cleaning its files, and the
    /// returned backend still owns the profile until it is stopped.
    async fn stop_host(
        self,
    ) -> (
        crate::updates::Service,
        Backend,
        crate::desktop::Integration,
    ) {
        #[cfg(target_os = "macos")]
        if let Some(acceptance) = self.acceptance
            && let Err(error) = acceptance.finish().await
        {
            eprintln!("Update backup retained: {error}");
        }
        #[cfg(any(target_os = "macos", windows))]
        self.tray_monitor.abort();
        self.signal.abort();
        self.updates.shutdown();
        self.update_background.abort();
        self.desktop.close();
        (self.updates, self.backend, self.desktop)
    }

    /// Stop host work and let cancelled update staging clean its files. The
    /// returned backend still owns the profile until it is stopped.
    pub async fn release_host(self) -> (Backend, Result<(), String>) {
        let (updates, backend, desktop) = self.stop_host().await;
        let (_, desktop_result) = tokio::join!(updates.finish_preparation(), desktop.shutdown());
        // Return backend ownership even when native cleanup failed: callers
        // must still drain it, and must not proceed with an update handoff.
        (backend, desktop_result)
    }

    /// The whole drain, for a process that may exit as soon as this returns.
    /// Call from a thread outside the runtime (the UI's main thread).
    /// Staging cleanup runs alongside the backend stop, so a slow preparation
    /// can't use up the limit before the backend is asked to stop.
    pub fn finish_within(
        self,
        runtime: &tokio::runtime::Handle,
        limit: std::time::Duration,
    ) -> Result<(), String> {
        runtime.block_on(async move {
            tokio::time::timeout(limit, async move {
                let (updates, backend, desktop) = self.stop_host().await;
                let (_, desktop_result, backend_result) = tokio::join!(
                    updates.finish_preparation(),
                    desktop.shutdown(),
                    backend.stop()
                );
                desktop_result.and(backend_result)
            })
            .await
            .map_err(|_| format!("Shutdown did not finish within {limit:?}; exiting anyway."))?
        })
    }
}

/// Hands the teardown to the quit observer, leaving nothing for the code after
/// the event loop. An armed update stays with that code: it drains without a
/// time limit, because the installer may start only once the backend stopped.
pub fn take_on_quit(
    teardown: &std::cell::Cell<Option<Teardown>>,
    update_armed: bool,
) -> Option<Teardown> {
    if update_armed { None } else { teardown.take() }
}

pub struct Backend {
    pub stop: oneshot::Sender<()>,
    pub services: JoinHandle<Result<(), String>>,
}
impl Backend {
    /// Release profile ownership and report how the backend ended.
    pub async fn stop(self) -> Result<(), String> {
        let _ = self.stop.send(());
        self.services
            .await
            .map_err(|error| format!("Backend shutdown failed: {error}"))
            .and_then(|result| result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn teardown(
        runtime: &tokio::runtime::Runtime,
        services: impl FnOnce(
            oneshot::Receiver<()>,
            async_channel::Receiver<crate::desktop::Action>,
        ) -> JoinHandle<Result<(), String>>,
    ) -> Teardown {
        let _entered = runtime.enter();
        let (desktop, requests) = crate::desktop::Integration::new();
        let (stop, stopped) = oneshot::channel();
        let idle = || tokio::spawn(std::future::pending::<()>());
        Teardown {
            #[cfg(target_os = "macos")]
            acceptance: None,
            #[cfg(any(target_os = "macos", windows))]
            tray_monitor: idle(),
            signal: idle(),
            updates: crate::updates::Service::disabled(),
            update_background: idle(),
            desktop,
            backend: Backend {
                stop,
                services: services(stopped, requests),
            },
        }
    }

    #[test]
    fn quit_releases_the_host_before_stopping_the_backend() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let teardown = teardown(&runtime, |stopped, requests| {
            tokio::spawn(async move {
                stopped.await.map_err(|_| "stop was never requested")?;
                // Backend shutdown must not race new main-thread dispatch.
                assert!(requests.is_closed());
                Err("backend failed".into())
            })
        });
        assert_eq!(
            teardown.finish_within(runtime.handle(), Duration::from_secs(5)),
            Err("backend failed".into())
        );
    }

    #[test]
    fn quit_leaves_nothing_for_after_the_loop_unless_an_update_is_armed() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let slot = std::cell::Cell::new(Some(teardown(&runtime, |_stopped, _requests| {
            tokio::spawn(async { Ok(()) })
        })));
        assert!(take_on_quit(&slot, true).is_none());
        let quit = take_on_quit(&slot, false).expect("quit drains when no update is armed");
        assert!(slot.take().is_none());
        assert_eq!(quit.finish_within(runtime.handle(), LIMIT), Ok(()));
    }

    #[test]
    fn quit_stops_waiting_for_a_backend_that_never_drains() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let teardown = teardown(&runtime, |_stopped, _requests| {
            tokio::spawn(std::future::pending())
        });
        let started = Instant::now();
        let result = teardown.finish_within(runtime.handle(), Duration::from_millis(50));
        assert!(result.unwrap_err().contains("did not finish"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_stuck_preparation_does_not_hold_back_the_backend_stop() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (stopped, observed) = std::sync::mpsc::channel();
        let teardown = teardown(&runtime, |requested, _requests| {
            tokio::spawn(async move {
                let _ = requested.await;
                stopped.send(()).unwrap();
                Ok(())
            })
        });
        teardown.updates.hold_preparation();
        assert!(
            teardown
                .finish_within(runtime.handle(), Duration::from_millis(200))
                .unwrap_err()
                .contains("did not finish")
        );
        observed.recv_timeout(Duration::from_secs(1)).unwrap();
    }
}
