//! A standard AppKit sidebar material behind GPUI's transparent surface.
#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs, deprecated)] // Legacy objc/cocoa match GPUI's macOS backend ABI.
pub fn sync(window: &gpui_kit::Window, dark: bool) {
    use objc::{
        class, msg_send,
        runtime::{BOOL, Object, YES},
        sel, sel_impl,
    };
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // Called only from GPUI's main-thread window lifecycle. The raw view and its
    // subviews remain owned by the live Window, and no pointers escape this call.
    unsafe {
        let view = handle.ns_view.as_ptr() as *mut Object;
        let native_window: *mut Object = msg_send![view, window];
        if native_window.is_null() {
            return;
        }
        let content: *mut Object = msg_send![native_window, contentView];
        let children: *mut Object = msg_send![content, subviews];
        let count: usize = msg_send![children, count];
        let identifier: *mut Object =
            msg_send![class!(NSString), stringWithUTF8String:c"newport.sidebar.material".as_ptr()];
        let mut sidebar: *mut Object = std::ptr::null_mut();
        for index in 0..count {
            let child: *mut Object = msg_send![children, objectAtIndex:index];
            let effect: BOOL = msg_send![child, isKindOfClass:class!(NSVisualEffectView)];
            if effect != YES {
                continue;
            }
            if (*child).class().name() == "BlurredView" {
                // GPUI's subclass strips material layers in updateLayer. A plain
                // NSVisualEffectView preserves AppKit's tint and contrast behavior.
                let _: () = msg_send![child, setHidden:YES];
            } else {
                let existing: *mut Object = msg_send![child, identifier];
                let matches: BOOL = msg_send![existing, isEqualToString:identifier];
                if matches == YES {
                    sidebar = child;
                }
            }
        }
        if sidebar.is_null() {
            let frame: cocoa::foundation::NSRect = msg_send![content, bounds];
            let allocated: *mut Object = msg_send![class!(NSVisualEffectView), alloc];
            sidebar = msg_send![allocated, initWithFrame:frame];
            if sidebar.is_null() {
                return;
            }
            let _: () = msg_send![sidebar, setIdentifier:identifier];
            let _: () = msg_send![sidebar, setAutoresizingMask:18_usize]; // width + height
            let _: () = msg_send![content, addSubview:sidebar positioned:-1_isize relativeTo:std::ptr::null_mut::<Object>()];
            let _: *mut Object = msg_send![sidebar, autorelease]; // parent retains it
        }
        {
            let child = sidebar;
            let name = if dark {
                c"NSAppearanceNameDarkAqua"
            } else {
                c"NSAppearanceNameAqua"
            };
            let name: *mut Object = msg_send![class!(NSString), stringWithUTF8String:name.as_ptr()];
            let appearance: *mut Object = msg_send![class!(NSAppearance), appearanceNamed:name];
            let _: () = msg_send![child, setAppearance:appearance];
            let _: () = msg_send![child, setMaterial:7_isize]; // NSVisualEffectMaterialSidebar
            let _: () = msg_send![child, setBlendingMode:0_isize]; // BehindWindow
            let _: () = msg_send![child, setState:0_isize]; // FollowsWindowActiveState
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn sync(_: &gpui_kit::Window, _: bool) {}
