//! Remote forwards: a `tcpip-forward` registered on one connection, and the
//! `forwarded-tcpip` channels the server opens for it.
use crate::{Channel, Error, Handler, link::ConnectionId, link::Event};
use russh::{
    ChannelOpenFailure,
    client::{self, ChannelOpenHandle, Msg},
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::{mpsc, watch};

/// Channel opens a listener holds before its owner takes them; the server's
/// next one is refused.
pub(crate) const BACKLOG: usize = 128;

/// Where a connection's handler sends each forwarded channel, by the port the
/// server bound.
pub(crate) type Routes = Arc<Mutex<RouteTable>>;

#[derive(Default)]
pub(crate) struct RouteTable {
    pub(crate) ports: HashMap<u32, mpsc::Sender<Incoming>>,
    /// Set while a forward for port 0 waits for its reply. The server may
    /// open a channel for the port it bound before the route is in place, so
    /// a channel for a port nobody routes waits for this to clear.
    pub(crate) binding: Option<watch::Receiver<()>>,
}

/// A connection's `tcpip-forward` and `cancel-tcpip-forward` requests. They
/// run one at a time in order, so a cancel can't overtake the forward that
/// replaces it on the same port.
pub(crate) enum Forward {
    Listen(Listen),
    Cancel { address: String, port: u32 },
}

pub(crate) type Forwards = mpsc::UnboundedSender<Forward>;

/// Runs a connection's forward requests until it closes.
pub(crate) async fn forward_in_order(
    handle: Arc<client::Handle<Handler>>,
    mut forwards: mpsc::UnboundedReceiver<Forward>,
) {
    while let Some(forward) = forwards.recv().await {
        match forward {
            Forward::Listen(listen) => listen.run(&handle).await,
            Forward::Cancel { address, port } => {
                if let Err(error) = handle.cancel_tcpip_forward(address, port).await {
                    log::debug!("Cancelling the forward of port {port} failed: {error}");
                }
            }
        }
    }
}

/// A `tcpip-forward` request and what its answer needs.
pub(crate) struct Listen {
    pub(crate) request: crate::link::Request,
    pub(crate) address: String,
    pub(crate) port: u32,
    pub(crate) sender: mpsc::Sender<Incoming>,
    pub(crate) channels: mpsc::Receiver<Incoming>,
    pub(crate) deadline: std::time::Duration,
    pub(crate) connection: ConnectionId,
    pub(crate) routes: Routes,
    pub(crate) lost: watch::Receiver<Option<Error>>,
    pub(crate) events: mpsc::UnboundedSender<Event>,
    pub(crate) forwards: Forwards,
}

impl Listen {
    async fn run(self, handle: &client::Handle<Handler>) {
        let Listen {
            mut request,
            address,
            port,
            sender,
            channels,
            deadline,
            connection,
            routes,
            lost,
            events,
            forwards,
        } = self;
        // Dropped once the route is in place or the request failed, which
        // releases channels held for it.
        let binding = (port == 0).then(|| {
            let (binding, waiting) = watch::channel(());
            routes.lock().unwrap().binding = Some(waiting);
            binding
        });
        let forwarding = handle.tcpip_forward(address.clone(), port);
        tokio::pin!(forwarding);
        // The request runs to its end even if the requester gives up, so a
        // forward the server sets up late is cancelled, not leaked.
        let result = tokio::select! {
            result = &mut forwarding => result,
            () = tokio::time::sleep(deadline) => {
                request.fail_early(Error::TimedOut);
                let _ = events.send(Event::OpenTimedOut);
                forwarding.await
            }
        };
        let bound = match result {
            Ok(bound) if port == 0 => bound,
            Ok(_) => port,
            Err(error) => {
                let mut table = routes.lock().unwrap();
                if port == 0 {
                    table.binding = None;
                } else {
                    table.ports.remove(&port);
                }
                drop(table);
                let _ = events.send(Event::Unlisten(connection));
                match error {
                    russh::Error::RequestDenied => request.fail(Error::RequestDenied),
                    _ => {
                        let _ = events.send(Event::OpenFailed {
                            connection,
                            request,
                        });
                    }
                }
                return;
            }
        };
        if binding.is_some() {
            let mut table = routes.lock().unwrap();
            table.ports.insert(bound, sender);
            table.binding = None;
        }
        drop(binding);
        // An unanswered listener is dropped here, which cancels it.
        let listener = Listener {
            channels,
            address,
            port: bound,
            connection,
            routes,
            lost,
            events,
            forwards,
        };
        if let Some(crate::link::Reply::Listener(reply)) = request.reply.take() {
            let _ = reply.send(Ok(listener));
        }
    }
}

/// A remote forward on one connection. Dropping it cancels the forward on the
/// server.
pub struct Listener {
    pub(crate) channels: mpsc::Receiver<Incoming>,
    pub(crate) address: String,
    pub(crate) port: u32,
    pub(crate) connection: ConnectionId,
    pub(crate) routes: Routes,
    pub(crate) lost: watch::Receiver<Option<Error>>,
    pub(crate) events: mpsc::UnboundedSender<Event>,
    pub(crate) forwards: Forwards,
}

impl Listener {
    /// The server's next forwarded channel, still waiting for an answer, or
    /// None once the connection is lost.
    pub async fn accept(&mut self) -> Option<Incoming> {
        let mut lost = self.lost.clone();
        tokio::select! {
            // A channel on a lost connection is useless.
            biased;
            _ = lost.wait_for(Option::is_some) => None,
            channel = self.channels.recv() => channel,
        }
    }

    /// Resolves when the connection under this listener closes, with why.
    pub async fn lost(&self) -> Error {
        let mut lost = self.lost.clone();
        match lost.wait_for(Option::is_some).await {
            Ok(error) => error.clone().unwrap_or(Error::Closed),
            Err(_) => Error::Closed,
        }
    }

    /// The port the server bound, which the server picks when the request
    /// asked for port 0.
    pub fn port(&self) -> u32 {
        self.port
    }

    /// Which of the link's connections holds the forward.
    pub fn connection(&self) -> u64 {
        self.connection
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        // Closed first, so the route below reads as ours and a newer
        // listener on the same port keeps its own.
        self.channels.close();
        let mut routes = self.routes.lock().unwrap();
        if routes
            .ports
            .get(&self.port)
            .is_some_and(mpsc::Sender::is_closed)
        {
            routes.ports.remove(&self.port);
        }
        drop(routes);
        // Queued now, before a listen on the same port that follows the drop.
        let _ = self.forwards.send(Forward::Cancel {
            address: std::mem::take(&mut self.address),
            port: self.port,
        });
        let _ = self.events.send(Event::Unlisten(self.connection));
    }
}

/// A `forwarded-tcpip` channel the server asked to open, which its listener's
/// owner accepts or rejects. Dropping it rejects it.
pub struct Incoming {
    pub(crate) channel: russh::Channel<Msg>,
    pub(crate) reply: ChannelOpenHandle,
    pub(crate) connection: ConnectionId,
    pub(crate) events: mpsc::UnboundedSender<Event>,
    pub(crate) lost: watch::Receiver<Option<Error>>,
}

impl Incoming {
    /// Confirms the channel to the server.
    pub async fn accept(self) -> Channel {
        // Sent before the channel can close, so the link counts it first.
        let _ = self
            .events
            .send(Event::Accepted(self.connection, self.channel.id()));
        self.reply.accept().await;
        Channel::new(self.channel, self.connection, self.lost)
    }

    /// Refuses the channel, for example because its local target didn't
    /// answer.
    pub async fn reject(self, reason: ChannelOpenFailure) {
        self.reply.reject(reason).await;
    }

    /// Which of the link's connections the server asked on.
    pub fn connection(&self) -> u64 {
        self.connection
    }
}
