//! Destination checks: a channel to each destination, opened and closed; no remote shell or payload.
use crate::model::{DestinationHealth, DestinationStatus};
use std::{
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
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
/// A port on this Mac's loopback, in both families: a local server may
/// listen on only one, as Node does on `[::1]` for `localhost`.
pub(super) fn this_mac(port: u16) -> [SocketAddr; 2] {
    [
        SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        SocketAddr::from((Ipv6Addr::LOCALHOST, port)),
    ]
}
/// A remote forward's destination is a port on this Mac's loopback.
pub(super) async fn monitor_local(local: u16, remote: u16, health: Health) {
    let target = this_mac(local);
    loop {
        let connect = tokio::net::TcpStream::connect(&target[..]);
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
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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
    #[tokio::test]
    async fn this_macs_port_is_reached_in_either_family() {
        for ip in [
            std::net::IpAddr::from(Ipv4Addr::LOCALHOST),
            Ipv6Addr::LOCALHOST.into(),
        ] {
            let listener = tokio::net::TcpListener::bind((ip, 0)).await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let connected = tokio::net::TcpStream::connect(&this_mac(port)[..])
                .await
                .unwrap();
            assert_eq!(connected.peer_addr().unwrap().ip(), ip);
        }
    }
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
