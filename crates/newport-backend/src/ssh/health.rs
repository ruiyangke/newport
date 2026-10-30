//! Destination checks: a channel to each destination, opened and closed; no remote shell or payload.
use crate::model::{DestinationHealth, DestinationStatus};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub(super) type Health = Arc<Mutex<Vec<DestinationHealth>>>;
const UNAVAILABLE: &str = "The server couldn’t connect to it.";
pub(super) const BLOCKED: &str = "The SSH server doesn’t allow forwarding here.";
const TIMED_OUT: &str = "The check timed out.";
const INCOMPLETE: &str = "The check didn’t finish.";

/// What a failed destination check over a link says about the destination.
/// `None` is a check that ran out of time.
pub(super) fn destination(error: Option<&newport_ssh::Error>) -> (DestinationStatus, String) {
    use russh::ChannelOpenFailure;
    match error {
        Some(newport_ssh::Error::ChannelRefused {
            reason: ChannelOpenFailure::ConnectFailed,
        }) => (DestinationStatus::Unavailable, UNAVAILABLE.into()),
        Some(newport_ssh::Error::ChannelRefused {
            reason: ChannelOpenFailure::AdministrativelyProhibited,
        }) => (DestinationStatus::Blocked, BLOCKED.into()),
        None => (DestinationStatus::Unknown, TIMED_OUT.into()),
        Some(_) => (DestinationStatus::Unknown, INCOMPLETE.into()),
    }
}
/// A remote forward's destination is a port on this Mac's loopback.
pub(super) async fn monitor_local(local: u16, remote: u16, health: Health) {
    loop {
        let connect = tokio::net::TcpStream::connect(("127.0.0.1", local));
        let (status, message) = match tokio::time::timeout(Duration::from_secs(1), connect).await {
            Ok(Ok(_)) => (DestinationStatus::Reachable, None),
            _ => (
                DestinationStatus::Unavailable,
                Some(format!(
                    "Nothing is listening on this computer at port {local}."
                )),
            ),
        };
        record(&health, remote, status, message);
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}
pub(super) fn record(
    health: &Health,
    remote: u16,
    status: DestinationStatus,
    message: Option<String>,
) {
    if let Some(entry) = health
        .lock()
        .unwrap()
        .iter_mut()
        .find(|h| h.remote_port == remote)
    {
        entry.status = status;
        entry.message = message;
        entry.checked_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs());
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn link_refusals_tell_a_stopped_destination_from_a_forbidden_one() {
        let refused = |reason| newport_ssh::Error::ChannelRefused { reason };
        fn status(error: Option<&newport_ssh::Error>) -> DestinationStatus {
            destination(error).0
        }
        assert_eq!(
            status(Some(&refused(russh::ChannelOpenFailure::ConnectFailed))),
            DestinationStatus::Unavailable
        );
        assert_eq!(
            status(Some(&refused(
                russh::ChannelOpenFailure::AdministrativelyProhibited
            ))),
            DestinationStatus::Blocked
        );
        assert_eq!(status(None), DestinationStatus::Unknown);
        assert_eq!(
            status(Some(&newport_ssh::Error::Lost("reset".into()))),
            DestinationStatus::Unknown
        );
    }
}
