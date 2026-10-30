//! The ways a link reaches its server: TCP, a ProxyCommand, or a channel
//! through another link.
mod command;
mod jump;
mod tcp;

pub use command::CommandDialer;
pub use jump::JumpDialer;
pub use tcp::TcpDialer;
