use objc2::AnyThread;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSPasteboard};
use objc2_foundation::{NSDictionary, NSString, NSURL, NSUTF8StringEncoding};
use std::path::PathBuf;

pub fn revision() -> Option<isize> {
    Some(NSPasteboard::generalPasteboard().changeCount())
}

fn string_bytes(string: &NSString, limit: usize) -> Option<Vec<u8>> {
    (string.lengthOfBytesUsingEncoding(NSUTF8StringEncoding) <= limit)
        .then(|| string.to_string().into_bytes())
}
fn data(board: &NSPasteboard, kind: &str, limit: usize) -> Option<Vec<u8>> {
    let t = NSString::from_str(kind);
    if matches!(
        kind,
        "public.utf8-plain-text" | "public.html" | "public.url"
    ) {
        return board
            .stringForType(&t)
            .and_then(|s| string_bytes(&s, limit));
    }
    if let Some(data) = board.dataForType(&t) {
        return (data.len() <= limit).then(|| data.to_vec());
    }
    if kind == "public.png" {
        let tiff = board
            .dataForType(&NSString::from_str("public.tiff"))
            .or_else(|| {
                NSImage::initWithPasteboard(NSImage::alloc(), board)
                    .and_then(|image| image.TIFFRepresentation())
            })?;
        if tiff.len() > limit {
            return None;
        }
        let image = NSBitmapImageRep::imageRepWithData(&tiff)?;
        return unsafe {
            image.representationUsingType_properties(
                NSBitmapImageFileType::PNG,
                &NSDictionary::new(),
            )
        }
        .filter(|d| d.len() <= limit)
        .map(|d| d.to_vec());
    }
    None
}
fn is_file_url(url: &str) -> bool {
    url.trim_start()
        .get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("file:"))
}
fn urls(board: &NSPasteboard, limit: usize) -> Option<Vec<u8>> {
    let mut urls = Vec::new();
    let mut found = false;
    if let Some(items) = board.pasteboardItems() {
        for item in items.iter() {
            if let Some(url) = item.stringForType(&NSString::from_str("public.url")) {
                // A `file:` URL names a Mac path, useless on the server.
                if is_file_url(&url.to_string()) {
                    continue;
                }
                // RFC 2483: text/uri-list lines end with CRLF.
                let separator = 2 * usize::from(found);
                let remaining = limit.checked_sub(urls.len())?.checked_sub(separator)?;
                let bytes = string_bytes(&url, remaining)?;
                if found {
                    urls.extend_from_slice(b"\r\n");
                }
                urls.extend_from_slice(&bytes);
                found = true;
            }
        }
    }
    if !found { None } else { Some(urls) }
}

/// Local paths of the files on `board`, one per pasteboard item. None unless
/// every item is a local file URL and the board is still at `revision`. Up to
/// [`super::files::MAX_READ_ITEMS`] items are returned so that `plan` refuses
/// more than `MAX_ITEMS` with a named error.
pub(super) fn file_paths(board: &NSPasteboard, revision: isize) -> Option<Vec<PathBuf>> {
    if board.changeCount() != revision {
        return None;
    }
    let items = board.pasteboardItems()?;
    if items.is_empty() || items.len() > super::files::MAX_READ_ITEMS {
        return None;
    }
    let kind = NSString::from_str("public.file-url");
    let mut paths = Vec::with_capacity(items.len());
    for item in items.iter() {
        let url = NSURL::URLWithString(&*item.stringForType(&kind)?)?;
        if !url.isFileURL() {
            return None;
        }
        // Resolves `file:///.file/id=...` references; plain file URLs pass through.
        let url = url.filePathURL()?;
        let path = PathBuf::from(url.path()?.to_string());
        if !path.is_absolute() {
            return None;
        }
        paths.push(path);
    }
    (board.changeCount() == revision).then_some(paths)
}

/// `{"version":1,"paths":[...]}` of the copied files' local paths.
fn local_paths(board: &NSPasteboard, revision: isize, limit: usize) -> Option<Vec<u8>> {
    let paths = file_paths(board, revision)?
        .into_iter()
        .map(|path| path.into_os_string().into_string().ok())
        .collect::<Option<Vec<_>>>()?;
    #[derive(serde::Serialize)]
    struct LocalPaths {
        version: u32,
        paths: Vec<String>,
    }
    serde_json::to_vec(&LocalPaths { version: 1, paths })
        .ok()
        .filter(|bytes| bytes.len() <= limit)
}

