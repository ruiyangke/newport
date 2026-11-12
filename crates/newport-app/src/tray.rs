//! Status polling belongs to application composition, never to the GUI thread.
use crate::desktop::Integration;
use newport_client::backend::Backend;
use newport_protocol::{
    common::Present,
    desktop::{
        Operation, Reply,
        configuration::{self, ConnectionRow, ConnectionStatus},
    },
};
use std::time::Duration;

fn status(tunnels: &[ConnectionRow]) -> (usize, bool) {
    (
        tunnels
            .iter()
            .filter(|row| row.value.status == ConnectionStatus::Connected)
            .count(),
        tunnels
            .iter()
            .any(|row| row.value.status == ConnectionStatus::Error),
    )
}

pub async fn monitor(backend: Backend, desktop: Integration) {
    let mut published = None;
    loop {
        let client = backend.clone();
        let next = tokio::task::spawn_blocking(move || {
            let Reply::Configuration(reply) = client
                .execute_with_timeout(
                    Operation::Configuration(Box::new(configuration::Operation::Snapshot(
                        Box::new(Present {}),
                    ))),
                    Duration::from_secs(2),
                )
                .ok()?
            else {
                return None;
            };
            let configuration::Reply::Snapshot(snapshot) = *reply else {
                return None;
            };
            Some(status(&snapshot.runtime.tunnels))
        })
        .await
        .ok()
        .flatten();
        if published != Some(next) {
            if !desktop.tray_status(next).await {
                return;
            }
            published = Some(next);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(status: ConnectionStatus) -> ConnectionRow {
        ConnectionRow {
            id: uuid::Uuid::new_v4(),
            value: configuration::ConnectionState {
                status,
                error_message: None,
                reconnect_attempt: 0,
            },
        }
    }
    #[test]
    fn status_counts_connected_tunnels_and_reports_errors() {
        assert_eq!(status(&[]), (0, false));
        assert_eq!(
            status(&[
                row(ConnectionStatus::Connected),
                row(ConnectionStatus::Connecting),
                row(ConnectionStatus::Error),
                row(ConnectionStatus::Connected)
            ]),
            (2, true)
        );
        assert_eq!(status(&[row(ConnectionStatus::Error)]), (0, true));
    }
}
