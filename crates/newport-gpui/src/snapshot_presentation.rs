//! Window invalidation is narrower than snapshot acceptance. Every snapshot is
//! retained and reconciled; only state consumed by this window requests a redraw.
use newport_protocol::desktop::configuration::{Runtime, Snapshot};
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Surface {
    General,
    Setup,
    Integration,
}

pub(super) fn changed(
    previous: Option<&Snapshot>,
    next: &Snapshot,
    server: Option<Uuid>,
    surface: Surface,
) -> bool {
    let Some(previous) = previous else {
        return true;
    };
    // Revision advances for any server's runtime observation. Configuration,
    // backend identity and load errors still matter.
    let Snapshot {
        instance_id,
        revision: _,
        config,
        runtime,
        load_error,
    } = next;
    if *instance_id != previous.instance_id
        || *config != previous.config
        || *load_error != previous.load_error
    {
        return true;
    }
    // Destructured whole, so a new runtime field must choose whether it is
    // shared or belongs to one server.
    let Runtime {
        agent_transport: _,
        agent_runtime: _,
        agent_permissions: _,
        agent_status: _,
        agent_installation: _,
        tunnels,
        tunnel_health,
        clipboard: _,
        clipboard_messages: _,
        connection_revisions: _,
        awaiting_agent: _,
    } = runtime;
    let before = &previous.runtime;
    // Tunnel state remains shared: the sidebar displays the global connected
    // count, and Connections also consumes tunnel health and action state.
    if *tunnels != before.tunnels || *tunnel_health != before.tunnel_health {
        return true;
    }
    let Some(server) = server else {
        return false;
    };
    if before
        .agent_transport(server)
        .map(|transport| transport.status)
        != runtime
            .agent_transport(server)
            .map(|transport| transport.status)
    {
        return true;
    }
    // Needle found missing (or back) decides whether setup takes the window.
    if before.agent_status(server).map(|status| status.installed)
        != runtime.agent_status(server).map(|status| status.installed)
    {
        return true;
    }
    // Connection epochs invalidate the current page's owner. The header
    // subscribes to the global connectivity store. The wizard shows progress.
    // Remaining agent state belongs to the Integration page. Accepting it
    // still updates permission checks and retained state on every page.
    if before.connection_revision(server) != runtime.connection_revision(server)
        || matches!(surface, Surface::Setup | Surface::Integration)
            && before.agent_installation(server) != runtime.agent_installation(server)
    {
        return true;
    }
    if surface == Surface::Integration
        && (before.agent_transport(server) != runtime.agent_transport(server)
            || before.agent_runtime(server) != runtime.agent_runtime(server)
            || before.agent_permissions(server) != runtime.agent_permissions(server)
            || before.agent_status(server) != runtime.agent_status(server)
            || before.clipboard(server) != runtime.clipboard(server)
            || before.clipboard_message(server) != runtime.clipboard_message(server))
    {
        return true;
    }
    before.awaiting_agent(server) != runtime.awaiting_agent(server)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot_fixture::{self as fixture, NEXT_INSTANCE, SECOND, SERVER};
    use newport_protocol::desktop::configuration::{
        ConnectionStatus, InstallStage, RuntimeStatus, TransportStatus,
    };

    /// A named change to a snapshot.
    type Change = (&'static str, fn(&mut Snapshot));

    fn before() -> Snapshot {
        fixture::snapshot(vec![
            fixture::server(SERVER, "Selected"),
            fixture::server(SECOND, "Other"),
        ])
    }

    #[test]
    fn transport_status_changes_remain_visible_without_repainting_for_waiting_requests() {
        let mut before = before();
        fixture::set(
            &mut before.runtime.agent_transport,
            SERVER,
            fixture::transport(TransportStatus::Connecting),
        );
        let mut next = before.clone();
        let mut waiting = fixture::transport(TransportStatus::Connecting);
        waiting.waiting = 2;
        fixture::set(&mut next.runtime.agent_transport, SERVER, waiting);
        assert!(!changed(
            Some(&before),
            &next,
            Some(SERVER),
            Surface::General
        ));
        fixture::set(
            &mut next.runtime.agent_transport,
            SECOND,
            fixture::transport(TransportStatus::Connected),
        );
        assert!(!changed(
            Some(&before),
            &next,
            Some(SERVER),
            Surface::General
        ));
        fixture::set(
            &mut next.runtime.agent_transport,
            SERVER,
            fixture::transport(TransportStatus::Connected),
        );
        assert!(changed(
            Some(&before),
            &next,
            Some(SERVER),
            Surface::General
        ));
    }

    #[test]
    fn needle_presence_repaints_every_page_but_other_status_does_not() {
        let mut before = before();
        fixture::set(&mut before.runtime.agent_status, SERVER, {
            let mut status = fixture::installed(true);
            status.checked_at = 1;
            status
        });
        let mut checked = before.clone();
        fixture::set(&mut checked.runtime.agent_status, SERVER, {
            let mut status = fixture::installed(true);
            status.checked_at = 2;
            status
        });
        assert!(!changed(
            Some(&before),
            &checked,
            Some(SERVER),
            Surface::General
        ));
        let mut missing = before.clone();
        fixture::set(
            &mut missing.runtime.agent_status,
            SERVER,
            fixture::installed(false),
        );
        assert!(changed(
            Some(&before),
            &missing,
            Some(SERVER),
            Surface::General
        ));
        assert!(!changed(
            Some(&before),
            &missing,
            Some(SECOND),
            Surface::General
        ));
    }

    #[test]
    fn each_known_runtime_owner_is_selected_independently() {
        let before = before();
        let changes: [Change; 8] = [
            ("agent transport", |next| {
                fixture::set(
                    &mut next.runtime.agent_transport,
                    SECOND,
                    fixture::transport(TransportStatus::Connected),
                )
            }),
            ("agent runtime", |next| {
                fixture::set(
                    &mut next.runtime.agent_runtime,
                    SECOND,
                    fixture::agent_runtime(RuntimeStatus::Starting, None),
                )
            }),
            ("agent permissions", |next| {
                fixture::set(
                    &mut next.runtime.agent_permissions,
                    SECOND,
                    fixture::permissions(true, true),
                )
            }),
            ("agent status", |next| {
                fixture::set(
                    &mut next.runtime.agent_status,
                    SECOND,
                    fixture::installed(true),
                )
            }),
            ("agent installation", |next| {
                fixture::set(
                    &mut next.runtime.agent_installation,
                    SECOND,
                    fixture::install(InstallStage::Uploading, 1, 2),
                )
            }),
            ("clipboard", |next| {
                fixture::set(
                    &mut next.runtime.clipboard,
                    SECOND,
                    fixture::connection(ConnectionStatus::Connected),
                )
            }),
            ("clipboard messages", |next| {
                fixture::set(
                    &mut next.runtime.clipboard_messages,
                    SECOND,
                    "Copied".into(),
                )
            }),
            ("connection revisions", |next| {
                fixture::set(&mut next.runtime.connection_revisions, SECOND, 2)
            }),
        ];
        for (name, change) in changes {
            let mut next = before.clone();
            next.revision = 2;
            change(&mut next);
            assert!(
                !changed(Some(&before), &next, Some(SERVER), Surface::Integration),
                "{name}"
            );
            assert!(
                changed(Some(&before), &next, Some(SECOND), Surface::Integration),
                "{name}"
            );
            assert!(
                changed(Some(&next), &before, Some(SECOND), Surface::Integration),
                "removing {name}"
            );
        }
    }

    #[test]
    fn shared_state_remains_observable() {
        let before = before();
        let changes: [Change; 5] = [
            ("backend instance", |next| next.instance_id = NEXT_INSTANCE),
            ("load error", |next| {
                next.load_error = Some("cannot read profile".into())
            }),
            ("configuration", |next| next.config.servers.clear()),
            ("forwards", |next| {
                fixture::set(
                    &mut next.runtime.tunnels,
                    SECOND,
                    fixture::connection(ConnectionStatus::Connected),
                )
            }),
            ("forward health", |next| {
                fixture::set(
                    &mut next.runtime.tunnel_health,
                    SECOND,
                    newport_protocol::desktop::configuration::DestinationHealthList {
                        values: Vec::new(),
                    },
                )
            }),
        ];
        for (name, change) in changes {
            let mut next = before.clone();
            change(&mut next);
            assert!(
                changed(Some(&before), &next, Some(SERVER), Surface::Integration),
                "{name}"
            );
        }
        assert!(changed(None, &before, Some(SERVER), Surface::General));
    }

    #[test]
    fn approval_wait_is_scoped_and_revision_alone_is_not_presentation() {
        let mut before = before();
        before.runtime.awaiting_agent = vec![SECOND];
        let mut next = before.clone();
        next.revision = 2;
        next.runtime.awaiting_agent.clear();
        assert!(!changed(
            Some(&before),
            &next,
            Some(SERVER),
            Surface::Integration
        ));
        assert!(changed(
            Some(&before),
            &next,
            Some(SECOND),
            Surface::Integration
        ));
    }
}
