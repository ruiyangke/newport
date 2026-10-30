//! SSH links: everything Newport has open to one server.
//!
//! A [`Link`] owns the server's SSH connections, logs in one connection at a
//! time, hands out [`Channel`]s, and publishes one [`Status`].
//! Consumers speak their own protocols over their channels and decide for
//! themselves how to recover when a channel reports its connection lost.
//!
//! The design is `crates/newport-backend/src/ssh/transport.adoc`.
mod channel;
mod dial;
mod error;
mod handler;
mod inject;
mod link;
mod listener;
mod status;

pub use channel::{Channel, Fate};
pub use dial::{CommandDialer, JumpDialer, TcpDialer};
pub use error::{Error, Retry};
pub use handler::Handler;
pub use inject::{Abort, Authenticator, BoxFuture, Dialed, Dialer, HostKeys, Parts, Stream};
pub use link::{Limits, Link, Open, Target};
pub use listener::{Incoming, Listener};
pub use status::{Login, Reach, Status};
