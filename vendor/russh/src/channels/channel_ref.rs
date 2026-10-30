use tokio::sync::mpsc::Sender;

use super::WindowSizeRef;
use crate::ChannelMsg;

/// A handle to the [`super::Channel`]'s to be able to transmit messages
/// to it and update it's `window_size`.
#[derive(Debug)]
pub struct ChannelRef {
    pub(super) sender: Sender<ChannelMsg>,
    pub(super) window_size: WindowSizeRef,
    /// The receive window reopens as the reader takes data
    /// ([`super::WindowCredit`]), not as data arrives.
    pub(crate) credited: bool,
}

impl ChannelRef {
    pub fn new(sender: Sender<ChannelMsg>) -> Self {
        Self {
            sender,
            window_size: WindowSizeRef::new(0),
            credited: false,
        }
    }

    /// A channel whose queue never fills: the peer can't send more than the
    /// window, and the window reopens only as the reader takes data.
    pub(crate) fn credited(sender: Sender<ChannelMsg>) -> Self {
        Self {
            credited: true,
            ..Self::new(sender)
        }
    }

    pub(crate) fn window_size(&self) -> &WindowSizeRef {
        &self.window_size
    }
}

impl std::ops::Deref for ChannelRef {
    type Target = Sender<ChannelMsg>;

    fn deref(&self) -> &Self::Target {
        &self.sender
    }
}
