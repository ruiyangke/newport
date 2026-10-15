//! One retained file transfer on an explicitly captured desktop backend.
//! The original write is never replayed. Cancellation acknowledges a request,
//! while only the original transfer reply establishes its outcome.
use crate::backend::Backend;
use serde::Deserialize;
use serde_json::{Value, json};
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
type Reply = Result<Value, String>;
pub type Completion = Result<Option<String>, String>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Download,
    Upload,
}
impl Direction {
    fn method(self) -> &'static str {
        match self {
            Self::Download => "native.files_download",
            Self::Upload => "native.files_upload",
        }
    }
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
    /// Emitted once. A null backend completion is retained for compatibility;
    /// native chooser cancellation happens before a Transfer is created.
    pub completion: Option<Completion>,
}

pub struct Transfer {
    backend: Backend,
    connection: Value,
    operation: String,
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
        expected_connection: Value,
        local_path: std::path::PathBuf,
    ) -> Self {
        let operation = uuid::Uuid::new_v4().to_string();
        let name = path
            .rsplit('/')
            .find(|part| !part.is_empty())
            .unwrap_or(&path)
            .to_owned();
        let worker = backend.clone();
        let args = json!({"id":server,"path":path,"operation":operation,"expectedConnection":expected_connection,"localPath":local_path.into_os_string()});
        let (send, result) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let _ = send.send(worker.call(direction.method(), args));
        });
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
    pub fn operation(&self) -> &str {
        &self.operation
    }
    pub fn connection(&self) -> &Value {
        &self.connection
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
            "files_cancel",
            &self.operation,
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
            if let Ok(value) = reply
                && let Ok(Some(progress)) = serde_json::from_value::<Option<Progress>>(value)
            {
                changed |= self.accept_progress(progress);
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
                "files_progress",
                &self.operation,
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
        let result = reply.and_then(|value| match value {
            Value::Null => Ok(None),
            Value::String(path) => Ok(Some(path)),
            _ => Err("Invalid file transfer result. Check the destination before retrying; the request was not replayed.".into()),
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
            Ok(Value::Null) => self.cancel_requested = true,
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
                "files_cancel",
                &self.operation,
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
    method: &'static str,
    operation: &str,
    timeout: Duration,
) -> mpsc::Receiver<Reply> {
    let backend = backend.clone();
    let args = json!({"operation":operation});
    let (send, receive) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let _ = send.send(backend.call_with_timeout(method, args, timeout));
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
mod tests {
    use super::*;
    use newport_platform::ipc::protocol::{Request, Response, VERSION};
    use newport_platform::ipc::{Listener, Stream};
    use std::{
        io::{Read, Write},
        path::PathBuf,
    };

    const SERVER: &str = "12345678-1234-4234-8234-123456789abc";
    struct Host {
        path: PathBuf,
        listener: Listener,
    }
    impl Host {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("np-ft-{}.sock", uuid::Uuid::new_v4().simple()));
            let listener = Listener::bind(&path).unwrap();
            listener.set_nonblocking(true).unwrap();
            Self { path, listener }
        }
        fn backend(&self) -> Backend {
            Backend::at_path(self.path.clone())
        }
        fn next(&self, method: &str) -> (Stream, Request) {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match self.listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let mut header = [0; 4];
                        stream.read_exact(&mut header).unwrap();
                        let size = u32::from_be_bytes(header) as usize;
                        assert!(size < 1024 * 1024);
                        let mut bytes = vec![0; size];
                        stream.read_exact(&mut bytes).unwrap();
                        let request: Request = rmp_serde::from_slice(&bytes).unwrap();
                        assert_eq!(request.version, VERSION);
                        assert_eq!(request.method, method);
                        return (stream, request);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "missing {method}");
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("{error}"),
                }
            }
        }
        fn quiet(&self) {
            let deadline = Instant::now() + Duration::from_millis(20);
            while Instant::now() < deadline {
                match self.listener.accept() {
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    _ => panic!("unexpected extra RPC"),
                }
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
    impl Drop for Host {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
    fn reply(mut stream: Stream, value: Reply) {
        let (result, error) = match value {
            Ok(value) => (value, None),
            Err(error) => (Value::Null, Some(json!(error))),
        };
        let bytes = rmp_serde::to_vec_named(&Response {
            version: VERSION,
            result,
            error,
        })
        .unwrap();
        stream
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .unwrap();
        stream.write_all(&bytes).unwrap();
    }
    fn connection() -> Value {
        json!({"host":"original.example", "port":22,"username":"alice", "auth":"agent"})
    }
    fn start(host: &Host, direction: Direction) -> (Transfer, Stream, Instant) {
        let transfer = Transfer::start(
            host.backend(),
            SERVER.into(),
            "/srv/資料/file's.txt".into(),
            direction,
            connection(),
            "/tmp/資料/file's.txt".into(),
        );
        let (original, request) = host.next(direction.method());
        assert_eq!(
            request.args,
            json!({"id":SERVER,"path":"/srv/資料/file's.txt","operation":transfer.operation(),"expectedConnection":connection(),"localPath":std::ffi::OsString::from("/tmp/資料/file's.txt")})
        );
        let now = transfer.last_progress;
        (transfer, original, now)
    }
    fn until(
        transfer: &mut Transfer,
        now: Instant,
        done: impl Fn(&Transfer) -> bool,
    ) -> Vec<Completion> {
        let mut completions = vec![];
        for _ in 0..1000 {
            if let Some(result) = transfer.poll_at(now).completion {
                completions.push(result);
            }
            if done(transfer) {
                return completions;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("transfer did not reach expected state");
    }
    fn progress(host: &Host, transfer: &mut Transfer, now: &mut Instant, value: Reply) {
        *now += PROGRESS_INTERVAL;
        assert!(transfer.poll_at(*now).completion.is_none());
        let (stream, request) = host.next("files_progress");
        assert_eq!(request.args, json!({"operation":transfer.operation()}));
        reply(stream, value);
        assert!(until(transfer, *now, |t| t.progress_reply.is_none()).is_empty());
    }

    #[test]
    fn transfer_captures_scope_and_each_direction_delivers_completion_once() {
        for direction in [Direction::Upload, Direction::Download] {
            let host = Host::new();
            let (mut transfer, original, now) = start(&host, direction);
            assert!(uuid::Uuid::parse_str(transfer.operation()).is_ok());
            assert_eq!(transfer.server(), SERVER);
            assert_eq!(transfer.path(), "/srv/資料/file's.txt");
            assert_eq!(transfer.name(), "file's.txt");
            assert_eq!(transfer.connection(), &connection());
            assert_eq!(transfer.direction(), direction);
            reply(original, Ok(json!("/completed/資料.txt")));
            assert_eq!(
                until(&mut transfer, now, Transfer::finished),
                vec![Ok(Some("/completed/資料.txt".into()))]
            );
            assert_eq!(
                transfer.poll_at(now + Duration::from_secs(60)),
                Update::default()
            );
            assert!(!transfer.cancel());
            drop(transfer);
            host.quiet();
        }
    }
    #[test]
    fn null_backend_completion_is_delivered_once() {
        let host = Host::new();
        let (mut transfer, original, now) = start(&host, Direction::Download);
        reply(original, Ok(Value::Null));
        assert_eq!(
            until(&mut transfer, now, Transfer::finished),
            vec![Ok(None)]
        );
        assert!(transfer.progress().is_none());
        host.quiet();
    }
    #[test]
    fn progress_is_coalesced_and_empty_invalid_or_regressive_readings_preserve_bytes() {
        let host = Host::new();
        let (mut transfer, original, mut now) = start(&host, Direction::Upload);
        now += PROGRESS_INTERVAL;
        transfer.poll_at(now);
        let (held, request) = host.next("files_progress");
        assert_eq!(request.args, json!({"operation":transfer.operation()}));
        now += Duration::from_secs(10);
        for _ in 0..20 {
            transfer.poll_at(now);
        }
        host.quiet();
        reply(
            held,
            Ok(json!({"name":"選択.txt", "completed":10, "total":20})),
        );
        until(&mut transfer, now, |t| t.progress().is_some());
        // One follow-up may start after the held request settles; never two.
        let (followup, _) = host.next("files_progress");
        reply(followup, Ok(Value::Null));
        until(&mut transfer, now, |t| t.progress_reply.is_none());
        let retained = transfer.progress().unwrap().clone();
        for value in [
            Ok(Value::Null),
            Err("temporary progress failure".into()),
            Ok(json!({"name":"", "completed":0, "total":null})),
            Ok(json!({"name":"選択.txt", "completed":9, "total":20})),
            Ok(json!({"name":"another.txt", "completed":20, "total":20})),
            Ok(json!({"name":"選択.txt", "completed":25, "total":20})),
            Ok(json!({"name":"選択.txt", "completed":-1, "total":20})),
            Ok(json!({"name":"選択.txt", "total":20})),
            Ok(json!({"name":"x".repeat(MAX_PROGRESS_NAME + 1), "completed":10, "total":20})),
            Ok(json!({"name":"\0", "completed":10, "total":20})),
        ] {
            progress(&host, &mut transfer, &mut now, value);
            assert_eq!(transfer.progress(), Some(&retained));
        }
        progress(
            &host,
            &mut transfer,
            &mut now,
            Ok(json!({"name":"選択.txt", "completed":15, "total":null})),
        );
        assert_eq!(transfer.progress().unwrap().completed, 15);
        assert_eq!(transfer.progress().unwrap().total, Some(20));
        progress(
            &host,
            &mut transfer,
            &mut now,
            Ok(json!({"name":"選択.txt", "completed":16, "total":16})),
        );
        assert_eq!(transfer.progress().unwrap().total, Some(20));
        reply(original, Ok(json!("/srv/選択.txt")));
        until(&mut transfer, now, Transfer::finished);
    }
    #[test]
    fn progress_distinguishes_unknown_total_and_empty_file() {
        for total in [Value::Null, json!(0)] {
            let host = Host::new();
            let (mut transfer, original, mut now) = start(&host, Direction::Download);
            progress(
                &host,
                &mut transfer,
                &mut now,
                Ok(json!({"name":"empty", "completed":0, "total":total})),
            );
            assert_eq!(transfer.progress().unwrap().total, total.as_u64());
            reply(original, Ok(json!("/empty")));
            until(&mut transfer, now, Transfer::finished);
        }
    }
    #[test]
    fn cancellation_errors_are_retryable_and_acknowledgement_does_not_finish_transfer() {
        let host = Host::new();
        let (mut transfer, original, now) = start(&host, Direction::Download);
        for value in [Err("Cancellation transport failed".into()), Ok(json!(true))] {
            assert!(transfer.cancel());
            assert!(!transfer.cancel());
            assert!(transfer.cancelling() && transfer.cancel_pending());
            let (cancel, request) = host.next("files_cancel");
            assert_eq!(request.args, json!({"operation":transfer.operation()}));
            host.quiet();
            reply(cancel, value);
            assert!(until(&mut transfer, now, |t| t.cancel_error().is_some()).is_empty());
            assert!(!transfer.cancelling());
            assert!(!transfer.finished());
        }
        assert!(transfer.cancel());
        assert!(transfer.cancel_error().is_none());
        let (cancel, _) = host.next("files_cancel");
        reply(cancel, Ok(Value::Null));
        assert!(until(&mut transfer, now, |t| !t.cancel_pending()).is_empty());
        assert!(transfer.cancelling());
        assert!(!transfer.finished());
        assert!(!transfer.cancel());
        reply(original, Err("File operation cancelled".into()));
        assert_eq!(
            until(&mut transfer, now, Transfer::finished),
            vec![Err("File operation cancelled".into())]
        );
        assert!(!transfer.cancelling());
        host.quiet();
    }
    #[test]
    fn original_completion_wins_pending_cancel_and_late_progress_races() {
        let host = Host::new();
        let (mut transfer, original, mut now) = start(&host, Direction::Download);
        progress(
            &host,
            &mut transfer,
            &mut now,
            Ok(json!({"name":"file", "completed":5, "total":10})),
        );
        now += PROGRESS_INTERVAL;
        transfer.poll_at(now);
        let (late_progress, _) = host.next("files_progress");
        assert!(transfer.cancel());
        let (late_cancel, _) = host.next("files_cancel");
        reply(original, Ok(json!("/saved")));
        assert_eq!(
            until(&mut transfer, now, Transfer::finished),
            vec![Ok(Some("/saved".into()))]
        );
        reply(late_cancel, Err("late cancellation error".into()));
        reply(
            late_progress,
            Ok(json!({"name":"file", "completed":10, "total":10})),
        );
        assert_eq!(
            transfer.poll_at(now + Duration::from_secs(30)),
            Update::default()
        );
        assert_eq!(transfer.progress().unwrap().completed, 5);
        assert!(transfer.cancel_error().is_none());
        assert!(!transfer.cancelling());
        drop(transfer);
        host.quiet();
    }
    #[test]
    fn interrupted_write_is_never_replayed_and_requests_best_effort_cancel() {
        let host = Host::new();
        let (mut transfer, original, now) = start(&host, Direction::Upload);
        drop(original);
        let completions = until(&mut transfer, now, Transfer::finished);
        assert_eq!(completions.len(), 1);
        assert!(completions[0].as_ref().unwrap_err().contains("not retried"));
        let (cancel, request) = host.next("files_cancel");
        assert_eq!(request.args, json!({"operation":transfer.operation()}));
        reply(cancel, Ok(Value::Null));
        for _ in 0..10 {
            assert_eq!(transfer.poll_at(now), Update::default());
        }
        drop(transfer);
        host.quiet();
    }
    #[test]
    fn malformed_write_completion_stays_uncertain_instead_of_success() {
        let host = Host::new();
        let (mut transfer, original, now) = start(&host, Direction::Download);
        reply(original, Ok(json!({"ok":true})));
        let results = until(&mut transfer, now, Transfer::finished);
        assert!(
            results[0]
                .as_ref()
                .unwrap_err()
                .contains("Check the destination")
        );
        let (cancel, _) = host.next("files_cancel");
        reply(cancel, Ok(Value::Null));
        host.quiet();
    }
    #[test]
    fn immediate_retirement_cancels_but_holds_the_original_until_it_finishes() {
        let host = Host::new();
        let (mut transfer, original, now) = start(&host, Direction::Upload);
        transfer.retire();
        transfer.retire();
        assert!(transfer.retired());
        assert!(!transfer.finished());
        let (cancel, request) = host.next("files_cancel");
        assert_eq!(request.args, json!({"operation":transfer.operation()}));
        reply(cancel, Ok(Value::Null));
        assert!(until(&mut transfer, now, |t| !t.cancel_pending()).is_empty());
        assert!(
            !transfer.finished(),
            "cancel acknowledgement cannot release the write owner"
        );
        assert!(!transfer.cancel());
        assert!(
            transfer
                .poll_at(now + Duration::from_secs(60))
                .completion
                .is_none()
        );
        host.quiet();
        reply(original, Ok(json!("/old-scope-result")));
        assert!(until(&mut transfer, now, Transfer::finished).is_empty());
        assert_eq!(transfer.poll_at(now), Update::default());
        drop(transfer);
        host.quiet();
    }
    #[test]
    fn drop_cancels_only_the_captured_backend_and_operation() {
        let first = Host::new();
        let second = Host::new();
        let (transfer, original, _) = start(&first, Direction::Download);
        let operation = transfer.operation().to_owned();
        let (mut other, other_original, now) = start(&second, Direction::Upload);
        assert_ne!(operation, other.operation());
        drop(transfer);
        let (cancel, request) = first.next("files_cancel");
        assert_eq!(request.args, json!({"operation":operation}));
        reply(cancel, Ok(Value::Null));
        reply(original, Err("File operation cancelled".into()));
        second.quiet();
        reply(other_original, Ok(Value::Null));
        until(&mut other, now, Transfer::finished);
    }
    #[test]
    fn dropping_after_cancel_acknowledgement_does_not_duplicate_cancel() {
        let host = Host::new();
        let (mut transfer, original, now) = start(&host, Direction::Download);
        transfer.cancel();
        let (cancel, _) = host.next("files_cancel");
        reply(cancel, Ok(Value::Null));
        until(&mut transfer, now, |t| !t.cancel_pending());
        drop(transfer);
        host.quiet();
        reply(original, Err("File operation cancelled".into()));
    }
    #[test]
    fn retained_cancellation_error_is_utf8_safe_and_bounded() {
        let host = Host::new();
        let (mut transfer, original, now) = start(&host, Direction::Download);
        transfer.cancel();
        let (cancel, _) = host.next("files_cancel");
        reply(cancel, Err("錯".repeat(MAX_CANCEL_ERROR)));
        until(&mut transfer, now, |t| t.cancel_error().is_some());
        assert!(transfer.cancel_error().unwrap().len() <= MAX_CANCEL_ERROR);
        assert!(!transfer.cancelling());
        reply(original, Ok(Value::Null));
        until(&mut transfer, now, Transfer::finished);
    }
}
