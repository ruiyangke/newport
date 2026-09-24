use objc2::AnyThread;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSPasteboard};
use objc2_foundation::{NSDictionary, NSString};

fn data(board: &NSPasteboard, kind: &str) -> Option<Vec<u8>> {
    let t = NSString::from_str(kind);
    if matches!(
        kind,
        "public.utf8-plain-text" | "public.html" | "public.url" | "public.file-url"
    ) {
        return board.stringForType(&t).map(|s| s.to_string().into_bytes());
    }
    if let Some(data) = board.dataForType(&t) {
        return Some(data.to_vec());
    }
    if kind == "public.png" {
        let tiff = board
            .dataForType(&NSString::from_str("public.tiff"))
            .or_else(|| {
                NSImage::initWithPasteboard(NSImage::alloc(), board)
                    .and_then(|image| image.TIFFRepresentation())
            })?;
        let image = NSBitmapImageRep::imageRepWithData(&tiff)?;
        return unsafe {
            image.representationUsingType_properties(
                NSBitmapImageFileType::PNG,
                &NSDictionary::new(),
            )
        }
        .map(|d| d.to_vec());
    }
    None
}
fn urls(board: &NSPasteboard) -> Option<Vec<u8>> {
    let mut urls = Vec::new();
    if let Some(items) = board.pasteboardItems() {
        for item in items.iter() {
            if let Some(url) = item
                .stringForType(&NSString::from_str("public.url"))
                .or_else(|| item.stringForType(&NSString::from_str("public.file-url")))
            {
                urls.push(url.to_string());
            }
        }
    }
    if urls.is_empty() {
        None
    } else {
        Some(urls.join("\n").into_bytes())
    }
}

/// Read on the app's main thread. The shared consumer applies size/format limits.
/// `None` means unchanged or changed during capture: discard collected formats.
/// A returned revision, including an empty copy, replaces the previous snapshot.
pub fn capture(
    previous: Option<isize>,
    mut add: impl FnMut(&str, Option<Vec<u8>>),
) -> Option<isize> {
    let board = NSPasteboard::generalPasteboard();
    let count = board.changeCount();
    if previous == Some(count) {
        return None;
    }
    add("text/plain", data(&board, "public.utf8-plain-text"));
    add("image/png", data(&board, "public.png"));
    add("text/html", data(&board, "public.html"));
    add("text/uri-list", urls(&board));
    if let Some(types) = board.types() {
        for kind in types.iter().map(|t| t.to_string()) {
            if !matches!(
                kind.as_str(),
                "public.utf8-plain-text"
                    | "public.html"
                    | "public.png"
                    | "public.url"
                    | "public.file-url"
            ) {
                add(&kind, data(&board, &kind));
            }
        }
    }
    // Retry next tick if an external app changed the pasteboard during capture.
    if board.changeCount() != count {
        return None;
    }
    Some(count)
}
