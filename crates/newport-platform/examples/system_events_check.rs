//! Exercise process-local AppKit notification delivery without sleeping the OS.
#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    use newport_platform::events::{Event, install};
    use objc2_app_kit::{
        NSWorkspace, NSWorkspaceDidWakeNotification, NSWorkspaceWillSleepNotification,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SLEEP: AtomicUsize = AtomicUsize::new(0);
    static WAKE: AtomicUsize = AtomicUsize::new(0);
    fn observe(event: Event) {
        match event {
            Event::Sleep => {
                SLEEP.fetch_add(1, Ordering::SeqCst);
            }
            Event::Wake => {
                WAKE.fetch_add(1, Ordering::SeqCst);
            }
            Event::NetworkChanged | Event::Unlocked => {}
        }
    }

    let rejected = std::thread::spawn(|| install(observe).is_err())
        .join()
        .unwrap();
    anyhow::ensure!(rejected, "Off-main-thread installation was accepted");
    install(observe)?;
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    unsafe {
        center.postNotificationName_object(NSWorkspaceWillSleepNotification, None);
        center.postNotificationName_object(NSWorkspaceDidWakeNotification, None);
    }
    anyhow::ensure!(
        SLEEP.load(Ordering::SeqCst) == 1,
        "Sleep callback was not delivered exactly once"
    );
    anyhow::ensure!(
        WAKE.load(Ordering::SeqCst) == 1,
        "Wake callback was not delivered exactly once"
    );
    println!(
        "Observers installed; process-local sleep/wake callbacks delivered; worker-thread install rejected."
    );
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("This notification delivery check requires macOS")
}
