//! Bounded integration events while requests wait for acknowledgments. Export
//! payloads go straight into their bounded receiver, never the deferred queue.
use super::{checked_event, read_event, transport};
use crate::{agent_runtime::protocol::clipboard_export::FRAME, clipboard_export::Receiver};
use std::collections::VecDeque;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::{io::AsyncRead, sync::mpsc, task::JoinHandle};

pub(super) struct Events {
    input: mpsc::Receiver<Result<(u8, Vec<u8>), String>>,
    deferred: VecDeque<(u8, Vec<u8>)>,
    pub(super) exports: Receiver,
}
/// Abandoning a partially written request or an awaited ACK makes subsequent
/// ACK attribution ambiguous. Invalidate results immediately and reconnect.
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
    pub(super) fn request_guard(&self) -> Result<RequestGuard, String> {
        let fence = self.exports.connection_fence();
        if fence.load(Ordering::Acquire) {
            return Err(transport("agent request was interrupted; reconnect"));
        }
        Ok(RequestGuard {
            fence,
            completed: false,
        })
    }
    pub(super) fn new(input: mpsc::Receiver<Result<(u8, Vec<u8>), String>>) -> Self {
        Self {
            input,
            deferred: VecDeque::new(),
            exports: Receiver::default(),
        }
    }
    pub(super) fn start(
        mut reader: impl AsyncRead + Unpin + Send + 'static,
    ) -> (Self, JoinHandle<()>) {
        let (tx, events) = mpsc::channel(8);
        let task = tokio::spawn(async move {
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
    pub(super) async fn next(&mut self) -> Result<(u8, Vec<u8>), String> {
        if self.exports.connection_fence().load(Ordering::Acquire) {
            self.exports.disconnect();
            return Err(transport("agent request was interrupted; reconnect"));
        }
        if let Some(event) = self.deferred.pop_front() {
            return Ok(event);
        }
        self.receive().await
    }
    pub(super) async fn receive(&mut self) -> Result<(u8, Vec<u8>), String> {
        let result = async {
            if self.exports.connection_fence().load(Ordering::Acquire) {
                return Err(transport("agent request was interrupted; reconnect"));
            }
            let (kind, data) = self
                .input
                .recv()
                .await
                .ok_or_else(|| transport("agent exited"))??;
            let (kind, data) = checked_event(kind, data)?;
            if kind == FRAME {
                self.exports.accept(&data)?;
                // The caller observes current metadata or receives completed
                // reads. No payload copy is retained as a notification.
                return Ok((kind, vec![]));
            }
            Ok((kind, data))
        }
        .await;
        if result.is_err() {
            self.exports.disconnect();
        }
        result
    }
    pub(super) fn defer(&mut self, kind: u8, data: Vec<u8>) -> Result<(), String> {
        let count = |kinds: &[u8]| {
            self.deferred
                .iter()
                .filter(|(kind, _)| kinds.contains(kind))
                .count()
        };
        match kind {
            b'C' if count(b"C") < 8 => (),
            b'G' if data.is_empty() => {
                if count(b"G") > 0 {
                    return Ok(());
                }
            }
            b'L' | b'l' | b'K' | b'a' if count(b"LlKa") < 64 => (),
            FRAME if self.exports.enabled() && data.is_empty() => {
                if count(&[FRAME]) > 0 {
                    return Ok(());
                }
            }
            _ => {
                self.exports.disconnect();
                return Err("Unexpected or excessive agent events".into());
            }
        }
        self.deferred.push_back((kind, data));
        Ok(())
    }
}

#[cfg(test)]
#[path = "agent_events_tests.rs"]
mod tests;
