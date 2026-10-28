use super::*;
use crate::runtime_protocol::clipboard::RouteOutcome;

fn route(revision: u64, worker: &str) -> Route {
    Route {
        revision,
        worker_id: Some(worker.into()),
        exports: true,
        focused: true,
    }
}
fn state() -> State {
    State::new("runtime", PathBuf::from("/unused/clipboard.sock"))
}

#[test]
fn retirement_requires_positive_native_grant_evidence() {
    let mut state = state();
    assert!(state.configure_route_v2(route(1, "unknown")).is_err());
    state.revoke("unknown");
    assert!(state.configure_route_v2(route(1, "unknown")).is_err());
    state.grant("producer", "integration").unwrap();
    assert!(state.configure_route_v2(route(1, "producer")).is_err());
    state.revoke("producer");
    assert!(state.configure_route_v2(route(1, "producer")).is_err());
    let grant = state.grant("native", "remote-apps").unwrap();
    state.configure_route_v2(route(1, "native")).unwrap();
    state.revoke("native");
    assert!(state.authenticate(&grant).is_err());
    assert!(state.grant("native", "remote-apps").is_err());
    let reply = state.configure_route_v2(route(2, "native")).unwrap();
    assert_eq!(reply.revision, 2);
    assert_eq!(reply.worker_id.as_deref(), Some("native"));
    assert_eq!(reply.outcome, RouteOutcome::Retired);
    assert!(
        state.configure_route(route(2, "native")).is_err(),
        "v1 changed"
    );
    assert!(state.route.worker_id.is_none());
}

#[test]
fn retirement_cannot_hide_revision_or_closed_runtime_errors() {
    let mut state = state();
    state.grant("native", "remote-apps").unwrap();
    state.configure_route_v2(route(5, "native")).unwrap();
    state.revoke("native");
    for revision in [0, 4, 5] {
        assert!(state.configure_route_v2(route(revision, "native")).is_err());
    }
    state.close();
    assert!(state.configure_route_v2(route(6, "native")).is_err());
}

#[test]
fn retirement_does_not_change_a_new_providers_route_or_copy_epoch() {
    let mut state = state();
    state.grant("old", "remote-apps").unwrap();
    state.configure_route_v2(route(1, "old")).unwrap();
    state.revoke("old");
    let access = state.grant("new", "remote-apps").unwrap();
    let grant = state.authenticate(&access).unwrap();
    state.configure_route_v2(route(2, "new")).unwrap();
    let lease = state.provider_lease(&grant).unwrap();
    let cancellation = state.export_cancelled.clone();
    let revision = state.revision;
    assert_eq!(
        state.configure_route_v2(route(3, "old")).unwrap().outcome,
        RouteOutcome::Retired
    );
    assert_eq!(state.route, route(2, "new"));
    assert_eq!(state.provider_lease(&grant), Some(lease));
    assert_eq!(state.revision, revision);
    assert!(!cancellation.load(Ordering::Acquire));
}

#[test]
fn tombstones_are_bounded_and_never_evicted_into_ambiguous_retirement() {
    let mut state = state();
    for id in 0..crate::runtime_protocol::MAX_REGISTRATIONS {
        let worker = id.to_string();
        state.grant(&worker, "remote-apps").unwrap();
        state.revoke(&worker);
    }
    assert_eq!(
        state.retired.len(),
        crate::runtime_protocol::MAX_REGISTRATIONS
    );
    assert!(state.grant("overflow", "remote-apps").is_err());
    assert!(state.configure_route_v2(route(1, "overflow")).is_err());
    assert_eq!(
        state.configure_route_v2(route(1, "0")).unwrap().outcome,
        RouteOutcome::Retired
    );
    assert!(state.grant("0", "remote-apps").is_err());
}

#[test]
fn applied_results_preserve_idempotency_and_do_not_enable_import_permission() {
    let mut state = state();
    let access = state.grant("native", "remote-apps").unwrap();
    let grant = state.authenticate(&access).unwrap();
    let reply = state.configure_route_v2(route(1, "native")).unwrap();
    assert_eq!(reply.outcome, RouteOutcome::Applied);
    assert_eq!(state.configure_route_v2(route(1, "native")).unwrap(), reply);
    assert!(!state.enabled);
    assert!(state.enabled(&grant, true).is_err());
    state.revoke("native");
    assert!(state.enabled(&grant, true).is_err());
}
