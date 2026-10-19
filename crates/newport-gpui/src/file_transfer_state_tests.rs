//! Held local RPCs exercise transfer ownership without touching remote/local files.
use crate::file_test_backend::reply;
use crate::{Newport, Page};
use gpui_kit::{
    AppContext, Context, Entity, IntoElement, Render, TestAppContext, VisualTestContext, Window,
    div,
};
use newport_client::{
    backend::Backend,
    file_transfer::{Direction, Transfer},
};
use newport_platform::ipc::protocol::{Request, VERSION};
use newport_platform::ipc::{Listener, Stream};
use serde_json::json;
use std::{
    io::Read,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

struct Host {
    path: PathBuf,
    listener: Listener,
}
impl Host {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("np-t-{}.sock", uuid::Uuid::new_v4().simple()));
        let listener = Listener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        Self { path, listener }
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
                    stream
                        .set_write_timeout(Some(Duration::from_secs(2)))
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
                    uuid::Uuid::parse_str(request.args["operation"].as_str().unwrap()).unwrap();
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
}
impl Drop for Host {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
struct IdleHost;
impl Render for IdleHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
fn fixture(
    cx: &mut TestAppContext,
    direction: Direction,
) -> (
    Entity<Newport>,
    &mut VisualTestContext,
    Host,
    Stream,
    String,
) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        crate::theme::install(false, cx);
    });
    let host = Host::new();
    let mut owner = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        owner = Some(cx.new(|cx| {
            let mut view = crate::page_perf_tests::owner(window, cx);
            let connection =
                crate::terminal_scope::connection(view.selected_server().unwrap(), &view.snapshot);
            view.page = Page::Overview;
            view.file_transfer = Some(Transfer::start(
                Backend::at_path(host.path.clone()),
                "fixture".into(),
                "/remote/file.txt".into(),
                direction,
                connection,
                "/tmp/unused-local-transfer-fixture".into(),
            ));
            view
        }));
        IdleHost
    });
    let owner = owner.unwrap();
    let (stream, request) = host.next(match direction {
        Direction::Download => "native.files_download",
        Direction::Upload => "native.files_upload",
    });
    assert_eq!(request.args["id"], "fixture");
    assert_eq!(request.args["path"], "/remote/file.txt");
    assert_eq!(
        request.args["expectedConnection"],
        owner.read_with(cx, |view, _| view
            .file_transfer
            .as_ref()
            .unwrap()
            .connection()
            .clone())
    );
    (
        owner,
        cx,
        host,
        stream,
        request.args["operation"].as_str().unwrap().into(),
    )
}
fn poll(view: &Entity<Newport>, cx: &mut VisualTestContext) -> bool {
    let changed = view.update(cx, |view, cx| {
        let changed = view.poll_file_transfer(cx);
        if changed {
            cx.notify();
        }
        changed
    });
    cx.run_until_parked();
    changed
}
fn finish(view: &Entity<Newport>, cx: &mut VisualTestContext) -> usize {
    let mut notices = 0;
    for _ in 0..1000 {
        notices += usize::from(poll(view, cx));
        if view.read_with(cx, |view, _| view.file_transfer.is_none()) {
            return notices;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("transfer did not settle");
}

#[gpui_kit::test]
fn hidden_transfer_progress_is_retained_and_completion_is_delivered_once(cx: &mut TestAppContext) {
    let (view, cx, host, result, operation) = fixture(cx, Direction::Download);
    for (page, visible, completed, expected) in [
        (Page::Overview, true, 100, 0),
        (Page::Files, true, 200, 1),
        (Page::Files, false, 300, 0),
    ] {
        view.update(cx, |view, _| {
            view.page = page;
            view.window_visible = visible;
        });
        thread::sleep(Duration::from_millis(510));
        assert!(!poll(&view, cx));
        let (stream, request) = host.next("files_progress");
        assert_eq!(request.args["operation"], operation);
        reply(
            stream,
            Ok(json!({"name":"file.txt","completed":completed,"total":1000})),
        );
        let mut notices = 0;
        for _ in 0..1000 {
            notices += usize::from(poll(&view, cx));
            if view.read_with(cx, |view, _| {
                view.file_transfer
                    .as_ref()
                    .unwrap()
                    .progress()
                    .is_some_and(|progress| progress.completed == completed)
            }) {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            view.read_with(cx, |view, _| view
                .file_transfer
                .as_ref()
                .unwrap()
                .progress()
                .unwrap()
                .completed),
            completed
        );
        assert_eq!(notices, expected);
    }
    let notices = cx.new(|cx: &mut Context<usize>| {
        cx.observe(&view, |count, _, _| *count += 1).detach();
        0
    });
    view.update(cx, |view, cx| {
        view.window_visibility_changed(true, cx);
        view.window_visibility_changed(true, cx);
        view.page = Page::Overview;
    });
    cx.run_until_parked();
    assert_eq!(
        notices.read_with(cx, |count, _| *count),
        1,
        "restoring the window publishes retained state once"
    );
    // A cancellation acknowledgement may arrive after leaving Files. Retain
    // its error for Retry cancel without repainting the unrelated page.
    view.update(cx, |view, _| {
        view.page = Page::Files;
        assert!(view.cancel_file_transfer(&operation));
        view.page = Page::Overview;
    });
    let (cancel, request) = host.next("files_cancel");
    assert_eq!(request.args["operation"], operation);
    reply(cancel, Err("temporary cancellation failure"));
    for _ in 0..1000 {
        assert!(!poll(&view, cx));
        if view.read_with(cx, |view, _| {
            view.file_transfer
                .as_ref()
                .unwrap()
                .cancel_error()
                .is_some()
        }) {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(view.read_with(cx, |view, _| {
        view.file_transfer
            .as_ref()
            .unwrap()
            .cancel_error()
            .is_some()
    }));
    reply(result, Ok(json!("/tmp/unused-local-transfer-fixture")));
    assert_eq!(finish(&view, cx), 1);
    assert!(view.read_with(cx, |view, _| view.notification.is_some()));
    assert!(!poll(&view, cx));
}

#[gpui_kit::test]
fn completed_transfer_waits_for_configuration_revalidation(cx: &mut TestAppContext) {
    for recovery in ["same", "replacement", "deleted"] {
        let (view, cx, _host, result, _) = fixture(cx, Direction::Upload);
        let mut restored = view.update(cx, |view, _| {
            let snapshot = view.snapshot.clone();
            view.snapshot["loadError"] = json!("Vault temporarily unavailable");
            view.snapshot["config"]["servers"] = json!([]);
            snapshot
        });
        reply(result, Ok(json!("/remote/file.txt")));
        for _ in 0..30 {
            assert!(
                !poll(&view, cx),
                "unvalidated completion must remain retained"
            );
            thread::sleep(Duration::from_millis(1));
        }
        assert!(view.read_with(cx, |view, _| view.file_transfer.is_some()));
        assert!(view.read_with(cx, |view, _| view.notification.is_none()));
        if recovery == "replacement" {
            restored["runtime"]["connectionRevisions"]["fixture"] = json!(2);
        } else if recovery == "deleted" {
            restored["config"]["servers"] = json!([]);
        }
        view.update(cx, |view, _| view.snapshot = restored);
        assert_eq!(finish(&view, cx), 1);
        assert_eq!(
            view.read_with(cx, |view, _| view.notification.is_some()),
            recovery == "same"
        );
        assert!(!poll(&view, cx));
    }
}
