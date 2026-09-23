//! Destination TCP checks use the existing SSH transport; no remote shell or payload.
use super::Connection;
use crate::model::{DestinationHealth, DestinationStatus};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Semaphore;

pub(super) type Health = Arc<Mutex<Vec<DestinationHealth>>>;
fn failure(error: Option<&russh::Error>) -> (DestinationStatus, String) {
    use russh::{ChannelOpenFailure, Error};
    match error {
        Some(Error::ChannelOpenFailure(ChannelOpenFailure::ConnectFailed)) =>
            (DestinationStatus::Unavailable, "SSH is connected, but the server could not connect to this destination. The service may be stopped or unreachable.".into()),
        Some(Error::ChannelOpenFailure(ChannelOpenFailure::AdministrativelyProhibited)) =>
            (DestinationStatus::Blocked, "The SSH server does not permit forwarding to this destination.".into()),
        None => (DestinationStatus::Unknown, "The destination check timed out after 3 seconds.".into()),
        Some(_) => (DestinationStatus::Unknown, "The SSH server could not complete the destination check.".into()),
    }
}
pub(super) async fn monitor(
    connection: Arc<Connection>,
    host: String,
    remote: u16,
    health: Health,
    slots: Arc<Semaphore>,
) {
    loop {
        let Ok(permit) = slots.acquire().await else {
            return;
        };
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            connection.handle.channel_open_direct_tcpip(
                host.clone(),
                u32::from(remote),
                "127.0.0.1",
                0,
            ),
        )
        .await;
        let (status, message) = match result {
            Ok(Ok(channel)) => {
                // Bound cleanup too: a stalled close must not consume a probe slot forever.
                let _ = tokio::time::timeout(Duration::from_secs(1), channel.close()).await;
                (DestinationStatus::Reachable, None)
            }
            Ok(Err(error)) => {
                let (status, message) = failure(Some(&error));
                (status, Some(message))
            }
            Err(_) => {
                let (status, message) = failure(None);
                (status, Some(message))
            }
        };
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
        drop(permit);
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distinguishes_failed_destination_from_policy_and_unknown() {
        assert_eq!(
            failure(Some(&russh::Error::ChannelOpenFailure(
                russh::ChannelOpenFailure::ConnectFailed
            )))
            .0,
            DestinationStatus::Unavailable
        );
        assert_eq!(
            failure(Some(&russh::Error::ChannelOpenFailure(
                russh::ChannelOpenFailure::AdministrativelyProhibited
            )))
            .0,
            DestinationStatus::Blocked
        );
        assert_eq!(failure(None).0, DestinationStatus::Unknown);
        assert_eq!(
            failure(Some(&russh::Error::HUP)).0,
            DestinationStatus::Unknown
        );
    }
}
