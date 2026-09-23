//! Temporary loopback listeners for remote browser authentication.
use super::Connection;
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::{
    net::TcpListener,
    task::{JoinHandle, JoinSet},
};

const LIFETIME: Duration = Duration::from_secs(300);

#[derive(Debug, PartialEq, Eq)]
pub struct Endpoint {
    host: String,
    port: u16,
}

/// Only explicit HTTP loopback callbacks are eligible. Never resolve arbitrary hosts.
pub fn endpoint(url: &url::Url) -> Result<Option<Endpoint>, String> {
    let values: Vec<_> = url
        .query_pairs()
        .filter(|(key, _)| key == "redirect_uri")
        .collect();
    if values.len() > 1 {
        return Err("Multiple redirect URLs; callback forwarding was not started.".into());
    }
    let detected = if let Some((_, value)) = values.first() {
        let redirect = url::Url::parse(value).map_err(|_| "Invalid redirect URL.")?;
        let host = match redirect.host() {
            Some(url::Host::Domain("localhost")) => Some("localhost".to_owned()),
            Some(url::Host::Ipv4(ip)) if ip.is_loopback() => Some(ip.to_string()),
            Some(url::Host::Ipv6(ip)) if ip == Ipv6Addr::LOCALHOST => Some(ip.to_string()),
            _ => None,
        };
        if let Some(host) = host {
            if redirect.scheme() != "http"
                || !redirect.username().is_empty()
                || redirect.password().is_some()
                || redirect.fragment().is_some()
            {
                return Err("Only HTTP loopback callbacks are supported.".into());
            }
            let port = redirect
                .port_or_known_default()
                .filter(|p| *p != 0)
                .ok_or("Invalid callback port.")?;
            Some(Endpoint { host, port })
        } else {
            None
        }
    } else {
        None
    };
    Ok(detected)
}

async fn bind(endpoint: &Endpoint) -> Result<Vec<TcpListener>, String> {
    let addresses = if endpoint.host == "localhost" {
        vec![
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        ]
    } else {
        vec![endpoint
            .host
            .parse::<IpAddr>()
            .map_err(|_| "Invalid callback address.")?]
    };
    let mut listeners = Vec::new();
    for ip in addresses {
        listeners.push(TcpListener::bind(SocketAddr::new(ip, endpoint.port)).await
            .map_err(|_| format!("Cannot listen on {ip}:{}. Close the app using that port or use device-code login.", endpoint.port))?);
    }
    Ok(listeners)
}

struct Lease(JoinHandle<()>);
impl Drop for Lease {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub struct Callbacks {
    connection: Arc<Connection>,
    leases: Vec<Lease>,
}
impl Callbacks {
    pub(super) fn new(connection: Arc<Connection>) -> Self {
        Self {
            connection,
            leases: Vec::new(),
        }
    }
    pub async fn prepare(&mut self, endpoint: Endpoint) -> Result<(), String> {
        self.leases.retain(|lease| !lease.0.is_finished());
        if self.leases.len() >= 8 {
            return Err("Too many pending browser logins. Wait for them to expire.".into());
        }
        let listeners = bind(&endpoint).await?;
        let connection = self.connection.clone();
        self.leases.push(Lease(tokio::spawn(async move {
            let mut workers = JoinSet::new();
            for listener in listeners {
                let connection = connection.clone();
                let host = endpoint.host.clone();
                workers.spawn(super::relay::serve(
                    listener,
                    connection,
                    host,
                    endpoint.port,
                    super::relay::Policy::CALLBACK,
                ));
            }
            let _ = tokio::time::timeout(LIFETIME, async {
                while workers.join_next().await.is_some() {}
            })
            .await;
            // Dropping JoinSet also cancels all active connections.
        })));
        Ok(())
    }
    pub fn rollback_last(&mut self) {
        self.leases.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn detect(value: &str) -> Result<Option<Endpoint>, String> {
        endpoint(&url::Url::parse(value).unwrap())
    }
    #[test]
    fn detects_only_explicit_loopback_callbacks() {
        for host in ["localhost", "127.0.0.1", "[::1]"] {
            assert_eq!(
                detect(&format!(
                    "https://login.example/?redirect_uri=http%3A%2F%2F{host}%3A1455%2Fcallback"
                ))
                .unwrap()
                .unwrap()
                .port,
                1455
            );
        }
        for value in [
            "https://login.example/#/device?user_code=abc",
            "https://login.example/?redirect_uri=https://provider.example/done",
            "https://login.example/?redirect_uri=http://localhost.evil:1455/",
        ] {
            assert!(detect(value).unwrap().is_none());
        }
        for value in [
            "https://login.example/?redirect_uri=http://localhost:0/",
            "https://login.example/?redirect_uri=https://localhost:1455/",
            "https://login.example/?redirect_uri=http://u:p@localhost:1455/",
            "https://login.example/?redirect_uri=x&redirect_uri=y",
        ] {
            assert!(detect(value).is_err());
        }
    }
    #[tokio::test]
    async fn occupied_port_fails_without_stealing_listener() {
        let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
        assert!(bind(&Endpoint {
            host: "127.0.0.1".into(),
            port: occupied.local_addr().unwrap().port()
        })
        .await
        .is_err());
    }
    #[tokio::test]
    #[ignore = "Run with scripts/test-ssh.sh"]
    async fn callback_uses_existing_ssh_and_releases_listener_on_drop() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let session =
            super::super::ExecSession::connect(&super::super::integration_tests::server())
                .await
                .unwrap();
        let reservation = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = reservation.local_addr().unwrap().port();
        drop(reservation);
        let mut callbacks = session.callbacks();
        callbacks
            .prepare(Endpoint {
                host: "127.0.0.1".into(),
                port,
            })
            .await
            .unwrap();
        let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let request = b"GET /callback?code=example&state=test HTTP/1.1\r\nHost: localhost\r\n\r\n";
        socket.write_all(request).await.unwrap();
        let mut echoed = vec![0; request.len()];
        tokio::time::timeout(Duration::from_secs(5), socket.read_exact(&mut echoed))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(echoed, request);
        drop(socket);
        drop(callbacks);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if TcpListener::bind(("127.0.0.1", port)).await.is_ok() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(session.execute("true", None).await.is_ok());
        session.close().await;
    }
}
