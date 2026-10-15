//! Server ports exposed by remote forwards (ssh -R), relayed to this Mac's loopback.
use russh::{Channel, ChannelOpenFailure, client};
use std::{collections::HashMap, sync::Mutex, time::Duration};
use tokio::{net::TcpStream, task::JoinSet};

const MAX_CLIENTS: usize = 128;

/// Exposed server ports and their accepted clients. Clients are aborted with
/// the forwarding, never outliving it.
#[derive(Default)]
pub(super) struct RemoteForwards {
    ports: Mutex<HashMap<u32, u16>>,
    clients: Mutex<JoinSet<()>>,
}
impl RemoteForwards {
    pub fn expose(&self, remote: u32, local: u16) {
        self.ports.lock().unwrap().insert(remote, local);
    }
    /// Runs on the session task, so connecting to the Mac happens elsewhere.
    pub fn accept(
        &self,
        channel: Channel<client::Msg>,
        port: u32,
        reply: client::ChannelOpenHandle,
    ) {
        // An unrequested port is rejected by dropping its reply.
        let Some(local) = self.ports.lock().unwrap().get(&port).copied() else {
            return;
        };
        let mut clients = self.clients.lock().unwrap();
        while clients.try_join_next().is_some() {}
        if clients.len() >= MAX_CLIENTS {
            return;
        }
        clients.spawn(async move {
            let target = tokio::time::timeout(
                Duration::from_secs(3),
                TcpStream::connect(("127.0.0.1", local)),
            )
            .await;
            let Ok(Ok(mut socket)) = target else {
                reply.reject(ChannelOpenFailure::ConnectFailed).await;
                return;
            };
            reply.accept().await;
            let mut stream = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
        });
    }
    pub fn stop(&self) {
        self.clients.lock().unwrap().abort_all();
    }
}
