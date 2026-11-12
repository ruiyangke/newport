//! A message-window owner keeps delayed rendering off the desktop UI thread.
//! Sources enqueue bounded asynchronous demand; Win32 callbacks wait only here.
use super::super::{MAX_BYTES, MAX_FORMATS, MAX_WAIT, Payload, Pending, Source};
use super::wire::{PublicationRejection, PublicationStage};
use super::{Files, clipboard_owner, clipboard_sequence};
use crate::{
    CONCEALED_FORMAT, FILES_FORMAT,
    windows_attachment::FileWrite,
    windows_commit::Commit,
    windows_limits::{MAX_IMAGE_BYTES, NativeBudget, publication_limit},
};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeSet, VecDeque},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
        mpsc,
    },
    time::Instant,
};
use tokio::sync::oneshot;
use windows_sys::Win32::{Foundation::*, System::DataExchange::*, UI::WindowsAndMessaging::*};

const SLOTS: usize = 8;
const TICK_MS: u32 = 20;
#[path = "attachment.rs"]
mod attachment;
#[path = "reader.rs"]
mod reader;
type Reply<T> = oneshot::Sender<Option<T>>;

struct Lease {
    current: AtomicBool,
    native: AtomicU32,
    owner: usize,
    running: Arc<AtomicBool>,
}
impl Lease {
    fn current(&self) -> bool {
        self.current.load(Ordering::Acquire)
            && self.running.load(Ordering::Acquire)
            && clipboard_sequence() == self.native.load(Ordering::Acquire)
            && clipboard_owner() == self.owner
    }
}
#[derive(Clone)]
pub struct Snapshot {
    pub revision: isize,
    pub formats: Vec<String>,
    pub origin: Option<u64>,
    lease: Arc<Lease>,
}
impl Snapshot {
    pub fn is_current(&self) -> bool {
        self.lease.current()
    }
    pub(super) fn identity(&self) -> (u32, usize) {
        (self.lease.native.load(Ordering::Acquire), self.lease.owner)
    }
}
#[derive(Default)]
struct Resources {
    slots: AtomicUsize,
    bytes: AtomicUsize,
}
pub struct Representation {
    bytes: Vec<u8>,
    lease: Arc<Lease>,
    resources: Arc<Resources>,
}
impl Payload for Representation {
    fn bytes(&self) -> Option<&[u8]> {
        self.lease.current().then_some(&self.bytes)
    }
}
impl Drop for Representation {
    fn drop(&mut self) {
        self.resources.slots.fetch_sub(1, Ordering::AcqRel);
        self.resources
            .bytes
            .fetch_sub(self.bytes.len(), Ordering::AcqRel);
    }
}
pub struct Publication {
    pub generation: u64,
    import: Arc<Import>,
    running: Arc<AtomicBool>,
}
impl Publication {
    pub fn is_current(&self) -> bool {
        self.running.load(Ordering::Acquire) && self.import.owns_baseline()
    }
    pub(super) fn owner(&self) -> usize {
        self.import.owner
    }
    pub(super) fn preparation(&self) -> Option<(isize, (u32, usize))> {
        match &*self
            .import
            .stage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            Stage::Preparing(snapshot) => Some((snapshot.revision, snapshot.identity())),
            Stage::Committing | Stage::Native { .. } => None,
        }
    }
}
impl Drop for Publication {
    fn drop(&mut self) {
        self.import.revoke();
    }
}
struct Proposal(Option<Arc<dyn Source>>);
impl Drop for Proposal {
    fn drop(&mut self) {
        if let Some(source) = &self.0 {
            source.publication_revoked();
        }
    }
}
struct Import {
    generation: u64,
    owner: usize,
    active: AtomicBool,
    source: Arc<dyn Source>,
    formats: Vec<(u32, String)>,
    native_budget: Mutex<NativeBudget>,
    stage: Mutex<Stage>,
    concealed: bool,
}
enum Stage {
    Preparing(Snapshot),
    // The clipboard lock prevents another writer while native ownership moves.
    Committing,
    Native { files: Option<Files> },
}
impl Import {
    fn current(&self) -> bool {
        self.active.load(Ordering::Acquire) && self.source.is_current()
    }
    fn revoke(&self) {
        if self.active.swap(false, Ordering::AcqRel) {
            self.source.publication_revoked();
        }
    }
    fn owns_baseline(&self) -> bool {
        if !self.current() {
            return false;
        }
        match &*self
            .stage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            Stage::Preparing(snapshot) => snapshot.is_current(),
            Stage::Committing => true,
            Stage::Native { .. } => clipboard_owner() == self.owner,
        }
    }
}
impl Drop for Import {
    fn drop(&mut self) {
        self.revoke();
    }
}
enum Action {
    Snapshot(Reply<Snapshot>),
    Read {
        revision: isize,
        format: String,
        reply: Reply<Representation>,
    },
    Publish {
        expected: isize,
        formats: Vec<String>,
        source: Arc<dyn Source>,
        trace: Arc<Mutex<PublicationRejection>>,
        reply: Reply<Publication>,
    },
    Attach {
        generation: u64,
        files: Files,
        commit: Arc<Commit>,
        reply: Reply<FileWrite>,
    },
}
struct Inner {
    running: Arc<AtomicBool>,
    worker: OnceLock<Option<mpsc::SyncSender<Action>>>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
    }
}
#[derive(Clone)]
pub struct Owner(Arc<Inner>);
impl Default for Owner {
    fn default() -> Self {
        Self::new()
    }
}
impl Owner {
    /// No native calls until the first polled authorized operation.
    pub fn new() -> Self {
        Self(Arc::new(Inner {
            running: Arc::new(AtomicBool::new(true)),
            worker: OnceLock::new(),
        }))
    }
    pub fn close(&self) {
        self.0.running.store(false, Ordering::Release);
    }
    fn enqueue(&self, action: Action) -> bool {
        if !self.0.running.load(Ordering::Acquire) {
            return false;
        }
        let sender = self.0.worker.get_or_init(|| {
            if !self.0.running.load(Ordering::Acquire) {
                return None;
            }
            let (send, receive) = mpsc::sync_channel(SLOTS);
            let running = self.0.running.clone();
            std::thread::Builder::new()
                .name("clipboard-owner".into())
                .spawn(move || {
                    run(running, receive);
                })
                .ok()
                .map(|_| send)
        });
        sender
            .as_ref()
            .is_some_and(|sender| sender.try_send(action).is_ok())
    }
    pub async fn snapshot(&self) -> Option<Snapshot> {
        let (reply, result) = oneshot::channel();
        self.enqueue(Action::Snapshot(reply));
        tokio::time::timeout(MAX_WAIT, result).await.ok()?.ok()?
    }
    pub async fn read(&self, revision: isize, format: String) -> Option<Representation> {
        let (reply, result) = oneshot::channel();
        self.enqueue(Action::Read {
            revision,
            format,
            reply,
        });
        tokio::time::timeout(MAX_WAIT, result).await.ok()?.ok()?
    }
    pub(super) fn publish(
        &self,
        expected: isize,
        formats: Vec<String>,
        source: Arc<dyn Source>,
    ) -> impl std::future::Future<Output = (Option<Publication>, PublicationRejection)> + Send + use<>
    {
        let owner = self.clone();
        let claimed = source.begin_publication();
        let mut proposal = Proposal(claimed.then(|| source.clone()));
        let trace = Arc::new(Mutex::new(PublicationRejection {
            stage: PublicationStage::Source,
            expected: super::wire::revision(expected),
            observed: None,
            native_error: None,
        }));
        async move {
            let publication = async {
                if !claimed {
                    return None;
                }
                trace
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .stage = PublicationStage::Queue;
                let (reply, result) = oneshot::channel();
                owner.enqueue(Action::Publish {
                    expected,
                    formats,
                    source,
                    trace: trace.clone(),
                    reply,
                });
                let publication = tokio::time::timeout(MAX_WAIT, result).await.ok()?.ok()??;
                proposal.0.take();
                Some(publication)
            }
            .await;
            let rejection = *trace
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (publication, rejection)
        }
    }
    /// Only validated cache capabilities enter the native owner. Dropping the
    /// future cancels an attachment until its final native commit claim wins.
    pub fn attach_files(
        &self,
        generation: u64,
        files: Files,
    ) -> impl std::future::Future<Output = FileWrite> + Send + use<> {
        let owner = self.clone();
        let commit = Arc::new(Commit::default());
        struct Cancel(Arc<Commit>);
        impl Drop for Cancel {
            fn drop(&mut self) {
                self.0.cancel();
            }
        }
        let cancel = Cancel(commit.clone());
        async move {
            let _cancel = cancel;
            let (reply, result) = oneshot::channel();
            if !owner.enqueue(Action::Attach {
                generation,
                files,
                commit,
                reply,
            }) {
                return FileWrite::Unchanged;
            }
            match tokio::time::timeout(MAX_WAIT, result).await {
                Ok(Ok(Some(outcome))) => outcome,
                _ => FileWrite::Uncertain,
            }
        }
    }
}
#[derive(Default)]
struct State {
    revision: isize,
    generation: u64,
    snapshot: Option<Snapshot>,
    import: Option<Arc<Import>>,
    preparation: Option<Arc<Import>>,
}
struct Privacy {
    native: u32,
    owner: usize,
    deadline: Instant,
    receive: oneshot::Receiver<Option<bool>>,
    concealed: Option<bool>,
}
struct Window {
    running: Arc<AtomicBool>,
    actions: mpsc::Receiver<Action>,
    resources: Arc<Resources>,
    state: RefCell<State>,
    deferred: RefCell<VecDeque<Action>>,
    rendering: Cell<bool>,
    privacy: RefCell<Option<Privacy>>,
}
impl Window {
    fn invalidate(&self) {
        let mut state = self.state.borrow_mut();
        if let Some(snapshot) = state.snapshot.take() {
            snapshot.lease.current.store(false, Ordering::Release);
        }
    }
    fn observe(&self) -> Option<Snapshot> {
        self.retire_preparation();
        let existing = self.state.borrow().snapshot.clone();
        if let Some(snapshot) = existing.filter(Snapshot::is_current) {
            return Some(snapshot);
        }
        let import = self.state.borrow().import.clone();
        if let Some(import) =
            import.filter(|import| import.current() && clipboard_owner() == import.owner)
        {
            // Windows may synthesize another bitmap/text format without changing the owner.
            self.native_changed();
            if let Some(snapshot) = self
                .state
                .borrow()
                .snapshot
                .clone()
                .filter(|snapshot| snapshot.origin == Some(import.generation))
            {
                return Some(snapshot);
            }
        }
        self.invalidate();
        let (native, mut formats) = crate::windows::offer(None)?;
        if formats.iter().any(|format| format == CONCEALED_FORMAT)
            && !self.privacy(native as u32)?
        {
            formats.retain(|format| format != CONCEALED_FORMAT);
        }
        if clipboard_sequence() != native as u32 {
            return None;
        }
        let import = self.state.borrow_mut().import.take();
        if let Some(import) = import {
            import.revoke();
        }
        self.replace_snapshot(native as u32, formats, None)
    }
    fn retire_preparation(&self) {
        let obsolete = self
            .state
            .borrow()
            .preparation
            .as_ref()
            .is_some_and(|preparation| !preparation.owns_baseline());
        if obsolete && let Some(preparation) = self.state.borrow_mut().preparation.take() {
            preparation.revoke();
        }
    }
    fn privacy(&self, native: u32) -> Option<bool> {
        let owner = clipboard_owner();
        let mut pending = self.privacy.borrow_mut();
        if pending
            .as_ref()
            .is_none_or(|pending| pending.native != native || pending.owner != owner)
        {
            *pending = Some(Privacy {
                native,
                owner,
                deadline: Instant::now() + MAX_WAIT,
                receive: reader::privacy(native, owner, self.running.clone()),
                concealed: None,
            });
        }
        let pending = pending.as_mut()?;
        if pending.concealed.is_none() {
            match pending.receive.try_recv() {
                Ok(value) => pending.concealed = Some(value.unwrap_or(true)),
                Err(oneshot::error::TryRecvError::Closed) => pending.concealed = Some(true),
                Err(oneshot::error::TryRecvError::Empty) if Instant::now() >= pending.deadline => {
                    pending.concealed = Some(true)
                }
                Err(oneshot::error::TryRecvError::Empty) => return None,
            }
        }
        if clipboard_sequence() != native || clipboard_owner() != owner {
            return None;
        }
        pending.concealed
    }
    fn replace_snapshot(
        &self,
        native: u32,
        formats: Vec<String>,
        origin: Option<u64>,
    ) -> Option<Snapshot> {
        self.invalidate();
        let mut state = self.state.borrow_mut();
        state.revision = state.revision.checked_add(1)?;
        let snapshot = Snapshot {
            revision: state.revision,
            formats,
            origin,
            lease: Arc::new(Lease {
                current: AtomicBool::new(true),
                native: AtomicU32::new(native),
                owner: clipboard_owner(),
                running: self.running.clone(),
            }),
        };
        state.snapshot = Some(snapshot.clone());
        Some(snapshot)
    }
    fn native_changed(&self) {
        if let Some(snapshot) = &self.state.borrow().snapshot {
            snapshot
                .lease
                .native
                .store(clipboard_sequence(), Ordering::Release);
        }
    }
    fn lost(&self) {
        self.invalidate();
        if let Some(preparation) = self.state.borrow_mut().preparation.take() {
            preparation.revoke();
        }
        let import = self.state.borrow_mut().import.take();
        if let Some(import) = import {
            import.revoke();
        }
    }
    fn withdraw(&self, hwnd: HWND) {
        self.retire_preparation();
        let import = self.state.borrow().import.clone();
        let Some(import) = import.filter(|import| !import.current()) else {
            return;
        };
        if clipboard_owner() != hwnd as usize {
            self.lost();
            return;
        }
        let Ok(_clipboard) = clipboard_win::Clipboard::new_for(hwnd as _) else {
            return;
        };
        if clipboard_owner() != hwnd as usize {
            return;
        }
        // Preserve imported provenance after revocation so it is never echoed as a local copy.
        if clipboard_win::raw::empty().is_ok() {
            self.state.borrow_mut().import = None;
            self.replace_snapshot(clipboard_sequence(), Vec::new(), Some(import.generation));
        }
    }
    fn action(&self, hwnd: HWND, action: Action) {
        match action {
            Action::Snapshot(reply) => {
                if !reply.is_closed() {
                    let _ = reply.send(self.observe());
                }
            }
            Action::Read {
                revision,
                format,
                reply,
            } => {
                if reply.is_closed() {
                    return;
                }
                let Some(snapshot) = self.observe().filter(|s| {
                    s.revision == revision && s.origin.is_none() && s.formats.contains(&format)
                }) else {
                    return;
                };
                reader::read(snapshot, format, self.resources.clone(), reply);
            }
            Action::Publish {
                expected,
                formats,
                source,
                trace,
                reply,
            } => {
                {
                    let mut trace = trace
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    trace.stage = PublicationStage::Source;
                    trace.native_error = None;
                }
                if reply.is_closed() || !source.is_current() {
                    return;
                }
                trace
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .stage = PublicationStage::Observe;
                let Some(snapshot) = self.observe() else {
                    self.defer(Action::Publish {
                        expected,
                        formats,
                        source,
                        trace,
                        reply,
                    });
                    return;
                };
                {
                    let mut trace = trace
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    trace.stage = PublicationStage::Baseline;
                    trace.observed = Some(super::wire::revision(snapshot.revision));
                }
                if snapshot.revision != expected {
                    return;
                }
                trace
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .stage = PublicationStage::Formats;
                let Some(native_formats) = native_formats(&formats) else {
                    return;
                };
                let generation = {
                    trace
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .stage = PublicationStage::Generation;
                    let mut state = self.state.borrow_mut();
                    let Some(next) = state.generation.checked_add(1) else {
                        return;
                    };
                    state.generation = next;
                    next
                };
                if native_formats.is_empty() {
                    trace
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .stage = PublicationStage::Admission;
                    if !snapshot.is_current() || !source.preparation_confirmed() {
                        return;
                    }
                    let preparation = Arc::new(Import {
                        generation,
                        owner: hwnd as usize,
                        active: AtomicBool::new(true),
                        source,
                        formats: Vec::new(),
                        native_budget: Mutex::default(),
                        concealed: formats.iter().any(|format| format == CONCEALED_FORMAT),
                        stage: Mutex::new(Stage::Preparing(snapshot)),
                    });
                    let previous = self
                        .state
                        .borrow_mut()
                        .preparation
                        .replace(preparation.clone());
                    if let Some(previous) = previous {
                        previous.revoke();
                    }
                    let _ = reply.send(Some(Publication {
                        generation,
                        import: preparation,
                        running: self.running.clone(),
                    }));
                    return;
                }
                trace
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .stage = PublicationStage::Open;
                let _clipboard = match clipboard_win::Clipboard::new_for(hwnd as _) {
                    Ok(clipboard) => clipboard,
                    Err(error) => {
                        trace
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .native_error = Some(error.raw_code() as u32);
                        self.defer(Action::Publish {
                            expected,
                            formats,
                            source,
                            trace,
                            reply,
                        });
                        return;
                    }
                };
                let import = Arc::new(Import {
                    generation,
                    owner: hwnd as usize,
                    active: AtomicBool::new(true),
                    source,
                    formats: native_formats,
                    native_budget: Mutex::new(NativeBudget::default()),
                    concealed: formats.iter().any(|format| format == CONCEALED_FORMAT),
                    stage: Mutex::new(Stage::Native { files: None }),
                });
                trace
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .stage = PublicationStage::Validate;
                if !snapshot.is_current() || reply.is_closed() || !import.current() {
                    return;
                }
                // Admission is claimed by the parent before a foreign callback can block.
                trace
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .stage = PublicationStage::Admission;
                if !import.source.publication_confirmed() {
                    return;
                }
                trace
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .stage = PublicationStage::Empty;
                if let Err(error) = clipboard_win::raw::empty() {
                    trace
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .native_error = Some(error.raw_code() as u32);
                    return;
                }
                // SetClipboardData(NULL) returns NULL even when the delayed format was accepted.
                for (native, _) in &import.formats {
                    // SAFETY: The clipboard is open; a null handle declares delayed rendering.
                    let error = unsafe {
                        SetLastError(0);
                        SetClipboardData(*native, std::ptr::null_mut());
                        GetLastError()
                    };
                    if error != 0 {
                        trace
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .native_error = Some(error);
                    }
                }
                let formats: Vec<_> = formats
                    .into_iter()
                    .filter(|format| {
                        format == CONCEALED_FORMAT
                            || import.formats.iter().any(|(_, mime)| mime == format)
                    })
                    .collect();
                let complete = import
                    .formats
                    .iter()
                    .all(|(format, _)| clipboard_win::is_format_avail(*format));
                trace
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .stage = PublicationStage::DelayedFormats;
                let concealment = if formats.contains(&CONCEALED_FORMAT.to_owned()) {
                    conceal()
                } else {
                    Ok(())
                };
                if let (true, Err(error)) = (complete, concealment) {
                    let mut trace = trace
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    trace.stage = PublicationStage::Concealment;
                    trace.native_error = Some(error);
                } else if complete && concealment.is_ok() && reply.is_closed() {
                    trace
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .stage = PublicationStage::Reply;
                }
                if !complete || concealment.is_err() || reply.is_closed() {
                    let _ = clipboard_win::raw::empty();
                    self.replace_snapshot(clipboard_sequence(), Vec::new(), Some(generation));
                    return;
                }
                self.state.borrow_mut().import = Some(import.clone());
                trace
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .stage = PublicationStage::Reply;
                self.replace_snapshot(clipboard_sequence(), formats, Some(generation));
                // A newer owner may replace the copy as soon as the native lock is released.
                drop(_clipboard);
                let _ = reply.send(Some(Publication {
                    generation,
                    import,
                    running: self.running.clone(),
                }));
            }
            Action::Attach {
                generation,
                files,
                commit,
                reply,
            } => {
                self.attach(hwnd, generation, files, commit, reply);
            }
        }
    }
    fn defer(&self, action: Action) {
        let mut deferred = self.deferred.borrow_mut();
        if deferred.len() < SLOTS {
            deferred.push_back(action);
        }
    }
    fn render(&self, hwnd: HWND, format: u32, deadline: Instant) {
        if self.rendering.replace(true) {
            return;
        }
        struct Rendering<'a>(&'a Cell<bool>);
        impl Drop for Rendering<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        let _rendering = Rendering(&self.rendering);
        let import = self.state.borrow().import.clone();
        let Some(import) = import.filter(|i| i.current()) else {
            return;
        };
        let Some((_, mime)) = import.formats.iter().find(|(native, _)| *native == format) else {
            return;
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return;
        }
        let (pending, reply) = Pending::new(remaining);
        import.source.request(mime, reply);
        let current = || {
            self.running.load(Ordering::Acquire)
                && import.current()
                && clipboard_owner() == hwnd as usize
        };
        let Some(payload) = pending.wait(current) else {
            return;
        };
        let Some(bytes) = payload.bytes().filter(|b| b.len() <= MAX_BYTES) else {
            return;
        };
        let Some(encoded) = encode(format, mime, bytes) else {
            return;
        };
        if !import
            .native_budget
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .permits(format, encoded.len(), publication_limit(MAX_BYTES))
        {
            return;
        }
        // Keep the source's payload fence alive until ownership transfers to Windows.
        if current()
            && payload.bytes().is_some()
            && clipboard_win::raw::set_without_clear(format, &encoded).is_ok()
        {
            import
                .native_budget
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .record(format, encoded.len());
            self.native_changed();
        }
    }
}
fn native_formats(formats: &[String]) -> Option<Vec<(u32, String)>> {
    if formats.is_empty() || formats.len() > MAX_FORMATS {
        return None;
    }
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for mime in formats {
        if !seen.insert(mime) || mime.len() > 256 || mime.chars().any(char::is_control) {
            return None;
        }
        let native = match mime.as_str() {
            "text/plain" => clipboard_win::formats::CF_UNICODETEXT,
            "text/html" => clipboard_win::register_format("HTML Format")?.get(),
            "image/png" => clipboard_win::register_format("PNG")?.get(),
            "text/rtf" => clipboard_win::register_format("Rich Text Format")?.get(),
            "image/jpeg" => clipboard_win::register_format("JFIF")?.get(),
            "image/tiff" => clipboard_win::formats::CF_TIFF,
            "application/pdf" | "image/svg+xml" => clipboard_win::register_format(mime)?.get(),
            CONCEALED_FORMAT | FILES_FORMAT => continue,
            _ => continue,
        };
        result.push((native, mime.clone()));
        if mime == "image/png" {
            result.push((clipboard_win::formats::CF_DIBV5, mime.clone()));
        }
    }
    (!result.is_empty() || formats.iter().any(|format| format == FILES_FORMAT)).then_some(result)
}
fn encode(native: u32, mime: &str, bytes: &[u8]) -> Option<Vec<u8>> {
    if native == clipboard_win::formats::CF_DIBV5 {
        return crate::windows_bitmap::png_to_dib(bytes, MAX_BYTES, MAX_IMAGE_BYTES);
    }
    match mime {
        "text/plain" => crate::windows_formats::unicode_text(bytes),
        "text/html" => crate::windows_formats::html_document(bytes),
        "image/png" | "text/rtf" | "image/jpeg" | "image/tiff" | "application/pdf"
        | "image/svg+xml" => Some(bytes.to_vec()),
        _ => None,
    }
}
fn conceal() -> Result<(), u32> {
    for name in [
        "ExcludeClipboardContentFromMonitorProcessing",
        "CanIncludeInClipboardHistory",
        "CanUploadToCloudClipboard",
    ] {
        let format = clipboard_win::register_format(name).ok_or_else(|| {
            // SAFETY: Reads the calling thread's last-error value.
            unsafe { GetLastError() }
        })?;
        clipboard_win::raw::set_without_clear(format.get(), &[0, 0, 0, 0])
            .map_err(|error| error.raw_code() as u32)?;
    }
    Ok(())
}
unsafe extern "system" fn procedure(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if message == WM_NCCREATE {
        // SAFETY: WM_NCCREATE's lParam points to the CREATESTRUCTW for this call;
        // lpCreateParams is the `Window` that `run` keeps alive until the window is destroyed.
        let create = unsafe { &*(l as *const CREATESTRUCTW) };
        // SAFETY: hwnd is the window being created on this thread.
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
    }
    // SAFETY: hwnd belongs to this thread's window procedure.
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Window;
    if !pointer.is_null() {
        // SAFETY: The pointer was stored from `run`'s boxed `Window`, which outlives the
        // window; all access is on this thread and only through shared references.
        let window = unsafe { &*pointer };
        match message {
            WM_RENDERFORMAT => {
                window.render(hwnd, w as u32, Instant::now() + MAX_WAIT);
                return 0;
            }
            WM_RENDERALLFORMATS => {
                if let Ok(_clipboard) = clipboard_win::Clipboard::new_for(hwnd as _)
                    && clipboard_owner() == hwnd as usize
                {
                    let import = window.state.borrow().import.clone();
                    let deadline = Instant::now() + MAX_WAIT;
                    if let Some(import) = import {
                        for (format, _) in &import.formats {
                            window.render(hwnd, *format, deadline);
                        }
                    }
                }
                return 0;
            }
            WM_DESTROYCLIPBOARD => {
                window.lost();
                return 0;
            }
            WM_TIMER => {
                if !window.running.load(Ordering::Acquire) {
                    let import = window.state.borrow().import.clone();
                    if let Some(import) = import {
                        import.revoke();
                    }
                    window.withdraw(hwnd);
                    window.lost();
                    // SAFETY: hwnd is this thread's own window.
                    unsafe {
                        DestroyWindow(hwnd);
                        PostQuitMessage(0);
                    }
                    return 0;
                }
                window.withdraw(hwnd);
                let deferred = std::mem::take(&mut *window.deferred.borrow_mut());
                for action in deferred {
                    window.action(hwnd, action);
                }
                for _ in 0..SLOTS {
                    let Ok(action) = window.actions.try_recv() else {
                        break;
                    };
                    window.action(hwnd, action);
                }
                return 0;
            }
            WM_CLIPBOARDUPDATE => {
                let _ = window.observe();
                return 0;
            }
            _ => {}
        }
    }
    // SAFETY: Forwards the window procedure's own arguments unchanged.
    unsafe { DefWindowProcW(hwnd, message, w, l) }
}
fn run(running: Arc<AtomicBool>, actions: mpsc::Receiver<Action>) {
    let window = Box::new(Window {
        running: running.clone(),
        actions,
        resources: Arc::default(),
        state: RefCell::default(),
        deferred: RefCell::default(),
        rendering: Cell::new(false),
        privacy: RefCell::default(),
    });
    let name: Vec<u16> = format!("NewportClipboardOwner-{}\0", uuid::Uuid::new_v4())
        .encode_utf16()
        .collect();
    // SAFETY: `name` is NUL-terminated and outlives the class and window. `window` is
    // boxed and outlives the window, which is destroyed before `run` returns, and the
    // message loop runs on this thread, the only one that touches it.
    unsafe {
        let class = WNDCLASSW {
            lpfnWndProc: Some(procedure),
            lpszClassName: name.as_ptr(),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            running.store(false, Ordering::Release);
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
            (&*window as *const Window).cast(),
        );
        if !hwnd.is_null() {
            if AddClipboardFormatListener(hwnd) != 0 && SetTimer(hwnd, 1, TICK_MS, None) != 0 {
                let mut message = MSG::default();
                while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            running.store(false, Ordering::Release);
            window.lost();
            RemoveClipboardFormatListener(hwnd);
            DestroyWindow(hwnd);
        }
        UnregisterClassW(name.as_ptr(), std::ptr::null_mut());
    }
    running.store(false, Ordering::Release);
}

#[cfg(test)]
pub(super) fn fail_file_data_for_fixture() {
    attachment::FAIL_FILE_DATA.store(true, Ordering::Release);
}

#[cfg(test)]
type FileTransitionHook = Box<dyn FnOnce(u64) -> bool + Send>;
#[cfg(test)]
static FILE_TRANSITION_HOOK: Mutex<Option<FileTransitionHook>> = Mutex::new(None);

#[cfg(test)]
pub(super) fn set_file_transition_hook_for_fixture(hook: FileTransitionHook) {
    if attachment::FAIL_FILE_DATA.load(Ordering::Acquire) {
        *FILE_TRANSITION_HOOK.lock().unwrap() = Some(hook);
    }
}

#[cfg(test)]
fn file_transition_for_fixture(generation: u64) -> bool {
    let hook = FILE_TRANSITION_HOOK.lock().unwrap().take();
    hook.is_some_and(|hook| hook(generation))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestSource {
        current: AtomicBool,
        revoked: AtomicUsize,
    }
    impl Source for TestSource {
        fn is_current(&self) -> bool {
            self.current.load(Ordering::Acquire)
        }
        fn request(&self, _: &str, _: crate::provider::Reply) {}
        fn publication_revoked(&self) {
            self.current.store(false, Ordering::Release);
            self.revoked.fetch_add(1, Ordering::AcqRel);
        }
    }

    fn committing_publication() -> (Publication, Arc<TestSource>) {
        let source = Arc::new(TestSource {
            current: AtomicBool::new(true),
            revoked: AtomicUsize::new(0),
        });
        let publication = Publication {
            generation: 1,
            // Committing must not inspect any ambient clipboard identity.
            import: Arc::new(Import {
                generation: 1,
                owner: 0,
                active: AtomicBool::new(true),
                source: source.clone(),
                formats: Vec::new(),
                native_budget: Mutex::default(),
                stage: Mutex::new(Stage::Committing),
                concealed: false,
            }),
            running: Arc::new(AtomicBool::new(true)),
        };
        (publication, source)
    }

    #[test]
    fn claimed_native_transition_survives_identity_retirement_without_hiding_revocation() {
        let (publication, source) = committing_publication();
        assert!(publication.is_current());
        assert!(publication.preparation().is_none());
        source.current.store(false, Ordering::Release);
        assert!(
            !publication.is_current(),
            "external authority revocation remains immediate"
        );
        assert_eq!(source.revoked.load(Ordering::Acquire), 0);
        drop(publication);
        assert_eq!(source.revoked.load(Ordering::Acquire), 1);
    }

    #[test]
    fn dropping_a_claimed_publication_or_stopping_owner_revokes_without_waiting_for_native_work() {
        let (publication, source) = committing_publication();
        drop(publication);
        assert!(!source.is_current());
        assert_eq!(source.revoked.load(Ordering::Acquire), 1);

        let (publication, source) = committing_publication();
        publication.running.store(false, Ordering::Release);
        assert!(!publication.is_current());
        drop(publication);
        assert!(!source.is_current());
    }
}
