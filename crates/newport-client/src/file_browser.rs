//! Scoped Files reads, independent of rendering and media preparation.
//! Each slot retains one physical read and only its newest desired replacement.
use crate::{backend::Backend, desktop_inputs, file_result};
use newport_protocol::desktop::{self, files};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::{sync::mpsc, thread, time::Duration};

const READ_TIMEOUT: Duration = Duration::from_secs(60);
const CANCEL_TIMEOUT: Duration = Duration::from_secs(10);
type Reply = Result<Value, String>;

#[derive(Clone, Debug, PartialEq)]
pub struct Preview {
    /// Selection identity, including repeated selections of the same path.
    pub token: u64,
    pub path: String,
    pub data: Option<Value>,
    pub error: Option<String>,
    pub pending: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Update {
    pub changed: bool,
    /// Only a successful listing with a different canonical folder sets this.
    pub folder_changed: bool,
    pub listing_changed: bool,
    /// A reply accepted for the current preview, including a current error.
    /// Opening/closing a selection is immediately reflected in the getters.
    pub preview_changed: bool,
}

pub struct Browser {
    backend: Backend,
    server: String,
    connection: Value,
    retired: bool,
    changed: bool,
    listing: Option<Value>,
    attempted: Option<String>,
    canonical: Option<String>,
    listing_pending: bool,
    listing_error: Option<String>,
    listing_revision: u64,
    listing_data_revision: u64,
    listing_token: u64,
    listing_reads: ReadSlot,
    preview: Option<Preview>,
    preview_token: u64,
    preview_reads: ReadSlot,
}
impl Browser {
    /// Construction never dispatches a request. The host decides when to read.
    pub fn new(backend: Backend, server: String, connection: Value) -> Self {
        Self {
            backend,
            server,
            connection,
            retired: false,
            changed: false,
            listing: None,
            attempted: None,
            canonical: None,
            listing_pending: false,
            listing_error: None,
            listing_revision: 0,
            listing_data_revision: 0,
            listing_token: 0,
            listing_reads: ReadSlot::default(),
            preview: None,
            preview_token: 0,
            preview_reads: ReadSlot::default(),
        }
    }
    pub fn server(&self) -> &str {
        &self.server
    }
    pub fn connection(&self) -> &Value {
        &self.connection
    }
    pub fn retired(&self) -> bool {
        self.retired
    }
    pub fn listing(&self) -> Option<&Value> {
        self.listing.as_ref()
    }
    pub fn attempted_path(&self) -> Option<&str> {
        self.attempted.as_deref()
    }
    pub fn canonical_path(&self) -> Option<&str> {
        self.canonical.as_deref()
    }
    pub fn listing_pending(&self) -> bool {
        self.listing_pending
    }
    pub fn listing_error(&self) -> Option<&str> {
        self.listing_error.as_deref()
    }
    pub fn listing_revision(&self) -> u64 {
        self.listing_revision
    }
    /// Stable identity for filtering/sorting projections. Request completion
    /// still advances `listing_revision` to fence callbacks from older renders.
    pub fn listing_data_revision(&self) -> u64 {
        self.listing_data_revision
    }
    pub fn preview(&self) -> Option<&Preview> {
        self.preview.as_ref()
    }
    /// Includes cancelled requests still draining, even after preview closes.
    pub fn has_pending(&self) -> bool {
        self.listing_reads.pending() || self.preview_reads.pending()
    }

