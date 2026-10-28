use super::*;
use crate::graphical_session::{Identity, Runtime};

fn identity(remote: &Remote) -> Identity {
    Identity {
        runtime: Runtime {
            instance: remote.runtime,
            build_sha256: "a".repeat(64),
            transport_generation: 1,
        },
        session: Uuid::new_v4(),
        worker: remote.worker,
        display_generation: 1,
    }
}
fn selected(
    router: &Router,
    remote: &Remote,
) -> (
    super::super::display::Display,
    super::super::display::Presenter,
) {
    let display = router.register_display(identity(remote)).unwrap();
    let presenter = display.presenter().unwrap();
    assert!(presenter.set_focused(true));
    (display, presenter)
}
fn acknowledge(endpoint: &Endpoint, remote: &Remote, revision: u64) {
    let focus = endpoint.focused_display().unwrap();
    assert!(
        endpoint
            .begin_display_focus(&focus)
            .unwrap()
            .acknowledge(Grant::new(remote.runtime, remote.worker, revision).unwrap())
    );
}

#[tokio::test]
async fn switching_apps_inside_one_display_preserves_focus_epoch() {
    let (router, _, _, mut remote, endpoint) = setup().await;
    let (_display, presenter) = selected(&router, &remote);
    let mut changes = router.observe_focus();
    let focus = endpoint.focused_display().unwrap();
    let request = endpoint.begin_display_focus(&focus).unwrap();
    remote.offer(2, 2);
    assert!(request.acknowledge(Grant::new(remote.runtime, remote.worker, 2).unwrap()));
    for _ in 0..15 {
        assert!(presenter.set_focused(true));
    }
    assert!(!changes.has_changed().unwrap());
    assert!(focus.is_current());
    assert!(endpoint.propose(remote.lease()).is_some());
    presenter.set_focused(false);
    assert!(changes.has_changed().unwrap());
    changes.borrow_and_update();
    assert!(!focus.is_current());
    assert!(endpoint.focused_display().is_none());
}

#[tokio::test]
async fn blur_and_refocus_reject_both_snapshot_and_inflight_acknowledgment() {
    let (router, _, _, remote, endpoint) = setup().await;
    let (_display, presenter) = selected(&router, &remote);
    let old = endpoint.focused_display().unwrap();
    let request = endpoint.begin_display_focus(&old).unwrap();
    presenter.set_focused(false);
    presenter.set_focused(true);
    assert!(!request.is_current());
    assert!(!request.acknowledge(Grant::new(remote.runtime, remote.worker, 2).unwrap()));
    assert!(!old.is_current());
    assert!(endpoint.begin_display_focus(&old).is_none());
    acknowledge(&endpoint, &remote, 3);
}

#[tokio::test]
async fn retired_presenter_cannot_steal_or_clear_replacement_focus() {
    let (router, _, _, remote, endpoint) = setup().await;
    let (display, old) = selected(&router, &remote);
    let previous = endpoint.focused_display().unwrap();
    let new = display.presenter().unwrap();
    assert!(!previous.is_current());
    assert!(endpoint.focused_display().is_none());
    assert!(new.set_focused(true));
    let current = endpoint.focused_display().unwrap();
    assert!(!old.set_focused(true));
    assert!(!old.set_focused(false));
    drop(old);
    assert!(current.is_current());
    drop(new);
    assert!(!current.is_current());
}

#[tokio::test]
async fn new_session_focus_rejects_delayed_work_and_old_session_blur() {
    let (router, _, _, remote, endpoint) = setup().await;
    let (_display, presenter) = selected(&router, &remote);
    let old = endpoint.focused_display().unwrap();
    let request = endpoint.begin_display_focus(&old).unwrap();
    let other = Remote::new();
    let other_endpoint = router.bind(&other.lease()).unwrap();
    let (_other_display, _other_presenter) = selected(&router, &other);
    let current = other_endpoint.focused_display().unwrap();
    assert!(!old.is_current());
    assert!(endpoint.focused_display().is_none());
    assert!(endpoint.begin_display_focus(&current).is_none());
    assert!(endpoint.begin_display_focus(&old).is_none());
    assert!(!request.is_current());
    presenter.set_focused(false);
    drop(presenter);
    assert!(current.is_current());
    acknowledge(&other_endpoint, &other, 1);
}

#[tokio::test]
async fn ui_focus_alone_grants_no_native_write_authority() {
    let (router, _, _, mut remote, endpoint) = setup().await;
    let (_display, _presenter) = selected(&router, &remote);
    remote.offer(2, 2);
    assert!(endpoint.propose(remote.lease()).is_none());
    let focus = endpoint.focused_display().unwrap();
    let request = endpoint.begin_display_focus(&focus).unwrap();
    assert!(endpoint.propose(remote.lease()).is_none());
    assert!(!request.acknowledge(Grant::new(remote.runtime, Uuid::new_v4(), 2).unwrap()));
    acknowledge(&endpoint, &remote, 2);
    assert!(endpoint.propose(remote.lease()).is_some());
}

#[tokio::test]
async fn blur_synchronously_fences_native_confirmation_without_a_runtime_reply() {
    let (router, _, mut requests, mut remote, endpoint) = setup().await;
    let (_display, presenter) = selected(&router, &remote);
    remote.offer(2, 2);
    acknowledge(&endpoint, &remote, 2);
    let demands = Demands::default();
    let published = start(&router, &mut remote, &endpoint, &demands).await;
    let request = requests.recv().await.unwrap();
    presenter.set_focused(false);
    assert!(!request.source.publication_confirmed());
    drop(request);
    assert!(!published.await.unwrap());
}

