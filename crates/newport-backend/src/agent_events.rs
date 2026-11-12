//! Bounded integration events while requests wait for acknowledgments. Export
//! payloads go straight into their bounded receiver, never the deferred queue.
use super::{Event, Failure, failure, read_event, transport};
use crate::clipboard_export::Receiver;
use newport_protocol::{
    encoding::protobuf::tokio::FrameReader,
    integration::{SERVER_LIMITS, ServerMessage},
};
use std::collections::VecDeque;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::{io::AsyncRead, sync::mpsc, task::JoinHandle};

pub(super) struct Events {
    input: mpsc::Receiver<Result<ServerMessage, Failure>>,
    deferred: VecDeque<Event>,
    pub(super) exports: Receiver,
}
/// An interrupted write or unacknowledged state change has an unknown outcome.
/// Invalidate connection-owned results before another operation can use them.
pub(super) struct RequestGuard {
    fence: Arc<AtomicBool>,
    completed: bool,
}
impl RequestGuard {
    pub(super) fn complete(&mut self) {
        self.completed = true;
    }
}
impl Drop for RequestGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.fence.store(true, Ordering::Release);
        }
    }
}
impl Events {
    pub(super) fn request_guard(&self) -> Result<RequestGuard, Failure> {
        let fence = self.exports.connection_fence();
        if fence.load(Ordering::Acquire) {
            return Err(transport("agent request was interrupted; reconnect"));
        }
        Ok(RequestGuard {
            fence,
            completed: false,
        })
    }
    pub(super) fn new(input: mpsc::Receiver<Result<ServerMessage, Failure>>) -> Self {
        Self {
            input,
            deferred: VecDeque::new(),
            exports: Receiver::default(),
        }
    }
    pub(super) fn start(reader: impl AsyncRead + Unpin + Send + 'static) -> (Self, JoinHandle<()>) {
        let (tx, events) = mpsc::channel(8);
        let task = tokio::spawn(async move {
            let mut reader = FrameReader::new(reader, SERVER_LIMITS);
            loop {
                let event = read_event(&mut reader).await;
                let failed = event.is_err();
                if tx.send(event).await.is_err() || failed {
                    break;
                }
            }
        });
        (Self::new(events), task)
    }
    pub(super) async fn next(&mut self) -> Result<Event, Failure> {
        if self.exports.connection_fence().load(Ordering::Acquire) {
            self.exports.disconnect();
            return Err(transport("agent request was interrupted; reconnect"));
        }
        if let Some(event) = self.deferred.pop_front() {
            return Ok(event);
        }
        self.receive().await
    }
    pub(super) async fn receive(&mut self) -> Result<Event, Failure> {
        let result = async {
            if self.exports.connection_fence().load(Ordering::Acquire) {
                return Err(transport("agent request was interrupted; reconnect"));
            }
            let message = self
                .input
                .recv()
                .await
                .ok_or_else(|| transport("agent exited"))??;
            Ok(match message {
                ServerMessage::Failed(error) => return Err(failure(error)),
                ServerMessage::Export(event) => {
                    self.exports.accept(event)?;
                    // Only metadata is deferred; payloads stay in the bounded receiver.
                    Event::ExportsChanged
                }
                ServerMessage::Response(value) => Event::Response(value),
                ServerMessage::Browser(value) => Event::Browser(value),
                ServerMessage::ClipboardRequest(value) => Event::ClipboardRequest(value),
                ServerMessage::GuiNotice => Event::GuiNotice,
            })
        }
        .await;
        if result.is_err() {
            self.exports.disconnect();
        }
        result
    }
    pub(super) fn defer(&mut self, event: Event) -> Result<(), Failure> {
        match &event {
            Event::ClipboardRequest(_)
                if self
                    .deferred
                    .iter()
                    .filter(|event| matches!(event, Event::ClipboardRequest(_)))
                    .count()
                    < 8 => {}
            Event::GuiNotice => {
                if self.deferred.contains(&Event::GuiNotice) {
                    return Ok(());
                }
            }
            Event::ExportsChanged if self.exports.enabled() => {
                if self.deferred.contains(&Event::ExportsChanged) {
                    return Ok(());
                }
            }
            _ => {
                self.exports.disconnect();
                return Err("Unexpected or excessive agent events".into());
            }
        }
        self.deferred.push_back(event);
        Ok(())
    }
}

#[cfg(test)]
#[path = "agent_events_tests.rs"]
mod tests;
