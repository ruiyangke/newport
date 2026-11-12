use super::html::html_fragment;
use clipboard_win::{
    Clipboard, Getter,
    formats::{self, Format},
};

const MAX_CAPTURE_BYTES: usize = super::provider::MAX_BYTES;

pub(super) fn current_revision() -> Option<isize> {
    clipboard_win::seq_num().map(|count| count.get() as isize)
}
static REVISION: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static OBSERVER_ALIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static OBSERVER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
pub fn revision() -> Option<isize> {
    if *OBSERVER.get_or_init(install_observer)
        && OBSERVER_ALIVE.load(std::sync::atomic::Ordering::Acquire)
    {
        Some(REVISION.load(std::sync::atomic::Ordering::Acquire))
    } else {
        current_revision()
    }
}
fn install_observer() -> bool {
    use windows_sys::Win32::{Foundation::*, System::DataExchange::*, UI::WindowsAndMessaging::*};
    unsafe extern "system" fn procedure(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        if message == WM_CLIPBOARDUPDATE {
            if let Some(count) = current_revision() {
                REVISION.store(count, std::sync::atomic::Ordering::Release);
            }
            return 0;
        }
        // SAFETY: Forwards the window procedure's own arguments unchanged.
        unsafe { DefWindowProcW(hwnd, message, w, l) }
    }
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    // SAFETY (the block below): `name` is NUL-terminated and outlives the class and
    // window; the window is created, pumped and destroyed on this one thread.
    std::thread::spawn(move || unsafe {
        let name: Vec<u16> = "NewportClipboardObserver\0".encode_utf16().collect();
        let class = WNDCLASSW {
            lpfnWndProc: Some(procedure),
            lpszClassName: name.as_ptr(),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            let _ = tx.send(false);
            return;
        }
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
        if hwnd.is_null() || AddClipboardFormatListener(hwnd) == 0 {
            if !hwnd.is_null() {
                DestroyWindow(hwnd);
            }
            let _ = tx.send(false);
            return;
        }
        if let Some(count) = current_revision() {
            REVISION.store(count, std::sync::atomic::Ordering::Release);
        }
        OBSERVER_ALIVE.store(true, std::sync::atomic::Ordering::Release);
        let _ = tx.send(true);
        let mut message = MSG::default();
        while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        OBSERVER_ALIVE.store(false, std::sync::atomic::Ordering::Release);
        RemoveClipboardFormatListener(hwnd);
        DestroyWindow(hwnd);
    });
    let installed = rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap_or(false);
    if !installed {
        log::warn!("Clipboard notifications unavailable; using revision polling");
    }
    installed
}

fn bounded_memory_format(format: u32) -> bool {
    let limit = if matches!(format, formats::CF_DIBV5 | formats::CF_DIB) {
        super::windows_limits::MAX_IMAGE_BYTES
    } else {
        MAX_CAPTURE_BYTES
    };
    clipboard_win::size(format).is_some_and(|size| size.get() <= limit)
}

/// Capture a consistent revision. A busy clipboard is retried on the next tick.
/// Collection and publication limits remain in the shared synchronization layer.
fn capture_selected(
    selected: Option<&str>,
    previous: Option<isize>,
    mut add: impl FnMut(&str, Option<Vec<u8>>),
) -> Option<isize> {
    let count = clipboard_win::seq_num()?.get() as isize;
    if previous == Some(count) {
        return None;
    }
    let _clipboard = Clipboard::new().ok()?;
    if selected.is_none_or(|s| s == "text/plain")
        && formats::Unicode.is_format_avail()
        && bounded_memory_format(formats::CF_UNICODETEXT)
    {
        let mut bytes = Vec::new();
        formats::RawData(formats::CF_UNICODETEXT)
            .read_clipboard(&mut bytes)
            .ok()?;
        add("text/plain", super::windows_formats::decode_unicode(&bytes));
    }
    let png = clipboard_win::register_format("PNG")
        .filter(|format| clipboard_win::is_format_avail(format.get()));
    if let Some(format) = png
        .filter(|_| selected.is_none_or(|s| s == "image/png"))
        .filter(|format| bounded_memory_format(format.get()))
    {
        let mut bytes = Vec::new();
        formats::RawData(format.get())
            .read_clipboard(&mut bytes)
            .ok()?;
        add("image/png", Some(bytes));
    }
    if selected.is_none_or(|s| s == "image/png") && png.is_none() {
        for native in [formats::CF_DIBV5, formats::CF_DIB] {
            if clipboard_win::is_format_avail(native) && bounded_memory_format(native) {
                let mut dib = Vec::new();
                if formats::RawData(native).read_clipboard(&mut dib).is_ok()
                    && let Some(png) = super::windows_bitmap::dib_to_png(
                        &dib,
                        super::windows_limits::MAX_IMAGE_BYTES,
                        MAX_CAPTURE_BYTES,
                    )
                {
                    add("image/png", Some(png));
                    break;
                }
            }
        }
    }
    if let Some(format) = formats::Html::new().filter(|_| selected.is_none_or(|s| s == "text/html"))
    {
        let mut html = Vec::new();
        if format.is_format_avail() && bounded_memory_format((&format).into()) {
            // Parse offsets with bounds checks; do not let malformed optional
            // HTML discard valid text or enter the dependency's unsafe parser.
            if formats::RawData(format.into())
                .read_clipboard(&mut html)
                .is_ok()
            {
                add("text/html", html_fragment(&html).map(<[u8]>::to_vec));
            }
        }
    }
    for (mime, native) in rich_formats() {
        if selected.is_none_or(|selected| selected == mime)
            && clipboard_win::is_format_avail(native)
            && bounded_memory_format(native)
        {
            let mut bytes = Vec::new();
            if formats::RawData(native).read_clipboard(&mut bytes).is_ok() {
                add(mime, Some(bytes));
            }
        }
    }
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    if selected.is_none_or(|s| s == super::LOCAL_PATHS_FORMAT)
        && formats::FileList.is_format_avail()
        && bounded_memory_format(formats::CF_HDROP)
    {
        formats::FileList.read_clipboard(&mut files).ok()?;
        let paths = files
            .into_iter()
            .map(|path| {
                path.is_absolute()
                    .then(|| path.into_os_string().into_string().ok())
                    .flatten()
            })
            .collect::<Option<Vec<_>>>()?;
        if !paths.is_empty() {
            add(
                super::LOCAL_PATHS_FORMAT,
                serde_json::to_vec(&serde_json::json!({"version": 1, "paths": paths})).ok(),
            );
        }
    }
    if selected == Some(super::CONCEALED_FORMAT) && concealed_open() {
        add(super::CONCEALED_FORMAT, Some(b"secret".to_vec()));
    }
    if clipboard_win::seq_num()?.get() as isize != count {
        return None;
    }
    Some(count)
}