#[tokio::test]
async fn confirmed_copy_survives_presenter_detach_but_not_display_retirement() {
    let (router, native, mut requests, mut remote, endpoint) = setup().await;
    let (display, presenter) = selected(&router, &remote);
    remote.offer(2, 2);
    acknowledge(&endpoint, &remote, 2);
    let demands = Demands::default();
    let published = start(&router, &mut remote, &endpoint, &demands).await;
    let request = requests.recv().await.unwrap();
    let source = request.source.clone();
    let publication = native.confirm(&request);
    drop(presenter); // Before native completion is delivered to the router.
    assert!(publication.is_current());
    request.reply.send(publication).ok().unwrap();
    assert!(published.await.unwrap());
    assert!(source.is_current());
    drop(display);
    assert!(!source.is_current());
    let other = Remote::new();
    let (_display, _presenter) = selected(&router, &other);
    let endpoint = router.bind(&other.lease()).unwrap();
    acknowledge(&endpoint, &other, 1);
    assert!(endpoint.propose(other.lease()).is_none());
    router.offer(None).await.unwrap();
    assert!(endpoint.propose(other.lease()).is_none());
    native.withdraw();
    router.offer(None).await.unwrap();
    assert!(endpoint.propose(other.lease()).is_some());
}

#[tokio::test]
async fn retiring_background_display_preserves_another_displays_copy() {
    let (router, native, mut requests, mut remote, endpoint) = setup().await;
    let (display, presenter) = selected(&router, &remote);
    let other = Remote::new();
    let (other_display, other_presenter) = selected(&router, &other);
    presenter.set_focused(true);
    remote.offer(2, 2);
    acknowledge(&endpoint, &remote, 2);
    let demands = Demands::default();
    let published = start(&router, &mut remote, &endpoint, &demands).await;
    let request = requests.recv().await.unwrap();
    let source = request.source.clone();
    let publication = native.confirm(&request);
    request.reply.send(publication).ok().unwrap();
    assert!(published.await.unwrap());
    drop(other_display);
    assert!(!other_presenter.set_focused(true));
    assert!(source.is_current());
    assert!(endpoint.focused_display().unwrap().is_current());
    drop(display);
    assert!(!source.is_current());
}

#[tokio::test]
async fn runtime_retirement_fences_only_exact_display_and_preserves_new_copy() {
    let (router, native, mut requests, mut remote, endpoint) = setup().await;
    let old_identity = identity(&remote);
    let old_display = router.register_display(old_identity.clone()).unwrap();
    let old_presenter = old_display.presenter().unwrap();
    old_presenter.set_focused(true);
    // A newer display uses this same runtime/integration endpoint. Its copy
    // must survive retirement of the older worker, including after blur.
    remote.worker = Uuid::new_v4();
    let new_identity = identity(&remote);
    let new_display = router.register_display(new_identity.clone()).unwrap();
    let new_presenter = new_display.presenter().unwrap();
    new_presenter.set_focused(true);
    remote.offer(2, 2);
    acknowledge(&endpoint, &remote, 2);
    let demands = Demands::default();
    let published = start(&router, &mut remote, &endpoint, &demands).await;
    let request = requests.recv().await.unwrap();
    let source = request.source.clone();
    let publication = native.confirm(&request);
    request.reply.send(publication).ok().unwrap();
    assert!(published.await.unwrap());
    let focus = endpoint.focused_display().unwrap();
    endpoint.retire_display(&old_identity);
    assert!(!old_presenter.set_focused(true));
    assert!(focus.is_current());
    assert!(source.is_current());
    new_presenter.set_focused(false);
    endpoint.retire_display(&old_identity);
    assert!(source.is_current(), "background copy was discarded");
    let mut wrong_generation = new_identity.clone();
    wrong_generation.display_generation += 1;
    endpoint.retire_display(&wrong_generation);
    assert!(source.is_current());
    assert!(new_presenter.set_focused(true));
    endpoint.retire_display(&new_identity);
    assert!(!source.is_current());
    assert!(!new_presenter.set_focused(true));
}

#[tokio::test]
async fn stale_cross_router_and_closed_handles_cannot_reauthorize() {
    let (router, _, _, remote, endpoint) = setup().await;
    let (display, presenter) = selected(&router, &remote);
    let focus = endpoint.focused_display().unwrap();
    let (native, _) = Native::new();
    let other = Router::new(native);
    let other_endpoint = other.bind(&remote.lease()).unwrap();
    assert!(other_endpoint.begin_display_focus(&focus).is_none());
    router.close();
    assert!(!focus.is_current());
    assert!(!presenter.set_focused(true));
    assert!(display.presenter().is_none());
    assert!(endpoint.begin_display_focus(&focus).is_none());
    assert!(router.register_display(identity(&remote)).is_none());
}

#[tokio::test]
async fn registration_rejects_duplicate_workers_and_old_handles_stay_retired() {
    let (router, _, _, remote, endpoint) = setup().await;
    let identity = identity(&remote);
    let display = router.register_display(identity.clone()).unwrap();
    let old = display.presenter().unwrap();
    assert!(router.register_display(identity.clone()).is_none());
    let mut invalid = identity.clone();
    invalid.worker = Uuid::nil();
    assert!(router.register_display(invalid).is_none());
    old.set_focused(true);
    let focus = endpoint.focused_display().unwrap();
    drop(display);
    assert!(!focus.is_current());
    assert!(!old.set_focused(true));
    let next = router.register_display(identity).unwrap();
    let presenter = next.presenter().unwrap();
    presenter.set_focused(true);
    drop(old);
    assert!(endpoint.focused_display().is_some());
}
