//! Window visibility without destroying the retained native workspace.
use raw_window_handle::{RawWindowHandle, WindowHandle};
use windows_sys::Win32::{
    Foundation::HWND,
    System::Threading::GetCurrentThreadId,
    UI::WindowsAndMessaging::{
        GetWindowThreadProcessId, IsIconic, SW_HIDE, SW_RESTORE, SW_SHOW, ShowWindowAsync,
    },
};

fn owned_window(handle: WindowHandle<'_>) -> Result<HWND, String> {
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return Err("Expected a Windows window handle.".into());
    };
    let hwnd = handle.hwnd.get() as HWND;
    // Borrowed handles remain valid for the request. Visibility is only changed
    // from the owning UI thread, never by a background backend worker.
    if unsafe { GetWindowThreadProcessId(hwnd, std::ptr::null_mut()) != GetCurrentThreadId() } {
        return Err("Window visibility must be changed on its owning thread.".into());
    }
    Ok(hwnd)
}

fn request(hwnd: HWND, command: i32) -> Result<(), String> {
    // Queue the transition: synchronous ShowWindow can reenter GPUI's native
    // visibility/focus callbacks while the application is already borrowed.
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
    Ok(unsafe { IsIconic(owned_window(handle)?) } != 0)
}

pub fn reveal(handle: WindowHandle<'_>) -> Result<(), String> {
    let hwnd = owned_window(handle)?;
    // SW_RESTORE would unmaximize a maximized window. Restore only minimized
    // windows; ordinary hide/show must preserve the current placement.
    let command = if unsafe { IsIconic(hwnd) } != 0 {
        SW_RESTORE
    } else {
        SW_SHOW
    };
    request(hwnd, command)
}

#[cfg(test)]
mod tests;