/// The pasteboard types `read_bounded` can serve, by canonical format. Other
/// types are not offered: a raw macOS type name means nothing to a remote app.
/// File URLs are offered as the files formats by `offer_from`, never as a URI
/// list: a Mac path is useless on the server.
fn canonical(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "public.utf8-plain-text" => "text/plain",
        "public.html" => "text/html",
        "public.rtf" => "text/rtf",
        "public.png" | "public.tiff" => "image/png",
        "public.jpeg" => "image/jpeg",
        "com.adobe.pdf" => "application/pdf",
        "public.url" => "text/uri-list",
        super::CONCEALED_TYPE => super::CONCEALED_FORMAT,
        _ => return None,
    })
}

/// Whether the board holds items and every one of them is a file. A copy of
/// files together with other things offers no files: the remote side has one
/// item per file, so a partial file list would silently drop the rest.
fn only_files(board: &NSPasteboard) -> bool {
    let kind = NSString::from_str("public.file-url");
    board.pasteboardItems().is_some_and(|items| {
        !items.is_empty()
            && items
                .iter()
                .all(|item| item.types().iter().any(|candidate| *candidate == *kind))
    })
}

/// Inspect types only. Never asks a pasteboard owner to render its data.
pub fn offer(previous: Option<isize>) -> Option<(isize, Vec<String>)> {
    let board = NSPasteboard::generalPasteboard();
    offer_from(&board, previous)
}
pub(super) fn offer_from(
    board: &NSPasteboard,
    previous: Option<isize>,
) -> Option<(isize, Vec<String>)> {
    let revision = board.changeCount();
    if previous == Some(revision) {
        return None;
    }
    let types: Vec<String> = board
        .types()
        .map(|types| types.iter().map(|s| s.to_string()).collect())
        .unwrap_or_default();
    let mut formats = Vec::new();
    for canonical in types.iter().filter_map(|kind| canonical(kind)) {
        if !formats.iter().any(|s| s == canonical) {
            formats.push(canonical.to_owned());
        }
    }
    if only_files(board) {
        // Files go as files; a `public.url` beside them is their Mac path.
        formats.retain(|format| format != "text/uri-list");
        formats.push(super::FILES_FORMAT.to_owned());
        formats.push(super::LOCAL_PATHS_FORMAT.to_owned());
    }
    (board.changeCount() == revision).then_some((revision, formats))
}
pub fn read_format(revision: isize, format: &str) -> Option<Vec<u8>> {
    let board = NSPasteboard::generalPasteboard();
    read_from(&board, revision, format)
}
fn read_from(board: &NSPasteboard, revision: isize, format: &str) -> Option<Vec<u8>> {
    read_bounded(board, revision, format, super::provider::MAX_BYTES)
}
pub(super) fn read_bounded(
    board: &NSPasteboard,
    revision: isize,
    format: &str,
    limit: usize,
) -> Option<Vec<u8>> {
    if board.changeCount() != revision {
        return None;
    }
    let bytes = match format {
        "text/plain" => data(board, "public.utf8-plain-text", limit),
        "image/png" => data(board, "public.png", limit),
        "text/html" => data(board, "public.html", limit),
        "text/rtf" => data(board, "public.rtf", limit),
        "image/jpeg" => data(board, "public.jpeg", limit),
        "application/pdf" => data(board, "com.adobe.pdf", limit),
        "text/uri-list" => urls(board, limit),
        super::CONCEALED_FORMAT => board
            .types()
            .is_some_and(|types| {
                types
                    .iter()
                    .any(|kind| kind.to_string() == super::CONCEALED_TYPE)
            })
            .then(|| b"secret".to_vec()),
        super::LOCAL_PATHS_FORMAT => local_paths(board, revision, limit),
        _ => None,
    };
    if board.changeCount() != revision {
        return None;
    }
    bytes.filter(|bytes| bytes.len() <= limit)
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

    #[test]
    fn offers_only_readable_formats_and_never_mac_paths() {
        objc2::rc::autoreleasepool(|_| {
            let board = NSPasteboard::pasteboardWithUniqueName();
            board.clearContents();
            let set = |bytes: &[u8], kind: &str| {
                let data = objc2_foundation::NSData::with_bytes(bytes);
                assert!(board.setData_forType(Some(&data), &NSString::from_str(kind)));
            };
            set(br"{\rtf1 \b Newport\b0}", "public.rtf");
            set(b"%PDF-1.4 fixture", "com.adobe.pdf");
            set(b"https://newport.app/", "public.url");
            // AppKit reads NSStringPboardType as UTF-8 text; the raw name is never offered.
            set(b"plain", "NSStringPboardType");
            set(b"fixture", "com.example.private");
            let (revision, mut formats) = offer_from(&board, None).unwrap();
            formats.sort();
            assert_eq!(
                formats,
                ["application/pdf", "text/plain", "text/rtf", "text/uri-list"]
            );
            assert_eq!(read_from(&board, revision, "text/plain").unwrap(), b"plain");
            assert_eq!(
                read_from(&board, revision, "text/rtf").unwrap(),
                br"{\rtf1 \b Newport\b0}"
            );
            assert_eq!(
                read_from(&board, revision, "application/pdf").unwrap(),
                b"%PDF-1.4 fixture"
            );
            assert_eq!(
                read_from(&board, revision, "text/uri-list").unwrap(),
                b"https://newport.app/"
            );
            assert!(read_from(&board, revision, "NSStringPboardType").is_none());
            assert!(read_from(&board, revision, "com.example.private").is_none());

            board.clearContents();
            set(b"file:///Users/someone/report.pdf", "public.file-url");
            let (revision, formats) = offer_from(&board, None).unwrap();
            assert_eq!(
                formats,
                [super::super::FILES_FORMAT, super::super::LOCAL_PATHS_FORMAT]
            );
            assert!(
                !formats
                    .iter()
                    .any(|kind| kind.contains("Users") || kind.contains("file:"))
            );
            assert!(read_from(&board, revision, "text/uri-list").is_none());
            assert!(read_from(&board, revision, super::super::FILES_FORMAT).is_none());

            board.clearContents();
            let urls = |values: &[&str]| {
                let items: Vec<_> = values
                    .iter()
                    .map(|value| {
                        let item = objc2_app_kit::NSPasteboardItem::new();
                        assert!(item.setString_forType(
                            &NSString::from_str(value),
                            &NSString::from_str("public.url")
                        ));
                        item
                    })
                    .collect();
                write_items(&board, &items);
                board.changeCount()
            };
            let revision = urls(&[
                "FILE:///Users/someone/a.txt",
                "https://newport.app/a",
                "File:///x",
            ]);
            assert_eq!(
                read_from(&board, revision, "text/uri-list").unwrap(),
                b"https://newport.app/a"
            );
            let revision = urls(&["file:///Users/someone/a.txt", "FILE:///Users/someone/b.txt"]);
            assert!(read_from(&board, revision, "text/uri-list").is_none());

            board.clearContents();
            set(b"", super::super::CONCEALED_TYPE);
            set(b"hunter2", "public.utf8-plain-text");
            let (revision, mut formats) = offer_from(&board, None).unwrap();
            formats.sort();
            assert_eq!(formats, [super::super::CONCEALED_FORMAT, "text/plain"]);
            assert_eq!(
                read_from(&board, revision, super::super::CONCEALED_FORMAT).unwrap(),
                b"secret"
            );
            board.clearContents();
            set(b"x", "public.utf8-plain-text");
            let (revision, formats) = offer_from(&board, None).unwrap();
            assert_eq!(formats, ["text/plain"]);
            assert!(read_from(&board, revision, super::super::CONCEALED_FORMAT).is_none());

            board.clearContents();
            set(b"Finder text", "public.utf8-plain-text");
            set(b"file:///Users/someone/report.pdf", "public.file-url");
            let (_, formats) = offer_from(&board, None).unwrap();
            assert_eq!(
                formats,
                [
                    "text/plain",
                    super::super::FILES_FORMAT,
                    super::super::LOCAL_PATHS_FORMAT
                ]
            );
        });
    }

    fn file_item(url: &str) -> objc2::rc::Retained<objc2_app_kit::NSPasteboardItem> {
        let item = objc2_app_kit::NSPasteboardItem::new();
        assert!(item.setString_forType(
            &NSString::from_str(url),
            &NSString::from_str("public.file-url")
        ));
        item
    }

    fn write_items(
        board: &NSPasteboard,
        items: &[objc2::rc::Retained<objc2_app_kit::NSPasteboardItem>],
    ) {
        use objc2::runtime::ProtocolObject;
        board.clearContents();
        let objects: Vec<_> = items
            .iter()
            .map(|i| ProtocolObject::from_ref(&**i))
            .collect();
        assert!(board.writeObjects(&objc2_foundation::NSArray::from_slice(&objects)));
    }

    #[test]
    fn files_are_offered_only_when_every_item_is_a_file() {
        objc2::rc::autoreleasepool(|_| {
            let board = NSPasteboard::pasteboardWithUniqueName();
            let text_item = |text: &str| {
                let item = objc2_app_kit::NSPasteboardItem::new();
                assert!(item.setString_forType(
                    &NSString::from_str(text),
                    &NSString::from_str("public.utf8-plain-text")
                ));
                item
            };
            let files = [super::super::FILES_FORMAT, super::super::LOCAL_PATHS_FORMAT];
            write_items(
                &board,
                &[
                    file_item("file:///tmp/a.txt"),
                    file_item("file:///tmp/b.txt"),
                ],
            );
            assert_eq!(offer_from(&board, None).unwrap().1, files);

            write_items(&board, &[file_item("file:///tmp/a.txt"), text_item("note")]);
            let (revision, formats) = offer_from(&board, None).unwrap();
            assert_eq!(formats, ["text/plain"]);
            assert!(read_from(&board, revision, super::super::FILES_FORMAT).is_none());
            assert!(read_from(&board, revision, super::super::LOCAL_PATHS_FORMAT).is_none());

            write_items(&board, &[text_item("note"), file_item("file:///tmp/a.txt")]);
            assert_eq!(offer_from(&board, None).unwrap().1, ["text/plain"]);

            write_items(&board, &[text_item("note")]);
            assert_eq!(offer_from(&board, None).unwrap().1, ["text/plain"]);
            board.clearContents();
            assert!(offer_from(&board, None).unwrap().1.is_empty());
        });
    }

    #[test]
    fn local_paths_are_json_and_fenced_to_the_revision() {
        objc2::rc::autoreleasepool(|_| {
            let board = NSPasteboard::pasteboardWithUniqueName();
            write_items(
                &board,
                &[
                    file_item("file:///tmp/a%20b.txt"),
                    file_item("file:///tmp/%E5%90%8D.txt"),
                ],
            );
            let revision = board.changeCount();
            let format = super::super::LOCAL_PATHS_FORMAT;
            assert_eq!(
                read_from(&board, revision, format).unwrap(),
                r#"{"version":1,"paths":["/tmp/a b.txt","/tmp/名.txt"]}"#.as_bytes()
            );
            assert!(read_from(&board, revision + 1, format).is_none());
            assert!(read_bounded(&board, revision, format, 10).is_none());
            write_items(&board, &[file_item("file:///tmp/a.txt")]);
            assert!(read_from(&board, revision, format).is_none());
            write_items(&board, &[file_item("https://newport.app/a")]);
            assert!(read_from(&board, board.changeCount(), format).is_none());
        });
    }

    #[test]
    fn more_than_the_item_limit_is_still_read_so_the_plan_can_refuse_it() {
        objc2::rc::autoreleasepool(|_| {
            let board = NSPasteboard::pasteboardWithUniqueName();
            let items: Vec<_> = (0..super::super::files::MAX_ITEMS + 1)
                .map(|index| file_item(&format!("file:///tmp/{index}.txt")))
                .collect();
            write_items(&board, &items);
            let paths = file_paths(&board, board.changeCount()).unwrap();
            assert_eq!(paths.len(), super::super::files::MAX_ITEMS + 1);
            assert_eq!(
                super::super::files::plan(&paths, &super::super::files::Limits::CLIPBOARD),
                Err(super::super::files::ManifestError::TooManyItems)
            );
        });
    }

    #[test]
    fn file_urls_resolve_to_local_paths_and_respect_the_revision() {
        objc2::rc::autoreleasepool(|_| {
            let board = NSPasteboard::pasteboardWithUniqueName();
            let dir = std::env::temp_dir().canonicalize().unwrap();
            let paths = [
                dir.join("plain.txt"),
                dir.join("with space.txt"),
                dir.join("名前 ✓.txt"),
            ];
            let items: Vec<_> = paths
                .iter()
                .map(|p| {
                    file_item(
                        NSURL::fileURLWithPath(&NSString::from_str(p.to_str().unwrap()))
                            .absoluteString()
                            .unwrap()
                            .to_string()
                            .as_str(),
                    )
                })
                .collect();
            write_items(&board, &items);
            let revision = board.changeCount();
            assert_eq!(file_paths(&board, revision).unwrap(), paths);
            assert!(file_paths(&board, revision + 1).is_none());

            write_items(&board, &[file_item("https://newport.app/a")]);
            assert!(file_paths(&board, board.changeCount()).is_none());

            let mixed = objc2_app_kit::NSPasteboardItem::new();
            assert!(mixed.setString_forType(
                &NSString::from_str("https://newport.app/"),
                &NSString::from_str("public.url")
            ));
            write_items(&board, &[file_item("file:///tmp/a.txt"), mixed]);
            assert!(file_paths(&board, board.changeCount()).is_none());

            board.clearContents();
            assert!(file_paths(&board, board.changeCount()).is_none());

            let stale = board.changeCount();
            write_items(&board, &[file_item("file:///tmp/a.txt")]);
            assert!(file_paths(&board, stale).is_none());
        });
    }
}
