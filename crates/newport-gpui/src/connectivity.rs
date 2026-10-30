//! Application-global read model. Every page observes this store; asynchronous
//! feature replies and terminal events never write connectivity into it.
use gpui_kit::{App, BorrowAppContext, Context, Global, Subscription};
pub use newport_domain::connectivity::{Phase, ServerState, Snapshot};
use std::collections::HashSet;
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

pub fn server<'a>(id: &str, cx: &'a App) -> Option<&'a ServerState> {
    cx.try_global::<Connectivity>()?
        .snapshot
        .servers
        .get(&Uuid::parse_str(id).ok()?)
}

pub fn scoped<'a>(id: &str, snapshot: &serde_json::Value, cx: &'a App) -> Option<&'a ServerState> {
    let store = cx.try_global::<Connectivity>()?;
    if snapshot["instanceId"].as_str()? != store.snapshot.instance_id.to_string() {
        return None;
    }
    server(id, cx).filter(|state| {
        state.connection_revision
            == snapshot["runtime"]["connectionRevisions"][id]
                .as_u64()
                .unwrap_or(0)
    })
}

pub fn for_scope<'a>(scope: &serde_json::Value, cx: &'a App) -> Option<&'a ServerState> {
    use newport_domain::connection_scope::{instance, revision};
    let store = cx.try_global::<Connectivity>()?;
    if instance(scope)? != store.snapshot.instance_id.to_string() {
        return None;
    }
    server(scope[0].as_str()?, cx)
        .filter(|state| state.connection_revision == revision(scope).unwrap_or(0))
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
        // Reached through a jump host, as every connection field counts.
        let server = serde_json::json!({"id":id,"sshHost":"h","sshUser":"u","sshPort":22,"proxy":{"jump":"j"}});
        let snapshot = |revision: u64| serde_json::json!({"instanceId":instance,"runtime":{"connectionRevisions":{id.to_string():revision}}});
        let scope = newport_domain::connection_scope::connection(&server, &snapshot(3));
        assert_eq!(
            cx.read(|cx| for_scope(&scope, cx).map(|state| state.phase)),
            Some(Phase::Connected)
        );
        let stale = newport_domain::connection_scope::connection(&server, &snapshot(2));
        assert!(cx.read(|cx| for_scope(&stale, cx).is_none()));
    }
}
