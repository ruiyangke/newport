use crate::{
    Incoming,
    link::{ConnectionId, Event},
    listener::Routes,
};
use russh::{
    ChannelId, ChannelOpenFailure,
    client::{self, ChannelOpenHandle, DisconnectReason, Msg, Session},
    keys::{HashAlg, PublicKeyOrCertificate},
};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, watch};

/// The russh client handler of one connection: checks the host key against
/// the link's pinned one, routes forwarded channels to their listeners, and
/// reports channel closes and disconnects to the link.
pub struct Handler {
    pub(crate) connection: ConnectionId,
    pub(crate) events: mpsc::UnboundedSender<Event>,
    pub(crate) host_key: Arc<HostKey>,
    pub(crate) routes: Routes,
    pub(crate) lost: watch::Receiver<Option<crate::Error>>,
}

/// The connection's loop owns its handler, so the handler going away means
/// the connection ended, however it ended.
impl Drop for Handler {
    fn drop(&mut self) {
        let _ = self
            .events
            .send(Event::Lost(self.connection, "the connection closed".into()));
    }
}

/// The verifier, and the fingerprint the first connection accepted since the
/// link started or the person asked to retry: every later connection must
/// present the same one.
pub(crate) struct HostKey {
    pub(crate) target: crate::Target,
    pub(crate) verifier: Arc<dyn crate::HostKeys>,
    pinned: Mutex<Option<String>>,
    /// Why the login in progress had its key refused.
    pub(crate) refused: Mutex<Option<crate::Error>>,
}

impl HostKey {
    pub(crate) fn new(target: crate::Target, verifier: Arc<dyn crate::HostKeys>) -> Self {
        Self {
            target,
            verifier,
            pinned: Mutex::default(),
            refused: Mutex::default(),
        }
    }

    /// Forget the pinned key, so the verifier alone decides the next one.
    pub(crate) fn unpin(&self) {
        *self.pinned.lock().unwrap() = None;
    }

    fn check(&self, key: &PublicKeyOrCertificate, fingerprint: String) -> bool {
        if let Err(error) = self.verifier.verify(&self.target, key) {
            *self.refused.lock().unwrap() = Some(error);
            return false;
        }
        let mut pinned = self.pinned.lock().unwrap();
        match pinned.as_ref() {
            None => {
                *pinned = Some(fingerprint);
                true
            }
            Some(known) if *known == fingerprint => true,
            Some(_) => {
                *self.refused.lock().unwrap() = Some(crate::Error::HostKeyChanged { fingerprint });
                false
            }
        }
    }
}

/// Hands a forwarded channel's open to its listener, unanswered.
struct Deliver {
    connection: ConnectionId,
    events: mpsc::UnboundedSender<Event>,
    lost: watch::Receiver<Option<crate::Error>>,
}

impl Deliver {
    async fn to(
        self,
        route: &mpsc::Sender<Incoming>,
        channel: russh::Channel<Msg>,
        reply: ChannelOpenHandle,
    ) {
        let Ok(permit) = route.try_reserve() else {
            reply.reject(ChannelOpenFailure::ResourceShortage).await;
            return;
        };
        permit.send(Incoming {
            channel,
            reply,
            connection: self.connection,
            events: self.events,
            lost: self.lost,
        });
    }
}

impl client::Handler for Handler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let fingerprint = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.fingerprint(HashAlg::Sha256),
            PublicKeyOrCertificate::Certificate(certificate) => {
                certificate.public_key().fingerprint(HashAlg::Sha256)
            }
        };
        Ok(self.host_key.check(key, fingerprint.to_string()))
    }

    /// Never waits on the link or a listener: this runs on the connection's
    /// loop, which would stall every other channel.
    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<Msg>,
        _: &str,
        connected_port: u32,
        _: &str,
        _: u32,
        reply: ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        let (route, binding) = {
            let routes = self.routes.lock().unwrap();
            (
                routes.ports.get(&connected_port).cloned(),
                routes.binding.clone(),
            )
        };
        let deliver = Deliver {
            connection: self.connection,
            events: self.events.clone(),
            lost: self.lost.clone(),
        };
        match (route, binding) {
            (Some(route), _) => deliver.to(&route, channel, reply).await,
            (None, Some(mut binding)) => {
                // It may be for the port a forward is being bound to.
                let routes = self.routes.clone();
                tokio::spawn(async move {
                    let _ = binding.changed().await;
                    let route = routes.lock().unwrap().ports.get(&connected_port).cloned();
                    match route {
                        Some(route) => deliver.to(&route, channel, reply).await,
                        None => reply.reject(ChannelOpenFailure::ConnectFailed).await,
                    }
                });
            }
            (None, None) => reply.reject(ChannelOpenFailure::ConnectFailed).await,
        }
        Ok(())
    }

    async fn channel_close(
        &mut self,
        channel: ChannelId,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        let _ = self
            .events
            .send(Event::ChannelClosed(self.connection, channel));
        Ok(())
    }

    async fn disconnected(
        &mut self,
        reason: DisconnectReason<Self::Error>,
    ) -> Result<(), Self::Error> {
        let cause = match &reason {
            DisconnectReason::ReceivedDisconnect(info) => {
                format!("the server disconnected ({:?})", info.reason_code)
            }
            DisconnectReason::Error(error) => error.to_string(),
        };
        let _ = self.events.send(Event::Lost(self.connection, cause));
        match reason {
            DisconnectReason::ReceivedDisconnect(_) => Ok(()),
            DisconnectReason::Error(error) => Err(error),
        }
    }
}
