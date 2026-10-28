//! One host clipboard policy owner. Integration channels supply authenticated
//! offers/copy acknowledgments; native access and GUI focus are independent.
//! No state lock contains native calls, awaits or transport work.
use crate::{
    clipboard_export::{
        CommittedCopy, Connection, Lease,
        source::{Demands, Grant, Source},
    },
    desktop_integration::{
        ClipboardAccess, ClipboardData, ClipboardFuture, ClipboardOffer, ClipboardOrigin,
        ClipboardPublication,
    },
};
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone)]
pub struct Router(Arc<Inner>);
struct Inner {
    native: Arc<dyn ClipboardAccess>,
    closed: Arc<AtomicBool>,
    state: Mutex<State>,
    focus_changed: tokio::sync::watch::Sender<()>,
    /// Where the files of server copies are downloaded; the newest copy wins
    /// across every server.
    #[cfg(unix)]
    downloads: Option<Arc<crate::clipboard_downloads::Cache>>,
}
#[derive(Default)]
struct State {
    next: u64,
    native: Option<ClipboardOffer>,
    endpoints: Vec<Weak<Binding>>,
    focus: Option<Focus>,
    focusing: Option<(u64, u64)>, // request, endpoint
    pending: Option<Pending>,
    accepted: Option<Accepted>,
    displays: Vec<Weak<display::Binding>>,
    selected: Option<display::Selection>,
}
struct Binding {
    id: u64,
    runtime: String,
    connection: Arc<AtomicBool>,
    active: AtomicBool,
    observed: Mutex<Option<i64>>,
    route: Mutex<u64>,
}
impl Binding {
    fn current(&self) -> bool {
        self.active.load(Ordering::Acquire) && !self.connection.load(Ordering::Acquire)
    }
    fn owns(&self, lease: &Lease) -> bool {
        self.current()
            && lease.is_current()
            && self.runtime == lease.offer().offer.runtime
            && Arc::ptr_eq(&self.connection, lease.connection_fence())
    }
}
struct Focus {
    binding: Arc<Binding>,
    grant: Arc<Grant>,
}
struct Pending {
    id: u64,
    binding: Arc<Binding>,
    grant: Arc<Grant>,
    expected: isize,
    source: Option<Arc<Source>>,
}
impl Drop for Pending {
    fn drop(&mut self) {
        if let Some(source) = &self.source {
            source.revoke();
        }
    }
}
struct Accepted {
    binding: Arc<Binding>,
    source: Arc<Source>,
    publication: Box<dyn ClipboardPublication>,
    /// The download of this copy's files.
    #[cfg(unix)]
    download: Option<(
        Arc<crate::clipboard_downloads::Cache>,
        crate::clipboard_downloads::Ticket,
    )>,
}
impl Drop for Accepted {
    fn drop(&mut self) {
        self.source.revoke();
        #[cfg(unix)]
        if let Some((cache, ticket)) = &self.download {
            cache.retire(ticket);
        }
    }
}

/// A copy the native clipboard now holds, as its native publication.
pub struct Published {
    #[cfg(unix)]
    pub(crate) generation: u64,
    #[cfg(unix)]
    pub(crate) source: Arc<Source>,
}

/// One live integration connection. Drop/revoke also fences its pending and
/// accepted copies. A replacement connection requires a new endpoint.
pub struct Endpoint {
    inner: Weak<Inner>,
    binding: Arc<Binding>,
}
impl Endpoint {
    /// A committed copy may be pasted into another runtime through the same
    /// native owner. Suppress its echo only to the originating runtime. This
    /// does not grant clipboard permission: the integration pump still checks
    /// the destination's permission ticket for metadata and every data chunk.
    pub(crate) fn may_forward_import(&self, offer: &ClipboardOffer) -> bool {
        let ClipboardOrigin::Imported { publication } = offer.origin else {
            return false;
        };
        let Some(inner) = self.inner.upgrade() else {
            return false;
        };
        if inner.closed.load(Ordering::Acquire) || !self.binding.current() {
            return false;
        }
        let state = inner.state.lock().unwrap();
        state.native.as_ref() == Some(offer)
            && state.accepted.as_ref().is_some_and(|accepted| {
                accepted.binding.runtime != self.binding.runtime
                    && accepted.binding.current()
                    && accepted.publication.generation() == publication
                    && accepted.publication.is_current()
                    && accepted.source.is_current()
            })
    }

