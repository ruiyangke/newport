//! Session-owned lifecycle execution. Callers subscribe to an operation; they
//! cannot cancel its writes by dropping the subscription.
use std::{future::Future, sync::Mutex};
use tokio::{sync::watch, task::JoinHandle};
use uuid::Uuid;

pub(crate) const INTERRUPTED: &str = "Needle operation interrupted because the server session ended. Resume to reconcile its outcome.";
type Outcome = Result<String, String>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    AutoUpdate,
    Install {
        integrations: bool,
    },
    Helpers,
    Recover,
    Remove {
        build: [u8; 64],
    },
    RestoreRemoval,
    RestoreHelpers {
        build: [u8; 64],
        ownership: [u8; 64],
    },
    Keep {
        index: usize,
        fingerprint: [u8; 64],
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Request {
    pub id: Uuid,
    pub generation: u64,
    pub action: Action,
}

struct Running {
    request: Request,
    result: watch::Receiver<Option<Outcome>>,
    task: JoinHandle<()>,
}
impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}
#[derive(Default)]
pub(crate) struct Slot(Mutex<Option<Running>>);
impl Slot {
    pub fn active(&self) -> Option<Request> {
        self.0
            .lock()
            .unwrap()
            .as_ref()
            .filter(|r| {
                !r.task.is_finished()
                    && r.result.has_changed().is_ok()
                    && r.result.borrow().is_none()
            })
            .map(|r| r.request)
    }
    pub async fn run(
        &self,
        request: Request,
        mut retired: watch::Receiver<bool>,
        work: impl Future<Output = Outcome> + Send + 'static,
    ) -> Outcome {
        let mut result = {
            let mut active = self.0.lock().unwrap();
            if request.id.is_nil() {
                return Err("Invalid Needle operation ID".into());
            }
            if *retired.borrow() {
                return Err(INTERRUPTED.into());
            }
            if let Some(running) = active.as_ref().filter(|r| {
                !r.task.is_finished()
                    && r.result.has_changed().is_ok()
                    && r.result.borrow().is_none()
            }) {
                if running.request != request {
                    return Err("Another Needle lifecycle operation is running. Wait for it to finish before retrying.".into());
                }
                running.result.clone()
            } else {
                let (publish, result) = watch::channel(None);
                let task = tokio::spawn(async move {
                    let outcome = tokio::select! {
                        biased;
                        _ = async {
                            while !*retired.borrow_and_update() {
                                if retired.changed().await.is_err() { break; }
                            }
                        } => Err(INTERRUPTED.into()),
                        outcome = work => outcome,
                    };
                    publish.send_replace(Some(outcome));
                });
                *active = Some(Running {
                    request,
                    result: result.clone(),
                    task,
                });
                result
            }
        };
        loop {
            if let Some(outcome) = result.borrow_and_update().clone() {
                return outcome;
            }
            result.changed().await.map_err(|_| {
                "Needle operation stopped unexpectedly. Resume to reconcile its outcome.".to_owned()
            })?;
        }
    }

    pub async fn stop(&self) {
        let active = self.0.lock().unwrap().take();
        if let Some(mut active) = active {
            active.task.abort();
            let _ = (&mut active.task).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::sync::oneshot;

    fn request() -> Request {
        Request {
            id: Uuid::new_v4(),
            generation: 1,
            action: Action::Helpers,
        }
    }
    struct Active(Arc<AtomicUsize>);
    impl Drop for Active {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn cancellation_releases_only_the_waiter_and_duplicates_join() {
        let slot = Arc::new(Slot::default());
        let (_retire, retired) = watch::channel(false);
        let req = request();
        let (started, ready) = oneshot::channel();
        let (finish, wait) = oneshot::channel();
        let first = tokio::spawn({
            let slot = slot.clone();
            let retired = retired.clone();
            async move {
                slot.run(req, retired, async move {
                    started.send(()).unwrap();
                    wait.await.unwrap();
                    Ok("original result".into())
                })
                .await
            }
        });
        ready.await.unwrap();
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        let duplicate = slot.run(req, retired.clone(), async { panic!("duplicate ran") });
        tokio::pin!(duplicate);
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(
                duplicate.as_mut().poll(cx).is_pending()
            ))
            .await
        );
        for other in [
            Request {
                id: Uuid::new_v4(),
                ..req
            },
            Request {
                action: Action::Recover,
                ..req
            },
            Request {
                generation: 2,
                ..req
            },
        ] {
            assert!(
                slot.run(other, retired.clone(), async {
                    panic!("conflicting work ran")
                })
                .await
                .unwrap_err()
                .contains("Another")
            );
        }
        finish.send(()).unwrap();
        assert_eq!(duplicate.await.unwrap(), "original result");
        slot.stop().await;
    }

    #[tokio::test]
    async fn retirement_cancels_work_and_rejects_late_admission() {
        let slot = Arc::new(Slot::default());
        let (retire, retired) = watch::channel(false);
        let active = Arc::new(AtomicUsize::new(1));
        let (started, ready) = oneshot::channel();
        let waiter = tokio::spawn({
            let slot = slot.clone();
            let retired = retired.clone();
            let active = active.clone();
            async move {
                slot.run(request(), retired, async move {
                    let _active = Active(active);
                    started.send(()).unwrap();
                    std::future::pending::<Outcome>().await
                })
                .await
            }
        });
        ready.await.unwrap();
        retire.send_replace(true);
        assert_eq!(waiter.await.unwrap().unwrap_err(), INTERRUPTED);
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert_eq!(
            slot.run(request(), retired, async { panic!("retired work ran") })
                .await
                .unwrap_err(),
            INTERRUPTED
        );
        slot.stop().await;
    }

    #[tokio::test]
    async fn shutdown_waits_for_work_to_drop() {
        let slot = Arc::new(Slot::default());
        let (_retire, retired) = watch::channel(false);
        let active = Arc::new(AtomicUsize::new(1));
        let (started, ready) = oneshot::channel();
        let waiter = tokio::spawn({
            let slot = slot.clone();
            let active = active.clone();
            async move {
                slot.run(request(), retired, async move {
                    let _active = Active(active);
                    started.send(()).unwrap();
                    std::future::pending::<Outcome>().await
                })
                .await
            }
        });
        ready.await.unwrap();
        slot.stop().await;
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert!(waiter.await.unwrap().unwrap_err().contains("reconcile"));
        slot.stop().await;
    }

    #[tokio::test]
    async fn executor_panic_wakes_waiters_and_allows_reconciliation() {
        let slot = Slot::default();
        let (_retire, retired) = watch::channel(false);
        let req = request();
        assert!(
            slot.run(req, retired.clone(), async {
                panic!("fixture executor panic")
            })
            .await
            .unwrap_err()
            .contains("reconcile")
        );
        assert_eq!(
            slot.run(req, retired, async { Ok("reconciled".into()) })
                .await
                .unwrap(),
            "reconciled"
        );
        slot.stop().await;
    }
}
