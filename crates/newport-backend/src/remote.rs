//! Commands and SFTP over a server's link, each on channels of their own.
pub mod command;
pub mod sftp;

pub use sftp::Sftp;

use newport_ssh::{Error, Fate};
use std::time::Duration;

/// How long a channel that ended early waits to learn whether its
/// connection was lost, which the link reports just after the channel ends.
const LOST_GRACE: Duration = Duration::from_millis(250);

/// Why a channel ended early, when it was its connection's loss: the link
/// reports that just after the channel ends.
pub(crate) async fn lost(fate: &Fate) -> Option<Error> {
    tokio::time::timeout(LOST_GRACE, fate.lost()).await.ok()
}

/// A request's failure as the app receives it: its text, and the link
/// error's code when the server couldn't be reached, for the app to word.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Failure {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<&'static str>,
    pub message: String,
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self {
            code: Some(error.code()),
            message: error.to_string(),
        }
    }
}

impl From<command::Error> for Failure {
    fn from(error: command::Error) -> Self {
        match error {
            command::Error::Link(error) => error.into(),
            error => error.to_string().into(),
        }
    }
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self {
            code: None,
            message,
        }
    }
}

impl From<Failure> for String {
    fn from(failure: Failure) -> Self {
        failure.message
    }
}

/// A request on a channel that couldn't be sent: its channel or connection
/// has closed.
fn unsent(error: impl std::fmt::Display) -> Error {
    Error::Lost(error.to_string())
}

/// Commands on a server's link, for fixture tests that drive the server
/// directly.
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct Shell(pub newport_ssh::Link);

#[cfg(test)]
impl Shell {
    pub(crate) fn new(server: &crate::model::Server) -> Self {
        Self(crate::ssh::link(server, crate::ssh::no_jump_hosts))
    }
    pub(crate) async fn execute(
        &self,
        command: &str,
        input: Option<&[u8]>,
    ) -> Result<String, String> {
        Ok(command::stdout(&self.0, command, input).await?)
    }
    pub(crate) async fn stream(
        &self,
        command: &str,
    ) -> Result<russh::ChannelStream<russh::client::Msg>, String> {
        Ok(command::stream(&self.0, command).await?.into_stream())
    }
}