    pub(crate) fn observes_clipboard(&self) -> bool {
        self.inner.upgrade().is_some_and(|inner| {
            if inner.closed.load(Ordering::Acquire) || !self.binding.current() {
                return false;
            }
            let state = inner.state.lock().unwrap();
            state
                .focus
                .as_ref()
                .is_some_and(|f| f.binding.id == self.binding.id)
                || state
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.binding.id == self.binding.id)
                || state
                    .accepted
                    .as_ref()
                    .is_some_and(|a| a.binding.id == self.binding.id && a.source.is_current())
        })
    }
    /// Reserve the global focus generation before sending the runtime route.
    /// A later request on any endpoint invalidates this request's acknowledgment.
    /// The caller must bind the live backend display lease before requesting.
    #[cfg(test)]
    pub(crate) fn begin_focus(&self, worker: uuid::Uuid) -> Option<FocusRequest> {
        let inner = self.inner.upgrade()?;
        let mut state = inner.state.lock().unwrap();
        self.reserve_focus(&inner, &mut state, worker)
    }
    fn reserve_focus(
        &self,
        inner: &Inner,
        state: &mut State,
        worker: uuid::Uuid,
    ) -> Option<FocusRequest> {
        if inner.closed.load(Ordering::Acquire) || !self.binding.current() {
            return None;
        }
        let id = state.next.checked_add(1)?;
        state.next = id;
        lose_focus(state);
        state.focusing = Some((id, self.binding.id));
        Some(FocusRequest {
            inner: self.inner.clone(),
            binding: self.binding.clone(),
            worker: worker.to_string(),
            id,
            armed: true,
        })
    }
    /// A blur from an older endpoint cannot clear another endpoint's focus or
    /// its pending acknowledgment. Permission/session retirement uses revoke.
    pub fn blur(&self) -> bool {
        let Some(inner) = self.inner.upgrade() else {
            return false;
        };
        let mut state = inner.state.lock().unwrap();
        if inner.closed.load(Ordering::Acquire) || !self.binding.current() {
            return false;
        }
        if state
            .focusing
            .is_some_and(|(_, endpoint)| endpoint == self.binding.id)
        {
            state.focusing = None;
        }
        if state
            .focus
            .as_ref()
            .is_some_and(|f| f.binding.id == self.binding.id)
        {
            lose_focus(&mut state);
        }
        true
    }
    #[cfg(test)]
    pub(crate) fn focus(&self, grant: Option<Grant>) -> bool {
        match grant {
            Some(grant) => self
                .begin_focus(uuid::Uuid::parse_str(grant.worker()).unwrap())
                .is_some_and(|request| request.acknowledge(grant)),
            None => self.blur(),
        }
    }
    /// Capture the already observed native revision before sending Commit.
    /// None means no eligible new source, no native baseline, or publication
    /// still pending. The driver retains its latest offer while busy; it must
    /// not replay a source already proposed merely because focus returned.
    pub fn propose(&self, lease: Lease) -> Option<Proposal> {
        let inner = self.inner.upgrade()?;
        let mut state = inner.state.lock().unwrap();
        if inner.closed.load(Ordering::Acquire)
            || !self.binding.owns(&lease)
            || state.pending.is_some()
        {
            return None;
        }
        let focus = state.focus.as_ref()?;
        if focus.binding.id != self.binding.id || !focus.grant.matches(lease.offer()) {
            return None;
        }
        let grant = focus.grant.clone();
        let native = state.native.as_ref()?;
        if state.accepted.as_ref().is_some_and(|a| {
            !a.publication.is_current()
                || native.origin
                    != (ClipboardOrigin::Imported {
                        publication: a.publication.generation(),
                    })
        }) {
            // Revocation withdraws the native promise asynchronously. Wait for
            // its metadata before consuming the next remote source revision.
            return None;
        }
        let expected = native.revision;
        let mut observed = self.binding.observed.lock().unwrap();
        if observed
            .as_ref()
            .is_some_and(|old| lease.offer().offer.revision <= *old)
        {
            return None;
        }
        let id = state.next.checked_add(1)?;
        state.next = id;
        *observed = Some(lease.offer().offer.revision);
        state.pending = Some(Pending {
            id,
            binding: self.binding.clone(),
            grant,
            expected,
            source: None,
        });
        Some(Proposal {
            inner: Arc::downgrade(&inner),
            id,
            lease,
            expected,
            armed: true,
        })
    }
    pub fn revoke(&self) {
        self.binding.active.store(false, Ordering::Release);
        self.withdraw();
    }
    /// Revoke copies and pending writes without retiring the integration
    /// endpoint. A later permission epoch requires fresh runtime authority.
    pub(crate) fn withdraw(&self) {
        if let Some(inner) = self.inner.upgrade() {
            let mut state = inner.state.lock().unwrap();
            if state
                .focusing
                .is_some_and(|(_, endpoint)| endpoint == self.binding.id)
            {
                state.focusing = None;
            }
            if state
                .focus
                .as_ref()
                .is_some_and(|f| f.binding.id == self.binding.id)
            {
                lose_focus(&mut state);
            }
            if state
                .pending
                .as_ref()
                .is_some_and(|p| p.binding.id == self.binding.id)
            {
                state.pending = None;
            }
            if let Some(accepted) = state
                .accepted
                .as_ref()
                .filter(|a| a.binding.id == self.binding.id)
            {
                // Retain only the publication identity until native metadata
                // confirms withdrawal. No source/read authority survives.
                accepted.source.revoke();
            }
        }
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        self.revoke();
    }
}

