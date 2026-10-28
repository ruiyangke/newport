use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::{Semaphore, oneshot};
#[path = "graphical_attachment_tests.rs"]
mod attachment;
#[cfg(unix)]
#[path = "graphical_session_terminal_tests.rs"]
mod terminal;

struct Probe {
    updates: watch::Sender<DriverState>,
    stops: AtomicUsize,
    owner: Weak<Shared>,
    requests: Mutex<Vec<(launch::Target, oneshot::Sender<launch::Outcome>)>>,
    app_next: std::sync::atomic::AtomicU64,
    app_states: Mutex<BTreeMap<std::num::NonZeroU64, launch::State>>,
    app_stops: Mutex<Vec<std::num::NonZeroU64>>,
    app_endings: Mutex<BTreeMap<std::num::NonZeroU64, launch::Ending>>,
    app_notices: Mutex<BTreeMap<std::num::NonZeroU64, launch::Notice>>,
}
impl Probe {
    fn publish(&self, windows: u32, applications: u32) {
        self.updates.send_replace(DriverState::Ready {
            windows,
            applications,
        });
    }
}
impl Driver for Probe {
    fn state(&self) -> watch::Receiver<DriverState> {
        self.updates.subscribe()
    }
    fn launch(&self, target: launch::Target) -> Result<launch::Reply, String> {
        if let Some(owner) = self.owner.upgrade() {
            drop(
                owner
                    .state
                    .try_lock()
                    .expect("launch called under owner lock"),
            );
        }
        let mut updates = self.updates.borrow().clone();
        let DriverState::Ready { applications, .. } = &mut updates else {
            return Err("Driver stopped".into());
        };
        *applications += 1;
        let (send, reply) = oneshot::channel();
        self.requests.lock().unwrap().push((target, send));
        self.updates.send_replace(updates);
        Ok(Box::pin(async move {
            reply
                .await
                .unwrap_or_else(|_| launch::Outcome::Unknown("Peer ended".into()))
        }))
    }
    fn launch_application(&self, target: launch::Target) -> Result<launch::Submitted, String> {
        let reply = self.launch(target)?;
        let request = (self.app_next.fetch_add(1, Ordering::SeqCst) + 1)
            .try_into()
            .unwrap();
        self.app_states
            .lock()
            .unwrap()
            .insert(request, launch::State::Pending);
        Ok(launch::Submitted { request, reply })
    }
    fn application_state(&self, request: std::num::NonZeroU64) -> Result<launch::State, String> {
        if let Some(owner) = self.owner.upgrade() {
            drop(
                owner
                    .state
                    .try_lock()
                    .expect("application state under owner lock"),
            );
        }
        self.app_states
            .lock()
            .unwrap()
            .get(&request)
            .copied()
            .ok_or_else(|| "Unknown app".into())
    }
    fn application_ending(
        &self,
        request: std::num::NonZeroU64,
    ) -> Result<Option<launch::Ending>, String> {
        self.application_state(request)?;
        Ok(self.app_endings.lock().unwrap().get(&request).cloned())
    }
    fn application_notice(
        &self,
        request: std::num::NonZeroU64,
    ) -> Result<Option<launch::Notice>, String> {
        self.application_state(request)?;
        Ok(self.app_notices.lock().unwrap().get(&request).copied())
    }
    fn stop_application(&self, request: std::num::NonZeroU64) -> Result<bool, String> {
        self.application_state(request)?;
        self.app_stops.lock().unwrap().push(request);
        let mut states = self.app_states.lock().unwrap();
        let state = states.get_mut(&request).unwrap();
        if *state == launch::State::Stopping || *state == launch::State::Ended {
            return Ok(false);
        }
        *state = launch::State::Stopping;
        Ok(true)
    }
    fn stop(&self) {
        // Stopping may call back into the owner. A lock inversion here would
        // deadlock the real private adapter's cleanup too.
        if let Some(owner) = self.owner.upgrade() {
            drop(
                owner
                    .state
                    .try_lock()
                    .expect("driver stopped under owner lock"),
            );
        }
        self.stops.fetch_add(1, Ordering::SeqCst);
        self.updates.send_replace(DriverState::Closed);
        self.requests.lock().unwrap().clear();
    }
}
struct Validity(Arc<AtomicBool>);
impl Fence for Validity {
    fn validate(&self) -> Result<(), String> {
        if self.0.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err("Transport retired".into())
        }
    }
}
#[derive(Clone)]
struct Fixture {
    runtime: Runtime,
    session: Uuid,
    driver: Arc<Probe>,
    valid: Arc<AtomicBool>,
}
impl Fixture {
    fn new(slot: &Slot) -> Self {
        Self {
            runtime: Runtime {
                instance: Uuid::new_v4(),
                build_sha256: "a".repeat(64),
                transport_generation: 1,
            },
            session: Uuid::new_v4(),
            driver: Arc::new(Probe {
                updates: watch::channel(DriverState::Ready {
                    windows: 0,
                    applications: 0,
                })
                .0,
                stops: AtomicUsize::new(0),
                owner: Arc::downgrade(&slot.shared),
                requests: Mutex::new(Vec::new()),
                app_next: Default::default(),
                app_states: Default::default(),
                app_stops: Default::default(),
                app_endings: Default::default(),
                app_notices: Default::default(),
            }),
            valid: Arc::new(AtomicBool::new(true)),
        }
    }
    fn prepared(&self, generation: u64) -> Prepared {
        Prepared {
            identity: Identity {
                runtime: self.runtime.clone(),
                session: self.session,
                worker: Uuid::new_v4(),
                display_generation: generation,
            },
            attached: Attached {
                terminal_bus: Default::default(),
                environment: SharedGuiEnvironment {
                    session: self.session,
                    display: Some(":77".into()),
                    authority: Some("/tmp/newport-session/Xauthority".into()),
                    wayland_socket: "/tmp/newport-session/wayland-0".into(),
                    fallback_bus: None,
                    toolkit: BTreeMap::new(),
                    application: BTreeMap::new(),
                },
                driver: self.driver.clone(),
            },
            fence: Box::new(Validity(self.valid.clone())),
        }
    }
    fn prepared_mode(&self, generation: u64, mode: SharedDisplayMode) -> Prepared {
        let mut prepared = self.prepared(generation);
        if mode == SharedDisplayMode::Wayland {
            prepared.attached.environment.display = None;
            prepared.attached.environment.authority = None;
        }
        prepared
    }
    async fn acquire(&self, slot: &Slot, demand: Demand) -> Lease {
        let fixture = self.clone();
        slot.acquire(self.runtime.clone(), demand, move |generation| async move {
            Ok(fixture.prepared(generation))
        })
        .await
        .unwrap()
    }
}
async fn changed_until(slot: &Slot, predicate: impl Fn(&[Snapshot]) -> bool) {
    let mut changes = slot.shared.changes.subscribe();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !predicate(&slot.snapshot()) {
            changes.changed().await.unwrap();
        }
    })
    .await
    .expect("owner did not reach expected state");
}
async fn eventually(predicate: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !predicate() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("operation did not complete");
}
struct Dropped(Arc<AtomicUsize>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn command() -> launch::Target {
    launch::Target::Command {
        argv: vec!["app".into(), "$(literal); 'argument'".into()],
    }
}

#[tokio::test]
async fn changing_display_mode_cannot_reuse_a_live_opposite_display() {
    for mode in [SharedDisplayMode::Mixed, SharedDisplayMode::Wayland] {
        let opposite = if mode == SharedDisplayMode::Mixed {
            SharedDisplayMode::Wayland
        } else {
            SharedDisplayMode::Mixed
        };
        let slot = Slot::default();
        let fixture = Fixture::new(&slot);
        let preparing = fixture.clone();
        let current = slot
            .acquire_mode(
                fixture.runtime.clone(),
                mode,
                Demand::Terminal,
                move |generation| async move { Ok(preparing.prepared_mode(generation, mode)) },
            )
            .await
            .unwrap();
        fixture.driver.publish(3, 3);
        for runtime in [
            fixture.runtime.clone(),
            Runtime {
                build_sha256: "b".repeat(64),
                ..fixture.runtime.clone()
            },
        ] {
            let attempt = slot
                .acquire_mode(runtime, opposite, Demand::Launch, |_| async {
                    panic!("conflicting mode must not start another worker")
                })
                .await;
            assert!(attempt.is_err(), "opposite mode reused the live display");
        }
        current.environment().unwrap();
        assert_eq!(slot.snapshot()[0].mode, mode);
        assert_eq!(fixture.driver.stops.load(Ordering::SeqCst), 0);
        drop(current);
        changed_until(&slot, |state| state[0].terminals == 0).await;
        assert_eq!(
            fixture.driver.stops.load(Ordering::SeqCst),
            0,
            "windows retain their display"
        );
        fixture.driver.publish(0, 0);
        slot.drained().await;
        let mut replacement = Fixture::new(&slot);
        replacement.runtime = fixture.runtime.clone();
        let preparing = replacement.clone();
        let next = slot
            .acquire_mode(
                replacement.runtime.clone(),
                opposite,
                Demand::Terminal,
                move |generation| async move { Ok(preparing.prepared_mode(generation, opposite)) },
            )
            .await
            .unwrap();
        opposite
            .validate_environment(&next.environment().unwrap())
            .unwrap();
        assert_eq!(slot.snapshot()[0].mode, opposite);
    }
}

#[tokio::test]
async fn prepared_environment_must_match_the_selected_mode() {
    for mode in [SharedDisplayMode::Mixed, SharedDisplayMode::Wayland] {
        let opposite = if mode == SharedDisplayMode::Mixed {
            SharedDisplayMode::Wayland
        } else {
            SharedDisplayMode::Mixed
        };
        let slot = Slot::default();
        let fixture = Fixture::new(&slot);
        let preparing = fixture.clone();
        let attempt = slot
            .acquire_mode(
                fixture.runtime.clone(),
                mode,
                Demand::Terminal,
                move |generation| async move { Ok(preparing.prepared_mode(generation, opposite)) },
            )
            .await;
        assert!(attempt.is_err());
        slot.drained().await;
        assert_eq!(fixture.driver.stops.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn native_requests_coalesce_and_opposite_mode_cannot_join_pending_startup() {
    let slot = Arc::new(Slot::default());
    let fixture = Fixture::new(&slot);
    let gate = Arc::new(Semaphore::new(0));
    let waiting = slot.clone();
    let preparing = fixture.clone();
    let release = gate.clone();
    let task = tokio::spawn(async move {
        waiting
            .acquire_mode(
                preparing.runtime.clone(),
                SharedDisplayMode::Wayland,
                Demand::Terminal,
                move |generation| async move {
                    let _permit = release.acquire().await.unwrap();
                    Ok(preparing.prepared_mode(generation, SharedDisplayMode::Wayland))
                },
            )
            .await
            .unwrap()
    });
    changed_until(&slot, |state| {
        state.len() == 1 && state[0].phase == Phase::Preparing
    })
    .await;
    assert!(
        slot.acquire_mode(
            fixture.runtime.clone(),
            SharedDisplayMode::Mixed,
            Demand::Launch,
            |_| async { panic!("opposite mode cannot join pending startup") }
        )
        .await
        .is_err()
    );
    gate.add_permits(1);
    let first = task.await.unwrap();
    let mut leases = vec![first];
    for _ in 0..15 {
        leases.push(
            slot.acquire_mode(
                fixture.runtime.clone(),
                SharedDisplayMode::Wayland,
                Demand::Launch,
                |_| async { panic!("same mode must reuse the ready display") },
            )
            .await
            .unwrap(),
        );
    }
    assert_eq!(slot.snapshot().len(), 1);
    assert_eq!(slot.snapshot()[0].launches, 15);
    assert!(
        leases
            .iter()
            .all(|lease| lease.identity().unwrap() == leases[0].identity().unwrap())
    );
    drop(leases);
    slot.drained().await;
}

#[tokio::test]
async fn clipboard_identity_validation_never_adopts_replacements_or_creates_demand() {
    let slot = Slot::default();
    let fixture = Fixture::new(&slot);
    let terminal = fixture.acquire(&slot, Demand::Terminal).await;
    let identity = terminal.identity().unwrap();
    slot.validate_identity(&identity).unwrap();
    assert_eq!(slot.snapshot()[0].clipboard, 0);
    for variant in 0..6 {
        let mut invalid = identity.clone();
        match variant {
            0 => invalid.worker = Uuid::new_v4(),
            1 => invalid.session = Uuid::new_v4(),
            2 => invalid.runtime.instance = Uuid::new_v4(),
            3 => invalid.runtime.build_sha256 = "b".repeat(64),
            4 => invalid.runtime.transport_generation += 1,
            _ => invalid.display_generation += 1,
        }
        assert!(slot.validate_identity(&invalid).is_err());
    }
    fixture.valid.store(false, Ordering::SeqCst);
    assert!(slot.validate_identity(&identity).is_err());
    fixture.valid.store(true, Ordering::SeqCst);
    slot.close();
    assert!(slot.validate_identity(&identity).is_err());
}

#[tokio::test]
async fn fifteen_launches_use_one_generation_and_report_independent_outcomes() {
    let slot = Slot::default();
    let fixture = Fixture::new(&slot);
    let terminal = fixture.acquire(&slot, Demand::Terminal).await;
    let identity = terminal.identity().unwrap();
    let mut pending = Vec::new();
    for _ in 0..15 {
        let lease = fixture.acquire(&slot, Demand::Launch).await;
        assert_eq!(lease.identity().unwrap(), identity);
        pending.push(lease.launch(command()).unwrap());
        // Pending demand must already be published, before any reply is polled.
        drop(lease);
    }
    drop(terminal);
    let snapshot = slot.snapshot();
    assert_eq!(snapshot.len(), 1);
    assert_eq!((snapshot[0].launches, snapshot[0].applications), (0, 15));
    assert_eq!(fixture.driver.stops.load(Ordering::SeqCst), 0);
    let requests = std::mem::take(&mut *fixture.driver.requests.lock().unwrap());
    assert_eq!(requests.len(), 15);
    fixture.driver.publish(0, 8); // Eight windowless applications survive replies.
    for (index, (target, reply)) in requests.into_iter().enumerate().rev() {
        assert_eq!(target, command());
        reply
            .send(if index % 2 == 0 {
                launch::Outcome::Accepted
            } else {
                launch::Outcome::Rejected("Not installed".into())
            })
            .unwrap();
    }
    for (index, request) in pending.into_iter().enumerate() {
        assert_eq!(
            request.outcome().await,
            if index % 2 == 0 {
                launch::Outcome::Accepted
            } else {
                launch::Outcome::Rejected("Not installed".into())
            }
        );
    }
    assert_eq!(fixture.driver.stops.load(Ordering::SeqCst), 0);
    fixture.driver.publish(0, 0);
    slot.drained().await;
    assert_eq!(fixture.driver.stops.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn dropped_or_timed_out_reply_does_not_cancel_demand_or_replay_launch() {
    for timeout in [false, true] {
        let slot = Slot::default();
        let fixture = Fixture::new(&slot);
        let lease = fixture.acquire(&slot, Demand::Launch).await;
        let pending = lease.launch(command()).unwrap();
        drop(lease);
        if timeout {
            // The deadline starts at submission, not at the first poll.
            tokio::time::advance(Duration::from_secs(31)).await;
            assert!(matches!(
                pending.outcome().await,
                launch::Outcome::Unknown(_)
            ));
        } else {
            drop(pending);
        }
        assert_eq!(slot.snapshot()[0].applications, 1);
        assert_eq!(fixture.driver.stops.load(Ordering::SeqCst), 0);
        let requests = std::mem::take(&mut *fixture.driver.requests.lock().unwrap());
        assert_eq!(requests.len(), 1);
        let (_, reply) = requests.into_iter().next().unwrap();
        fixture.driver.publish(0, 1);
        assert!(reply.send(launch::Outcome::Accepted).is_err());
        assert_eq!(fixture.driver.stops.load(Ordering::SeqCst), 0);
        fixture.driver.publish(0, 0);
        slot.drained().await;
    }
}

#[tokio::test]
async fn owner_close_fences_new_launches_and_pending_reply_does_not_retain_driver() {
    let slot = Slot::default();
    let fixture = Fixture::new(&slot);
    let weak = Arc::downgrade(&fixture.driver);
    let lease = fixture.acquire(&slot, Demand::Launch).await;
    let pending = lease.launch(command()).unwrap();
    slot.close();
    assert!(lease.launch(command()).is_err());
    assert!(matches!(
        pending.outcome().await,
        launch::Outcome::Unknown(_)
    ));
    drop(fixture);
    assert!(weak.upgrade().is_none());
    drop(lease);
}

#[tokio::test]
async fn ending_all_displays_drops_held_ones_and_admits_new_ones() {
    let slot = Slot::default();
    let fixture = Fixture::new(&slot);
    let terminal = fixture.acquire(&slot, Demand::Terminal).await;
    let identity = terminal.identity().unwrap();
    slot.end_all();
    assert!(slot.snapshot().is_empty());
    assert!(slot.validate_identity(&identity).is_err());
    drop(terminal);
    let again = Fixture::new(&slot).acquire(&slot, Demand::Terminal).await;
    slot.validate_identity(&again.identity().unwrap()).unwrap();
}

#[tokio::test]
async fn concurrent_consumers_share_startup_even_when_initiator_cancels() {
    let slot = Arc::new(Slot::default());
    let fixture = Fixture::new(&slot);
    let starts = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(Semaphore::new(0));
    let mut callers = Vec::new();
    for index in 0..21 {
        let (caller_slot, fixture, caller_starts, gate) =
            (slot.clone(), fixture.clone(), starts.clone(), gate.clone());
        callers.push(tokio::spawn(async move {
            let demand = [Demand::Terminal, Demand::Clipboard, Demand::Launch][index % 3];
            caller_slot
                .acquire(
                    fixture.runtime.clone(),
                    demand,
                    move |generation| async move {
                        caller_starts.fetch_add(1, Ordering::SeqCst);
                        let _permit = gate.acquire().await.unwrap();
                        Ok(fixture.prepared(generation))
                    },
                )
                .await
        }));
        // Ensure this is the caller that initiates the owned startup.
        if index == 0 {
            eventually(|| starts.load(Ordering::SeqCst) == 1).await;
        }
    }
    changed_until(&slot, |s| {
        s[0].terminals + s[0].clipboard + s[0].launches == 21
    })
    .await;
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    let initiator = callers.remove(0);
    initiator.abort();
    assert!(initiator.await.is_err_and(|error| error.is_cancelled()));
    assert_eq!(slot.snapshot()[0].phase, Phase::Preparing);
    gate.add_permits(1);
    let mut leases = Vec::new();
    for caller in callers {
        leases.push(caller.await.unwrap().unwrap());
    }
    let identity = leases[0].identity().unwrap();
    let environment = leases[0].environment().unwrap();
    for lease in &leases {
        assert_eq!(lease.identity().unwrap(), identity);
        assert_eq!(lease.environment().unwrap(), environment);
    }
    let current = &slot.snapshot()[0];
    assert_eq!(
        (current.terminals, current.clipboard, current.launches),
        (6, 7, 7)
    );
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    drop(leases);
    assert!(slot.snapshot().is_empty());
    assert_eq!(fixture.driver.stops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancelling_final_waiter_cancels_owned_preparation_and_allows_new_generation() {
    let slot = Arc::new(Slot::default());
    let fixture = Fixture::new(&slot);
    let dropped = Arc::new(AtomicUsize::new(0));
    let (started, ready) = oneshot::channel();
    let caller = {
        let (slot, fixture, dropped) = (slot.clone(), fixture.clone(), dropped.clone());
        tokio::spawn(async move {
            slot.acquire(
                fixture.runtime.clone(),
                Demand::Terminal,
                move |_| async move {
                    let _drop = Dropped(dropped);
                    started.send(()).unwrap();
                    std::future::pending::<Result<Prepared, String>>().await
                },
            )
            .await
        })
    };
    ready.await.unwrap();
    let old_generation = slot.snapshot()[0].generation;
    caller.abort();
    assert!(caller.await.is_err_and(|error| error.is_cancelled()));
    eventually(|| dropped.load(Ordering::SeqCst) == 1).await;
    assert!(slot.snapshot().is_empty());
    let lease = fixture.acquire(&slot, Demand::Terminal).await;
    assert!(lease.generation() > old_generation);
    drop(lease);
    assert_eq!(fixture.driver.stops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn releasing_launch_sees_fresh_application_state_before_watcher_runs() {
    let slot = Slot::default();
    let fixture = Fixture::new(&slot);
    let launch = fixture.acquire(&slot, Demand::Launch).await;
    fixture.driver.publish(1, 1);
    drop(launch); // Deliberately no yield: the owner watcher still sees zero.
    assert_eq!(slot.snapshot()[0].windows, 1);
    assert_eq!(fixture.driver.stops.load(Ordering::SeqCst), 0);
    fixture.driver.publish(0, 1); // A windowless app still owns its session.
    changed_until(&slot, |s| s[0].windows == 0).await;
    assert_eq!(fixture.driver.stops.load(Ordering::SeqCst), 0);
    fixture.driver.publish(0, 0);
    slot.drained().await;
    assert_eq!(fixture.driver.stops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn clipboard_and_terminal_demand_keep_idle_display_ready_after_last_window() {
    let slot = Slot::default();
    let fixture = Fixture::new(&slot);
    let clipboard = fixture.acquire(&slot, Demand::Clipboard).await;
    let environment = clipboard.environment().unwrap();
    fixture.driver.publish(1, 1);
    changed_until(&slot, |s| s[0].windows == 1).await;
    let terminal = fixture.acquire(&slot, Demand::Terminal).await;
    assert_eq!(clipboard.identity().unwrap(), terminal.identity().unwrap());
    fixture.driver.publish(0, 0);
    changed_until(&slot, |s| s[0].windows == 0).await;
    drop(terminal);
    assert_eq!(clipboard.environment().unwrap(), environment);
    assert_eq!(fixture.driver.stops.load(Ordering::SeqCst), 0);
    drop(clipboard);
    slot.drained().await;
    assert_eq!(fixture.driver.stops.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn transport_failure_fences_old_leases_and_explicit_retry_never_adopts_old_generation() {
    let slot = Slot::default();
    let fixture = Fixture::new(&slot);
    let old = fixture.acquire(&slot, Demand::Terminal).await;
    let generation = old.generation();
    fixture.valid.store(false, Ordering::SeqCst);
    assert_eq!(old.environment().unwrap_err(), "Transport retired");
    assert!(old.launch(command()).is_err());
    assert!(fixture.driver.requests.lock().unwrap().is_empty());
    tokio::time::advance(Duration::from_millis(251)).await;
    changed_until(&slot, |s| matches!(s[0].phase, Phase::Failed(_))).await;
    assert_eq!(fixture.driver.stops.load(Ordering::SeqCst), 1);
    let mut next = Fixture::new(&slot);
    next.runtime = fixture.runtime.clone();
    next.session = fixture.session;
    let replacement = next.acquire(&slot, Demand::Terminal).await;
    assert!(replacement.generation() > generation);
    assert!(old.environment().is_err());
    assert!(old.identity().is_err());
    assert!(old.launch(command()).is_err());
    assert!(next.driver.requests.lock().unwrap().is_empty());
    drop(old);
    assert!(replacement.environment().is_ok());
    assert_eq!(next.driver.stops.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn driver_failure_with_no_consumers_retires_running_app_and_unblocks_drain() {
    let slot = Slot::default();
    let fixture = Fixture::new(&slot);
    let lease = fixture.acquire(&slot, Demand::Launch).await;
    fixture.driver.publish(1, 1);
    drop(lease);
    fixture
        .driver
        .updates
        .send_replace(DriverState::Failed("compositor crashed".into()));
    slot.drained().await;
    assert!(slot.snapshot().is_empty());
    assert_eq!(fixture.driver.stops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn runtime_instance_build_and_transport_changes_use_separate_pinned_sessions() {
    let slot = Slot::default();
    let first = Fixture::new(&slot);
    let mut fixtures = vec![first.clone()];
    let mut leases = vec![first.acquire(&slot, Demand::Terminal).await];
    for change in 0..3 {
        let mut next = Fixture::new(&slot);
        next.runtime = first.runtime.clone();
        match change {
            0 => next.runtime.instance = Uuid::new_v4(),
            1 => next.runtime.build_sha256 = "b".repeat(64),
            _ => next.runtime.transport_generation += 1,
        }
        leases.push(next.acquire(&slot, Demand::Terminal).await);
        fixtures.push(next);
    }
    for (index, lease) in leases.iter().enumerate() {
        assert_eq!(lease.identity().unwrap().runtime, fixtures[index].runtime);
    }
    let fifth = Fixture::new(&slot);
    let denied = slot
        .acquire(fifth.runtime.clone(), Demand::Terminal, |_| async {
            panic!("overlap limit must reject before starting another worker")
        })
        .await;
    assert!(denied.err().unwrap().contains("still in use"));
    drop(leases.remove(0));
    assert_eq!(first.driver.stops.load(Ordering::SeqCst), 1);
    for lease in &leases {
        lease.environment().unwrap();
    }
    leases.push(fifth.acquire(&slot, Demand::Terminal).await);
    assert_eq!(slot.snapshot().len(), LIMIT);
    drop(leases);
    slot.drained().await;
}

#[tokio::test]
async fn invalid_preparation_never_exports_environment_and_stops_adapter() {
    for mismatch in 0..7 {
        let slot = Slot::default();
        let fixture = Fixture::new(&slot);
        let factory = fixture.clone();
        let result = slot
            .acquire(
                fixture.runtime.clone(),
                Demand::Terminal,
                move |generation| async move {
                    let mut prepared = factory.prepared(generation);
                    match mismatch {
                        0 => prepared.identity.runtime.instance = Uuid::new_v4(),
                        1 => prepared.identity.worker = Uuid::nil(),
                        2 => prepared.identity.session = Uuid::new_v4(),
                        3 => prepared.identity.display_generation += 1,
                        4 => prepared.attached.environment.wayland_socket = "relative".into(),
                        5 => factory.valid.store(false, Ordering::SeqCst),
                        _ => {
                            factory.driver.updates.send_replace(DriverState::Closed);
                        }
                    }
                    Ok(prepared)
                },
            )
            .await;
        assert!(result.is_err(), "mismatch {mismatch} accepted");
        assert_eq!(fixture.driver.stops.load(Ordering::SeqCst), 1);
        assert!(slot.snapshot().is_empty());
    }
}

#[tokio::test(start_paused = true)]
async fn timeout_and_panicked_startup_do_not_strand_waiters() {
    for panic in [false, true] {
        let slot = Slot::default();
        let fixture = Fixture::new(&slot);
        let dropped = Arc::new(AtomicUsize::new(0));
        let guard = Dropped(dropped.clone());
        let result = slot
            .acquire(
                fixture.runtime.clone(),
                Demand::Terminal,
                move |_| async move {
                    let _guard = guard;
                    assert!(!panic, "simulated adapter panic");
                    std::future::pending::<Result<Prepared, String>>().await
                },
            )
            .await;
        let error = result.err().unwrap();
        assert!(
            error.contains(if panic { "interrupted" } else { "timed out" }),
            "{error}"
        );
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        assert!(slot.snapshot().is_empty());
        fixture
            .acquire(&slot, Demand::Clipboard)
            .await
            .environment()
            .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attachment_finishing_after_owner_close_is_stopped_without_publication() {
    let slot = Arc::new(Slot::default());
    let fixture = Fixture::new(&slot);
    let (started, ready) = oneshot::channel();
    let (release, blocked) = std::sync::mpsc::sync_channel(1);
    let caller = {
        let (slot, fixture) = (slot.clone(), fixture.clone());
        tokio::spawn(async move {
            slot.acquire(
                fixture.runtime.clone(),
                Demand::Terminal,
                move |generation| async move {
                    started.send(()).unwrap();
                    // Simulate attachment completing in the same poll as close.
                    blocked.recv_timeout(Duration::from_secs(2)).unwrap();
                    Ok(fixture.prepared(generation))
                },
            )
            .await
        })
    };
    ready.await.unwrap();
    slot.close();
    release.send(()).unwrap();
    assert!(caller.await.unwrap().is_err());
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.driver.stops.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(slot.snapshot().is_empty());
    let result = slot
        .acquire(fixture.runtime.clone(), Demand::Terminal, |_| async {
            panic!("closed owner restarted")
        })
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn owner_drop_stops_driver_even_if_presentation_keeps_driver_arc_and_lease() {
    let slot = Slot::default();
    let fixture = Fixture::new(&slot);
    let lease = fixture.acquire(&slot, Demand::Terminal).await;
    drop(slot);
    assert!(lease.environment().is_err());
    assert_eq!(fixture.driver.stops.load(Ordering::SeqCst), 1);
    drop(lease);
    assert_eq!(fixture.driver.stops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn maintenance_waits_for_preparation_blocks_new_admission_then_allows_retry() {
    let slot = Arc::new(Slot::default());
    let read = slot.admission().await.unwrap();
    let (entered, ready) = oneshot::channel();
    let (release, finish) = oneshot::channel();
    let maintenance = {
        let slot = slot.clone();
        tokio::spawn(async move {
            let _maintenance = slot.maintenance().await;
            entered.send(()).unwrap();
            finish.await.unwrap();
        })
    };
    tokio::task::yield_now().await;
    assert!(!maintenance.is_finished());
    drop(read);
    ready.await.unwrap();
    let pending = {
        let slot = slot.clone();
        tokio::spawn(async move {
            drop(slot.admission().await.unwrap());
        })
    };
    tokio::task::yield_now().await;
    assert!(!pending.is_finished());
    release.send(()).unwrap();
    maintenance.await.unwrap();
    pending.await.unwrap();
    slot.close();
    assert!(slot.admission().await.is_err());
}

#[tokio::test]
async fn registry_keeps_profiles_and_saved_entries_separate_and_shutdown_stops_only_its_sessions() {
    use crate::server_session::Registry;
    let first_directory = tempfile::tempdir().unwrap();
    let second_directory = tempfile::tempdir().unwrap();
    let first = Registry::new(first_directory.path());
    let second = Registry::new(second_directory.path());
    let server: crate::model::Server = serde_json::from_value(serde_json::json!({
        "id": Uuid::new_v4(), "name": "GUI ownership fixture", "sshUser": "fixture",
        "sshHost": "127.0.0.1", "sshPort": 9
    }))
    .unwrap();
    let mut duplicate = server.clone();
    duplicate.id = Uuid::new_v4();
    let a = first.session(&server, 0);
    let b = first.session(&duplicate, 0);
    let c = second.session(&server, 0);
    for owner in [&a, &b, &c] {
        assert!(!owner.has_transport());
        assert!(owner.graphical_sessions().is_empty());
    }
    let mut fixtures = Vec::new();
    let mut leases = Vec::new();
    for owner in [&a, &b, &c] {
        let mut fixture = Fixture::new(&owner.graphical);
        fixture.session = owner.integration_session_id();
        leases.push(fixture.acquire(&owner.graphical, Demand::Clipboard).await);
        fixtures.push(fixture);
    }
    let identities: Vec<_> = leases
        .iter()
        .map(|lease| lease.identity().unwrap().session)
        .collect();
    assert_ne!(identities[0], identities[1]);
    assert_ne!(identities[0], identities[2]);
    first.shutdown().await;
    assert!(leases[0].environment().is_err());
    assert!(leases[1].environment().is_err());
    assert!(leases[2].environment().is_ok());
    assert_eq!(fixtures[0].driver.stops.load(Ordering::SeqCst), 1);
    assert_eq!(fixtures[1].driver.stops.load(Ordering::SeqCst), 1);
    assert_eq!(fixtures[2].driver.stops.load(Ordering::SeqCst), 0);
    second.shutdown().await;
    assert!(leases[2].environment().is_err());
    assert_eq!(fixtures[2].driver.stops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn application_control_requires_every_part_of_the_original_display_identity() {
    let slot = Slot::default();
    let fixture = Fixture::new(&slot);
    let lease = fixture.acquire(&slot, Demand::Launch).await;
    let started = lease.launch_application(command()).unwrap();
    let app = started.application;
    assert_eq!(
        slot.application_state(&app).unwrap(),
        launch::State::Pending
    );
    for part in 0..6 {
        let mut invalid = app.clone();
        match part {
            0 => invalid.display.runtime.instance = Uuid::new_v4(),
            1 => invalid.display.runtime.build_sha256 = "b".repeat(64),
            2 => invalid.display.runtime.transport_generation += 1,
            3 => invalid.display.session = Uuid::new_v4(),
            4 => invalid.display.worker = Uuid::new_v4(),
            _ => invalid.display.display_generation += 1,
        }
        assert!(slot.application_state(&invalid).is_err());
        assert!(slot.stop_application(&invalid).is_err());
    }
    assert!(fixture.driver.app_stops.lock().unwrap().is_empty());
    drop(lease);
    let snapshot = &slot.snapshot()[0];
    assert_eq!(
        (snapshot.launches, snapshot.terminals, snapshot.clipboard),
        (0, 0, 0)
    );
    assert_eq!(snapshot.applications, 1);
    assert!(slot.stop_application(&app).unwrap());
    assert_eq!(
        slot.application_state(&app).unwrap(),
        launch::State::Stopping
    );
    assert_eq!(fixture.driver.stops.load(Ordering::SeqCst), 0);
    slot.close();
    assert!(slot.stop_application(&app).is_err());
    assert!(matches!(
        started.pending.outcome().await,
        launch::Outcome::Unknown(_)
    ));
}

#[tokio::test]
async fn serialized_application_handle_cannot_control_a_replacement_display() {
    let slot = Slot::default();
    let fixture = Fixture::new(&slot);
    let lease = fixture.acquire(&slot, Demand::Launch).await;
    let started = lease.launch_application(command()).unwrap();
    let app: launch::Application =
        serde_json::from_value(serde_json::to_value(&started.application).unwrap()).unwrap();
    drop(lease);
    drop(started);
    fixture.driver.publish(0, 0);
    slot.drained().await;
    let replacement = Fixture::new(&slot);
    let lease = replacement.acquire(&slot, Demand::Launch).await;
    let next = lease.launch_application(command()).unwrap();
    assert_eq!(next.application.request, app.request); // Same local ID, different display.
    assert_eq!(slot.application_state(&app).unwrap(), launch::State::Ended);
    assert!(!slot.stop_application(&app).unwrap());
    assert!(replacement.driver.app_stops.lock().unwrap().is_empty());
    assert!(slot.stop_application(&next.application).unwrap());
    replacement.valid.store(false, Ordering::SeqCst);
    assert!(slot.application_state(&next.application).is_err());
    assert!(slot.stop_application(&next.application).is_err());
    slot.close();
}

#[tokio::test]
async fn endings_outlive_the_display_that_reported_them() {
    let slot = Slot::default();
    let fixture = Fixture::new(&slot);
    let lease = fixture.acquire(&slot, Demand::Launch).await;
    let failed = lease.launch_application(command()).unwrap().application;
    let quiet = lease.launch_application(command()).unwrap().application;
    let boom = launch::Ending::Failed("boom".into());
    fixture
        .driver
        .app_endings
        .lock()
        .unwrap()
        .insert(failed.request, boom.clone());
    assert_eq!(
        slot.application_ending(&failed).unwrap(),
        Some(boom.clone())
    );
    assert_eq!(slot.application_ending(&quiet).unwrap(), None);
    drop(lease);
    fixture.driver.publish(0, 0);
    slot.drained().await;
    assert_eq!(fixture.driver.stops.load(Ordering::SeqCst), 1);
    assert_eq!(
        slot.application_state(&failed).unwrap(),
        launch::State::Ended
    );
    assert_eq!(slot.application_ending(&failed).unwrap(), Some(boom));
    assert_eq!(slot.application_ending(&quiet).unwrap(), None);
    let mut other = failed.clone();
    other.display.worker = Uuid::new_v4();
    assert!(slot.application_ending(&other).is_err());
    slot.close();
    assert!(slot.application_ending(&failed).is_err());
}

#[tokio::test]
async fn notices_are_read_from_the_live_display_only() {
    let slot = Slot::default();
    let fixture = Fixture::new(&slot);
    let lease = fixture.acquire(&slot, Demand::Launch).await;
    let snap = lease.launch_application(command()).unwrap().application;
    let native = lease.launch_application(command()).unwrap().application;
    let notice = launch::Notice::DialogsElsewhere(launch::Elsewhere::OtherDisplay);
    fixture
        .driver
        .app_notices
        .lock()
        .unwrap()
        .insert(snap.request, notice);
    assert_eq!(slot.application_notice(&snap).unwrap(), Some(notice));
    assert_eq!(slot.application_notice(&native).unwrap(), None);
    let mut other = snap.clone();
    other.display.worker = Uuid::new_v4();
    assert!(slot.application_notice(&other).is_err());
    drop(lease);
    fixture.driver.publish(0, 0);
    slot.drained().await;
    assert_eq!(slot.application_notice(&snap).unwrap(), None);
    slot.close();
}

#[tokio::test]
async fn normal_completion_is_bounded_metadata_and_never_inferred_from_failure() {
    let slot = Slot::default();
    let mut finished = Vec::new();
    for _ in 0..=FINISHED_LIMIT {
        let fixture = Fixture::new(&slot);
        let lease = fixture.acquire(&slot, Demand::Launch).await;
        let started = lease.launch_application(command()).unwrap();
        finished.push(started.application.clone());
        drop(started);
        // This exercises the release path; the replacement-display test uses
        // the watcher path. Neither can retain a driver for observation.
        fixture.driver.publish(0, 0);
        drop(lease);
        slot.drained().await;
        assert_eq!(fixture.driver.stops.load(Ordering::SeqCst), 1);
    }
    assert_eq!(
        slot.shared.state.lock().unwrap().finished.len(),
        FINISHED_LIMIT
    );
    assert!(slot.application_state(&finished[0]).is_err());
    assert!(slot.stop_application(&finished[0]).is_err());
    for application in &finished[1..] {
        assert_eq!(
            slot.application_state(application).unwrap(),
            launch::State::Ended
        );
        assert!(!slot.stop_application(application).unwrap());
        let mut unknown = application.clone();
        unknown.request = 2.try_into().unwrap();
        assert!(slot.application_state(&unknown).is_err());
        assert!(slot.stop_application(&unknown).is_err());
    }
    let fixture = Fixture::new(&slot);
    let lease = fixture.acquire(&slot, Demand::Launch).await;
    let started = lease.launch_application(command()).unwrap();
    fixture.driver.updates.send_replace(DriverState::Closed);
    drop(lease);
    slot.drained().await;
    assert!(slot.application_state(&started.application).is_err());
    assert!(slot.stop_application(&started.application).is_err());
    slot.close();
    assert!(slot.application_state(finished.last().unwrap()).is_err());
}
