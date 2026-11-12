//! The one-page HTTP listener on 127.0.0.1 that receives the browser's
//! sign-in redirect. It reads only request heads and answers each connection
//! once, then closes it.
use std::{net::Ipv4Addr, time::Duration};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
    task::JoinSet,
};
use url::Url;

const HEAD_LIMIT: usize = 8 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
// Any local process can connect, so unread connections are bounded to keep
// file descriptors available.
const MAX_CONCURRENT_READS: usize = 32;

pub(super) struct Listener {
    listener: TcpListener,
    port: u16,
    // Browsers open speculative connections that never send a request, so
    // heads are read concurrently rather than one connection at a time.
    reads: JoinSet<Option<Request>>,
}

pub(super) struct Request {
    stream: TcpStream,
    pub target: String,
}

impl Listener {
    pub async fn bind() -> std::io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let port = listener.local_addr()?.port();
        Ok(Self {
            listener,
            port,
            reads: JoinSet::new(),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The next well-formed GET request. Other connections are answered and dropped.
    pub async fn next(&mut self) -> Request {
        loop {
            tokio::select! {
                accepted = self.listener.accept() => {
                    // Beyond the cap the stream is dropped, which closes it.
                    if let Ok((stream, _)) = accepted
                        && self.reads.len() < MAX_CONCURRENT_READS
                    {
                        self.reads.spawn(read_head(stream));
                    }
                }
                Some(read) = self.reads.join_next(), if !self.reads.is_empty() => {
                    if let Ok(Some(request)) = read {
                        return request;
                    }
                }
            }
        }
    }
}

async fn read_head(mut stream: TcpStream) -> Option<Request> {
    let mut head = Vec::with_capacity(1024);
    let read = tokio::time::timeout(READ_TIMEOUT, async {
        let mut chunk = [0u8; 1024];
        while !head.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = stream.read(&mut chunk).await.ok()?;
            if count == 0 || head.len() + count > HEAD_LIMIT {
                return None;
            }
            head.extend_from_slice(&chunk[..count]);
        }
        Some(())
    })
    .await;
    if !matches!(read, Ok(Some(()))) {
        return None;
    }
    let line = std::str::from_utf8(&head)
        .ok()?
        .split("\r\n")
        .next()?
        .to_owned();
    let mut parts = line.split(' ');
    let (method, target) = (parts.next()?, parts.next()?);
    if !target.starts_with('/') {
        return None;
    }
    if method != "GET" {
        respond(stream, "405 Method Not Allowed", &[], "").await;
        return None;
    }
    Some(Request {
        stream,
        target: target.to_owned(),
    })
}

impl Request {
    pub async fn redirect(self, location: &Url) {
        respond(
            self.stream,
            "303 See Other",
            &[("Location", location.as_str())],
            "",
        )
        .await;
    }

    pub async fn not_found(self) {
        respond(self.stream, "404 Not Found", &[], "Not found.").await;
    }

    pub async fn invalid(self) {
        respond(
            self.stream,
            "400 Bad Request",
            &[],
            "This sign-in link isn’t valid. Start again from Newport.",
        )
        .await;
    }
}

async fn respond(mut stream: TcpStream, status: &str, headers: &[(&str, &str)], body: &str) {
    let mut response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str("\r\n");
    response.push_str(body);
    let _ = tokio::time::timeout(WRITE_TIMEOUT, async {
        stream.write_all(response.as_bytes()).await?;
        stream.shutdown().await
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connections_beyond_the_cap_are_closed_and_a_real_request_still_completes() {
        let mut listener = Listener::bind().await.unwrap();
        let addr = (Ipv4Addr::LOCALHOST, listener.port());
        let mut held = Vec::new();
        for _ in 0..MAX_CONCURRENT_READS {
            held.push(TcpStream::connect(addr).await.unwrap());
        }
        // Let the listener take the silent connections into its read set.
        let idle = tokio::time::timeout(Duration::from_millis(300), listener.next()).await;
        assert!(idle.is_err());
        assert_eq!(listener.reads.len(), MAX_CONCURRENT_READS);

        let mut extra = Vec::new();
        for _ in 0..8 {
            extra.push(TcpStream::connect(addr).await.unwrap());
        }
        let idle = tokio::time::timeout(Duration::from_millis(300), listener.next()).await;
        assert!(idle.is_err());
        assert_eq!(listener.reads.len(), MAX_CONCURRENT_READS);
        for mut stream in extra {
            let mut byte = [0u8; 1];
            let read = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut byte)).await;
            assert!(
                matches!(read, Ok(Ok(0) | Err(_))),
                "extra connection left open"
            );
        }

        drop(held);
        let idle = tokio::time::timeout(Duration::from_millis(300), listener.next()).await;
        assert!(idle.is_err());
        assert_eq!(listener.reads.len(), 0);
        let mut client = TcpStream::connect(addr).await.unwrap();
        client
            .write_all(b"GET /callback?code=1 HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let request = tokio::time::timeout(Duration::from_secs(5), listener.next())
            .await
            .unwrap();
        assert_eq!(request.target, "/callback?code=1");
    }
}
