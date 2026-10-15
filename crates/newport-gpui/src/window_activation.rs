//! Restore native minimization before asking GPUI to activate a workspace.
#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
pub(super) fn restore(window: &gpui_kit::Window) -> Result<(), String> {
    use objc::{
        msg_send,
        runtime::{BOOL, Object, YES},
        sel, sel_impl,
    };
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return Ok(());
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return Ok(());
    };
    // The live Window owns the view; called synchronously on GPUI's main thread.
    unsafe {
        let view = handle.ns_view.as_ptr() as *mut Object;
        let native: *mut Object = msg_send![view, window];
        if !native.is_null() {
            let minimized: BOOL = msg_send![native, isMiniaturized];
            if minimized == YES {
                let _: () = msg_send![native, deminiaturize: std::ptr::null_mut::<Object>()];
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
pub(super) fn restore(window: &gpui_kit::Window) -> Result<(), String> {
    use raw_window_handle::HasWindowHandle;
    HasWindowHandle::window_handle(window)
        .map_err(|error| error.to_string())
        .and_then(newport_platform::window::reveal)
}

#[cfg(not(any(target_os = "macos", windows)))]
pub(super) fn restore(_: &gpui_kit::Window) -> Result<(), String> {
    Ok(())
}

pub(super) fn hide(window: &gpui_kit::Window, cx: &mut gpui_kit::App) {
    #[cfg(windows)]
    {
        use raw_window_handle::HasWindowHandle;
        let _ = cx;
        let result = HasWindowHandle::window_handle(window)
            .map_err(|error| error.to_string())
            .and_then(newport_platform::window::hide);
        if let Err(error) = result {
            eprintln!("Cannot hide Newport: {error}");
        }
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        cx.hide();
    }
}
