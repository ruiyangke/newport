//! System events are recovery hints, not evidence that an SSH server is reachable.
use std::{
    sync::{Mutex, OnceLock},
    time::Duration,
};
use tokio::sync::watch;

#[derive(Clone, Copy, Default)]
struct State {
    sleeping: bool,
    sleep_epoch: u64,
}

fn state() -> &'static watch::Sender<State> {
    static STATE: OnceLock<watch::Sender<State>> = OnceLock::new();
    STATE.get_or_init(|| watch::channel(State::default()).0)
}

type Listener = Box<dyn FnMut(Event) -> bool + Send>;

fn listeners() -> &'static Mutex<Vec<Listener>> {
    static LISTENERS: OnceLock<Mutex<Vec<Listener>>> = OnceLock::new();
    LISTENERS.get_or_init(Mutex::default)
}

/// Calls `listener` with every event from now on, until it returns false.
pub fn listen(listener: impl FnMut(Event) -> bool + Send + 'static) {
    listeners()
        .lock()
        .expect("recovery listeners")
        .push(Box::new(listener));
}

#[derive(Clone)]
pub struct Recovery(watch::Receiver<State>);

pub fn subscribe() -> Recovery {
    Recovery(state().subscribe())
}

impl Recovery {
    pub fn sleep_epoch(&self) -> u64 {
        self.0.borrow().sleep_epoch
    }

    pub fn sleeping(&self) -> bool {
        self.0.borrow().sleeping
    }

    pub async fn changed(&mut self) {
        let _ = self.0.changed().await;
    }

    /// Check the existing authenticated connection after the OS resumes and
    /// network notifications settle. A failed check never replays application work.
    pub async fn revalidate(&mut self, probe: impl std::future::Future<Output = bool>) -> bool {
        self.ready().await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        matches!(
            tokio::time::timeout(Duration::from_secs(3), probe).await,
            Ok(true)
        )
    }

    pub async fn ready(&mut self) {
        while self.0.borrow_and_update().sleeping {
            if self.0.changed().await.is_err() {
                return;
            }
        }
    }

    /// Wake/network changes shorten backoff. Coalesce bursts before retrying.
    /// Callers select against cancellation so manual disconnect always wins.
    pub async fn wait(&mut self, delay: Duration) {
        tokio::select! {
            _ = tokio::time::sleep(delay) => {},
            _ = self.0.changed() => {
                tokio::time::sleep(Duration::from_millis(500)).await;
            },
        }
        self.ready().await;
    }
}

pub use newport_platform::events::Event;

/// The application supplies OS lifecycle hints; they never prove reachability.
pub fn report(event: Event) {
    publish(state(), event);
    listeners()
        .lock()
        .expect("recovery listeners")
        .retain_mut(|listener| listener(event));
}

fn publish(events: &watch::Sender<State>, event: Event) {
    events.send_modify(|state| match event {
        Event::Sleep | Event::Wake => {
            state.sleeping = matches!(event, Event::Sleep);
            state.sleep_epoch = state.sleep_epoch.wrapping_add(1);
        }
        Event::NetworkChanged | Event::Unlocked => {}
    });
}

#[cfg(test)]
pub(crate) fn fixture() -> (Recovery, impl Fn(Event)) {
    let (tx, rx) = watch::channel(State::default());
    (Recovery(rx), move |event| publish(&tx, event))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn wake_check_waits_for_wake_then_bounds_a_stale_socket() {
        let (mut recovery, report) = fixture();
        report(Event::Sleep);
        let check = tokio::spawn(async move { recovery.revalidate(std::future::pending()).await });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(3600)).await;
        assert!(!check.is_finished(), "do not probe or retry while asleep");
        report(Event::Wake);
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(500)).await;
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(3)).await;
        assert!(
            !check.await.unwrap(),
            "silent old sockets must fail the bounded check"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn network_check_preserves_responsive_connection() {
        let (mut recovery, report) = fixture();
        report(Event::NetworkChanged);
        recovery.changed().await;
        assert!(recovery.revalidate(async { true }).await);
    }

    #[tokio::test]
    async fn sleep_blocks_recovery_until_wake() {
        let (tx, rx) = watch::channel(State {
            sleeping: true,
            ..State::default()
        });
        let mut recovery = Recovery(rx);
        assert!(
            tokio::time::timeout(Duration::from_millis(10), recovery.ready())
                .await
                .is_err()
        );
        tx.send_modify(|s| s.sleeping = false);
        tokio::time::timeout(Duration::from_millis(100), recovery.ready())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn network_event_shortens_backoff() {
        let (tx, rx) = watch::channel(State::default());
        let mut recovery = Recovery(rx);
        tx.send_modify(|_| {});
        tokio::time::timeout(
            Duration::from_secs(2),
            recovery.wait(Duration::from_secs(30)),
        )
        .await
        .unwrap();
    }
}
