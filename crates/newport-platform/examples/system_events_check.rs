//! Exercise process-local system event delivery without sleeping, locking or
//! switching the OS session.
#[cfg(any(target_os = "macos", windows))]
use newport_platform::events::Event;
#[cfg(any(target_os = "macos", windows))]
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(any(target_os = "macos", windows))]
static SLEEP: AtomicUsize = AtomicUsize::new(0);
#[cfg(any(target_os = "macos", windows))]
static WAKE: AtomicUsize = AtomicUsize::new(0);
#[cfg(any(target_os = "macos", windows))]
static UNLOCKED: AtomicUsize = AtomicUsize::new(0);

#[cfg(any(target_os = "macos", windows))]
fn observe(event: Event) {
    let count = match event {
        Event::Sleep => &SLEEP,
        Event::Wake => &WAKE,
        Event::Unlocked => &UNLOCKED,
        Event::NetworkChanged => return,
    };
    count.fetch_add(1, Ordering::SeqCst);
}

#[cfg(any(target_os = "macos", windows))]
fn delivered_once(count: &AtomicUsize, what: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        count.load(Ordering::SeqCst) == 1,
        "{what} callback was not delivered exactly once"
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    use newport_platform::events::install;
    use objc2_app_kit::{
        NSWorkspace, NSWorkspaceDidWakeNotification, NSWorkspaceSessionDidBecomeActiveNotification,
        NSWorkspaceWillSleepNotification,
    };

    let rejected = std::thread::spawn(|| install(observe).is_err())
        .join()
        .unwrap();
    anyhow::ensure!(rejected, "Off-main-thread installation was accepted");
    install(observe)?;
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    unsafe {
        center.postNotificationName_object(NSWorkspaceWillSleepNotification, None);
        center.postNotificationName_object(NSWorkspaceDidWakeNotification, None);
        center.postNotificationName_object(NSWorkspaceSessionDidBecomeActiveNotification, None);
    }
    delivered_once(&SLEEP, "Sleep")?;
    delivered_once(&WAKE, "Wake")?;
    delivered_once(&UNLOCKED, "Unlock")?;
    println!(
        "Observers installed; process-local sleep/wake/unlock callbacks delivered; worker-thread install rejected."
    );
    Ok(())
}

/// Sends the session window the messages Windows sends it, and checks that
/// Windows took its registration for session changes.
#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use newport_platform::events::install;
    use std::ptr;
    use windows_sys::Win32::{
        System::RemoteDesktop::WTSUnRegisterSessionNotification,
        UI::WindowsAndMessaging::{
            FindWindowExW, HWND_MESSAGE, SendMessageW, WM_WTSSESSION_CHANGE, WTS_SESSION_LOCK,
            WTS_SESSION_UNLOCK,
        },
    };

    install(observe)?;
    let name: Vec<u16> = "NewportSessionEvents\0".encode_utf16().collect();
    let window =
        unsafe { FindWindowExW(HWND_MESSAGE, ptr::null_mut(), name.as_ptr(), ptr::null()) };
    anyhow::ensure!(!window.is_null(), "The session window wasn't created");
    unsafe {
        SendMessageW(window, WM_WTSSESSION_CHANGE, WTS_SESSION_LOCK as usize, 0);
        SendMessageW(window, WM_WTSSESSION_CHANGE, WTS_SESSION_UNLOCK as usize, 0);
    }
    delivered_once(&UNLOCKED, "Unlock")?;
    anyhow::ensure!(
        unsafe { WTSUnRegisterSessionNotification(window) } != 0,
        "The session window wasn't registered for session changes"
    );
    println!("Session window installed and registered; unlock delivered once, lock ignored.");
    Ok(())
}

#[cfg(not(any(target_os = "macos", windows)))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("This notification delivery check requires macOS or Windows")
}
