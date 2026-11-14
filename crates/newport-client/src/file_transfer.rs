//! One retained file transfer on an explicitly captured desktop backend.
//! The original write is never replayed. Cancellation acknowledges a request,
//! while only the original transfer reply establishes its outcome.
use crate::{backend::Backend, desktop_inputs};
use newport_protocol::desktop::{self, configuration::ConnectionScope, files};
use serde::Deserialize;
#[cfg(test)]
use serde_json::json;
use std::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const PROGRESS_INTERVAL: Duration = Duration::from_millis(500);
const PROGRESS_TIMEOUT: Duration = Duration::from_secs(2);
const CANCEL_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_PROGRESS_NAME: usize = 4096;
const MAX_CANCEL_ERROR: usize = 4096;
type Reply = Result<files::Reply, String>;
/// The transferred file's path, or why the transfer failed.
pub type Completion = Result<Destination, String>;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Destination {
    Remote(String),
    Local(std::path::PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Download,
    Upload,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Progress {
    pub name: String,
    pub completed: u64,
    pub total: Option<u64>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Update {
    pub changed: bool,
    /// Emitted once. Chooser cancellation happens before a Transfer is created.
    pub completion: Option<Completion>,
}

pub struct Transfer {
    backend: Backend,
    connection: Option<ConnectionScope>,
    operation: uuid::Uuid,
    server: String,
    path: String,
    name: String,
    direction: Direction,
    result: mpsc::Receiver<Reply>,
    completion: Option<Completion>,
    finished: bool,
    retired: bool,
    progress: Option<Progress>,
    progress_reply: Option<mpsc::Receiver<Reply>>,
    last_progress: Instant,
    cancel_reply: Option<mpsc::Receiver<Reply>>,
    cancel_requested: bool,
    cancel_error: Option<String>,
}
impl Transfer {
    pub fn start(
        backend: Backend,
        server: String,
        path: String,
        direction: Direction,
        expected_connection: Option<ConnectionScope>,
        local_path: std::path::PathBuf,
    ) -> Self {
        let operation = uuid::Uuid::new_v4();
        let name = path
            .rsplit('/')
            .find(|part| !part.is_empty())
            .unwrap_or(&path)
            .to_owned();
        let request = (|| -> Result<files::Operation, desktop_inputs::Invalid> {
            let server = desktop_inputs::identity(&server, "file server identity")?;
            let expected = desktop_inputs::captured(expected_connection.as_ref())?;
            let local_path = desktop_inputs::native_path(&local_path);
            Ok(match direction {
                Direction::Download => files::Operation::Download(Box::new(files::DownloadArgs {
                    server,
                    operation,
                    path: path.clone(),
                    expected,
                    local_path,
                })),
                Direction::Upload => files::Operation::Upload(Box::new(files::UploadArgs {
                    server,
                    operation,
                    path: path.clone(),
                    expected,
                    local_path,
                })),
            })
        })();
        let result = request_result(&backend, request, Duration::from_secs(24 * 60 * 60));
        Self {
            backend,
            connection: expected_connection,
            operation,
            server,
            path,
            name,
            direction,
            result,
            completion: None,
            finished: false,
            retired: false,
            progress: None,
            progress_reply: None,
            last_progress: Instant::now(),
            cancel_reply: None,
            cancel_requested: false,
            cancel_error: None,
        }
    }
    pub fn operation(&self) -> uuid::Uuid {
        self.operation
    }
    pub fn connection(&self) -> Option<&ConnectionScope> {
        self.connection.as_ref()
    }
    pub fn retired(&self) -> bool {
        self.retired
    }
    /// Stop scheduling reads, request cancellation, and retain the original
    /// worker until it replies. Obsolete completions are never delivered.
    pub fn retire(&mut self) {
        if !self.retired {
            self.cancel();
            self.retired = true;
        }
    }
    pub fn server(&self) -> &str {
        &self.server
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn direction(&self) -> Direction {
        self.direction
    }
    pub fn progress(&self) -> Option<&Progress> {
        self.progress.as_ref()
    }
    pub fn finished(&self) -> bool {
        self.finished
    }
    pub fn cancelling(&self) -> bool {
        self.cancel_pending() || self.cancel_requested
    }
    pub fn cancel_pending(&self) -> bool {
        self.cancel_reply.is_some()
    }
    /// A failed or malformed cancellation acknowledgement permits explicit
    /// retry. It never establishes that the original transfer stopped.
    pub fn cancel_error(&self) -> Option<&str> {
        self.cancel_error.as_deref()
    }

    /// Submit at most one cancellation request at a time, to this operation's
    /// original backend. Returns false after completion or while cancelling.
    pub fn cancel(&mut self) -> bool {
        self.receive_completion();
        self.poll_cancel();
        if self.finished || self.retired || self.cancelling() {
            return false;
        }
        self.cancel_error = None;
        self.cancel_reply = Some(request(
            &self.backend,
            files::Operation::Cancel(Box::new(files::CancelArgs {
                operation: self.operation,
            })),
            CANCEL_TIMEOUT,
        ));
        true
    }

    pub fn poll(&mut self) -> Update {
        self.poll_at(Instant::now())
    }
    fn poll_at(&mut self, now: Instant) -> Update {
        self.receive_completion();
        if self.finished {
            return self.take_completion();
        }
        let mut changed = self.poll_cancel();
        if let Some(reply) = self.progress_reply.as_ref().and_then(ready) {
            self.progress_reply = None;
            if let Ok(files::Reply::Progress(value)) = reply
                && let Some(progress) = value.progress
            {
                changed |= self.accept_progress(Progress {
                    name: progress.name,
                    completed: progress.completed,
                    total: progress.total,
                });
            }
        }
        // Prefer a transfer result that arrived while draining ancillary reads.
        self.receive_completion();
        if self.finished {
            return self.take_completion();
        }
        if !self.retired
            && self.progress_reply.is_none()
            && now.saturating_duration_since(self.last_progress) >= PROGRESS_INTERVAL
        {
            self.last_progress = now;
            self.progress_reply = Some(request(
                &self.backend,
                files::Operation::Progress(Box::new(files::ProgressArgs {
                    operation: self.operation,
                })),
                PROGRESS_TIMEOUT,
            ));
        }
        Update {
            changed,
            completion: None,
        }
    }

    fn take_completion(&mut self) -> Update {
        let completion = self.completion.take();
        Update {
            changed: completion.is_some(),
            completion: if self.retired { None } else { completion },
        }
    }
    fn receive_completion(&mut self) {
        if self.finished {
            return;
        }
        let Some(reply) = ready(&self.result) else {
            return;
        };
        let result = reply.and_then(|value| {
            let value = match (self.direction, value) {
                (Direction::Upload, files::Reply::Upload(value)) | (Direction::Download, files::Reply::Download(value)) => value,
                _ => return Err("Invalid file transfer result. Check the destination before retrying; the request was not replayed.".into()),
            };
            match value.path {
                Some(files::TransferPath::Remote(value)) if self.direction == Direction::Upload => Ok(Destination::Remote(*value)),
                Some(files::TransferPath::Local(value)) if self.direction == Direction::Download => desktop_inputs::local_path(*value).map(Destination::Local).map_err(|error| error.to_string()),
                _ => Err("File transfer returned an invalid destination. Check it before retrying; the request was not replayed.".into()),
            }
        });
        if result.is_err() {
            self.best_effort_cancel();
        }
        self.finished = true;
        self.completion = Some(result);
        self.progress_reply = None;
        self.cancel_reply = None;
        self.cancel_requested = false;
        self.cancel_error = None;
    }
    fn poll_cancel(&mut self) -> bool {
        let Some(reply) = self.cancel_reply.as_ref().and_then(ready) else {
            return false;
        };
        self.cancel_reply = None;
        match reply {
            Ok(files::Reply::Cancel(_)) => self.cancel_requested = true,
            result => {
                let error = result
                    .err()
                    .unwrap_or_else(|| "Invalid cancellation acknowledgement".into());
                let mut message = format!("Cancellation could not be confirmed: {error}");
                if message.len() > MAX_CANCEL_ERROR {
                    let mut end = MAX_CANCEL_ERROR;
                    while !message.is_char_boundary(end) {
                        end -= 1;
                    }
                    message.truncate(end);
                }
                self.cancel_error = Some(message);
                self.cancel_requested = false;
            }
        }
        true
    }
    fn accept_progress(&mut self, mut next: Progress) -> bool {
        if next.name.is_empty()
            || next.name.len() > MAX_PROGRESS_NAME
            || next.name.contains('\0')
            || next.total.is_some_and(|total| next.completed > total)
        {
            return false;
        }
        if let Some(previous) = &self.progress {
            if next.name != previous.name || next.completed < previous.completed {
                return false;
            }
            next.total = match (previous.total, next.total) {
                (Some(old), Some(new)) => Some(old.max(new)),
                (Some(old), None) => Some(old),
                (None, total) => total,
            };
            if next.total.is_some_and(|total| next.completed > total) {
                return false;
            }
        }
        if self.progress.as_ref() == Some(&next) {
            return false;
        }
        self.progress = Some(next);
        true
    }
    fn best_effort_cancel(&mut self) {
        self.poll_cancel();
        if !self.cancelling() {
            self.cancel_reply = Some(request(
                &self.backend,
                files::Operation::Cancel(Box::new(files::CancelArgs {
                    operation: self.operation,
                })),
                CANCEL_TIMEOUT,
            ));
        }
    }
}
impl Drop for Transfer {
    fn drop(&mut self) {
        // Avoid cancelling an already returned successful write just because
        // its owner closed before the next UI poll.
        self.receive_completion();
        if !self.finished {
            self.best_effort_cancel();
        }
    }
}

fn request(
    backend: &Backend,
    operation: files::Operation,
    timeout: Duration,
) -> mpsc::Receiver<Reply> {
    request_result(backend, Ok(operation), timeout)
}
fn request_result(
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
            .and_then(|value| match value {
                desktop::Reply::Files(value) => Ok(*value),
                _ => Err("Unexpected file transfer response".into()),
            });
        let _ = send.send(result);
    });
    receive
}
fn ready(receiver: &mpsc::Receiver<Reply>) -> Option<Reply> {
    match receiver.try_recv() {
        Ok(reply) => Some(reply),
        Err(mpsc::TryRecvError::Empty) => None,
        Err(mpsc::TryRecvError::Disconnected) => Some(Err("File transfer worker stopped before confirming the result. Check the destination before retrying.".into())),
    }
}

#[cfg(test)]
mod tests;
