use super::*;
use crate::file_ipc_fixture::{self as rpc, Observation as Request, Peer as Stream};
use newport_platform::ipc::Listener;
type FixtureReply = Result<Value, String>;
use std::path::PathBuf;

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
                Ok((stream, _)) => {
                    let stream = Stream::admit(stream);
                    let request = stream.observation();
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
fn reply(mut stream: Stream, result: FixtureReply) {
    stream.reply(result);
}

fn connection() -> Value {
    rpc::connection()
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
    let (original, request) = host.next(match direction {
        Direction::Upload => "files_upload",
        Direction::Download => "files_download",
    });
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
fn progress(host: &Host, transfer: &mut Transfer, now: &mut Instant, value: FixtureReply) {
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
        assert!(!transfer.operation().is_nil());
        assert_eq!(transfer.server(), SERVER);
        assert_eq!(transfer.path(), "/srv/資料/file's.txt");
        assert_eq!(transfer.name(), "file's.txt");
        assert_eq!(transfer.connection(), &connection());
        assert_eq!(transfer.direction(), direction);
        reply(original, Ok(json!("/completed/資料.txt")));
        assert_eq!(
            until(&mut transfer, now, Transfer::finished),
            vec![Ok(match direction {
                Direction::Upload => Destination::Remote("/completed/資料.txt".into()),
                Direction::Download => Destination::Local("/completed/資料.txt".into()),
            })]
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
fn completion_without_a_path_is_invalid_and_cancels() {
    let host = Host::new();
    let (mut transfer, original, now) = start(&host, Direction::Download);
    reply(original, Ok(Value::Null));
    let completions = until(&mut transfer, now, Transfer::finished);
    assert_eq!(completions.len(), 1);
    assert!(
        completions[0]
            .as_ref()
            .unwrap_err()
            .contains("invalid destination")
    );
    let (cancel, request) = host.next("files_cancel");
    assert_eq!(request.args, json!({"operation":transfer.operation()}));
    reply(cancel, Ok(Value::Null));
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
        vec![Ok(Destination::Local("/saved".into()))]
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
            .contains("invalid destination")
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
    let operation = transfer.operation();
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
