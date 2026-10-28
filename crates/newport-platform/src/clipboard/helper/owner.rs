//! One background owner for local observation/reads and remote publication.
//! Construction and handle destruction do not perform native or socket I/O.
//! The helper starts on the first authorized operation; permission/focus policy
//! belongs to the caller. A lost helper is replaced only on a subsequent call,
//! never by replaying an old read or publication.
mod session;
use super::{Board, Client};
use crate::clipboard::provider::{MAX_BYTES, MAX_WAIT, Source};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::time::Duration;
use tokio::sync::{oneshot, watch};

const SLOTS: usize = 8;
const TICK: Duration = Duration::from_millis(10);
type Reply<T> = oneshot::Sender<Option<T>>;

#[derive(Default)]
struct Continuity {
    revision: isize,
    generation: u64,
    // An imported native cache can survive a helper crash. Keep its provenance
    // until native ownership changes; reconnect must not call it a local copy.
    origin: Option<(isize, u64)>,
}

struct Lease {
    current: AtomicBool,
    native: isize,
    running: Arc<AtomicBool>,
}
impl Lease {
    fn current(&self) -> bool {
        self.current.load(Ordering::Acquire) && self.running.load(Ordering::Acquire)
    }
}

/// Logical revisions never repeat during an Owner's lifetime, including after
/// a helper restart with the same native change count. Keep this object while
/// using an offer; `is_current` also fences disconnects and publication changes.
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
    bytes: AtomicUsize,
    slots: AtomicUsize,
}

/// A local representation retains its source fence and memory/slot reservation
/// after delivery. Holding completed results cannot bypass the aggregate limit.
pub struct Representation {
    data: Vec<u8>,
    lease: Arc<Lease>,
    resources: Arc<Resources>,
}
impl Representation {
    fn reserve(lease: Arc<Lease>, resources: &Arc<Resources>) -> Option<Self> {
        resources
            .slots
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                (used < SLOTS).then_some(used + 1)
            })
            .ok()?;
        Some(Self {
            data: Vec::new(),
            lease,
            resources: resources.clone(),
        })
    }
    fn append(&mut self, bytes: &[u8]) -> bool {
        if self
            .resources
            .bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes.len())
                    .filter(|used| *used <= MAX_BYTES)
            })
            .is_err()
        {
            return false;
        }
        if self.data.try_reserve_exact(bytes.len()).is_err() {
            self.resources
                .bytes
                .fetch_sub(bytes.len(), Ordering::AcqRel);
            return false;
        }
        self.data.extend_from_slice(bytes);
        true
    }
    pub fn bytes(&self) -> Option<&[u8]> {
        self.lease.current().then_some(&self.data)
    }
}
impl Drop for Representation {
    fn drop(&mut self) {
        self.resources
            .bytes
            .fetch_sub(self.data.len(), Ordering::AcqRel);
        self.resources.slots.fetch_sub(1, Ordering::AcqRel);
    }
}

impl crate::clipboard::provider::Payload for Representation {
    fn bytes(&self) -> Option<&[u8]> {
        Representation::bytes(self)
    }
}

/// Dropping a publication revokes it without blocking the UI. A new source
/// requires a new publication even when its formats and server are unchanged.
pub struct Publication {
    pub generation: u64,
    active: Arc<AtomicBool>,
    source: Arc<dyn Source>,
    running: Arc<AtomicBool>,
}
impl Publication {
    pub fn is_current(&self) -> bool {
        self.active.load(Ordering::Acquire)
            && self.running.load(Ordering::Acquire)
            && self.source.is_current()
    }
}
impl Drop for Publication {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
        self.source.publication_revoked();
    }
}

// Cancellation can happen before the queued action becomes an Import. Retain
// a rejection guard until the native owner hands back an accepted publication.
struct Proposal(Option<Arc<dyn Source>>);
impl Proposal {
    fn accepted(mut self) {
        self.0.take();
    }
}
impl Drop for Proposal {
    fn drop(&mut self) {
        if let Some(source) = &self.0 {
            source.publication_revoked();
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
        reply: Reply<Publication>,
    },
    AttachFiles {
        generation: u64,
        folder: String,
        reply: Reply<isize>,
    },
}
impl Action {
    fn abandoned(&self) -> bool {
        match self {
            Self::Snapshot(reply) => reply.is_closed(),
            Self::Read { reply, .. } => reply.is_closed(),
            Self::Publish { reply, .. } => reply.is_closed(),
            Self::AttachFiles { reply, .. } => reply.is_closed(),
        }
    }
}

struct Inner {
    executable: PathBuf,
    board: Board,
    running: Arc<AtomicBool>,
    worker: OnceLock<Option<mpsc::SyncSender<Action>>>,
    completion: watch::Sender<bool>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
    }
}

#[derive(Clone)]
pub struct Owner(Arc<Inner>);
impl Owner {
    pub fn new(executable: PathBuf, board: Board) -> Self {
        Self(Arc::new(Inner {
            executable,
            board,
            running: Arc::new(AtomicBool::new(true)),
            worker: OnceLock::new(),
            completion: watch::channel(true).0,
        }))
    }

    /// Fence and stop this owner permanently. Child shutdown/reaping remains on
    /// the background thread. A stopped owner cannot silently restart.
    pub fn close(&self) {
        self.0.running.store(false, Ordering::Release);
    }

