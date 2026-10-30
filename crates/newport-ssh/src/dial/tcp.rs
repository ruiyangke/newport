use crate::{Abort, BoxFuture, Dialed, Dialer, Error, Target};
use std::{future::Future, io, time::Duration};
use tokio::sync::watch;

/// On macOS, connections to the local network fail at once with "No route
/// to host" while the system settles an app's Local Network access, as it
/// does for each new build (TN3179). Network framework waits for it; so does
/// this, briefly, before the host counts as unreachable.
#[cfg(target_os = "macos")]
const LOCAL_NETWORK_WAITS: &[Duration] = &[
    Duration::from_millis(100),
    Duration::from_millis(200),
    Duration::from_millis(400),
    Duration::from_millis(800),
    Duration::from_millis(1500),
];
#[cfg(not(target_os = "macos"))]
const LOCAL_NETWORK_WAITS: &[Duration] = &[];

/// TCP to the target's host and port.
pub struct TcpDialer;

impl Dialer for TcpDialer {
    fn dial<'a>(
        &'a self,
        target: &'a Target,
        _: &'a watch::Sender<bool>,
    ) -> BoxFuture<'a, Result<Dialed, Error>> {
        Box::pin(async move {
            let stream = connect_settled(
                || tokio::net::TcpStream::connect((target.host.as_str(), target.port)),
                LOCAL_NETWORK_WAITS,
            )
            .await
            .map_err(|error| Error::Unreachable(error.to_string()))?;
            stream.set_nodelay(true).ok();
            let std = stream
                .into_std()
                .map_err(|error| Error::Unreachable(error.to_string()))?;
            let shutdown = std
                .try_clone()
                .map_err(|error| Error::Unreachable(error.to_string()))?;
            let stream = tokio::net::TcpStream::from_std(std)
                .map_err(|error| Error::Unreachable(error.to_string()))?;
            Ok(Dialed {
                stream: Box::new(stream),
                abort: Abort::new(move || {
                    let _ = shutdown.shutdown(std::net::Shutdown::Both);
                }),
            })
        })
    }
}

/// Connects, trying again after each of `waits` while the host has no route.
async fn connect_settled<T, F, C>(mut connect: F, waits: &[Duration]) -> io::Result<T>
where
    F: FnMut() -> C,
    C: Future<Output = io::Result<T>>,
{
    let mut waits = waits.iter();
    loop {
        match connect().await {
            Err(error) if error.kind() == io::ErrorKind::HostUnreachable => match waits.next() {
                Some(wait) => tokio::time::sleep(*wait).await,
                None => return Err(error),
            },
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::connect_settled;
    use std::{cell::Cell, io, time::Duration};

    const WAITS: &[Duration] = &[Duration::from_millis(1); 3];

    async fn attempts(results: &[io::ErrorKind]) -> (io::Result<()>, usize) {
        let calls = Cell::new(0);
        let result = connect_settled(
            || {
                let call = calls.get();
                calls.set(call + 1);
                let kind = results.get(call).copied();
                async move { kind.map_or(Ok(()), |kind| Err(io::Error::from(kind))) }
            },
            WAITS,
        )
        .await;
        (result, calls.get())
    }

    #[tokio::test]
    async fn no_route_while_access_settles_is_tried_again() {
        let (result, calls) = attempts(&[io::ErrorKind::HostUnreachable]).await;
        assert!(result.is_ok());
        assert_eq!(calls, 2);
    }

    #[tokio::test]
    async fn no_route_that_lasts_is_unreachable_after_the_waits() {
        let (result, calls) = attempts(&[io::ErrorKind::HostUnreachable; 5]).await;
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::HostUnreachable);
        assert_eq!(calls, WAITS.len() + 1);
    }

    #[tokio::test]
    async fn other_failures_are_not_tried_again() {
        for kind in [io::ErrorKind::ConnectionRefused, io::ErrorKind::TimedOut] {
            let (result, calls) = attempts(&[kind]).await;
            assert_eq!(result.unwrap_err().kind(), kind);
            assert_eq!(calls, 1);
        }
    }
}
