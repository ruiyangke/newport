//! Ordered integration-completion regressions. This boundary test does not
//! attribute the separately observed SSH handoff failure to this ordering.
//! No sockets, OS clipboard or SSH run.
use super::recover_connection;
use crate::{
    agent_runtime::protocol::clipboard_export::COPY_VERSION,
    clipboard_export::{Commit, CommittedCopy, Lease, Read, Receiver},
    clipboard_router::driver::route::Resolution as ClipboardRoute,
    clipboard_router::{
        Router,
        driver::{Driver, Transport, Work, route},
    },
    desktop_integration::{
        ClipboardAccess, ClipboardData, ClipboardFuture, ClipboardOffer, ClipboardPublication,
        ClipboardSource,
    },
    graphical_session::{Identity, Runtime},
    integration_permissions::Gate,
};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, watch};

// Exact rejection from tools/agent/src/clipboard_service/state.rs after a
// graphical worker's grant is revoked, even if the runtime stays healthy.
const RETIRED_PROVIDER: &str =
    "Native clipboard provider must be an admitted non-integration worker";

struct NoNativeWrites;
impl ClipboardAccess for NoNativeWrites {
    fn offer(&self, _: Option<isize>) -> ClipboardFuture<ClipboardOffer> {
        Box::pin(async { None })
    }
    fn read(&self, _: isize, _: String) -> ClipboardFuture<Box<dyn ClipboardData>> {
        panic!("A route acknowledgment must not read the native clipboard")
    }
    fn publish(
        &self,
        _: isize,
        _: Vec<String>,
        _: Arc<dyn ClipboardSource>,
    ) -> ClipboardFuture<Box<dyn ClipboardPublication>> {
        panic!("A stale route must never write the native clipboard")
    }
}
struct NoPayload;
impl Transport for NoPayload {
    fn lease(&self) -> Option<Lease> {
        None
    }
    async fn commit(&mut self, _: &Lease) -> Result<Commit, String> {
        panic!("A stale route cannot commit clipboard payloads")
    }
    async fn read(&mut self, _: &CommittedCopy, _: &str) -> Result<Read, String> {
        panic!("A stale route cannot request clipboard payloads")
    }
}
struct Call {
    focused: bool,
    reply: oneshot::Sender<Result<ClipboardRoute, String>>,
}
struct Control {
    identity: Identity,
    gate: Arc<Gate>,
    locally_valid: AtomicBool,
    calls: mpsc::UnboundedSender<Call>,
}
impl route::Control for Control {
    fn permissions(&self) -> &Arc<Gate> {
        &self.gate
    }
    fn validate(&self, identity: &Identity) -> bool {
        *identity == self.identity && self.locally_valid.load(Ordering::SeqCst)
    }
    fn apply(
        &self,
        identity: Identity,
        exports: bool,
        focused: bool,
    ) -> Pin<Box<dyn Future<Output = Result<ClipboardRoute, String>> + Send>> {
        assert_eq!(identity, self.identity);
        assert!(exports);
        let (reply, response) = oneshot::channel();
        self.calls.send(Call { focused, reply }).unwrap();
        Box::pin(async {
            response
                .await
                .expect("Probe must explicitly release the route reply")
        })
    }
}
async fn completed(driver: &mut Driver) -> Result<(), String> {
    loop {
        let work = driver.next().await;
        let completed = matches!(work, Work::Route(route::Work::Complete(_)));
        // This is the same production Driver completion used by run_once's
        // integration select branch, including its error propagation.
        let result = driver.complete(work, &mut NoPayload).await;
        if completed {
            return result;
        }
        result?;
    }
}
async fn stale_completion(
    invalidate_before_completion: bool,
    reply: Result<ClipboardRoute, String>,
    revoke_permission: bool,
) -> Result<(), String> {
    let retired = matches!(
        reply,
        Ok(ClipboardRoute::Retired | ClipboardRoute::Superseded)
    );
    let identity = Identity {
        runtime: Runtime {
            instance: uuid::Uuid::new_v4(),
            build_sha256: "a".repeat(64),
            transport_generation: 1,
        },
        session: uuid::Uuid::new_v4(),
        worker: uuid::Uuid::new_v4(),
        display_generation: 1,
    };
    let gate = Arc::new(Gate::new(true, false));
    assert!(gate.acknowledge(crate::agent::control::Applied {
        permissions: gate.snapshot().desired,
        error: None,
    }));
    let (calls, mut incoming) = mpsc::unbounded_channel();
    let control = Arc::new(Control {
        identity: identity.clone(),
        gate: gate.clone(),
        locally_valid: AtomicBool::new(true),
        calls,
    });
    let mut receiver = Receiver::default();
    receiver
        .enable_version(&identity.runtime.instance.to_string(), COPY_VERSION)
        .unwrap();
    let connection = receiver.connection().unwrap();
    let router = Router::new(Arc::new(NoNativeWrites));
    let display = router.register_display(identity).unwrap();
    let presenter = display.presenter().unwrap();
    let mut driver =
        Driver::with_route_control(router, connection.clone(), control.clone()).unwrap();
    assert!(presenter.set_focused(true));
    driver.tick();
    let ready = incoming.recv().await.unwrap();
    assert!(ready.focused);
    ready.reply.send(Ok(ClipboardRoute::Applied(2))).unwrap();
    completed(&mut driver).await.unwrap();
    assert!(driver.endpoint.observes_clipboard());

    // Blur queues a route change while the backend still considers the worker
    // Ready. The oneshot represents the independent remote revocation reply.
    assert!(presenter.set_focused(false));
    driver.tick();
    let rejected = incoming.recv().await.unwrap();
    assert!(!rejected.focused);
    if invalidate_before_completion {
        control.locally_valid.store(false, Ordering::SeqCst);
    }
    if revoke_permission {
        gate.update(false, false);
    }
    rejected.reply.send(reply).unwrap();
    let result = completed(&mut driver).await;
    if retired {
        assert!(
            !presenter.set_focused(true),
            "Retired presenter regained focus"
        );
        // The runtime reply alone must stop replay even before local invalidation.
        driver.tick();
        assert!(incoming.try_recv().is_err(), "Retired worker was replayed");
    }
    if revoke_permission {
        assert!(gate.clipboard_ticket().is_none());
    }
    if !invalidate_before_completion {
        // Deliberately after consuming the reply: no scheduler or sleep race.
        assert!(control.locally_valid.load(Ordering::SeqCst));
        control.locally_valid.store(false, Ordering::SeqCst);
    }
    driver.tick();
    assert!(!driver.endpoint.observes_clipboard());
    assert!(incoming.try_recv().is_err(), "Retired display reauthorized");
    assert!(
        connection.is_current(),
        "The simulated runtime itself disconnected"
    );
    result
}

