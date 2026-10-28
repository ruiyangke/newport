use super::protocol::{
    Access, Chunk, CopyLease, ExportOffer, NativeRequest, Offer, Origin, ProviderLease,
    ProviderPoll, Route,
};
use crate::{clipboard_wire, demand::Store};
use std::{
    collections::{HashMap, HashSet},
    io,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone)]
pub(super) struct Grant {
    pub worker: String,
    pub producer: bool,
    pub cancelled: Arc<AtomicBool>,
    token: String,
    connections: Arc<Mutex<Vec<std::sync::Weak<std::os::unix::net::UnixStream>>>>,
}
impl Grant {
    pub(super) fn track(
        &self,
        stream: &std::os::unix::net::UnixStream,
    ) -> io::Result<Arc<std::os::unix::net::UnixStream>> {
        let mut connections = self.connections.lock().unwrap();
        if self.cancelled.load(Ordering::Acquire) {
            return Err(io::Error::other("Clipboard worker lease ended"));
        }
        connections.retain(|connection| connection.strong_count() > 0);
        let control = Arc::new(stream.try_clone()?);
        connections.push(Arc::downgrade(&control));
        Ok(control)
    }
    fn revoke(&self) {
        self.cancelled.store(true, Ordering::Release);
        for connection in self
            .connections
            .lock()
            .unwrap()
            .drain(..)
            .filter_map(|connection| connection.upgrade())
        {
            let _ = connection.shutdown(std::net::Shutdown::Both);
        }
    }
}
pub(super) struct State {
    pub runtime: String,
    pub path: PathBuf,
    grants: HashMap<String, Grant>,
    // Never evicted during this runtime. Admission stops at the runtime's
    // lifetime registration bound, so an unknown worker is never inferred to
    // have retired and a retired worker cannot be reauthorized by reusing ID.
    retired: HashSet<String>,
    owner: Option<String>,
    enabled: bool,
    pub revision: i64,
    pub store: Arc<Store>,
    source: Option<Source>,
    last_host: Option<clipboard_wire::Offer>,
    route: Route,
    binding: u64,
    native_seen: Option<(u64, Vec<String>, bool)>,
    export_cancelled: Arc<AtomicBool>,
    committed: Option<Committed>,
    upstream_next: u64,
    requests: HashMap<u64, Pending>,
    closed: bool,
}
#[derive(Clone, PartialEq, Eq)]
enum Target {
    Host(i64),
    Native(ProviderLease, u64),
}
struct Source {
    target: Target,
    formats: Vec<String>,
}
struct Committed {
    lease: CopyLease,
    connection: String,
    cancelled: Arc<AtomicBool>,
}
impl Source {
    fn origin(&self) -> Origin {
        match &self.target {
            Target::Host(_) => Origin::Host,
            Target::Native(lease, revision) => Origin::Native {
                worker: lease.worker.clone(),
                revision: *revision,
            },
        }
    }
}
const PENDING_TIMEOUT: Duration = Duration::from_secs(15);
const PENDING_CEILING: Duration = Duration::from_secs(30 * 60);
struct Pending {
    internal: clipboard_wire::Request,
    target: Target,
    expires: Instant,
    /// Keep-alives cannot extend `expires` past this.
    ceiling: Instant,
}
pub(super) type Shared = Arc<Mutex<State>>;
#[cfg(test)]
#[path = "keep_alive_tests.rs"]
mod keep_alive_tests;
#[cfg(test)]
#[path = "route_tests.rs"]
mod route_tests;
impl State {
    pub fn new(runtime: &str, path: PathBuf) -> Self {
        let state = Self {
            runtime: runtime.into(),
            path,
            grants: HashMap::new(),
            retired: HashSet::new(),
            owner: None,
            enabled: false,
            revision: 1,
            store: Store::new(),
            source: None,
            last_host: None,
            route: Route::default(),
            binding: 0,
            native_seen: None,
            export_cancelled: Default::default(),
            committed: None,
            upstream_next: 0,
            requests: HashMap::new(),
            closed: false,
        };
        state.store.update(clipboard_wire::Offer {
            revision: state.revision,
            formats: vec![],
        });
        state
    }
    pub fn grant(&mut self, worker: &str, service: &str) -> io::Result<Access> {
        if self.closed
            || self.grants.contains_key(worker)
            || self.retired.contains(worker)
            || self.grants.len() >= crate::runtime_protocol::MAX_WORKERS
            || self.grants.len() + self.retired.len() >= crate::runtime_protocol::MAX_REGISTRATIONS
        {
            return Err(io::Error::other("Clipboard worker admission unavailable"));
        }
        let token = uuid::Uuid::new_v4().to_string();
        self.grants.insert(
            worker.into(),
            Grant {
                worker: worker.into(),
                producer: service == "integration",
                token: token.clone(),
                cancelled: Arc::new(AtomicBool::new(false)),
                connections: Default::default(),
            },
        );
        Ok(Access {
            path: self.path.clone(),
            runtime: self.runtime.clone(),
            worker: worker.into(),
            token,
        })
    }
    pub fn authenticate(&self, access: &Access) -> io::Result<Grant> {
        self.grants
            .get(&access.worker)
            .filter(|grant| {
                !self.closed
                    && access.runtime == self.runtime
                    && access.path == self.path
                    && access.token == grant.token
                    && !grant.cancelled.load(Ordering::Acquire)
            })
            .cloned()
            .ok_or_else(|| io::Error::other("Stale or unregistered clipboard worker"))
    }
    pub fn current(&self, grant: &Grant) -> io::Result<()> {
        if self.closed || grant.cancelled.load(Ordering::Acquire) {
            Err(io::Error::other("Clipboard worker lease ended"))
        } else {
            Ok(())
        }
    }
    fn producer(&self, grant: &Grant) -> io::Result<()> {
        self.current(grant)?;
        if !grant.producer || self.owner.as_deref() != Some(grant.worker.as_str()) {
            return Err(io::Error::other("Worker does not own clipboard imports"));
        }
        Ok(())
    }
    fn reset(&mut self) -> io::Result<()> {
        self.cancel_copy();
        self.store.close();
        self.requests.clear();
        self.source = None;
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| io::Error::other("Clipboard revisions exhausted"))?;
        self.store = Store::new();
        self.store.update(clipboard_wire::Offer {
            revision: self.revision,
            formats: vec![],
        });
        Ok(())
    }
    pub fn revoke(&mut self, worker: &str) {
        if let Some(grant) = self.grants.remove(worker) {
            if !grant.producer {
                self.retired.insert(grant.worker.clone());
            }
            grant.revoke();
        }
        if self.owner.as_deref() == Some(worker) {
            self.owner = None;
            self.enabled = false;
            self.last_host = None;
            self.route.exports = false;
            self.route.focused = false;
            self.cancel_exports();
            if self.is_host() && self.reset().is_err() {
                self.close();
            }
        }
        if self.route.worker_id.as_deref() == Some(worker) {
            self.route.worker_id = None;
            self.route.exports = false;
            self.route.focused = false;
            self.native_seen = None;
            self.cancel_exports();
            if !self.is_host() && self.reset().is_err() {
                self.close();
            }
        }
    }
    pub fn close(&mut self) {
        self.closed = true;
        self.cancel_exports();
        self.store.close();
        self.requests.clear();
        for grant in self.grants.values() {
            grant.revoke();
        }
    }
    pub fn claim(&mut self, grant: &Grant) -> io::Result<()> {
        self.current(grant)?;
        if !grant.producer
            || self
                .owner
                .as_ref()
                .is_some_and(|owner| owner != &grant.worker)
        {
            return Err(io::Error::other(
                "Clipboard import producer already owned or unauthorized",
            ));
        }
        self.owner = Some(grant.worker.clone());
        Ok(())
    }
    pub fn enabled(&mut self, grant: &Grant, enabled: bool) -> io::Result<()> {
        self.producer(grant)?;
        if self.enabled != enabled {
            self.last_host = None;
            if self.source.is_none() || self.is_host() {
                self.reset()?;
            }
            self.enabled = enabled;
        }
        Ok(())
    }
    pub fn publish(
        &mut self,
        grant: &Grant,
        revision: i64,
        formats: Vec<String>,
    ) -> io::Result<()> {
        self.producer(grant)?;
        if !self.enabled {
            return Err(io::Error::other("Clipboard import permission is disabled"));
        }
        let offer = clipboard_wire::Offer { revision, formats };
        let offer = clipboard_wire::Offer::decode(&offer.encode())?;
        if self.last_host.as_ref().is_some_and(|source| {
            source.revision == offer.revision && source.formats == offer.formats
        }) {
            return Ok(());
        }
        self.reset()?;
        self.last_host = Some(offer.clone());
        self.source = Some(Source {
            target: Target::Host(offer.revision),
            formats: offer.formats.clone(),
        });
        self.store.update(clipboard_wire::Offer {
            revision: self.revision,
            formats: offer.formats,
        });
        Ok(())
    }
    pub fn offer(&self, grant: &Grant) -> io::Result<Offer> {
        self.current(grant)?;
        Ok(Offer {
            runtime: self.runtime.clone(),
            revision: self.revision,
            origin: self.source.as_ref().map_or(Origin::Empty, Source::origin),
            formats: self
                .source
                .as_ref()
                .map_or_else(Vec::new, |source| source.formats.clone()),
        })
    }
    pub fn readable(
        &self,
        grant: &Grant,
        runtime: &str,
        revision: i64,
        format: &str,
    ) -> io::Result<Arc<Store>> {
        self.current(grant)?;
        if (self.is_host() && !self.enabled)
            || runtime != self.runtime
            || revision != self.revision
            || self
                .source
                .as_ref()
                .is_none_or(|source| !source.formats.iter().any(|mime| mime == format))
        {
            return Err(io::Error::other(
                "Clipboard offer changed or permission revoked",
            ));
        }
        Ok(self.store.clone())
    }
    pub fn poll(&mut self, grant: &Grant) -> io::Result<Vec<(u64, i64, String)>> {
        self.producer(grant)?;
        let Some(Source {
            target: Target::Host(revision),
            ..
        }) = &self.source
        else {
            return Ok(vec![]);
        };
        let revision = *revision;
        Ok(self
            .take_requests()?
            .into_iter()
            .map(|(id, format)| (id, revision, format))
            .collect())
    }
    fn take_requests(&mut self) -> io::Result<Vec<(u64, String)>> {
        let target = self
            .source
            .as_ref()
            .ok_or_else(|| io::Error::other("Clipboard source missing"))?
            .target
            .clone();
        let now = Instant::now();
        self.requests.retain(|_, pending| now < pending.expires);
        let mut frames = vec![];
        self.store.poll(&mut frames)?;
        let mut events = vec![];
        let mut input = frames.as_slice();
        while !input.is_empty() {
            let (kind, bytes) = crate::wire::read_limited(&mut input, 512)?;
            if kind != b'C' {
                return Err(io::Error::other("Invalid internal clipboard request"));
            }
            let request = clipboard_wire::Request::decode(&bytes)?;
            self.upstream_next = self
                .upstream_next
                .checked_add(1)
                .ok_or_else(|| io::Error::other("Clipboard request IDs exhausted"))?;
            events.push((self.upstream_next, request.format.clone()));
            if self.requests.len() >= 8 {
                return Err(io::Error::other(
                    "Clipboard producer request limit exceeded",
                ));
            }
            self.requests.insert(
                self.upstream_next,
                Pending {
                    internal: request,
                    target: target.clone(),
                    expires: now + PENDING_TIMEOUT,
                    ceiling: now + PENDING_CEILING,
                },
            );
        }
        Ok(events)
    }
    pub fn reply(&mut self, grant: &Grant, bytes: &[u8]) -> io::Result<()> {
        self.producer(grant)?;
        let (id, revision, status, done, bytes) = clipboard_wire::parse_reply(bytes)?;
        if !self.enabled
            || self
                .source
                .as_ref()
                .is_none_or(|source| source.target != Target::Host(revision))
        {
            return Ok(());
        }
        let Some(pending) = self.requests.get(&id) else {
            return Ok(());
        };
        if pending.target != Target::Host(revision) {
            return Ok(());
        }
        let now = Instant::now();
        if now >= pending.expires {
            self.requests.remove(&id);
            return Ok(());
        }
        self.store.reply(&clipboard_wire::reply(
            &pending.internal,
            status,
            done,
            &bytes,
        ))?;
        if done {
            self.requests.remove(&id);
        } else if let Some(pending) = self.requests.get_mut(&id) {
            // For the files, a not-done frame, an empty keep-alive included, is
            // progress; other formats keep the fixed window.
            if pending.internal.format == crate::runtime_protocol::clipboard::FILES_FORMAT {
                pending.expires = (now + PENDING_TIMEOUT).min(pending.ceiling);
            }
        }
        Ok(())
    }
    fn is_host(&self) -> bool {
        self.source
            .as_ref()
            .is_some_and(|source| matches!(source.target, Target::Host(_)))
    }
    fn cancel_exports(&mut self) {
        self.cancel_copy();
        self.cancel_focused_exports();
    }
    fn cancel_focused_exports(&mut self) {
        self.export_cancelled.store(true, Ordering::Release);
        self.export_cancelled = Default::default();
    }
    fn cancel_copy(&mut self) {
        if let Some(copy) = self.committed.take() {
            copy.cancelled.store(true, Ordering::Release);
        }
    }
    pub fn configure_route(&mut self, route: Route) -> io::Result<()> {
        if self.closed || route.revision == 0 || route.revision < self.route.revision {
            return Err(io::Error::other(
                "Invalid or stale clipboard route revision",
            ));
        }
        if route.revision == self.route.revision {
            return if route == self.route {
                Ok(())
            } else {
                Err(io::Error::other("Clipboard route revision already used"))
            };
        }
        if let Some(worker) = &route.worker_id {
            if !self
                .grants
                .get(worker)
                .is_some_and(|grant| !grant.producer && !grant.cancelled.load(Ordering::Acquire))
            {
                return Err(io::Error::other(
                    "Native clipboard provider must be an admitted non-integration worker",
                ));
            }
        } else if route.exports || route.focused {
            return Err(io::Error::other(
                "Clipboard export requires a native provider",
            ));
        }
        if route.worker_id != self.route.worker_id {
            self.binding = self
                .binding
                .checked_add(1)
                .ok_or_else(|| io::Error::other("Clipboard provider epochs exhausted"))?;
            self.native_seen = None;
            if self
                .source
                .as_ref()
                .is_some_and(|source| matches!(source.target, Target::Native(..)))
            {
                self.reset()?;
            }
        }
        self.cancel_focused_exports();
        if !route.exports {
            self.cancel_copy();
        }
        self.route = route;
        Ok(())
    }
    pub fn configure_route_v2(
        &mut self,
        route: Route,
    ) -> io::Result<crate::runtime_protocol::clipboard::RouteReply> {
        use crate::runtime_protocol::clipboard::{RouteOutcome, RouteReply};
        // A retired-provider response must not hide malformed/stale revisions
        // or replace a newer provider's route, selections or accepted copy.
        if self.closed || route.revision == 0 || route.revision <= self.route.revision {
            self.configure_route(route.clone())?;
        } else if route
            .worker_id
            .as_ref()
            .is_some_and(|id| self.retired.contains(id))
        {
            return Ok(RouteReply {
                revision: route.revision,
                worker_id: route.worker_id,
                outcome: RouteOutcome::Retired,
            });
        } else {
            self.configure_route(route.clone())?;
        }
        Ok(RouteReply {
            revision: route.revision,
            worker_id: route.worker_id,
            outcome: RouteOutcome::Applied,
        })
    }
    fn provider_lease(&self, grant: &Grant) -> Option<ProviderLease> {
        (self.route.worker_id.as_deref() == Some(grant.worker.as_str())).then(|| ProviderLease {
            runtime: self.runtime.clone(),
            worker: grant.worker.clone(),
            epoch: self.binding,
        })
    }
    fn provider(&self, grant: &Grant, lease: &ProviderLease) -> io::Result<()> {
        self.current(grant)?;
        if self.provider_lease(grant).as_ref() != Some(lease) {
            return Err(io::Error::other("Native clipboard provider lease changed"));
        }
        Ok(())
    }
    pub fn native_poll(&mut self, grant: &Grant) -> io::Result<ProviderPoll> {
        self.current(grant)?;
        let lease = self.provider_lease(grant);
        let source = self
            .source
            .as_ref()
            .and_then(|source| match &source.target {
                Target::Native(owner, revision) if lease.as_ref() == Some(owner) => {
                    Some((owner.clone(), *revision))
                }
                _ => None,
            });
        let requests = if let Some((lease, revision)) = source {
            self.take_requests()?
                .into_iter()
                .map(|(id, format)| NativeRequest {
                    lease: lease.clone(),
                    id,
                    revision,
                    format,
                })
                .collect()
        } else {
            vec![]
        };
        Ok(ProviderPoll {
            lease,
            offer: self.offer(grant)?,
            requests,
        })
    }
    pub fn native_publish(
        &mut self,
        grant: &Grant,
        lease: ProviderLease,
        expected_runtime: &str,
        expected_revision: i64,
        revision: u64,
        formats: Vec<String>,
    ) -> io::Result<bool> {
        self.provider(grant, &lease)?;
        if expected_runtime != self.runtime || revision == 0 {
            return Err(io::Error::other("Invalid native clipboard source identity"));
        }
        let offer = clipboard_wire::Offer {
            revision: 0,
            formats,
        };
        let formats = clipboard_wire::Offer::decode(&offer.encode())?.formats;
        if let Some((seen, previous, accepted)) = &self.native_seen {
            if revision < *seen || (revision == *seen && formats != *previous) {
                return Err(io::Error::other(
                    "Native clipboard source revision already used",
                ));
            }
            if revision == *seen {
                return Ok(*accepted
                    && self
                        .source
                        .as_ref()
                        .is_some_and(|source| source.target == Target::Native(lease, revision)));
            }
        }
        let accepted = expected_revision == self.revision;
        // Even a rejected copy is consumed. It cannot be retried later against
        // a newer host offer and thereby resurrect a superseded native copy.
        self.native_seen = Some((revision, formats.clone(), accepted));
        if !accepted {
            return Ok(false);
        }
        self.reset()?;
        self.source = Some(Source {
            target: Target::Native(lease, revision),
            formats: formats.clone(),
        });
        self.store.update(clipboard_wire::Offer {
            revision: self.revision,
            formats,
        });
        Ok(true)
    }
    pub fn native_reply(
        &mut self,
        grant: &Grant,
        request: NativeRequest,
        chunk: Option<Chunk>,
    ) -> io::Result<()> {
        self.provider(grant, &request.lease)?;
        let target = Target::Native(request.lease, request.revision);
        if self
            .source
            .as_ref()
            .is_none_or(|source| source.target != target)
        {
            return Ok(());
        }
        let Some(pending) = self.requests.get(&request.id) else {
            return Ok(());
        };
        if pending.target != target || pending.internal.format != request.format {
            return Err(io::Error::other(
                "Native clipboard reply does not match its request",
            ));
        }
        if Instant::now() >= pending.expires {
            self.requests.remove(&request.id);
            return Ok(());
        }
        let (status, done, bytes) = match chunk {
            Some(chunk)
                if chunk.bytes.len() <= clipboard_wire::CHUNK
                    && (chunk.eof || !chunk.bytes.is_empty()) =>
            {
                (0, chunk.eof, chunk.bytes)
            }
            None => (1, true, vec![]),
            _ => return Err(io::Error::other("Invalid native clipboard reply chunk")),
        };
        self.store.reply(&clipboard_wire::reply(
            &pending.internal,
            status,
            done,
            &bytes,
        ))?;
        if done {
            self.requests.remove(&request.id);
        }
        Ok(())
    }
    pub fn export_offer(&self, grant: &Grant) -> io::Result<Option<ExportOffer>> {
        self.producer(grant)?;
        if !self.route.exports || !self.route.focused || !self.source.as_ref().is_some_and(|source| {
            !source.formats.is_empty() && matches!(&source.target, Target::Native(lease, _) if self.route.worker_id.as_ref() == Some(&lease.worker) && lease.epoch == self.binding)
        }) { return Ok(None); }
        Ok(Some(ExportOffer {
            offer: self.offer(grant)?,
            route_revision: self.route.revision,
        }))
    }
    pub fn export_access(&self, grant: &Grant, lease: &ExportOffer) -> io::Result<Arc<AtomicBool>> {
        if self.export_offer(grant)?.as_ref() != Some(lease) {
            return Err(io::Error::other(
                "Clipboard export permission, focus or offer changed",
            ));
        }
        Ok(self.export_cancelled.clone())
    }
    pub fn commit_copy(
        &mut self,
        grant: &Grant,
        connection: &str,
        offer: ExportOffer,
    ) -> io::Result<Option<CopyLease>> {
        // A stale proposal must neither gain authority nor replace a copy that
        // was already accepted. Focus is checked under the same lock as commit.
        if self.export_offer(grant)?.as_ref() != Some(&offer) {
            return Ok(None);
        }
        self.cancel_copy();
        let lease = CopyLease {
            id: uuid::Uuid::new_v4().to_string(),
            offer,
        };
        self.committed = Some(Committed {
            lease: lease.clone(),
            connection: connection.into(),
            cancelled: Default::default(),
        });
        Ok(Some(lease))
    }
    pub fn copy_status(&self, grant: &Grant, connection: &str) -> io::Result<Option<CopyLease>> {
        self.producer(grant)?;
        Ok(self
            .committed
            .as_ref()
            .filter(|copy| copy.connection == connection)
            .map(|copy| copy.lease.clone()))
    }
    pub fn copy_access(&self, grant: &Grant, lease: &CopyLease) -> io::Result<Arc<AtomicBool>> {
        self.producer(grant)?;
        self.committed
            .as_ref()
            .filter(|copy| &copy.lease == lease)
            .map(|copy| copy.cancelled.clone())
            .ok_or_else(|| io::Error::other("Committed clipboard copy was revoked"))
    }
    pub fn release_copy(
        &mut self,
        grant: &Grant,
        connection: &str,
        lease: &CopyLease,
    ) -> io::Result<()> {
        self.producer(grant)?;
        if self
            .committed
            .as_ref()
            .is_some_and(|copy| &copy.lease == lease)
        {
            self.release_copy_connection(connection);
        }
        Ok(())
    }
    pub fn release_copy_connection(&mut self, connection: &str) {
        if self
            .committed
            .as_ref()
            .is_some_and(|copy| copy.connection == connection)
        {
            self.cancel_copy();
        }
    }
}
