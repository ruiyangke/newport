use super::*;
use crate::clipboard::helper::{CHUNK, Command, Event, Packet};
use crate::clipboard::provider::{MAX_FORMATS, Payload, Pending};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::time::Instant;

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn formats_valid(formats: &[String]) -> bool {
    let mut unique = BTreeSet::new();
    formats.len() <= MAX_FORMATS
        && formats.iter().all(|format| {
            !format.is_empty()
                && format.len() <= 256
                && !format.chars().any(char::is_control)
                && unique.insert(format)
        })
}

struct Read {
    data: Representation,
    reply: Reply<Representation>,
    deadline: Instant,
}
struct Import {
    generation: u64,
    formats: Vec<String>,
    source: Arc<dyn Source>,
    active: Arc<AtomicBool>,
    reply: Option<Reply<Publication>>,
    native: Option<isize>,
}
impl Import {
    fn current(&self) -> bool {
        self.active.load(Ordering::Acquire)
            && self.source.is_current()
            && self.reply.as_ref().is_none_or(|reply| !reply.is_closed())
    }
}
impl Drop for Import {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
        self.source.publication_revoked();
    }
}
struct Export {
    pending: Option<Pending>,
    data: Option<Box<dyn Payload>>,
    offset: usize,
    deadline: Instant,
}

