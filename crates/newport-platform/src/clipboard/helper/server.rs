use super::wire::{self, Board, Command, Event, VERSION};
mod local;
use crate::clipboard::provider::{
    MAX_BYTES, MAX_FORMATS, MAX_WAIT, Payload, Publication, Reply, Source,
};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSPasteboard};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSRunLoop, NSString};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

const QUEUE: usize = 8;
const TEST_PREFIX: &str = "org.newport.test.lazy.";

struct Lease {
    generation: u64,
    current: AtomicBool,
}
struct Bytes {
    data: Vec<u8>,
    budget: Arc<AtomicUsize>,
    lease: Arc<Lease>,
    connected: Arc<AtomicBool>,
}
impl Bytes {
    fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.budget
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes.len())
                    .filter(|used| *used <= MAX_BYTES)
            })
            .map_err(|_| invalid("Clipboard helper memory limit"))?;
        if self.data.try_reserve_exact(bytes.len()).is_err() {
            self.budget.fetch_sub(bytes.len(), Ordering::AcqRel);
            return Err(invalid("Clipboard helper allocation failed"));
        }
        self.data.extend_from_slice(bytes);
        Ok(())
    }
}
impl Payload for Bytes {
    fn bytes(&self) -> Option<&[u8]> {
        (self.lease.current.load(Ordering::Acquire) && self.connected.load(Ordering::Acquire))
            .then_some(&self.data)
    }
}
impl Drop for Bytes {
    fn drop(&mut self) {
        self.budget.fetch_sub(self.data.len(), Ordering::AcqRel);
    }
}
struct Transfer {
    reply: Reply,
    bytes: Bytes,
}
enum Action {
    Publish {
        lease: Arc<Lease>,
        expected: isize,
        formats: Vec<String>,
    },
    AttachFiles {
        lease: Arc<Lease>,
        expected: isize,
        folder: String,
    },
    Revoke(u64),
    ReadLocal(Arc<local::Read>),
}
#[derive(Default)]
struct State {
    generation: u64,
    lease: Option<Arc<Lease>>,
    read_id: u64,
    pending: BTreeMap<u64, Transfer>,
    actions: VecDeque<Action>,
    local_id: u64,
    local: BTreeMap<u64, Arc<local::Read>>,
}
enum Output {
    Event(Event),
    Local(local::Transfer),
}
struct Shared {
    state: Mutex<State>,
    connected: Arc<AtomicBool>,
    budget: Arc<AtomicUsize>,
    local_budget: Arc<AtomicUsize>,
    observed: AtomicIsize,
    output: mpsc::SyncSender<Output>,
    socket: UnixStream,
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
/// Where downloaded clipboard files live: the app cache for the general board,
/// a directory named after the unique board for disposable test boards.
fn files_root(board: &Board) -> Option<PathBuf> {
    match board {
        Board::General {} => Some(dirs::cache_dir()?.join("Newport").join("Clipboard")),
        Board::Test { name } => Some(std::env::temp_dir().join(name)),
    }
}
/// A UUID-named directory directly under `root`, or under one instance folder
/// in it (`<instance>/<uuid>`, the instance named with lowercase letters and
/// digits), with no symlink involved.
fn attachable_folder(root: &Path, folder: &str) -> Option<PathBuf> {
    let (instance, name) = match folder.split_once('/') {
        Some((instance, name)) => (Some(instance), name),
        None => (None, folder),
    };
    if instance.is_some_and(|instance| {
        instance.is_empty()
            || instance.len() > 64
            || !instance
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    }) {
        return None;
    }
    let id = uuid::Uuid::parse_str(name).ok()?;
    if id.hyphenated().to_string() != name.to_ascii_lowercase() {
        return None;
    }
    let root = root.canonicalize().ok()?;
    let parent = instance.map_or_else(|| root.clone(), |instance| root.join(instance));
    let candidate = root.join(folder).canonicalize().ok()?;
    (candidate.parent() == Some(parent.as_path())
        && candidate.file_name().and_then(|n| n.to_str()) == Some(name)
        && candidate.is_dir())
    .then_some(candidate)
}
fn formats_valid(formats: &[String]) -> bool {
    let mut unique = BTreeSet::new();
    !formats.is_empty()
        && formats.len() <= MAX_FORMATS
        && formats.iter().all(|format| {
            !format.is_empty()
                && format.len() <= 256
                && !format.chars().any(char::is_control)
                && unique.insert(format)
        })
}
impl Shared {
    fn stop(&self) {
        self.connected.store(false, Ordering::Release);
        let _ = self.socket.shutdown(Shutdown::Both);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(lease) = state.lease.take() {
            lease.current.store(false, Ordering::Release);
        }
        state.pending.clear();
        state.actions.clear();
        for read in state.local.values() {
            read.active.store(false, Ordering::Release);
        }
        state.local.clear();
    }
    fn send(&self, event: Event) {
        if self.output.try_send(Output::Event(event)).is_err() {
            self.stop();
        }
    }
    fn expire(&self) {
        let mut cancelled = Vec::new();
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.pending.retain(|id, transfer| {
                let keep = !transfer.reply.is_cancelled()
                    && transfer.bytes.lease.current.load(Ordering::Acquire);
                if !keep {
                    cancelled.push(Event::Cancelled {
                        generation: transfer.bytes.lease.generation,
                        id: *id,
                    });
                }
                keep
            });
            state.local.retain(|_, read| {
                let current = read.current(self);
                if !current {
                    read.active.store(false, Ordering::Release);
                }
                current
            });
        }
        for event in cancelled {
            self.send(event);
        }
    }
    fn command(&self, command: Command, bytes: &[u8]) -> io::Result<()> {
        if !bytes.is_empty() && !matches!(command, Command::Data { .. }) {
            return Err(invalid("Unexpected helper command bytes"));
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match command {
            Command::ReadLocal {
                id,
                revision,
                format,
            } => {
                if id == 0
                    || id <= state.local_id
                    || format.is_empty()
                    || format.len() > 256
                    || format.chars().any(char::is_control)
                {
                    return Err(invalid("Invalid local clipboard read"));
                }
                state.local_id = id;
                if state.local.len() >= QUEUE || state.actions.len() >= QUEUE {
                    drop(state);
                    self.send(Event::LocalFailed { id });
                    return Ok(());
                }
                let read = local::Read::new(id, revision, format);
                state.local.insert(id, read.clone());
                state.actions.push_back(Action::ReadLocal(read));
            }
            Command::CancelLocal { id } => {
                if id == 0 || id > state.local_id {
                    return Err(invalid("Unknown local clipboard read"));
                }
                if let Some(read) = state.local.remove(&id) {
                    read.active.store(false, Ordering::Release);
                }
            }
            Command::Publish {
                generation,
                expected_revision,
                formats,
            } => {
                if generation == 0 || generation <= state.generation || !formats_valid(&formats) {
                    return Err(invalid("Invalid clipboard helper publication"));
                }
                if state.actions.len() >= QUEUE {
                    return Err(invalid("Clipboard helper action queue full"));
                }
                if let Some(old) = state.lease.take() {
                    old.current.store(false, Ordering::Release);
                }
                let lease = Arc::new(Lease {
                    generation,
                    current: AtomicBool::new(true),
                });
                state.generation = generation;
                state.lease = Some(lease.clone());
                state.actions.push_back(Action::Publish {
                    lease,
                    expected: expected_revision,
                    formats,
                });
            }
            Command::AttachFiles {
                generation,
                expected_revision,
                folder,
            } => {
                let lease = state
                    .lease
                    .clone()
                    .filter(|l| l.generation == generation && l.current.load(Ordering::Acquire));
                match lease {
                    Some(lease) if state.actions.len() < QUEUE && folder.len() <= 64 => {
                        state.actions.push_back(Action::AttachFiles {
                            lease,
                            expected: expected_revision,
                            folder,
                        });
                    }
                    _ => {
                        drop(state);
                        self.send(Event::Attached {
                            generation,
                            revision: None,
                        });
                        return Ok(());
                    }
                }
            }
            Command::Revoke { generation } => {
                if generation == 0 || generation > state.generation {
                    return Err(invalid("Unknown clipboard helper generation"));
                }
                if let Some(lease) = &state.lease
                    && lease.generation == generation
                {
                    lease.current.store(false, Ordering::Release);
                }
                if state.actions.len() >= QUEUE {
                    return Err(invalid("Clipboard helper action queue full"));
                }
                state.actions.push_back(Action::Revoke(generation));
            }
            Command::Data {
                generation,
                id,
                offset,
                eof,
            } => {
                let last_id = state.read_id;
                let last_generation = state.generation;
                let Some(transfer) = state.pending.get_mut(&id) else {
                    // Already cancelled requests may have chunks in flight.
                    // IDs never repeat on this connection; discard them without
                    // storing payloads or granting authority to a new offer.
                    if id > 0 && id <= last_id && generation > 0 && generation <= last_generation {
                        return Ok(());
                    }
                    return Err(invalid("Unknown clipboard helper read"));
                };
                if transfer.bytes.lease.generation != generation
                    || offset != transfer.bytes.data.len()
                    || (bytes.is_empty() && !eof)
                {
                    return Err(invalid("Invalid clipboard helper data sequence"));
                }
                transfer.bytes.append(bytes)?;
                if eof {
                    let transfer = state.pending.remove(&id).unwrap();
                    // No source calls while holding the control state lock.
                    drop(state);
                    transfer.reply.complete(Box::new(transfer.bytes));
                    return Ok(());
                }
            }
            Command::Failed { generation, id } => {
                if id == 0 || id > state.read_id || generation == 0 || generation > state.generation
                {
                    return Err(invalid("Unknown failed clipboard helper read"));
                }
                if state
                    .pending
                    .get(&id)
                    .is_some_and(|t| t.bytes.lease.generation != generation)
                {
                    return Err(invalid("Clipboard helper source mismatch"));
                }
                state.pending.remove(&id);
            }
            Command::Stop {} => {
                drop(state);
                self.stop();
                return Ok(());
            }
            Command::Hello { .. } => return Err(invalid("Repeated clipboard helper handshake")),
        }
        drop(state);
        self.expire();
        Ok(())
    }
    fn finish_local(&self, id: u64) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .local
            .remove(&id);
    }
}

