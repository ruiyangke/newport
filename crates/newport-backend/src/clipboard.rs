use crate::model::Server;
#[cfg(test)]
use crate::ssh::ExecSession;
#[cfg(test)]
use std::collections::BTreeMap;
use std::time::Duration;
#[allow(dead_code)]
#[path = "../../../tools/agent/src/clipboard_wire.rs"]
mod clipboard_wire;
#[allow(dead_code)]
#[path = "../../../tools/agent/src/gui_protocol.rs"]
mod gui_protocol;
use tokio::sync::watch;

#[derive(Default)]
struct GuiLaunches(
    std::collections::HashMap<uuid::Uuid, std::sync::Arc<std::sync::atomic::AtomicBool>>,
);
impl Drop for GuiLaunches {
    fn drop(&mut self) {
        for cancelled in self.0.values() {
            cancelled.store(true, std::sync::atomic::Ordering::Release);
        }
    }
}

const MAX_DATA: usize = 32 * 1024 * 1024;

#[cfg(test)]
fn archive(mut formats: BTreeMap<String, Vec<u8>>) -> Result<Vec<u8>, String> {
    let mut targets: Vec<String> = formats.keys().cloned().collect();
    for (alias, kind) in [
        ("UTF8_STRING", "text/plain"),
        ("STRING", "text/plain"),
        ("TEXT", "text/plain"),
        ("text/plain;charset=utf-8", "text/plain"),
        ("text/plain; charset=utf-8", "text/plain"),
        ("public.utf8-plain-text", "text/plain"),
        ("public.html", "text/html"),
        ("public.png", "image/png"),
        ("public.url", "text/uri-list"),
        ("public.file-url", "text/uri-list"),
    ] {
        if formats.contains_key(kind) {
            targets.push(alias.into());
        }
    }
    targets.push("TARGETS".into());
    formats.insert("TARGETS".into(), (targets.join("\n") + "\n").into_bytes());
    let mut builder = tar::Builder::new(Vec::new());
    for (kind, bytes) in formats {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o600);
        header.set_cksum();
        builder
            .append_data(&mut header, kind, bytes.as_slice())
            .map_err(|e| e.to_string())?;
    }
    builder.into_inner().map_err(|e| e.to_string())
}

struct ClipboardReply {
    request: clipboard_wire::Request,
    chunks: Option<std::collections::VecDeque<(u8, Vec<u8>)>>,
    permission_epoch: Option<u64>,
}
async fn on_main<T: Send + 'static>(
    app: &crate::desktop_integration::Desktop,
    action: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.on_main_thread(Box::new(move || {
        if !tx.is_closed() {
            let _ = tx.send(action());
        }
    }))?;
    tokio::time::timeout(Duration::from_secs(5), rx)
        .await
        .map_err(|_| "Timed out reading the clipboard".to_owned())?
        .map_err(|e| e.to_string())
}
#[cfg(test)]
async fn clipboard_offer(
    app: &crate::desktop_integration::Desktop,
    previous: Option<isize>,
) -> Result<Option<clipboard_wire::Offer>, String> {
    clipboard_offer_checked(app, previous, None).await
}
async fn clipboard_offer_checked(
    app: &crate::desktop_integration::Desktop,
    previous: Option<isize>,
    permissions: Option<std::sync::Arc<crate::integration_permissions::Gate>>,
) -> Result<Option<clipboard_wire::Offer>, String> {
    let ticket = permissions.as_ref().and_then(|p| p.clipboard_ticket());
    let desktop = app.clone();
    let result = on_main(app, move || {
        let read = || {
            desktop
                .clipboard_offer(previous)
                .map(|(revision, mut formats)| {
                    formats.retain(|s| clipboard_wire::valid_format(s) && s != "TARGETS");
                    formats.truncate(256);
                    log::info!(
                        "Clipboard offer revision={revision} formats={}",
                        formats.len()
                    );
                    clipboard_wire::Offer {
                        revision: revision as i64,
                        formats,
                    }
                })
        };
        match permissions {
            Some(gate) => ticket
                .and_then(|ticket| gate.clipboard_at(ticket, read))
                .flatten(),
            None => read(),
        }
    })
    .await;
    match result {
        Err(error) if error == "Timed out reading the clipboard" => {
            log::warn!("Clipboard metadata temporarily unavailable; retrying");
            Ok(None)
        }
        other => other,
    }
}
#[cfg(test)]
async fn read_requested(
    app: &crate::desktop_integration::Desktop,
    request: clipboard_wire::Request,
) -> ClipboardReply {
    read_requested_checked(app, request, None).await
}
async fn read_requested_checked(
    app: &crate::desktop_integration::Desktop,
    request: clipboard_wire::Request,
    permissions: Option<std::sync::Arc<crate::integration_permissions::Gate>>,
) -> ClipboardReply {
    let permission_epoch = permissions.as_ref().and_then(|p| p.clipboard_ticket());
    let revision = request.revision;
    let format = request.format.clone();
    let started = tokio::time::Instant::now();
    let desktop = app.clone();
    let bytes = on_main(app, move || {
        let read = || {
            let (current, formats) = desktop.clipboard_offer(None)?;
            if current as i64 != revision || !formats.contains(&format) {
                return None;
            }
            desktop
                .read_clipboard_format(current, &format)
                .filter(|b| b.len() <= MAX_DATA)
        };
        match permissions {
            Some(gate) => permission_epoch
                .and_then(|ticket| gate.clipboard_at(ticket, read))
                .flatten(),
            None => read(),
        }
    })
    .await
    .ok()
    .flatten();
    log::info!(
        "Clipboard request id={} revision={} success={} bytes={} elapsed_ms={}",
        request.id,
        revision,
        bytes.is_some(),
        bytes.as_ref().map_or(0, Vec::len),
        started.elapsed().as_millis()
    );
    let original_bytes = bytes.as_ref().map_or(0, Vec::len);
    // Compression runs off the UI and async event-loop threads.
    let chunks = match bytes {
        Some(bytes) => tokio::task::spawn_blocking(move || {
            if bytes.is_empty() {
                return Ok(std::collections::VecDeque::from([(0, Vec::new())]));
            }
            bytes
                .chunks(clipboard_wire::CHUNK)
                .map(clipboard_wire::compress_chunk)
                .collect::<std::io::Result<std::collections::VecDeque<_>>>()
        })
        .await
        .ok()
        .and_then(Result::ok),
        None => None,
    };
    if let Some(chunks) = &chunks {
        let transfer_bytes: usize = chunks.iter().map(|(_, bytes)| bytes.len()).sum();
        log::info!(
            "Clipboard transfer prepared id={} revision={} original_bytes={} transfer_bytes={} compressed_chunks={}",
            request.id,
            revision,
            original_bytes,
            transfer_bytes,
            chunks.iter().filter(|(status, _)| *status == 2).count()
        );
    }
    ClipboardReply {
        request,
        chunks,
        permission_epoch,
    }
}

