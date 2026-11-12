use super::*;
use crate::file_ipc_fixture::{self as rpc, Observation as Request, Peer as Stream};
use newport_platform::ipc::Listener;
type FixtureReply = Result<Value, String>;
use std::{path::PathBuf, time::Instant};

struct Host {
    path: PathBuf,
    listener: Listener,
}
impl Host {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("np-fb-{}.sock", uuid::Uuid::new_v4().simple()));
        let listener = Listener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        Self { path, listener }
    }
    fn browser(&self) -> Browser {
        Browser::new(
            Backend::at_path(self.path.clone()),
            rpc::SERVER.into(),
            rpc::connection(),
        )
    }
    fn receive(&self) -> (Stream, Request) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    let stream = Stream::admit(stream);
                    let request = stream.observation();
                    return (stream, request);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "missing RPC");
                    thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("{error}"),
            }
        }
    }
    fn next(&self, method: &str) -> (Stream, Request) {
        let (stream, request) = self.receive();
        assert_eq!(request.method, method);
        (stream, request)
    }
    fn quiet(&self) {
        let deadline = Instant::now() + Duration::from_millis(15);
        while Instant::now() < deadline {
            match self.listener.accept() {
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                _ => panic!("unexpected RPC"),
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

fn listing(path: &str) -> Value {
    json!({"path":path,"entries":[]})
}
#[test]
fn preview_contract_accepts_text_and_supported_images() {
    for mime in ["image/png", "image/jpeg", "image/gif", "image/webp"] {
        assert!(
            valid_preview(serde_json::json!({"kind":"image","content":"data","mime":mime})).is_ok()
        );
    }
    for kind in ["image", "pdf", "binary"] {
        assert!(
            valid_preview(
                serde_json::json!({"kind":kind,"content":"data","mime":"application/octet-stream"})
            )
            .is_err()
        );
    }
    assert!(
        valid_preview(
            serde_json::json!({"kind":"text","content":"","mime":"text/plain","truncated":false})
        )
        .is_ok()
    );
}

fn preview(content: &str) -> Value {
    json!({"kind":"text","content":content,"mime":"text/plain","truncated":false})
}
fn until(browser: &mut Browser, done: impl Fn(&Browser) -> bool) -> Vec<Update> {
    let mut updates = vec![];
    for _ in 0..1000 {
        updates.push(browser.poll());
        if done(browser) {
            return updates;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("browser did not reach expected state");
}
fn loaded(host: &Host, browser: &mut Browser, path: &str, canonical: &str) -> Vec<Update> {
    assert!(browser.navigate(path.into()));
    let (stream, request) = host.next("files_list");
    assert_eq!(request.args["path"], path);
    reply(stream, Ok(listing(canonical)));
    until(browser, |browser| !browser.has_pending())
}
fn acknowledge_cancel(host: &Host, operation: &Value) {
    let (cancel, request) = host.next("files_cancel");
    assert_eq!(request.args, json!({"operation":operation}));
    reply(cancel, Ok(Value::Null));
}

#[test]
fn construction_is_idle_and_reads_capture_backend_server_connection_and_exact_path() {
    let host = Host::new();
    let mut browser = host.browser();
    host.quiet();
    assert!(!browser.refresh());
    assert!(!browser.retry());
    assert_eq!(browser.server(), rpc::SERVER);
    assert!(browser.navigate("/資料/it's here".into()));
    let (stream, request) = host.next("files_list");
    assert!(uuid::Uuid::parse_str(request.args["operation"].as_str().unwrap()).is_ok());
    assert_eq!(request.args["id"], browser.server());
    assert_eq!(&request.args["expectedConnection"], browser.connection());
    assert_eq!(request.args["path"], "/資料/it's here");
    reply(stream, Ok(listing("/canonical")));
    let updates = until(&mut browser, |browser| !browser.has_pending());
    assert_eq!(
        updates
            .iter()
            .filter(|u| u.folder_changed && u.listing_changed)
            .count(),
        1
    );
    assert_eq!(browser.canonical_path(), Some("/canonical"));
    assert_eq!(browser.attempted_path(), Some("/資料/it's here"));
    assert_eq!(browser.listing_revision(), 1);
    host.quiet();
}
#[test]
fn rapid_navigation_cancels_one_physical_read_and_launches_only_the_newest_desired_path() {
    let host = Host::new();
    let mut browser = host.browser();
    browser.navigate("/a".into());
    let (old, request) = host.next("files_list");
    browser.navigate("/b".into());
    let (cancel, cancellation) = host.next("files_cancel");
    assert_eq!(cancellation.args["operation"], request.args["operation"]);
    for n in 0..100 {
        browser.navigate(format!("/newest-{n}"));
        browser.poll();
    }
    host.quiet();
    reply(old, Ok(listing("/stale-a")));
    let updates = until(&mut browser, |browser| {
        browser
            .listing_reads
            .active
            .as_ref()
            .is_some_and(|work| work.reply.is_none())
    });
    assert!(
        updates
            .iter()
            .all(|u| !u.listing_changed && !u.folder_changed)
    );
    assert!(browser.listing().is_none());
    assert!(browser.listing_pending());
    host.quiet(); // cancellation worker is also retained until it replies
    reply(cancel, Ok(Value::Null));
    until(&mut browser, |browser| {
        browser
            .listing_reads
            .active
            .as_ref()
            .is_some_and(|work| work.intent.path == "/newest-99")
    });
    let (latest, latest_request) = host.next("files_list");
    assert_eq!(latest_request.args["path"], "/newest-99");
    assert_ne!(latest_request.args["operation"], request.args["operation"]);
    reply(latest, Ok(listing("/canonical-last")));
    until(&mut browser, |browser| !browser.has_pending());
    assert_eq!(browser.listing_revision(), 1);
    assert_eq!(browser.canonical_path(), Some("/canonical-last"));
    host.quiet();
}
#[test]
fn failed_navigation_retains_listing_attempt_and_canonical_and_retry_uses_the_attempt() {
    let host = Host::new();
    let mut browser = host.browser();
    loaded(&host, &mut browser, ".", "/home/alice");
    let previous = browser.listing().unwrap().clone();
    browser.navigate("/denied".into());
    assert_eq!(browser.listing(), Some(&previous));
    let (read, request) = host.next("files_list");
    reply(read, Err("Permission denied".into()));
    until(&mut browser, |browser| {
        browser
            .listing_reads
            .active
            .as_ref()
            .is_some_and(|work| work.cancel_sent)
    });
    acknowledge_cancel(&host, &request.args["operation"]);
    until(&mut browser, |browser| !browser.has_pending());
    assert_eq!(browser.listing(), Some(&previous));
    assert_eq!(browser.attempted_path(), Some("/denied"));
    assert_eq!(browser.canonical_path(), Some("/home/alice"));
    assert_eq!(browser.listing_error(), Some("Permission denied"));
    assert_eq!(browser.listing_revision(), 1);
    assert!(browser.retry());
    assert!(browser.listing_error().is_none());
    let (retry, repeated) = host.next("files_list");
    assert_eq!(repeated.args["path"], "/denied");
    assert_ne!(repeated.args["operation"], request.args["operation"]);
    reply(retry, Ok(listing("/different")));
    let updates = until(&mut browser, |browser| !browser.has_pending());
    assert_eq!(
        updates
            .iter()
            .filter(|update| update.folder_changed)
            .count(),
        1
    );
    assert_eq!(browser.listing_revision(), 2);
}
#[test]
fn same_folder_alias_and_refresh_do_not_report_a_folder_change_or_reset_state() {
    let host = Host::new();
    let mut browser = host.browser();
    loaded(&host, &mut browser, ".", "/home/alice");
    let data_revision = browser.listing_data_revision();
    let updates = loaded(&host, &mut browser, "/home/alice/.", "/home/alice");
    assert!(updates.iter().all(|u| !u.folder_changed));
    assert_eq!(browser.listing_revision(), 2);
    assert_eq!(browser.listing_data_revision(), data_revision);
    browser.refresh();
    let (read, request) = host.next("files_list");
    assert_eq!(request.args["path"], "/home/alice");
    reply(
        read,
        Ok(
            json!({"path":"/home/alice","entries":[{"name":"new.txt","path":"/home/alice/new.txt","kind":"file"}]}),
        ),
    );
    let updates = until(&mut browser, |browser| !browser.has_pending());
    assert!(updates.iter().any(|u| u.listing_changed));
    assert!(updates.iter().all(|u| !u.folder_changed));
    assert_eq!(browser.listing_revision(), 3);
    assert!(browser.listing_data_revision() > data_revision);
}
#[test]
fn preview_a_b_a_gets_fresh_identity_and_stale_a_cannot_fill_the_new_a() {
    let host = Host::new();
    let mut browser = host.browser();
    let first = browser.open_preview("/a".into()).unwrap();
    let (old, request) = host.next("files_preview");
    assert_eq!(
        request.args["expectedConnection"],
        browser.connection().clone()
    );
    let middle = browser.open_preview("/b".into()).unwrap();
    let latest = browser.open_preview("/a".into()).unwrap();
    assert!(first < middle && middle < latest);
    acknowledge_cancel(&host, &request.args["operation"]);
    until(&mut browser, |browser| {
        browser
            .preview_reads
            .active
            .as_ref()
            .unwrap()
            .cancel
            .is_none()
    });
    assert!(browser.preview().unwrap().pending);
    assert!(browser.preview().unwrap().data.is_none());
    host.quiet(); // cancellation acknowledgement does not finish the old read
    reply(old, Ok(preview("old a")));
    let updates = until(&mut browser, |browser| {
        browser
            .preview_reads
            .active
            .as_ref()
            .is_some_and(|work| work.intent.token == latest)
    });
    assert!(updates.iter().all(|u| !u.preview_changed));
    let (read, request) = host.next("files_preview");
    assert_eq!(request.args["path"], "/a");
    reply(read, Ok(preview("new a")));
    let updates = until(&mut browser, |browser| !browser.has_pending());
    assert_eq!(updates.iter().filter(|u| u.preview_changed).count(), 1);
    assert!(browser.take_preview_data(first).is_none());
    assert_eq!(browser.take_preview_data(latest), Some(preview("new a")));
    assert!(browser.take_preview_data(latest).is_none());
    let retained = browser.preview().unwrap();
    assert_eq!(retained.token, latest);
    assert_eq!(retained.path, "/a");
    assert!(!retained.pending && retained.error.is_none());
    assert_eq!(browser.poll(), Update::default());
    host.quiet();
}
#[test]
fn preview_error_retry_clears_data_and_changes_token_without_replaying_automatically() {
    let host = Host::new();
    let mut browser = host.browser();
    let first = browser.open_preview("/text".into()).unwrap();
    let (read, request) = host.next("files_preview");
    reply(read, Err("File is unavailable".into()));
    let updates = until(&mut browser, |browser| {
        browser
            .preview_reads
            .active
            .as_ref()
            .is_some_and(|work| work.cancel_sent)
    });
    assert!(updates.iter().any(|u| u.preview_changed));
    assert!(!browser.preview().unwrap().pending);
    acknowledge_cancel(&host, &request.args["operation"]);
    let updates = until(&mut browser, |browser| !browser.has_pending());
    assert!(updates.iter().all(|u| !u.preview_changed));
    assert_eq!(
        browser.preview().unwrap().error.as_deref(),
        Some("File is unavailable")
    );
    host.quiet();
    let second = browser.retry_preview().unwrap();
    assert_ne!(first, second);
    assert!(browser.preview().unwrap().error.is_none());
    assert!(browser.preview().unwrap().data.is_none());
    let (read, request) = host.next("files_preview");
    assert_eq!(request.args["path"], "/text");
    reply(read, Ok(preview("")));
    until(&mut browser, |browser| !browser.has_pending());
    assert_eq!(browser.take_preview_data(second), Some(preview("")));
}
#[test]
fn authoritative_error_is_immediate_while_held_cancellation_still_bounds_retries() {
    let host = Host::new();
    let mut browser = host.browser();
    loaded(&host, &mut browser, ".", "/kept");
    browser.navigate("/denied".into());
    let (read, request) = host.next("files_list");
    reply(read, Err("Permission denied".into()));
    let updates = until(&mut browser, |browser| browser.listing_error().is_some());
    assert!(updates.iter().any(|u| u.changed));
    assert!(
        updates
            .iter()
            .all(|u| !u.folder_changed && !u.listing_changed)
    );
    assert_eq!(browser.listing_error(), Some("Permission denied"));
    assert!(
        !browser.listing_pending(),
        "read error must not wait for cancellation acknowledgement"
    );
    assert!(
        browser.has_pending(),
        "the cancellation worker remains owned"
    );
    assert_eq!(browser.canonical_path(), Some("/kept"));
    let (cancel, cancellation) = host.next("files_cancel");
    assert_eq!(cancellation.args["operation"], request.args["operation"]);
    browser.retry();
    browser.retry();
    browser.poll();
    assert!(browser.listing_pending());
    host.quiet();
    reply(cancel, Ok(Value::Null));
    until(&mut browser, |browser| {
        browser
            .listing_reads
            .active
            .as_ref()
            .is_some_and(|work| work.intent.token == browser.listing_token)
    });
    let (retry, request) = host.next("files_list");
    assert_eq!(request.args["path"], "/denied");
    reply(retry, Ok(listing("/kept")));
    let updates = until(&mut browser, |browser| !browser.has_pending());
    assert_eq!(updates.iter().filter(|u| u.listing_changed).count(), 1);
    assert!(updates.iter().all(|u| !u.folder_changed));
    assert!(browser.listing_error().is_none());
    host.quiet();
}
#[test]
fn navigation_closes_preview_and_late_result_never_repopulates_it() {
    let host = Host::new();
    let mut browser = host.browser();
    browser.open_preview("/old.txt".into());
    let (old, old_request) = host.next("files_preview");
    browser.navigate("/folder".into());
    assert!(browser.preview().is_none());
    let mut folder = None;
    for _ in 0..2 {
        let (stream, request) = host.receive();
        match request.method.as_str() {
            "files_cancel" => {
                assert_eq!(request.args["operation"], old_request.args["operation"]);
                reply(stream, Ok(Value::Null));
            }
            "files_list" => folder = Some(stream),
            other => panic!("unexpected {other}"),
        }
    }
    reply(folder.unwrap(), Ok(listing("/folder")));
    reply(old, Ok(preview("obsolete")));
    let updates = until(&mut browser, |browser| !browser.has_pending());
    assert!(updates.iter().all(|u| !u.preview_changed));
    assert!(browser.preview().is_none());
    assert_eq!(browser.canonical_path(), Some("/folder"));
}
#[test]
fn suspend_retains_successful_folder_and_only_explicit_refresh_resumes() {
    let host = Host::new();
    let mut browser = host.browser();
    loaded(&host, &mut browser, ".", "/kept");
    browser.navigate("/pending".into());
    let (old, request) = host.next("files_list");
    browser.suspend();
    assert!(!browser.retired());
    assert!(!browser.listing_pending());
    assert!(browser.has_pending());
    acknowledge_cancel(&host, &request.args["operation"]);
    reply(old, Ok(listing("/ignored")));
    let updates = until(&mut browser, |browser| !browser.has_pending());
    assert!(
        updates
            .iter()
            .all(|u| !u.listing_changed && !u.folder_changed)
    );
    assert_eq!(browser.canonical_path(), Some("/kept"));
    assert_eq!(browser.listing_revision(), 1);
    host.quiet();
    browser.refresh();
    let (read, request) = host.next("files_list");
    assert_eq!(request.args["path"], "/kept");
    reply(read, Ok(listing("/kept")));
    until(&mut browser, |browser| !browser.has_pending());
}
#[test]
fn retire_cancels_both_slots_drains_originals_and_never_delivers_obsolete_data() {
    let host = Host::new();
    let mut browser = host.browser();
    browser.navigate("/folder".into());
    let (listing, listing_request) = host.next("files_list");
    browser.open_preview("/file".into());
    let (preview, preview_request) = host.next("files_preview");
    browser.retire();
    browser.retire();
    assert!(browser.retired());
    assert!(!browser.navigate("/new".into()));
    assert!(browser.open_preview("/new".into()).is_none());
    let mut cancelled = vec![];
    for _ in 0..2 {
        let (stream, request) = host.next("files_cancel");
        cancelled.push(request.args["operation"].clone());
        reply(stream, Ok(Value::Null));
    }
    assert!(cancelled.contains(&listing_request.args["operation"]));
    assert!(cancelled.contains(&preview_request.args["operation"]));
    until(&mut browser, |browser| {
        browser
            .listing_reads
            .active
            .as_ref()
            .unwrap()
            .cancel
            .is_none()
            && browser
                .preview_reads
                .active
                .as_ref()
                .unwrap()
                .cancel
                .is_none()
    });
    assert!(browser.has_pending());
    host.quiet();
    reply(listing, Ok(super::tests::listing("/stale")));
    reply(preview, Ok(super::tests::preview("stale")));
    let updates = until(&mut browser, |browser| !browser.has_pending());
    assert!(
        updates
            .iter()
            .all(|u| !u.listing_changed && !u.folder_changed && !u.preview_changed)
    );
    assert!(browser.listing().is_none() && browser.preview().is_none());
}
#[test]
fn drop_cancels_both_operations_at_the_original_backend() {
    let host = Host::new();
    let other = Host::new();
    let mut browser = host.browser();
    browser.navigate("/folder".into());
    let (listing, listing_request) = host.next("files_list");
    browser.open_preview("/file".into());
    let (preview, preview_request) = host.next("files_preview");
    drop(browser);
    let mut operations = vec![];
    for _ in 0..2 {
        let (stream, request) = host.next("files_cancel");
        operations.push(request.args["operation"].clone());
        reply(stream, Ok(Value::Null));
    }
    assert!(operations.contains(&listing_request.args["operation"]));
    assert!(operations.contains(&preview_request.args["operation"]));
    reply(listing, Err("File operation cancelled".into()));
    reply(preview, Err("File operation cancelled".into()));
    host.quiet();
    other.quiet();
}
#[test]
fn interrupted_read_is_cancelled_and_never_replayed_without_an_explicit_retry() {
    let host = Host::new();
    let mut browser = host.browser();
    browser.navigate("/folder".into());
    let (read, request) = host.next("files_list");
    drop(read);
    until(&mut browser, |browser| {
        browser
            .listing_reads
            .active
            .as_ref()
            .is_some_and(|work| work.cancel_sent)
    });
    acknowledge_cancel(&host, &request.args["operation"]);
    until(&mut browser, |browser| !browser.has_pending());
    let ended = newport_protocol::desktop::wire::Error::Ended.to_string();
    assert_eq!(browser.listing_error(), Some(ended.as_str()));
    assert_eq!(browser.listing_revision(), 0);
    host.quiet();
    browser.retry();
    let (read, retry) = host.next("files_list");
    assert_eq!(retry.args["path"], "/folder");
    assert_ne!(retry.args["operation"], request.args["operation"]);
    reply(read, Ok(listing("/folder")));
    until(&mut browser, |browser| !browser.has_pending());
}
#[test]
fn malformed_listing_and_preview_projections_are_rejected() {
    for value in [
        json!({"entries":[]}),
        json!({"path":"","entries":[]}),
        json!({"path":"/wrong","entries":null}),
    ] {
        assert_eq!(
            valid_listing(value).unwrap_err(),
            "Invalid folder listing from the backend.",
        );
    }
    assert_eq!(
        valid_preview(json!({"kind":"text","mime":"text/plain"})).unwrap_err(),
        "Invalid file preview from the backend.",
    );
}

#[test]
fn malformed_listing_retains_last_success_and_preview_shape_errors_are_scoped() {
    let host = Host::new();
    let mut browser = host.browser();
    loaded(&host, &mut browser, ".", "/kept");
    for (value, cancelled, expected_error) in [
        (json!({"entries":[]}), true, "Invalid file fixture path"),
        (
            json!({"path":"","entries":[]}),
            false,
            "Invalid folder listing from the backend.",
        ),
        (
            json!({"path":"/wrong","entries":null}),
            true,
            "Invalid folder listing from the backend.",
        ),
    ] {
        browser.navigate("/bad".into());
        let (stream, request) = host.next("files_list");
        reply(stream, Ok(value));
        let mut updates = until(&mut browser, |browser| browser.listing_error().is_some());
        assert_eq!(
            browser
                .listing_reads
                .active
                .as_ref()
                .is_some_and(|work| work.cancel_sent),
            cancelled,
        );
        if cancelled {
            acknowledge_cancel(&host, &request.args["operation"]);
            updates.extend(until(&mut browser, |browser| !browser.has_pending()));
        } else {
            assert!(!browser.has_pending());
        }
        assert_eq!(browser.listing_error(), Some(expected_error));
        host.quiet();
        assert!(
            updates
                .iter()
                .all(|u| !u.folder_changed && !u.listing_changed)
        );
        assert_eq!(browser.canonical_path(), Some("/kept"));
        assert_eq!(browser.listing_revision(), 1);
        assert!(browser.listing_error().is_some());
    }
    browser.open_preview("/bad-preview".into());
    let (read, request) = host.next("files_preview");
    reply(read, Ok(json!({"kind":"text","mime":"text/plain"})));
    until(&mut browser, |browser| {
        browser.preview().unwrap().error.is_some()
    });
    assert!(
        browser
            .preview_reads
            .active
            .as_ref()
            .is_some_and(|work| work.cancel_sent)
    );
    acknowledge_cancel(&host, &request.args["operation"]);
    until(&mut browser, |browser| !browser.has_pending());
    assert_eq!(
        browser.preview().unwrap().error.as_deref(),
        Some("Invalid file fixture content"),
    );
    assert_eq!(browser.canonical_path(), Some("/kept"));
    assert_eq!(browser.listing_revision(), 1);
    browser.close_preview();
    assert!(browser.preview().is_none());
    host.quiet();
}
