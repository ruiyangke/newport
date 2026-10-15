//! Native startup recovery. Resolve installation intent before reading the
//! profile; accept a backup only after UI initialization and backend health.
use super::install::{Installation, bundle_for_executable};
use newport_platform::updates::macos::{RecoveryStatus, StartupGuard, Transaction};
use std::{
    future::Future,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
};
use tokio::{sync::oneshot, task::JoinHandle};

pub fn guard() -> Result<StartupGuard, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    StartupGuard::acquire(&bundle_for_executable(&executable)?)
}

pub enum Startup {
    Continue(Transaction),
    Restart(Installation),
}

pub fn resolve(mut transaction: Transaction) -> Result<Startup, String> {
    let status = transaction.inspect()?;
    let running =
        semver::Version::parse(env!("CARGO_PKG_VERSION")).map_err(|error| error.to_string())?;
    if can_continue(&status, &running) {
        Ok(Startup::Continue(transaction))
    } else {
        Ok(Startup::Restart(Installation::resume(
            transaction,
            status.intended,
            status.accepted,
        )))
    }
}

fn can_continue(status: &RecoveryStatus, running: &semver::Version) -> bool {
    // A process can still be executing the old inode after an atomic exchange.
    // Neither the on-disk version nor the executable version alone is enough.
    status.installed == status.intended && running == &status.intended
}

const PENDING: u8 = 0;
const CANCELLED: u8 = 1;
const ACCEPTING: u8 = 2;

pub struct Acceptance {
    state: Arc<AtomicU8>,
    stop: oneshot::Sender<()>,
    task: JoinHandle<Result<(), String>>,
}
impl Acceptance {
    pub fn start(
        transaction: Transaction,
        ready: oneshot::Receiver<()>,
        endpoint: PathBuf,
    ) -> Self {
        Self::spawn(
            ready,
            async move { crate::health::read(endpoint).await.map(|_| ()) },
            move || {
                let running = semver::Version::parse(env!("CARGO_PKG_VERSION"))
                    .map_err(|error| error.to_string())?;
                transaction.accept(&running)
            },
        )
    }

    fn spawn(
        ready: oneshot::Receiver<()>,
        health: impl Future<Output = Result<(), String>> + Send + 'static,
        accept: impl FnOnce() -> Result<(), String> + Send + 'static,
    ) -> Self {
        let state = Arc::new(AtomicU8::new(PENDING));
        let acceptance = state.clone();
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            let initialized = async {
                ready
                    .await
                    .map_err(|_| "The UI did not initialize; update backup retained.".to_owned())?;
                health.await
            };
            tokio::select! {
                biased;
                _ = stopped => return Ok(()),
                result = initialized => result?,
            }
            tokio::task::spawn_blocking(move || {
                // Linearize acceptance against Quit. A cleanup already begun
                // finishes before backend shutdown; otherwise preserve backup.
                if acceptance
                    .compare_exchange(PENDING, ACCEPTING, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
                {
                    return Ok(());
                }
                accept()
            })
            .await
            .map_err(|error| format!("Update acceptance failed: {error}"))?
        });
        Self { state, stop, task }
    }

    /// Cancel unfinished readiness/health checks, but join cleanup if it already
    /// started. Never leave a detached cleanup worker when releasing the profile.
    pub async fn finish(self) -> Result<(), String> {
        let _ =
            self.state
                .compare_exchange(PENDING, CANCELLED, Ordering::AcqRel, Ordering::Acquire);
        let _ = self.stop.send(());
        self.task
            .await
            .map_err(|error| format!("Update acceptance failed: {error}"))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn only_the_intended_installed_and_running_version_can_open_the_profile() {
        for accepted in [false, true] {
            for installed in ["1.0.0", "2.0.0"] {
                for intended in ["1.0.0", "2.0.0"] {
                    for running in ["1.0.0", "2.0.0", "3.0.0"] {
                        let status = RecoveryStatus {
                            installed: installed.parse().unwrap(),
                            intended: intended.parse().unwrap(),
                            accepted,
                        };
                        assert_eq!(
                            can_continue(&status, &running.parse().unwrap()),
                            installed == intended && running == intended
                        );
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn acceptance_requires_both_ui_and_health_and_preserves_backup_on_failure() {
        for healthy in [false, true] {
            let calls = Arc::new(AtomicUsize::new(0));
            let called = calls.clone();
            let (ready, initialized) = oneshot::channel();
            let (checked, check) = oneshot::channel();
            let acceptance = Acceptance::spawn(
                initialized,
                async move {
                    let _ = checked.send(());
                    if healthy {
                        Ok(())
                    } else {
                        Err("locked profile".into())
                    }
                },
                move || {
                    called.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                },
            );
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            ready.send(()).unwrap();
            check.await.unwrap();
            // Join the actual task before finish: do not race cancellation with
            // the success being tested, or treat an arbitrary sleep as proof.
            let Acceptance { task, stop, .. } = acceptance;
            assert_eq!(task.await.unwrap().is_ok(), healthy);
            drop(stop);
            assert_eq!(calls.load(Ordering::SeqCst), usize::from(healthy));
        }
    }

    #[tokio::test]
    async fn quit_cancels_missing_ui_or_pending_health_without_acceptance() {
        for initialized in [false, true] {
            let (ready, receiver) = oneshot::channel();
            let (checking, checked) = oneshot::channel();
            let acceptance = Acceptance::spawn(
                receiver,
                async move {
                    checking.send(()).unwrap();
                    std::future::pending().await
                },
                || panic!("Cancelled startup must retain its backup"),
            );
            if initialized {
                ready.send(()).unwrap();
                checked.await.unwrap();
            }
            acceptance.finish().await.unwrap();
        }
    }

    #[tokio::test]
    async fn quit_joins_cleanup_that_already_started() {
        let (ready, receiver) = oneshot::channel();
        let (began, started) = oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let acceptance = Acceptance::spawn(receiver, async { Ok(()) }, move || {
            began.send(()).unwrap();
            wait.recv().unwrap();
            Ok(())
        });
        ready.send(()).unwrap();
        started.await.unwrap();
        let mut finish = Box::pin(acceptance.finish());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut finish)
                .await
                .is_err()
        );
        release.send(()).unwrap();
        finish.await.unwrap();
    }
}
