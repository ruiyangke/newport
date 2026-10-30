//! Shared listener-to-SSH relay for persistent tunnels and temporary callbacks.
use std::{future::Future, io, net::SocketAddr, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpListener,
    task::JoinSet,
};

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

/// Whether an accept error is about one connection or the process rather than
/// the listener, so the listener keeps serving.
#[derive(Debug, PartialEq)]
enum AcceptFailure {
    /// That one connection is gone.
    Connection,
    /// Out of file descriptors; wait for some to free up.
    Exhausted,
    /// The listener itself is unusable.
    Fatal,
}

/// EMFILE, ENFILE and ENOMEM have the same numbers on Linux and macOS, and
/// `std::io::ErrorKind` has no stable kind for them.
#[cfg(unix)]
const EXHAUSTED_ERRNOS: [i32; 3] = [24, 23, 12];
#[cfg(not(unix))]
const EXHAUSTED_ERRNOS: [i32; 0] = [];

fn classify_accept(error: &io::Error) -> AcceptFailure {
    use io::ErrorKind::*;
    match error.kind() {
        ConnectionAborted | ConnectionReset | Interrupted | WouldBlock | TimedOut => {
            AcceptFailure::Connection
        }
        _ if error
            .raw_os_error()
            .is_some_and(|code| EXHAUSTED_ERRNOS.contains(&code)) =>
        {
            AcceptFailure::Exhausted
        }
        _ => AcceptFailure::Fatal,
    }
}

/// Pause before accepting again when the process has no descriptors left.
const EXHAUSTED_BACKOFF: Duration = Duration::from_millis(100);

/// Dropping this future cancels every accepted client along with the listener.
/// `open` gives the channel for one accepted connection from `origin`, or
/// nothing when none could be opened.
pub(super) async fn serve<Open, Opening, Stream>(
    listener: TcpListener,
    open: Open,
    policy: Policy,
) -> io::Result<()>
where
    Open: Fn(SocketAddr) -> Opening,
    Opening: Future<Output = Option<Stream>> + Send + 'static,
    Stream: AsyncRead + AsyncWrite + Unpin + Send,
{
    let mut clients = JoinSet::new();
    let mut logged = std::collections::HashSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (mut socket, origin) = match accepted {
                    Ok(accepted) => accepted,
                    Err(error) => {
                        match classify_accept(&error) {
                            AcceptFailure::Fatal => return Err(error),
                            AcceptFailure::Connection => {}
                            AcceptFailure::Exhausted => tokio::time::sleep(EXHAUSTED_BACKOFF).await,
                        }
                        if logged.insert(error.kind()) {
                            log::warn!("Accepting a forwarded connection failed: {error}");
                        }
                        continue;
                    }
                };
                if clients.len() >= policy.max_clients { continue; }
                let open = open(origin);
                clients.spawn(async move {
                    if let Ok(Some(mut stream)) = tokio::time::timeout(Duration::from_secs(10), open).await {
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

#[cfg(test)]
mod tests {
    use super::{AcceptFailure, classify_accept};
    use std::io;

    #[test]
    fn transient_accept_errors_do_not_end_the_listener() {
        for kind in [
            io::ErrorKind::ConnectionAborted,
            io::ErrorKind::ConnectionReset,
            io::ErrorKind::Interrupted,
            io::ErrorKind::WouldBlock,
            io::ErrorKind::TimedOut,
        ] {
            assert_eq!(
                classify_accept(&io::Error::from(kind)),
                AcceptFailure::Connection
            );
        }
        assert_eq!(
            classify_accept(&io::Error::from(io::ErrorKind::PermissionDenied)),
            AcceptFailure::Fatal
        );
    }

    #[cfg(unix)]
    #[test]
    fn exhausted_and_invalid_listener_errors_are_distinct() {
        for code in [libc::EMFILE, libc::ENFILE, libc::ENOMEM] {
            assert_eq!(
                classify_accept(&io::Error::from_raw_os_error(code)),
                AcceptFailure::Exhausted
            );
        }
        for code in [libc::EBADF, libc::EINVAL] {
            assert_eq!(
                classify_accept(&io::Error::from_raw_os_error(code)),
                AcceptFailure::Fatal
            );
        }
    }
}
