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

#[cfg(target_os = "macos")]
pub fn install() -> anyhow::Result<()> {
    use block2::RcBlock;
    use core_foundation::{
        array::CFArray,
        runloop::{kCFRunLoopCommonModes, CFRunLoop},
        string::CFString,
    };
    use objc2_app_kit::{
        NSWorkspace, NSWorkspaceDidWakeNotification, NSWorkspaceWillSleepNotification,
    };
    use system_configuration::dynamic_store::{
        SCDynamicStoreBuilder, SCDynamicStoreCallBackContext,
    };

    // Install on Tauri's main thread; its AppKit run loop delivers both sources.
    let store = SCDynamicStoreBuilder::new("Porthop network recovery")
        .callback_context(SCDynamicStoreCallBackContext {
            callout: |_, _, _: &mut ()| {
                log::debug!("Network configuration changed; notifying recovery tasks");
                events().send_modify(|_| {});
            },
            info: (),
        })
        .build()
        .ok_or_else(|| anyhow::anyhow!("Cannot create network observer"))?;
    let patterns = CFArray::from_CFTypes(&[
        CFString::from("State:/Network/Global/.*"),
        CFString::from("State:/Network/Service/.*/(IPv4|IPv6|DNS)"),
        CFString::from("State:/Network/Interface/.*/Link"),
    ]);
    anyhow::ensure!(
        store.set_notification_keys(&CFArray::<CFString>::from_CFTypes(&[]), &patterns),
        "Cannot subscribe to network changes"
    );
    let source = store
        .create_run_loop_source()
        .ok_or_else(|| anyhow::anyhow!("Cannot create network event source"))?;
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    // The notification center retains these observers for the application lifetime.
    // Blocks capture only a bool and access a thread-safe watch sender.
    unsafe {
        for (name, sleeping) in [
            (NSWorkspaceWillSleepNotification, true),
            (NSWorkspaceDidWakeNotification, false),
        ] {
            let callback = RcBlock::new(move |_| {
                log::info!(
                    "System {}; notifying recovery tasks",
                    if sleeping { "sleeping" } else { "awake" }
                );
                events().send_modify(|state| state.sleeping = sleeping);
            });
            let _observer = center.addObserverForName_object_queue_usingBlock(
                Some(name),
                None,
                None,
                &callback,
            );
        }
        CFRunLoop::get_current().add_source(&source, kCFRunLoopCommonModes);
    }
    // The run loop retains the source, which retains its dynamic store context.
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn install() -> anyhow::Result<()> {
    Ok(())
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
