use super::super::read_source;
use super::*;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
use tokio::io::{AsyncRead, AsyncWrite};
use wire::{Message, Outcome, Packet};

/// The helper connection: outgoing packets, incoming packets, and the reader
/// and writer tasks.
pub(super) type Channels = (
    mpsc::Sender<Packet>,
    mpsc::Receiver<Result<Packet, String>>,
    tokio::task::JoinHandle<()>,
    tokio::task::JoinHandle<()>,
);
/// A payload read for the helper, with the reply that announces it.
type Demand = tokio::task::JoinHandle<(Packet, Option<Box<dyn Payload>>)>;

pub(super) fn channels<T>(stream: T, mut decoder: wire::Reader) -> Channels
where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    let (send, mut outgoing) = mpsc::channel::<Packet>(SLOTS);
    let (incoming, receive) = mpsc::channel(2);
    let input = incoming.clone();
    let read = tokio::spawn(async move {
        loop {
            let packet = decoder.read(&mut reader).await.map_err(|e| e.to_string());
            let failed = packet.is_err();
            if input.send(packet).await.is_err() || failed {
                break;
            }
        }
    });
    let write = tokio::spawn(async move {
        while let Some(packet) = outgoing.recv().await {
            if let Err(error) = wire::write(&mut writer, packet).await {
                let _ = incoming.send(Err(error.to_string())).await;
                break;
            }
        }
    });
    (send, receive, read, write)
}
fn send(sender: &mpsc::Sender<Packet>, message: impl Into<Packet>) -> Result<(), String> {
    sender
        .try_send(message.into())
        .map_err(|_| "Clipboard helper IPC queue exhausted".into())
}
struct Pending {
    id: u64,
    action: Action,
    deadline: Instant,
}
struct Host {
    imports: HashMap<u64, Arc<Import>>,
    generations: HashMap<u64, u64>,
    snapshot: Option<Snapshot>,
    pending: Option<Pending>,
    next: u64,
    claimed: bool,
    running: Arc<AtomicBool>,
    resources: Arc<Resources>,
    retained: HashMap<u64, Files>,
}
impl Host {
    fn command(&mut self, action: Action) -> Option<Message> {
        self.next = self.next.checked_add(1)?;
        let id = self.next;
        let message = match &action {
            Action::Snapshot(reply) if !reply.is_closed() => Message::Snapshot { id },
            Action::Read {
                revision,
                format,
                reply,
            } if !reply.is_closed()
                && self.snapshot.as_ref().is_some_and(|s| {
                    s.revision == *revision
                        && s.is_current()
                        && s.origin.is_none()
                        && s.formats.contains(format)
                }) =>
            {
                Message::Read {
                    id,
                    revision: wire::revision(*revision),
                    format: format.clone(),
                }
            }
            Action::Publish {
                expected,
                formats,
                source,
                attempt,
                reply,
            } if self.imports.len() + self.retained.len() < SLOTS
                && !reply.is_closed()
                && !attempt.canceled()
                && source.is_current()
                && formats.len() <= super::super::MAX_FORMATS =>
            {
                self.imports.insert(
                    id,
                    Arc::new(Import {
                        active: AtomicBool::new(true),
                        released: AtomicBool::new(false),
                        notified: AtomicBool::new(false),
                        source: source.clone(),
                        owner: AtomicUsize::new(0),
                        attachment: Attachment::new(None),
                        files: Mutex::new(None),
                    }),
                );
                Message::Publish {
                    id,
                    expected: wire::revision(*expected),
                    formats: formats.clone(),
                }
            }
            Action::Attach {
                generation,
                files,
                attempt,
                reply,
            } if !reply.is_closed() && !attempt.canceled() => {
                let import = self.imports.get(self.generations.get(generation)?)?;
                if !import.current()
                    || import
                        .files
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .is_some()
                {
                    // A claimed file copy is never replayed or replaced by another lease.
                    return None;
                }
                let folder = wire::path(files.folder()).ok()?;
                // Parent pins the cache lease before the worker can expose its paths.
                *import
                    .files
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(files.clone());
                Message::Attach {
                    id,
                    generation: *generation,
                    folder,
                }
            }
            _ => return None,
        };
        self.pending = Some(Pending {
            id,
            action,
            deadline: Instant::now() + MAX_WAIT,
        });
        Some(message)
    }
    fn admit(&mut self, source: u64, preparation: bool) -> bool {
        let Some(pending) = &self.pending else {
            return false;
        };
        let Some(import) = self.imports.get(&source) else {
            return false;
        };
        let attempt = match &pending.action {
            Action::Publish { attempt, reply, .. }
                if pending.id == source && !reply.is_closed() =>
            {
                attempt
            }
            Action::Attach {
                generation,
                attempt,
                reply,
                ..
            } if !preparation
                && !reply.is_closed()
                && self.generations.get(generation) == Some(&source) =>
            {
                attempt
            }
            _ => return false,
        };
        let attaching = matches!(pending.action, Action::Attach { .. });
        let confirm = || {
            import.active.load(Ordering::Acquire)
                && import.source.is_current()
                && if preparation {
                    import.source.preparation_confirmed()
                } else {
                    import.source.publication_confirmed()
                }
        };
        let accepted = attempt.admit(|| {
            if attaching {
                import.attachment.claim(confirm)
            } else {
                confirm()
            }
        });
        if accepted && !preparation {
            self.claimed = true;
        }
        accepted
    }
    fn complete(&mut self, id: u64, outcome: Option<Outcome>, data: Vec<u8>) -> Result<(), String> {
        let Some(pending) = self.pending.take().filter(|pending| pending.id == id) else {
            return Err("Unexpected clipboard helper receipt".into());
        };
        match pending.action {
            Action::Snapshot(reply) => {
                let result = if let Some(Outcome::Snapshot {
                    revision,
                    identity,
                    formats,
                    origin,
                }) = outcome
                {
                    let revision =
                        wire::native_revision(revision).map_err(|error| error.to_string())?;
                    let identity = wire::native_identity(identity);
                    if formats.len() > super::super::MAX_FORMATS
                        || formats.iter().any(|f| f.len() > 256)
                    {
                        return Err("Invalid clipboard formats".into());
                    }
                    if identity.matches() && eligible(identity) {
                        if let Some(previous) = self
                            .snapshot
                            .as_ref()
                            .filter(|s| s.revision == revision && s.lease.identity == identity)
                        {
                            let _ = reply.send(Some(previous.clone()));
                            return Ok(());
                        }
                        if let Some(previous) = self.snapshot.take() {
                            previous.lease.valid.store(false, Ordering::Release);
                        }
                        let snapshot = Snapshot {
                            revision,
                            formats,
                            origin,
                            lease: Arc::new(Lease {
                                identity,
                                valid: AtomicBool::new(true),
                                running: self.running.clone(),
                            }),
                        };
                        self.snapshot = Some(snapshot.clone());
                        Some(snapshot)
                    } else {
                        None
                    }
                } else {
                    None
                };
                let _ = reply.send(result);
            }
            Action::Read {
                revision, reply, ..
            } => {
                let result = if let Some(Outcome::Read {
                    revision: observed,
                    identity,
                }) = outcome
                {
                    let observed =
                        wire::native_revision(observed).map_err(|error| error.to_string())?;
                    let identity = wire::native_identity(identity);
                    self.snapshot
                        .as_ref()
                        .filter(|s| {
                            s.revision == revision
                                && observed == revision
                                && s.lease.identity == identity
                                && s.is_current()
                        })
                        .and_then(|snapshot| {
                            if self.resources.slots.load(Ordering::Acquire) >= SLOTS
                                || self
                                    .resources
                                    .bytes
                                    .load(Ordering::Acquire)
                                    .checked_add(data.len())
                                    .is_none_or(|n| n > MAX_BYTES)
                            {
                                return None;
                            }
                            self.resources.slots.fetch_add(1, Ordering::AcqRel);
                            self.resources.bytes.fetch_add(data.len(), Ordering::AcqRel);
                            Some(Representation {
                                bytes: data,
                                lease: snapshot.lease.clone(),
                                resources: self.resources.clone(),
                            })
                        })
                } else {
                    None
                };
                let _ = reply.send(result);
            }
            Action::Publish { reply, .. } => {
                let Some(import) = self.imports.get(&id).cloned() else {
                    return Err("Missing publication source".into());
                };
                if let Some(Outcome::Published {
                    generation,
                    owner,
                    preparation,
                }) = outcome
                {
                    import.owner.store(
                        usize::try_from(owner).map_err(|_| "Invalid clipboard owner")?,
                        Ordering::Release,
                    );
                    import.attachment.prepare(
                        preparation
                            .map(|(revision, identity)| {
                                Ok::<_, std::io::Error>((
                                    wire::native_revision(revision)?,
                                    wire::native_identity(identity),
                                ))
                            })
                            .transpose()
                            .map_err(|error| error.to_string())?,
                    );
                    self.generations.insert(generation, id);
                    #[cfg(test)]
                    super::tests::after_native_close();
                    let _ = reply.send(Some(Publication {
                        generation,
                        import,
                        running: self.running.clone(),
                    }));
                    self.claimed = false;
                } else {
                    if let Some(Outcome::PublicationRejected(rejection)) = outcome {
                        log::warn!("Windows clipboard publication rejected: {rejection:?}");
                        #[cfg(test)]
                        eprintln!("WINDOWS_CLIPBOARD_PUBLICATION_REJECTED {rejection:?}");
                    }
                    import.revoke();
                    if self.claimed
                        && let Some(identity) = Identity::current()
                    {
                        *UNCERTAIN
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(identity);
                    }
                }
            }
            Action::Attach {
                generation, reply, ..
            } => {
                let result = match outcome {
                    Some(Outcome::Attached { result }) => {
                        wire::native_receipt(result).map_err(|error| error.to_string())?
                    }
                    _ => FileWrite::Uncertain,
                };
                let import = self
                    .generations
                    .get(&generation)
                    .and_then(|source| self.imports.get(source));
                if let Some(import) = import {
                    import.attachment.settle(result);
                    if result == FileWrite::Unchanged {
                        // Only a native no-mutation receipt permits releasing this lease.
                        import
                            .files
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .take();
                        if matches!(import.attachment.stage(), AttachmentStage::Canceled) {
                            import.revoke();
                        }
                    }
                }
                match result {
                    FileWrite::Committed { revision, complete } => {
                        self.claimed = false;
                        let _ = reply.send(complete.then_some(revision));
                    }
                    FileWrite::Unchanged => {
                        self.claimed = false;
                        let _ = reply.send(None);
                    }
                    FileWrite::Uncertain => {
                        // Stop/reap before any later operation can reinterpret this copy.
                        self.claimed = true;
                        let _ = reply.send(None);
                        return Err("Clipboard file commit has no reliable native receipt".into());
                    }
                }
            }
        }
        Ok(())
    }
}
pub(super) async fn run(
    executable: WorkerExecutable,
    running: Arc<AtomicBool>,
    mut actions: mpsc::Receiver<Action>,
) -> Result<(), String> {
    if !running.load(Ordering::Acquire) {
        return Ok(());
    }
    let (mut child, mut pipe) = process::spawn(&executable)
        .await
        .map_err(|e| e.to_string())?;
    let mut decoder = wire::Reader::new();
    let nonce = uuid::Uuid::new_v4().to_string();
    let handshake = tokio::time::timeout(Duration::from_secs(3), async {
        wire::write(
            &mut pipe,
            Message::Hello {
                version: wire::VERSION,
                nonce: nonce.clone(),
            }
            .into(),
        )
        .await
        .map_err(|e| e.to_string())?;
        match decoder
            .read(&mut pipe)
            .await
            .map_err(|e| e.to_string())?
            .message
        {
            Message::Ready {
                version,
                nonce: echoed,
            } if version == wire::VERSION && echoed == nonce => Ok(()),
            _ => Err("Clipboard helper handshake mismatch".into()),
        }
    })
    .await
    .map_err(|_| "Clipboard helper handshake timed out".to_owned())
    .and_then(|r| r);
    if let Err(error) = handshake {
        drop(pipe);
        if let Err(cleanup) = child.finish().await {
            process::quarantine(child, Vec::new());
            return Err(cleanup);
        }
        return Err(error);
    }
    let (sender, mut receive, reader, writer) = channels(pipe, decoder);
    let mut host = Host {
        imports: HashMap::new(),
        generations: HashMap::new(),
        snapshot: None,
        pending: None,
        next: 0,
        claimed: false,
        running: running.clone(),
        resources: Arc::default(),
        retained: HashMap::new(),
    };
    let mut demand: Option<Demand> = None;
    let mut granted: Option<(u64, Instant, Box<dyn Payload>)> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(10));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let result = async {
        while running.load(Ordering::Acquire) {
            tokio::select! {
                packet = receive.recv() => {
                    let packet = packet.ok_or("Clipboard helper disconnected")??;
                    match packet.message {
                        Message::Done { id, outcome } => host.complete(id, outcome, packet.data)?,
                        Message::Admit { id, source, preparation } => { let accepted = host.admit(source, preparation); send(&sender, Message::Admitted { id, accepted })?; }
                        Message::Revoked { source } => {
                            if let Some(import) = host.imports.get(&source) { import.released.store(true, Ordering::Release); import.revoke(); }
                            host.retained.remove(&source);
                        }
                        Message::Consumed { id } => {
                            if granted.as_ref().is_none_or(|(expected, _, _)| *expected != id) { return Err("Unexpected clipboard payload consumption".into()); }
                            granted = None;
                        }
                        Message::Demand { id, source, format } => {
                            if demand.is_some() || granted.is_some() || format.len() > 256 { send(&sender, Message::Data { id, present: false })?; continue; }
                            let import = host.imports.get(&source).cloned();
                            demand = Some(tokio::spawn(async move {
                                let payload = match import { Some(import) if import.active.load(Ordering::Acquire) && import.source.is_current() => read_source(&*import.source, &format).await, _ => None };
                                let bytes = payload.as_ref().and_then(|data| data.bytes()).filter(|b| b.len() <= MAX_BYTES).map(|b| b.to_vec());
                                (Packet { message: Message::Data { id, present: bytes.is_some() }, data: bytes.unwrap_or_default() }, payload)
                            }));
                        }
                        _ => return Err("Unexpected clipboard helper message".into()),
                    }
                }
                action = actions.recv(), if host.pending.is_none() => {
                    let Some(action) = action else { break; };
                    if let Some(command) = host.command(action) { send(&sender, command)?; }
                }
                _ = tick.tick() => {
                    if host.pending.as_ref().is_some_and(|p| Instant::now() >= p.deadline) { return Err("Clipboard helper operation timed out".into()); }
                    let obsolete: Vec<_> = host.imports.iter().filter(|(_, import)| !import.active.load(Ordering::Acquire) || !import.source.is_current()).map(|(id, _)| *id).collect();
                    for source in obsolete {
                        if let Some(import) = host.imports.get(&source) {
                            import.revoke();
                            if !import.notified.swap(true, Ordering::AcqRel) { send(&sender, Message::Revoke { source })?; }
                        }
                        let pending = host.pending.as_ref().is_some_and(|pending| match &pending.action {
                            Action::Publish { .. } => pending.id == source,
                            Action::Attach { generation, .. } => host.generations.get(generation) == Some(&source),
                            _ => false,
                        });
                        if pending { continue; }
                        if let Some(import) = host.imports.remove(&source)
                            && !import.released.load(Ordering::Acquire)
                                && let Some(files) = import.files.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone() { host.retained.insert(source, files); }
                        host.generations.retain(|_, id| *id != source);
                    }
                    if granted.as_ref().is_some_and(|(_, deadline, _)| Instant::now() >= *deadline) { return Err("Clipboard payload consumption timed out".into()); }
                    if demand.as_ref().is_some_and(|task| task.is_finished()) {
                        let (mut packet, payload) = demand.take().unwrap().await.map_err(|e| e.to_string())?;
                        let Message::Data { id, present } = &mut packet.message else { unreachable!() };
                        if let Some(payload) = payload.filter(|payload| payload.bytes().is_some()) {
                            if *present { granted = Some((*id, Instant::now() + MAX_WAIT, payload)); }
                        } else { *present = false; packet.data.clear(); }
                        send(&sender, packet)?;
                    }
                }
            }
        }
        Ok(())
    }.await;
    #[cfg(test)]
    if result.is_err() {
        eprintln!("WINDOWS_CLIPBOARD_SESSION_FAILED");
    }
    running.store(false, Ordering::Release);
    if let Some(task) = demand {
        task.abort();
        let _ = task.await;
    }
    let _ = send(&sender, Message::Stop);
    // Retain imports and typed file leases until this exact child is reaped.
    let owned = host.imports.values().any(|import| {
        let owner = import.owner.load(Ordering::Acquire);
        owner != 0 && clipboard_owner() == owner
    });
    for import in host.imports.values() {
        import.revoke();
    }
    let before = Identity::current();
    let cleanup = child.finish().await;
    if (host.claimed || owned)
        && let Some(identity) = Identity::current().or(before)
    {
        *UNCERTAIN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(identity);
    }
    reader.abort();
    writer.abort();
    let _ = reader.await;
    let _ = writer.await;
    if let Err(error) = cleanup {
        let mut files: Vec<_> = host.retained.into_values().collect();
        files.extend(host.imports.values().filter_map(|import| {
            import
                .files
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }));
        process::quarantine(child, files);
        return Err(error);
    }
    result
}
