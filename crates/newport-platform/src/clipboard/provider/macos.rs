use super::{MAX_BYTES, MAX_FORMATS, MAX_WAIT, Pending, Source};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, MainThreadMarker, define_class, msg_send};
use objc2_app_kit::{
    NSPasteboard, NSPasteboardItem, NSPasteboardItemDataProvider, NSPasteboardWriting,
};
use objc2_foundation::{NSArray, NSData, NSObject, NSObjectProtocol, NSString, NSURL};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// Explicit UTI mappings only: an arbitrary remote MIME name is not a native
// UTI. A URI list becomes its first link (see `first_link`).
fn native_type(format: &str) -> Option<&'static str> {
    Some(match format {
        "text/uri-list" => "public.url",
        "text/plain" => "public.utf8-plain-text",
        "text/html" => "public.html",
        "text/rtf" | "application/rtf" => "public.rtf",
        "image/png" => "public.png",
        "image/jpeg" => "public.jpeg",
        "image/tiff" => "public.tiff",
        "image/svg+xml" => "public.svg-image",
        "application/pdf" => "com.adobe.pdf",
        super::super::CONCEALED_FORMAT => super::super::CONCEALED_TYPE,
        _ => return None,
    })
}

/// The first web or mail link in a text/uri-list (RFC 2483: CRLF lines, `#`
/// comments). File URLs are skipped: a server path is not a file on this Mac,
/// and files need content transfer rather than a link.
fn first_link(list: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(list)
        .ok()?
        .trim_start_matches('\u{feff}');
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .find(|line| is_link(line))
        .map(|link| link.as_bytes().to_vec())
}

/// A hierarchical URL (`scheme://…`) or a mail/telephone address, without
/// whitespace or control characters. Plain words with a colon are not links.
fn is_link(line: &str) -> bool {
    let Some((scheme, rest)) = line.split_once(':') else {
        return false;
    };
    let scheme_valid = scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c));
    let kind_valid = rest.starts_with("//")
        || ["mailto", "tel", "sms"]
            .iter()
            .any(|s| scheme.eq_ignore_ascii_case(s));
    scheme_valid
        && kind_valid
        && !scheme.eq_ignore_ascii_case("file")
        && !rest.is_empty()
        && !line.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Most entries one attachment may publish.
const MAX_FILES: usize = 10_000;

struct Offer {
    source: Arc<dyn Source>,
    formats: Vec<(String, &'static str)>,
    active: AtomicBool,
    revision: AtomicIsize,
    bytes: Mutex<usize>,
    wait: Duration,
}

impl Offer {
    fn current(&self, board: &NSPasteboard) -> bool {
        self.active.load(Ordering::Acquire)
            && self.source.is_current()
            && board.changeCount() == self.revision.load(Ordering::Acquire)
    }

    fn provide(&self, board: &NSPasteboard, item: &NSPasteboardItem, kind: &NSString) {
        let kind_string = kind.to_string();
        if kind_string == super::super::TRANSIENT_TYPE {
            // A marker for clipboard managers; it has no remote source.
            if self.current(board) {
                item.setData_forType(&NSData::with_bytes(&[]), kind);
            }
            return;
        }
        let Some((format, _)) = self.formats.iter().find(|(_, uti)| *uti == kind_string) else {
            return;
        };
        if !self.current(board) {
            return;
        }
        let (pending, reply) = Pending::new(self.wait);
        self.source.request(format, reply);
        let Some(payload) = pending.wait(|| self.current(board)) else {
            return;
        };
        let Some(bytes) = payload.bytes() else {
            return;
        };
        if bytes.len() > MAX_BYTES
            || (format == "text/plain" && std::str::from_utf8(bytes).is_err())
        {
            return;
        }
        let link;
        let bytes = if format == "text/uri-list" {
            let Some(first) = first_link(bytes) else {
                return;
            };
            link = first;
            link.as_slice()
        } else {
            bytes
        };
        let mut used = self.bytes.lock().unwrap_or_else(|e| e.into_inner());
        if bytes.len() > MAX_BYTES.saturating_sub(*used) {
            return;
        }
        let data = NSData::with_bytes(bytes);
        // Recheck after waiting and allocating. Keep the payload's independent
        // transport fence alive through setData; never cache a detached Vec.
        if payload.bytes().is_none() || pending.expired() || !self.current(board) {
            return;
        }
        if item.setData_forType(&data, kind) {
            *used += bytes.len();
        }
    }
}

define_class!(
    // NSObject imposes no subclassing requirements. All Rust state is either
    // immutable or synchronized; no native object is shared between threads.
    #[unsafe(super = NSObject)]
    #[ivars = Arc<Offer>]
    struct NewportLazyPasteboardProvider;

    unsafe impl NSObjectProtocol for NewportLazyPasteboardProvider {}

    unsafe impl NSPasteboardItemDataProvider for NewportLazyPasteboardProvider {
        #[unsafe(method(pasteboard:item:provideDataForType:))]
        fn provide(&self, board: Option<&NSPasteboard>, item: &NSPasteboardItem, kind: &NSString) {
            // Do not unwind Rust panics across AppKit's Objective-C boundary.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if let Some(board) = board { self.ivars().provide(board, item, kind); }
            }));
            if result.is_err() { log::warn!("Native clipboard provider failed"); }
        }
    }
);

