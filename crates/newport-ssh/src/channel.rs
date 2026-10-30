use crate::Error;
use russh::client;
use std::ops::{Deref, DerefMut};
use tokio::sync::watch;

type SshChannel = russh::Channel<client::Msg>;

/// A russh channel handed to a consumer, with the fate of its connection.
/// Dropping it closes the channel; its session slot is free once the server
/// has closed it too.
pub struct Channel {
    inner: Option<SshChannel>,
    connection: u64,
    lost: watch::Receiver<Option<Error>>,
    /// Where the close runs when the channel is dropped on another thread.
    runtime: tokio::runtime::Handle,
}

impl Channel {
    pub(crate) fn new(
        inner: SshChannel,
        connection: u64,
        lost: watch::Receiver<Option<Error>>,
    ) -> Self {
        Self {
            inner: Some(inner),
            connection,
            lost,
            runtime: tokio::runtime::Handle::current(),
        }
    }

    /// Which of the link's connections carries this channel.
    pub fn connection(&self) -> u64 {
        self.connection
    }

    /// The channel's data as a byte stream. Dropping the stream closes the
    /// channel.
    pub fn into_stream(mut self) -> russh::ChannelStream<client::Msg> {
        self.take().into_stream()
    }

    /// The channel's halves, for a consumer that reads and writes from
    /// different tasks. Closing the write half closes the channel.
    pub fn split(mut self) -> (russh::ChannelReadHalf, russh::ChannelWriteHalf<client::Msg>) {
        self.take().split()
    }

    /// Resolves when the connection under this channel closes, with why.
    pub async fn lost(&self) -> Error {
        self.fate().lost().await
    }

    /// The connection's fate, kept after the channel becomes a stream.
    pub fn fate(&self) -> Fate {
        Fate {
            connection: self.connection,
            lost: self.lost.clone(),
        }
    }

    fn take(&mut self) -> SshChannel {
        self.inner
            .take()
            .expect("a channel holds its russh channel until dropped")
    }
}

/// Whether the connection under a channel is still open, for a consumer that
/// turned the channel into a stream.
#[derive(Clone)]
pub struct Fate {
    connection: u64,
    lost: watch::Receiver<Option<Error>>,
}

impl Fate {
    /// Which of the link's connections carried the channel.
    pub fn connection(&self) -> u64 {
        self.connection
    }

    /// Why the connection closed, once it has.
    pub fn error(&self) -> Option<Error> {
        if self.lost.has_changed().is_err() && self.lost.borrow().is_none() {
            return Some(Error::Closed);
        }
        self.lost.borrow().clone()
    }

    /// Resolves when the connection closes, with why.
    pub async fn lost(&self) -> Error {
        let mut lost = self.lost.clone();
        match lost.wait_for(Option::is_some).await {
            Ok(error) => error.clone().unwrap_or(Error::Closed),
            Err(_) => Error::Closed,
        }
    }
}

impl Deref for Channel {
    type Target = SshChannel;
    fn deref(&self) -> &SshChannel {
        self.inner
            .as_ref()
            .expect("a channel holds its russh channel until dropped")
    }
}

impl DerefMut for Channel {
    fn deref_mut(&mut self) -> &mut SshChannel {
        self.inner
            .as_mut()
            .expect("a channel holds its russh channel until dropped")
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        let Some(inner) = self.inner.take() else {
            return;
        };
        self.runtime.spawn(async move {
            let _ = inner.close().await;
        });
    }
}
