//! System events are recovery hints, not evidence that an SSH server is reachable.
use std::{sync::OnceLock, time::Duration};
use tokio::sync::watch;

#[derive(Clone, Copy, Default)]
struct State {
    sleeping: bool,
}

fn events() -> &'static watch::Sender<State> {
    static EVENTS: OnceLock<watch::Sender<State>> = OnceLock::new();
    EVENTS.get_or_init(|| watch::channel(State::default()).0)
}

pub struct Recovery(watch::Receiver<State>);

pub fn subscribe() -> Recovery {
    Recovery(events().subscribe())
}

impl Recovery {
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

pub fn install() -> anyhow::Result<()> {
    crate::platform::events::install(|event| {
        use crate::platform::events::Event;
        events().send_modify(|state| match event {
            Event::Sleep => state.sleeping = true,
            Event::Wake => state.sleeping = false,
            Event::NetworkChanged => {}
        });
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sleep_blocks_recovery_until_wake() {
        let (tx, rx) = watch::channel(State { sleeping: true });
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