async fn recover(invalidate_before_completion: bool) -> (Result<(), String>, usize, usize) {
    let attempts = AtomicUsize::new(0);
    let retries = AtomicUsize::new(0);
    let (_stop, cancel) = watch::channel(false);
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        recover_connection(
            || {
                attempts.fetch_add(1, Ordering::SeqCst);
                stale_completion(
                    invalidate_before_completion,
                    Ok(ClipboardRoute::Retired),
                    false,
                )
            },
            cancel,
            || {
                retries.fetch_add(1, Ordering::SeqCst);
            },
            Duration::ZERO,
            None,
        ),
    )
    .await
    .expect("Synchronized route completion/recovery stalled");
    (
        result,
        attempts.load(Ordering::SeqCst),
        retries.load(Ordering::SeqCst),
    )
}

#[tokio::test]
async fn invalidation_before_stale_reply_preserves_integration_completion() {
    let (result, attempts, retries) = recover(true).await;
    assert_eq!(result, Ok(()));
    assert_eq!((attempts, retries), (1, 0));
}

#[tokio::test]
async fn stale_reply_before_invalidation_preserves_integration_without_retry() {
    let (result, attempts, retries) = recover(false).await;
    assert_eq!(result, Ok(()));
    assert_eq!((attempts, retries), (1, 0));
}

#[tokio::test]
async fn legacy_permission_protocol_and_transport_errors_are_not_retirement() {
    for error in [
        RETIRED_PROVIDER,
        "Clipboard permission denied",
        "Invalid clipboard route acknowledgment",
        "transport read failed",
    ] {
        for invalidation_first in [false, true] {
            let result = tokio::time::timeout(
                Duration::from_secs(2),
                stale_completion(invalidation_first, Err(error.into()), false),
            )
            .await
            .unwrap();
            assert_eq!(result, Err(error.into()));
        }
    }
}

#[tokio::test]
async fn typed_retirement_does_not_restore_revoked_clipboard_permission() {
    for invalidation_first in [false, true] {
        tokio::time::timeout(
            Duration::from_secs(2),
            stale_completion(invalidation_first, Ok(ClipboardRoute::Retired), true),
        )
        .await
        .unwrap()
        .unwrap();
    }
}

#[tokio::test]
async fn local_preflight_cancellation_preserves_integration_without_replay() {
    tokio::time::timeout(
        Duration::from_secs(2),
        stale_completion(true, Ok(ClipboardRoute::Superseded), false),
    )
    .await
    .unwrap()
    .unwrap();
}
