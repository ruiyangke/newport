use objc2::AnyThread;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSPasteboard};
use objc2_foundation::{NSDictionary, NSString};

pub fn revision() -> Option<isize> {
    Some(NSPasteboard::generalPasteboard().changeCount())
}

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

/// Inspect types only. Never asks a pasteboard owner to render its data.
pub fn offer(previous: Option<isize>) -> Option<(isize, Vec<String>)> {
    let board = NSPasteboard::generalPasteboard();
    offer_from(&board, previous)
}
fn offer_from(board: &NSPasteboard, previous: Option<isize>) -> Option<(isize, Vec<String>)> {
    let revision = board.changeCount();
    if previous == Some(revision) {
        return None;
    }
    let types: Vec<String> = board
        .types()
        .map(|types| types.iter().map(|s| s.to_string()).collect())
        .unwrap_or_default();
    let mut formats = Vec::new();
    for kind in &types {
        let canonical = match kind.as_str() {
            "public.utf8-plain-text" => "text/plain",
            "public.png" | "public.tiff" => "image/png",
            "public.html" => "text/html",
            "public.url" | "public.file-url" => "text/uri-list",
            other => other,
        };
        if !formats.iter().any(|s| s == canonical) {
            formats.push(canonical.to_owned());
        }
    }
    (board.changeCount() == revision).then_some((revision, formats))
}
pub fn read_format(revision: isize, format: &str) -> Option<Vec<u8>> {
    let board = NSPasteboard::generalPasteboard();
    read_from(&board, revision, format)
}
fn read_from(board: &NSPasteboard, revision: isize, format: &str) -> Option<Vec<u8>> {
    if board.changeCount() != revision {
        return None;
    }
    let bytes = match format {
        "text/plain" => data(board, "public.utf8-plain-text"),
        "image/png" => data(board, "public.png"),
        "text/html" => data(board, "public.html"),
        "text/uri-list" => urls(board),
        other => data(board, other),
    };
    if board.changeCount() != revision {
        return None;
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_pasteboard_preserves_unicode_formats_and_rejects_stale_reads() {
        objc2::rc::autoreleasepool(|_| {
            // Never read or replace the user's general clipboard.
            let board = NSPasteboard::pasteboardWithUniqueName();
            board.clearContents();
            assert!(board.setString_forType(
                &NSString::from_str("Newport 世界\n"),
                &NSString::from_str("public.utf8-plain-text")
            ));
            assert!(board.setString_forType(
                &NSString::from_str("<b>世界</b>"),
                &NSString::from_str("public.html")
            ));
            let (revision, formats) = offer_from(&board, None).unwrap();
            assert!(formats.iter().any(|kind| kind == "text/plain"));
            assert!(formats.iter().any(|kind| kind == "text/html"));
            assert!(offer_from(&board, Some(revision)).is_none());
            assert_eq!(
                read_from(&board, revision, "text/plain").unwrap(),
                "Newport 世界\n".as_bytes()
            );
            assert_eq!(
                read_from(&board, revision, "text/html").unwrap(),
                "<b>世界</b>".as_bytes()
            );
            board.clearContents();
            assert!(read_from(&board, revision, "text/plain").is_none());
            let (cleared, formats) = offer_from(&board, Some(revision)).unwrap();
            assert_ne!(cleared, revision);
            assert!(formats.is_empty());
        });
    }
}
