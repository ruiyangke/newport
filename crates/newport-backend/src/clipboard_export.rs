//! Requested remote clipboard bytes. Runtime copy acknowledgment grants read
//! authority, not a native clipboard write. Publication must separately validate
//! focus and the native revision; reads of an accepted publication may outlive
//! focus under their acknowledged-copy lease.
use crate::agent_runtime::protocol::{
    clipboard::ExportOffer,
    clipboard_export::{self as wire, Control, Event},
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{sync::oneshot, time::Instant};
#[path = "clipboard_export_copy.rs"]
mod copy;
#[path = "clipboard_export_source.rs"]
pub mod source;
pub use copy::{Commit, CommittedCopy};

const TIMEOUT: Duration = Duration::from_secs(20);
const CHANGED: &str = "Remote clipboard offer or export permission changed";

struct Source {
    offer: ExportOffer,
    current: Arc<AtomicBool>,
}
/// A negotiated integration receiver, including before its first native offer.
/// Only the receiver issues this identity; caller-supplied runtime IDs cannot
/// bind a host clipboard endpoint or survive replacement of the connection.
#[derive(Clone)]
pub struct Connection {
    runtime: String,
    disconnected: Arc<AtomicBool>,
}
impl Connection {
    pub fn is_current(&self) -> bool {
        !self.disconnected.load(Ordering::Acquire)
    }
    pub(crate) fn runtime(&self) -> &str {
        &self.runtime
    }
    pub(crate) fn fence(&self) -> &Arc<AtomicBool> {
        &self.disconnected
    }
}
/// The exact offer and connection accepted by the event pump. A clone retains
/// revocation, not a snapshot of permission. Only the receiver can issue one.
#[derive(Clone)]
pub struct Lease {
    offer: ExportOffer,
    current: Arc<AtomicBool>,
    disconnected: Arc<AtomicBool>,
}
impl Lease {
    pub(crate) fn connection(&self) -> Connection {
        Connection {
            runtime: self.offer.offer.runtime.clone(),
            disconnected: self.disconnected.clone(),
        }
    }
    pub(crate) fn connection_fence(&self) -> &Arc<AtomicBool> {
        &self.disconnected
    }
    pub fn offer(&self) -> &ExportOffer {
        &self.offer
    }
    pub fn is_current(&self) -> bool {
        self.current.load(Ordering::Acquire) && !self.disconnected.load(Ordering::Acquire)
    }
}
pub struct Representation {
    offer: ExportOffer,
    format: String,
    bytes: Vec<u8>,
    current: Arc<AtomicBool>,
    budget: Arc<AtomicUsize>,
    slots: Arc<AtomicUsize>,
    disconnected: Arc<AtomicBool>,
    copy: Option<String>,
}
impl std::fmt::Debug for Representation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Representation")
            .field("format", &self.format)
            .field("bytes", &self.bytes.len())
            .finish_non_exhaustive()
    }
}
impl Representation {
    pub fn offer(&self) -> &ExportOffer {
        &self.offer
    }
    pub fn format(&self) -> &str {
        &self.format
    }
    pub fn copy_id(&self) -> Option<&str> {
        self.copy.as_deref()
    }
    /// Focused data follows its offer's focus route. Acknowledged-copy data
    /// survives focus changes, but source replacement, release, disable or
    /// connection shutdown still invalidates it. Neither grants native write
    /// permission or overrides a newer local clipboard value.
    pub fn bytes(&self) -> Result<&[u8], String> {
        if self.disconnected.load(Ordering::Acquire) || !self.current.load(Ordering::Acquire) {
            return Err(CHANGED.into());
        }
        Ok(&self.bytes)
    }
    fn append(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.budget
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes.len())
                    .filter(|used| *used <= wire::MAX_BYTES)
            })
            .map_err(|_| "Remote clipboard memory limit reached")?;
        if self.bytes.try_reserve_exact(bytes.len()).is_err() {
            self.budget.fetch_sub(bytes.len(), Ordering::AcqRel);
            return Err("Could not allocate remote clipboard data".into());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
}
impl Drop for Representation {
    fn drop(&mut self) {
        self.budget.fetch_sub(self.bytes.len(), Ordering::AcqRel);
        self.slots.fetch_sub(1, Ordering::AcqRel);
    }
}

