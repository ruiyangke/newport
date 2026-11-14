//! Application-global read model. Every page observes this store; asynchronous
//! feature replies and terminal events never write connectivity into it.
use gpui_kit::{App, BorrowAppContext, Context, Global, Subscription};
pub use newport_domain::connectivity::{Phase, ServerState, Snapshot};
use newport_protocol::desktop::configuration;
use std::{
    collections::{BTreeMap, HashSet},
    time::Duration,
};
use uuid::Uuid;

#[derive(Default)]
pub struct Connectivity {
    snapshot: Snapshot,
    retired_instances: HashSet<Uuid>,
}
impl Global for Connectivity {}

pub fn install(cx: &mut App) {
    if !cx.has_global::<Connectivity>() {
        cx.set_global(Connectivity::default());
    }
}

/// Called only by the application host's backend subscription. Ordinary page
/// snapshots may be older and must never roll this store backward.
pub fn publish(snapshot: Snapshot, cx: &mut App) {
    install(cx);
    let current = cx.global::<Connectivity>();
    if current.retired_instances.contains(&snapshot.instance_id)
        || (current.snapshot.instance_id == snapshot.instance_id
            && current.snapshot.revision >= snapshot.revision)
    {
        return;
    }
    cx.update_global::<Connectivity, _>(|current, _| {
        if current.snapshot.instance_id != snapshot.instance_id
            && !current.snapshot.instance_id.is_nil()
        {
            current
                .retired_instances
                .insert(current.snapshot.instance_id);
        }
        current.snapshot = snapshot;
    });
}

/// Each connected server's latest SSH round trip. Kept apart from
/// [`Connectivity`], so a new reading is never a connection change.
#[derive(Default)]
pub struct Latencies(BTreeMap<Uuid, Latency>);
impl Global for Latencies {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Latency {
    pub round_trip: Duration,
    pub speed: Speed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Speed {
    #[default]
    Normal,
    Slow,
    VerySlow,
}
impl Speed {
    /// The speed a reading shows after `self`. A slower speed lasts until a
    /// reading is well under its threshold, so a connection near one doesn't
    /// flicker between colors.
    pub fn next(self, round_trip: Duration) -> Self {
        let ms = round_trip.as_millis();
        let past =
            |speed: Self, enter: u128, leave: u128| ms >= if self >= speed { leave } else { enter };
        if past(Self::VerySlow, 400, 320) {
            Self::VerySlow
        } else if past(Self::Slow, 150, 120) {
            Self::Slow
        } else {
            Self::Normal
        }
    }
}

/// Called only by the application host's backend subscription.
pub fn publish_latencies(round_trips: BTreeMap<Uuid, Duration>, cx: &mut App) {
    let previous = cx
        .try_global::<Latencies>()
        .map(|latencies| latencies.0.clone())
        .unwrap_or_default();
    let latencies = round_trips
        .into_iter()
        .map(|(id, round_trip)| {
            let shown = previous.get(&id).map(|l| l.speed).unwrap_or_default();
            let speed = shown.next(round_trip);
            (id, Latency { round_trip, speed })
        })
        .collect();
    cx.set_global(Latencies(latencies));
}

pub fn latency(id: &str, cx: &App) -> Option<Latency> {
    cx.try_global::<Latencies>()?
        .0
        .get(&Uuid::parse_str(id).ok()?)
        .copied()
}

/// How the title bar shows a round trip.
pub fn latency_label(latency: Duration) -> String {
    match latency.as_millis() {
        0 => "<1 ms".into(),
        ms @ 1..1000 => format!("{ms} ms"),
        _ => format!("{:.1} s", latency.as_secs_f64()),
    }
}

/// Notifies an entity when the latency of the server it displays changes.
pub fn observe_latency<T: 'static>(
    cx: &mut Context<T>,
    server_id: impl Fn(&T) -> &str + 'static,
) -> Subscription {
    if !cx.has_global::<Latencies>() {
        cx.set_global(Latencies::default());
    }
    let mut previous = None;
    cx.observe_global::<Latencies>(move |this, cx| {
        let next = latency(server_id(this), cx);
        if next != previous {
            previous = next;
            cx.notify();
        }
    })
}

pub fn server<'a>(id: &str, cx: &'a App) -> Option<&'a ServerState> {
    cx.try_global::<Connectivity>()?
        .snapshot
        .servers
        .get(&Uuid::parse_str(id).ok()?)
}

