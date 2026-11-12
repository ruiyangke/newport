//! Window visibility without destroying the retained native workspace, and
//! alerts for a process that has no window.
use raw_window_handle::{RawWindowHandle, WindowHandle};
use windows_sys::Win32::{
    Foundation::HWND,
    System::Threading::GetCurrentThreadId,
    UI::WindowsAndMessaging::{
        GetWindowThreadProcessId, IsIconic, MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MessageBoxW,
        SW_HIDE, SW_RESTORE, SW_SHOW, ShowWindowAsync,
    },
};

fn owned_window(handle: WindowHandle<'_>) -> Result<HWND, String> {
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return Err("Expected a Windows window handle.".into());
    };
    let hwnd = handle.hwnd.get() as HWND;
    // Visibility is only changed from the owning UI thread, never by a
    // background backend worker.
    // SAFETY: the borrowed window handle is valid for the request; a handle that
    // died only makes the call fail.
    if unsafe { GetWindowThreadProcessId(hwnd, std::ptr::null_mut()) != GetCurrentThreadId() } {
        return Err("Window visibility must be changed on its owning thread.".into());
    }
    Ok(hwnd)
}

fn request(hwnd: HWND, command: i32) -> Result<(), String> {
    // Queue the transition: synchronous ShowWindow can reenter GPUI's native
    // visibility/focus callbacks while the application is already borrowed.
    // SAFETY: posts a message to the window; no memory is shared.
    if unsafe { ShowWindowAsync(hwnd, command) } == 0 {
        Err(format!(
            "Cannot change window visibility: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

pub fn hide(handle: WindowHandle<'_>) -> Result<(), String> {
    request(owned_window(handle)?, SW_HIDE)
}

/// Minimized windows must not replace the application's saved restore mode.
pub fn is_minimized(handle: WindowHandle<'_>) -> Result<bool, String> {
    // SAFETY: a plain query on a window handle.
    Ok(unsafe { IsIconic(owned_window(handle)?) } != 0)
}

pub fn reveal(handle: WindowHandle<'_>) -> Result<(), String> {
    let hwnd = owned_window(handle)?;
    // SW_RESTORE would unmaximize a maximized window. Restore only minimized
    // windows; ordinary hide/show must preserve the current placement.
    // SAFETY: a plain query on a window handle.
    let command = if unsafe { IsIconic(hwnd) } != 0 {
        SW_RESTORE
    } else {
        SW_SHOW
    };
    request(hwnd, command)
}

/// What a taskbar pin of the window runs, with its name and icon, under the
/// window's own AppUserModelID: for a window whose process alone wouldn't
/// open the same thing again.
pub fn set_relaunch(
    handle: WindowHandle<'_>,
    app_id: &str,
    command: &str,
    display_name: &str,
    icon: &str,
) -> Result<(), String> {
    use crate::app_shortcuts::os::string_value;
    use windows::Win32::{
        Foundation::HWND as Window,
        Storage::EnhancedStorage::{
            PKEY_AppUserModel_ID, PKEY_AppUserModel_RelaunchCommand,
            PKEY_AppUserModel_RelaunchDisplayNameResource, PKEY_AppUserModel_RelaunchIconResource,
        },
        UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow},
    };
    let hwnd = owned_window(handle)?;
    // SAFETY: the window belongs to this thread; the store is released here.
    let set = unsafe {
        SHGetPropertyStoreForWindow::<IPropertyStore>(Window(hwnd)).and_then(|store| {
            for (key, value) in [
                (PKEY_AppUserModel_ID, app_id),
                (PKEY_AppUserModel_RelaunchCommand, command),
                (PKEY_AppUserModel_RelaunchDisplayNameResource, display_name),
                (PKEY_AppUserModel_RelaunchIconResource, icon),
            ] {
                store.SetValue(&key, &string_value(value)?)?;
            }
            store.Commit()
        })
    };
    set.map_err(|error| format!("Cannot set what a taskbar pin of Newport opens: {error}"))
}

/// An error alert with no owner window, for a process that exits after it:
/// a release build has no console to print to. Blocks until dismissed.
pub fn alert(title: &str, message: &str) {
    let wide = |text: &str| text.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (title, message) = (wide(title), wide(message));
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
    }
}

#[cfg(test)]
mod tests;
