use super::*;
use crate::{
    clipboard_router::driver::route::Resolution as ClipboardRoute,
    clipboard_router::driver::route::{Control, Routes, Work},
    graphical_session::{Identity, Runtime},
    integration_permissions::Gate,
};
use std::{future::Future, pin::Pin, time::Duration};

struct Call {
    identity: Identity,
    exports: bool,
    focused: bool,
    reply: oneshot::Sender<Result<ClipboardRoute, String>>,
}
struct Authority {
    identity: Identity,
    other: Mutex<Option<Identity>>,
    gate: Arc<Gate>,
    valid: AtomicBool,
    calls: mpsc::UnboundedSender<Call>,
}
impl Control for Authority {
    fn permissions(&self) -> &Arc<Gate> {
        &self.gate
    }
    fn validate(&self, identity: &Identity) -> bool {
        (self.valid.load(Ordering::Acquire) && self.identity == *identity)
            || self.other.lock().unwrap().as_ref() == Some(identity)
    }
    fn apply(
        &self,
        identity: Identity,
        exports: bool,
        focused: bool,
    ) -> Pin<Box<dyn Future<Output = Result<ClipboardRoute, String>> + Send>> {
        let (reply, response) = oneshot::channel();
        self.calls
            .send(Call {
                identity,
                exports,
                focused,
                reply,
            })
            .unwrap();
        Box::pin(async {
            response
                .await
                .unwrap_or_else(|_| Err("Runtime closed".into()))
        })
    }
}
fn allowed(gate: &Gate) {
    assert!(gate.acknowledge(crate::agent::control::Applied {
        permissions: gate.snapshot().desired,
        error: None,
    }));
}
struct Fixture {
    router: Router,
    native: Arc<Native>,
    native_requests: mpsc::UnboundedReceiver<Request>,
    remote: Remote,
    endpoint: Endpoint,
    _display: crate::clipboard_router::display::Display,
    presenter: crate::clipboard_router::display::Presenter,
    authority: Arc<Authority>,
    calls: mpsc::UnboundedReceiver<Call>,
    routes: Routes,
}
impl Fixture {
    async fn new() -> Self {
        let (router, native, native_requests, remote, endpoint) = super::setup().await;
        let identity = Identity {
            runtime: Runtime {
                instance: remote.runtime,
                build_sha256: "a".repeat(64),
                transport_generation: 1,
            },
            session: Uuid::new_v4(),
            worker: remote.worker,
            display_generation: 1,
        };
        let display = router.register_display(identity.clone()).unwrap();
        let presenter = display.presenter().unwrap();
        let (sender, calls) = mpsc::unbounded_channel();
        let gate = Arc::new(Gate::new(true, false));
        allowed(&gate);
        let authority = Arc::new(Authority {
            identity,
            other: Mutex::new(None),
            gate,
            valid: AtomicBool::new(true),
            calls: sender,
        });
        let routes = Routes::new(&router, authority.clone());
        presenter.set_focused(true);
        Self {
            router,
            native,
            native_requests,
            remote,
            endpoint,
            _display: display,
            presenter,
            authority,
            calls,
            routes,
        }
    }
    async fn next(&mut self) -> Work {
        tokio::time::timeout(Duration::from_secs(1), self.routes.next())
            .await
            .unwrap()
    }
    async fn completed(&mut self) -> Result<(), String> {
        loop {
            let work = self.next().await;
            let complete = matches!(work, Work::Complete(_));
            self.routes.complete(work, &self.endpoint)?;
            if complete {
                return Ok(());
            }
        }
    }
    fn call(&mut self, exports: bool, focused: bool) -> Call {
        let call = self.calls.try_recv().unwrap();
        assert_eq!(call.identity, self.authority.identity);
        assert_eq!((call.exports, call.focused), (exports, focused));
        call
    }
    async fn focus(&mut self) {
        self.routes.refresh(&self.endpoint);
        self.call(true, true)
            .reply
            .send(Ok(ClipboardRoute::Applied(2)))
            .unwrap();
        self.completed().await.unwrap();
        self.remote.offer(2, 2);
    }
}

