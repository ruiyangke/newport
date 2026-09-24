use crate::platform::events::Event;
use std::{ptr, sync::OnceLock};
use windows_sys::Win32::{
    Foundation::HANDLE, NetworkManagement::IpHelper::*, Networking::WinSock::AF_UNSPEC,
    System::Power::*, UI::WindowsAndMessaging::*,
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
        // thread-safe notification sink and never touch Tauri UI objects.
    }
    Ok(())
}
