//! A jump server: its `direct-tcpip` channels connect to real TCP addresses,
//! as OpenSSH's do for `ProxyJump`.
use russh::{
    Channel, ChannelOpenFailure, keys,
    server::{self, Auth, ChannelOpenHandle, Msg, Server as _, Session},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::net::{TcpListener, TcpStream};

/// What the jump server saw.
#[derive(Default)]
pub struct Forwards {
    pub connections: AtomicUsize,
    /// Session channels asked for; a jump host needs none.
    pub sessions: AtomicUsize,
    /// Forwarding channels open right now.
    pub open: AtomicUsize,
    /// Forwards whose address couldn't be reached.
    pub refused: AtomicUsize,
}

pub struct JumpServer {
    pub port: u16,
    pub seen: Arc<Forwards>,
}

pub async fn jump_server(host_key: &str) -> JumpServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Forwards::default());
    let config = Arc::new(server::Config {
        keys: vec![keys::PrivateKey::from_openssh(host_key).unwrap()],
        auth_rejection_time: Duration::from_millis(1),
        ..Default::default()
    });
    let mut accepting = Accepting { seen: seen.clone() };
    tokio::spawn(async move {
        let _ = accepting.run_on_socket(config, &listener).await;
    });
    JumpServer { port, seen }
}

struct Accepting {
    seen: Arc<Forwards>,
}

impl server::Server for Accepting {
    type Handler = Jump;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Jump {
        self.seen.connections.fetch_add(1, Ordering::SeqCst);
        Jump {
            seen: self.seen.clone(),
        }
    }
}

struct Jump {
    seen: Arc<Forwards>,
}

impl server::Handler for Jump {
    type Error = russh::Error;

    async fn auth_publickey_offered(
        &mut self,
        _: &str,
        _: server::PublicKeyOffer<'_>,
    ) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn auth_publickey(
        &mut self,
        _: &str,
        _: server::VerifiedPublicKey<'_>,
    ) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        _: Channel<Msg>,
        reply: ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        self.seen.sessions.fetch_add(1, Ordering::SeqCst);
        reply
            .reject(ChannelOpenFailure::AdministrativelyProhibited)
            .await;
        Ok(())
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host: &str,
        port: u32,
        _: &str,
        _: u32,
        reply: ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        let seen = self.seen.clone();
        let address = format!("{host}:{port}");
        tokio::spawn(async move {
            let Ok(mut tcp) = TcpStream::connect(&address).await else {
                seen.refused.fetch_add(1, Ordering::SeqCst);
                reply.reject(ChannelOpenFailure::ConnectFailed).await;
                return;
            };
            reply.accept().await;
            seen.open.fetch_add(1, Ordering::SeqCst);
            let mut stream = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut stream, &mut tcp).await;
            seen.open.fetch_sub(1, Ordering::SeqCst);
        });
        Ok(())
    }
}
