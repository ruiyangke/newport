//! Product identity, read from the application manifest `resources/app.json`.
pub fn version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../../../resources/app.json"))
                .expect("valid Newport application manifest");
        manifest["version"]
            .as_str()
            .expect("Newport version")
            .to_owned()
    })
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
pub fn show() {
    use objc::{class, msg_send, runtime::Object, sel, sel_impl};
    // Menu dispatch runs on the main thread. AppKit owns the standard panel;
    // the autoreleased dictionary and strings do not escape this call.
    unsafe {
        let options: *mut Object = msg_send![class!(NSMutableDictionary), dictionary];
        // Supply the icon explicitly as well: Launch Services may retain a
        // placeholder for an unsigned development bundle rebuilt in place.
        let icon = include_bytes!("../../../resources/icons/icon.png");
        let data: *mut Object =
            msg_send![class!(NSData), dataWithBytes:icon.as_ptr() length:icon.len()];
        let allocated: *mut Object = msg_send![class!(NSImage), alloc];
        let image: *mut Object = msg_send![allocated, initWithData:data];
        if !image.is_null() {
            let key: *mut Object =
                msg_send![class!(NSString), stringWithUTF8String:c"ApplicationIcon".as_ptr()];
            let _: () = msg_send![options, setObject:image forKey:key];
            let _: () = msg_send![image, release];
        }
        for (key, value) in [
            ("ApplicationName", "Newport"),
            ("ApplicationVersion", version()),
            ("Version", ""),
        ] {
            let string = |text: &str| -> *mut Object {
                let text = std::ffi::CString::new(text).expect("manifest contains no NUL");
                msg_send![class!(NSString), stringWithUTF8String:text.as_ptr()]
            };
            let _: () = msg_send![options, setObject:string(value) forKey:string(key)];
        }
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, orderFrontStandardAboutPanelWithOptions:options];
    }
}

#[cfg(not(target_os = "macos"))]
pub fn show(window: &mut gpui_kit::Window, cx: &mut gpui_kit::App) {
    use gpui_kit::component::WindowExt;
    window.open_alert_dialog(cx, |dialog, _, _| {
        dialog
            .title("About Newport")
            .description(format!("Newport {}", version()))
            .show_cancel(false)
            .ok_text("Done")
    });
}
