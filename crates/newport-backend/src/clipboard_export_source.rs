//! Demand bridge between the native owner and the integration event pump.
//! No socket or native API is called from a provider callback. The integration
//! owner dequeues requests, starts the focused or acknowledged-copy read, and runs
//! `Demand::complete` beside (never instead of) polling `Agent::event`.
use super::{CommittedCopy, Lease, Read, Representation, wire};
use crate::agent_runtime::protocol::clipboard::{ExportOffer, Origin};
use newport_platform::clipboard::provider::{Payload, Reply, Source as NativeSource};
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use uuid::Uuid;

struct Authorization {
    runtime: String,
    worker: String,
    route_revision: u64,
    current: AtomicBool,
    transition: Mutex<()>,
    permission: Option<Permission>,
}
struct Permission {
    gate: Arc<crate::integration_permissions::Gate>,
    ticket: u64,
}
impl Authorization {
    fn permitted<T>(&self, action: impl FnOnce() -> T) -> Option<T> {
        match &self.permission {
            Some(permission) => permission.gate.clipboard_at(permission.ticket, action),
            None => Some(action()),
        }
    }
    fn revoke(&self) {
        let _transition = self.transition.lock().unwrap();
        self.current.store(false, Ordering::Release);
    }
}
/// One explicit GUI export/focus grant, independent of host-import permission.
/// The router must issue this only after binding the live backend display lease
/// and acknowledging its runtime route. It must drop/revoke it on focus loss,
/// permission change, display retirement or transport replacement. Never renew
/// an old grant: a new focus epoch requires a new object and route revision.
/// Focused v1 sources always follow this grant. A v2 source requires it until
/// the native owner confirms publication, then follows its acknowledged copy.
/// Permission/display/transport retirement must also revoke accepted sources;
/// revoking this focus grant alone intentionally preserves their copies.
pub struct Grant(Arc<Authorization>);
impl Grant {
    pub(crate) fn is_current(&self) -> bool {
        self.0
            .permitted(|| self.0.current.load(Ordering::Acquire))
            .unwrap_or(false)
    }
    pub(crate) fn runtime(&self) -> &str {
        &self.0.runtime
    }
    pub(crate) fn worker(&self) -> &str {
        &self.0.worker
    }
    pub(crate) fn revision(&self) -> u64 {
        self.0.route_revision
    }
    pub(crate) fn matches(&self, offer: &ExportOffer) -> bool {
        self.is_current()
            && offer.offer.runtime == self.0.runtime
            && offer.route_revision == self.0.route_revision
            && matches!(&offer.offer.origin, Origin::Native { worker, .. } if *worker == self.0.worker)
    }
    pub fn new(runtime: Uuid, worker: Uuid, route_revision: u64) -> Option<Self> {
        (route_revision > 0).then(|| {
            Self(Arc::new(Authorization {
                runtime: runtime.to_string(),
                worker: worker.to_string(),
                route_revision,
                current: AtomicBool::new(true),
                transition: Mutex::new(()),
                permission: None,
            }))
        })
    }
    /// Production export authority retains the exact acknowledged permission
    /// epoch. Disabling sharing also fences accepted copies after GUI blur;
    /// re-enabling cannot revive bytes from the previous permission epoch.
    pub(crate) fn permitted(
        runtime: Uuid,
        worker: Uuid,
        route_revision: u64,
        gate: Arc<crate::integration_permissions::Gate>,
        ticket: u64,
    ) -> Option<Self> {
        let mut grant = Self::new(runtime, worker, route_revision)?;
        let permission = Permission {
            gate: gate.clone(),
            ticket,
        };
        gate.clipboard_at(ticket, || {
            Arc::get_mut(&mut grant.0).unwrap().permission = Some(permission);
            grant
        })
    }
    pub fn revoke(&self) {
        self.0.revoke();
    }
}
impl Drop for Grant {
    fn drop(&mut self) {
        self.revoke();
    }
}

