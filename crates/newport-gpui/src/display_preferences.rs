//! Main-thread observer for macOS accessibility display preferences.
#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
mod platform {
    use objc::{
        class,
        declare::ClassDecl,
        msg_send,
        runtime::{BOOL, Class, Object, Sel, YES},
        sel, sel_impl,
    };
    use std::{
        marker::PhantomData,
        rc::Rc,
        sync::{
            OnceLock,
            atomic::{AtomicU64, Ordering},
        },
    };

    static REVISION: AtomicU64 = AtomicU64::new(0);
    extern "C" fn changed(_: &Object, _: Sel, _: *mut Object) {
        REVISION.fetch_add(1, Ordering::Relaxed);
    }

    pub struct DisplayPreferences {
        observer: *mut Object,
        revision: u64,
        pub opaque: bool,
        // AppKit registration, preference reads and cleanup stay on the UI thread.
        _main_thread: PhantomData<Rc<()>>,
    }

    impl DisplayPreferences {
        pub fn new(_: &gpui_kit::App) -> Self {
            static OBSERVER_CLASS: OnceLock<&'static Class> = OnceLock::new();
            let class = OBSERVER_CLASS.get_or_init(|| {
                let mut class =
                    ClassDecl::new("NewportDisplayPreferenceObserver", class!(NSObject))
                        .expect("unique observer class");
                unsafe {
                    class.add_method(
                        sel!(displayOptionsChanged:),
                        changed as extern "C" fn(&Object, Sel, *mut Object),
                    );
                }
                class.register()
            });
            unsafe {
                let observer: *mut Object = msg_send![*class, new];
                let workspace: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
                let center: *mut Object = msg_send![workspace, notificationCenter];
                let name: *mut Object = msg_send![class!(NSString), stringWithUTF8String:c"NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification".as_ptr()];
                let _: () = msg_send![center, addObserver:observer selector:sel!(displayOptionsChanged:) name:name object:std::ptr::null_mut::<Object>()];
                Self {
                    observer,
                    revision: REVISION.load(Ordering::Relaxed),
                    opaque: opaque_sidebar(),
                    _main_thread: PhantomData,
                }
            }
        }

        pub fn refresh(&mut self) -> bool {
            let revision = REVISION.load(Ordering::Relaxed);
            if revision == self.revision {
                return false;
            }
            self.revision = revision;
            let opaque = unsafe { opaque_sidebar() };
            let changed = opaque != self.opaque;
            self.opaque = opaque;
            changed
        }
    }

    unsafe fn opaque_sidebar() -> bool {
        unsafe {
            let workspace: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
            let reduce: BOOL = msg_send![workspace, accessibilityDisplayShouldReduceTransparency];
            let contrast: BOOL = msg_send![workspace, accessibilityDisplayShouldIncreaseContrast];
            reduce == YES || contrast == YES
        }
    }

    impl Drop for DisplayPreferences {
        fn drop(&mut self) {
            unsafe {
                let workspace: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
                let center: *mut Object = msg_send![workspace, notificationCenter];
                let _: () = msg_send![center, removeObserver:self.observer];
                let _: () = msg_send![self.observer, release];
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub struct DisplayPreferences {
        pub opaque: bool,
    }
    impl DisplayPreferences {
        pub fn new(_: &gpui_kit::App) -> Self {
            Self { opaque: true }
        }
        pub fn refresh(&mut self) -> bool {
            false
        }
    }
}
pub use platform::DisplayPreferences;
