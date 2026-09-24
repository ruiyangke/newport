use clipboard_win::{
    formats::{self, Format},
    Clipboard, Getter,
};
use std::io::Cursor;

const MAX_CAPTURE_BYTES: usize = 128 * 1024 * 1024;

pub fn revision() -> Option<isize> {
    clipboard_win::seq_num().map(|count| count.get() as isize)
}

fn bounded_memory_format(format: u32) -> bool {
    clipboard_win::size(format).is_some_and(|size| size.get() <= MAX_CAPTURE_BYTES)
}

fn bounded_bitmap() -> bool {
    use windows_sys::Win32::Graphics::Gdi::{GetObjectW, BITMAP};
    let Ok(handle) = clipboard_win::raw::get_clipboard_data(formats::CF_BITMAP) else {
        return false;
    };
    let mut bitmap = BITMAP::default();
    if unsafe {
        GetObjectW(
            handle.as_ptr(),
            std::mem::size_of::<BITMAP>() as i32,
            (&mut bitmap as *mut BITMAP).cast(),
        )
    } == 0
    {
        return false;
    }
    let size = usize::try_from(bitmap.bmWidth).ok().and_then(|width| {
        width
            .checked_mul(usize::try_from(bitmap.bmHeight).ok()?)?
            .checked_mul(4)
    });
    size.is_some_and(|bytes| bytes > 0 && bytes <= MAX_CAPTURE_BYTES)
}

/// Capture a consistent revision. A busy clipboard is retried on the next tick.
/// Collection and publication limits remain in the shared synchronization layer.
pub fn capture(
    previous: Option<isize>,
    mut add: impl FnMut(&str, Option<Vec<u8>>),
) -> Option<isize> {
    let count = clipboard_win::seq_num()?.get() as isize;
    if previous == Some(count) {
        return None;
    }
    let _clipboard = Clipboard::new().ok()?;
    let mut text = String::new();
    if formats::Unicode.is_format_avail() && bounded_memory_format(formats::CF_UNICODETEXT) {
        formats::Unicode.read_clipboard(&mut text).ok()?;
        add("text/plain", Some(text.into_bytes()));
    }
    let png = clipboard_win::register_format("PNG")
        .filter(|format| clipboard_win::is_format_avail(format.get()));
    if let Some(format) = png.filter(|format| bounded_memory_format(format.get())) {
        let mut bytes = Vec::new();
        formats::RawData(format.get())
            .read_clipboard(&mut bytes)
            .ok()?;
        add("image/png", Some(bytes));
    }
    let mut bitmap = Vec::new();
    if png.is_none() && formats::Bitmap.is_format_avail() && bounded_bitmap() {
        formats::Bitmap.read_clipboard(&mut bitmap).ok()?;
        let mut reader =
            image::ImageReader::with_format(Cursor::new(bitmap), image::ImageFormat::Bmp);
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(128 * 1024 * 1024);
        reader.limits(limits);
        if let Ok(image) = reader.decode() {
            let mut png = Cursor::new(Vec::new());
            if image.write_to(&mut png, image::ImageFormat::Png).is_ok() {
                add("image/png", Some(png.into_inner()));
            }
        }
    }
    if let Some(format) = formats::Html::new() {
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
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    if formats::FileList.is_format_avail() && bounded_memory_format(formats::CF_HDROP) {
        formats::FileList.read_clipboard(&mut files).ok()?;
        let urls: Vec<_> = files
            .into_iter()
            .filter_map(|path| url::Url::from_file_path(path).ok())
            .map(|url| url.to_string())
            .collect();
        if !urls.is_empty() {
            add("text/uri-list", Some(urls.join("\n").into_bytes()));
        }
    }
    if clipboard_win::seq_num()?.get() as isize != count {
        return None;
    }
    Some(count)
}

fn html_fragment(bytes: &[u8]) -> Option<&[u8]> {
    // CF_HTML offsets are byte positions, not Unicode character indices.
    let header_end = bytes
        .iter()
        .position(|byte| *byte == b'<')
        .unwrap_or(bytes.len());
    let header = std::str::from_utf8(&bytes[..header_end]).ok()?;
    let offset = |name: &str| -> Option<usize> {
        header
            .lines()
            .find_map(|line| line.strip_prefix(name))?
            .trim()
            .parse()
            .ok()
    };
    bytes.get(offset("StartFragment:")?..offset("EndFragment:")?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn html_offsets_are_bounded_byte_positions() {
        let prefix = "Version:1.0\r\nStartFragment:0000000065\r\nEndFragment:0000000067\r\n";
        let start = prefix.len() + 4;
        let input = format!(
            "Version:1.0\r\nStartFragment:{start:010}\r\nEndFragment:{:010}\r\n<!--é-->",
            start + 2
        );
        assert_eq!(html_fragment(input.as_bytes()), Some("é".as_bytes()));
        assert!(html_fragment(b"StartFragment:999\nEndFragment:1000\n<x>").is_none());
        assert!(html_fragment(b"StartFragment:40\nEndFragment:1\n<x>").is_none());
        assert!(html_fragment(b"StartFragment:-1\nEndFragment:1\n<x>").is_none());
    }
}