struct Remote {
    shared: Arc<Shared>,
    lease: Arc<Lease>,
}
impl Source for Remote {
    fn is_current(&self) -> bool {
        self.shared.connected.load(Ordering::Acquire) && self.lease.current.load(Ordering::Acquire)
    }
    fn request(&self, format: &str, reply: Reply) {
        if !self.is_current() {
            return;
        }
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if !self.is_current() || state.pending.len() >= QUEUE {
            return;
        }
        let Some(id) = state.read_id.checked_add(1) else {
            drop(state);
            self.shared.stop();
            return;
        };
        state.read_id = id;
        state.pending.insert(
            id,
            Transfer {
                reply,
                bytes: Bytes {
                    data: Vec::new(),
                    budget: self.shared.budget.clone(),
                    lease: self.lease.clone(),
                    connected: self.shared.connected.clone(),
                },
            },
        );
        // Queue demand while it is still registered, so a concurrent revoke
        // cannot enqueue cancellation before the corresponding Read event.
        let sent = self.shared.output.try_send(Output::Event(Event::Read {
            generation: self.lease.generation,
            id,
            format: format.into(),
        }));
        drop(state);
        if sent.is_err() {
            self.shared.stop();
        }
    }
}

pub(super) fn run(mut socket: UnixStream) -> io::Result<()> {
    let hello = wire::receive::<Command>(&mut socket, Duration::from_secs(3))?
        .ok_or(io::ErrorKind::TimedOut)?;
    if !hello.bytes.is_empty() {
        return Err(invalid("Invalid clipboard helper handshake"));
    }
    let Command::Hello {
        version: VERSION,
        board,
    } = hello.header
    else {
        return Err(invalid("Clipboard helper version mismatch"));
    };
    if let Board::Test { name } = &board
        && !name
            .strip_prefix(TEST_PREFIX)
            .is_some_and(|suffix| uuid::Uuid::parse_str(suffix).is_ok())
    {
        return Err(invalid("Clipboard helper test name is not unique"));
    }
    let main = MainThreadMarker::new()
        .ok_or_else(|| invalid("Clipboard helper requires its main thread"))?;
    NSApplication::sharedApplication(main)
        .setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
    let read_board = board.clone();
    let root = files_root(&board);
    let board = match board {
        Board::General {} => NSPasteboard::generalPasteboard(),
        Board::Test { name } => NSPasteboard::pasteboardWithName(&NSString::from_str(&name)),
    };
    let (output, events) = mpsc::sync_channel(32);
    let shared = Arc::new(Shared {
        state: Mutex::new(State::default()),
        connected: Arc::new(AtomicBool::new(true)),
        budget: Arc::new(AtomicUsize::new(0)),
        local_budget: Arc::new(AtomicUsize::new(0)),
        observed: AtomicIsize::new(-1),
        output,
        socket: socket.try_clone()?,
    });
    let mut writer = socket.try_clone()?;
    let outgoing = shared.clone();
    let writer = std::thread::spawn(move || {
        let mut local: VecDeque<local::Transfer> = VecDeque::new();
        while outgoing.connected.load(Ordering::Acquire) {
            let timeout = if local.is_empty() {
                Duration::from_millis(100)
            } else {
                Duration::ZERO
            };
            match events.recv_timeout(timeout) {
                Ok(Output::Event(event)) => {
                    if wire::send(&mut writer, &event, &[]).is_err() {
                        break;
                    }
                }
                Ok(Output::Local(transfer)) => local.push_back(transfer),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => break,
            }
            if let Some(mut transfer) = local.pop_front() {
                match transfer.send_chunk(&mut writer, &outgoing) {
                    Ok(true) => outgoing.finish_local(transfer.read.id),
                    Ok(false) => local.push_back(transfer),
                    Err(_) => break,
                }
            }
        }
        outgoing.stop();
    });
    let incoming = shared.clone();
    let reader = std::thread::spawn(move || {
        while incoming.connected.load(Ordering::Acquire) {
            match wire::receive::<Command>(&mut socket, Duration::from_millis(100)) {
                Ok(Some(frame)) => {
                    if incoming.command(frame.header, &frame.bytes).is_err() {
                        break;
                    }
                    incoming.expire();
                }
                Ok(None) => incoming.expire(),
                Err(_) => break,
            }
        }
        incoming.stop();
    });
    let (local_requests, local_work) = mpsc::sync_channel::<Arc<local::Read>>(QUEUE);
    let reading = shared.clone();
    let local_reader = std::thread::spawn(move || {
        // Native objects are created and released on this worker, not moved
        // between threads. Payload retrieval must not stall metadata/focus and
        // publication on the helper's main loop.
        let board = match read_board {
            Board::General {} => NSPasteboard::generalPasteboard(),
            Board::Test { name } => NSPasteboard::pasteboardWithName(&NSString::from_str(&name)),
        };
        while reading.connected.load(Ordering::Acquire) {
            let read = match local_work.recv_timeout(Duration::from_millis(100)) {
                Ok(read) => read,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => break,
            };
            objc2::rc::autoreleasepool(|_| {
                match local::Transfer::capture(&board, read.clone(), &reading) {
                    Some(transfer) => {
                        if reading.output.try_send(Output::Local(transfer)).is_err() {
                            reading.stop();
                        }
                    }
                    None => {
                        reading.finish_local(read.id);
                        reading.send(Event::LocalFailed { id: read.id });
                    }
                }
            });
        }
    });
    shared.send(Event::Ready { version: VERSION });
    let mut publication: Option<(u64, Publication)> = None;
    let mut observed = None;
    while shared.connected.load(Ordering::Acquire) {
        objc2::rc::autoreleasepool(|_| {
            let action = shared
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .actions
                .pop_front();
            match action {
                Some(Action::ReadLocal(read)) => {
                    let imported = publication
                        .as_ref()
                        .is_some_and(|(_, p)| p.revision() == read.revision);
                    if imported
                        || !read.current(&shared)
                        || local_requests.try_send(read.clone()).is_err()
                    {
                        shared.finish_local(read.id);
                        shared.send(Event::LocalFailed { id: read.id });
                    }
                }
                Some(Action::Publish {
                    lease,
                    expected,
                    formats,
                }) => {
                    let generation = lease.generation;
                    let remote = Arc::new(Remote {
                        shared: shared.clone(),
                        lease: lease.clone(),
                    });
                    match Publication::publish(
                        main,
                        board.clone(),
                        expected,
                        &formats,
                        remote,
                        MAX_WAIT,
                    ) {
                        Ok(next) => {
                            let revision = next.revision();
                            publication = Some((generation, next));
                            shared.send(Event::Published {
                                generation,
                                revision,
                            });
                        }
                        Err(_) => {
                            lease.current.store(false, Ordering::Release);
                            shared.send(Event::Rejected { generation });
                        }
                    }
                }
                Some(Action::AttachFiles {
                    lease,
                    expected,
                    folder,
                }) => {
                    let revision = publication
                        .as_ref()
                        .filter(|(owner, _)| *owner == lease.generation)
                        .filter(|_| lease.current.load(Ordering::Acquire))
                        .and_then(|(_, p)| {
                            let folder = attachable_folder(root.as_deref()?, &folder)?;
                            p.attach_files(expected, &folder)
                        });
                    shared.send(Event::Attached {
                        generation: lease.generation,
                        revision,
                    });
                }
                Some(Action::Revoke(generation)) => {
                    let revision = if publication
                        .as_ref()
                        .is_some_and(|(owner, _)| *owner == generation)
                    {
                        let (_, old) = publication.take().unwrap();
                        old.withdraw_revision()
                    } else {
                        None
                    };
                    shared.send(Event::Revoked {
                        generation,
                        revision,
                    });
                }
                None => {}
            }
            // Native ownership can stay unchanged while a producer adds types
            // after clearing. Observe metadata as well as the ownership count;
            // never render a payload while checking for these updates.
            if let Some((revision, formats)) = crate::clipboard::macos::offer_from(&board, None)
                && observed.as_ref().is_none_or(|(old_revision, old_formats)| {
                    *old_revision != revision || *old_formats != formats
                })
            {
                observed = Some((revision, formats.clone()));
                shared.observed.store(revision, Ordering::Release);
                // Native metadata is bounded too; never let a local owner flood
                // IPC with arbitrary representation names.
                if formats.len() > MAX_FORMATS
                    || formats
                        .iter()
                        .any(|f| f.len() > 256 || f.chars().any(char::is_control))
                {
                    shared.stop();
                    return;
                }
                let origin = publication
                    .as_ref()
                    .filter(|(_, p)| p.revision() == revision)
                    .map(|(g, _)| *g);
                shared.send(Event::Snapshot {
                    revision,
                    formats,
                    origin,
                });
            }
            if !NSRunLoop::currentRunLoop().runMode_beforeDate(
                unsafe { NSDefaultRunLoopMode },
                &NSDate::dateWithTimeIntervalSinceNow(0.01),
            ) {
                std::thread::sleep(Duration::from_millis(10));
            }
        });
    }
    if let Some((_, publication)) = publication {
        publication.withdraw();
    }
    shared.stop();
    drop(local_requests);
    local_reader
        .join()
        .map_err(|_| invalid("Clipboard helper local reader failed"))?;
    reader
        .join()
        .map_err(|_| invalid("Clipboard helper control reader failed"))?;
    writer
        .join()
        .map_err(|_| invalid("Clipboard helper event writer failed"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_uuid_directories_directly_under_the_root_are_attachable() {
        let base =
            std::env::temp_dir().join(format!("org.newport.test.root.{}", uuid::Uuid::new_v4()));
        let root = base.join("Clipboard");
        let outside = base.join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let good = uuid::Uuid::new_v4().to_string();
        std::fs::create_dir(root.join(&good)).unwrap();
        let canonical = root.canonicalize().unwrap();
        assert_eq!(attachable_folder(&root, &good), Some(canonical.join(&good)));
        // Symlinks to outside or to a differently named entry, nesting,
        // traversal, non-UUID names, files and missing entries are refused.
        let link = uuid::Uuid::new_v4().to_string();
        std::os::unix::fs::symlink(&outside, root.join(&link)).unwrap();
        let alias = uuid::Uuid::new_v4().to_string();
        std::os::unix::fs::symlink(root.join(&good), root.join(&alias)).unwrap();
        let file = uuid::Uuid::new_v4().to_string();
        std::fs::write(root.join(&file), b"x").unwrap();
        std::fs::create_dir(root.join(&good).join("inner")).unwrap();
        let nested = format!("{good}/inner");
        let traversal = "../outside".to_string();
        let absolute = outside.to_str().unwrap().to_string();
        for refused in [
            link,
            alias,
            file,
            nested,
            traversal,
            absolute,
            "plain".into(),
            uuid::Uuid::new_v4().to_string(),
            String::new(),
        ] {
            assert_eq!(attachable_folder(&root, &refused), None, "{refused}");
        }
        assert_eq!(attachable_folder(&base.join("missing"), &good), None);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn uuid_directories_in_one_instance_folder_are_attachable() {
        let base =
            std::env::temp_dir().join(format!("org.newport.test.root.{}", uuid::Uuid::new_v4()));
        let root = base.join("Clipboard");
        let outside = base.join("outside");
        let good = uuid::Uuid::new_v4().to_string();
        std::fs::create_dir_all(root.join("0a1b2c3d4e5f6a7b").join(&good)).unwrap();
        std::fs::create_dir_all(outside.join(&good)).unwrap();
        let canonical = root.canonicalize().unwrap();
        let nested = format!("0a1b2c3d4e5f6a7b/{good}");
        assert_eq!(
            attachable_folder(&root, &nested),
            Some(canonical.join(&nested))
        );
        // An instance folder that is a link, or named outside lowercase
        // letters and digits, and deeper nesting are refused.
        std::os::unix::fs::symlink(&outside, root.join("linked")).unwrap();
        std::fs::create_dir_all(root.join("UPPER").join(&good)).unwrap();
        std::fs::create_dir_all(root.join("a-b").join(&good)).unwrap();
        std::fs::create_dir_all(root.join("x/y").join(&good)).unwrap();
        for refused in [
            format!("linked/{good}"),
            format!("UPPER/{good}"),
            format!("a-b/{good}"),
            format!("../outside/{good}"),
            format!("/{good}"),
            format!("x/y/{good}"),
            format!("0a1b2c3d4e5f6a7b/{}", uuid::Uuid::new_v4()),
            format!("{}/{good}", "a".repeat(65)),
        ] {
            assert_eq!(attachable_folder(&root, &refused), None, "{refused}");
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_boards_use_a_root_named_after_their_unique_name() {
        let name = format!("{TEST_PREFIX}{}", uuid::Uuid::new_v4());
        assert_eq!(
            files_root(&Board::Test { name: name.clone() }),
            Some(std::env::temp_dir().join(name))
        );
        assert!(
            files_root(&Board::General {})
                .unwrap()
                .ends_with("Newport/Clipboard")
        );
    }
    #[test]
    fn assembled_and_delivered_payloads_share_a_budget_and_keep_the_lease() {
        let budget = Arc::new(AtomicUsize::new(0));
        let lease = Arc::new(Lease {
            generation: 1,
            current: AtomicBool::new(true),
        });
        let connected = Arc::new(AtomicBool::new(true));
        let mut first = Bytes {
            data: Vec::new(),
            budget: budget.clone(),
            lease: lease.clone(),
            connected: connected.clone(),
        };
        let mut second = Bytes {
            data: Vec::new(),
            budget: budget.clone(),
            lease: lease.clone(),
            connected: connected.clone(),
        };
        first.append(&vec![1; MAX_BYTES / 2]).unwrap();
        second.append(&vec![2; MAX_BYTES / 2]).unwrap();
        assert!(second.append(&[1]).is_err());
        assert_eq!(budget.load(Ordering::Acquire), MAX_BYTES);
        lease.current.store(false, Ordering::Release);
        assert!(first.bytes().is_none());
        assert_eq!(budget.load(Ordering::Acquire), MAX_BYTES);
        drop(first);
        assert_eq!(budget.load(Ordering::Acquire), MAX_BYTES / 2);
        drop(second);
        assert_eq!(budget.load(Ordering::Acquire), 0);
    }
}