#[tokio::test]
async fn route_ack_and_permission_are_required_before_copy_commit() {
    let mut f = Fixture::new().await;
    f.remote.offer(2, 2);
    f.routes.refresh(&f.endpoint);
    assert!(f.endpoint.propose(f.remote.lease()).is_none());
    for _ in 0..15 {
        f.routes.refresh(&f.endpoint);
    }
    let call = f.call(true, true);
    assert!(f.calls.try_recv().is_err(), "duplicate control request");
    call.reply.send(Ok(ClipboardRoute::Applied(2))).unwrap();
    f.completed().await.unwrap();
    assert!(f.endpoint.propose(f.remote.lease()).is_some());
    f.routes.refresh(&f.endpoint);
    assert!(f.calls.try_recv().is_err(), "unchanged focus reauthorized");
}

#[tokio::test]
async fn late_focused_ack_is_followed_by_blur_without_enabling_native_writes() {
    let mut f = Fixture::new().await;
    f.routes.refresh(&f.endpoint);
    let focused = f.call(true, true);
    f.presenter.set_focused(false);
    f.routes.refresh(&f.endpoint);
    assert!(f.calls.try_recv().is_err(), "overlapping control requests");
    focused.reply.send(Ok(ClipboardRoute::Applied(2))).unwrap();
    f.completed().await.unwrap();
    assert!(f.endpoint.propose(f.remote.lease()).is_none());
    f.call(true, false)
        .reply
        .send(Ok(ClipboardRoute::Applied(3)))
        .unwrap();
    f.completed().await.unwrap();
    f.presenter.set_focused(true);
    f.routes.refresh(&f.endpoint);
    f.call(true, true)
        .reply
        .send(Ok(ClipboardRoute::Applied(4)))
        .unwrap();
    f.completed().await.unwrap();
    f.remote.offer(2, 4);
    assert!(f.endpoint.propose(f.remote.lease()).is_some());
}

#[tokio::test]
async fn accepted_copy_survives_blur_route_but_permission_disable_fences_it_synchronously() {
    let mut f = Fixture::new().await;
    f.focus().await;
    let demands = Demands::default();
    let published = start(&f.router, &mut f.remote, &f.endpoint, &demands).await;
    let request = f.native_requests.recv().await.unwrap();
    let source = request.source.clone();
    let publication = f.native.confirm(&request);
    request.reply.send(publication).ok().unwrap();
    assert!(published.await.unwrap());
    f.presenter.set_focused(false);
    f.routes.refresh(&f.endpoint);
    f.call(true, false)
        .reply
        .send(Ok(ClipboardRoute::Applied(3)))
        .unwrap();
    f.completed().await.unwrap();
    assert!(source.is_current());
    f.authority.gate.update(false, false);
    // No route refresh, event handling or remote acknowledgment has run.
    assert!(!source.is_current());
    f.routes.refresh(&f.endpoint);
    f.call(false, false)
        .reply
        .send(Ok(ClipboardRoute::Applied(4)))
        .unwrap();
    f.completed().await.unwrap();
    f.authority.gate.update(true, false);
    allowed(&f.authority.gate);
    assert!(
        !source.is_current(),
        "reenabling revived an old accepted copy"
    );
}

#[tokio::test]
async fn permission_disable_before_native_confirmation_cannot_publish() {
    let mut f = Fixture::new().await;
    f.focus().await;
    let demands = Demands::default();
    let published = start(&f.router, &mut f.remote, &f.endpoint, &demands).await;
    let request = f.native_requests.recv().await.unwrap();
    f.authority.gate.update(false, false);
    assert!(!request.source.publication_confirmed());
    drop(request);
    assert!(!published.await.unwrap());
}

#[tokio::test]
async fn permission_disable_and_reenable_during_ack_requires_fresh_route() {
    let mut f = Fixture::new().await;
    f.routes.refresh(&f.endpoint);
    let old = f.call(true, true);
    f.authority.gate.update(false, false);
    f.authority.gate.update(true, false);
    allowed(&f.authority.gate);
    old.reply.send(Ok(ClipboardRoute::Applied(2))).unwrap();
    f.completed().await.unwrap();
    assert!(f.endpoint.propose(f.remote.lease()).is_none());
    f.call(true, true)
        .reply
        .send(Ok(ClipboardRoute::Applied(3)))
        .unwrap();
    f.completed().await.unwrap();
    f.remote.offer(2, 3);
    assert!(f.endpoint.propose(f.remote.lease()).is_some());
}

