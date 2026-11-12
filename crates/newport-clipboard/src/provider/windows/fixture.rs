use clipboard_win::{Clipboard, Getter, formats};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{DataExchange::*, StationsAndDesktops::*},
    UI::WindowsAndMessaging::*,
};

fn object_name(handle: HANDLE) -> String {
    let mut name = [0u16; 256];
    let mut needed = 0;
    assert_ne!(
        unsafe {
            GetUserObjectInformationW(
                handle,
                UOI_NAME,
                name.as_mut_ptr().cast(),
                std::mem::size_of_val(&name) as u32,
                &mut needed,
            )
        },
        0,
        "fixture object identity must be verifiable before clipboard access"
    );
    let end = name
        .iter()
        .position(|unit| *unit == 0)
        .expect("native object name terminator");
    String::from_utf16(&name[..end]).unwrap()
}
fn fixture_desktop(name: &str) -> bool {
    name.strip_prefix("ClipboardFixture-").is_some_and(|nonce| {
        nonce.len() == 32 && nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}
pub(super) fn isolated_station() {
    assert_eq!(
        std::env::var("NEWPORT_CLIPBOARD_TEST_STATION_OWNERSHIP").as_deref(),
        Ok("fresh-create-only"),
        "fixture requires a station freshly created by the owned harness"
    );
    let expected = std::env::var("NEWPORT_CLIPBOARD_TEST_STATION")
        .expect("native fixture requires an owned test station");
    let expected_desktop = std::env::var("NEWPORT_CLIPBOARD_TEST_DESKTOP")
        .expect("native fixture requires its unique owned desktop");
    assert!(
        fixture_desktop(&expected_desktop),
        "refusing a desktop outside the fixture namespace"
    );
    let station = unsafe { GetProcessWindowStation() };
    let actual = object_name(station);
    assert!(
        !actual.eq_ignore_ascii_case("WinSta0"),
        "interactive clipboard is forbidden"
    );
    assert_eq!(
        actual, expected,
        "station must exactly match the fresh harness capability"
    );
    let mut flags = USEROBJECTFLAGS::default();
    let mut needed = 0;
    assert_ne!(
        unsafe {
            GetUserObjectInformationW(
                station,
                UOI_FLAGS,
                (&mut flags as *mut USEROBJECTFLAGS).cast(),
                std::mem::size_of::<USEROBJECTFLAGS>() as u32,
                &mut needed,
            )
        },
        0,
        "fixture station visibility must be verifiable"
    );
    assert_eq!(
        flags.dwFlags & WSF_VISIBLE as u32,
        0,
        "visible clipboard stations are forbidden"
    );
    let desktop =
        unsafe { GetThreadDesktop(windows_sys::Win32::System::Threading::GetCurrentThreadId()) };
    assert_eq!(
        object_name(desktop),
        expected_desktop,
        "desktop must exactly match the owned harness capability"
    );
}
#[test]
fn station_fixture_requires_a_unique_owned_desktop_name() {
    assert!(fixture_desktop(
        "ClipboardFixture-0123456789abcdef0123456789abcdef"
    ));
    assert!(!fixture_desktop("Default"));
    assert!(!fixture_desktop("ClipboardFixture"));
    assert!(!fixture_desktop(
        "ClipboardFixture-0123456789abcdef0123456789abcdeg"
    ));
}
pub(super) fn wait(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "native fixture condition timed out"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
type ResultBytes = Result<Vec<u8>, String>;
enum Command {
    Replace(Vec<(u32, Vec<u8>)>, mpsc::SyncSender<()>),
    Read(u32, mpsc::SyncSender<ResultBytes>),
    Hold(Arc<AtomicBool>, mpsc::SyncSender<()>),
    Delay(Arc<AtomicBool>, Arc<AtomicBool>, mpsc::SyncSender<()>),
}
struct State {
    delayed: Mutex<Option<(Arc<AtomicBool>, Arc<AtomicBool>)>>,
}
unsafe extern "system" fn procedure(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if message == WM_NCCREATE {
        let create = unsafe { &*(l as *const CREATESTRUCTW) };
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
    }
    if message == WM_RENDERFORMAT {
        let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const State;
        if !pointer.is_null() {
            let delayed = unsafe { &*pointer }.delayed.lock().unwrap().clone();
            if let Some((release, entered)) = delayed {
                entered.store(true, Ordering::Release);
                let deadline = Instant::now() + Duration::from_secs(5);
                while !release.load(Ordering::Acquire) && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
                let bytes = text("delayed fixture");
                let _ = clipboard_win::raw::set_without_clear(w as u32, &bytes);
            }
        }
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, message, w, l) }
}
pub(super) fn text(value: &str) -> Vec<u8> {
    value
        .encode_utf16()
        .chain(Some(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}
pub(super) struct Native {
    sender: mpsc::Sender<Command>,
    running: Arc<AtomicBool>,
    releases: Mutex<Vec<Arc<AtomicBool>>>,
    stopped: mpsc::Receiver<()>,
}
impl Native {
    pub(super) fn new() -> Self {
        let (sender, receive) = mpsc::channel();
        let (ready, started) = mpsc::sync_channel(1);
        let (done, stopped) = mpsc::sync_channel(1);
        let running = Arc::new(AtomicBool::new(true));
        let active = running.clone();
        std::thread::spawn(move || unsafe {
            let state = Box::new(State {
                delayed: Mutex::new(None),
            });
            let name: Vec<_> = format!("NewportClipboardFixture-{}\0", std::process::id())
                .encode_utf16()
                .collect();
            let class = WNDCLASSW {
                lpfnWndProc: Some(procedure),
                lpszClassName: name.as_ptr(),
                ..Default::default()
            };
            assert_ne!(RegisterClassW(&class), 0);
            let hwnd = CreateWindowExW(
                0,
                name.as_ptr(),
                name.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                (&*state as *const State).cast(),
            );
            assert!(!hwnd.is_null());
            ready.send(()).unwrap();
            while active.load(Ordering::Acquire) {
                let mut message = MSG::default();
                while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                let Ok(command) = receive.recv_timeout(Duration::from_millis(5)) else {
                    continue;
                };
                let deadline = Instant::now() + Duration::from_secs(3);
                let clipboard = loop {
                    if let Ok(clipboard) = Clipboard::new_for(hwnd as _) {
                        break clipboard;
                    }
                    assert!(
                        Instant::now() < deadline,
                        "fixture could not open clipboard"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                };
                match command {
                    Command::Replace(values, reply) => {
                        clipboard_win::raw::empty().unwrap();
                        *state.delayed.lock().unwrap() = None;
                        for (native, bytes) in values {
                            clipboard_win::raw::set_without_clear(native, &bytes).unwrap();
                        }
                        drop(clipboard);
                        let _ = reply.send(());
                    }
                    Command::Read(native, reply) => {
                        let mut bytes = Vec::new();
                        let result = formats::RawData(native)
                            .read_clipboard(&mut bytes)
                            .map(|_| bytes)
                            .map_err(|error| error.to_string());
                        drop(clipboard);
                        let _ = reply.send(result);
                    }
                    Command::Hold(release, reply) => {
                        let _ = reply.send(());
                        let deadline = Instant::now() + Duration::from_secs(5);
                        while !release.load(Ordering::Acquire) && Instant::now() < deadline {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                    }
                    Command::Delay(release, entered, reply) => {
                        clipboard_win::raw::empty().unwrap();
                        *state.delayed.lock().unwrap() = Some((release, entered));
                        SetClipboardData(formats::CF_UNICODETEXT, std::ptr::null_mut());
                        drop(clipboard);
                        let _ = reply.send(());
                    }
                }
            }
            DestroyWindow(hwnd);
            UnregisterClassW(name.as_ptr(), std::ptr::null_mut());
            let _ = done.send(());
        });
        started.recv_timeout(Duration::from_secs(3)).unwrap();
        Self {
            sender,
            running,
            releases: Mutex::new(Vec::new()),
            stopped,
        }
    }
    pub(super) fn replace(&self, values: Vec<(u32, Vec<u8>)>) {
        let (reply, receive) = mpsc::sync_channel(1);
        self.sender.send(Command::Replace(values, reply)).unwrap();
        receive.recv_timeout(Duration::from_secs(5)).unwrap();
    }
    pub(super) fn read(&self, native: u32) -> ResultBytes {
        let (reply, receive) = mpsc::sync_channel(1);
        self.sender.send(Command::Read(native, reply)).unwrap();
        receive.recv_timeout(Duration::from_secs(5)).unwrap()
    }
    pub(super) fn hold(&self) -> Arc<AtomicBool> {
        let release = Arc::new(AtomicBool::new(false));
        self.releases.lock().unwrap().push(release.clone());
        let (reply, receive) = mpsc::sync_channel(1);
        self.sender
            .send(Command::Hold(release.clone(), reply))
            .unwrap();
        receive.recv_timeout(Duration::from_secs(3)).unwrap();
        release
    }
    pub(super) fn delayed(&self) -> (Arc<AtomicBool>, Arc<AtomicBool>) {
        let release = Arc::new(AtomicBool::new(false));
        let entered = Arc::new(AtomicBool::new(false));
        self.releases.lock().unwrap().push(release.clone());
        let (reply, receive) = mpsc::sync_channel(1);
        self.sender
            .send(Command::Delay(release.clone(), entered.clone(), reply))
            .unwrap();
        receive.recv_timeout(Duration::from_secs(3)).unwrap();
        (release, entered)
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        for release in self.releases.lock().unwrap().iter() {
            release.store(true, Ordering::Release);
        }
        self.running.store(false, Ordering::Release);
        let _ = self.stopped.recv_timeout(Duration::from_secs(3));
    }
}
pub(super) fn png() -> Vec<u8> {
    use image::ImageEncoder;
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(
            &[255, 0, 20, 128, 0, 255, 30, 255],
            1,
            2,
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
    bytes
}
pub(super) fn assert_pixels(png: &[u8]) {
    assert_eq!(
        image::load_from_memory_with_format(png, image::ImageFormat::Png)
            .unwrap()
            .into_rgba8()
            .as_raw(),
        &[255, 0, 20, 128, 0, 255, 30, 255]
    );
}

pub(super) fn assert_text(bytes: &[u8], expected: &str) {
    assert_eq!(bytes.len() % 2, 0);
    let units: Vec<_> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes(*b))
        .collect();
    let end = units
        .iter()
        .position(|unit| *unit == 0)
        .expect("native text terminator");
    assert_eq!(String::from_utf16(&units[..end]).unwrap(), expected);
}

pub(super) fn copy_on_current_thread(value: &str) {
    unsafe {
        let name: Vec<u16> = format!("NewportClipboardCopy-{:?}\0", std::thread::current().id())
            .encode_utf16()
            .collect();
        let class = WNDCLASSW {
            lpfnWndProc: Some(DefWindowProcW),
            lpszClassName: name.as_ptr(),
            ..Default::default()
        };
        assert_ne!(RegisterClassW(&class), 0);
        let hwnd = CreateWindowExW(
            0,
            name.as_ptr(),
            name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        );
        assert!(!hwnd.is_null());
        {
            let _clipboard = Clipboard::new_for(hwnd as _).unwrap();
            clipboard_win::raw::empty().unwrap();
            clipboard_win::raw::set_without_clear(formats::CF_UNICODETEXT, &text(value)).unwrap();
        }
        DestroyWindow(hwnd);
        UnregisterClassW(name.as_ptr(), std::ptr::null_mut());
    }
}

pub(super) fn probe_mode() -> String {
    let mode = std::env::var("NEWPORT_CLIPBOARD_PROBE_MODE")
        .expect("cross-process probe mode is required");
    assert!(
        matches!(mode.as_str(), "render" | "privacy" | "destroy"),
        "unknown cross-process probe mode"
    );
    mode
}
fn checkpoint(value: &str) {
    use std::io::Write;
    let mut output = std::io::stdout().lock();
    writeln!(output, "{value}").unwrap();
    output.flush().unwrap();
}
pub(super) struct Probe(mpsc::Receiver<String>);
impl Probe {
    pub(super) fn new() -> Self {
        isolated_station();
        probe_mode();
        let (sender, receive) = mpsc::sync_channel(3);
        std::thread::spawn(move || {
            use std::io::Read;
            let mut input = std::io::stdin().lock();
            for _ in 0..3 {
                let mut line = Vec::new();
                loop {
                    let mut byte = [0];
                    if input.read_exact(&mut byte).is_err() {
                        return;
                    }
                    if byte[0] == b'\n' {
                        break;
                    }
                    if line.len() == 32 {
                        return;
                    }
                    if byte[0] != b'\r' {
                        line.push(byte[0]);
                    }
                }
                let Ok(line) = String::from_utf8(line) else {
                    return;
                };
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        let probe = Self(receive);
        checkpoint("CLIPBOARD_PARENT_READY");
        probe.receive("ready");
        probe
    }
    fn receive(&self, expected: &str) {
        assert_eq!(
            self.0
                .recv_timeout(Duration::from_secs(10))
                .expect("station harness checkpoint timed out"),
            expected
        );
    }
    pub(super) fn blocked(&self) {
        checkpoint("CLIPBOARD_PARENT_STARTED");
        self.receive("blocked");
    }
    pub(super) fn terminated(&self) {
        checkpoint("CLIPBOARD_PARENT_STOPPED");
        self.receive("terminated");
    }
    pub(super) fn recovered(&self) {
        checkpoint("CLIPBOARD_READER_RECOVERED");
    }
}
struct StalledOwner {
    mode: String,
}
unsafe extern "system" fn stalled_procedure(
    hwnd: HWND,
    message: u32,
    w: WPARAM,
    l: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = unsafe { &*(l as *const CREATESTRUCTW) };
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
    }
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const StalledOwner;
    if !pointer.is_null() {
        let state = unsafe { &*pointer };
        if (message == WM_RENDERFORMAT && state.mode != "destroy")
            || (message == WM_DESTROYCLIPBOARD && state.mode == "destroy")
        {
            checkpoint("CLIPBOARD_CHILD_ENTERED");
            // Only the outer harness can end this owned process. No timer can make the probe pass.
            loop {
                std::thread::park();
            }
        }
    }
    unsafe { DefWindowProcW(hwnd, message, w, l) }
}
pub(super) fn stalled_process() {
    isolated_station();
    let state = Box::new(StalledOwner { mode: probe_mode() });
    unsafe {
        let name: Vec<u16> = format!("NewportClipboardStalledOwner-{}\0", std::process::id())
            .encode_utf16()
            .collect();
        let class = WNDCLASSW {
            lpfnWndProc: Some(stalled_procedure),
            lpszClassName: name.as_ptr(),
            ..Default::default()
        };
        assert_ne!(RegisterClassW(&class), 0);
        let hwnd = CreateWindowExW(
            0,
            name.as_ptr(),
            name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            (&*state as *const StalledOwner).cast(),
        );
        assert!(!hwnd.is_null());
        {
            let _clipboard = Clipboard::new_for(hwnd as _).unwrap();
            clipboard_win::raw::empty().unwrap();
            if state.mode == "render" {
                SetClipboardData(formats::CF_UNICODETEXT, std::ptr::null_mut());
                assert!(clipboard_win::is_format_avail(formats::CF_UNICODETEXT));
            } else {
                clipboard_win::raw::set_without_clear(
                    formats::CF_UNICODETEXT,
                    &text("owned cross-process fixture"),
                )
                .unwrap();
                if state.mode == "privacy" {
                    let policy = clipboard_win::register_format("CanIncludeInClipboardHistory")
                        .unwrap()
                        .get();
                    SetClipboardData(policy, std::ptr::null_mut());
                    assert!(clipboard_win::is_format_avail(policy));
                }
            }
        }
        checkpoint("CLIPBOARD_CHILD_READY");
        loop {
            let mut message = MSG::default();
            assert!(GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0);
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}