/// An outstanding focus change, ordered against all other remote sessions.
/// A runtime acknowledgment alone cannot renew an obsolete host focus epoch.
pub struct FocusRequest {
    inner: Weak<Inner>,
    binding: Arc<Binding>,
    worker: String,
    id: u64,
    armed: bool,
}
impl FocusRequest {
    pub fn is_current(&self) -> bool {
        self.inner.upgrade().is_some_and(|inner| {
            !inner.closed.load(Ordering::Acquire)
                && self.binding.current()
                && inner.state.lock().unwrap().focusing == Some((self.id, self.binding.id))
        })
    }
    /// Apply only the grant returned after the actual runtime route succeeds.
    /// This consumes the request; failure leaves newer focus ownership alone.
    pub fn acknowledge(mut self, grant: Grant) -> bool {
        let Some(inner) = self.inner.upgrade() else {
            return false;
        };
        let mut state = inner.state.lock().unwrap();
        if inner.closed.load(Ordering::Acquire)
            || !self.binding.current()
            || state.focusing != Some((self.id, self.binding.id))
            || grant.runtime() != self.binding.runtime
            || grant.worker() != self.worker
            || !grant.is_current()
        {
            return false;
        }
        let mut route = self.binding.route.lock().unwrap();
        if grant.revision() <= *route {
            return false;
        }
        *route = grant.revision();
        state.focusing = None;
        state.focus = Some(Focus {
            binding: self.binding.clone(),
            grant: Arc::new(grant),
        });
        self.armed = false;
        true
    }
}
impl Drop for FocusRequest {
    fn drop(&mut self) {
        if self.armed
            && let Some(inner) = self.inner.upgrade()
        {
            let mut state = inner.state.lock().unwrap();
            if state.focusing == Some((self.id, self.binding.id)) {
                state.focusing = None;
            }
        }
    }
}

