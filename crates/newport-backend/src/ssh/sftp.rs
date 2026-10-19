//! SFTP uses the same authentication and host-key policy as terminals and tunnels.
use super::Connection;
use crate::model::Server;
use anyhow::{Context, Result};
use russh_sftp::client::RawSftpSession;
use std::{
    io,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context as TaskContext, Poll},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// SFTP request IDs multiplex independent file operations on one SSH channel.
/// The connection holds only a Weak reference, so idle operations release it.
pub struct Session {
    raw: RawSftpSession,
    closed: Arc<AtomicBool>,
}
impl std::ops::Deref for Session {
    type Target = RawSftpSession;
    fn deref(&self) -> &Self::Target {
        &self.raw
    }
}
impl Session {
    pub(super) async fn open(connection: &Connection) -> Result<Self> {
        let channel = connection.open_session().await?;
        channel.request_subsystem(true, "sftp").await?;
        let closed = Arc::new(AtomicBool::new(false));
        let raw = RawSftpSession::new(Stream {
            inner: channel.into_stream(),
            closed: closed.clone(),
        });
        raw.init().await.context("Server does not provide SFTP")?;
        Ok(Self { raw, closed })
    }
    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}
struct Stream {
    inner: russh::ChannelStream<russh::client::Msg>,
    closed: Arc<AtomicBool>,
}
impl Drop for Stream {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
    }
}
impl AsyncRead for Stream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let capacity = buf.remaining();
        let result = Pin::new(&mut self.inner).poll_read(cx, buf);
        if matches!(&result, Poll::Ready(Err(_)))
            || matches!(&result, Poll::Ready(Ok(())))
                && capacity > 0
                && buf.filled().len() == before
        {
            self.closed.store(true, Ordering::Release);
        }
        result
    }
}
impl AsyncWrite for Stream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.inner).poll_write(cx, bytes);
        if matches!(&result, Poll::Ready(Err(_))) {
            self.closed.store(true, Ordering::Release);
        }
        result
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

pub struct Sftp {
    pub session: Arc<Session>,
    /// Saved connection identity without the transient connection revision.
    /// Failed staging cleanup can be retried after reconnecting, but never on
    /// a different endpoint, user, authentication profile or server record.
    cleanup_scope: serde_json::Value,
    server: uuid::Uuid,
    _connection: Option<Connection>,
    _lease: Option<crate::server_session::Lease>,
}
impl Sftp {
    /// Reuse the application's authenticated transport for background VFS work.
    pub(crate) async fn shared(
        owner: &std::sync::Arc<crate::server_session::ServerSession>,
    ) -> Result<Self> {
        let server = owner.server();
        let lease = owner.acquire(server).await.map_err(anyhow::Error::msg)?;
        let session = tokio::time::timeout(Duration::from_secs(20), lease.sftp())
            .await
            .context("SFTP setup timed out")??;
        Ok(Self {
            session,
            server: server.id,
            cleanup_scope: newport_domain::connection_scope::connection(
                &serde_json::to_value(server)?,
                &serde_json::Value::Null,
            ),
            _connection: None,
            _lease: Some(lease),
        })
    }
    pub(crate) fn is_stale(&self) -> bool {
        self.session.is_closed() || self._lease.as_ref().is_some_and(|lease| lease.is_closed())
    }
    pub fn server_id(&self) -> uuid::Uuid {
        self.server
    }
    pub fn cleanup_scope(&self) -> &serde_json::Value {
        &self.cleanup_scope
    }
    pub async fn connect(server: &Server) -> Result<Self> {
        let connection = Connection::connect(server).await?;
        let session = tokio::time::timeout(Duration::from_secs(20), Session::open(&connection))
            .await
            .context("SFTP setup timed out")??;
        Ok(Self {
            session: Arc::new(session),
            server: server.id,
            cleanup_scope: newport_domain::connection_scope::connection(
                &serde_json::to_value(server)?,
                &serde_json::Value::Null,
            ),
            _connection: Some(connection),
            _lease: None,
        })
    }
}
