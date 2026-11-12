//! Worker-only native composition. Parent source callbacks cross the private pipe;
//! no profile, transport, credentials, or GUI runtime is initialized here.
use super::super::Reply as SourceReply;
use super::*;
use std::{collections::HashMap, sync::mpsc as blocking, time::Duration};
use tokio::net::windows::named_pipe::NamedPipeClient;
use wire::{Message, Outcome, Packet};
enum Event {
    Admit {
        source: u64,
        preparation: bool,
        reply: blocking::SyncSender<bool>,
    },
    Demand {
        source: u64,
        format: String,
        reply: SourceReply,
    },
    Revoked(u64),
    Consumed(u64),
    #[cfg(test)]
    SweepPublications {
        generation: u64,
        reply: blocking::SyncSender<bool>,
    },
    Done {
        id: u64,
        outcome: Option<Outcome>,
        data: Vec<u8>,
        publication: Option<native::Publication>,
    },
}
struct Proxy {
    id: u64,
    current: Arc<AtomicBool>,
    events: mpsc::Sender<Event>,
}
impl Proxy {
    fn admit(&self, preparation: bool) -> bool {
        if !self.is_current() {
            return false;
        }
        let (reply, receive) = blocking::sync_channel(1);
        if self
            .events
            .try_send(Event::Admit {
                source: self.id,
                preparation,
                reply,
            })
            .is_err()
        {
            return false;
        }
        receive.recv_timeout(MAX_WAIT).unwrap_or(false) && self.is_current()
    }
}
impl Source for Proxy {
    fn is_current(&self) -> bool {
        self.current.load(Ordering::Acquire)
    }
    fn request(&self, format: &str, reply: SourceReply) {
        if self.is_current() {
            let _ = self.events.try_send(Event::Demand {
                source: self.id,
                format: format.into(),
                reply,
            });
        }
    }
    fn publication_confirmed(&self) -> bool {
        self.admit(false)
    }
    fn preparation_confirmed(&self) -> bool {
        self.admit(true)
    }
    fn publication_revoked(&self) {
        if self.current.swap(false, Ordering::AcqRel) {
            let _ = self.events.try_send(Event::Revoked(self.id));
        }
    }
}
struct Bytes {
    bytes: Vec<u8>,
    source: Arc<AtomicBool>,
    id: u64,
    events: mpsc::Sender<Event>,
}
impl Drop for Bytes {
    fn drop(&mut self) {
        let _ = self.events.try_send(Event::Consumed(self.id));
    }
}
impl Payload for Bytes {
    fn bytes(&self) -> Option<&[u8]> {
        self.source.load(Ordering::Acquire).then_some(&self.bytes)
    }
}
fn send(sender: &mpsc::Sender<Packet>, packet: impl Into<Packet>) -> Result<(), String> {
    sender
        .try_send(packet.into())
        .map_err(|_| "Clipboard worker IPC queue exhausted".into())
}
fn retain_publications(publications: &mut HashMap<u64, native::Publication>) {
    publications.retain(|_, publication| publication.is_current());
}
async fn send_completion(
    sender: &mpsc::Sender<Packet>,
    operation: &mut Option<tokio::task::JoinHandle<()>>,
    packet: Packet,
) -> Result<(), String> {
    // Receiving Done proves the native future finished, but its task may still
    // be returning from send on another executor thread. Retire it before the
    // parent can acknowledge this result and submit another command.
    operation
        .take()
        .ok_or("Missing clipboard operation")?
        .await
        .map_err(|error| error.to_string())?;
    send(sender, packet)
}
pub(super) async fn run(mut pipe: NamedPipeClient, open: OpenFiles) -> Result<(), String> {
    let mut decoder = wire::Reader::new();
    let hello = tokio::time::timeout(Duration::from_secs(3), decoder.read(&mut pipe))
        .await
        .map_err(|_| "Clipboard worker handshake timed out")?
        .map_err(|e| e.to_string())?;
    let Message::Hello { version, nonce } = hello.message else {
        return Err("Missing clipboard worker handshake".into());
    };
    if version != wire::VERSION || uuid::Uuid::parse_str(&nonce).is_err() {
        return Err("Invalid clipboard worker handshake".into());
    }
    wire::write(&mut pipe, Message::Ready { version, nonce }.into())
        .await
        .map_err(|e| e.to_string())?;
    let (sender, mut receive, reader, writer) = session::channels(pipe, decoder);
    let (events, mut event) = mpsc::channel(SLOTS);
    #[cfg(test)]
    {
        let events = events.clone();
        native::set_file_transition_hook_for_fixture(Box::new(move |generation| {
            let (reply, receipt) = blocking::sync_channel(1);
            events
                .try_send(Event::SweepPublications { generation, reply })
                .is_ok()
                && receipt
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap_or(false)
        }));
    }
    let owner = native::Owner::new();
    let mut publications: HashMap<u64, native::Publication> = HashMap::new();
    let mut sources: HashMap<u64, Arc<Proxy>> = HashMap::new();
    let mut admissions: HashMap<u64, blocking::SyncSender<bool>> = HashMap::new();
    let mut demands: HashMap<u64, (Arc<AtomicBool>, SourceReply)> = HashMap::new();
    let mut next = 0u64;
    let mut latest = 0u64;
    let mut operation = None::<tokio::task::JoinHandle<()>>;
    let mut tick = tokio::time::interval(Duration::from_millis(10));
    let result = async {
        loop {
            tokio::select! {
                packet = receive.recv() => {
                    let packet = packet.ok_or("Clipboard host disconnected")??;
                    match packet.message {
                        Message::Stop => break,
                        Message::Admitted { id, accepted } => { let reply = admissions.remove(&id).ok_or("Unknown admission receipt")?; let _ = reply.send(accepted); }
                        Message::Data { id, present } => {
                            if let Some((source, reply)) = demands.remove(&id) {
                                if present {
                                    if source.load(Ordering::Acquire) && !reply.is_cancelled() { reply.complete(Box::new(Bytes { bytes: packet.data, source, id, events: events.clone() })); }
                                    else { send(&sender, Message::Consumed { id })?; }
                                }
                            } else if present { send(&sender, Message::Consumed { id })?; }
                        }
                        Message::Revoke { source } => {
                            if let Some(source) = sources.remove(&source) { source.current.store(false, Ordering::Release); }
                            retain_publications(&mut publications);
                            send(&sender, Message::Revoked { source })?;
                        }
                        command @ (Message::Snapshot { .. } | Message::Read { .. } | Message::Publish { .. } | Message::Attach { .. }) => {
                            if operation.is_some() { return Err("Concurrent clipboard worker operation".into()); }
                            let owner = owner.clone(); let events = events.clone();
                            let (id, future): (u64, std::pin::Pin<Box<dyn std::future::Future<Output = Event> + Send>>) = match command {
                                Message::Snapshot { id } => (id, Box::pin(async move {
                                    let outcome = owner.snapshot().await.map(|snapshot| {
                                        let (sequence, owner) = snapshot.identity();
                                        Outcome::Snapshot { revision: wire::revision(snapshot.revision), identity: wire::identity(Identity { sequence, owner: wire::owner(owner) }), formats: snapshot.formats, origin: snapshot.origin }
                                    });
                                    Event::Done { id, outcome, data: Vec::new(), publication: None }
                                })),
                                Message::Read { id, revision, format } => {let revision=wire::native_revision(revision).map_err(|error|error.to_string())?; (id, Box::pin(async move {
                                    let snapshot = owner.snapshot().await.filter(|s| s.revision == revision);
                                    let value = owner.read(revision, format).await;
                                    let result = snapshot.filter(|s| s.is_current()).and_then(|snapshot| {
                                        let data = value.as_ref()?.bytes()?.to_vec();
                                        let (sequence, owner) = snapshot.identity();
                                        Some((Outcome::Read { revision: wire::revision(revision), identity: wire::identity(Identity { sequence, owner: wire::owner(owner) }) }, data))
                                    });
                                    let (outcome, data) = result.map_or((None, Vec::new()), |(outcome, data)| (Some(outcome), data));
                                    Event::Done { id, outcome, data, publication: None }
                                }))},
                                Message::Publish { id, expected, formats } => {
                                    let expected=wire::native_revision(expected).map_err(|error|error.to_string())?;
                                    if sources.len() >= SLOTS || formats.len() > super::super::MAX_FORMATS || formats.iter().any(|f| f.len() > 256) { return Err("Clipboard publication budget exceeded".into()); }
                                    let source = Arc::new(Proxy { id, current: Arc::new(AtomicBool::new(true)), events: events.clone() });
                                    sources.insert(id, source.clone());
                                    (id, Box::pin(async move {
                                        let (publication, rejection) = owner.publish(expected, formats, source).await;
                                        let outcome = Some(publication.as_ref().map_or(Outcome::PublicationRejected(rejection), |publication| Outcome::Published {
                                            generation: publication.generation,
                                            owner: wire::owner(publication.owner()),
                                            preparation: publication.preparation().map(|(revision, (sequence, owner))| (wire::revision(revision), wire::identity(Identity { sequence, owner: wire::owner(owner) }))),
                                        }));
                                        Event::Done { id, outcome, data: Vec::new(), publication }
                                    }))
                                }
                                Message::Attach { id, generation, folder } => {let folder=wire::native_path(folder).map_err(|error|error.to_string())?; (id, Box::pin(async move {
                                    let files = tokio::task::spawn_blocking(move || open(&folder)).await.ok().and_then(Result::ok);
                                    let result = match files { Some(files) => owner.attach_files(generation, files).await, None => FileWrite::Unchanged };
                                    Event::Done { id, outcome: Some(Outcome::Attached { result: wire::receipt(result) }), data: Vec::new(), publication: None }
                                }))},
                                _ => unreachable!(),
                            };
                            if id <= latest { return Err("Replayed clipboard command".into()); } latest = id;
                            let events = events.clone();
                            operation = Some(tokio::spawn(async move { let result = future.await; let _ = events.send(result).await; }));
                        }
                        _ => return Err("Unexpected clipboard host message".into()),
                    }
                }
                event = event.recv() => {
                    match event.ok_or("Clipboard native owner disconnected")? {
                        Event::Admit { source, preparation, reply } => {
                            if admissions.len() >= SLOTS { return Err("Clipboard admission budget exceeded".into()); }
                            next = next.checked_add(1).ok_or("Clipboard request sequence exhausted")?;
                            admissions.insert(next, reply); send(&sender, Message::Admit { id: next, source, preparation })?;
                        }
                        Event::Demand { source, format, reply } => {
                            if demands.len() >= SLOTS { continue; }
                            let Some(proxy) = sources.get(&source).filter(|source| source.is_current()) else { continue; };
                            next = next.checked_add(1).ok_or("Clipboard request sequence exhausted")?;
                            demands.insert(next, (proxy.current.clone(), reply)); send(&sender, Message::Demand { id: next, source, format })?;
                        }
                        Event::Revoked(source) => { send(&sender, Message::Revoked { source })?; }
                        Event::Consumed(id) => { send(&sender, Message::Consumed { id })?; }
                        #[cfg(test)]
                        Event::SweepPublications { generation, reply } => {
                            retain_publications(&mut publications);
                            let _ = reply.send(publications.contains_key(&generation));
                        }
                        Event::Done { id, outcome, data, publication } => {
                            if let Some(publication) = publication { publications.insert(publication.generation, publication); }
                            send_completion(&sender, &mut operation, Packet { message: Message::Done { id, outcome }, data }).await?;
                        }
                    }
                }
                _ = tick.tick() => {
                    demands.retain(|_, (source, reply)| source.load(Ordering::Acquire) && !reply.is_cancelled());
                    sources.retain(|_, source| source.is_current());
                    retain_publications(&mut publications);
                }
            }
        }
        Ok(())
    }.await;
    for source in sources.values() {
        source.current.store(false, Ordering::Release);
    }
    owner.close();
    if let Some(operation) = operation {
        operation.abort();
        let _ = operation.await;
    }
    reader.abort();
    writer.abort();
    let _ = reader.await;
    let _ = writer.await;
    // The parent owns bounded termination/reap, including native threads blocked
    // in another process. This worker never tries to terminate a Rust thread.
    result
}

#[cfg(test)]
mod tests {
    use super::{Message, Packet, send_completion};
    use std::time::Duration;

    #[tokio::test]
    async fn acknowledgment_waits_until_the_native_operation_task_retires() {
        let (release, wait) = tokio::sync::oneshot::channel::<()>();
        let mut operation = Some(tokio::spawn(async move {
            let _ = wait.await;
        }));
        let (sender, mut receive) = tokio::sync::mpsc::channel(1);
        let completion = tokio::spawn(async move {
            send_completion(
                &sender,
                &mut operation,
                Packet::from(Message::Done {
                    id: 1,
                    outcome: None,
                }),
            )
            .await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(50), receive.recv())
                .await
                .is_err(),
            "A next command must not become eligible while the preceding task is live"
        );
        release.send(()).unwrap();
        let packet = tokio::time::timeout(Duration::from_secs(1), receive.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            packet.message,
            Message::Done {
                id: 1,
                outcome: None
            }
        ));
        completion.await.unwrap().unwrap();
    }
}
