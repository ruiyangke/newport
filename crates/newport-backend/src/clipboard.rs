use crate::desktop_integration::{ClipboardOffer, ClipboardOrigin};
use crate::model::Server;
use newport_protocol::clipboard::imports::{self, Encoding};
use newport_protocol::integration::{Reply, Request};
#[cfg(test)]
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::sync::watch;
#[path = "clipboard_metadata.rs"]
mod metadata;

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
    request: imports::Request,
    chunks: Option<std::collections::VecDeque<(Encoding, Vec<u8>)>>,
    permission_epoch: Option<u64>,
    // Retain the native connection/source fence and memory reservation through
    // compression and the final wire chunk, not just the initial native read.
    data: Option<Box<dyn crate::desktop_integration::ClipboardData>>,
}
impl ClipboardReply {
    fn data_current(&self) -> bool {
        self.chunks.is_some() && self.data.as_ref().is_none_or(|data| data.bytes().is_some())
    }
    fn invalidate(&mut self) {
        self.chunks = None;
        self.data = None;
    }
}
async fn next_reply_chunk(
    app: &crate::desktop_integration::Desktop,
    permissions: &std::sync::Arc<crate::integration_permissions::Gate>,
    reply: &mut ClipboardReply,
) -> (Encoding, bool, Vec<u8>) {
    let access = Some(permissions.clone());
    let current = if reply.data_current() && permission_current(&access, reply.permission_epoch) {
        if reply.data.is_some() {
            // The shared native owner observes changes continuously. Its data
            // fence is authoritative without another metadata round trip for
            // every chunk. Adapters without a native owner still need their counter read.
            Some(reply.request.revision as isize)
        } else {
            revision(app, access).await
        }
    } else {
        None
    };
    if !reply.data_current()
        || current != Some(reply.request.revision as isize)
        || !permission_current(&Some(permissions.clone()), reply.permission_epoch)
    {
        reply.invalidate();
    }
    let (status, bytes) = reply
        .chunks
        .as_mut()
        .and_then(|chunks| chunks.pop_front())
        .unwrap_or((Encoding::Failed, Vec::new()));
    let done = reply.chunks.as_ref().is_none_or(|chunks| chunks.is_empty());
    (status, done, bytes)
}
struct LegacyData(Vec<u8>);
impl crate::desktop_integration::ClipboardData for LegacyData {
    fn bytes(&self) -> Option<&[u8]> {
        Some(&self.0)
    }
}
type Permissions = Option<std::sync::Arc<crate::integration_permissions::Gate>>;
fn permission_current(permissions: &Permissions, ticket: Option<u64>) -> bool {
    permissions
        .as_ref()
        .is_none_or(|gate| ticket.is_some() && gate.clipboard_ticket() == ticket)
}
/// Never hold the permission mutex across native I/O. Subscribe before checking
/// the ticket, and discard the future/result on any change of its authority.
async fn authorized<T>(
    permissions: &Permissions,
    ticket: Option<u64>,
    work: impl std::future::Future<Output = Option<T>>,
) -> Option<T> {
    let Some(gate) = permissions else {
        return work.await;
    };
    let mut changes = gate.subscribe();
    tokio::pin!(work);
    loop {
        if !permission_current(permissions, ticket) {
            return None;
        }
        tokio::select! {
            biased;
            changed = changes.changed() => { if changed.is_err() { return None; } },
            result = &mut work => return permission_current(permissions, ticket).then_some(result).flatten(),
        }
    }
}
/// `serves_files`: whether reads of `application/x-newport-files` from the
/// server the offer is forwarded to are served (see `clipboard_files::offered`).
fn forwardable(format: &str, serves_files: bool) -> bool {
    imports::valid_format(format)
        && format != "TARGETS"
        && format != newport_platform::clipboard::LOCAL_PATHS_FORMAT
        && (serves_files || format != newport_platform::clipboard::FILES_FORMAT)
}
fn wire_offer(offer: (isize, Vec<String>)) -> imports::Offer {
    wire_offer_for(offer, false)
}
fn wire_offer_for(
    (revision, mut formats): (isize, Vec<String>),
    serves_files: bool,
) -> imports::Offer {
    formats.retain(|s| forwardable(s, serves_files));
    formats.truncate(256);
    imports::Offer {
        revision: revision as i64,
        formats,
    }
}
#[cfg(test)]
fn host_offer(offer: &ClipboardOffer) -> Option<imports::Offer> {
    host_offer_for(offer, false)
}
/// Without a destination-aware router, only a real local copy (including a
/// real clear) may become a host offer. Imported promises require the scoped
/// cross-runtime check in the integration pump; they must not echo to source.
fn host_offer_for(offer: &ClipboardOffer, serves_files: bool) -> Option<imports::Offer> {
    (offer.origin == ClipboardOrigin::Local)
        .then(|| wire_offer_for((offer.revision, offer.formats.clone()), serves_files))
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
) -> Result<Option<imports::Offer>, String> {
    clipboard_offer_checked(app, previous, None).await
}
#[cfg(test)]
async fn clipboard_offer_checked(
    app: &crate::desktop_integration::Desktop,
    previous: Option<isize>,
    permissions: Permissions,
) -> Result<Option<imports::Offer>, String> {
    let ticket = permissions.as_ref().and_then(|p| p.clipboard_ticket());
    Ok(clipboard_offer_at(app, previous, permissions, ticket)
        .await?
        .and_then(|offer| host_offer(&offer)))
}
async fn clipboard_offer_at(
    app: &crate::desktop_integration::Desktop,
    previous: Option<isize>,
    permissions: Permissions,
    ticket: Option<u64>,
) -> Result<Option<ClipboardOffer>, String> {
    if let Some(native) = app.clipboard_access() {
        let offer = authorized(&permissions, ticket, async move {
            tokio::time::timeout(Duration::from_secs(5), native.offer(previous))
                .await
                .ok()
                .flatten()
        })
        .await;
        return Ok(offer);
    }
    let desktop = app.clone();
    let result = on_main(app, move || {
        let read = || {
            desktop
                .clipboard_offer(previous)
                .map(|(revision, formats)| ClipboardOffer::local(revision, formats))
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
    request: imports::Request,
) -> ClipboardReply {
    read_requested_checked(app, request, None).await
}
async fn read_requested_checked(
    app: &crate::desktop_integration::Desktop,
    request: imports::Request,
    permissions: Permissions,
) -> ClipboardReply {
    let permission_epoch = permissions.as_ref().and_then(|p| p.clipboard_ticket());
    let revision = request.revision;
    let format = request.format.clone();
    if format == newport_platform::clipboard::LOCAL_PATHS_FORMAT {
        return ClipboardReply {
            request,
            chunks: None,
            permission_epoch,
            data: None,
        };
    }
    let started = tokio::time::Instant::now();
    let native_fence = app.clipboard_access().is_some();
    let data = read_local(app, revision, format, &permissions, permission_epoch).await;
    let original_bytes = data
        .as_ref()
        .and_then(|data| data.bytes())
        .map_or(0, <[u8]>::len);
    // Compression retains the original guarded representation. Recheck both
    // fences between chunks; no permission lock covers compression or OS work.
    let prepared = match data {
        Some(data) if permission_current(&permissions, permission_epoch) => {
            let access = permissions.clone();
            tokio::task::spawn_blocking(move || {
                let bytes = data.bytes().filter(|b| b.len() <= MAX_DATA)?;
                let mut chunks = std::collections::VecDeque::new();
                if bytes.is_empty() {
                    chunks.push_back((Encoding::Raw, Vec::new()));
                }
                for chunk in bytes.chunks(imports::CHUNK) {
                    if !permission_current(&access, permission_epoch) || data.bytes().is_none() {
                        return None;
                    }
                    chunks.push_back(imports::compression::compress_chunk(chunk).ok()?);
                }
                if !permission_current(&access, permission_epoch) || data.bytes().is_none() {
                    return None;
                }
                Some((data, chunks))
            })
            .await
            .ok()
            .flatten()
        }
        _ => None,
    };
    let (data, chunks) = match prepared {
        Some((data, chunks))
            if permission_current(&permissions, permission_epoch) && data.bytes().is_some() =>
        {
            (native_fence.then_some(data), Some(chunks))
        }
        _ => (None, None),
    };
    log::info!(
        "Clipboard request id={} revision={} success={} bytes={} elapsed_ms={}",
        request.id,
        revision,
        chunks.is_some(),
        original_bytes,
        started.elapsed().as_millis()
    );
    ClipboardReply {
        request,
        chunks,
        permission_epoch,
        data,
    }
}

/// One representation of the local clipboard at `revision`, read under the
/// permission ticket. Remote requests are filtered by the caller.
async fn read_local(
    app: &crate::desktop_integration::Desktop,
    revision: i64,
    format: String,
    permissions: &Permissions,
    permission_epoch: Option<u64>,
) -> Option<Box<dyn crate::desktop_integration::ClipboardData>> {
    if let Some(native) = app.clipboard_access() {
        return authorized(permissions, permission_epoch, async move {
            tokio::time::timeout(
                Duration::from_secs(20),
                native.read(revision as isize, format),
            )
            .await
            .ok()
            .flatten()
        })
        .await;
    }
    let desktop = app.clone();
    let access = permissions.clone();
    on_main(app, move || {
        let read = || {
            let (current, formats) = desktop.clipboard_offer(None)?;
            if current as i64 != revision || !formats.contains(&format) {
                return None;
            }
            desktop
                .read_clipboard_format(current, &format)
                .filter(|b| b.len() <= MAX_DATA)
        };
        match access {
            Some(gate) => permission_epoch
                .and_then(|ticket| gate.clipboard_at(ticket, read))
                .flatten(),
            None => read(),
        }
    })
    .await
    .ok()
    .flatten()
    .map(|bytes| Box::new(LegacyData(bytes)) as Box<dyn crate::desktop_integration::ClipboardData>)
}

/// A server read of `application/x-newport-files`: the local paths of the
/// requested revision are read from this machine's clipboard, never from the
/// server, and the upload ends when the permission ticket changes.
async fn read_files<C: crate::clipboard_files::Connect>(
    app: crate::desktop_integration::Desktop,
    files: crate::clipboard_files::Files<C>,
    request: imports::Request,
    permissions: Permissions,
) -> ClipboardReply {
    let permission_epoch = permissions.as_ref().and_then(|p| p.clipboard_ticket());
    let revision = request.revision;
    let started = tokio::time::Instant::now();
    let local_paths = async {
        read_local(
            &app,
            revision,
            newport_platform::clipboard::LOCAL_PATHS_FORMAT.to_owned(),
            &permissions,
            permission_epoch,
        )
        .await
        .and_then(|data| data.bytes().map(<[u8]>::to_vec))
    };
    let served = authorized(&permissions, permission_epoch, async {
        Some(
            files
                .serve_result(revision, permission_epoch, local_paths)
                .await,
        )
    })
    .await;
    // A paste that is superseded or whose permission was revoked says nothing.
    if let Some(Err(crate::clipboard_files::Failure::Failed(reason))) = &served {
        app.clipboard_files_status(
            files.server(),
            crate::desktop_integration::ClipboardFilesStatus::UploadFailed(reason.clone()),
        );
    }
    let bytes = served.and_then(Result::ok);
    let chunks = bytes.and_then(|bytes| {
        bytes
            .chunks(imports::CHUNK)
            .map(|chunk| imports::compression::compress_chunk(chunk).ok())
            .collect()
    });
    log::info!(
        "Clipboard files request id={} revision={} success={} elapsed_ms={}",
        request.id,
        revision,
        chunks.is_some(),
        started.elapsed().as_millis()
    );
    ClipboardReply {
        request,
        chunks,
        permission_epoch,
        data: None,
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

/// Why the integration worker stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Interruption {
    /// The connection under the worker was lost, or no channel could be
    /// opened. The link recovers; the next attempt waits for it.
    Link(newport_ssh::Error),
    /// Needle's runtime upgraded; start again on the new one.
    Upgraded,
    /// Needle or its control channel was interrupted; trying again after a
    /// pause can work.
    Interrupted(String),
    /// Trying again won't help, such as an error the agent declared.
    Failed(String),
}
impl std::fmt::Display for Interruption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Interruption::Link(error) => error.fmt(f),
            Interruption::Upgraded => f.write_str(crate::agent_runtime::UPGRADED),
            Interruption::Interrupted(message) | Interruption::Failed(message) => {
                f.write_str(message)
            }
        }
    }
}
impl From<crate::agent::Failure> for Interruption {
    fn from(failure: crate::agent::Failure) -> Self {
        match failure {
            crate::agent::Failure::Interrupted(_) => Interruption::Interrupted(failure.to_string()),
            crate::agent::Failure::Refused(message) => Interruption::Failed(message),
        }
    }
}
impl From<crate::agent_runtime::Error> for Interruption {
    fn from(error: crate::agent_runtime::Error) -> Self {
        use crate::agent_runtime::Error;
        match error {
            Error::Link(error) => Interruption::Link(error),
            Error::Interrupted(message) => Interruption::Interrupted(message),
            Error::Admission(ref source) if source.transient() => {
                Interruption::Interrupted(error.to_string())
            }
            Error::Remote(ref remote) if remote.retry == newport_protocol::common::Retry::Yes => {
                Interruption::Interrupted(error.to_string())
            }
            other => Interruption::Failed(other.to_string()),
        }
    }
}

/// What stopped a running integration worker, before it is told apart by
/// the fate of the worker's channel.
enum Stop {
    Needle(crate::agent_runtime::Error),
    Agent(crate::agent::Failure),
    Upgraded,
}
impl From<crate::agent_runtime::Error> for Stop {
    fn from(error: crate::agent_runtime::Error) -> Self {
        Stop::Needle(error)
    }
}
impl From<crate::agent::Failure> for Stop {
    fn from(failure: crate::agent::Failure) -> Self {
        Stop::Agent(failure)
    }
}
impl From<String> for Stop {
    fn from(message: String) -> Self {
        Stop::Agent(message.into())
    }
}
impl From<&str> for Stop {
    fn from(message: &str) -> Self {
        Stop::Agent(message.into())
    }
}

// Retry link losses, interrupted runtimes and agent-declared temporary
// failures (including exhausted disk space). Permission, ownership and other
// failures the agent declares remain permanent. Each agent owns its files
// until the SSH channel closes or its lease expires.
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
    Fut: std::future::Future<Output = Result<(), Interruption>>,
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
    Fut: std::future::Future<Output = Result<(), Interruption>>,
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
        let started = tokio::time::Instant::now();
        if let Some(changes) = &mut changes {
            changes.borrow_and_update();
        }
        let result = tokio::select! {
            biased;
            _ = cancel.changed() => return Ok(()),
            result = attempt() => result,
            // An attempt waiting for the link says why it can't log in.
            _ = report_login(owner, &blocked) => unreachable!("reporting never ends"),
        };
        let Err(error) = result else { return Ok(()) };
        match &error {
            Interruption::Upgraded => {
                retrying();
                continue;
            }
            // The link owns recovery, including when approval needs an
            // explicit Retry; the next attempt waits for it.
            Interruption::Link(error) if error.retry() != newport_ssh::Retry::Never => {
                retrying();
                continue;
            }
            _ => {}
        }
        if let Some(owner) = owner {
            owner.agent_runtime.probe().await;
            let runtime = owner.agent_runtime.snapshot();
            if runtime.status == "failed" {
                if !runtime.retryable {
                    return Err(runtime.error.unwrap_or_else(|| error.to_string()));
                }
                retrying();
                tokio::select! {
                    _ = cancel.changed() => return Ok(()),
                    result = owner.agent_runtime.wait_for_retry() => result?,
                }
                continue;
            }
        }
        if let Interruption::Failed(error) = error {
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

/// Reports each login failure on the session's link, for as long as it runs.
async fn report_login(
    owner: Option<&crate::server_session::ServerSession>,
    blocked: &impl Fn(String),
) {
    let Some(owner) = owner else {
        return std::future::pending().await;
    };
    let mut status = owner.link().status();
    loop {
        let failure = {
            let status = status.borrow_and_update();
            match (&status.reach, &status.login) {
                (newport_ssh::Reach::Failed(error), _)
                | (_, Some(newport_ssh::Login::Failed(error))) => Some(error.to_string()),
                _ => None,
            }
        };
        if let Some(failure) = failure {
            blocked(failure);
        }
        if status.changed().await.is_err() {
            return std::future::pending().await;
        }
    }
}

pub async fn run(
    owner: std::sync::Arc<crate::server_session::ServerSession>,
    server: Server,
    client: uuid::Uuid,
    app: crate::desktop_integration::Desktop,
    cancel: watch::Receiver<bool>,
    ready: impl Fn(String),
    waiting: impl Fn(Option<String>),
) -> Result<(), String> {
    let initial = if owner.permissions.clipboard_enabled() {
        revision(&app, Some(owner.permissions.clone())).await
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
                && let Some(count) = revision(&app, Some(owner.permissions.clone())).await
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

async fn revision(
    app: &crate::desktop_integration::Desktop,
    permissions: Permissions,
) -> Option<isize> {
    let ticket = permissions.as_ref().and_then(|p| p.clipboard_ticket());
    if let Some(native) = app.clipboard_access() {
        return authorized(&permissions, ticket, async move {
            tokio::time::timeout(Duration::from_secs(5), native.offer(None))
                .await
                .ok()
                .flatten()
                .map(|offer| offer.revision)
        })
        .await;
    }
    let desktop = app.clone();
    on_main(app, move || match permissions {
        Some(gate) => ticket
            .and_then(|ticket| gate.clipboard_at(ticket, || desktop.clipboard_revision()))
            .flatten(),
        None => desktop.clipboard_revision(),
    })
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
    ready: impl Fn(String),
) -> Result<(), Interruption> {
    let _permission_session = owner.permissions.session();
    let session = owner
        .needle(&server)
        .map_err(|_| Interruption::from(crate::agent_runtime::Error::Stale))?;
    let mut fate = None;
    let outcome: Result<(), Stop> = async {
        let launched = tokio::select! {
            _ = cancel.changed() => return Ok(()),
            launched = session.launch(
                crate::agent::Service::Integration {
                    client,
                    clipboard: server.clipboard_enabled,
                    browser: server.browser_enabled,
                },
            ) => launched,
        };
        let stream = launched?;
        fate = stream.fate().cloned();
        let mut agent = crate::agent::Agent::start(
            stream,
            app.clone(),
            owner.permissions.clone(),
            owner.callbacks(),
        )
        .await?;
        let mut changes = owner.permissions.subscribe();
        let desired = *changes.borrow_and_update();
        let permission_epoch = owner.permissions.snapshot().session_epoch;
        session.apply_permissions(permission_epoch, desired).await?;
        let mut exports = crate::clipboard_router::driver::Driver::attach(&app, &mut agent, &session.runtime_hello()?, &owner, &server.name).await?;
        let files = {
            let hello = session.runtime_hello()?;
            crate::clipboard_files::offered(|version| hello.require(newport_protocol::clipboard::SERVICE_CAPABILITY, version).is_ok())
        }
        .then(|| crate::clipboard_files::Files::new(crate::clipboard_files::Shared(owner.clone())).named(&server.name));
        let gui_notices = app.supports_gui_notices() && session.supports_gui_notices();
        if gui_notices { agent.request(Request::GuiNotices).await?; }
        let mut last_gui_notice: Option<std::time::Instant> = None;
        ready("Agent connected.".into());
        log::info!(
            "Integration agent connected (clipboard={}, browser={})",
            server.clipboard_enabled,
            server.browser_enabled
        );
        let mut heartbeat = tokio::time::Instant::now();
        let mut timer = tokio::time::interval(Duration::from_millis(200));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut reads = tokio::task::JoinSet::new();
        // Uploads run outside the bounded payload reads; keep-alives hold the
        // server's wait open while one runs.
        let mut file_reads = tokio::task::JoinSet::new();
        let mut keep_alives = crate::clipboard_files::KeepAlives::default();
        let mut metadata = metadata::Metadata::default();
        let mut replies = std::collections::VecDeque::<ClipboardReply>::new();
        let traffic = async {
            loop {
                if *cancel.borrow() { break; }
                if let Some(exports) = &mut exports { exports.propose(&mut agent).await?; }
                tokio::select! {
                    biased;
                    _ = cancel.changed() => break,
                    changed = changes.changed() => {
                        if changed.is_err() { break; }
                        let was_enabled = owner.permissions.clipboard_enabled();
                        if !was_enabled {
                            reads.abort_all();
                            file_reads.abort_all();
                            keep_alives.clear();
                            if let Some(files) = &files { files.revoke(); }
                            metadata.cancel();
                            replies.clear();
                        }
                        changes.borrow_and_update();
                    },
                    event = agent.event() => {
                        match event? {
                            // The receiver has already applied and fenced this
                            // event; the router sees its latest lease above.
                            crate::agent::Event::ExportsChanged if exports.is_some() => {},
                            crate::agent::Event::Browser(request) => agent.open(request).await?,
                            crate::agent::Event::GuiNotice if gui_notices => {
                                session.validate()?;
                                if last_gui_notice.is_none_or(|at| at.elapsed() >= Duration::from_secs(3)) {
                                    last_gui_notice = Some(std::time::Instant::now());
                                    app.request_gui_setup(&server.id.to_string());
                                }
                            }
                            crate::agent::Event::ClipboardRequest(request) => {
                                if request.format == newport_platform::clipboard::FILES_FORMAT {
                                    match &files {
                                        Some(files) if owner.permissions.clipboard_enabled() && file_reads.len() < 8 => {
                                            keep_alives.start(request.id, request.clone());
                                            file_reads.spawn(read_files(app.clone(), files.clone(), request, Some(owner.permissions.clone())));
                                        }
                                        _ => agent.clipboard_reply(imports::Reply::new(&request, Encoding::Failed, true, vec![])).await?,
                                    }
                                } else if !owner.permissions.clipboard_enabled() || reads.len() + replies.len() >= 8 {
                                    agent.clipboard_reply(imports::Reply::new(&request, Encoding::Failed, true, vec![])).await?;
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
                            Err(error) => return Err(error.to_string().into()),
                        }
                    }
                    result = file_reads.join_next(), if !file_reads.is_empty() => {
                        match result.unwrap() {
                            Ok(reply) => {
                                keep_alives.finish(reply.request.id);
                                if owner.permissions.clipboard_enabled() { replies.push_back(reply); }
                            }
                            Err(error) if error.is_cancelled() => {},
                            Err(error) => return Err(error.to_string().into()),
                        }
                    }
                    work = async { exports.as_mut().unwrap().next().await }, if exports.is_some() => {
                        exports.as_mut().unwrap().complete(work, &mut agent).await?;
                    }
                    result = metadata.next(&owner.permissions), if metadata.pending() => {
                        if let Some((ticket, offer)) = result? {
                            let revision = offer.revision;
                            if let Some(files) = &files { files.observe(revision as i64); }
                            let host = host_offer_for(&offer, files.is_some()).or_else(|| {
                                exports.as_ref()
                                    .is_some_and(|driver| driver.endpoint.may_forward_import(&offer))
                                    .then(|| wire_offer((offer.revision, offer.formats.clone())))
                            });
                            let observed = host.is_some() || exports.is_none() || offer.origin == ClipboardOrigin::Local;
                            if let Some(host) = host {
                                let Reply::Acknowledged { message: backend } = agent.request(Request::ClipboardOffer(host)).await? else {
                                    return Err("Invalid clipboard offer acknowledgment".into());
                                };
                                heartbeat = tokio::time::Instant::now();
                                ready(backend);
                            }
                            // Native ownership can be visible before the
                            // publication acknowledgment reaches the router.
                            // Retry a suppressed import on the normal timer;
                            // otherwise that race permanently loses its offer.
                            if observed { metadata.observed(ticket, revision); }
                        }
                    }
                    // One bounded chunk per turn lets events and timers run between writes.
                    _ = tokio::task::yield_now(), if !replies.is_empty() => {
                        let mut reply = replies.pop_front().unwrap();
                        let (status, done, bytes) = next_reply_chunk(&app, &owner.permissions, &mut reply).await;
                        agent.clipboard_reply(imports::Reply::new(&reply.request, status, done, bytes)).await?;
                        if !done { replies.push_back(reply); }
                    }
                    _ = timer.tick() => {
                        agent.maintain_clipboard_exports().await?;
                        if let Some(exports) = &mut exports { exports.tick(); }
                        // Sent before, and never after, the request's final reply.
                        for request in keep_alives.due(tokio::time::Instant::now()) {
                            agent.clipboard_reply(imports::Reply::new(&request, Encoding::Raw, false, vec![])).await?;
                        }
                        if let Some(files) = &files { files.tick(); }
                        if reads.is_empty() {
                            metadata.start(&app, &owner.permissions);
                        }
                        if heartbeat.elapsed() >= Duration::from_secs(5) {
                            agent.request(Request::Heartbeat).await?;
                            heartbeat = tokio::time::Instant::now();
                        }
                    }
                }
            }
            Ok::<(), Stop>(())
        };
        let outcome = tokio::select! {
            _ = session.wait_for_upgrade() => Err(Stop::Upgraded),
            result = traffic => result,
            result = session.watch_permissions(permission_epoch, || ready("Agent permissions updated.".into())) => result.map_err(Stop::Needle),
        };
        reads.abort_all();
        file_reads.abort_all();
        if let Some(files) = &files { files.shutdown().await; }
        metadata.cancel();
        drop(exports);
        agent.close().await;
        outcome
    }
    .await;
    let outcome = match outcome {
        Ok(()) => Ok(()),
        Err(Stop::Upgraded) => Err(Interruption::Upgraded),
        Err(Stop::Needle(error)) => Err(error.into()),
        // The agent's stream ends the same way whatever ended it: the
        // connection under it, its runtime, or the agent itself.
        Err(Stop::Agent(failure)) => {
            let lost = match fate.as_ref() {
                Some(fate) => crate::remote::lost(fate).await,
                None => None,
            };
            Err(match lost {
                Some(error) => Interruption::Link(error),
                None if session.is_closed() => Interruption::Interrupted(failure.to_string()),
                None => failure.into(),
            })
        }
    };
    owner.permissions.disconnected(
        outcome
            .as_ref()
            .err()
            .filter(|error| **error != Interruption::Upgraded)
            .map(ToString::to_string),
    );
    outcome
}

#[cfg(test)]
#[path = "clipboard_route_recovery_tests.rs"]
mod route_recovery_tests;

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
        managed_bridge(false).await;
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- clipboard::tests::managed_bridge"]
    async fn managed_bridge_routes_through_shared_owner_and_negotiates_exports() {
        managed_bridge(true).await;
    }

    async fn managed_bridge(routed: bool) {
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
        let adapter: Desktop = if routed {
            Arc::new(RoutedDesktop(crate::clipboard_router::Router::new(adapter)))
        } else {
            adapter
        };
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
                move |_| {
                    ready.send_modify(|count| *count += 1);
                },
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(10), changed.changed())
            .await
            .unwrap()
            .unwrap();
        if routed {
            owner
                .agent_runtime
                .snapshot()
                .hello
                .unwrap()
                .require(
                    newport_protocol::clipboard::exports::CAPABILITY,
                    newport_protocol::clipboard::exports::VERSION,
                )
                .unwrap();
        }
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
        let run =
            |command: &'static str| crate::remote::command::stdout(owner.link(), command, None);
        // Polling is only for metadata publication after permission acknowledgement.
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if run("fixture-clipboard").await.as_deref() == Ok("fixture") {
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
        assert!(run("fixture-clipboard").await.is_err());
        assert!(owner.permissions.browser_enabled());
        assert_eq!(run("printf ok").await.unwrap(), "ok");
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

    struct RoutedDesktop(crate::clipboard_router::Router);
    impl DesktopIntegration for RoutedDesktop {
        fn clipboard_router(&self) -> Option<crate::clipboard_router::Router> {
            Some(self.0.clone())
        }
        fn clipboard_access(&self) -> Option<Arc<dyn crate::desktop_integration::ClipboardAccess>> {
            Some(Arc::new(self.0.clone()))
        }
        fn on_main_thread(&self, _: Box<dyn FnOnce() + Send>) -> Result<(), String> {
            panic!("Router used UI-thread clipboard fallback")
        }
        fn clipboard_revision(&self) -> Option<isize> {
            panic!("Router used legacy clipboard")
        }
        fn clipboard_offer(&self, _: Option<isize>) -> Option<(isize, Vec<String>)> {
            panic!("Router used legacy clipboard")
        }
        fn read_clipboard_format(&self, _: isize, _: &str) -> Option<Vec<u8>> {
            panic!("Router used legacy clipboard")
        }
        fn open_url(&self, _: &str) -> Result<(), String> {
            Err("Browser not enabled in this fixture".into())
        }
    }
    impl crate::desktop_integration::ClipboardAccess for TestDesktop {
        fn offer(
            &self,
            previous: Option<isize>,
        ) -> crate::desktop_integration::ClipboardFuture<ClipboardOffer> {
            let offer = self
                .clipboard_offer(previous)
                .map(|(revision, formats)| ClipboardOffer::local(revision, formats));
            Box::pin(async move { offer })
        }
        fn read(
            &self,
            revision: isize,
            format: String,
        ) -> crate::desktop_integration::ClipboardFuture<
            Box<dyn crate::desktop_integration::ClipboardData>,
        > {
            struct Data(Vec<u8>);
            impl crate::desktop_integration::ClipboardData for Data {
                fn bytes(&self) -> Option<&[u8]> {
                    Some(&self.0)
                }
            }
            let data = self.read_clipboard_format(revision, &format);
            Box::pin(async move {
                data.map(|v| {
                    Box::new(Data(v)) as Box<dyn crate::desktop_integration::ClipboardData>
                })
            })
        }
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
        manager
            .prepare_integration(server.id)
            .unwrap()
            .run(Arc::new(TestDesktop::default()))
            .await
            .unwrap();
        let mut status = owner.link().status();
        let failed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let newport_ssh::Reach::Failed(error) = &status.borrow_and_update().reach {
                    break error.clone();
                }
                status.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        assert!(
            matches!(failed, newport_ssh::Error::Credentials(_)),
            "{failed:?}"
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
        owner.retry();
        assert_eq!(
            crate::remote::command::stdout(owner.link(), "printf ok", None)
                .await
                .unwrap(),
            "ok"
        );
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
        owner.permissions.disconnected(None);
        owner.integration.stop().await;
        assert!(!owner.permissions.browser_enabled());
        assert_eq!(
            crate::remote::command::stdout(owner.link(), "printf ok", None)
                .await
                .unwrap(),
            "ok"
        );
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
            let git = owner.needle(&server).unwrap();
            let worker = git.launch(crate::agent::Service::Git).await.unwrap();
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
            assert_eq!(owner.reached(), 1);
            assert_eq!(
                crate::remote::command::stdout(owner.link(), "printf ok", None)
                    .await
                    .unwrap(),
                "ok"
            );
            let fresh = owner.needle(&server).unwrap();
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
        assert_eq!(owner.reached(), 1);
        crate::manager::shutdown(&state).await;
    }

    #[tokio::test]
    async fn queued_clipboard_read_cannot_cross_disable_and_reenable() {
        use crate::integration_permissions::Gate;
        use newport_protocol::runtime::Applied;
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
                imports::Request {
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
                imports::Request {
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
            imports::Request {
                id: 2,
                revision: 42,
                format: "text/plain".into(),
            },
        )
        .await;
        assert!(reply.chunks.is_some());
        assert_eq!(adapter.reads.load(Ordering::SeqCst), 1);
        assert_eq!(revision(&desktop, None).await, Some(42));
    }

    #[tokio::test(start_paused = true)]
    async fn timed_out_desktop_dispatch_never_reads_clipboard_later() {
        let adapter = Arc::new(TestDesktop::default());
        adapter.queue.store(true, Ordering::SeqCst);
        let desktop: Desktop = adapter.clone();
        let read = tokio::spawn(async move {
            read_requested(
                &desktop,
                imports::Request {
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
                    Err(Interruption::Interrupted("Disconnected".into()))
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
        // No total deadline; the backoff is capped.
        assert_eq!(started.elapsed(), Duration::from_secs(90));
    }

    #[tokio::test]
    async fn timeout_recovery_retries_but_preserves_permanent_errors() {
        let (_tx, rx) = watch::channel(false);
        let mut attempts = 0;
        recover_connection(
            || {
                attempts += 1;
                std::future::ready(match attempts {
                    1 => Err(crate::agent::Failure::Interrupted(
                        "clipboard upload timed out".into(),
                    )
                    .into()),
                    2 => Err(Interruption::Link(newport_ssh::Error::TimedOut)),
                    _ => Ok(()),
                })
            },
            rx.clone(),
            || {},
            Duration::ZERO,
            None,
        )
        .await
        .unwrap();
        assert_eq!(attempts, 3);

        let mut attempts = 0;
        recover_connection(
            || {
                attempts += 1;
                std::future::ready(if attempts < 6 {
                    Err(Interruption::Link(newport_ssh::Error::Lost(
                        "Connection reset by peer".into(),
                    )))
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
                std::future::ready(Err(Interruption::Failed(
                    "Clipboard sync is already active for this server account.".into(),
                )))
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
                        Err(Interruption::Interrupted("No space left on device".into()))
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
                    Err(Interruption::Interrupted("No space left on device".into()))
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
                    Err(crate::agent::failure(newport_protocol::common::Error {
                        code: "disk_full".into(),
                        message: "No space left on device (os error 28) at path /clipboard/.tmp"
                            .into(),
                        retry: newport_protocol::common::Retry::Yes,
                    })
                    .into())
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
                std::future::ready(Err(Interruption::Interrupted("request timed out".into())))
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
            std::future::pending::<Result<(), Interruption>>,
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
            Interruption::Link(newport_ssh::Error::Busy),
            crate::agent::Failure::Interrupted("early eof".into()).into(),
            crate::agent_runtime::Error::Interrupted("Needle control request timed out".into())
                .into(),
            crate::agent_runtime::Error::Interrupted(crate::agent_runtime::BUSY.into()).into(),
            crate::agent_runtime::Error::Remote(newport_protocol::common::Error {
                code: "busy".into(),
                message: "worker limit reached".into(),
                retry: newport_protocol::common::Retry::Yes,
            })
            .into(),
        ] {
            let (_tx, rx) = watch::channel(false);
            let mut attempts = 0;
            let started = tokio::time::Instant::now();
            recover_connection(
                || {
                    attempts += 1;
                    std::future::ready(if attempts < 13 {
                        Err(error.clone())
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
                    Err(Interruption::Link(newport_ssh::Error::AgentUnavailable))
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
                std::future::ready(Err(crate::agent::failure(
                    newport_protocol::common::Error {
                        code: "read_only".into(),
                        message: "SSH transport interrupted: disk is read-only".into(),
                        retry: newport_protocol::common::Retry::No,
                    },
                )
                .into()))
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

    #[tokio::test]
    async fn runtime_failures_without_retry_advice_do_not_recover_automatically() {
        use newport_protocol::common::{Error as RemoteError, Retry};
        for code in ["busy", "control_failed", "unavailable"] {
            for retry in [Retry::No, Retry::Unknown] {
                let (_tx, rx) = watch::channel(false);
                let mut attempts = 0;
                let remote = crate::agent_runtime::Error::Remote(RemoteError {
                    code: code.into(),
                    message: "SSH transport interrupted: please retry".into(),
                    retry,
                });
                let expected = remote.to_string();
                let error = recover_connection(
                    || {
                        attempts += 1;
                        std::future::ready(Err(remote.clone().into()))
                    },
                    rx,
                    || panic!("Retry requires explicit positive advice"),
                    Duration::ZERO,
                    None,
                )
                .await
                .unwrap_err();
                assert_eq!(attempts, 1);
                assert_eq!(error, expected);
            }
        }
    }

    #[test]
    fn worker_admission_recovery_distinguishes_interruption_from_invalid_replies() {
        use crate::{agent_runtime::Error, service_channel::AdmissionError};
        use newport_protocol::encoding::protobuf::FrameError;
        let interrupted = Error::Admission(Arc::new(AdmissionError::Closed));
        assert!(matches!(
            Interruption::from(interrupted),
            Interruption::Interrupted(_)
        ));
        let invalid = Error::Admission(Arc::new(AdmissionError::Frame(FrameError::Malformed)));
        assert!(matches!(
            Interruption::from(invalid),
            Interruption::Failed(_)
        ));
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
            proxy: None,
        };
        let session = crate::ssh::link(&server, crate::ssh::no_jump_hosts);
        let run = |command: &'static str| crate::remote::command::stdout(&session, command, None);
        let stream = crate::remote::command::stream(&session, "fixture-agent")
            .await
            .unwrap()
            .into_stream();
        use newport_protocol::{
            encoding::protobuf::tokio::FrameReader,
            integration::{
                self, BrowserReply, BrowserRequest, Call, ClientMessage, ServerMessage,
                wire::Message,
            },
        };
        let mut stream = FrameReader::new(stream, integration::SERVER_LIMITS);
        async fn event(
            stream: &mut FrameReader<impl tokio::io::AsyncRead + Unpin>,
        ) -> ServerMessage {
            let body = tokio::time::timeout(Duration::from_secs(5), stream.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            ServerMessage::decode(body).unwrap()
        }
        let bytes = archive(BTreeMap::from([(
            "text/plain".into(),
            "SSH clipboard 世界\n".as_bytes().to_vec(),
        )]))
        .unwrap();
        let call = Call {
            id: 1,
            request: Request::Snapshot(bytes.into()),
        };
        stream
            .get_mut()
            .write_all(&ClientMessage::Call(call.clone()).encode().unwrap())
            .await
            .unwrap();
        let ServerMessage::Response(response) = event(&mut stream).await else {
            panic!("expected response")
        };
        response.validate_for(&call).unwrap();
        assert_eq!(
            run("fixture-clipboard").await.unwrap(),
            "SSH clipboard 世界\n"
        );
        let (opened, ()) = tokio::join!(run("fixture-open"), async {
            assert_eq!(
                event(&mut stream).await,
                ServerMessage::Browser(BrowserRequest {
                    id: 1,
                    url: "https://example.com/login?code=fixture".into()
                })
            );
            stream
                .get_mut()
                .write_all(
                    &ClientMessage::Browser(BrowserReply {
                        id: 1,
                        success: true,
                        message: None,
                    })
                    .encode()
                    .unwrap(),
                )
                .await
                .unwrap();
        });
        opened.unwrap();
        stream
            .get_mut()
            .write_all(&ClientMessage::Quit.encode().unwrap())
            .await
            .unwrap();
        let mut end = Vec::new();
        stream.get_mut().read_to_end(&mut end).await.unwrap();
        assert!(run("fixture-clipboard").await.is_err());
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
            let setup = crate::ssh::link(&server, crate::ssh::no_jump_hosts);
            crate::agent_lifecycle::manage_test(&setup, &server, false, false)
                .await
                .unwrap();
            let old_hash = crate::agent_lifecycle::activate_variant_test(&setup).await;
            let mut manager = crate::manager::Manager::new(crate::profile::Store::for_test(
                directory.path().into(),
            ));
            manager.config.servers.push(server.clone());
            let owner = manager.sessions.session(&server, 0);
            owner.adopt_build_for_test(&old_hash).await.unwrap();
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
            owner.install(&server, false).await.unwrap();
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
            assert_eq!(owner.reached(), 1);
            manager.shutdown().await;
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
        let git = owner.needle(&server).unwrap();
        let stream = git.launch(crate::agent::Service::Git).await.unwrap();
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
        let fresh = owner.needle(&server).unwrap();
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
        assert_eq!(owner.reached(), 1);
        manager.shutdown().await;
        assert!(!owner.permissions.browser_enabled());
    }
}

#[cfg(test)]
#[path = "clipboard_async_tests.rs"]
mod async_tests;