/// A single pending copy, including the native revision captured before the
/// remote acknowledgment. Dropping it cancels only its own publication attempt.
pub struct Proposal {
    inner: Weak<Inner>,
    id: u64,
    lease: Lease,
    expected: isize,
    armed: bool,
}
impl Proposal {
    fn accepted(mut self) {
        self.armed = false;
    }
    pub fn lease(&self) -> &Lease {
        &self.lease
    }
    pub fn is_current(&self) -> bool {
        self.inner.upgrade().is_some_and(|inner| {
            !inner.closed.load(Ordering::Acquire)
                && self.lease.is_current()
                && inner
                    .state
                    .lock()
                    .unwrap()
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.id == self.id && p.binding.current())
        })
    }
}
impl Drop for Proposal {
    fn drop(&mut self) {
        if self.armed
            && let Some(inner) = self.inner.upgrade()
        {
            let mut state = inner.state.lock().unwrap();
            if state.pending.as_ref().is_some_and(|p| p.id == self.id) {
                state.pending = None;
            }
        }
    }
}
fn lose_focus(state: &mut State) {
    if let Some(focus) = state.focus.take() {
        focus.grant.revoke();
    }
    // A native confirmation can win the race before its awaiting task runs.
    // Its source already has the copy lifetime and must survive this switch.
    if state
        .pending
        .as_ref()
        .is_some_and(|p| p.source.as_ref().is_none_or(|s| !s.is_current()))
    {
        state.pending = None;
    }
}