/// A server's connectivity, only while the store describes the same backend
/// instance and connection revision as `snapshot`.
pub fn scoped<'a>(
    id: &str,
    snapshot: Option<&configuration::Snapshot>,
    cx: &'a App,
) -> Option<&'a ServerState> {
    let store = cx.try_global::<Connectivity>()?;
    let snapshot = snapshot?;
    if snapshot.instance_id != store.snapshot.instance_id {
        return None;
    }
    let id = Uuid::parse_str(id).ok()?;
    store
        .snapshot
        .servers
        .get(&id)
        .filter(|state| state.connection_revision == snapshot.runtime.connection_revision(id))
}

/// A server's connectivity, only while the store describes the backend
/// instance and connection revision `scope` was captured at.
pub fn for_scope<'a>(
    scope: Option<&configuration::ConnectionScope>,
    cx: &'a App,
) -> Option<&'a ServerState> {
    let scope = scope?;
    let store = cx.try_global::<Connectivity>()?;
    if scope.backend_instance != store.snapshot.instance_id {
        return None;
    }
    store
        .snapshot
        .servers
        .get(&scope.settings.server_id)
        .filter(|state| state.connection_revision == scope.connection_revision)
}

/// Subscribe once for an entity, scoped to the server it currently displays.
/// Returning the subscription lets retained panels own their listener lifetime.
pub fn observe<T: 'static>(
    cx: &mut Context<T>,
    server_id: impl Fn(&T) -> &str + 'static,
    changed: impl Fn(&mut T, &mut Context<T>) + 'static,
) -> Subscription {
    install(cx);
    let mut previous = cx.global::<Connectivity>().snapshot.clone();
    cx.observe_global::<Connectivity>(move |this, cx| {
        let next = cx.global::<Connectivity>().snapshot.clone();
        let id = Uuid::parse_str(server_id(this)).ok();
        let relevant = previous.instance_id != next.instance_id
            || id.is_some_and(|id| previous.servers.get(&id) != next.servers.get(&id));
        previous = next;
        if relevant {
            changed(this, cx);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_reads_in_whole_milliseconds() {
        assert_eq!(latency_label(Duration::from_micros(400)), "<1 ms");
        assert_eq!(latency_label(Duration::from_micros(23_600)), "23 ms");
        assert_eq!(latency_label(Duration::from_millis(999)), "999 ms");
        assert_eq!(latency_label(Duration::from_millis(1_250)), "1.2 s");
    }

    #[test]
    fn a_slow_color_lasts_until_readings_are_well_under_its_threshold() {
        let ms = Duration::from_millis;
        let mut speed = Speed::Normal;
        let mut shown = Vec::new();
        for reading in [149, 150, 130, 119, 400, 330, 319, 100] {
            speed = speed.next(ms(reading));
            shown.push(speed);
        }
        use Speed::*;
        assert_eq!(
            shown,
            [Normal, Slow, Slow, Normal, VerySlow, VerySlow, Slow, Normal]
        );
        assert_eq!(Normal.next(ms(500)), VerySlow);
        assert_eq!(Normal.next(ms(350)), Slow);
    }
    use gpui_kit as gpui;
    use gpui_kit::AppContext;

    struct Listener {
        server: String,
        notifications: usize,
    }
    #[gpui::test]
    fn every_subscriber_observes_one_state_and_old_replies_cannot_rewind_it(
        cx: &mut gpui::TestAppContext,
    ) {
        let id = Uuid::new_v4();
        let other = Uuid::new_v4();
        let instance = Uuid::new_v4();
        let mut listeners = Vec::new();
        cx.update(|cx| {
            for server in [id, id, other] {
                listeners.push(cx.new(|cx| {
                    observe(
                        cx,
                        |this: &Listener| &this.server,
                        |this, cx| {
                            this.notifications += 1;
                            cx.notify();
                        },
                    )
                    .detach();
                    Listener {
                        server: server.to_string(),
                        notifications: 0,
                    }
                }));
            }
        });
        cx.run_until_parked();
        cx.update(|cx| {
            publish(
                Snapshot {
                    instance_id: instance,
                    revision: 1,
                    servers: [(id, ServerState::default())].into(),
                },
                cx,
            )
        });
        cx.run_until_parked();
        let mut connected = Snapshot {
            instance_id: instance,
            revision: 2,
            servers: [(
                id,
                ServerState {
                    phase: Phase::Connected,
                    ..Default::default()
                },
            )]
            .into(),
        };
        cx.update(|cx| publish(connected.clone(), cx));
        cx.run_until_parked();
        assert_eq!(listeners[0].read_with(cx, |l, _| l.notifications), 2);
        assert_eq!(listeners[1].read_with(cx, |l, _| l.notifications), 2);
        assert_eq!(listeners[2].read_with(cx, |l, _| l.notifications), 1);
        connected.revision = 1;
        connected.servers.get_mut(&id).unwrap().phase = Phase::Failed;
        cx.update(|cx| publish(connected.clone(), cx));
        assert_eq!(
            cx.read(|cx| server(&id.to_string(), cx).unwrap().phase),
            Phase::Connected
        );
        let replacement = Uuid::new_v4();
        cx.update(|cx| {
            publish(
                Snapshot {
                    instance_id: replacement,
                    ..Default::default()
                },
                cx,
            )
        });
        connected.revision = 99;
        cx.update(|cx| publish(connected, cx));
        assert!(cx.read(|cx| server(&id.to_string(), cx).is_none()));
    }

    #[gpui::test]
    fn a_scope_finds_its_server_from_the_identity_the_app_makes(cx: &mut gpui::TestAppContext) {
        let id = Uuid::new_v4();
        let instance = Uuid::new_v4();
        cx.update(|cx| {
            install(cx);
            publish(
                Snapshot {
                    instance_id: instance,
                    revision: 1,
                    servers: [(
                        id,
                        ServerState {
                            connection_revision: 3,
                            phase: Phase::Connected,
                            ..Default::default()
                        },
                    )]
                    .into(),
                },
                cx,
            )
        });
        use crate::snapshot_fixture as fixture;
        // Reached through a jump host, as every connection field counts.
        let server = configuration::Server {
            proxy: Some(configuration::Proxy::Jump(Box::new(Uuid::new_v4()))),
            ..fixture::server(id, "Server")
        };
        let snapshot = |revision: u64| {
            let mut snapshot = fixture::snapshot(vec![server.clone()]);
            snapshot.instance_id = instance;
            fixture::set(&mut snapshot.runtime.connection_revisions, id, revision);
            snapshot
        };
        let current = snapshot(3);
        let scope = Some(current.connection_scope(&server));
        assert_eq!(
            cx.read(|cx| for_scope(scope.as_ref(), cx).map(|state| state.phase)),
            Some(Phase::Connected)
        );
        assert_eq!(
            cx.read(|cx| scoped(&id.to_string(), Some(&current), cx).map(|state| state.phase)),
            Some(Phase::Connected)
        );
        let stale = snapshot(2);
        let stale_scope = Some(stale.connection_scope(&server));
        assert!(cx.read(|cx| for_scope(stale_scope.as_ref(), cx).is_none()));
        assert!(cx.read(|cx| scoped(&id.to_string(), Some(&stale), cx).is_none()));
    }
}
