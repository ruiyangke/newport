use super::*;
use raw_window_handle::Win32WindowHandle;
use std::{
    num::NonZeroIsize,
    ptr::null_mut,
    time::{Duration, Instant},
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, GetWindowRect, IsWindow, IsWindowVisible,
    IsZoomed, MSG, PM_REMOVE, PeekMessageW, SW_MAXIMIZE, SW_MINIMIZE, WS_OVERLAPPEDWINDOW,
};

struct Fixture(HWND);
impl Fixture {
    fn new() -> Self {
        let class: Vec<_> = "STATIC\0".encode_utf16().collect();
        let title: Vec<_> = "Newport visibility fixture\0".encode_utf16().collect();
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                30,
                30,
                320,
                240,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
            )
        };
        assert!(!hwnd.is_null(), "{}", std::io::Error::last_os_error());
        Self(hwnd)
    }

    fn handle(&self) -> WindowHandle<'_> {
        let raw = Win32WindowHandle::new(NonZeroIsize::new(self.0 as isize).unwrap());
        // This fixture owns the HWND until Drop on this same test thread.
        unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(raw)) }
    }

    fn wait(&self, condition: impl Fn(HWND) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let mut message: MSG = unsafe { std::mem::zeroed() };
            while unsafe { PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) } != 0 {
                unsafe {
                    DispatchMessageW(&message);
                }
            }
            if condition(self.0) {
                return;
            }
            assert!(Instant::now() < deadline, "Window transition timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn bounds(&self) -> (i32, i32, i32, i32) {
        let mut rect = unsafe { std::mem::zeroed() };
        assert_ne!(unsafe { GetWindowRect(self.0, &mut rect) }, 0);
        (rect.left, rect.top, rect.right, rect.bottom)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.0);
        }
    }
}

#[test]
fn hide_reveal_retains_the_window_and_maximized_placement() {
    let window = Fixture::new();
    assert_eq!(unsafe { IsWindowVisible(window.0) }, 0);
    reveal(window.handle()).unwrap();
    window.wait(|hwnd| unsafe { IsWindowVisible(hwnd) != 0 });
    let bounds = window.bounds();
    for _ in 0..2 {
        hide(window.handle()).unwrap();
        window.wait(|hwnd| unsafe { IsWindowVisible(hwnd) == 0 });
        assert_ne!(unsafe { IsWindow(window.0) }, 0);
        reveal(window.handle()).unwrap();
        window.wait(|hwnd| unsafe { IsWindowVisible(hwnd) != 0 });
        assert_eq!(window.bounds(), bounds);
    }
    request(window.0, SW_MAXIMIZE).unwrap();
    window.wait(|hwnd| unsafe { IsZoomed(hwnd) != 0 });
    let maximized = window.bounds();
    hide(window.handle()).unwrap();
    window.wait(|hwnd| unsafe { IsWindowVisible(hwnd) == 0 });
    reveal(window.handle()).unwrap();
    window.wait(|hwnd| unsafe { IsWindowVisible(hwnd) != 0 });
    assert_ne!(unsafe { IsZoomed(window.0) }, 0);
    assert_eq!(window.bounds(), maximized);
}

#[test]
fn reveal_restores_minimized_windows_and_requires_owning_thread() {
    let window = Fixture::new();
    request(window.0, SW_MINIMIZE).unwrap();
    window.wait(|hwnd| unsafe { IsIconic(hwnd) != 0 });
    reveal(window.handle()).unwrap();
    window.wait(|hwnd| unsafe { IsWindowVisible(hwnd) != 0 && IsIconic(hwnd) == 0 });

    // A different thread cannot change this UI thread's live window.
    let raw = window.0 as isize;
    assert!(
        std::thread::spawn(move || {
            let handle = Win32WindowHandle::new(NonZeroIsize::new(raw).unwrap());
            // The owning fixture stays alive until this joined call completes.
            let handle = unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(handle)) };
            hide(handle)
        })
        .join()
        .unwrap()
        .is_err()
    );
    assert_ne!(unsafe { IsWindowVisible(window.0) }, 0);
}

#[test]
fn minimizing_a_maximized_window_preserves_its_restore_mode() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowPlacement, WINDOWPLACEMENT, WPF_RESTORETOMAXIMIZED,
    };
    let window = Fixture::new();
    reveal(window.handle()).unwrap();
    window.wait(|hwnd| unsafe { IsWindowVisible(hwnd) != 0 });
    let normal = window.bounds();
    request(window.0, SW_MAXIMIZE).unwrap();
    window.wait(|hwnd| unsafe { IsZoomed(hwnd) != 0 });
    let maximized = window.bounds();
    request(window.0, SW_MINIMIZE).unwrap();
    window.wait(|hwnd| unsafe { IsIconic(hwnd) != 0 });
    let mut placement: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
    placement.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
    assert_ne!(unsafe { GetWindowPlacement(window.0, &mut placement) }, 0);
    assert!(is_minimized(window.handle()).unwrap());
    assert_eq!(unsafe { IsZoomed(window.0) }, 0);
    assert_ne!(placement.flags & WPF_RESTORETOMAXIMIZED, 0);
    reveal(window.handle()).unwrap();
    window.wait(|hwnd| unsafe { IsIconic(hwnd) == 0 && IsZoomed(hwnd) != 0 });
    assert!(!is_minimized(window.handle()).unwrap());
    assert_eq!(window.bounds(), maximized);
    request(window.0, SW_RESTORE).unwrap();
    window.wait(|hwnd| unsafe { IsZoomed(hwnd) == 0 });
    assert_eq!(window.bounds(), normal);
}
