//! Bounded host demand over the existing integration stream. Blocking native
//! payload waits run on cancellable tasks; they never hold the integration loop.
use crate::{
    clipboard_service::{Chunk, CopyLease, CopySession, ExportOffer, ReadCancellation, Reader},
    runtime_protocol::clipboard_export::{self as protocol, Control, Event},
};
use std::{collections::BTreeMap, io, sync::mpsc, thread};

struct Transfer {
    committed: bool,
    cancellation: ReadCancellation,
    credit: Option<mpsc::SyncSender<()>>,
    chunks: mpsc::Receiver<io::Result<Chunk>>,
    task: Option<thread::JoinHandle<()>>,
    offset: usize,
}
enum Authority {
    Focused(ExportOffer),
    Committed(CopyLease),
}
impl Transfer {
    fn new(reader: Reader, authority: Authority, format: String) -> io::Result<Self> {
        let committed = matches!(&authority, Authority::Committed(_));
        let cancellation = ReadCancellation::default();
        let cancel = cancellation.clone();
        let (credit, input) = mpsc::sync_channel(1);
        let (output, chunks) = mpsc::sync_channel(1);
        let task = thread::Builder::new()
            .name("clipboard-export".into())
            .spawn(move || {
                let mut read = match authority {
                    Authority::Focused(offer) => reader.read_export(&offer, &format, cancel),
                    Authority::Committed(copy) => reader.read_committed(&copy, &format, cancel),
                };
                while input.recv().is_ok() {
                    let chunk = match &mut read {
                        Ok(read) => read.next(protocol::CHUNK),
                        Err(error) => Err(io::Error::other(error.to_string())),
                    };
                    let finished = chunk.as_ref().map_or(true, |chunk| chunk.eof);
                    if output.send(chunk).is_err() || finished {
                        break;
                    }
                }
            })?;
        // Exactly one credit is issued for every drained result.
        let _ = credit.try_send(());
        Ok(Self {
            committed,
            cancellation,
            credit: Some(credit),
            chunks,
            task: Some(task),
            offset: 0,
        })
    }
    fn cancel(&mut self) {
        self.cancellation.cancel();
        self.credit.take();
    }
    fn finished(&self) -> bool {
        self.task
            .as_ref()
            .is_none_or(thread::JoinHandle::is_finished)
    }
}
impl Drop for Transfer {
    fn drop(&mut self) {
        self.cancel();
        if let Some(task) = self.task.take() {
            let _ = task.join();
        }
    }
}