/// One queue per integration connection. Its eight slots include queued work,
/// pending reads and delivered representations still held by the native owner.
/// Replacing sources does not reset this budget. Dropping the queue fences all
/// its sources and held data, even if a transport has not observed EOF yet.
pub struct Demands {
    sender: mpsc::Sender<Demand>,
    receiver: mpsc::Receiver<Demand>,
    slots: Arc<Semaphore>,
    binding: Mutex<Option<Binding>>,
    copy_source: Mutex<Weak<SourceState>>,
}
struct Binding {
    connection: Weak<AtomicBool>,
    revision: u64,
    grant: Weak<Authorization>,
}
impl Default for Demands {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel(wire::MAX_READS);
        Self {
            sender,
            receiver,
            slots: Arc::new(Semaphore::new(wire::MAX_READS)),
            binding: Mutex::default(),
            copy_source: Mutex::default(),
        }
    }
}
impl Demands {
    pub fn source(&self, lease: Lease, grant: &Grant) -> Option<Arc<Source>> {
        self.bind(Authority::Focused(lease), grant)
    }
    /// Consumes one acknowledged copy for one native publication. It remains
    /// focus-bound until the native owner confirms publication. Failed binding
    /// drops the copy, releasing its runtime authority on normal maintenance.
    pub fn source_copy(&self, copy: CommittedCopy, grant: &Grant) -> Option<Arc<Source>> {
        self.bind(
            Authority::Copy {
                copy,
                claimed: AtomicBool::new(false),
                published: AtomicBool::new(false),
            },
            grant,
        )
    }
    fn bind(&self, authority: Authority, grant: &Grant) -> Option<Arc<Source>> {
        let offer = authority.offer();
        if !grant.matches(offer) {
            return None;
        }
        let source = Arc::new(Source(Arc::new(SourceState {
            authority,
            grant: grant.0.clone(),
            active: AtomicBool::new(true),
            sender: self.sender.clone(),
            slots: self.slots.clone(),
        })));
        if !source.is_current() {
            return None;
        }
        let connection = Arc::downgrade(source.0.authority.connection());
        let mut binding = self.binding.lock().unwrap();
        if !source.is_current() {
            return None;
        }
        if let Some(last) = binding.as_ref() {
            // UUID equality does not establish transport continuity. A new
            // integration connection always needs its own queue and grants.
            if !last.connection.ptr_eq(&connection) {
                return None;
            }
            let same = last.grant.ptr_eq(&Arc::downgrade(&grant.0));
            if grant.0.route_revision < last.revision
                || grant.0.route_revision == last.revision && !same
            {
                return None;
            }
            if !same && let Some(previous) = last.grant.upgrade() {
                previous.revoke();
            }
        }
        *binding = Some(Binding {
            connection,
            revision: grant.0.route_revision,
            grant: Arc::downgrade(&grant.0),
        });
        // Install lifetime tracking under the same binding lock. A delayed
        // older source must not replace the copy released when this queue dies.
        if source.copy().is_some() {
            *self.copy_source.lock().unwrap() = Arc::downgrade(&source.0);
        }
        Some(source)
    }
    pub async fn receive(&mut self) -> Option<Demand> {
        self.receiver.recv().await
    }
}
impl Drop for Demands {
    fn drop(&mut self) {
        if let Some(source) = self.copy_source.get_mut().unwrap().upgrade() {
            Source(source).revoke();
        }
    }
}

enum Authority {
    Focused(Lease),
    Copy {
        copy: CommittedCopy,
        claimed: AtomicBool,
        published: AtomicBool,
    },
}
impl Authority {
    fn offer(&self) -> &ExportOffer {
        match self {
            Self::Focused(lease) => lease.offer(),
            Self::Copy { copy, .. } => copy.offer(),
        }
    }
    fn connection(&self) -> &Arc<AtomicBool> {
        match self {
            Self::Focused(lease) => &lease.disconnected,
            Self::Copy { copy, .. } => copy.connection_fence(),
        }
    }
    fn copy(&self) -> Option<&CommittedCopy> {
        match self {
            Self::Copy { copy, .. } => Some(copy),
            _ => None,
        }
    }
}