pub(super) struct Session<'a> {
    client: Client,
    continuity: &'a mut Continuity,
    snapshot: Option<Snapshot>,
    snapshots: Vec<Reply<Snapshot>>,
    running: Arc<AtomicBool>,
    resources: Arc<Resources>,
    read_id: u64,
    reads: BTreeMap<u64, Read>,
    generation: u64,
    import: Option<Import>,
    attach: Option<(u64, Reply<isize>)>,
    export_id: u64,
    exports: BTreeMap<u64, Export>,
}
impl<'a> Session<'a> {
    pub fn new(
        client: Client,
        continuity: &'a mut Continuity,
        resources: Arc<Resources>,
        running: Arc<AtomicBool>,
    ) -> Self {
        Self {
            client,
            continuity,
            snapshot: None,
            snapshots: Vec::new(),
            running,
            resources,
            read_id: 0,
            reads: BTreeMap::new(),
            generation: 0,
            import: None,
            attach: None,
            export_id: 0,
            exports: BTreeMap::new(),
        }
    }
    fn send(&self, command: Command, bytes: &[u8]) -> io::Result<()> {
        self.client.sender().send(&command, bytes)
    }
    fn invalidate(&mut self) {
        if let Some(snapshot) = self.snapshot.take() {
            snapshot.lease.current.store(false, Ordering::Release);
        }
    }
    pub fn run(&mut self, first: Action, actions: &mpsc::Receiver<Action>) -> io::Result<()> {
        self.action(first)?;
        while self.running.load(Ordering::Acquire) {
            match actions.try_recv() {
                Ok(action) => self.action(action)?,
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
            self.maintain()?;
            if let Some(packet) = self.client.receive_packet(TICK)? {
                self.event(packet)?;
            }
        }
        Ok(())
    }
    fn action(&mut self, action: Action) -> io::Result<()> {
        if action.abandoned() || !self.running.load(Ordering::Acquire) {
            return Ok(());
        }
        match action {
            Action::Snapshot(reply) => {
                if let Some(snapshot) = &self.snapshot {
                    let _ = reply.send(Some(snapshot.clone()));
                } else {
                    self.snapshots.retain(|reply| !reply.is_closed());
                    if self.snapshots.len() < SLOTS {
                        self.snapshots.push(reply);
                    }
                }
            }
            Action::Read {
                revision,
                format,
                reply,
            } => {
                let Some(snapshot) = &self.snapshot else {
                    return Ok(());
                };
                // Imported promises are never read recursively. The policy
                // router may use their recorded remote source instead.
                if snapshot.revision != revision
                    || !snapshot.is_current()
                    || snapshot.origin.is_some()
                    || !snapshot.formats.contains(&format)
                {
                    return Ok(());
                }
                let Some(data) = Representation::reserve(snapshot.lease.clone(), &self.resources)
                else {
                    return Ok(());
                };
                self.read_id = self
                    .read_id
                    .checked_add(1)
                    .ok_or_else(|| invalid("Clipboard read ID exhausted"))?;
                self.send(
                    Command::ReadLocal {
                        id: self.read_id,
                        revision: snapshot.lease.native,
                        format,
                    },
                    &[],
                )?;
                self.reads.insert(
                    self.read_id,
                    Read {
                        data,
                        reply,
                        deadline: Instant::now() + MAX_WAIT,
                    },
                );
            }
            Action::Publish {
                expected,
                formats,
                source,
                reply,
            } => {
                let Some(snapshot) = &self.snapshot else {
                    return Ok(());
                };
                if snapshot.revision != expected
                    || !snapshot.is_current()
                    || !source.is_current()
                    || formats.is_empty()
                    || !formats_valid(&formats)
                {
                    return Ok(());
                }
                let native = snapshot.lease.native;
                self.generation = self
                    .continuity
                    .generation
                    .checked_add(1)
                    .ok_or_else(|| invalid("Clipboard generation exhausted"))?;
                self.continuity.generation = self.generation;
                self.import = None;
                self.exports.clear();
                self.send(
                    Command::Publish {
                        generation: self.generation,
                        expected_revision: native,
                        formats: formats.clone(),
                    },
                    &[],
                )?;
                self.import = Some(Import {
                    generation: self.generation,
                    formats,
                    source,
                    active: Arc::new(AtomicBool::new(true)),
                    reply: Some(reply),
                    native: None,
                });
            }
            Action::AttachFiles {
                generation,
                folder,
                reply,
            } => {
                // One attach at a time, only for the live imported copy while
                // its native revision is the one we know.
                let Some(native) = self
                    .import
                    .as_ref()
                    .filter(|i| i.generation == generation && i.current() && i.reply.is_none())
                    .and_then(|i| i.native)
                    .filter(|_| self.attach.is_none())
                else {
                    return Ok(());
                };
                self.send(
                    Command::AttachFiles {
                        generation,
                        expected_revision: native,
                        folder,
                    },
                    &[],
                )?;
                self.attach = Some((generation, reply));
            }
        }
        Ok(())
    }
    fn event(&mut self, packet: Packet) -> io::Result<()> {
        match packet.event {
            Event::Snapshot {
                revision,
                formats,
                origin,
            } => {
                if !formats_valid(&formats) || origin.is_some_and(|g| g == 0 || g > self.generation)
                {
                    return Err(invalid("Invalid clipboard metadata"));
                }
                let origin = if let Some(generation) = origin {
                    self.continuity.origin = Some((revision, generation));
                    Some(generation)
                } else if let Some((native, generation)) = self.continuity.origin
                    && native == revision
                {
                    Some(generation)
                } else {
                    self.continuity.origin = None;
                    None
                };
                // Duplicate native snapshots do not create a new logical copy.
                if let Some(old) = &self.snapshot
                    && old.lease.native == revision
                    && old.origin == origin
                    && old.formats == formats
                {
                    return Ok(());
                }
                self.invalidate();
                if self.import.as_ref().is_some_and(|i| match i.native {
                    Some(native) => native != revision || origin != Some(i.generation),
                    // Exact duplicates already returned above. Any observed
                    // external metadata change invalidates a pending write,
                    // including types added under the same native change count.
                    None => origin != Some(i.generation),
                }) {
                    let import = self.import.take().unwrap();
                    self.exports.clear();
                    self.send(
                        Command::Revoke {
                            generation: import.generation,
                        },
                        &[],
                    )?;
                }
                self.continuity.revision = self
                    .continuity
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| invalid("Clipboard revision exhausted"))?;
                let snapshot = Snapshot {
                    revision: self.continuity.revision,
                    formats,
                    origin,
                    lease: Arc::new(Lease {
                        current: AtomicBool::new(true),
                        native: revision,
                        running: self.running.clone(),
                    }),
                };
                for reply in self.snapshots.drain(..) {
                    let _ = reply.send(Some(snapshot.clone()));
                }
                self.snapshot = Some(snapshot);
            }
            Event::LocalData {
                id,
                revision,
                offset,
                eof,
            } => {
                if id == 0 || id > self.read_id {
                    return Err(invalid("Unknown local clipboard result"));
                }
                if let Some(read) = self.reads.get_mut(&id) {
                    if revision != read.data.lease.native
                        || offset != read.data.data.len()
                        || (packet.bytes.is_empty() && !eof)
                    {
                        return Err(invalid("Invalid local clipboard result sequence"));
                    }
                    if !read.data.append(&packet.bytes) {
                        self.send(Command::CancelLocal { id }, &[])?;
                        self.reads.remove(&id);
                    } else if eof {
                        let read = self.reads.remove(&id).unwrap();
                        if read.data.bytes().is_some() && Instant::now() < read.deadline {
                            let _ = read.reply.send(Some(read.data));
                        }
                    }
                }
            }
            Event::LocalFailed { id } => {
                if id == 0 || id > self.read_id {
                    return Err(invalid("Unknown failed local clipboard read"));
                }
                self.reads.remove(&id);
            }
            Event::Published {
                generation,
                revision,
            } => {
                if generation == 0 || generation > self.generation {
                    return Err(invalid("Unknown clipboard publication"));
                }
                self.continuity.origin = Some((revision, generation));
                if let Some(import) = self.import.as_mut().filter(|i| i.generation == generation) {
                    import.native = Some(revision);
                    if import.reply.is_some() {
                        if import.current() && import.source.publication_confirmed() {
                            let _ = import.reply.take().unwrap().send(Some(Publication {
                                generation,
                                active: import.active.clone(),
                                source: import.source.clone(),
                                running: self.running.clone(),
                            }));
                        } else {
                            import.active.store(false, Ordering::Release);
                            import.source.publication_revoked();
                            import.reply.take();
                        }
                    }
                    self.invalidate();
                }
            }
            Event::Attached {
                generation,
                revision,
            } => {
                if generation == 0 || generation > self.generation {
                    return Err(invalid("Unknown clipboard attachment"));
                }
                let Some((pending, reply)) = self.attach.take() else {
                    return Err(invalid("Unexpected clipboard attachment"));
                };
                if pending != generation {
                    return Err(invalid("Mismatched clipboard attachment"));
                }
                if let Some(revision) = revision {
                    self.continuity.origin = Some((revision, generation));
                    if let Some(import) =
                        self.import.as_mut().filter(|i| i.generation == generation)
                    {
                        import.native = Some(revision);
                    }
                    self.invalidate();
                }
                let _ = reply.send(revision);
            }
            event @ (Event::Rejected { generation } | Event::Revoked { generation, .. }) => {
                if generation == 0 || generation > self.generation {
                    return Err(invalid("Unknown clipboard revocation"));
                }
                if let Event::Revoked {
                    revision: Some(native),
                    ..
                } = event
                    && self
                        .continuity
                        .origin
                        .is_none_or(|(_, known)| known <= generation)
                {
                    // Withdrawing our own promise clears the board. That clear
                    // is still our write, not a new user copy to export back to
                    // the runtime. A subsequent real copy has another count.
                    self.continuity.origin = Some((native, generation));
                }
                if self
                    .import
                    .as_ref()
                    .is_some_and(|i| i.generation == generation)
                {
                    self.import = None;
                    self.exports.clear();
                    if let Some(old) = self.snapshot.as_ref().and_then(|s| s.origin) {
                        self.send(Command::Revoke { generation: old }, &[])?;
                        self.invalidate();
                    }
                }
            }
            Event::Read {
                generation,
                id,
                format,
            } => {
                if id == 0
                    || id <= self.export_id
                    || generation == 0
                    || generation > self.generation
                {
                    return Err(invalid("Invalid native clipboard demand"));
                }
                self.export_id = id;
                let Some(import) = self.import.as_ref().filter(|i| {
                    i.generation == generation && i.current() && i.formats.contains(&format)
                }) else {
                    return self.send(Command::Failed { generation, id }, &[]);
                };
                if self.exports.len() >= SLOTS {
                    return self.send(Command::Failed { generation, id }, &[]);
                }
                let (pending, reply) = Pending::new(MAX_WAIT);
                import.source.request(&format, reply);
                self.exports.insert(
                    id,
                    Export {
                        pending: Some(pending),
                        data: None,
                        offset: 0,
                        deadline: Instant::now() + MAX_WAIT,
                    },
                );
            }
            Event::Cancelled { generation, id } => {
                if id == 0 || id > self.export_id || generation == 0 || generation > self.generation
                {
                    return Err(invalid("Unknown cancelled clipboard demand"));
                }
                // IDs are unique on the helper connection; old cancellations
                // cannot cancel a new generation's demand.
                self.exports.remove(&id);
            }
            Event::Ready { .. } => return Err(invalid("Repeated clipboard handshake")),
        }
        Ok(())
    }
    fn maintain(&mut self) -> io::Result<()> {
        let cancelled: Vec<_> = self
            .reads
            .iter()
            .filter_map(|(id, read)| {
                (read.reply.is_closed()
                    || !read.data.lease.current()
                    || Instant::now() >= read.deadline)
                    .then_some(*id)
            })
            .collect();
        for id in cancelled {
            self.send(Command::CancelLocal { id }, &[])?;
            self.reads.remove(&id);
        }
        if self.import.as_ref().is_some_and(|i| !i.current()) {
            let import = self.import.take().unwrap();
            self.exports.clear();
            self.send(
                Command::Revoke {
                    generation: import.generation,
                },
                &[],
            )?;
            if let Some(old) = self
                .snapshot
                .as_ref()
                .and_then(|s| s.origin)
                .filter(|g| *g != import.generation)
            {
                self.send(Command::Revoke { generation: old }, &[])?;
                self.invalidate();
            }
            if self
                .snapshot
                .as_ref()
                .is_some_and(|s| s.origin == Some(import.generation))
            {
                self.invalidate();
            }
        }
        let Some(import) = &self.import else {
            return Ok(());
        };
        let generation = import.generation;
        // One chunk per outstanding demand per turn. Source callbacks queue
        // work; this loop never waits for one, so event delivery keeps running.
        let mut finished = Vec::new();
        for (id, export) in &mut self.exports {
            if let Some(pending) = &export.pending
                && let Some(data) = pending.poll()
            {
                export.pending = None;
                export.data = data;
            }
            let expired = Instant::now() >= export.deadline || !import.current();
            let bytes = export.data.as_ref().and_then(|d| d.bytes());
            if expired
                || (export.pending.is_none() && bytes.is_none())
                || bytes.is_some_and(|b| b.len() > MAX_BYTES)
            {
                self.client.sender().send(
                    &Command::Failed {
                        generation,
                        id: *id,
                    },
                    &[],
                )?;
                finished.push(*id);
            } else if let Some(bytes) = bytes {
                if export.offset > bytes.len() {
                    return Err(invalid("Clipboard payload changed length"));
                }
                let end = export.offset.saturating_add(CHUNK).min(bytes.len());
                let eof = end == bytes.len();
                self.client.sender().send(
                    &Command::Data {
                        generation,
                        id: *id,
                        offset: export.offset,
                        eof,
                    },
                    &bytes[export.offset..end],
                )?;
                export.offset = end;
                if eof {
                    finished.push(*id);
                }
            }
        }
        for id in finished {
            self.exports.remove(&id);
        }
        Ok(())
    }
}
impl Drop for Session<'_> {
    fn drop(&mut self) {
        self.invalidate();
        self.import = None;
        self.exports.clear();
        self.reads.clear();
        self.snapshots.clear();
    }
}

