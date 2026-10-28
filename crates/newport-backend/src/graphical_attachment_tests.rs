use super::*;
use crate::terminal::{display::Prepared as Display, shell::Registry};
use newport_domain::terminal::gui_protocol::attachment::{Check, Endpoints, Outcome, Reason};
fn request(
    environment: &SharedGuiEnvironment,
    terminal: crate::terminal::shell::Identity,
) -> Check {
    Check {
        id: Uuid::new_v4(),
        session: environment.session,
        terminal: terminal.terminal,
        epoch: terminal.epoch,
        endpoints: Endpoints {
            display: environment.display.clone(),
            authority: environment.authority.clone(),
            wayland_socket: environment.wayland_socket.clone(),
        },
    }
}
#[tokio::test]
async fn attachment_checks_match_exact_retained_endpoints_without_launch_or_acquisition() {
    for mode in [SharedDisplayMode::Mixed, SharedDisplayMode::Wayland] {
        let slot = Slot::default();
        let fixture = Fixture::new(&slot);
        let producer = fixture.clone();
        let lease = slot
            .acquire_mode(
                fixture.runtime.clone(),
                mode,
                Demand::Terminal,
                move |generation| async move { Ok(producer.prepared_mode(generation, mode)) },
            )
            .await
            .unwrap();
        let registry = Registry::default();
        let shell = registry.register(Uuid::new_v4()).unwrap();
        let attempt = shell.begin().unwrap();
        shell.configure(Some(Arc::new(|| {
            panic!("attachment check must never resolve a factory")
        })));
        let request = request(&lease.environment().unwrap(), attempt.identity);
        shell.initialize(Arc::new(Display::Shared(lease))).unwrap();
        assert_eq!(
            registry
                .check_attachment(&request, fixture.session)
                .unwrap()
                .finish(fixture.session)
                .unwrap(),
            Outcome::Current {}
        );
        for key in [
            "session",
            "terminal",
            "epoch",
            "waylandSocket",
            "authority",
            "display",
        ] {
            let mut changed = request.clone();
            let expected = match key {
                "session" => {
                    changed.session = Uuid::new_v4();
                    Reason::SessionMismatch
                }
                "terminal" => {
                    changed.terminal = Uuid::new_v4();
                    Reason::StaleShell
                }
                "epoch" => {
                    changed.epoch = Uuid::new_v4();
                    Reason::StaleShell
                }
                "waylandSocket" => {
                    changed.endpoints.wayland_socket = "/tmp/different".into();
                    Reason::EndpointMismatch
                }
                "authority" if mode == SharedDisplayMode::Mixed => {
                    changed.endpoints.authority = Some("/tmp/other-auth".into());
                    Reason::EndpointMismatch
                }
                "display" if mode == SharedDisplayMode::Mixed => {
                    changed.endpoints.display = Some(":78".into());
                    Reason::EndpointMismatch
                }
                _ => continue,
            };
            assert_eq!(
                registry
                    .check_attachment(&changed, fixture.session)
                    .unwrap()
                    .finish(fixture.session)
                    .unwrap(),
                Outcome::NotCurrent { reason: expected }
            );
        }
        assert!(fixture.driver.requests.lock().unwrap().is_empty());
        assert_eq!(slot.snapshot()[0].terminals, 1);
    }
}
#[tokio::test]
async fn attachment_check_rechecks_epoch_and_generation_before_reply() {
    let slot = Slot::default();
    let fixture = Fixture::new(&slot);
    let lease = fixture.acquire(&slot, Demand::Terminal).await;
    let registry = Registry::default();
    let shell = registry.register(Uuid::new_v4()).unwrap();
    let attempt = shell.begin().unwrap();
    let request = request(&lease.environment().unwrap(), attempt.identity);
    shell.initialize(Arc::new(Display::Shared(lease))).unwrap();
    let check = registry
        .check_attachment(&request, fixture.session)
        .unwrap();
    drop(attempt);
    let replacement = shell.begin().unwrap();
    assert_eq!(
        check.finish(fixture.session).unwrap(),
        Outcome::NotCurrent {
            reason: Reason::StaleShell
        }
    );
    let mut current = request.clone();
    current.epoch = replacement.identity.epoch;
    let check = registry
        .check_attachment(&current, fixture.session)
        .unwrap();
    let generation = slot.snapshot()[0].generation;
    slot.shared.failed(generation, "display retired".into());
    // The replacement deliberately exports identical endpoint strings.
    let mut next_fixture = Fixture::new(&slot);
    next_fixture.session = fixture.session;
    let next = next_fixture.acquire(&slot, Demand::Terminal).await;
    assert!(request.endpoints.matches(&next.environment().unwrap()));
    assert_ne!(generation, next.generation());
    assert!(check.finish(fixture.session).is_err());
    assert!(
        registry
            .check_attachment(&current, fixture.session)
            .is_err()
    );
    assert!(fixture.driver.requests.lock().unwrap().is_empty());
}
#[tokio::test]
async fn attachment_check_fences_worker_and_runtime_identity_changes() {
    for field in ["worker", "runtime", "generation"] {
        let slot = Slot::default();
        let fixture = Fixture::new(&slot);
        let lease = fixture.acquire(&slot, Demand::Terminal).await;
        let registry = Registry::default();
        let shell = registry.register(Uuid::new_v4()).unwrap();
        let attempt = shell.begin().unwrap();
        let request = request(&lease.environment().unwrap(), attempt.identity);
        let generation = lease.generation();
        shell.initialize(Arc::new(Display::Shared(lease))).unwrap();
        let check = registry
            .check_attachment(&request, fixture.session)
            .unwrap();
        {
            let mut state = slot.shared.state.lock().unwrap();
            let identity = &mut state
                .entries
                .get_mut(&generation)
                .unwrap()
                .prepared
                .as_mut()
                .unwrap()
                .identity;
            match field {
                "worker" => identity.worker = Uuid::new_v4(),
                "runtime" => identity.runtime.instance = Uuid::new_v4(),
                _ => identity.display_generation += 1,
            }
        }
        assert!(check.finish(fixture.session).is_err(), "{field}");
    }
}
struct Failure(Arc<Mutex<Option<String>>>);
impl Fence for Failure {
    fn validate(&self) -> Result<(), String> {
        self.0.lock().unwrap().clone().map_or(Ok(()), Err)
    }
}
#[tokio::test]
async fn attachment_errors_preserve_permission_and_transport_failures_in_both_orders() {
    for error in ["permission denied", "transport disconnected"] {
        for after_capture in [false, true] {
            let slot = Slot::default();
            let fixture = Fixture::new(&slot);
            let producer = fixture.clone();
            let failure = Arc::new(Mutex::new(None));
            let moved = failure.clone();
            let lease = slot
                .acquire(
                    fixture.runtime.clone(),
                    Demand::Terminal,
                    move |generation| async move {
                        let mut prepared = producer.prepared(generation);
                        prepared.fence = Box::new(Failure(moved));
                        Ok(prepared)
                    },
                )
                .await
                .unwrap();
            let registry = Registry::default();
            let shell = registry.register(Uuid::new_v4()).unwrap();
            let attempt = shell.begin().unwrap();
            let request = request(&lease.environment().unwrap(), attempt.identity);
            shell.initialize(Arc::new(Display::Shared(lease))).unwrap();
            if after_capture {
                let check = registry
                    .check_attachment(&request, fixture.session)
                    .unwrap();
                *failure.lock().unwrap() = Some(error.into());
                assert_eq!(check.finish(fixture.session).unwrap_err(), error);
            } else {
                *failure.lock().unwrap() = Some(error.into());
                assert_eq!(
                    registry
                        .check_attachment(&request, fixture.session)
                        .err()
                        .unwrap(),
                    error
                );
            }
            assert!(fixture.driver.requests.lock().unwrap().is_empty());
        }
    }
}
#[test]
fn attachment_without_shared_lease_never_initializes_one() {
    let registry = Registry::default();
    let shell = registry.register(Uuid::new_v4()).unwrap();
    let attempt = shell.begin().unwrap();
    shell.configure(Some(Arc::new(|| panic!("no implicit initialization"))));
    let session = Uuid::new_v4();
    let request = Check {
        id: Uuid::new_v4(),
        session,
        terminal: attempt.identity.terminal,
        epoch: attempt.identity.epoch,
        endpoints: Endpoints {
            display: None,
            authority: None,
            wayland_socket: "/tmp/unverified".into(),
        },
    };
    assert_eq!(
        registry
            .check_attachment(&request, session)
            .unwrap()
            .finish(session)
            .unwrap(),
        Outcome::NotCurrent {
            reason: Reason::NoSharedDisplay
        }
    );
    assert!(shell.display().unwrap().is_none());
}