/// One explicitly requested format. Dropping this handle abandons its demand;
/// the integration timer sends the corresponding cancellation. A completed
/// representation retains its memory charge until the caller drops it.
/// The integration owner must keep polling `Agent::event` while this result is
/// awaited; receiving a result does not itself pump the shared network stream.
pub struct Read {
    id: u64,
    deadline: Instant,
    reply: oneshot::Receiver<Result<Representation, String>>,
}
impl Read {
    pub fn id(&self) -> u64 {
        self.id
    }
    pub async fn receive(self) -> Result<Representation, String> {
        tokio::time::timeout_at(self.deadline, self.reply)
            .await
            .map_err(|_| "Remote clipboard read timed out".to_owned())?
            .map_err(|_| "Remote clipboard connection closed".to_owned())?
    }
}
struct Pending {
    data: Representation,
    reply: oneshot::Sender<Result<Representation, String>>,
    deadline: Instant,
}
#[derive(Default)]
pub(crate) struct Receiver {
    runtime: Option<String>,
    enabled: bool,
    disabling: bool,
    version: u32,
    source: Option<Source>,
    previous: Option<ExportOffer>,
    last_id: u64,
    pending: BTreeMap<u64, Pending>,
    // Keep each cancellation's authority kind until the remote acknowledgment:
    // a focus-only withdrawal drains focused reads, not copy reads.
    retiring: BTreeMap<u64, bool>,
    committing: Option<copy::Pending>,
    committed: Option<copy::State>,
    budget: Arc<AtomicUsize>,
    slots: Arc<AtomicUsize>,
    disconnected: Arc<AtomicBool>,
}
impl Receiver {
    #[cfg(test)]
    pub(crate) fn enable(&mut self, runtime: &str) -> Result<(), String> {
        self.enable_version(runtime, wire::VERSION)
    }
    pub(crate) fn enable_version(&mut self, runtime: &str, version: u32) -> Result<(), String> {
        if self.disconnected.load(Ordering::Acquire) {
            return Err("Clipboard integration connection must be replaced".into());
        }
        if !wire::VERSIONS.contains(&version) || self.enabled && self.version != version {
            return Err("Unsupported clipboard version or exports must first be disabled".into());
        }
        if uuid::Uuid::parse_str(runtime).is_err()
            || self
                .runtime
                .as_deref()
                .is_some_and(|bound| bound != runtime)
        {
            return Err(
                "Clipboard export runtime changed; reconnect the integration channel".into(),
            );
        }
        self.runtime = Some(runtime.into());
        if !self.enabled {
            self.previous = None;
            self.enabled = true;
            self.disabling = false;
            self.version = version;
        }
        Ok(())
    }
    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }
    pub(crate) fn offer(&self) -> Option<&ExportOffer> {
        self.source
            .as_ref()
            .filter(|_| !self.disconnected.load(Ordering::Acquire))
            .map(|source| &source.offer)
    }
    pub(crate) fn lease(&self) -> Option<Lease> {
        let source = self.source.as_ref()?;
        let lease = Lease {
            offer: source.offer.clone(),
            current: source.current.clone(),
            disconnected: self.disconnected.clone(),
        };
        lease.is_current().then_some(lease)
    }
    pub(crate) fn connection_fence(&self) -> Arc<AtomicBool> {
        self.disconnected.clone()
    }
    pub(crate) fn connection(&self) -> Option<Connection> {
        let connection = Connection {
            runtime: self.runtime.clone()?,
            disconnected: self.disconnected.clone(),
        };
        (self.enabled && !self.disabling && connection.is_current()).then_some(connection)
    }
    pub(crate) fn disconnect(&mut self) {
        self.disconnected.store(true, Ordering::Release);
        self.disable();
    }
    fn invalidate_focused(&mut self, reason: &str) {
        if let Some(source) = self.source.take() {
            source.current.store(false, Ordering::Release);
        }
        self.invalidate_reads(false, reason, true);
        self.abandon_stale_commit();
    }
    fn invalidate_reads(&mut self, committed: bool, reason: &str, remote: bool) {
        let ids: Vec<_> = self
            .pending
            .iter()
            .filter_map(|(id, pending)| (pending.data.copy.is_some() == committed).then_some(*id))
            .collect();
        for id in ids {
            self.fail(id, reason);
            if !remote {
                self.retiring.insert(id, committed);
            }
        }
        if remote {
            self.retiring.retain(|_, kind| *kind != committed);
        }
    }
    fn invalidate_all(&mut self, reason: &str) {
        self.invalidate_focused(reason);
        self.invalidate_copy(reason, true);
        if let Some(pending) = self.committing.take() {
            pending.fail(reason);
        }
    }
    pub(crate) fn disable(&mut self) {
        self.enabled = false;
        self.disabling = false;
        self.invalidate_all("Remote clipboard exports disconnected or disabled");
    }
    pub(crate) fn begin_disable(&mut self) {
        self.disabling = true;
        self.invalidate_all("Remote clipboard exports disabled");
    }
    pub(crate) fn read(
        &mut self,
        offer: &ExportOffer,
        format: &str,
    ) -> Result<(Read, Control), String> {
        let source = self
            .source
            .as_ref()
            .filter(|source| {
                self.enabled
                    && !self.disabling
                    && !self.disconnected.load(Ordering::Acquire)
                    && &source.offer == offer
            })
            .ok_or(CHANGED)?;
        self.start_read(offer, format, source.current.clone(), None)
    }
    fn start_read(
        &mut self,
        offer: &ExportOffer,
        format: &str,
        current: Arc<AtomicBool>,
        copy: Option<crate::agent_runtime::protocol::clipboard::CopyLease>,
    ) -> Result<(Read, Control), String> {
        if self.slots.load(Ordering::Acquire) + self.retiring.len() >= wire::MAX_READS {
            return Err("Too many remote clipboard reads".into());
        }
        if self.budget.load(Ordering::Acquire) >= wire::MAX_BYTES {
            return Err("Remote clipboard memory limit reached".into());
        }
        let id = self
            .last_id
            .checked_add(1)
            .ok_or("Clipboard export request IDs exhausted")?;
        let command = match &copy {
            Some(copy) => Control::ReadCopy {
                id,
                copy: copy.clone(),
                format: format.into(),
            },
            None => Control::Read {
                id,
                offer: offer.clone(),
                format: format.into(),
            },
        };
        command.encode().map_err(|e| e.to_string())?;
        self.last_id = id;
        let deadline = Instant::now() + TIMEOUT;
        let (reply, receive) = oneshot::channel();
        self.slots.fetch_add(1, Ordering::AcqRel);
        self.pending.insert(
            id,
            Pending {
                deadline,
                reply,
                data: Representation {
                    offer: offer.clone(),
                    format: format.into(),
                    bytes: vec![],
                    current,
                    budget: self.budget.clone(),
                    slots: self.slots.clone(),
                    disconnected: self.disconnected.clone(),
                    copy: copy.map(|copy| copy.id),
                },
            },
        );
        Ok((
            Read {
                id,
                deadline,
                reply: receive,
            },
            command,
        ))
    }
    fn fail(&mut self, id: u64, reason: &str) {
        if let Some(pending) = self.pending.remove(&id) {
            let _ = pending.reply.send(Err(reason.into()));
        }
    }
    pub(crate) fn cancel(&mut self, id: u64) {
        if let Some(pending) = self.pending.get(&id) {
            let committed = pending.data.copy.is_some();
            self.fail(id, "Remote clipboard read cancelled");
            self.retiring.insert(id, committed);
        }
    }
    pub(crate) fn expire(&mut self) {
        let now = Instant::now();
        if self
            .committing
            .as_ref()
            .is_some_and(|pending| now >= pending.deadline)
            || self
                .committed
                .as_ref()
                .and_then(|copy| copy.release_deadline)
                .is_some_and(|deadline| now >= deadline)
        {
            // Without its acknowledgment we cannot name/release the runtime
            // copy. Retire the connection rather than replay or leak authority.
            self.disconnect();
            return;
        }
        self.abandon_stale_commit();
        if self
            .committed
            .as_ref()
            .is_some_and(|copy| !copy.current.load(Ordering::Acquire))
        {
            self.invalidate_reads(true, CHANGED, false);
        }
        let expired: Vec<_> = self
            .pending
            .iter()
            .filter_map(|(id, pending)| {
                (pending.reply.is_closed() || now >= pending.deadline).then_some(*id)
            })
            .collect();
        for id in expired {
            let committed = self.pending[&id].data.copy.is_some();
            self.fail(id, "Remote clipboard read cancelled or timed out");
            self.retiring.insert(id, committed);
        }
    }
    pub(crate) fn cancellations(&self) -> Vec<u64> {
        self.retiring.keys().copied().collect()
    }
    pub(crate) fn cancelled(&mut self, id: u64) {
        self.retiring.remove(&id);
    }
    pub(crate) fn accept(&mut self, bytes: &[u8]) -> Result<(), String> {
        let result = self.accept_inner(bytes);
        if result.is_err() {
            self.disconnect();
        }
        result
    }
    fn accept_inner(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.disconnected.load(Ordering::Acquire) || !self.enabled {
            return Err("Unnegotiated clipboard export event".into());
        }
        let event = wire::Event::decode(bytes).map_err(|e| e.to_string())?;
        // Already-sent events can precede the disable acknowledgment. Drain
        // them without reviving metadata or data after local revocation.
        if self.disabling {
            return Ok(());
        }
        self.expire();
        if self.disconnected.load(Ordering::Acquire) {
            return Err("Clipboard acknowledgment timed out; reconnect the integration".into());
        }
        match event {
            Event::Committed { id, copy } => self.accept_commit(id, copy)?,
            Event::CopyRevoked { id } => self.accept_copy_revoked(&id)?,
            Event::Offer { offer } => {
                if self.offer() == offer.as_ref() {
                    return Ok(());
                }
                if let Some(offer) = &offer {
                    if Some(offer.offer.runtime.as_str()) != self.runtime.as_deref() {
                        return Err("Clipboard offer belongs to another runtime".into());
                    }
                    if self
                        .committed
                        .as_ref()
                        .is_some_and(|copy| copy.offer() != &offer.offer)
                    {
                        return Err("Clipboard source changed without revoking its copy".into());
                    }
                    if self.previous.as_ref().is_some_and(|previous| {
                        offer.offer.revision < previous.offer.revision
                            || offer.route_revision < previous.route_revision
                            || (offer.offer.revision == previous.offer.revision
                                && offer.offer != previous.offer)
                            || offer == previous
                    }) {
                        return Err(
                            "Clipboard export revision moved backwards or was reused".into()
                        );
                    }
                }
                self.invalidate_focused(CHANGED);
                if let Some(offer) = offer {
                    self.previous = Some(offer.clone());
                    self.source = Some(Source {
                        offer,
                        current: Arc::new(AtomicBool::new(true)),
                    });
                }
            }
            Event::Data { id, offset, chunk } => {
                if self.retiring.contains_key(&id) {
                    return Ok(());
                }
                let pending = self
                    .pending
                    .get_mut(&id)
                    .ok_or("Unknown or already completed clipboard export read")?;
                if offset != pending.data.bytes.len() {
                    return Err("Clipboard export chunk offset does not match".into());
                }
                if let Some(chunk) = chunk {
                    if let Err(error) = pending.data.append(&chunk.bytes) {
                        let committed = pending.data.copy.is_some();
                        self.fail(id, &error);
                        if !chunk.eof {
                            self.retiring.insert(id, committed);
                        }
                    } else if chunk.eof {
                        let pending = self.pending.remove(&id).unwrap();
                        let _ = pending.reply.send(Ok(pending.data));
                    }
                } else {
                    self.fail(id, "Remote clipboard format could not be read");
                }
            }
        }
        Ok(())
    }
}
impl Drop for Receiver {
    fn drop(&mut self) {
        self.disable();
    }
}

#[cfg(test)]
#[path = "clipboard_export_tests.rs"]
mod tests;
