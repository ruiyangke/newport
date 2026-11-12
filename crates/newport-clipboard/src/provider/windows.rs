//! Windows clipboard operations run in one explicitly configured child process.
//! Only nonblocking native identity checks and source policy remain in the host.
use super::{MAX_BYTES, MAX_WAIT, Payload, Source};
use crate::windows_attachment::{Attachment, FileWrite, Stage as AttachmentStage};
use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tokio::sync::{mpsc, oneshot, watch};
use windows_sys::Win32::System::DataExchange::{GetClipboardOwner, GetClipboardSequenceNumber};
mod native;
pub mod process;
mod session;
mod wire;
mod worker;
const SLOTS: usize = 8;

fn clipboard_owner() -> usize {
    // SAFETY: Takes no arguments and only reads the calling thread's desktop state.
    unsafe { GetClipboardOwner() as usize }
}
fn clipboard_sequence() -> u32 {
    // SAFETY: Takes no arguments and has no preconditions.
    unsafe { GetClipboardSequenceNumber() }
}
type Reply<T> = oneshot::Sender<Option<T>>;

/// A validated file copy whose paths stay valid while it is retained. The
/// owner keeps it until the native clipboard no longer exposes those paths.
pub trait RetainedFiles: Send + Sync + 'static {
    fn folder(&self) -> &Path;
    fn paths(&self) -> &[PathBuf];
}
pub type Files = Arc<dyn RetainedFiles>;
/// Reopens a copy by folder in the helper, which holds a lease of its own.
pub type OpenFiles = fn(&Path) -> io::Result<Files>;