#[cfg(test)]
mod publication_tests;
#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard::helper::{Sender, wire};
    use std::os::unix::net::UnixStream;
    use std::process::Command as Process;
    use std::sync::Mutex;

    // Exercise the parent protocol against a socket peer without AppKit or a
    // desktop. The short-lived child exists only to preserve Client ownership.
    pub(super) fn client() -> (Client, UnixStream) {
        let (stream, peer) = UnixStream::pair().unwrap();
        let sender = Sender(Arc::new(Mutex::new(stream.try_clone().unwrap())));
        let child = Process::new("/usr/bin/true").spawn().unwrap();
        (
            Client {
                child,
                stream,
                sender,
            },
            peer,
        )
    }
    pub(super) fn packet(event: Event, bytes: &[u8]) -> Packet {
        Packet {
            event,
            bytes: bytes.to_vec(),
        }
    }
    pub(super) fn offer(session: &mut Session<'_>) -> isize {
        session
            .event(packet(
                Event::Snapshot {
                    revision: 77,
                    formats: vec!["text/plain".into()],
                    origin: None,
                },
                &[],
            ))
            .unwrap();
        session.snapshot.as_ref().unwrap().revision
    }
    fn request(
        session: &mut Session<'_>,
        revision: isize,
    ) -> oneshot::Receiver<Option<Representation>> {
        let (reply, receive) = oneshot::channel();
        session
            .action(Action::Read {
                revision,
                format: "text/plain".into(),
                reply,
            })
            .unwrap();
        receive
    }
    #[test]
    fn owned_withdrawal_preserves_origin_but_a_later_local_copy_does_not() {
        struct Active;
        impl Source for Active {
            fn is_current(&self) -> bool {
                true
            }
            fn request(&self, _: &str, _: crate::clipboard::provider::Reply) {}
        }
        let (client, _peer) = client();
        let mut continuity = Continuity::default();
        let mut session = Session::new(
            client,
            &mut continuity,
            Arc::new(Resources::default()),
            Arc::new(AtomicBool::new(true)),
        );
        let expected = offer(&mut session);
        let (reply, _receive) = oneshot::channel();
        session
            .action(Action::Publish {
                expected,
                formats: vec!["text/plain".into()],
                source: Arc::new(Active),
                reply,
            })
            .unwrap();
        session
            .event(packet(
                Event::Published {
                    generation: 1,
                    revision: 78,
                },
                &[],
            ))
            .unwrap();
        session
            .event(packet(
                Event::Snapshot {
                    revision: 78,
                    formats: vec!["text/plain".into()],
                    origin: Some(1),
                },
                &[],
            ))
            .unwrap();
        session
            .event(packet(
                Event::Revoked {
                    generation: 1,
                    revision: Some(79),
                },
                &[],
            ))
            .unwrap();
        session
            .event(packet(
                Event::Snapshot {
                    revision: 79,
                    formats: vec![],
                    origin: None,
                },
                &[],
            ))
            .unwrap();
        assert_eq!(session.snapshot.as_ref().unwrap().origin, Some(1));
        session
            .event(packet(
                Event::Snapshot {
                    revision: 80,
                    formats: vec![],
                    origin: None,
                },
                &[],
            ))
            .unwrap();
        assert_eq!(session.snapshot.as_ref().unwrap().origin, None);
        // A late duplicate acknowledgment must not classify an unrelated copy.
        session
            .event(packet(
                Event::Revoked {
                    generation: 1,
                    revision: Some(79),
                },
                &[],
            ))
            .unwrap();
        session
            .event(packet(
                Event::Snapshot {
                    revision: 80,
                    formats: vec!["text/plain".into()],
                    origin: None,
                },
                &[],
            ))
            .unwrap();
        assert_eq!(session.snapshot.as_ref().unwrap().origin, None);
        let (reply, _receive_second) = oneshot::channel();
        session
            .action(Action::Publish {
                expected: session.snapshot.as_ref().unwrap().revision,
                formats: vec!["text/plain".into()],
                source: Arc::new(Active),
                reply,
            })
            .unwrap();
        session
            .event(packet(
                Event::Published {
                    generation: 2,
                    revision: 81,
                },
                &[],
            ))
            .unwrap();
        session
            .event(packet(
                Event::Snapshot {
                    revision: 81,
                    formats: vec!["text/plain".into()],
                    origin: Some(2),
                },
                &[],
            ))
            .unwrap();
        session
            .event(packet(
                Event::Revoked {
                    generation: 1,
                    revision: Some(79),
                },
                &[],
            ))
            .unwrap();
        assert_eq!(
            session.continuity.origin,
            Some((81, 2)),
            "late withdrawal erased a newer publication's provenance"
        );
    }

    #[test]
    fn abandoned_reads_cancel_and_late_chunks_cannot_fulfill_a_new_request() {
        let (client, mut peer) = client();
        let mut continuity = Continuity::default();
        let resources = Arc::new(Resources::default());
        let mut session = Session::new(
            client,
            &mut continuity,
            resources.clone(),
            Arc::new(AtomicBool::new(true)),
        );
        let revision = offer(&mut session);
        drop(request(&mut session, revision));
        assert!(matches!(
            wire::receive::<Command>(&mut peer, TICK)
                .unwrap()
                .unwrap()
                .header,
            Command::ReadLocal { id: 1, .. }
        ));
        session.maintain().unwrap();
        assert!(matches!(
            wire::receive::<Command>(&mut peer, TICK)
                .unwrap()
                .unwrap()
                .header,
            Command::CancelLocal { id: 1 }
        ));
        assert_eq!(resources.slots.load(Ordering::Acquire), 0);
        let mut second = request(&mut session, revision);
        session
            .event(packet(
                Event::LocalData {
                    id: 1,
                    revision: 77,
                    offset: 0,
                    eof: true,
                },
                b"old",
            ))
            .unwrap();
        assert!(second.try_recv().is_err());
        session
            .event(packet(
                Event::LocalData {
                    id: 2,
                    revision: 77,
                    offset: 0,
                    eof: true,
                },
                b"new",
            ))
            .unwrap();
        let data = second.try_recv().unwrap().unwrap();
        assert_eq!(data.bytes(), Some(&b"new"[..]));
        assert_eq!(resources.bytes.load(Ordering::Acquire), 3);
        drop(session);
        assert!(data.bytes().is_none());
        drop(data);
        assert_eq!(resources.bytes.load(Ordering::Acquire), 0);
        assert_eq!(resources.slots.load(Ordering::Acquire), 0);
    }
    #[test]
    fn malformed_offsets_and_future_ids_fail_instead_of_joining_payloads() {
        let (client, _peer) = client();
        let mut continuity = Continuity::default();
        let resources = Arc::new(Resources::default());
        let mut session = Session::new(
            client,
            &mut continuity,
            resources.clone(),
            Arc::new(AtomicBool::new(true)),
        );
        let revision = offer(&mut session);
        let mut receive = request(&mut session, revision);
        session
            .event(packet(
                Event::LocalData {
                    id: 1,
                    revision: 77,
                    offset: 0,
                    eof: false,
                },
                b"a",
            ))
            .unwrap();
        assert!(
            session
                .event(packet(
                    Event::LocalData {
                        id: 1,
                        revision: 77,
                        offset: 0,
                        eof: true
                    },
                    b"b"
                ))
                .is_err()
        );
        assert!(
            session
                .event(packet(Event::LocalFailed { id: 2 }, &[]))
                .is_err()
        );
        assert!(
            session
                .event(packet(
                    Event::LocalData {
                        id: 1,
                        revision: 78,
                        offset: 1,
                        eof: true
                    },
                    b"b"
                ))
                .is_err()
        );
        assert!(receive.try_recv().is_err());
        drop(session);
        assert_eq!(resources.bytes.load(Ordering::Acquire), 0);
        assert_eq!(resources.slots.load(Ordering::Acquire), 0);
        assert!(receive.try_recv().is_err());
    }
}
