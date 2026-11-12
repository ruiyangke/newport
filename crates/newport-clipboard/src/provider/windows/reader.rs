//! A foreign delayed owner can stall GetClipboardData. Keep one bounded read
//! lane for the process; cancellation never spawns a replacement blocked thread.
use super::*;
struct Read {
    snapshot: Snapshot,
    format: String,
    resources: Arc<Resources>,
    reply: Reply<Representation>,
}
#[cfg(test)]
pub(super) static THREADS_STARTED: AtomicUsize = AtomicUsize::new(0);
static READER: OnceLock<Option<mpsc::SyncSender<Job>>> = OnceLock::new();
enum Job {
    Read(Read),
    Privacy {
        native: u32,
        owner: usize,
        running: Arc<AtomicBool>,
        reply: Reply<bool>,
    },
}
fn enqueue(job: Job) {
    let reader = READER.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel::<Job>(SLOTS);
        std::thread::Builder::new()
            .name("clipboard-reader".into())
            .spawn(move || {
                #[cfg(test)]
                THREADS_STARTED.fetch_add(1, Ordering::AcqRel);
                while let Ok(job) = receiver.recv() {
                    match job {
                        Job::Read(job) => capture(job),
                        Job::Privacy {
                            native,
                            owner,
                            running,
                            reply,
                        } => {
                            if !reply.is_closed() && running.load(Ordering::Acquire) {
                                let result = crate::windows::read_privacy(native, owner);
                                if running.load(Ordering::Acquire) {
                                    let _ = reply.send(result);
                                }
                            }
                        }
                    }
                }
            })
            .ok()
            .map(|_| sender)
    });
    if let Some(reader) = reader {
        let _ = reader.try_send(job);
    }
}
pub(super) fn read(
    snapshot: Snapshot,
    format: String,
    resources: Arc<Resources>,
    reply: Reply<Representation>,
) {
    enqueue(Job::Read(Read {
        snapshot,
        format,
        resources,
        reply,
    }));
}
pub(super) fn privacy(
    native: u32,
    owner: usize,
    running: Arc<AtomicBool>,
) -> oneshot::Receiver<Option<bool>> {
    let (reply, receive) = oneshot::channel();
    enqueue(Job::Privacy {
        native,
        owner,
        running,
        reply,
    });
    receive
}
fn capture(job: Read) {
    let Read {
        snapshot,
        format,
        resources,
        reply,
    } = job;
    if reply.is_closed() || !snapshot.is_current() {
        return;
    }
    if resources
        .slots
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
            (used < SLOTS).then_some(used + 1)
        })
        .is_err()
    {
        return;
    }
    let mut data = Representation {
        bytes: Vec::new(),
        lease: snapshot.lease.clone(),
        resources: resources.clone(),
    };
    let Some(bytes) = crate::windows::read_format(
        snapshot.lease.native.load(Ordering::Acquire) as isize,
        &format,
    ) else {
        return;
    };
    if bytes.len() > MAX_BYTES || reply.is_closed() || !snapshot.is_current() {
        return;
    }
    if resources
        .bytes
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
            used.checked_add(bytes.len())
                .filter(|bytes| *bytes <= MAX_BYTES)
        })
        .is_err()
    {
        return;
    }
    data.bytes = bytes;
    let _ = reply.send(Some(data));
}