    pub fn navigate(&mut self, path: String) -> bool {
        self.navigate_with_refresh(path, false)
    }
    fn navigate_with_refresh(&mut self, path: String, refresh: bool) -> bool {
        if self.retired {
            return false;
        }
        self.close_preview();
        self.listing_token = next(self.listing_token);
        self.attempted = Some(path.clone());
        self.listing_pending = true;
        self.listing_error = None;
        self.listing_reads.replace(
            &self.backend,
            Intent {
                token: self.listing_token,
                path,
                refresh,
            },
        );
        self.launch();
        self.changed = true;
        true
    }
    /// Refresh the displayed folder; retry instead targets a failed attempt.
    pub fn refresh(&mut self) -> bool {
        self.canonical
            .clone()
            .or_else(|| self.attempted.clone())
            .is_some_and(|path| self.navigate_with_refresh(path, true))
    }
    pub fn retry(&mut self) -> bool {
        self.attempted
            .clone()
            .is_some_and(|path| self.navigate_with_refresh(path, true))
    }
    pub fn open_preview(&mut self, path: String) -> Option<u64> {
        if self.retired {
            return None;
        }
        self.preview_token = next(self.preview_token);
        let token = self.preview_token;
        self.preview = Some(Preview {
            token,
            path: path.clone(),
            data: None,
            error: None,
            pending: true,
        });
        self.preview_reads.replace(
            &self.backend,
            Intent {
                token,
                path,
                refresh: true,
            },
        );
        self.launch();
        self.changed = true;
        Some(token)
    }
    pub fn retry_preview(&mut self) -> Option<u64> {
        let path = self.preview.as_ref()?.path.clone();
        self.open_preview(path)
    }
    pub fn close_preview(&mut self) {
        if self.preview.take().is_some() {
            self.changed = true;
        }
        self.preview_token = next(self.preview_token);
        self.preview_reads.clear(&self.backend);
    }
    /// Move a large raw payload into media preparation once, without changing
    /// the selection's successful state or implicitly refetching it.
    pub fn take_preview_data(&mut self, token: u64) -> Option<Value> {
        self.preview
            .as_mut()
            .filter(|preview| preview.token == token && !preview.pending)?
            .data
            .take()
    }
    /// Leave the page while preserving its last successful folder. The same
    /// scope may later call navigate/refresh to resume; no read restarts itself.
    pub fn suspend(&mut self) {
        self.close_preview();
        self.listing_token = next(self.listing_token);
        self.listing_reads.clear(&self.backend);
        self.listing_pending = false;
        self.changed = true;
    }
    pub fn retire(&mut self) {
        if !self.retired {
            self.suspend();
            self.retired = true;
        }
    }
    pub fn poll(&mut self) -> Update {
        let mut update = Update {
            changed: std::mem::take(&mut self.changed),
            ..Default::default()
        };
        let listing = self.listing_reads.poll(&self.backend, &mut update.changed);
        if !self.retired
            && let Some((intent, result)) = listing
            && intent.token == self.listing_token
            && self.listing_pending
        {
            self.listing_pending = false;
            match result.and_then(valid_listing) {
                Ok(value) => {
                    let canonical = value["path"].as_str().unwrap().to_owned();
                    update.folder_changed = self.canonical.as_ref() != Some(&canonical);
                    self.canonical = Some(canonical);
                    self.listing_revision = next(self.listing_revision);
                    if self.listing.as_ref() != Some(&value) {
                        self.listing = Some(value);
                        self.listing_data_revision = self.listing_revision;
                    }
                    self.listing_error = None;
                    update.listing_changed = true;
                }
                Err(error) => self.listing_error = Some(error),
            }
        }
        let preview = self.preview_reads.poll(&self.backend, &mut update.changed);
        if !self.retired
            && let Some((intent, result)) = preview
            && let Some(preview) = self
                .preview
                .as_mut()
                .filter(|preview| preview.token == intent.token)
        {
            preview.pending = false;
            match result.and_then(valid_preview) {
                Ok(value) => {
                    preview.data = Some(value);
                    preview.error = None;
                }
                Err(error) => preview.error = Some(error),
            }
            update.preview_changed = true;
        }
        self.launch();
        update
    }
    fn launch(&mut self) {
        if self.retired {
            return;
        }
        self.listing_reads
            .launch(&self.backend, &self.server, &self.connection, false);
        self.preview_reads
            .launch(&self.backend, &self.server, &self.connection, true);
    }
}
impl Drop for Browser {
    fn drop(&mut self) {
        self.listing_reads.clear(&self.backend);
        self.preview_reads.clear(&self.backend);
    }
}

#[derive(Clone)]
struct Intent {
    refresh: bool,
    token: u64,
    path: String,
}
struct Work {
    intent: Intent,
    operation: uuid::Uuid,
    reply: Option<mpsc::Receiver<Reply>>,
    result: Option<Reply>,
    cancel: Option<mpsc::Receiver<Reply>>,
    cancel_sent: bool,
}
impl Work {
    fn cancel(&mut self, backend: &Backend) {
        if self.cancel_sent {
            return;
        }
        self.cancel_sent = true;
        self.cancel = Some(dispatch(
            backend,
            Ok(files::Operation::Cancel(Box::new(files::CancelArgs {
                operation: self.operation,
            }))),
            CANCEL_TIMEOUT,
        ));
    }
}
#[derive(Default)]
struct ReadSlot {
    active: Option<Work>,
    desired: Option<Intent>,
}
impl ReadSlot {
    fn pending(&self) -> bool {
        self.active.is_some() || self.desired.is_some()
    }
    fn replace(&mut self, backend: &Backend, intent: Intent) {
        if let Some(active) = &mut self.active {
            active.cancel(backend);
        }
        self.desired = Some(intent);
    }
    fn clear(&mut self, backend: &Backend) {
        self.desired = None;
        if let Some(active) = &mut self.active {
            active.cancel(backend);
        }
    }
    fn launch(&mut self, backend: &Backend, server: &str, connection: &Value, preview: bool) {
        if self.active.is_some() {
            return;
        }
        let Some(intent) = self.desired.take() else {
            return;
        };
        let operation = uuid::Uuid::new_v4();
        let request = (|| {
            let server = desktop_inputs::identity(server, "file server identity")?;
            let expected = Some(desktop_inputs::connection_scope(connection)?);
            Ok(if preview {
                files::Operation::Preview(Box::new(files::PreviewArgs {
                    server,
                    operation,
                    path: intent.path.clone(),
                    expected,
                }))
            } else {
                files::Operation::List(Box::new(files::ListArgs {
                    server,
                    operation,
                    path: intent.path.clone(),
                    expected,
                    refresh: intent.refresh,
                    project_root: None,
                }))
            })
        })();
        let reply = dispatch(backend, request, READ_TIMEOUT);
        self.active = Some(Work {
            intent,
            operation,
            reply: Some(reply),
            result: None,
            cancel: None,
            cancel_sent: false,
        });
    }
    fn poll(&mut self, backend: &Backend, changed: &mut bool) -> Option<(Intent, Reply)> {
        let active = self.active.as_mut()?;
        if let Some(result) = active.reply.as_ref().and_then(ready) {
            active.reply = None;
            // A transport failure may leave its server-side read running.
            // Cancellation is best effort; the read itself is never replayed.
            if result.is_err() {
                active.cancel(backend);
            }
            active.result = Some(result);
            *changed = true;
        }
        if active.cancel.as_ref().and_then(ready).is_some() {
            active.cancel = None;
            *changed = true;
        }
        // Deliver the authoritative read once, without waiting for ancillary
        // cancellation. Keep the slot occupied until both workers drain so
        // rapid supersessions cannot accumulate cancellation requests.
        if active.reply.is_none() && active.cancel.is_none() {
            let active = self.active.take().unwrap();
            return active.result.map(|result| (active.intent, result));
        }
        active
            .result
            .take()
            .map(|result| (active.intent.clone(), result))
    }
}
fn next(value: u64) -> u64 {
    value.wrapping_add(1).max(1)
}
fn dispatch(
    backend: &Backend,
    operation: Result<files::Operation, desktop_inputs::Invalid>,
    timeout: Duration,
) -> mpsc::Receiver<Reply> {
    let backend = backend.clone();
    let (send, receive) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let result = operation
            .map_err(|error| error.to_string())
            .and_then(|operation| {
                backend
                    .execute_with_timeout(desktop::Operation::Files(Box::new(operation)), timeout)
                    .map_err(|error| error.to_string())
            })
            .and_then(|reply| match reply {
                desktop::Reply::Files(value) => file_result::project(*value),
                _ => Err("Unexpected file response".into()),
            });
        let _ = send.send(result);
    });
    receive
}
fn ready(receiver: &mpsc::Receiver<Reply>) -> Option<Reply> {
    match receiver.try_recv() {
        Ok(result) => Some(result),
        Err(mpsc::TryRecvError::Empty) => None,
        Err(mpsc::TryRecvError::Disconnected) => Some(Err(
            "File read worker stopped before returning a result.".into(),
        )),
    }
}
fn valid_listing(value: Value) -> Reply {
    if value["path"]
        .as_str()
        .is_some_and(|path| !path.is_empty() && path.len() <= 16_384 && !path.contains('\0'))
        && value["entries"]
            .as_array()
            .is_some_and(|entries| entries.len() <= 20_000)
    {
        Ok(value)
    } else {
        Err("Invalid folder listing from the backend.".into())
    }
}
fn valid_preview(value: Value) -> Reply {
    let supported = (value["kind"] == "text" && value["mime"] == "text/plain")
        || (value["kind"] == "image"
            && matches!(
                value["mime"].as_str(),
                Some("image/png" | "image/jpeg" | "image/gif" | "image/webp")
            ));
    if supported && value["content"].is_string() {
        Ok(value)
    } else {
        Err("Invalid file preview from the backend.".into())
    }
}

#[cfg(test)]
mod tests;
