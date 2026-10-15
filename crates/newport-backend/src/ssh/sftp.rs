//! SFTP uses the same authentication and host-key policy as terminals and tunnels.
use super::Connection;
use crate::model::Server;
use anyhow::{Context, Result};
use russh_sftp::client::RawSftpSession;
use std::time::Duration;

pub struct Sftp {
    pub session: RawSftpSession,
    /// Saved connection identity without the transient connection revision.
    /// Failed staging cleanup can be retried after reconnecting, but never on
    /// a different endpoint, user, authentication profile or server record.
    cleanup_scope: serde_json::Value,
    server: uuid::Uuid,
    _connection: Connection,
}
impl Sftp {
    pub fn server_id(&self) -> uuid::Uuid {
        self.server
    }
    pub fn cleanup_scope(&self) -> &serde_json::Value {
        &self.cleanup_scope
    }
    pub async fn connect(server: &Server) -> Result<Self> {
        let connection = Connection::connect(server).await?;
        let session = tokio::time::timeout(Duration::from_secs(20), async {
            let channel = connection.handle.channel_open_session().await?;
            channel.request_subsystem(true, "sftp").await?;
            let session = RawSftpSession::new(channel.into_stream());
            session
                .init()
                .await
                .context("Server does not provide SFTP")?;
            Ok::<_, anyhow::Error>(session)
        })
        .await
        .context("SFTP setup timed out")??;
        Ok(Self {
            session,
            server: server.id,
            cleanup_scope: newport_domain::connection_scope::connection(
                &serde_json::to_value(server)?,
                &serde_json::Value::Null,
            ),
            _connection: connection,
        })
    }
}
