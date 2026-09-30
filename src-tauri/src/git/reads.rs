//! Local cancellation registration precedes request dispatch, so a fast abort
//! cannot race ahead of native request registration. No tombstones are needed.
use super::protocol::Error;
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};
use tokio::sync::watch;
use uuid::Uuid;

#[derive(Default)]
pub(super) struct Reads(Mutex<HashMap<(Uuid, Uuid), Entry>>);
struct Entry {
    signal: watch::Sender<bool>,
    claimed: bool,
    created: Instant,
}
pub(super) struct Read<'a> {
    registry: &'a Reads,
    key: (Uuid, Uuid),
    signal: watch::Receiver<bool>,
}
pub(super) fn cancelled_error() -> Error {
    Error::new("READ_CANCELLED", "Git read was cancelled.")
}
impl Reads {
    pub(super) fn register(&self, server: Uuid) -> Result<Uuid, Error> {
        let mut reads = self.0.lock().expect("Git read registry");
        // Abandoned registrations (e.g. a reloaded webview) never grow forever.
        reads.retain(|_, entry| entry.claimed || entry.created.elapsed() < Duration::from_secs(60));
        if reads.len() >= 256 {
            return Err(Error::new(
                "TOO_MANY_REQUESTS",
                "Too many pending Git reads.",
            ));
        }
        let id = Uuid::new_v4();
        let (signal, _) = watch::channel(false);
        reads.insert(
            (server, id),
            Entry {
                signal,
                claimed: false,
                created: Instant::now(),
            },
        );
        Ok(id)
    }
    pub(super) fn claim(&self, server: Uuid, id: Uuid) -> Result<Read<'_>, Error> {
        let mut reads = self.0.lock().expect("Git read registry");
        let entry = reads.get_mut(&(server, id)).ok_or_else(cancelled_error)?;
        if entry.claimed {
            return Err(Error::invalid("Git read ID was already used."));
        }
        entry.claimed = true;
        Ok(Read {
            registry: self,
            key: (server, id),
            signal: entry.signal.subscribe(),
        })
    }
    pub(super) fn cancel(&self, server: Uuid, id: Uuid) {
        if let Some(entry) = self
            .0
            .lock()
            .expect("Git read registry")
            .remove(&(server, id))
        {
            entry.signal.send_replace(true);
        }
    }
}
impl Drop for Read<'_> {
    fn drop(&mut self) {
        self.registry
            .0
            .lock()
            .expect("Git read registry")
            .remove(&self.key);
    }
}
pub(super) async fn cancelled(read: &mut Option<Read<'_>>) {
    match read {
        Some(read) => {
            if !*read.signal.borrow() {
                let _ = read.signal.changed().await;
            }
        }
        None => std::future::pending().await,
    }
}
/// A cancelled request must retire its partially consumed protocol stream.
/// This deliberately does not receive the shared connection, so it cannot
/// disconnect another lane while releasing a cancelled read.
pub(super) async fn response<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    slot: &mut Option<super::Client<S>>,
    request: super::Request,
    deadline: Duration,
    session_cancel: &mut watch::Receiver<bool>,
    read: &mut Option<Read<'_>>,
) -> Result<serde_json::Value, Error> {
    let mutation = super::is_mutation(&request);
    let client = slot.as_mut().expect("initialized Git client");
    let mut result = tokio::select! {
        biased;
        _ = cancelled(read) => Err(cancelled_error()),
        _ = session_cancel.changed() => Err(Error::new("CANCELLED", "Git session was disconnected.")),
        result = tokio::time::timeout(deadline, client.request(request)) =>
            result.unwrap_or_else(|_| Err(Error::transport("Git request timed out. Reconnect before continuing."))),
    };
    if !mutation {
        if let Err(error) = &mut result {
            if matches!(error.code.as_str(), "TRANSPORT_ERROR" | "PROTOCOL_ERROR") {
                // This stream is no longer reusable, but its SSH connection
                // may still be serving other read lanes successfully.
                error.code = "READ_CHANNEL_ERROR".into();
                error.message = "The Git read channel was interrupted. Retry this read.".into();
                *slot = None;
            }
        }
    }
    if result
        .as_ref()
        .is_err_and(|error| error.code == "READ_CANCELLED")
    {
        *slot = None;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cancellation_is_scoped_and_registration_cannot_race_or_replay() {
        let reads = Reads::default();
        let server = Uuid::new_v4();
        let a = reads.register(server).unwrap();
        let b = reads.register(server).unwrap();
        reads.cancel(Uuid::new_v4(), a);
        let mut first = Some(reads.claim(server, a).unwrap());
        assert!(reads.claim(server, a).is_err());
        let mut second = Some(reads.claim(server, b).unwrap());
        reads.cancel(server, a);
        tokio::time::timeout(Duration::from_millis(50), cancelled(&mut first))
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(5), cancelled(&mut second))
                .await
                .is_err()
        );
        drop(second);
        assert!(reads.claim(server, b).is_err());
        let early = reads.register(server).unwrap();
        reads.cancel(server, early);
        assert!(reads.claim(server, early).is_err());
        assert!(reads.0.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn repeated_search_cancellation_does_not_exhaust_registration_capacity() {
        let registry = Reads::default();
        let server = Uuid::new_v4();
        for turn in 0..1000 {
            let id = registry.register(server).unwrap();
            if turn % 2 == 0 {
                let mut read = Some(registry.claim(server, id).unwrap());
                registry.cancel(server, id);
                cancelled(&mut read).await;
                drop(read);
            } else {
                registry.cancel(server, id);
                assert!(registry.claim(server, id).is_err());
            }
        }
        assert!(registry.0.lock().unwrap().is_empty());
    }

    #[test]
    fn abandoned_registrations_are_bounded_and_expire() {
        let reads = Reads::default();
        let server = Uuid::new_v4();
        for _ in 0..256 {
            reads.register(server).unwrap();
        }
        assert!(reads.register(server).is_err());
        for entry in reads.0.lock().unwrap().values_mut() {
            entry.created = Instant::now() - Duration::from_secs(61);
        }
        assert!(reads.register(server).is_ok());
        assert_eq!(reads.0.lock().unwrap().len(), 1);
    }
}