/// Exactly one remote offer with both transport and host authorization fences.
#[derive(Clone)]
pub struct Source(Arc<SourceState>);
struct SourceState {
    authority: Authority,
    grant: Arc<Authorization>,
    active: AtomicBool,
    sender: mpsc::Sender<Demand>,
    slots: Arc<Semaphore>,
}
impl NativeSource for Source {
    fn is_current(&self) -> bool {
        Source::is_current(self)
    }
    fn begin_publication(&self) -> bool {
        let _transition = self.0.grant.transition.lock().unwrap();
        self.0
            .grant
            .permitted(|| {
                if !self.current_without_permission() {
                    self.revoke();
                    return false;
                }
                match &self.0.authority {
                    Authority::Focused(_) => true,
                    Authority::Copy { claimed, .. } => {
                        self.0.grant.current.load(Ordering::Acquire)
                            && claimed
                                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                                .is_ok()
                    }
                }
            })
            .unwrap_or(false)
    }
    fn publication_confirmed(&self) -> bool {
        // Focus revocation and publication acceptance share one boundary. A
        // focus loss cannot slip between checking the grant and accepting the
        // independent copy lifetime. Transport/source revocation remains live.
        let _transition = self.0.grant.transition.lock().unwrap();
        self.0
            .grant
            .permitted(|| {
                if !self.current_without_permission() {
                    self.revoke();
                    return false;
                }
                if let Authority::Copy {
                    claimed, published, ..
                } = &self.0.authority
                {
                    if !claimed.load(Ordering::Acquire) {
                        self.revoke();
                        return false;
                    }
                    published.store(true, Ordering::Release);
                }
                self.current_without_permission()
            })
            .unwrap_or(false)
    }
    fn publication_revoked(&self) {
        self.revoke();
    }
    fn request(&self, format: &str, reply: Reply) {
        if !self.readable()
            || reply.is_cancelled()
            || !self.offer().offer.formats.iter().any(|f| f == format)
        {
            return;
        }
        let Ok(slot) = self.0.slots.clone().try_acquire_owned() else {
            return;
        };
        let _ = self.0.sender.try_send(Demand {
            source: self.clone(),
            format: format.into(),
            reply,
            slot,
        });
    }
}
impl Source {
    pub fn offer(&self) -> &ExportOffer {
        self.0.authority.offer()
    }
    pub fn copy(&self) -> Option<&CommittedCopy> {
        self.0.authority.copy()
    }
    pub fn revoke(&self) {
        self.0.active.store(false, Ordering::Release);
        if let Some(copy) = self.copy() {
            copy.revoke();
        }
    }
    pub fn is_current(&self) -> bool {
        self.0
            .grant
            .permitted(|| self.current_without_permission())
            .unwrap_or(false)
    }
    fn current_without_permission(&self) -> bool {
        if !self.0.active.load(Ordering::Acquire) || self.0.sender.is_closed() {
            return false;
        }
        let focused = self.0.grant.current.load(Ordering::Acquire);
        match &self.0.authority {
            Authority::Focused(lease) => focused && lease.is_current(),
            Authority::Copy {
                copy, published, ..
            } => copy.is_current() && (published.load(Ordering::Acquire) || focused),
        }
    }
    fn readable(&self) -> bool {
        self.is_current()
            && match &self.0.authority {
                Authority::Focused(_) => true,
                Authority::Copy { published, .. } => published.load(Ordering::Acquire),
            }
    }
}

pub struct Demand {
    source: Source,
    format: String,
    reply: Reply,
    slot: OwnedSemaphorePermit,
}
impl Demand {
    pub fn offer(&self) -> &ExportOffer {
        self.source.offer()
    }
    /// Use `Agent::read_clipboard_copy` when present; otherwise use the focused
    /// v1 read. Completion rejects data from the other authority kind.
    pub fn copy(&self) -> Option<&CommittedCopy> {
        self.source.copy()
    }
    pub fn format(&self) -> &str {
        &self.format
    }
    /// Check before sending a remote read. Rejected demand is failed by Drop.
    pub fn is_current(&self) -> bool {
        self.source.readable() && !self.reply.is_cancelled()
    }
    /// Run on the integration executor while its event pump continues. Source
    /// revocation abandons the remote read within one bounded timer tick; the
    /// receiver then sends cancellation through its normal maintenance path.
    /// Dropping this future also cancels both ends, without another connection.
    pub async fn complete(self, read: Read) {
        let mut timer = tokio::time::interval(Duration::from_millis(10));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let result = read.receive();
        tokio::pin!(result);
        loop {
            if !self.is_current() {
                return;
            }
            tokio::select! {
                biased;
                _ = timer.tick() => {},
                result = &mut result => {
                    if let Ok(data) = result
                        && self.is_current()
                        && data.offer() == self.offer()
                        && data.copy_id() == self.copy().map(CommittedCopy::id)
                        && data.format() == self.format
                        && data.bytes().is_ok()
                    {
                        self.reply.complete(Box::new(Data {
                            source: self.source,
                            data,
                            _slot: self.slot,
                        }));
                    }
                    return;
                }
            }
        }
    }
}
struct Data {
    source: Source,
    data: Representation,
    _slot: OwnedSemaphorePermit,
}
impl Payload for Data {
    fn bytes(&self) -> Option<&[u8]> {
        self.source
            .readable()
            .then(|| self.data.bytes().ok())
            .flatten()
    }
}

#[cfg(test)]
#[path = "clipboard_export_source_tests.rs"]
mod tests;
