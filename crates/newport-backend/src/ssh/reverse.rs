//! Server ports exposed by remote forwards (ssh -R), relayed to this Mac's loopback.
use russh::ChannelOpenFailure;
use std::time::Duration;
use tokio::{net::TcpStream, task::JoinSet};

const MAX_CLIENTS: usize = 128;
/// How long a forwarded client waits for this Mac's target to answer.
const LOCAL_CONNECT: Duration = Duration::from_secs(3);

/// Relays each channel the server forwards on `listener` to `local` on this
/// Mac's loopback, until the listener's connection is lost. Returns why.
/// Clients end with it: their channels died with the connection.
pub(super) async fn serve(listener: &mut newport_ssh::Listener, local: u16) -> newport_ssh::Error {
    let mut clients = JoinSet::new();
    loop {
        tokio::select! {
            incoming = listener.accept() => {
                let Some(incoming) = incoming else { break };
                if clients.len() >= MAX_CLIENTS {
                    incoming.reject(ChannelOpenFailure::ResourceShortage).await;
                    continue;
                }
                clients.spawn(async move {
                    // The server's client sees a refusal, not an open channel
                    // that closes at once, when the target isn't there.
                    let target = tokio::time::timeout(
                        LOCAL_CONNECT,
                        TcpStream::connect(("127.0.0.1", local)),
                    )
                    .await;
                    let Ok(Ok(mut socket)) = target else {
                        incoming.reject(ChannelOpenFailure::ConnectFailed).await;
                        return;
                    };
                    let mut stream = incoming.accept().await.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
                });
            }
            _ = clients.join_next(), if !clients.is_empty() => {}
        }
    }
    listener.lost().await
}