impl Router {
    pub fn new(native: Arc<dyn ClipboardAccess>) -> Self {
        Self::with_cache(native, None)
    }
    /// Also downloads the files of server copies, into the cache of the
    /// instance using `profile`.
    pub fn with_file_downloads(
        native: Arc<dyn ClipboardAccess>,
        profile: &std::path::Path,
    ) -> Self {
        #[cfg(unix)]
        let root = crate::clipboard_downloads::Cache::default_root(profile);
        #[cfg(not(unix))]
        let root = {
            let _ = profile;
            None
        };
        Self::with_cache(native, root)
    }
    fn with_cache(native: Arc<dyn ClipboardAccess>, root: Option<std::path::PathBuf>) -> Self {
        #[cfg(not(unix))]
        let _ = root;
        Self(Arc::new(Inner {
            native,
            closed: Arc::new(AtomicBool::new(false)),
            state: Mutex::default(),
            focus_changed: tokio::sync::watch::channel(()).0,
            #[cfg(unix)]
            downloads: root.map(|root| Arc::new(crate::clipboard_downloads::Cache::new(root))),
        }))
    }
    #[cfg(all(test, unix))]
    pub(crate) fn with_download_root(
        native: Arc<dyn ClipboardAccess>,
        root: std::path::PathBuf,
    ) -> Self {
        Self::with_cache(native, Some(root))
    }
    /// Bind an offer issued by the integration receiver, not caller-supplied
    /// UUIDs. Each live connection gets one endpoint and its own observed source.
    pub fn bind(&self, lease: &Lease) -> Option<Endpoint> {
        if !lease.is_current() {
            return None;
        }
        self.bind_connection(lease.connection())
    }
    /// Register before a native offer exists, so focus and clipboard imports
    /// can be prepared without waiting for an application to copy something.
    pub fn bind_connection(&self, connection: Connection) -> Option<Endpoint> {
        let mut state = self.0.state.lock().unwrap();
        if self.0.closed.load(Ordering::Acquire) || !connection.is_current() {
            return None;
        }
        state
            .endpoints
            .retain(|w| w.upgrade().is_some_and(|e| e.current()));
        if state
            .endpoints
            .iter()
            .filter_map(Weak::upgrade)
            .any(|e| Arc::ptr_eq(&e.connection, connection.fence()))
        {
            return None;
        }
        let id = state.next.checked_add(1)?;
        state.next = id;
        let binding = Arc::new(Binding {
            id,
            runtime: connection.runtime().into(),
            connection: connection.fence().clone(),
            active: AtomicBool::new(true),
            observed: Mutex::default(),
            route: Mutex::new(0),
        });
        state.endpoints.push(Arc::downgrade(&binding));
        Some(Endpoint {
            inner: Arc::downgrade(&self.0),
            binding,
        })
    }
    /// Complete a matching runtime acknowledgment using the original native
    /// revision. Await beside the connection's event/demand pump. Cancellation
    /// drops both the native operation and proposal, releasing claimed authority.
    pub fn publish(
        &self,
        proposal: Proposal,
        copy: CommittedCopy,
        demands: &Demands,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<Published>> + Send>> {
        let source = {
            let mut state = self.0.state.lock().unwrap();
            state
                .pending
                .as_mut()
                .filter(|p| {
                    p.id == proposal.id
                        && p.binding.current()
                        && proposal.inner.ptr_eq(&Arc::downgrade(&self.0))
                        && proposal.lease.is_current()
                        && p.grant.matches(proposal.lease.offer())
                        && copy.offer() == proposal.lease.offer()
                        && Arc::ptr_eq(copy.connection_fence(), proposal.lease.connection_fence())
                })
                .and_then(|p| {
                    let source = demands.source_copy(copy, &p.grant)?;
                    p.source = Some(source.clone());
                    Some(source)
                })
        };
        let Some(source) = source else {
            return Box::pin(async move {
                drop(proposal);
                None
            });
        };
        let native = self.0.native.publish(
            proposal.expected,
            source.offer().offer.formats.clone(),
            source.clone(),
        );
        let inner = self.0.clone();
        Box::pin(async move {
            let publication = native.await?;
            let mut state = inner.state.lock().unwrap();
            if inner.closed.load(Ordering::Acquire)
                || !publication.is_current()
                || !source.is_current()
                || state
                    .pending
                    .as_ref()
                    .is_none_or(|p| p.id != proposal.id || !p.binding.current())
            {
                return None;
            }
            let mut pending = state.pending.take().unwrap();
            pending.source.take();
            let published = Published {
                #[cfg(unix)]
                generation: publication.generation(),
                #[cfg(unix)]
                source: source.clone(),
            };
            state.accepted = Some(Accepted {
                binding: pending.binding.clone(),
                source,
                publication,
                #[cfg(unix)]
                download: None,
            });
            drop(state);
            proposal.accepted();
            Some(published)
        })
    }
    /// Record a native observation from the one owner. Old asynchronous replies
    /// never replace newer observations or cancel a newer accepted copy.
    fn observe(&self, offer: &ClipboardOffer) -> bool {
        let mut state = self.0.state.lock().unwrap();
        if self.0.closed.load(Ordering::Acquire) {
            return false;
        }
        if let Some(previous) = &state.native {
            if offer.revision < previous.revision {
                return false;
            }
            if offer.revision == previous.revision {
                return offer == previous;
            }
        }
        if state.pending.as_ref().is_some_and(|p| {
            p.expected != offer.revision
                && (offer.origin == ClipboardOrigin::Local || p.source.is_none())
        }) {
            state.pending = None;
        }
        if state.accepted.as_ref().is_some_and(|a| {
            offer.origin
                != (ClipboardOrigin::Imported {
                    publication: a.publication.generation(),
                })
                || (!a.publication.is_current() && offer.formats.is_empty())
        }) {
            state.accepted = None;
        }
        state.native = Some(offer.clone());
        true
    }
    /// Starts the download of accepted publication `generation`'s files, if
    /// it is still the clipboard's copy. The ticket stays current while the
    /// copy does.
    #[cfg(unix)]
    pub(crate) fn begin_download(
        &self,
        generation: u64,
    ) -> Option<(
        Arc<crate::clipboard_downloads::Cache>,
        crate::clipboard_downloads::Ticket,
    )> {
        let cache = self.0.downloads.clone()?;
        let mut state = self.0.state.lock().unwrap();
        if self.0.closed.load(Ordering::Acquire) {
            return None;
        }
        let accepted = state.accepted.as_mut().filter(|a| {
            a.publication.generation() == generation
                && a.binding.current()
                && a.publication.is_current()
                && a.source.is_current()
        })?;
        let source = accepted.source.clone();
        let ticket = cache.begin(move || source.is_current());
        accepted.download = Some((cache.clone(), ticket.clone()));
        Some((cache, ticket))
    }
    /// Offers the downloaded `folder` in place of accepted publication
    /// `generation`, only while the native clipboard still holds that import.
    /// Yields the new native revision.
    #[cfg(unix)]
    pub(crate) async fn attach_files(
        &self,
        generation: u64,
        folder: &std::path::Path,
    ) -> Option<isize> {
        // The download can finish before the publication's own native
        // revision was observed; a newer local copy is observed here too.
        let _ = self.offer(None).await;
        let attach = {
            let state = self.0.state.lock().unwrap();
            let accepted = state.accepted.as_ref()?;
            if self.0.closed.load(Ordering::Acquire)
                || accepted.publication.generation() != generation
                || !accepted.binding.current()
                || !accepted.publication.is_current()
                || !accepted.source.is_current()
                || state.native.as_ref().is_none_or(|native| {
                    native.origin
                        != (ClipboardOrigin::Imported {
                            publication: generation,
                        })
                })
            {
                return None;
            }
            accepted.publication.attach_files(folder)
        };
        attach.await
    }
    /// Removes superseded downloads whose grace period is over.
    pub(crate) fn tick_downloads(&self) {
        #[cfg(unix)]
        if let Some(cache) = &self.0.downloads {
            cache.tick();
        }
    }
    /// Cancels downloads and removes every downloaded file, blocking on the
    /// removal; for quit, after [`Router::close`].
    pub fn clear_downloads(&self) {
        #[cfg(unix)]
        if let Some(cache) = &self.0.downloads {
            cache.clear();
        }
    }
    pub fn close(&self) {
        self.0.closed.store(true, Ordering::Release);
        let mut state = self.0.state.lock().unwrap();
        state.focusing = None;
        lose_focus(&mut state);
        state.pending = None;
        state.accepted = None;
        state.selected = None;
        self.0.focus_changed.send_replace(());
    }
}
impl ClipboardAccess for Router {
    fn offer(&self, previous: Option<isize>) -> ClipboardFuture<ClipboardOffer> {
        let router = self.clone();
        Box::pin(async move {
            if router.0.closed.load(Ordering::Acquire) {
                return None;
            }
            let offer = router.0.native.offer(previous).await?;
            router.observe(&offer).then_some(offer)
        })
    }
    fn read(&self, revision: isize, format: String) -> ClipboardFuture<Box<dyn ClipboardData>> {
        let router = self.clone();
        Box::pin(async move {
            let offer = router.offer(None).await?;
            if offer.revision != revision || !offer.formats.contains(&format) {
                return None;
            }
            let data = match offer.origin {
                ClipboardOrigin::Local => router.0.native.read(revision, format).await?,
                ClipboardOrigin::Imported { publication } => {
                    let source = {
                        let state = router.0.state.lock().unwrap();
                        let accepted = state.accepted.as_ref()?;
                        if state.native.as_ref() != Some(&offer)
                            || !accepted.binding.current()
                            || accepted.publication.generation() != publication
                            || !accepted.publication.is_current()
                            || !accepted.source.is_current()
                        {
                            return None;
                        }
                        accepted.source.clone()
                    };
                    // The helper deliberately refuses recursive reads of its
                    // own imported promise. Reuse the acknowledged remote copy
                    // and its live fences through the existing source pump.
                    newport_platform::clipboard::provider::read_source(source.as_ref(), &format)
                        .await?
                }
            };
            Some(Box::new(Data {
                data,
                closed: router.0.closed.clone(),
            }) as Box<dyn ClipboardData>)
        })
    }
}
struct Data {
    data: Box<dyn ClipboardData>,
    closed: Arc<AtomicBool>,
}
impl ClipboardData for Data {
    fn bytes(&self) -> Option<&[u8]> {
        if self.closed.load(Ordering::Acquire) {
            None
        } else {
            self.data.bytes()
        }
    }
}

#[cfg(test)]
#[path = "clipboard_router_tests.rs"]
mod tests;

#[path = "clipboard_router_driver.rs"]
pub(crate) mod driver;

#[path = "clipboard_router_display.rs"]
pub mod display;