#[derive(Clone)]
pub struct WorkerExecutable {
    path: PathBuf,
    arguments: Vec<String>,
}
impl WorkerExecutable {
    pub fn production(path: PathBuf) -> Self {
        Self {
            path,
            arguments: vec![process::ARGUMENT.into()],
        }
    }
    #[cfg(test)]
    fn fixture() -> Self {
        Self {
            path: std::env::current_exe().unwrap(),
            arguments: vec![
                "--exact".into(),
                "provider::windows::tests::clipboard_worker_entry".into(),
                "--ignored".into(),
                "--nocapture".into(),
            ],
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Identity {
    sequence: u32,
    owner: u64,
}
impl Identity {
    fn current() -> Option<Self> {
        let sequence = clipboard_sequence();
        let owner = clipboard_owner() as u64;
        let after = clipboard_sequence();
        let owner_after = clipboard_owner() as u64;
        (sequence != 0 && sequence == after && owner == owner_after)
            .then_some(Self { sequence, owner })
    }
    fn matches(self) -> bool {
        Some(self) == Self::current()
    }
}
// A killed child may have changed the clipboard without sending its receipt.
// Keep its final contents ineligible across new Owner instances until a new copy.
static UNCERTAIN: Mutex<Option<Identity>> = Mutex::new(None);
fn eligible(identity: Identity) -> bool {
    let mut barrier = UNCERTAIN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(old) = *barrier {
        // Owner destruction and synthesized formats are not evidence of a new
        // copy. Require a live different owner and a changed native sequence.
        if !new_copy_after(old, identity) {
            return false;
        }
        *barrier = None;
    }
    true
}
fn new_copy_after(previous: Identity, observed: Identity) -> bool {
    observed.owner != 0
        && observed.owner != previous.owner
        && observed.sequence != previous.sequence
}
struct Lease {
    identity: Identity,
    valid: AtomicBool,
    running: Arc<AtomicBool>,
}
impl Lease {
    fn current(&self) -> bool {
        self.valid.load(Ordering::Acquire)
            && self.running.load(Ordering::Acquire)
            && self.identity.matches()
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
struct Import {
    active: AtomicBool,
    released: AtomicBool,
    notified: AtomicBool,
    source: Arc<dyn Source>,
    owner: AtomicUsize,
    attachment: Attachment<(isize, Identity)>,
    files: Mutex<Option<Files>>,
}
impl Import {
    fn current(&self) -> bool {
        self.active.load(Ordering::Acquire)
            && self.source.is_current()
            && match self.attachment.stage() {
                AttachmentStage::Preparing((_, identity)) => identity.matches(),
                AttachmentStage::Committing(Some((_, identity))) => {
                    identity.matches() || self.owns_native()
                }
                AttachmentStage::Committing(None) | AttachmentStage::Native => self.owns_native(),
                AttachmentStage::Canceled | AttachmentStage::Uncertain => false,
            }
    }
    fn owns_native(&self) -> bool {
        let owner = self.owner.load(Ordering::Acquire);
        owner != 0 && clipboard_owner() == owner
    }
    fn revoke(&self) {
        if self.active.swap(false, Ordering::AcqRel) {
            self.source.publication_revoked();
        }
    }
}
pub struct Publication {
    pub generation: u64,
    import: Arc<Import>,
    running: Arc<AtomicBool>,
}
impl Publication {
    pub fn is_current(&self) -> bool {
        self.running.load(Ordering::Acquire) && self.import.current()
    }
    pub fn preparing_revision(&self) -> Option<isize> {
        match self.import.attachment.stage() {
            AttachmentStage::Preparing((revision, _)) => Some(revision),
            _ => None,
        }
    }
    pub fn cancel_preparation(&self) -> bool {
        if !self.import.attachment.cancel_preparation() {
            return false;
        }
        self.import.revoke();
        true
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
struct Attempt(Mutex<(bool, bool)>);
impl Attempt {
    fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new((false, false))))
    }
    fn admit(&self, confirm: impl FnOnce() -> bool) -> bool {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.0 || !confirm() {
            return false;
        }
        state.1 = true;
        true
    }
    fn canceled(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .0
    }
    #[cfg(test)]
    fn claimed(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .1
    }
}
struct Cancel(Arc<Attempt>);
impl Drop for Cancel {
    fn drop(&mut self) {
        let mut state = self
            .0
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.1 {
            state.0 = true;
        }
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
        attempt: Arc<Attempt>,
        reply: Reply<Publication>,
    },
    Attach {
        generation: u64,
        files: Files,
        attempt: Arc<Attempt>,
        reply: Reply<isize>,
    },
}
struct Inner {
    executable: WorkerExecutable,
    files: Option<OpenFiles>,
    running: Arc<AtomicBool>,
    sender: OnceLock<Option<mpsc::Sender<Action>>>,
    completion: watch::Sender<Option<Result<(), String>>>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
    }
}
#[derive(Clone)]
pub struct Owner(Arc<Inner>);
impl Owner {
    pub fn with_worker(executable: WorkerExecutable) -> Self {
        Self::new(executable, None)
    }
    /// Also attaches downloaded folders, which `open` retains while the
    /// clipboard offers them.
    pub fn with_files(executable: WorkerExecutable, open: OpenFiles) -> Self {
        Self::new(executable, Some(open))
    }
    fn new(executable: WorkerExecutable, files: Option<OpenFiles>) -> Self {
        Self(Arc::new(Inner {
            executable,
            files,
            running: Arc::new(AtomicBool::new(true)),
            sender: OnceLock::new(),
            completion: watch::channel(Some(Ok(()))).0,
        }))
    }
    pub fn close(&self) {
        self.0.running.store(false, Ordering::Release);
    }
    pub(crate) fn files(&self) -> Option<OpenFiles> {
        self.0.files
    }
    pub async fn shutdown(&self) -> Result<(), String> {
        self.close();
        self.0.sender.get_or_init(|| None);
        let mut completion = self.0.completion.subscribe();
        loop {
            if let Some(result) = completion.borrow_and_update().clone() {
                return result;
            }
            completion
                .changed()
                .await
                .map_err(|_| "Clipboard supervisor exited without cleanup receipt".to_owned())?;
        }
    }
    fn enqueue(&self, action: Action) {
        if !self.0.running.load(Ordering::Acquire) {
            return;
        }
        let sender = self.0.sender.get_or_init(|| {
            if !self.0.running.load(Ordering::Acquire) {
                return None;
            }
            let (send, receive) = mpsc::channel(SLOTS);
            let running = self.0.running.clone();
            let executable = self.0.executable.clone();
            let completion = self.0.completion.clone();
            completion.send_replace(None);
            let started = std::thread::Builder::new()
                .name("clipboard-supervisor".into())
                .spawn(move || {
                    let result = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| e.to_string())
                        .and_then(|runtime| {
                            runtime.block_on(session::run(executable, running.clone(), receive))
                        });
                    running.store(false, Ordering::Release);
                    completion.send_replace(Some(result));
                });
            match started {
                Ok(_) => Some(send),
                Err(error) => {
                    self.0.completion.send_replace(Some(Err(error.to_string())));
                    None
                }
            }
        });
        if let Some(sender) = sender {
            let _ = sender.try_send(action);
        }
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
    pub fn publish(
        &self,
        expected: isize,
        formats: Vec<String>,
        source: Arc<dyn Source>,
    ) -> impl std::future::Future<Output = Option<Publication>> + Send + use<> {
        let owner = self.clone();
        let claimed = source.begin_publication();
        let mut proposal = Proposal(claimed.then(|| source.clone()));
        let attempt = Attempt::new();
        let cancel = Cancel(attempt.clone());
        async move {
            let _cancel = cancel;
            if !claimed {
                return None;
            }
            let (reply, result) = oneshot::channel();
            owner.enqueue(Action::Publish {
                expected,
                formats,
                source,
                attempt,
                reply,
            });
            let publication = tokio::time::timeout(MAX_WAIT, result).await.ok()?.ok()??;
            proposal.0.take();
            Some(publication)
        }
    }
    pub fn attach_files(
        &self,
        generation: u64,
        files: Files,
    ) -> impl std::future::Future<Output = Option<isize>> + Send + use<> {
        let owner = self.clone();
        let attempt = Attempt::new();
        let cancel = Cancel(attempt.clone());
        async move {
            let _cancel = cancel;
            let (reply, result) = oneshot::channel();
            owner.enqueue(Action::Attach {
                generation,
                files,
                attempt,
                reply,
            });
            tokio::time::timeout(MAX_WAIT, result).await.ok()?.ok()?
        }
    }
}
#[cfg(test)]
mod tests;
