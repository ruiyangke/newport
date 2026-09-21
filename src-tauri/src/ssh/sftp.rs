//! SFTP uses the same authentication and host-key policy as terminals and tunnels.
use super::Connection;
use crate::model::Server;
use anyhow::{Context, Result};
use russh_sftp::client::RawSftpSession;
use std::time::Duration;

pub struct Sftp {
    pub session: RawSftpSession,
    _connection: Connection,
}
impl Sftp {
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
            _connection: connection,
        })
    }
}