/// One promised native offer. Construct and service it on the dedicated
/// clipboard helper's main run loop, not Newport's UI run loop: AppKit can
/// synchronously call the provider on the main thread, even for worker-thread
/// registration. No additional SSH connection is involved.
pub struct Publication {
    board: Retained<NSPasteboard>,
    offer: Arc<Offer>,
    _provider: Retained<NewportLazyPasteboardProvider>,
    _main: MainThreadMarker,
}

impl Publication {
    /// `source` is the host router's exact, revocable offer lease. Inspect and
    /// preflight everything before modifying the native pasteboard. AppKit has
    /// no atomic compare-and-swap; these checks narrow, not eliminate, the race
    /// with a different application's copy operation.
    pub fn publish(
        main: MainThreadMarker,
        board: Retained<NSPasteboard>,
        expected_revision: isize,
        formats: &[String],
        source: Arc<dyn Source>,
        wait: Duration,
    ) -> Result<Self, &'static str> {
        if formats.is_empty() || formats.len() > MAX_FORMATS {
            return Err("Invalid clipboard offer size");
        }
        let mut mapped = Vec::new();
        for format in formats {
            if let Some(kind) = native_type(format)
                && !mapped.iter().any(|(_, existing)| *existing == kind)
            {
                mapped.push((format.clone(), kind));
            }
        }
        if mapped.is_empty() {
            return Err("No supported native clipboard formats");
        }
        let mut types: Vec<_> = mapped
            .iter()
            .map(|(_, kind)| NSString::from_str(kind))
            .collect();
        // The names-only copy that precedes `attach_files` is replaced within
        // moments; clipboard managers need not record it.
        if formats
            .iter()
            .any(|format| format == super::super::FILES_FORMAT)
        {
            types.push(NSString::from_str(super::super::TRANSIENT_TYPE));
        }
        let offer = Arc::new(Offer {
            source,
            formats: mapped,
            active: AtomicBool::new(true),
            revision: AtomicIsize::new(expected_revision),
            bytes: Mutex::new(0),
            wait: wait.min(MAX_WAIT),
        });
        let provider = NewportLazyPasteboardProvider::alloc().set_ivars(offer.clone());
        // NSObject's init has the standard retained initialization signature.
        let provider: Retained<NewportLazyPasteboardProvider> =
            unsafe { msg_send![super(provider), init] };
        let item = NSPasteboardItem::new();
        if !item.setDataProvider_forTypes(
            ProtocolObject::from_ref(&*provider),
            &NSArray::from_retained_slice(&types),
        ) {
            return Err("Could not register clipboard provider");
        }
        if !offer.current(&board) {
            return Err("Clipboard ownership changed");
        }
        let cleared = board.clearContents();
        offer.revision.store(cleared, Ordering::Release);
        if !offer.current(&board) {
            return Err("Clipboard ownership changed");
        }
        let objects =
            NSArray::from_slice(&[ProtocolObject::<dyn NSPasteboardWriting>::from_ref(&*item)]);
        if !board.writeObjects(&objects) {
            return Err("Could not publish clipboard offer");
        }
        if !offer.current(&board) {
            offer.active.store(false, Ordering::Release);
            return Err("Clipboard ownership changed");
        }
        Ok(Self {
            board,
            offer,
            _provider: provider,
            _main: main,
        })
    }

    /// Replace this publication's promises with one `public.file-url` item per
    /// entry of `folder` (sorted by name), each also carrying its name as text.
    /// Everything is set eagerly; no provider callback is involved. Only while
    /// the board still holds this publication at `expected`; otherwise the
    /// board is untouched. Returns the new change count.
    pub fn attach_files(&self, expected: isize, folder: &Path) -> Option<isize> {
        let board = &self.board;
        if board.changeCount() != expected || !self.offer.current(board) {
            return None;
        }
        let mut entries: Vec<_> = std::fs::read_dir(folder)
            .ok()?
            .map(|entry| entry.map(|e| e.path()))
            .collect::<Result<_, _>>()
            .ok()?;
        if entries.is_empty() || entries.len() > MAX_FILES {
            return None;
        }
        entries.sort();
        let file_url = NSString::from_str("public.file-url");
        let text = NSString::from_str("public.utf8-plain-text");
        let mut items = Vec::with_capacity(entries.len());
        for path in &entries {
            let name = path.file_name()?.to_str()?;
            let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str()?));
            let location = url.absoluteString()?;
            let item = NSPasteboardItem::new();
            if !item.setString_forType(&location, &file_url)
                || !item.setString_forType(&NSString::from_str(name), &text)
            {
                return None;
            }
            items.push(item);
        }
        let objects: Vec<_> = items
            .iter()
            .map(|item| ProtocolObject::<dyn NSPasteboardWriting>::from_ref(&**item))
            .collect();
        if !self.offer.current(board) {
            return None;
        }
        let cleared = board.clearContents();
        self.offer.revision.store(cleared, Ordering::Release);
        if !self.offer.current(board) {
            return None;
        }
        if !board.writeObjects(&NSArray::from_slice(&objects)) || !self.offer.current(board) {
            return None;
        }
        Some(cleared)
    }

    pub fn revision(&self) -> isize {
        self.offer.revision.load(Ordering::Acquire)
    }

    /// Revoke outstanding and cached promises only while this publication still
    /// owns the board. Never clear a later local copy. This is also the explicit
    /// sharing-off/shutdown path; dropping alone only fences pending callbacks.
    pub fn withdraw(&self) -> bool {
        self.withdraw_revision().is_some()
    }
    /// Return the revision produced by our clear, not a later change-count
    /// observation which might already belong to another application's copy.
    pub(crate) fn withdraw_revision(&self) -> Option<isize> {
        self.offer.active.store(false, Ordering::Release);
        if self.board.changeCount() != self.revision() {
            return None;
        }
        Some(self.board.clearContents())
    }
}

