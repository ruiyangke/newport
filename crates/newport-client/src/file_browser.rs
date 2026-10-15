//! Scoped Files reads, independent of rendering and media preparation.
//! Each slot retains one physical read and only its newest desired replacement.
pub mod folders;

use crate::backend::Backend;
use serde_json::{Value, json};
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
            .launch(&self.backend, &self.server, &self.connection, "files_list");
        self.preview_reads.launch(
            &self.backend,
            &self.server,
            &self.connection,
            "files_preview",
        );
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
    operation: String,
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
            "files_cancel",
            json!({"operation":self.operation}),
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
    fn launch(
        &mut self,
        backend: &Backend,
        server: &str,
        connection: &Value,
        method: &'static str,
    ) {
        if self.active.is_some() {
            return;
        }
        let Some(intent) = self.desired.take() else {
            return;
        };
        let operation = uuid::Uuid::new_v4().to_string();
        let reply = dispatch(
            backend,
            method,
            json!({"id":server,"path":intent.path,"operation":operation,"expectedConnection":connection,"refresh":intent.refresh}),
            READ_TIMEOUT,
        );
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
    method: &'static str,
    args: Value,
    timeout: Duration,
) -> mpsc::Receiver<Reply> {
    let backend = backend.clone();
    let (send, receive) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let _ = send.send(backend.call_with_timeout(method, args, timeout));
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
    if matches!(value["kind"].as_str(), Some("text" | "image" | "pdf"))
        && value["content"].is_string()
        && value["mime"].is_string()
    {
        Ok(value)
    } else {
        Err("Invalid file preview from the backend.".into())
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
        time::Instant,
    };

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
                "server-a".into(),
                json!({"host":"original.example","port":22}),
            )
        }
        fn receive(&self) -> (Stream, Request) {
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
    fn reply(mut stream: Stream, result: Reply) {
        let (result, error) = match result {
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
    fn listing(path: &str) -> Value {
        json!({"path":path,"entries":[]})
    }
    fn preview(content: &str) -> Value {
        json!({"kind":"text","content":content,"mime":"text/plain"})
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
        assert_eq!(browser.server(), "server-a");
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
        assert!(browser.listing_error().unwrap().contains("not retried"));
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
    fn malformed_listing_retains_last_success_and_preview_shape_errors_are_scoped() {
        let host = Host::new();
        let mut browser = host.browser();
        loaded(&host, &mut browser, ".", "/kept");
        for value in [
            json!({"entries":[]}),
            json!({"path":"","entries":[]}),
            json!({"path":"/wrong","entries":null}),
        ] {
            browser.navigate("/bad".into());
            let (stream, _) = host.next("files_list");
            reply(stream, Ok(value));
            let updates = until(&mut browser, |browser| !browser.has_pending());
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
        let (read, _) = host.next("files_preview");
        reply(read, Ok(json!({"kind":"text","mime":"text/plain"})));
        until(&mut browser, |browser| !browser.has_pending());
        assert!(browser.preview().unwrap().error.is_some());
        browser.close_preview();
        assert!(browser.preview().is_none());
        host.quiet();
    }
}