#[tokio::test]
async fn preparing_display_retries_without_io_and_retired_display_cannot_use_reply() {
    let mut f = Fixture::new().await;
    f.authority.valid.store(false, Ordering::Release);
    f.routes.refresh(&f.endpoint);
    assert!(f.calls.try_recv().is_err());
    f.authority.valid.store(true, Ordering::Release);
    f.routes.refresh(&f.endpoint);
    let call = f.call(true, true);
    f.authority.valid.store(false, Ordering::Release);
    call.reply.send(Ok(ClipboardRoute::Applied(2))).unwrap();
    f.completed().await.unwrap();
    assert!(!f.endpoint.observes_clipboard());
    assert!(f.calls.try_recv().is_err());
}

#[tokio::test]
async fn rejected_runtime_route_fails_closed() {
    let mut f = Fixture::new().await;
    f.routes.refresh(&f.endpoint);
    f.call(true, true)
        .reply
        .send(Err("worker retired".into()))
        .unwrap();
    assert!(f.completed().await.is_err());
    assert!(f.endpoint.propose(f.remote.lease()).is_none());
}

#[tokio::test]
async fn delayed_old_display_completion_preserves_new_display_copy_and_authority() {
    for retirement in [false, true] {
        let mut f = Fixture::new().await;
        f.routes.refresh(&f.endpoint);
        let old = f.call(true, true);
        f.remote.worker = Uuid::new_v4();
        let mut identity = f.authority.identity.clone();
        identity.worker = f.remote.worker;
        identity.session = Uuid::new_v4();
        identity.display_generation += 1;
        *f.authority.other.lock().unwrap() = Some(identity.clone());
        let display = f.router.register_display(identity.clone()).unwrap();
        let presenter = display.presenter().unwrap();
        presenter.set_focused(true);
        let focus = f.endpoint.focused_display().unwrap();
        assert!(
            f.endpoint
                .begin_display_focus(&focus)
                .unwrap()
                .acknowledge(Grant::new(f.remote.runtime, f.remote.worker, 3).unwrap())
        );
        f.remote.offer(2, 3);
        let demands = Demands::default();
        let published = start(&f.router, &mut f.remote, &f.endpoint, &demands).await;
        let request = f.native_requests.recv().await.unwrap();
        let source = request.source.clone();
        let publication = f.native.confirm(&request);
        request.reply.send(publication).ok().unwrap();
        assert!(published.await.unwrap());
        f.authority.valid.store(false, Ordering::Release);
        old.reply
            .send(Ok(if retirement {
                ClipboardRoute::Retired
            } else {
                ClipboardRoute::Applied(2)
            }))
            .unwrap();
        f.completed().await.unwrap();
        assert!(
            source.is_current(),
            "old display invalidation fenced new copy"
        );
        assert!(focus.is_current());
        assert!(!f.presenter.set_focused(true));
        let fresh = f.calls.try_recv().unwrap();
        assert_eq!(fresh.identity, identity);
        fresh.reply.send(Ok(ClipboardRoute::Applied(4))).unwrap();
        f.completed().await.unwrap();
        assert!(source.is_current());
        assert!(f.calls.try_recv().is_err(), "old worker replayed");
    }
}

#[tokio::test]
async fn backend_preflight_cancels_obsolete_identity_without_runtime_or_transport_io() {
    let directory = tempfile::tempdir().unwrap();
    let registry = crate::server_session::Registry::new(directory.path());
    let server = serde_json::from_value(serde_json::json!({
        "id": Uuid::new_v4(), "name": "clipboard route fixture",
        "sshUser": "fixture", "sshHost": "127.0.0.1", "sshPort": 9
    }))
    .unwrap();
    let owner = registry.session(&server, 0);
    let control = crate::clipboard_router::driver::route::Backend::bind(&owner);
    let f = Fixture::new().await;
    let identity = f.authority.identity.clone();
    assert_eq!(
        control.apply(identity.clone(), true, true).await,
        Ok(ClipboardRoute::Superseded)
    );
    assert!(!owner.has_transport());
    assert_eq!(owner.agent_runtime.snapshot().status, "unknown");
    let _replacement = registry.session(&server, 1);
    assert!(owner.is_revoked());
    assert_eq!(
        control.apply(identity, true, true).await,
        Err("Server session ended before clipboard authorization".into())
    );
    registry.shutdown().await;
}
