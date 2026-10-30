//! The parts a link is built from: how it reaches the server and how it
//! logs in. Tests replace both.
use crate::{Error, Handler, Target};
use russh::keys::PublicKeyOrCertificate;
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::watch,
};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Stream: AsyncRead + AsyncWrite + Send + Unpin + 'static {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> Stream for T {}

/// Closes a dialed stream at once, even while a write on it is blocked.
#[derive(Clone)]
pub struct Abort(Arc<dyn Fn() + Send + Sync>);

impl Abort {
    pub fn new(abort: impl Fn() + Send + Sync + 'static) -> Self {
        Self(Arc::new(abort))
    }
    pub fn abort(&self) {
        (self.0)()
    }
}

pub struct Dialed {
    pub stream: Box<dyn Stream>,
    pub abort: Abort,
}

/// Reaches the server. A dial that waits for the person to approve something,
/// such as a jump host's login, sets `approval` to true while it waits; the
/// link pauses the dial's deadline and shows the wait.
pub trait Dialer: Send + Sync + 'static {
    fn dial<'a>(
        &'a self,
        target: &'a Target,
        approval: &'a watch::Sender<bool>,
    ) -> BoxFuture<'a, Result<Dialed, Error>>;

    /// How long a dial may take, given the link's own limit; `None` when the
    /// dial keeps its own deadlines.
    fn deadline(&self, default: Duration) -> Option<Duration> {
        Some(default)
    }

    /// The link was asked to try again; pass it on to whatever the dial
    /// waits for.
    fn retry(&self) {}

    /// The person asked the link to try again with a fresh login; pass it on
    /// to whatever the dial waits for.
    fn retry_fresh(&self) {}
}

/// Logs in on a connection whose handshake is done. While it waits for the
/// person to approve a login in their SSH agent, it sets `approval` to true;
/// the link pauses the login's deadline and shows the wait.
pub trait Authenticator: Send + Sync + 'static {
    fn authenticate<'a>(
        &'a self,
        handle: &'a mut russh::client::Handle<Handler>,
        target: &'a Target,
        approval: &'a watch::Sender<bool>,
    ) -> BoxFuture<'a, Result<(), Error>>;
}

/// Decides whether a server's host key is the right one, as `known_hosts`
/// does. The link also pins the key its first connection accepted, so every
/// later connection must present the same one.
pub trait HostKeys: Send + Sync + 'static {
    fn verify(&self, target: &Target, key: &PublicKeyOrCertificate) -> Result<(), Error>;
}

/// What a link is built from besides its target.
#[derive(Clone)]
pub struct Parts {
    pub dialer: std::sync::Arc<dyn Dialer>,
    pub authenticator: std::sync::Arc<dyn Authenticator>,
    pub host_keys: std::sync::Arc<dyn HostKeys>,
}
