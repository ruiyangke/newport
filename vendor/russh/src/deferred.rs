//! Outbound messages held back for channels whose earlier output still waits
//! on the peer's window.
//!
//! A channel whose data went beyond the peer's window keeps it queued
//! (`ChannelParams::pending_data`) until a `CHANNEL_WINDOW_ADJUST` admits it.
//! The session loop keeps taking outbound messages meanwhile: those for that
//! channel wait here, in order, behind its queued data, and everything else
//! goes out.
//!
//! Channel writers wait for the window before they send, so on their own they
//! never get a channel this far; it takes a write that bypasses the window
//! (`Handle::data`, `Session::data`). The window bounds what a writer adds
//! here. Nothing bounds such a bypassing caller, so once one channel holds
//! [`DEFERRED_BYTES`] or [`DEFERRED_MESSAGES`], the session stops taking
//! outbound messages until the window lets that channel drain, which pushes
//! back on every sender as the session did before.

use std::collections::{HashMap, VecDeque};

use crate::{ChannelId, ChannelMsg};

/// Data bytes one channel may hold back before the session stops taking
/// outbound messages.
pub(crate) const DEFERRED_BYTES: usize = 2 * 1024 * 1024;

/// Messages one channel may hold back before the session stops taking
/// outbound messages.
pub(crate) const DEFERRED_MESSAGES: usize = 1024;

/// The messages each channel holds back, in the order they were sent.
#[derive(Debug)]
pub(crate) struct Deferred<M> {
    channels: HashMap<ChannelId, Held<M>>,
}

#[derive(Debug)]
struct Held<M> {
    /// Each message with the data bytes it carries.
    messages: VecDeque<(M, usize)>,
    bytes: usize,
}

impl<M> Default for Deferred<M> {
    fn default() -> Self {
        Self {
            channels: HashMap::new(),
        }
    }
}

impl<M> Deferred<M> {
    /// Does `channel` hold messages back?
    pub(crate) fn holds(&self, channel: ChannelId) -> bool {
        self.channels.contains_key(&channel)
    }

    /// Hold `message` back behind the channel's earlier output.
    pub(crate) fn push(&mut self, channel: ChannelId, message: M, bytes: usize) {
        let held = self.channels.entry(channel).or_insert_with(|| Held {
            messages: VecDeque::new(),
            bytes: 0,
        });
        held.messages.push_back((message, bytes));
        held.bytes = held.bytes.saturating_add(bytes);
    }

    /// The channel's oldest held message.
    pub(crate) fn pop(&mut self, channel: ChannelId) -> Option<M> {
        let held = self.channels.get_mut(&channel)?;
        let (message, bytes) = held.messages.pop_front()?;
        held.bytes = held.bytes.saturating_sub(bytes);
        if held.messages.is_empty() {
            self.channels.remove(&channel);
        }
        Some(message)
    }

    /// Channels holding messages back.
    pub(crate) fn channels(&self) -> Vec<ChannelId> {
        self.channels.keys().copied().collect()
    }

    /// Has a channel reached its limit? Then the session takes no outbound
    /// messages until it drains.
    pub(crate) fn full(&self) -> bool {
        self.channels
            .values()
            .any(|held| held.bytes >= DEFERRED_BYTES || held.messages.len() >= DEFERRED_MESSAGES)
    }
}

/// The data bytes a channel message carries.
pub(crate) fn data_bytes(message: &ChannelMsg) -> usize {
    match message {
        ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => data.len(),
        _ => 0,
    }
}

/// The other end of a session loop under test, over cleartext packets.
#[cfg(test)]
pub(crate) mod peer {
    use std::time::Duration;

    use ssh_encoding::Encode;
    use tokio::io::DuplexStream;

    use crate::sshbuffer::{PacketWriter, SSHBuffer};
    use crate::{cipher, msg};

    pub(crate) struct Peer {
        stream: DuplexStream,
        buffer: SSHBuffer,
        writer: PacketWriter,
    }

    impl Peer {
        /// A peer, and the stream for the session loop.
        pub(crate) fn new() -> (Self, DuplexStream) {
            let (stream, session) = tokio::io::duplex(1 << 20);
            let peer = Self {
                stream,
                buffer: SSHBuffer::new(),
                writer: PacketWriter::clear(),
            };
            (peer, session)
        }

        /// The payload of the next packet, if one arrives within `wait`.
        pub(crate) async fn next_within(&mut self, wait: Duration) -> Option<Vec<u8>> {
            let mut key = cipher::clear::Key;
            let read = cipher::read(&mut self.stream, &mut self.buffer, &mut key);
            match tokio::time::timeout(wait, read).await {
                Ok(read) => {
                    read.unwrap();
                    Some(self.buffer.buffer[5..].to_vec())
                }
                Err(_) => None,
            }
        }

        /// The payload of the next packet.
        pub(crate) async fn next(&mut self) -> Vec<u8> {
            self.next_within(Duration::from_secs(5))
                .await
                .expect("a packet from the session")
        }

        /// Fails if the session sends anything soon.
        pub(crate) async fn assert_quiet(&mut self) {
            if let Some(payload) = self.next_within(Duration::from_millis(200)).await {
                panic!("unexpected packet {payload:?}");
            }
        }

        pub(crate) async fn send(&mut self, payload: Vec<u8>) {
            self.writer
                .write_packet(|packet| {
                    packet.extend_from_slice(&payload);
                    Ok(())
                })
                .unwrap();
            self.writer.flush_into(&mut self.stream).await.unwrap();
        }
    }

    pub(crate) fn data(recipient: u32, data: &[u8]) -> Vec<u8> {
        let mut payload = vec![msg::CHANNEL_DATA];
        recipient.encode(&mut payload).unwrap();
        data.encode(&mut payload).unwrap();
        payload
    }

    pub(crate) fn extended_data(recipient: u32, ext: u32, data: &[u8]) -> Vec<u8> {
        let mut payload = vec![msg::CHANNEL_EXTENDED_DATA];
        recipient.encode(&mut payload).unwrap();
        ext.encode(&mut payload).unwrap();
        data.encode(&mut payload).unwrap();
        payload
    }

    pub(crate) fn eof(recipient: u32) -> Vec<u8> {
        let mut payload = vec![msg::CHANNEL_EOF];
        recipient.encode(&mut payload).unwrap();
        payload
    }

    pub(crate) fn close(recipient: u32) -> Vec<u8> {
        let mut payload = vec![msg::CHANNEL_CLOSE];
        recipient.encode(&mut payload).unwrap();
        payload
    }

    pub(crate) fn window_adjust(recipient: u32, bytes: u32) -> Vec<u8> {
        let mut payload = vec![msg::CHANNEL_WINDOW_ADJUST];
        recipient.encode(&mut payload).unwrap();
        bytes.encode(&mut payload).unwrap();
        payload
    }
}