impl Drop for Publication {
    fn drop(&mut self) {
        self.offer.active.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_app_kit::NSPasteboardTypeString;

    #[test]
    fn uri_lists_become_their_first_non_file_link() {
        assert_eq!(native_type("text/uri-list"), Some("public.url"));
        assert_eq!(
            first_link(b"# comment\r\nhttps://newport.app/a?b=1\r\nhttps://example.org/\r\n")
                .unwrap(),
            b"https://newport.app/a?b=1"
        );
        assert_eq!(
            first_link(b"mailto:team@example.org").unwrap(),
            b"mailto:team@example.org"
        );
        // File URLs and non-links before the first real link are skipped.
        assert_eq!(
            first_link(b"file:///home/a.txt\r\nNote: hello\r\nhttps://example.org/x\r\n").unwrap(),
            b"https://example.org/x"
        );
        assert_eq!(
            first_link("\u{feff}https://newport.app/".as_bytes()).unwrap(),
            b"https://newport.app/"
        );
        for refused in [
            &b"file:///home/newport/report.pdf"[..],
            b"FILE:///tmp/x",
            b"",
            b"# only a comment\r\n",
            b"not a link",
            b"Note: hello",
            b"localhost:8080",
            b"C:\\x",
            b"https://exa mple.org",
            b"1http://x",
            b"\xff\xfe",
        ] {
            assert!(first_link(refused).is_none(), "{refused:?}");
        }
    }

    struct Always;
    impl Source for Always {
        fn is_current(&self) -> bool {
            true
        }
        fn request(&self, _: &str, _: super::super::Reply) {}
    }
    fn published(board: &Retained<NSPasteboard>) -> Publication {
        board.clearContents();
        // Test-only: pasteboards are not main-thread bound, only the marker is.
        let main = unsafe { MainThreadMarker::new_unchecked() };
        Publication::publish(
            main,
            board.clone(),
            board.changeCount(),
            &["text/plain".into()],
            Arc::new(Always),
            Duration::from_secs(1),
        )
        .unwrap()
    }
    fn board_types(board: &NSPasteboard) -> Vec<String> {
        board
            .types()
            .unwrap()
            .iter()
            .map(|kind| kind.to_string())
            .collect()
    }
    fn publish_formats(board: &Retained<NSPasteboard>, formats: &[&str]) -> Publication {
        board.clearContents();
        // Test-only: pasteboards are not main-thread bound, only the marker is.
        let main = unsafe { MainThreadMarker::new_unchecked() };
        let formats: Vec<String> = formats.iter().map(|f| (*f).to_owned()).collect();
        Publication::publish(
            main,
            board.clone(),
            board.changeCount(),
            &formats,
            Arc::new(Always),
            Duration::from_secs(1),
        )
        .unwrap()
    }

    #[test]
    fn password_hints_become_concealed_and_names_only_copies_transient() {
        let board = NSPasteboard::pasteboardWithUniqueName();
        let concealed = super::super::super::CONCEALED_TYPE;
        let transient = super::super::super::TRANSIENT_TYPE;
        let _secret = publish_formats(
            &board,
            &["text/plain", super::super::super::CONCEALED_FORMAT],
        );
        let types = board_types(&board);
        assert!(types.iter().any(|kind| kind == concealed), "{types:?}");
        assert!(!types.iter().any(|kind| kind == transient), "{types:?}");

        let names_only =
            publish_formats(&board, &["text/plain", super::super::super::FILES_FORMAT]);
        let types = board_types(&board);
        assert!(types.iter().any(|kind| kind == transient), "{types:?}");
        assert!(!types.iter().any(|kind| kind == concealed), "{types:?}");
        // The marker is answered locally and carries no text.
        assert_eq!(
            board
                .dataForType(&NSString::from_str(transient))
                .map(|d| d.len()),
            Some(0)
        );
        // Files replace the names-only copy and drop the marker.
        let dir = folder(&["a"]);
        names_only
            .attach_files(names_only.revision(), dir.path())
            .unwrap();
        assert!(!board_types(&board).iter().any(|kind| kind == transient));
    }

    struct Folder(std::path::PathBuf);
    impl Folder {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn folder(names: &[&str]) -> Folder {
        let dir =
            std::env::temp_dir().join(format!("org.newport.test.attach.{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        for name in names {
            std::fs::write(dir.join(name), name).unwrap();
        }
        Folder(dir)
    }
    fn items(board: &NSPasteboard) -> Vec<(String, String)> {
        board
            .pasteboardItems()
            .unwrap()
            .iter()
            .map(|item| {
                let get = |kind: &str| {
                    item.stringForType(&NSString::from_str(kind))
                        .unwrap()
                        .to_string()
                };
                (get("public.file-url"), get("public.utf8-plain-text"))
            })
            .collect()
    }

    #[test]
    fn attaching_files_publishes_one_file_item_per_entry_and_advances_the_count() {
        let board = NSPasteboard::pasteboardWithUniqueName();
        let publication = published(&board);
        let dir = folder(&["b.txt", "a.txt", "c"]);
        let before = publication.revision();
        let revision = publication.attach_files(before, dir.path()).unwrap();
        assert_ne!(revision, before);
        assert_eq!(board.changeCount(), revision);
        assert_eq!(publication.revision(), revision);
        let items = items(&board);
        let names: Vec<_> = items.iter().map(|(_, name)| name.as_str()).collect();
        assert_eq!(names, ["a.txt", "b.txt", "c"]);
        for (url, name) in &items {
            assert!(url.starts_with("file://") && url.ends_with(name), "{url}");
        }
        // Ownership persists: withdraw clears while we still hold the board.
        assert!(publication.withdraw());
        assert!(board.pasteboardItems().is_none_or(|i| i.is_empty()));
    }

    #[test]
    fn attaching_after_a_foreign_copy_or_with_a_stale_count_leaves_the_board_alone() {
        let board = NSPasteboard::pasteboardWithUniqueName();
        let publication = published(&board);
        let dir = folder(&["a"]);
        assert!(
            publication
                .attach_files(publication.revision() - 1, dir.path())
                .is_none()
        );
        board.clearContents();
        board.setString_forType(&NSString::from_str("mine"), unsafe {
            NSPasteboardTypeString
        });
        let foreign = board.changeCount();
        assert!(
            publication
                .attach_files(publication.revision(), dir.path())
                .is_none()
        );
        assert!(publication.attach_files(foreign, dir.path()).is_none());
        assert_eq!(board.changeCount(), foreign);
        assert_eq!(
            board
                .stringForType(unsafe { NSPasteboardTypeString })
                .unwrap()
                .to_string(),
            "mine"
        );
        assert!(!publication.withdraw());
        assert_eq!(board.changeCount(), foreign);
    }

    #[test]
    fn empty_and_missing_folders_are_refused() {
        let board = NSPasteboard::pasteboardWithUniqueName();
        let publication = published(&board);
        let revision = publication.revision();
        assert!(
            publication
                .attach_files(revision, folder(&[]).path())
                .is_none()
        );
        assert!(
            publication
                .attach_files(revision, Path::new("/nonexistent/newport"))
                .is_none()
        );
        assert_eq!(board.changeCount(), revision);
    }
}
