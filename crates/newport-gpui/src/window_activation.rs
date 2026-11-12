//! Native window activation: restore minimized windows, and bring a window
//! forward on the current Space.
#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
pub(super) fn restore(window: &gpui_kit::Window) -> Result<(), String> {
    use objc::{
        msg_send,
        runtime::{BOOL, Object, YES},
        sel, sel_impl,
    };
    if let Some(native) = native(window) {
        // The live Window owns its NSWindow; called on GPUI's main thread.
        unsafe {
            let minimized: BOOL = msg_send![native, isMiniaturized];
            if minimized == YES {
                let _: () = msg_send![native, deminiaturize: std::ptr::null_mut::<Object>()];
            }
        }
    }
    Ok(())
}

/// The NSWindow behind `window`, if it is a native AppKit window.
#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
fn native(window: &gpui_kit::Window) -> Option<*mut objc::runtime::Object> {
    use objc::{msg_send, runtime::Object, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };
    // The live Window owns the view; called synchronously on GPUI's main thread.
    let native: *mut Object = unsafe {
        let view = handle.ns_view.as_ptr() as *mut Object;
        msg_send![view, window]
    };
    (!native.is_null()).then_some(native)
}

/// Brings `window` in front of the app's other windows and makes it key, on
/// the Space the person is on rather than switching to the Space it was left
/// on. A minimized window comes back from the Dock. Doesn't activate the app.
#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
pub(super) fn bring_forward(window: &gpui_kit::Window) {
    use objc::{
        msg_send,
        runtime::{BOOL, Object, YES},
        sel, sel_impl,
    };
    const CAN_JOIN_ALL_SPACES: usize = 1 << 0;
    const MOVE_TO_ACTIVE_SPACE: usize = 1 << 1;
    const FULL_SCREEN: usize = 1 << 14;
    let _ = restore(window);
    if let Some(native) = native(window) {
        // The live Window owns its NSWindow; called on GPUI's main thread.
        unsafe {
            let minimized: BOOL = msg_send![native, isMiniaturized];
            let here: BOOL = msg_send![native, isOnActiveSpace];
            let style: usize = msg_send![native, styleMask];
            let behavior: usize = msg_send![native, collectionBehavior];
            // A full-screen window keeps its own Space.
            if minimized != YES
                && here != YES
                && style & FULL_SCREEN == 0
                && behavior & (CAN_JOIN_ALL_SPACES | MOVE_TO_ACTIVE_SPACE) == 0
            {
                let _: () =
                    msg_send![native, setCollectionBehavior: behavior | MOVE_TO_ACTIVE_SPACE];
                let _: () = msg_send![native, makeKeyAndOrderFront: std::ptr::null_mut::<Object>()];
                let _: () = msg_send![native, setCollectionBehavior: behavior];
            }
        }
    }
    window.activate_window();
}

#[cfg(not(target_os = "macos"))]
pub(super) fn bring_forward(window: &gpui_kit::Window) {
    let _ = restore(window);
    window.activate_window();
}

/// Whether Newport is the active app. Some window of an active app is
/// usually key; on macOS the app can also be active with none.
pub(super) fn app_is_active(cx: &mut gpui_kit::App) -> bool {
    #[cfg(all(target_os = "macos", not(test)))]
    #[allow(unexpected_cfgs)]
    {
        use objc::{
            class, msg_send,
            runtime::{BOOL, Object, YES},
            sel, sel_impl,
        };
        // Called on GPUI's main thread, where NSApp already exists.
        let active: BOOL = unsafe {
            let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
            msg_send![app, isActive]
        };
        if active == YES {
            return true;
        }
    }
    cx.windows().into_iter().any(|handle| {
        handle
            .update(cx, |_, window, _| window.is_window_active())
            .unwrap_or(false)
    })
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
