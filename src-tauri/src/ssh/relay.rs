//! Shared listener-to-SSH relay for persistent tunnels and temporary callbacks.
use super::Connection;
use std::{io, sync::Arc, time::Duration};
use tokio::{net::TcpListener, task::JoinSet};

#[derive(Clone, Copy)]
pub(super) struct Policy {
    pub max_clients: usize,
    pub transfer_timeout: Option<Duration>,
}
impl Policy {
    pub const TUNNEL: Self = Self {
        max_clients: 128,
        transfer_timeout: None,
    };
    pub const CALLBACK: Self = Self {
        max_clients: 16,
        transfer_timeout: Some(Duration::from_secs(30)),
    };
}

/// Dropping this future cancels every accepted client along with the listener.
pub(super) async fn serve(
    listener: TcpListener,
    connection: Arc<Connection>,
    host: String,
    port: u16,
    policy: Policy,
) -> io::Result<()> {
    let mut clients = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (mut socket, origin) = accepted?;
                if clients.len() >= policy.max_clients { continue; }
                let connection = connection.clone();
                let host = host.clone();
                clients.spawn(async move {
                    let open = connection.handle.channel_open_direct_tcpip(
                        host, u32::from(port), origin.ip().to_string(), u32::from(origin.port()),
                    );
                    if let Ok(Ok(channel)) = tokio::time::timeout(Duration::from_secs(10), open).await {
                        let mut stream = channel.into_stream();
                        let transfer = tokio::io::copy_bidirectional(&mut socket, &mut stream);
                        if let Some(timeout) = policy.transfer_timeout {
                            let _ = tokio::time::timeout(timeout, transfer).await;
                        } else {
                            let _ = transfer.await;
                        }
                    }
                });
            }
            _ = clients.join_next(), if !clients.is_empty() => {}
        }
    }
}
