//! Presentation of backend tunnel health.
use newport_protocol::desktop::configuration::{
    ConnectionState, ConnectionStatus, DestinationHealth, DestinationStatus,
};

pub fn status(state: Option<&ConnectionState>, health: &[DestinationHealth]) -> String {
    status_for(state, health, None)
}

/// A discovered row represents one destination; a saved range represents all.
/// Transport failures apply to the whole forward regardless of destination.
pub fn status_for(
    state: Option<&ConnectionState>,
    health: &[DestinationHealth],
    remote_port: Option<u16>,
) -> String {
    let status = state.map_or(ConnectionStatus::Disconnected, |state| state.status);
    let word = match status {
        ConnectionStatus::Disconnected => "disconnected",
        ConnectionStatus::Connecting => "connecting",
        ConnectionStatus::Connected => "connected",
        ConnectionStatus::Reconnecting => "reconnecting",
        ConnectionStatus::Error => "error",
    };
    if let Some(error) = state
        .and_then(|state| state.error_message.as_deref())
        .filter(|s| !s.is_empty())
    {
        return format!("{word} · {error}");
    }
    if status == ConnectionStatus::Connected {
        let health: Vec<_> = health
            .iter()
            .filter(|h| remote_port.is_none_or(|port| h.remote_port == port))
            .collect();
        if let Some(issue) = health.iter().find(|h| {
            matches!(
                h.status,
                DestinationStatus::Unavailable | DestinationStatus::Blocked
            )
        }) {
            // The row's status already says the destination is down: say why.
            return issue
                .message
                .as_deref()
                .filter(|message| !message.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| match issue.status {
                    DestinationStatus::Blocked => "Destination blocked".into(),
                    _ => "Destination unavailable".into(),
                });
        }
        if !health.is_empty()
            && health
                .iter()
                .all(|h| h.status == DestinationStatus::Reachable)
        {
            return "Connected · destination reachable".into();
        }
        return "Connected · destination not confirmed".into();
    }
    word.into()
}

#[cfg(test)]
mod tests {
    use super::{status, status_for};
    use newport_protocol::desktop::configuration::{
        ConnectionState, ConnectionStatus, DestinationHealth, DestinationStatus,
    };
    fn state(status: ConnectionStatus, error: Option<&str>) -> ConnectionState {
        ConnectionState {
            status,
            error_message: error.map(Into::into),
            reconnect_attempt: 0,
        }
    }
    fn health(
        remote_port: u16,
        status: DestinationStatus,
        message: Option<&str>,
    ) -> DestinationHealth {
        DestinationHealth {
            local_port: remote_port,
            remote_port,
            status,
            message: message.map(Into::into),
            checked_at: None,
        }
    }
    #[test]
    fn connected_transport_does_not_claim_destination_is_up() {
        let connected = state(ConnectionStatus::Connected, None);
        let mut checks = vec![health(
            8080,
            DestinationStatus::Unavailable,
            Some("Connection refused"),
        )];
        assert_eq!(status(Some(&connected), &checks), "Connection refused");
        checks[0].message = None;
        assert_eq!(status(Some(&connected), &checks), "Destination unavailable");
        assert!(status(Some(&connected), &[]).contains("not confirmed"));
    }

    #[test]
    fn status_scopes_range_health_to_the_displayed_destination() {
        let connected = state(ConnectionStatus::Connected, None);
        let checks = [
            health(
                8079,
                DestinationStatus::Unavailable,
                Some("Connection refused"),
            ),
            health(8080, DestinationStatus::Reachable, None),
            health(8081, DestinationStatus::Checking, None),
        ];
        assert_eq!(
            status_for(Some(&connected), &checks, Some(8080)),
            "Connected · destination reachable"
        );
        assert_eq!(
            status_for(Some(&connected), &checks, Some(8079)),
            "Connection refused"
        );
        assert!(status_for(Some(&connected), &checks, Some(8081)).contains("not confirmed"));
        assert!(status_for(Some(&connected), &checks, Some(8082)).contains("not confirmed"));
        assert_eq!(status(Some(&connected), &checks), "Connection refused");
    }

    #[test]
    fn transport_failures_are_not_scoped_and_disconnected_health_is_stale() {
        let checks = [health(8080, DestinationStatus::Reachable, None)];
        let lost = state(ConnectionStatus::Reconnecting, Some("SSH connection lost"));
        assert_eq!(
            status_for(Some(&lost), &checks, Some(8080)),
            "reconnecting · SSH connection lost"
        );
        let failed = state(ConnectionStatus::Error, None);
        assert_eq!(status_for(Some(&failed), &checks, Some(8080)), "error");
        let stopped = state(ConnectionStatus::Disconnected, None);
        assert_eq!(
            status_for(Some(&stopped), &checks, Some(8080)),
            "disconnected"
        );
        assert_eq!(status_for(None, &checks, Some(8080)), "disconnected");
    }
}