pub fn offer(previous: Option<isize>) -> Option<(isize, Vec<String>)> {
    let count = current_revision()?;
    if previous == Some(count) {
        return None;
    }
    let _clipboard = Clipboard::new().ok()?;
    let mut formats = Vec::new();
    if formats::Unicode.is_format_avail() {
        formats.push("text/plain".into());
    }
    if formats::Bitmap.is_format_avail()
        || clipboard_win::register_format("PNG")
            .is_some_and(|f| clipboard_win::is_format_avail(f.get()))
    {
        formats.push("image/png".into());
    }
    if formats::Html::new().is_some_and(|f| f.is_format_avail()) {
        formats.push("text/html".into());
    }
    for (mime, native) in rich_formats() {
        if clipboard_win::is_format_avail(native) {
            formats.push(mime.into());
        }
    }
    if formats::FileList.is_format_avail() {
        formats.push(super::FILES_FORMAT.into());
        formats.push(super::LOCAL_PATHS_FORMAT.into());
    }
    if concealed_open() {
        formats.push(super::CONCEALED_FORMAT.into());
    }
    (current_revision() == Some(count)).then_some((count, formats))
}
pub fn read_format(expected: isize, format: &str) -> Option<Vec<u8>> {
    if current_revision() != Some(expected) {
        return None;
    }
    let mut result = None;
    let captured = capture_selected(Some(format), None, |kind, bytes| {
        if kind == format {
            result = bytes;
        }
    });
    if captured != Some(expected) {
        return None;
    }
    result
}

/// The owner resolves policy values on its reader lane before publishing this offer.
fn concealed_open() -> bool {
    [
        "ExcludeClipboardContentFromMonitorProcessing",
        "CanIncludeInClipboardHistory",
        "CanUploadToCloudClipboard",
    ]
    .iter()
    .any(|name| {
        clipboard_win::register_format(name)
            .is_some_and(|format| clipboard_win::is_format_avail(format.get()))
    })
}

fn rich_formats() -> Vec<(&'static str, u32)> {
    let mut formats = vec![("image/tiff", formats::CF_TIFF)];
    for (mime, native) in [
        ("text/rtf", "Rich Text Format"),
        ("image/jpeg", "JFIF"),
        ("application/pdf", "application/pdf"),
        ("image/svg+xml", "image/svg+xml"),
    ] {
        if let Some(native) = clipboard_win::register_format(native) {
            formats.push((mime, native.get()));
        }
    }
    formats
}

/// Runs only on the foreign-owner reader lane. A delayed flag may block here.
pub(super) fn read_privacy(expected: u32, owner: usize) -> Option<bool> {
    use windows_sys::Win32::System::DataExchange::{GetClipboardOwner, GetClipboardSequenceNumber};
    // SAFETY: Both calls take no arguments and have no preconditions.
    let current = || unsafe {
        GetClipboardSequenceNumber() == expected && GetClipboardOwner() as usize == owner
    };
    if !current() {
        return None;
    }
    let _clipboard = Clipboard::new().ok()?;
    if !current() {
        return None;
    }
    let excluded = clipboard_win::register_format("ExcludeClipboardContentFromMonitorProcessing")
        .is_none_or(|native| clipboard_win::is_format_avail(native.get()));
    let concealed = excluded
        || ["CanIncludeInClipboardHistory", "CanUploadToCloudClipboard"]
            .iter()
            .any(|name| {
                let Some(native) = clipboard_win::register_format(name) else {
                    return true;
                };
                if !clipboard_win::is_format_avail(native.get()) {
                    return false;
                }
                if !clipboard_win::size(native.get()).is_some_and(|size| size.get() <= 64) {
                    return true;
                }
                let mut bytes = Vec::new();
                formats::RawData(native.get())
                    .read_clipboard(&mut bytes)
                    .is_err()
                    || !super::windows_formats::privacy_allows(&bytes)
            });
    current().then_some(concealed)
}