// Stable per local profile so reconnects can identify their previous agent.
pub fn client_identity(
    directory: &std::path::Path,
    server: uuid::Uuid,
) -> Result<uuid::Uuid, String> {
    persisted_identity(directory, &format!("clipboard-client-{server}"))
}

pub(crate) fn routing_identity(directory: &std::path::Path) -> Result<uuid::Uuid, String> {
    persisted_identity(directory, "integration-routing-identity")
}

fn persisted_identity(directory: &std::path::Path, name: &str) -> Result<uuid::Uuid, String> {
    let path = directory.join(name);
    match std::fs::read_to_string(&path) {
        Ok(value) => uuid::Uuid::parse_str(value.trim()).map_err(|e| e.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            use std::io::Write;
            let id = uuid::Uuid::new_v4();
            // Concurrent starts (including replacement connection owners) can
            // request the same identity. Publish a complete file atomically;
            // no reader should see a created-but-not-yet-written UUID.
            let mut file = tempfile::NamedTempFile::new_in(directory).map_err(|e| e.to_string())?;
            file.write_all(id.to_string().as_bytes())
                .map_err(|e| e.to_string())?;
            file.as_file().sync_all().map_err(|e| e.to_string())?;
            match file.persist_noclobber(&path) {
                Ok(_) => Ok(id),
                Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let value = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
                    uuid::Uuid::parse_str(value.trim()).map_err(|e| e.to_string())
                }
                Err(error) => Err(error.to_string()),
            }
        }
        Err(error) => Err(error.to_string()),
    }
}

// Retry transport failures and agent-declared temporary failures (including
// exhausted disk space), and temporarily unavailable selected SSH agents.
// Permission, ownership and other authentication failures remain permanent. Each agent owns its files until the SSH channel closes or its lease expires.
#[cfg(test)]
async fn recover_connection<F, Fut>(
    attempt: F,
    cancel: watch::Receiver<bool>,
    retrying: impl Fn(),
    delay: Duration,
    changes: Option<watch::Receiver<Option<isize>>>,
) -> Result<(), String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    recover_service(attempt, cancel, retrying, delay, changes, None, |_| {}).await
}
async fn recover_service<F, Fut>(
    mut attempt: F,
    mut cancel: watch::Receiver<bool>,
    retrying: impl Fn(),
    delay: Duration,
    mut changes: Option<watch::Receiver<Option<isize>>>,
    owner: Option<&crate::server_session::ServerSession>,
    blocked: impl Fn(String),
) -> Result<(), String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let mut wait = delay;
    let mut recovery = crate::recovery::subscribe();
    loop {
        if *cancel.borrow() {
            return Ok(());
        }
        tokio::select! {
            biased;
            _ = cancel.changed() => return Ok(()),
            _ = recovery.ready() => {},
        }
        if let Some(owner) = owner {
            tokio::select! {
                biased;
                _ = cancel.changed() => return Ok(()),
                result = owner.follow_transport(|error| match error {
                    Some(error) => blocked(error),
                    None => retrying(),
                }) => result?,
            }
        }
        let generation = owner.map(|owner| owner.transport_snapshot().generation);
        let started = tokio::time::Instant::now();
        if let Some(changes) = &mut changes {
            changes.borrow_and_update();
        }
        let result = tokio::select! {
            biased;
            _ = cancel.changed() => return Ok(()),
            result = attempt() => result,
        };
        let Err(error) = result else { return Ok(()) };
        if error == crate::agent_runtime::UPGRADED {
            retrying();
            continue;
        }
        if let Some(owner) = owner
            && (!owner.has_transport() || Some(owner.transport_snapshot().generation) != generation)
        {
            // Transport recovery is owned by the server session. Retain this
            // service's demand, including when approval needs explicit Retry.
            // A newer transport also supersedes a late error from the old one.
            retrying();
            continue;
        }
        if let Some(owner) = owner {
            owner.agent_runtime.probe().await;
            let runtime = owner.agent_runtime.snapshot();
            if runtime.status == "failed" {
                if !runtime.retryable {
                    return Err(runtime.error.unwrap_or(error));
                }
                retrying();
                tokio::select! {
                    _ = cancel.changed() => return Ok(()),
                    result = owner.agent_runtime.wait_for_retry() => result?,
                }
                continue;
            }
        }
        let retryable = !error.starts_with("Remote command failed")
            && !error.starts_with("Agent error:")
            && (error == crate::agent_runtime::BUSY
                || error.starts_with("SSH session capacity unavailable")
                || error.contains("Needle runtime disconnected")
                || error.starts_with("Needle connection interrupted:")
                || error == "Needle control request timed out"
                || error == "Needle runtime control timed out"
                || error.contains("Needle runtime changed before worker registration")
                || error.contains("SSH transport interrupted:")
                || error.starts_with("SSH agent unavailable:")
                || error.contains("SSH command timed out")
                || error.contains("SSH connection or authentication timed out"));
        if !retryable {
            log::error!("Integration stopped after a non-retryable error");
            return Err(error);
        }
        log::warn!("Integration interrupted; reconnecting: {error}");
        retrying();
        // A stable session starts a new recovery cycle.
        if started.elapsed() >= Duration::from_secs(60) {
            wait = delay;
        }
        tokio::select! {
            _ = cancel.changed() => return Ok(()),
            _ = async {
                if let Some(changes) = &mut changes
                    && changes.changed().await.is_ok() {
                        return;
                }
                std::future::pending::<()>().await;
            } => {},
            _ = recovery.wait(wait) => {}
        }
        wait = (wait * 2).min(Duration::from_secs(30));
    }
}

pub async fn run(
    owner: std::sync::Arc<crate::server_session::ServerSession>,
    server: Server,
    client: uuid::Uuid,
    app: crate::desktop_integration::Desktop,
    cancel: watch::Receiver<bool>,
    ready: impl Fn(String, bool),
    waiting: impl Fn(Option<String>),
) -> Result<(), String> {
    let initial = if owner.permissions.clipboard_enabled() {
        revision(&app).await
    } else {
        None
    };
    let (send, receive) = watch::channel(initial);
    let recovery = recover_service(
        || {
            run_once(
                owner.clone(),
                server.clone(),
                client,
                app.clone(),
                cancel.clone(),
                &ready,
            )
        },
        cancel.clone(),
        || waiting(None),
        Duration::from_secs(2),
        Some(receive),
        Some(&owner),
        |error| waiting(Some(error)),
    );
    // Observe only the cheap revision counter; actual contents are read by
    // run_once after reconnecting. Keep this scoped to the integration session.
    let monitor = async {
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;
            if owner.permissions.clipboard_enabled()
                && let Some(count) = revision(&app).await
            {
                send.send_if_modified(|previous| {
                    if *previous == Some(count) {
                        return false;
                    }
                    *previous = Some(count);
                    log::info!("Clipboard change detected revision={count}");
                    true
                });
            }
        }
    };
    tokio::select! {
        result = recovery => result,
        _ = monitor => unreachable!(),
    }
}