pub(crate) struct Relay {
    reader: Reader,
    enabled: bool,
    version: u32,
    copies: Option<CopySession>,
    copy: Option<CopyLease>,
    announced: bool,
    offer: Option<ExportOffer>,
    last_id: u64,
    last_sent: u64,
    reads: BTreeMap<u64, Transfer>,
    retiring: Vec<Transfer>,
}
impl Relay {
    pub(crate) fn new(reader: Reader) -> Self {
        Self {
            reader,
            enabled: false,
            version: protocol::VERSION,
            copies: None,
            copy: None,
            announced: false,
            offer: None,
            last_id: 0,
            last_sent: 0,
            reads: BTreeMap::new(),
            retiring: vec![],
        }
    }
    fn retire(&mut self, id: u64) {
        if let Some(mut transfer) = self.reads.remove(&id) {
            transfer.cancel();
            self.retiring.push(transfer);
        }
    }
    fn cancel_all(&mut self) {
        for (_, mut transfer) in std::mem::take(&mut self.reads) {
            transfer.cancel();
            self.retiring.push(transfer);
        }
    }
    fn cancel_kind(&mut self, committed: bool) {
        let ids: Vec<_> = self
            .reads
            .iter()
            .filter_map(|(id, transfer)| (transfer.committed == committed).then_some(*id))
            .collect();
        for id in ids {
            self.retire(id);
        }
    }
    fn request_id(&mut self, id: u64) -> io::Result<()> {
        if id == 0 || id <= self.last_id {
            return Err(io::Error::other("Clipboard export request ID reused"));
        }
        self.last_id = id;
        Ok(())
    }
    /// The caller acknowledges the command before emitting any returned failure.
    /// There is no unbounded queue of refused or cancelled requests.
    pub(crate) fn control(&mut self, command: Control) -> io::Result<Option<Event>> {
        self.retiring.retain(|transfer| !transfer.finished());
        match command {
            Control::Enable { version } if protocol::VERSIONS.contains(&version) => {
                if self.enabled && self.version != version {
                    return Err(io::Error::other("Disable exports before changing version"));
                }
                if !self.enabled {
                    if version == protocol::COPY_VERSION {
                        self.copies = Some(self.reader.copy_session()?);
                    }
                    self.version = version;
                    self.enabled = true;
                    self.announced = false;
                }
            }
            Control::Disable => {
                self.enabled = false;
                self.announced = false;
                self.offer = None;
                self.cancel_all();
                self.copy = None;
                self.copies = None;
            }
            Control::Cancel { id } => self.retire(id),
            Control::Read { id, offer, format } => {
                self.request_id(id)?;
                if !self.enabled
                    || !self.announced
                    || self.offer.as_ref() != Some(&offer)
                    || !offer.offer.formats.contains(&format)
                    || self.reads.len() + self.retiring.len() >= protocol::MAX_READS
                {
                    return Ok(Some(Event::Data {
                        id,
                        offset: 0,
                        chunk: None,
                    }));
                }
                self.reads.insert(
                    id,
                    Transfer::new(self.reader.clone(), Authority::Focused(offer), format)?,
                );
            }
            Control::Commit { id, offer } if self.copies.is_some() => {
                self.request_id(id)?;
                let copy = if self.announced && self.offer.as_ref() == Some(&offer) {
                    self.copies.as_mut().unwrap().commit(&offer)?
                } else {
                    None
                };
                if let Some(copy) = &copy {
                    self.cancel_kind(true);
                    self.copy = Some(copy.clone());
                }
                return Ok(Some(Event::Committed { id, copy }));
            }
            Control::Release { copy } if self.copies.is_some() => {
                if self.copy.as_ref() == Some(&copy) {
                    self.copies.as_mut().unwrap().release(&copy)?;
                    self.cancel_kind(true);
                    self.copy = None;
                    return Ok(Some(Event::CopyRevoked { id: copy.id }));
                }
            }
            Control::ReadCopy { id, copy, format } if self.copies.is_some() => {
                self.request_id(id)?;
                if self.copy.as_ref() != Some(&copy)
                    || !copy.offer.offer.formats.contains(&format)
                    || self.reads.len() + self.retiring.len() >= protocol::MAX_READS
                {
                    return Ok(Some(Event::Data {
                        id,
                        offset: 0,
                        chunk: None,
                    }));
                }
                self.reads.insert(
                    id,
                    Transfer::new(self.reader.clone(), Authority::Committed(copy), format)?,
                );
            }
            _ => return Err(io::Error::other("Unsupported clipboard export version")),
        }
        Ok(None)
    }
    /// Only metadata IPC runs here, never a wait for a native owner. At most one
    /// payload chunk per loop keeps browser requests and heartbeats responsive.
    pub(crate) fn poll(&mut self, output: &mut impl io::Write) -> io::Result<()> {
        self.retiring.retain(|transfer| !transfer.finished());
        if !self.enabled {
            return Ok(());
        }
        // Check copy authority after reading focused metadata. If that metadata
        // observes a newer source, its reset has already revoked the old copy;
        // do not announce the new source and then drain an old buffered chunk.
        let offer = self.reader.export_offer()?;
        if let Some(copies) = self.copies.as_mut() {
            let current = copies.current()?;
            if current != self.copy {
                self.cancel_kind(true);
                if let Some(copy) = self.copy.take() {
                    Self::send(output, &Event::CopyRevoked { id: copy.id })?;
                }
                // Only this relay's acknowledged Commit may install a copy.
                if current.is_some() {
                    return Err(io::Error::other("Unacknowledged clipboard copy"));
                }
            }
        }
        if !self.announced || self.offer != offer {
            self.cancel_kind(false);
            self.offer = offer;
            self.announced = true;
            Self::send(
                output,
                &Event::Offer {
                    offer: self.offer.clone(),
                },
            )?;
        }
        let mut ids: Vec<_> = self.reads.keys().copied().collect();
        ids.sort_by_key(|id| (*id <= self.last_sent, *id));
        for id in ids {
            let transfer = self.reads.get_mut(&id).unwrap();
            let chunk = match transfer.chunks.try_recv() {
                Ok(chunk) => chunk.ok(),
                Err(mpsc::TryRecvError::Empty) => continue,
                Err(mpsc::TryRecvError::Disconnected) => None,
            };
            let finished = chunk.as_ref().is_none_or(|chunk| chunk.eof);
            let count = chunk.as_ref().map_or(0, |chunk| chunk.bytes.len());
            Self::send(
                output,
                &Event::Data {
                    id,
                    offset: transfer.offset,
                    chunk,
                },
            )?;
            transfer.offset += count;
            if finished {
                self.retire(id);
            } else {
                // A disconnected task is reported as a failed read on the next
                // poll, so a partial representation can never silently succeed.
                let _ = transfer.credit.as_ref().unwrap().try_send(());
            }
            self.last_sent = id;
            break;
        }
        Ok(())
    }
    pub(crate) fn send(output: &mut impl io::Write, event: &Event) -> io::Result<()> {
        crate::wire::write(output, protocol::FRAME, &event.encode()?)
    }
}
impl Drop for Relay {
    fn drop(&mut self) {
        self.cancel_all();
        self.copies.take();
    }
}

#[cfg(test)]
#[path = "clipboard_export_relay_tests.rs"]
mod tests;