    /// Await actual helper teardown without joining a thread on the UI loop.
    pub async fn shutdown(&self) {
        self.close();
        // Synchronize with a concurrent first enqueue. Once closed, even a
        // not-yet-initialized owner can never start a worker later.
        self.0.worker.get_or_init(|| None);
        let mut completion = self.0.completion.subscribe();
        while !*completion.borrow_and_update() {
            if completion.changed().await.is_err() {
                break;
            }
        }
    }

    fn enqueue(&self, action: Action) {
        if !self.0.running.load(Ordering::Acquire) {
            return;
        }
        let sender = self.0.worker.get_or_init(|| {
            if !self.0.running.load(Ordering::Acquire) {
                return None;
            }
            let executable = self.0.executable.clone();
            let board = self.0.board.clone();
            let running = self.0.running.clone();
            let (tx, rx) = mpsc::sync_channel(SLOTS);
            self.0.completion.send_replace(false);
            let completion = Completion(self.0.completion.clone());
            std::thread::Builder::new()
                .name("clipboard-owner".into())
                .spawn(move || {
                    let _completion = completion;
                    run(executable, board, running, rx);
                })
                .ok()
                .map(|_| tx)
        });
        if let Some(sender) = sender {
            let _ = sender.try_send(action);
        }
    }

    pub async fn snapshot(&self) -> Option<Snapshot> {
        let (reply, result) = oneshot::channel();
        self.enqueue(Action::Snapshot(reply));
        tokio::time::timeout(Duration::from_secs(5), result)
            .await
            .ok()?
            .ok()?
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
        let proposal = Proposal(claimed.then(|| source.clone()));
        async move {
            if !claimed {
                return None;
            }
            let (reply, result) = oneshot::channel();
            owner.enqueue(Action::Publish {
                expected,
                formats,
                source,
                reply,
            });
            let publication = tokio::time::timeout(Duration::from_secs(5), result)
                .await
                .ok()?
                .ok()??;
            proposal.accepted();
            Some(publication)
        }
    }

    /// Replace the offer of publication `generation` with the files in
    /// `folder`, a UUID-named directory in the clipboard file cache, given
    /// relative to it (`<uuid>` or `<instance>/<uuid>`), as long
    /// as the board still holds that publication. Yields the new native change
    /// count, or `None` when refused; the publication then stays as it was.
    pub fn attach_files(
        &self,
        generation: u64,
        folder: String,
    ) -> impl std::future::Future<Output = Option<isize>> + Send + use<> {
        let owner = self.clone();
        async move {
            let (reply, result) = oneshot::channel();
            owner.enqueue(Action::AttachFiles {
                generation,
                folder,
                reply,
            });
            tokio::time::timeout(Duration::from_secs(5), result)
                .await
                .ok()?
                .ok()?
        }
    }
}

struct Completion(watch::Sender<bool>);
impl Drop for Completion {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}

fn run(
    executable: PathBuf,
    board: Board,
    running: Arc<AtomicBool>,
    actions: mpsc::Receiver<Action>,
) {
    let mut continuity = Continuity::default();
    let resources = Arc::new(Resources::default());
    while running.load(Ordering::Acquire) {
        let first = match actions.recv_timeout(TICK) {
            Ok(action) if !action.abandoned() => action,
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => return,
        };
        if !running.load(Ordering::Acquire) {
            return;
        }
        let client = match Client::spawn(&executable, board.clone()) {
            Ok(client) => client,
            Err(error) => {
                log::warn!("Clipboard helper startup failed: {error}");
                continue;
            }
        };
        let mut session =
            session::Session::new(client, &mut continuity, resources.clone(), running.clone());
        let result = session.run(first, &actions);
        if let Err(error) = result {
            log::warn!("Clipboard helper disconnected: {error}");
        }
        // Session drop invalidates delivered data before Client drop waits for
        // graceful shutdown. Pending operations fail rather than being replayed.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Candidate(AtomicBool);
    impl Source for Candidate {
        fn is_current(&self) -> bool {
            self.0.load(Ordering::Acquire)
        }
        fn request(&self, _: &str, _: crate::clipboard::provider::Reply) {}
        fn publication_revoked(&self) {
            self.0.store(false, Ordering::Release);
        }
    }

    #[tokio::test]
    async fn rejected_or_cancelled_enqueue_revokes_the_proposal_without_a_native_call() {
        for closed in [true, false] {
            let owner = Owner::new(
                PathBuf::from("unused"),
                Board::Test {
                    name: "org.newport.test.lazy.cancel".into(),
                },
            );
            let (sender, receiver) = mpsc::sync_channel(SLOTS);
            assert!(owner.0.worker.set(Some(sender)).is_ok());
            let source = Arc::new(Candidate(AtomicBool::new(true)));
            let unpolled = Arc::new(Candidate(AtomicBool::new(true)));
            drop(owner.publish(1, vec!["text/plain".into()], unpolled.clone()));
            assert!(!unpolled.is_current());
            assert!(receiver.try_recv().is_err());
            if closed {
                owner.close();
                assert!(
                    owner
                        .publish(1, vec!["text/plain".into()], source.clone())
                        .await
                        .is_none()
                );
                assert!(receiver.try_recv().is_err());
            } else {
                tokio::select! {
                    biased;
                    _ = owner.publish(1, vec!["text/plain".into()], source.clone()) => panic!("unprocessed action completed"),
                    _ = std::future::ready(()) => {}
                }
                assert!(receiver.try_recv().unwrap().abandoned());
            }
            assert!(
                !source.is_current(),
                "external source retained abandoned authority"
            );
        }
    }
}
