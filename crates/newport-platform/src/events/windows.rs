use super::Event;
use std::{ptr, sync::OnceLock};
use windows_sys::Win32::{
    Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM},
    NetworkManagement::IpHelper::*,
    Networking::WinSock::AF_UNSPEC,
    System::{LibraryLoader::GetModuleHandleW, Power::*, RemoteDesktop::*},
    UI::WindowsAndMessaging::*,
};
static NOTIFY: OnceLock<fn(Event)> = OnceLock::new();

unsafe extern "system" fn network(
    _: *const std::ffi::c_void,
    _: *const MIB_IPINTERFACE_ROW,
    _: MIB_NOTIFICATION_TYPE,
) {
    if let Some(notify) = NOTIFY.get() {
        notify(Event::NetworkChanged);
    }
}
unsafe extern "system" fn power(
    _: *const std::ffi::c_void,
    kind: u32,
    _: *const std::ffi::c_void,
) -> u32 {
    if let Some(notify) = NOTIFY.get() {
        match kind {
            PBT_APMSUSPEND => notify(Event::Sleep),
            PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND | PBT_APMRESUMECRITICAL => {
                notify(Event::Wake)
            }
            _ => {}
        }
    }
    0
}
unsafe extern "system" fn session(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_WTSSESSION_CHANGE
        && matches!(
            wparam as u32,
            WTS_SESSION_UNLOCK | WTS_CONSOLE_CONNECT | WTS_REMOTE_CONNECT
        )
        && let Some(notify) = NOTIFY.get()
    {
        notify(Event::Unlocked);
    }
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

/// Session changes arrive as window messages, so a message-only window on
/// this thread receives them through the application's message loop.
unsafe fn watch_session() -> anyhow::Result<()> {
    let name: Vec<u16> = "NewportSessionEvents\0".encode_utf16().collect();
    unsafe {
        let instance = GetModuleHandleW(ptr::null());
        let class = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(session),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: ptr::null_mut(),
            hCursor: ptr::null_mut(),
            hbrBackground: ptr::null_mut(),
            lpszMenuName: ptr::null(),
            lpszClassName: name.as_ptr(),
        };
        anyhow::ensure!(
            RegisterClassW(&class) != 0,
            "Cannot register the session window"
        );
        let window = CreateWindowExW(
            0,
            name.as_ptr(),
            name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            ptr::null_mut(),
            instance,
            ptr::null(),
        );
        anyhow::ensure!(!window.is_null(), "Cannot create the session window");
        anyhow::ensure!(
            WTSRegisterSessionNotification(window, NOTIFY_FOR_THIS_SESSION) != 0,
            "Cannot subscribe to session changes"
        );
    }
    Ok(())
}

pub fn install(notify: fn(Event)) -> anyhow::Result<()> {
    anyhow::ensure!(
        NOTIFY.set(notify).is_ok(),
        "System event observer is already installed"
    );
    unsafe {
        let mut network_handle: HANDLE = ptr::null_mut();
        let code = NotifyIpInterfaceChange(
            AF_UNSPEC,
            Some(network),
            ptr::null(),
            false,
            &mut network_handle,
        );
        anyhow::ensure!(code == 0, "Cannot subscribe to network changes: {code}");
        let params = DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
            Callback: Some(power),
            Context: ptr::null_mut(),
        };
        let mut power_handle = ptr::null_mut();
        let code = PowerRegisterSuspendResumeNotification(
            DEVICE_NOTIFY_CALLBACK,
            (&params as *const DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS)
                .cast_mut()
                .cast(),
            &mut power_handle,
        );
        if code != 0 {
            CancelMibChangeNotify2(network_handle);
            anyhow::bail!("Cannot subscribe to power changes: {code}");
        }
        // Process-lifetime observers, as on macOS. Callbacks use only the static
        // thread-safe notification sink and never touch UI objects.
        // Unlocks only shorten recovery; network and power events still work.
        if let Err(error) = watch_session() {
            log::warn!("Session changes unavailable: {error:#}");
        }
    }
    Ok(())
}