async fn revision(app: &crate::desktop_integration::Desktop) -> Option<isize> {
    let desktop = app.clone();
    on_main(app, move || desktop.clipboard_revision())
        .await
        .ok()
        .flatten()
}

/// One authenticated SSH connection carries all updates. No HTTP listener or forwarding.
async fn run_once(
    owner: std::sync::Arc<crate::server_session::ServerSession>,
    server: Server,
    client: uuid::Uuid,
    app: crate::desktop_integration::Desktop,
    mut cancel: watch::Receiver<bool>,
    ready: impl Fn(String, bool),
) -> Result<(), String> {
    let _permission_session = owner.permissions.session();
    let session = tokio::select! {
        _ = cancel.changed() => return Ok(()),
        result = owner.acquire(&server) => result?,
    };
    let outcome = async {
        let (stream, installed) = session.launch(
            crate::agent::Service::Integration {
                client,
                clipboard: server.clipboard_enabled,
                browser: server.browser_enabled,
            },
        ).await?;
        let path_needed = installed
            .lines()
            .any(|line| line == "NEWPORT_SHIM_PATH=missing");
        let mut agent = crate::agent::Agent::start(
            stream,
            app.clone(),
            owner.permissions.clone(),
            session.callbacks(),
        )
        .await?;
        let mut changes = owner.permissions.subscribe();
        let desired = *changes.borrow_and_update();
        let permission_epoch = owner.permissions.snapshot().session_epoch;
        session.apply_permissions(permission_epoch, desired).await?;
        let gui_notices = app.supports_gui_notices() && session.supports_gui_notices();
        if gui_notices { agent.request(b'G', &[]).await?; }
        let gui_launch = app.supports_gui_notices() && session.supports_gui_launch();
        if gui_launch { agent.request(b'L', &[]).await?; }
        let mut gui_pending = GuiLaunches::default();
        let mut gui_replies = tokio::task::JoinSet::new();
        let mut last_gui_notice: Option<std::time::Instant> = None;
        ready("Agent connected.".into(), path_needed);
        log::info!(
            "Integration agent connected (clipboard={}, browser={})",
            server.clipboard_enabled,
            server.browser_enabled
        );
        let mut previous = None;
        let mut heartbeat = tokio::time::Instant::now();
        let mut timer = tokio::time::interval(Duration::from_millis(200));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut reads = tokio::task::JoinSet::new();
        let mut replies = std::collections::VecDeque::<ClipboardReply>::new();
        let traffic = async {
            loop {
                if *cancel.borrow() { break; }
                tokio::select! {
                    biased;
                    _ = cancel.changed() => break,
                    changed = changes.changed() => {
                        if changed.is_err() { break; }
                        let was_enabled = owner.permissions.clipboard_enabled();
                        if !was_enabled { reads.abort_all(); replies.clear(); previous = None; }
                        changes.borrow_and_update();
                    },
                    event = agent.event() => {
                        let (kind, data) = event?;
                        match kind {
                            b'O' => agent.open(&data).await?,
                            b'G' if gui_notices && data.is_empty() => {
                                session.validate()?;
                                if last_gui_notice.is_none_or(|at| at.elapsed() >= Duration::from_secs(3)) {
                                    last_gui_notice = Some(std::time::Instant::now());
                                    app.request_gui_setup(&server.id.to_string());
                                }
                            }
                            b'L' if gui_launch => {
                                session.validate()?;
                                let request: gui_protocol::Request = gui_protocol::decode(&data).map_err(|e| e.to_string())?;
                                request.validate().map_err(|e| e.to_string())?;
                                if gui_pending.0.len() >= gui_protocol::MAX_PENDING || gui_pending.0.contains_key(&request.id) {
                                    let reply = gui_protocol::Reply { id: request.id, environment: None,
                                        error: Some("Too many pending GUI launches. Try again.".into()) };
                                    agent.request(b'J', &gui_protocol::encode(&reply).map_err(|e| e.to_string())?).await?;
                                    continue;
                                }
                                let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                                gui_pending.0.insert(request.id, cancelled.clone());
                                let (reply, response) = tokio::sync::oneshot::channel();
                                app.request_gui_launch(&server.id.to_string(), &request.terminal.to_string(), &request.application,
                                    Box::new(move |result| { let _ = reply.send(result); }), cancelled);
                                gui_replies.spawn(async move {
                                    let result = tokio::time::timeout(Duration::from_secs(gui_protocol::WAIT_SECONDS), response).await
                                        .ok().and_then(Result::ok).unwrap_or_else(|| Err("GUI launch cancelled or timed out.".into()));
                                    (request.id, result)
                                });
                            }
                            b'K' if gui_launch => {
                                let id: uuid::Uuid = gui_protocol::decode(&data).map_err(|e| e.to_string())?;
                                if let Some(cancelled) = gui_pending.0.remove(&id) {
                                    cancelled.store(true, std::sync::atomic::Ordering::Release);
                                }
                            }
                            b'C' => {
                                let request = clipboard_wire::Request::decode(&data).map_err(|e| e.to_string())?;
                                if !owner.permissions.clipboard_enabled() || reads.len() + replies.len() >= 8 {
                                    agent.clipboard_reply(&clipboard_wire::reply(&request, 1, true, &[])).await?;
                                } else {
                                    let app = app.clone();
                                    let permissions = owner.permissions.clone();
                                    reads.spawn(async move { read_requested_checked(&app, request, Some(permissions)).await });
                                }
                            }
                            _ => return Err("Unexpected agent event.".into()),
                        }
                    }
                    result = reads.join_next(), if !reads.is_empty() => {
                        match result.unwrap() {
                            Ok(reply) if owner.permissions.clipboard_enabled() => replies.push_back(reply),
                            Ok(_) => {},
                            Err(error) if error.is_cancelled() => {},
                            Err(error) => return Err(error.to_string()),
                        }
                    }
                    result = gui_replies.join_next(), if !gui_replies.is_empty() => {
                        if let Some(Ok((id, result))) = result
                            && let Some(cancelled) = gui_pending.0.remove(&id)
                        {
                            session.validate()?;
                            let result = result.and_then(|environment| environment.validate().map(|()| environment));
                            let (environment, error) = match result { Ok(env) => (Some(env.0), None), Err(error) => (None, Some(error)) };
                            agent.request(b'J', &gui_protocol::encode(&gui_protocol::Reply { id, environment, error })
                                .map_err(|e| e.to_string())?).await?;
                            cancelled.store(true, std::sync::atomic::Ordering::Release);
                        }
                    }
                    // One bounded chunk per turn lets events and timers run between writes.
                    _ = tokio::task::yield_now(), if !replies.is_empty() => {
                        let mut reply = replies.pop_front().unwrap();
                        let current = revision(&app).await;
                        if current != Some(reply.request.revision as isize)
                            || reply.permission_epoch != owner.permissions.clipboard_ticket()
                            || !owner.permissions.clipboard_enabled() { reply.chunks = None; }
                        let (status, bytes) = reply.chunks.as_mut().and_then(|chunks| chunks.pop_front()).unwrap_or((1, Vec::new()));
                        let done = reply.chunks.as_ref().is_none_or(|chunks| chunks.is_empty());
                        agent.clipboard_reply(&clipboard_wire::reply(&reply.request, status, done, &bytes)).await?;
                        if !done { replies.push_back(reply); }
                    }
                    _ = timer.tick() => {
                        if owner.permissions.clipboard_enabled() && reads.is_empty()
                            && let Some(offer) = clipboard_offer_checked(&app, previous, Some(owner.permissions.clone())).await? {
                                let revision = offer.revision;
                                let backend = agent.request(b'M', &offer.encode()).await?;
                                previous = Some(revision as isize);
                                heartbeat = tokio::time::Instant::now();
                                ready(backend, path_needed);
                        }
                        if heartbeat.elapsed() >= Duration::from_secs(5) {
                            agent.request(b'H', &[]).await?;
                            heartbeat = tokio::time::Instant::now();
                        }
                    }
                }
            }
            Ok(())
        };
        let outcome = tokio::select! {
            _ = session.wait_for_upgrade() => Err(crate::agent_runtime::UPGRADED.into()),
            result = traffic => result,
            result = session.watch_permissions(permission_epoch, || ready("Agent permissions updated.".into(), path_needed)) => result,
        };
        reads.abort_all();
        agent.close().await;
        outcome
    }
    .await;
    owner.permissions.disconnected(
        outcome
            .as_ref()
            .err()
            .filter(|e| e.as_str() != crate::agent_runtime::UPGRADED)
            .cloned(),
    );
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop_integration::{Desktop, DesktopIntegration};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering},
    };

    #[derive(Default)]
    struct TestDesktop {
        queue: AtomicBool,
        actions: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
        current: AtomicIsize,
        reads: AtomicUsize,
    }
    impl DesktopIntegration for TestDesktop {
        fn on_main_thread(&self, action: Box<dyn FnOnce() + Send>) -> Result<(), String> {
            if self.queue.load(Ordering::SeqCst) {
                self.actions.lock().unwrap().push(action);
            } else {
                action();
            }
            Ok(())
        }
        fn clipboard_revision(&self) -> Option<isize> {
            Some(self.current.load(Ordering::SeqCst))
        }
        fn clipboard_offer(&self, previous: Option<isize>) -> Option<(isize, Vec<String>)> {
            let current = self.clipboard_revision()?;
            (previous != Some(current)).then(|| {
                (
                    current,
                    vec![
                        "text/plain".into(),
                        "TARGETS".into(),
                        "invalid\nformat".into(),
                    ],
                )
            })
        }
        fn read_clipboard_format(&self, revision: isize, format: &str) -> Option<Vec<u8>> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            (self.clipboard_revision() == Some(revision) && format == "text/plain")
                .then(|| b"fixture".to_vec())
        }
        fn open_url(&self, _: &str) -> Result<(), String> {
            Err("Browser not enabled in this fixture".into())
        }
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- clipboard::tests::managed_bridge"]
    async fn managed_bridge_updates_permissions_without_reopening_channel() {
        let server: Server = serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::new_v4(), "name":"Managed bridge fixture",
            "sshUser":"fixture", "sshHost":"127.0.0.1",
            "sshPort":std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
            "identityFile":std::env::var("NEWPORT_TEST_IDENTITY").unwrap(),
            "clipboardEnabled":false, "browserEnabled":true
        }))
        .unwrap();
        let registry = crate::server_session::Registry::default();
        let owner = registry.session(&server, 0);
        let adapter = Arc::new(TestDesktop::default());
        adapter.current.store(42, Ordering::SeqCst);
        let (stop, cancel) = watch::channel(false);
        let task_owner = owner.clone();
        let task_server = server.clone();
        let (ready, mut changed) = watch::channel(0u64);
        let task = tokio::spawn(async move {
            run_once(
                task_owner,
                task_server,
                uuid::Uuid::new_v4(),
                adapter,
                cancel,
                move |_, _| {
                    ready.send_modify(|count| *count += 1);
                },
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(10), changed.changed())
            .await
            .unwrap()
            .unwrap();
        assert!(owner.permissions.browser_enabled());
        assert!(!owner.permissions.clipboard_enabled());
        owner.permissions.update(true, true);
        tokio::time::timeout(Duration::from_secs(10), async {
            while !owner.permissions.clipboard_enabled() {
                changed.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        let connection = owner.acquire(&server).await.unwrap();
        // Polling is only for metadata publication after permission acknowledgement.
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if connection
                    .execute("fixture-clipboard", None)
                    .await
                    .as_deref()
                    == Ok("fixture")
                {
                    break;
                }
                changed.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        owner.permissions.update(false, true);
        tokio::time::timeout(Duration::from_secs(5), async {
            while owner.permissions.snapshot().effective.revision
                != owner.permissions.snapshot().desired.revision
            {
                changed.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        assert!(connection.execute("fixture-clipboard", None).await.is_err());
        assert!(owner.permissions.browser_enabled());
        assert_eq!(connection.execute("printf ok", None).await.unwrap(), "ok");
        assert!(
            !task.is_finished(),
            "the same bridge remains running after revocation"
        );
        stop.send_replace(true);
        assert!(
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap()
                .is_ok()
        );
        assert!(!owner.permissions.browser_enabled());
        registry.shutdown().await;
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- clipboard::tests::sharing_resumes"]
    async fn sharing_resumes_after_another_feature_recovers_failed_authentication() {
        let directory = tempfile::tempdir().unwrap();
        let identity = directory.path().join("identity");
        let server: Server = serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::new_v4(), "name": "Sharing recovery fixture",
            "sshUser": "fixture", "sshHost": "127.0.0.1", "browserEnabled": true,
            "sshPort": std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
            "identityFile": identity
        }))
        .unwrap();
        let mut manager =
            crate::manager::Manager::new(crate::profile::Store::for_test(directory.path().into()));
        manager.config.servers.push(server.clone());
        let owner = manager.sessions.session(&server, 0);
        let connection_updates = manager.sessions.connections.subscribe();
        manager
            .prepare_integration(server.id)
            .unwrap()
            .run(Arc::new(TestDesktop::default()))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while owner.transport_snapshot().status != "failed" {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(!owner.transport_snapshot().retryable);
        assert_eq!(
            connection_updates.borrow().servers[&server.id].phase,
            newport_domain::connectivity::Phase::Failed
        );
        assert!(!owner.permissions.browser_enabled());
        assert!(
            owner
                .integration
                .snapshot()
                .unwrap()
                .connection
                .error_message
                .is_none(),
            "sharing must not retain a second copy of the global SSH error"
        );

        // Restore authentication, then retry from a different SSH consumer.
        // Sharing must recover without a preference toggle or its own restart.
        std::fs::copy(std::env::var("NEWPORT_TEST_IDENTITY").unwrap(), &identity).unwrap();
        owner.retry_transport_fresh();
        let lease = owner.acquire(&server).await.unwrap();
        assert_eq!(
            connection_updates.borrow().servers[&server.id].phase,
            newport_domain::connectivity::Phase::Connected
        );
        assert_eq!(lease.execute("printf ok", None).await.unwrap(), "ok");
        let recovered = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let snapshot = manager.snapshot();
                if snapshot.runtime.clipboard[&server.id].status == crate::model::Status::Connected
                    && owner.permissions.browser_enabled()
                {
                    assert!(
                        snapshot.runtime.clipboard[&server.id]
                            .error_message
                            .is_none()
                    );
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        manager.shutdown().await;
        recovered.expect("sharing must follow shared SSH recovery after a permanent auth failure");
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- clipboard::tests::integration_owner"]
    async fn integration_owner_deduplicates_start_and_isolates_retired_status() {
        let directory = tempfile::tempdir().unwrap();
        let server: Server = serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::new_v4(), "name": "Integration owner fixture",
            "sshUser": "fixture", "sshHost": "127.0.0.1", "browserEnabled": true,
            "sshPort": std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
            "identityFile": std::env::var("NEWPORT_TEST_IDENTITY").unwrap()
        }))
        .unwrap();
        let mut manager =
            crate::manager::Manager::new(crate::profile::Store::for_test(directory.path().into()));
        manager.config.servers.push(server.clone());
        let owner = manager.sessions.session(&server, 0);
        let adapter: Desktop = Arc::new(TestDesktop::default());
        manager
            .prepare_integration(server.id)
            .unwrap()
            .run(adapter.clone())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            while !owner.permissions.browser_enabled() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            manager.snapshot().runtime.clipboard[&server.id].status,
            crate::model::Status::Connected
        );
        let epoch = owner.permissions.snapshot().session_epoch;
        let runtime = owner.agent_runtime.snapshot().hello.unwrap().instance_id;
        manager
            .prepare_integration(server.id)
            .unwrap()
            .run(adapter.clone())
            .await
            .unwrap();
        assert_eq!(owner.permissions.snapshot().session_epoch, epoch);
        let transport = owner.acquire(&server).await.unwrap();
        owner.permissions.disconnected(None);
        owner.integration.stop().await;
        assert!(!owner.permissions.browser_enabled());
        assert_eq!(transport.execute("printf ok", None).await.unwrap(), "ok");
        assert_eq!(
            owner.agent_runtime.snapshot().hello.unwrap().instance_id,
            runtime
        );

        manager
            .prepare_integration(server.id)
            .unwrap()
            .run(adapter)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            while !owner.permissions.browser_enabled() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let weak = Arc::downgrade(&owner);
        manager.advance_connection_revision(server.id);
        let replacement = manager
            .sessions
            .session(&server, manager.connection_revision(server.id));
        assert!(
            !manager
                .snapshot()
                .runtime
                .clipboard
                .contains_key(&server.id)
        );
        // Simulate a late observer from the retired task; only its old owner can change.
        owner
            .integration
            .set_state(crate::model::ConnectionState::error("obsolete error"));
        assert!(
            !manager
                .snapshot()
                .runtime
                .clipboard
                .contains_key(&server.id)
        );
        assert!(replacement.integration.snapshot().is_none());
        drop(transport);
        drop(owner);
        tokio::time::timeout(Duration::from_secs(5), async {
            while weak.upgrade().is_some() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("retirement must release the task and owner");
        manager.shutdown().await;
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- clipboard::tests::runtime_loss"]
    async fn runtime_loss_recovers_bridge_and_invalidates_git_without_reconnecting_ssh() {
        let directory = tempfile::tempdir().unwrap();
        let server: Server = serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::new_v4(), "name": "Runtime recovery fixture", "sshUser":"fixture",
            "sshHost":"127.0.0.1", "browserEnabled":true,
            "sshPort":std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
            "identityFile": std::env::var("NEWPORT_TEST_IDENTITY").unwrap()
        }))
        .unwrap();
        let mut manager =
            crate::manager::Manager::new(crate::profile::Store::for_test(directory.path().into()));
        manager.config.servers.push(server.clone());
        let owner = manager.sessions.session(&server, 0);
        manager
            .prepare_integration(server.id)
            .unwrap()
            .run(Arc::new(TestDesktop::default()))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            while !owner.permissions.browser_enabled() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        for failure in 1..=2 {
            let runtime = owner.agent_runtime.snapshot().hello.unwrap().instance_id;
            let epoch = owner.permissions.snapshot().session_epoch;
            let git = owner.acquire(&server).await.unwrap();
            let (worker, _) = git.launch(crate::agent::Service::Git).await.unwrap();
            let started = tokio::time::Instant::now();
            owner.agent_runtime.stop_remote_for_test().await;
            tokio::time::timeout(Duration::from_secs(15), async {
                loop {
                    let snapshot = owner.agent_runtime.snapshot();
                    if snapshot
                        .hello
                        .as_ref()
                        .is_some_and(|h| h.instance_id != runtime)
                        && snapshot.status == "ready"
                        && owner.permissions.browser_enabled()
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("live integration must recover the runtime without user retry");
            assert!(
                started.elapsed()
                    >= Duration::from_millis(if failure == 1 { 1_590 } else { 3_190 })
            );
            assert!(git.is_closed());
            drop(worker);
            assert!(owner.permissions.snapshot().session_epoch > epoch);
            assert_eq!(owner.agent_runtime.snapshot().failures, failure);
            assert_eq!(owner.transport_snapshot().generation, 1);
            let fresh = owner.acquire(&server).await.unwrap();
            assert_eq!(fresh.execute("printf ok", None).await.unwrap(), "ok");
            fresh.launch(crate::agent::Service::Git).await.unwrap();
        }
        manager.shutdown().await;
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- clipboard::tests::reinstall_command"]
    async fn reinstall_command_does_not_interrupt_a_healthy_bridge_on_failure() {
        let directory = tempfile::tempdir().unwrap();
        let server: Server = serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::new_v4(), "name": "Reinstall fixture", "sshUser": "fixture",
            "sshHost": "127.0.0.1", "browserEnabled": true,
            "sshPort": std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
            "identityFile": std::env::var("NEWPORT_TEST_IDENTITY").unwrap(), "agentInstallApproved": false
        })).unwrap();
        let mut manager =
            crate::manager::Manager::new(crate::profile::Store::for_test(directory.path().into()));
        manager.config.servers.push(server.clone());
        let owner = manager.sessions.session(&server, 0);
        let adapter: Desktop = Arc::new(TestDesktop::default());
        manager
            .prepare_integration(server.id)
            .unwrap()
            .run(adapter.clone())
            .await
            .unwrap();
        let state = crate::manager::share(manager);
        tokio::time::timeout(Duration::from_secs(10), async {
            while !owner.permissions.browser_enabled() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let epoch = owner.permissions.snapshot().session_epoch;
        let instance = owner.agent_runtime.snapshot().hello.unwrap().instance_id;
        let error = crate::commands::reinstall_agent(&state, adapter, server.id)
            .await
            .unwrap_err();
        assert!(error.contains("approval"), "{error}");
        assert!(owner.permissions.browser_enabled());
        assert_eq!(owner.permissions.snapshot().session_epoch, epoch);
        assert_eq!(
            owner.agent_runtime.snapshot().hello.unwrap().instance_id,
            instance
        );
        assert_eq!(owner.transport_snapshot().generation, 1);
        crate::manager::shutdown(&state).await;
    }

    #[tokio::test]
    async fn queued_clipboard_read_cannot_cross_disable_and_reenable() {
        use crate::{agent::control::Applied, integration_permissions::Gate};
        let adapter = Arc::new(TestDesktop::default());
        adapter.current.store(42, Ordering::SeqCst);
        adapter.queue.store(true, Ordering::SeqCst);
        let desktop: Desktop = adapter.clone();
        let gate = Arc::new(Gate::new(true, true));
        gate.acknowledge(Applied {
            permissions: gate.snapshot().desired,
            error: None,
        });
        let task_gate = gate.clone();
        let task = tokio::spawn(async move {
            read_requested_checked(
                &desktop,
                clipboard_wire::Request {
                    id: 1,
                    revision: 42,
                    format: "text/plain".into(),
                },
                Some(task_gate),
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while adapter.actions.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        gate.update(false, true);
        gate.update(true, true);
        gate.acknowledge(Applied {
            permissions: gate.snapshot().desired,
            error: None,
        });
        for action in std::mem::take(&mut *adapter.actions.lock().unwrap()) {
            action();
        }
        assert!(task.await.unwrap().chunks.is_none());
        assert_eq!(adapter.reads.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn desktop_adapter_filters_metadata_and_refuses_stale_clipboard_reads() {
        let adapter = Arc::new(TestDesktop::default());
        adapter.current.store(42, Ordering::SeqCst);
        let desktop: Desktop = adapter.clone();
        let offer = clipboard_offer(&desktop, None).await.unwrap().unwrap();
        assert_eq!(offer.revision, 42);
        assert_eq!(offer.formats, ["text/plain"]);
        assert!(clipboard_offer(&desktop, Some(42)).await.unwrap().is_none());
        for (revision, format) in [(41, "text/plain"), (42, "image/png")] {
            let reply = read_requested(
                &desktop,
                clipboard_wire::Request {
                    id: 1,
                    revision,
                    format: format.into(),
                },
            )
            .await;
            assert!(reply.chunks.is_none());
        }
        assert_eq!(adapter.reads.load(Ordering::SeqCst), 0);
        let reply = read_requested(
            &desktop,
            clipboard_wire::Request {
                id: 2,
                revision: 42,
                format: "text/plain".into(),
            },
        )
        .await;
        assert!(reply.chunks.is_some());
        assert_eq!(adapter.reads.load(Ordering::SeqCst), 1);
        assert_eq!(revision(&desktop).await, Some(42));
    }

    #[tokio::test(start_paused = true)]
    async fn timed_out_desktop_dispatch_never_reads_clipboard_later() {
        let adapter = Arc::new(TestDesktop::default());
        adapter.queue.store(true, Ordering::SeqCst);
        let desktop: Desktop = adapter.clone();
        let read = tokio::spawn(async move {
            read_requested(
                &desktop,
                clipboard_wire::Request {
                    id: 1,
                    revision: 0,
                    format: "text/plain".into(),
                },
            )
            .await
        });
        tokio::task::yield_now().await;
        assert_eq!(adapter.actions.lock().unwrap().len(), 1);
        tokio::time::advance(Duration::from_secs(6)).await;
        assert!(read.await.unwrap().chunks.is_none());
        let action = adapter.actions.lock().unwrap().pop().unwrap();
        action();
        assert_eq!(adapter.reads.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn recovery_continues_beyond_request_deadline() {
        let (_tx, rx) = watch::channel(false);
        let mut attempts = 0;
        let started = tokio::time::Instant::now();
        recover_connection(
            || {
                attempts += 1;
                std::future::ready(if attempts < 7 {
                    Err("SSH transport interrupted: Disconnected".into())
                } else {
                    Ok(())
                })
            },
            rx,
            || {},
            Duration::from_secs(2),
            None,
        )
        .await
        .unwrap();
        assert_eq!(attempts, 7);
        // 2 + 4 + 8 + 16 + 30 + 30: no total deadline, capped backoff.
        assert_eq!(started.elapsed(), Duration::from_secs(90));
    }

    #[tokio::test]
    async fn timeout_recovery_retries_but_preserves_permanent_errors() {
        let (_tx, rx) = watch::channel(false);
        let mut attempts = 0;
        recover_connection(
            || {
                attempts += 1;
                std::future::ready(if attempts == 1 {
                    Err("Clipboard upload failed: SSH command timed out after 25 seconds".into())
                } else {
                    Ok(())
                })
            },
            rx.clone(),
            || {},
            Duration::ZERO,
            None,
        )
        .await
        .unwrap();
        assert_eq!(attempts, 2);

        let mut attempts = 0;
        recover_connection(
            || {
                attempts += 1;
                std::future::ready(if attempts < 6 {
                    Err("SSH transport interrupted: Connection reset by peer".into())
                } else {
                    Ok(())
                })
            },
            rx.clone(),
            || {},
            Duration::ZERO,
            None,
        )
        .await
        .unwrap();
        assert_eq!(attempts, 6);

        let mut attempts = 0;
        let error = recover_connection(
            || {
                attempts += 1;
                std::future::ready(Err(
                    "Clipboard sync is already active for this server account.".into(),
                ))
            },
            rx,
            || panic!("Ownership errors must not retry"),
            Duration::ZERO,
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(attempts, 1);
        assert!(error.contains("already active"));
    }

    #[tokio::test]
    async fn copy_during_failed_attempt_is_not_lost() {
        let (_cancel, rx) = watch::channel(false);
        let (changes, revisions) = watch::channel(Some(1));
        let mut attempts = 0;
        tokio::time::timeout(
            Duration::from_millis(200),
            recover_connection(
                || {
                    attempts += 1;
                    std::future::ready(if attempts == 1 {
                        changes.send(Some(2)).unwrap();
                        Err("SSH transport interrupted: No space left on device".into())
                    } else {
                        Ok(())
                    })
                },
                rx,
                || {},
                Duration::from_secs(30),
                Some(revisions),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(attempts, 2);
    }

    #[tokio::test]
    async fn new_copy_wakes_backoff_once_without_repeating_unchanged_copies() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let (_cancel, rx) = watch::channel(false);
        let (changes, revisions) = watch::channel(Some(1));
        let attempts = Arc::new(AtomicUsize::new(0));
        let count = attempts.clone();
        let recovery = recover_connection(
            move || {
                let attempt = count.fetch_add(1, Ordering::SeqCst) + 1;
                std::future::ready(if attempt < 3 {
                    Err("SSH transport interrupted: No space left on device".into())
                } else {
                    Ok(())
                })
            },
            rx,
            || {},
            Duration::from_secs(30),
            Some(revisions),
        );
        tokio::pin!(recovery);
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut recovery)
                .await
                .is_err()
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        changes.send(Some(2)).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut recovery)
                .await
                .is_err()
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut recovery)
                .await
                .is_err()
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        changes.send(Some(3)).unwrap();
        tokio::time::timeout(Duration::from_millis(200), &mut recovery)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn disk_full_agent_retries_until_space_is_available() {
        let (_tx, rx) = watch::channel(false);
        let mut attempts = 0;
        recover_connection(
            || {
                attempts += 1;
                std::future::ready(if attempts < 4 {
                    Err(crate::agent::checked_event(
                        b'T',
                        b"No space left on device (os error 28) at path /clipboard/.tmp".to_vec(),
                    )
                    .unwrap_err())
                } else {
                    Ok(())
                })
            },
            rx,
            || {},
            Duration::ZERO,
            None,
        )
        .await
        .unwrap();
        assert_eq!(attempts, 4);
    }

    #[tokio::test]
    async fn disabling_sync_cancels_pending_recovery() {
        let (tx, rx) = watch::channel(false);
        let mut attempts = 0;
        recover_connection(
            || {
                attempts += 1;
                std::future::ready(Err("SSH command timed out after 25 seconds".into()))
            },
            rx,
            || {
                tx.send(true).unwrap();
            },
            Duration::from_secs(60),
            None,
        )
        .await
        .unwrap();
        assert_eq!(attempts, 1);
    }

    #[tokio::test]
    async fn disabling_sync_cancels_an_inflight_attempt_without_waiting_for_timeout() {
        let (tx, rx) = watch::channel(false);
        let run = recover_connection(
            std::future::pending::<Result<(), String>>,
            rx,
            || panic!("Cancellation must not retry"),
            Duration::from_secs(60),
            None,
        );
        let cancel = async {
            tokio::task::yield_now().await;
            tx.send(true).unwrap();
        };
        let (result, _) = tokio::time::timeout(Duration::from_millis(200), async {
            tokio::join!(run, cancel)
        })
        .await
        .expect("Cancellation must interrupt in-flight work");
        result.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn needle_connection_losses_and_control_timeouts_recover_automatically() {
        for error in [
            "SSH session capacity unavailable. The server refused another channel",
            "Needle connection interrupted: early eof",
            "Needle control request timed out",
            "Needle runtime control timed out",
        ] {
            let (_tx, rx) = watch::channel(false);
            let mut attempts = 0;
            let started = tokio::time::Instant::now();
            recover_connection(
                || {
                    attempts += 1;
                    std::future::ready(if attempts < 13 {
                        Err(error.into())
                    } else {
                        Ok(())
                    })
                },
                rx,
                || {},
                Duration::from_secs(2),
                None,
            )
            .await
            .unwrap();
            assert_eq!(attempts, 13);
            assert_eq!(started.elapsed(), Duration::from_secs(270));
        }
    }

    #[tokio::test]
    async fn selected_ssh_agent_recovers_without_reenabling_sync() {
        let (_tx, rx) = watch::channel(false);
        let mut attempts = 0;
        recover_connection(
            || {
                attempts += 1;
                std::future::ready(if attempts == 1 {
                    Err("SSH agent unavailable: Unlock the agent".into())
                } else {
                    Ok(())
                })
            },
            rx,
            || {},
            Duration::ZERO,
            None,
        )
        .await
        .unwrap();
        assert_eq!(attempts, 2);
    }

    #[tokio::test]
    async fn permanent_agent_failures_do_not_retry_even_with_transport_words() {
        let (_tx, rx) = watch::channel(false);
        let error = recover_connection(
            || {
                std::future::ready(Err(
                    "Agent error: SSH transport interrupted: disk is read-only".into(),
                ))
            },
            rx,
            || panic!("Permanent agent errors must remain visible"),
            Duration::ZERO,
            None,
        )
        .await
        .unwrap_err();
        assert!(error.starts_with("Agent error:"));
    }
    #[test]
    fn client_identity_survives_restart_and_isolates_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let server = uuid::Uuid::new_v4();
        let first = client_identity(dir.path(), server).unwrap();
        assert_eq!(first, client_identity(dir.path(), server).unwrap());
        assert_ne!(
            first,
            client_identity(dir.path(), uuid::Uuid::new_v4()).unwrap()
        );
    }

    #[test]
    fn concurrent_identity_creation_publishes_one_complete_uuid() {
        let dir = tempfile::tempdir().unwrap();
        let server = uuid::Uuid::new_v4();
        let ready = std::sync::Barrier::new(16);
        let identities = std::thread::scope(|scope| {
            let tasks: Vec<_> = (0..16)
                .map(|_| {
                    scope.spawn(|| {
                        ready.wait();
                        client_identity(dir.path(), server).unwrap()
                    })
                })
                .collect();
            tasks
                .into_iter()
                .map(|t| t.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert!(identities.iter().all(|id| *id == identities[0]));
        assert_eq!(client_identity(dir.path(), server).unwrap(), identities[0]);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh and its isolated remote HOME"]
    async fn ssh_agent_snapshot_browser_and_cleanup() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let server = Server {
            id: uuid::Uuid::new_v4(),
            name: "Agent fixture".into(),
            ssh_user: "fixture".into(),
            ssh_host: "127.0.0.1".into(),
            ssh_port: std::env::var("NEWPORT_TEST_SSH_PORT")
                .unwrap()
                .parse()
                .unwrap(),
            identity_file: Some(std::env::var("NEWPORT_TEST_IDENTITY").unwrap()),
            agent_source: None,
            agent_key_fingerprint: None,
            auth_method: crate::model::AuthMethod::PublicKey,
            clipboard_enabled: false,
            browser_enabled: false,
            agent_install_approved: true,
            setup_complete: true,
        };
        let session = ExecSession::connect(&server).await.unwrap();
        let mut stream = session.stream("fixture-agent").await.unwrap();
        async fn event(stream: &mut (impl tokio::io::AsyncRead + Unpin), kind: u8) -> Vec<u8> {
            let mut header = [0; 5];
            tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut header))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(header[0], b'M');
            let mut data =
                vec![0; crate::agent::wire::payload_length(&header, 1024 * 1024).unwrap()];
            stream.read_exact(&mut data).await.unwrap();
            let (actual, data) = crate::agent::wire::decode(&data, 1024 * 1024).unwrap();
            assert_eq!(actual, kind);
            data
        }
        assert_eq!(event(&mut stream, b'R').await, b"newport-agent/1");
        let bytes = archive(BTreeMap::from([(
            "text/plain".into(),
            "SSH clipboard 世界\n".as_bytes().to_vec(),
        )]))
        .unwrap();
        let frame = crate::agent::wire::encode(b'S', &bytes).unwrap();
        stream.write_all(&frame).await.unwrap();
        event(&mut stream, b'A').await;
        assert_eq!(
            session.execute("fixture-clipboard", None).await.unwrap(),
            "SSH clipboard 世界\n"
        );
        let (opened, ()) = tokio::join!(session.execute("fixture-open", None), async {
            assert_eq!(
                crate::agent::wire::parse_browser_request(&event(&mut stream, b'O').await).unwrap(),
                (1, "https://example.com/login?code=fixture".into())
            );
            stream
                .write_all(
                    &crate::agent::wire::encode(
                        b'B',
                        &crate::agent::wire::browser_reply(1, true, None).unwrap(),
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
        });
        opened.unwrap();
        stream
            .write_all(&crate::agent::wire::encode(b'Q', &[]).unwrap())
            .await
            .unwrap();
        let mut end = Vec::new();
        stream.read_to_end(&mut end).await.unwrap();
        assert!(session.execute("fixture-clipboard", None).await.is_err());
        session.close().await;
    }
    mod remote_tests {
        use super::*;
        #[tokio::test]
        #[ignore = "Requires disposable Linux OpenSSH fixture"]
        async fn approved_upgrade_migrates_bridge_and_monitor_without_replacing_ssh() {
            let directory = tempfile::tempdir().unwrap();
            let server: Server = serde_json::from_value(serde_json::json!({
                "id": uuid::Uuid::new_v4(), "name": "Upgrade fixture", "sshUser":"fixture",
                "sshHost":"127.0.0.1", "browserEnabled":true, "agentInstallApproved": true,
                "sshPort":std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
                "identityFile": std::env::var("NEWPORT_TEST_IDENTITY").unwrap()
            }))
            .unwrap();
            let setup = ExecSession::connect(&server).await.unwrap();
            crate::agent_lifecycle::manage_test(&setup, false, false)
                .await
                .unwrap();
            let old_hash = crate::agent_lifecycle::activate_variant_test(&setup).await;
            let mut manager = crate::manager::Manager::new(crate::profile::Store::for_test(
                directory.path().into(),
            ));
            manager.config.servers.push(server.clone());
            let owner = manager.sessions.session(&server, 0);
            crate::monitor::snapshot(&owner).await.unwrap();
            assert_eq!(
                owner.agent_runtime.snapshot().hello.unwrap().build_sha256,
                old_hash
            );
            manager
                .prepare_integration(server.id)
                .unwrap()
                .run(Arc::new(TestDesktop::default()))
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(10), async {
                while !owner.permissions.browser_enabled() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            let old_ticket = owner.permissions.browser_ticket().unwrap();
            let old_epoch = owner.permissions.snapshot().session_epoch;
            owner
                .acquire(&server)
                .await
                .unwrap()
                .install(false)
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(10), async {
                while owner.permissions.snapshot().session_epoch == old_epoch
                    || !owner.permissions.browser_enabled()
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            assert!(owner.permissions.browser_at(old_ticket, || ()).is_none());
            assert!(owner.permissions.snapshot().desired.browser);
            assert!(!owner.permissions.snapshot().desired.clipboard);
            assert!(owner.permissions.snapshot().error.is_none());
            crate::monitor::snapshot(&owner).await.unwrap();
            let current = owner.agent_runtime.snapshot();
            assert_ne!(current.hello.unwrap().build_sha256, old_hash);
            assert!(current.instances.iter().all(|i| !i.retiring));
            assert_eq!(owner.transport_snapshot().generation, 1);
            manager.shutdown().await;
            setup.close().await;
        }
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- clipboard::tests"]
    async fn retiring_integration_keeps_permissions_when_new_runtime_fails() {
        let directory = tempfile::tempdir().unwrap();
        let server: Server = serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::new_v4(), "name": "Runtime rotation fixture", "sshUser":"fixture",
            "sshHost":"127.0.0.1", "browserEnabled":true,
            "sshPort":std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
            "identityFile": std::env::var("NEWPORT_TEST_IDENTITY").unwrap()
        }))
        .unwrap();
        let mut manager =
            crate::manager::Manager::new(crate::profile::Store::for_test(directory.path().into()));
        manager.config.servers.push(server.clone());
        let owner = manager.sessions.session(&server, 0);
        manager
            .prepare_integration(server.id)
            .unwrap()
            .run(Arc::new(TestDesktop::default()))
            .await
            .unwrap();
        async fn applied(owner: &crate::server_session::ServerSession) {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let state = owner.permissions.snapshot();
                    if state.effective.revision == state.desired.revision {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
        }
        applied(&owner).await;
        let epoch = owner.permissions.snapshot().session_epoch;
        let previous = owner.agent_runtime.snapshot().hello.unwrap().instance_id;
        owner.agent_runtime.exhaust_registration_budget_for_test();
        let git = owner.acquire(&server).await.unwrap();
        let (stream, _) = git.launch(crate::agent::Service::Git).await.unwrap();
        let (_client, _) = crate::git::client::Client::start(stream).await.unwrap();
        assert_ne!(
            owner.agent_runtime.snapshot().hello.unwrap().instance_id,
            previous
        );
        assert_eq!(owner.agent_runtime.retiring_for_test(), 1);
        assert!(owner.permissions.browser_enabled());
        owner.permissions.update(false, false);
        applied(&owner).await;
        assert!(!owner.permissions.browser_enabled());
        owner.permissions.update(false, true);
        applied(&owner).await;
        assert!(owner.permissions.browser_enabled());
        assert_eq!(owner.permissions.snapshot().session_epoch, epoch);
        // Failure of a new metrics/Git runtime must not revoke the bridge still
        // running on the healthy retiring runtime.
        owner.agent_runtime.stop_remote_for_test().await;
        owner.agent_runtime.probe().await;
        assert_eq!(owner.agent_runtime.snapshot().status, "failed");
        assert!(owner.permissions.browser_enabled());
        assert_eq!(owner.permissions.snapshot().session_epoch, epoch);
        owner.agent_runtime.retry_failed().await;
        let fresh = owner.acquire(&server).await.unwrap();
        fresh.launch(crate::agent::Service::Git).await.unwrap();
        assert!(owner.permissions.browser_enabled());
        let replacement = owner.agent_runtime.snapshot().hello.unwrap().instance_id;
        owner.agent_runtime.stop_runtime_for_test(&previous).await;
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if owner.permissions.snapshot().session_epoch > epoch
                    && owner.permissions.browser_enabled()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("failed retiring bridge should recover on the current runtime");
        assert_eq!(
            owner.agent_runtime.snapshot().hello.unwrap().instance_id,
            replacement
        );
        assert_eq!(owner.transport_snapshot().generation, 1);
        manager.shutdown().await;
        assert!(!owner.permissions.browser_enabled());
    }
}
