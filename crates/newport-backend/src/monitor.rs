//! A long-lived agent channel per server feature. Overview reads cannot queue
//! behind a service action, container prune, or Compose operation.
//! The agent samples on its own, so CPU figures are recent, not lifetime.
use crate::{agent, agent_runtime, needle::Needle, server_session::ServerSession};
use newport_protocol::{
    encoding::protobuf::{FrameError, tokio::FrameReader},
    monitor::{
        Call, InvalidRequest, Reply, Request, SERVER_LIMITS, metrics::Snapshot,
        stable_display::State, wire,
    },
};
use std::{
    sync::{Arc, Weak},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt},
    sync::Mutex,
};

const IDLE: Duration = Duration::from_secs(300);
// A server that cannot run the agent is not asked to upload it every sample;
// the reason is repeated until then, or until its connection settings change.
const UNAVAILABLE_FOR: Duration = Duration::from_secs(600);
const REPLY_TIMEOUT: Duration = Duration::from_secs(15);

type Channel = crate::agent_runtime::WorkerStream;

/// Monitor failures retain their cause until a feature presents them to the user.
#[derive(Debug)]
pub enum Error {
    Request(InvalidRequest),
    Runtime(agent_runtime::Error),
    Unavailable {
        feature: &'static str,
        source: agent_runtime::Error,
    },
    IdentifiersExhausted,
    Encode(wire::Error),
    Transport(FrameError),
    Closed,
    Timeout {
        duration: Duration,
        source: tokio::time::error::Elapsed,
    },
    Decode(wire::Error),
    Remote(newport_protocol::common::Error),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Request(error) => error.fmt(f),
            Self::Runtime(error) => error.fmt(f),
            Self::Unavailable { feature, source } => write!(
                f,
                "{feature} needs the Newport agent, which can’t run on this server: {source}"
            ),
            Self::IdentifiersExhausted => f.write_str("Monitor request identifiers exhausted."),
            Self::Encode(error) => error.fmt(f),
            Self::Transport(error) => write!(f, "SSH transport interrupted: {error}"),
            Self::Closed => f.write_str("SSH transport interrupted: the agent monitor closed"),
            Self::Timeout { duration, .. } => write!(
                f,
                "The agent monitor did not answer within {} seconds.",
                duration.as_secs()
            ),
            Self::Decode(error) => write!(f, "Invalid agent monitor reply: {error}"),
            Self::Remote(error) => f.write_str(&error.message),
        }
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Request(error) => Some(error),
            Self::Runtime(error) | Self::Unavailable { source: error, .. } => Some(error),
            Self::Encode(error) | Self::Decode(error) => Some(error),
            Self::Transport(FrameError::Io(error)) => Some(error),
            Self::Transport(FrameError::Decode(error)) => Some(error),
            Self::Transport(error) => Some(error),
            Self::Timeout { source, .. } => Some(source),
            Self::Remote(error) => Some(error),
            Self::IdentifiersExhausted | Self::Closed => None,
        }
    }
}
impl From<agent_runtime::Error> for Error {
    fn from(error: agent_runtime::Error) -> Self {
        Self::Runtime(error)
    }
}
impl From<Error> for String {
    fn from(error: Error) -> Self {
        error.to_string()
    }
}

#[derive(Default)]
pub(crate) struct Channels([Arc<Mutex<Slot>>; 6]);
impl Channels {
    pub(crate) fn for_request(&self, request: &Request) -> Arc<Mutex<Slot>> {
        let lane = match request {
            Request::Metrics | Request::StableDisplay => 0,
            Request::Services(_) => 1,
            Request::Containers(_) => 2,
            Request::Compose(_) => 3,
            Request::Ports(_) => 4,
            Request::FileWatch(_) => 5,
        };
        self.0[lane].clone()
    }
    pub(crate) async fn drain(&self) {
        for lane in &self.0 {
            lane.lock().await.drain().await;
        }
    }
}
struct Open {
    session: Needle,
    uses_session_slot: bool,
    reader: FrameReader<tokio::io::ReadHalf<Channel>>,
    next_id: u64,
    writer: tokio::io::WriteHalf<Channel>,
}
struct Unavailable {
    until: Instant,
    reason: agent_runtime::Error,
    runtime: Option<String>,
}
#[derive(Default)]
pub(crate) struct Slot {
    open: Option<Open>,
    unavailable: Option<Unavailable>,
    used: Option<Instant>,
}
impl Slot {
    pub(crate) async fn drain(&mut self) {
        self.close().await;
        self.unavailable = None;
    }
    async fn close(&mut self) {
        if let Some(mut open) = self.open.take() {
            let _ = open.writer.shutdown().await;
        }
    }
}

/// Explicit retries clear this session's link, runtime, and monitor failures.
pub async fn retry_installation(server: &Arc<ServerSession>) {
    server.retry();
    server.agent_runtime.retry_failed().await;
    server.monitor.drain().await;
}

/// Overview retries affect its own channel; another feature may be executing
/// a mutation that must finish on its existing protocol stream.
pub(crate) async fn retry_overview(server: &Arc<ServerSession>) {
    server.retry();
    server.agent_runtime.retry_failed().await;
    server
        .monitor
        .for_request(&Request::Metrics)
        .lock()
        .await
        .drain()
        .await;
}

/// An Overview reading from the server's agent.
pub async fn snapshot(server: &Arc<ServerSession>) -> Result<Snapshot, String> {
    match ask(server, &Request::Metrics, REPLY_TIMEOUT).await? {
        Reply::Metrics(value) => Ok(value),
        _ => Err("Invalid metrics response operation.".into()),
    }
}

/// Who shows the account's display, as the server's agent reports it.
pub(crate) async fn stable_display(
    server: &Arc<ServerSession>,
) -> Result<crate::graphical_session::Live, String> {
    match ask(server, &Request::StableDisplay, REPLY_TIMEOUT).await? {
        Reply::StableDisplay(value) => live_display(value, server.integration_session_id()),
        _ => Err("Invalid stable display response operation.".into()),
    }
}
/// `reply` as seen from the computer whose session on the server is `here`:
/// the session tells computers apart, as names needn't.
fn live_display(reply: State, here: uuid::Uuid) -> Result<crate::graphical_session::Live, String> {
    use crate::graphical_session::Live;
    use newport_protocol::monitor::stable_display::{State, Viewer};
    reply.validate().map_err(|error| error.to_string())?;
    Ok(match reply {
        State { running: false, .. } => Live::Stopped,
        State { viewer: None, .. } => Live::Unviewed,
        State {
            viewer: Some(Viewer { session, .. }),
            ..
        } if uuid::Uuid::parse_str(&session).ok() == Some(here) => Live::Here,
        State {
            viewer: Some(Viewer { session, computer }),
            ..
        } => Live::Viewed(crate::graphical_session::launch::ShownOn {
            computer,
            session: Some(session),
        }),
    })
}

/// Any request on the server's agent channel (Overview, Services), launching
/// using an already-installed agent. Requests to one server are
/// serialised within their feature channel; `timeout` bounds the reply.
pub async fn ask(
    server: &Arc<ServerSession>,
    request: &Request,
    timeout: Duration,
) -> Result<Reply, Error> {
    request.validate().map_err(Error::Request)?;
    if server.is_revoked() {
        return Err(agent_runtime::Error::Stale.into());
    }
    let shared = server.monitor.for_request(request);
    let mut slot = shared.lock().await;
    if slot
        .open
        .as_ref()
        .is_some_and(|open| open.session.is_closed() || open.session.is_draining())
    {
        slot.close().await;
        slot.unavailable = None;
    }
    if let Some(cached) = &slot.unavailable {
        let runtime = server.agent_runtime.snapshot().hello.map(|h| h.instance_id);
        if Instant::now() < cached.until && cached.runtime == runtime {
            return Err(unavailable(request, cached.reason.clone()));
        }
        slot.unavailable = None;
    }
    slot.used = Some(Instant::now());
    // Only known read-only operations may be replayed after an incomplete reply.
    let attempts = if retryable_read(request) { 2 } else { 1 };
    for attempt in 0..attempts {
        if slot.open.is_none() {
            match open(server).await? {
                Ok(open) => {
                    slot.open = Some(open);
                    close_when_idle(Arc::downgrade(&shared), server.link().pressure());
                }
                Err(reason) => {
                    slot.unavailable = Some(Unavailable {
                        until: Instant::now() + UNAVAILABLE_FOR,
                        reason: reason.clone(),
                        runtime: server.agent_runtime.snapshot().hello.map(|h| h.instance_id),
                    });
                    return Err(unavailable(request, reason));
                }
            }
        }
        let result = exchange_session(&mut slot.open, |mut open| async move {
            let call = Call {
                id: open.next_id,
                request: request.clone(),
            };
            open.next_id = open
                .next_id
                .checked_add(1)
                .ok_or(Error::IdentifiersExhausted)?;
            match exchange(&mut open.reader, &mut open.writer, &call, timeout).await {
                Ok(reply) => Ok((open, reply)),
                Err(error) => Err(error),
            }
        })
        .await;
        match result {
            Ok(reply) => {
                slot.open
                    .as_ref()
                    .expect("successful exchange")
                    .session
                    .validate()?;
                if slot
                    .open
                    .as_ref()
                    .is_some_and(|open| open.session.is_draining())
                {
                    slot.close().await;
                }
                // A complete failure is an answer, even when its retry hint is
                // Yes. Retain the channel and let the caller decide to retry.
                return answer(reply);
            }
            Err(error) => {
                if attempt + 1 == attempts {
                    return Err(error);
                }
            }
        }
    }
    unreachable!("the second attempt returns")
}

fn retryable_read(request: &Request) -> bool {
    use newport_protocol::monitor::{compose, containers, services};
    match request {
        Request::Metrics | Request::StableDisplay | Request::Ports(_) => true,
        Request::Services(
            services::Request::Snapshot
            | services::Request::Logs(_)
            | services::Request::Unit { .. },
        ) => true,
        Request::Containers(
            containers::Request::Snapshot
            | containers::Request::Inspect { .. }
            | containers::Request::Logs(_)
            | containers::Request::Disk { .. },
        ) => true,
        Request::Compose(
            compose::Request::Project { .. }
            | compose::Request::Job { .. }
            | compose::Request::Jobs,
        ) => true,
        Request::Services(services::Request::Action { .. })
        | Request::Containers(
            containers::Request::Action { .. }
            | containers::Request::ProjectAction { .. }
            | containers::Request::Remove { .. }
            | containers::Request::Prune { .. },
        )
        | Request::Compose(compose::Request::Run { .. } | compose::Request::Cancel { .. })
        | Request::FileWatch(_) => false,
    }
}

/// A cancelled or incomplete exchange must not leave a reusable stream behind:
/// its unread reply (or a partial frame) would belong to the next caller.
async fn exchange_session<S, T, F>(
    cached: &mut Option<S>,
    exchange: impl FnOnce(S) -> F,
) -> Result<T, Error>
where
    F: std::future::Future<Output = Result<(S, T), Error>>,
{
    let session = cached.take().expect("opened above");
    let (session, reply) = exchange(session).await?;
    *cached = Some(session);
    Ok(reply)
}

/// These features need the agent; say why it cannot run rather than show nothing.
fn unavailable(request: &Request, source: agent_runtime::Error) -> Error {
    let feature = match request {
        Request::Ports(_) => "Port discovery",
        _ => "Monitoring",
    };
    Error::Unavailable { feature, source }
}

/// Link and runtime recovery remain eligible on the next request. Only
/// actionable capability/installation failures get the longer unavailable cache.
async fn open(server: &Arc<ServerSession>) -> Result<Result<Open, agent_runtime::Error>, Error> {
    let session = server
        .needle(server.server())
        .map_err(|_| agent_runtime::Error::Stale)?;
    match session.launch(agent::Service::Monitor).await {
        Ok(stream) => {
            let uses_session_slot = stream.uses_session_slot();
            let (reader, writer) = tokio::io::split(stream);
            Ok(Ok(Open {
                session,
                uses_session_slot,
                reader: FrameReader::new(reader, SERVER_LIMITS),
                next_id: 1,
                writer,
            }))
        }
        Err(error) => {
            server.agent_runtime.probe().await;
            let runtime = server.agent_runtime.snapshot();
            if runtime.retryable || error.transient() {
                Err(error.into())
            } else {
                Ok(Err(error))
            }
        }
    }
}

fn close_when_idle(slot: Weak<Mutex<Slot>>, mut pressure: tokio::sync::watch::Receiver<u64>) {
    tokio::spawn(async move {
        // A reclaim that finds the channel busy is retried shortly instead of
        // waiting for the next idle check.
        let mut pending = false;
        loop {
            let wait = Duration::from_secs(if pending { 1 } else { 60 });
            let reclaim = tokio::select! {
                _ = tokio::time::sleep(wait) => pending,
                changed = pressure.changed() => {
                    if changed.is_err() { return; }
                    true
                }
            };
            let Some(slot) = slot.upgrade() else { return };
            // The same lock protects the entire exchange. Never interrupt a
            // running monitor action to make room for another SSH channel.
            let Ok(mut slot) = slot.try_lock() else {
                pending = reclaim;
                continue;
            };
            pending = false;
            if slot.open.is_none() {
                return;
            }
            let releases_capacity = slot
                .open
                .as_ref()
                .is_some_and(|open| open.uses_session_slot);
            if (reclaim && releases_capacity) || slot.used.is_some_and(|used| used.elapsed() > IDLE)
            {
                slot.close().await;
                return;
            }
        }
    });
}

async fn exchange(
    reader: &mut FrameReader<impl AsyncRead + Unpin>,
    writer: &mut (impl AsyncWrite + Unpin),
    call: &Call,
    timeout: Duration,
) -> Result<Reply, Error> {
    let message = wire::encode_request(call).map_err(Error::Encode)?;
    let frame = tokio::time::timeout(timeout, async {
        writer.write_all(&message).await.map_err(FrameError::from)?;
        writer.flush().await.map_err(FrameError::from)?;
        reader.next().await
    })
    .await
    .map_err(|source| Error::Timeout {
        duration: timeout,
        source,
    })?
    .map_err(Error::Transport)?
    .ok_or(Error::Closed)?;
    wire::decode_reply(frame, call)
        .map(|response| response.reply)
        .map_err(Error::Decode)
}

fn answer(reply: Reply) -> Result<Reply, Error> {
    match reply {
        Reply::Failed(error) => Err(Error::Remote(error)),
        reply => Ok(reply),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_protocol::{
        common::{Error as RemoteError, Retry},
        monitor::{
            CLIENT_LIMITS, Response, compose, containers, file_watch, services,
            stable_display::Viewer,
        },
    };
    use std::error::Error as _;
    use tokio::io::{AsyncReadExt, DuplexStream};

    fn requests() -> Vec<(Request, bool, usize)> {
        vec![
            (Request::Metrics, true, 0),
            (Request::StableDisplay, true, 0),
            (Request::Services(services::Request::Snapshot), true, 1),
            (
                Request::Services(services::Request::Logs(services::LogsRequest::default())),
                true,
                1,
            ),
            (
                Request::Services(services::Request::Unit {
                    scope: services::Scope::User,
                    unit: "app.service".into(),
                }),
                true,
                1,
            ),
            (
                Request::Services(services::Request::Action {
                    scope: services::Scope::System,
                    unit: "app.service".into(),
                    verb: services::Verb::Stop,
                }),
                false,
                1,
            ),
            (Request::Containers(containers::Request::Snapshot), true, 2),
            (
                Request::Containers(containers::Request::Inspect {
                    container: "web".into(),
                }),
                true,
                2,
            ),
            (
                Request::Containers(containers::Request::Logs(containers::LogsRequest {
                    target: containers::LogTarget::Container("web".into()),
                    cursor: None,
                    lines: None,
                })),
                true,
                2,
            ),
            (
                Request::Containers(containers::Request::Disk { part: None }),
                true,
                2,
            ),
            (
                Request::Containers(containers::Request::Action {
                    container: "web".into(),
                    verb: containers::ContainerVerb::Stop,
                }),
                false,
                2,
            ),
            (
                Request::Containers(containers::Request::ProjectAction {
                    project: "app".into(),
                    verb: containers::ProjectVerb::Restart,
                    expected_ids: vec!["a".repeat(64)],
                }),
                false,
                2,
            ),
            (
                Request::Containers(containers::Request::Remove {
                    containers: vec!["web".into()],
                }),
                false,
                2,
            ),
            (
                Request::Containers(containers::Request::Prune {
                    what: containers::PruneTarget::Containers,
                }),
                false,
                2,
            ),
            (
                Request::Compose(compose::Request::Project {
                    project: "app".into(),
                }),
                true,
                3,
            ),
            (
                Request::Compose(compose::Request::Job {
                    job: "abc".into(),
                    offset: 0,
                }),
                true,
                3,
            ),
            (Request::Compose(compose::Request::Jobs), true, 3),
            (
                Request::Compose(compose::Request::Run {
                    project: "app".into(),
                    action: compose::Action::Down {
                        volumes: false,
                        remove_orphans: false,
                    },
                }),
                false,
                3,
            ),
            (
                Request::Compose(compose::Request::Cancel { job: "abc".into() }),
                false,
                3,
            ),
            (Request::Ports(Default::default()), true, 4),
            (
                Request::FileWatch(file_watch::Request {
                    watch: "11111111-1111-4111-8111-111111111111".into(),
                    root: "/repo".into(),
                    directories: vec!["/repo".into()],
                    cursor: None,
                }),
                false,
                5,
            ),
            (
                Request::FileWatch(file_watch::Request {
                    watch: "11111111-1111-4111-8111-111111111111".into(),
                    root: "/repo".into(),
                    directories: vec![],
                    cursor: None,
                }),
                false,
                5,
            ),
        ]
    }
    #[tokio::test]
    async fn slow_features_keep_their_own_order_without_blocking_other_channels() {
        let channels = Channels::default();
        for lane in 0..6 {
            let busy = channels.0[lane].lock().await;
            for (request, _, expected) in requests() {
                let selected = channels.for_request(&request);
                assert!(Arc::ptr_eq(&selected, &channels.0[expected]), "{request:?}");
                assert_eq!(
                    selected.try_lock().is_err(),
                    lane == expected,
                    "{request:?}"
                );
            }
            drop(busy);
        }
        channels.drain().await;
    }
    #[test]
    fn reconnect_replays_reads_but_never_mutations_or_file_watches() {
        for (request, read, _) in requests() {
            request.validate().unwrap();
            assert_eq!(retryable_read(&request), read, "{request:?}");
        }
    }
    #[test]
    fn stable_display_identity_does_not_depend_on_computer_names() {
        use crate::graphical_session::Live;
        let here = uuid::Uuid::new_v4();
        let other = uuid::Uuid::new_v4().to_string();
        assert_eq!(live_display(State::default(), here).unwrap(), Live::Stopped);
        assert_eq!(
            live_display(
                State {
                    running: true,
                    viewer: None
                },
                here
            )
            .unwrap(),
            Live::Unviewed
        );
        let shown = |session: String| State {
            running: true,
            viewer: Some(Viewer {
                session,
                computer: Some("studio".into()),
            }),
        };
        assert_eq!(
            live_display(shown(here.to_string()), here).unwrap(),
            Live::Here
        );
        assert_eq!(
            live_display(shown(other.clone()), here).unwrap(),
            Live::Viewed(crate::graphical_session::launch::ShownOn {
                computer: Some("studio".into()),
                session: Some(other)
            })
        );
        let mut invalid = shown(here.to_string());
        invalid.running = false;
        assert!(live_display(invalid, here).is_err());
    }

    struct FixtureSession {
        reader: FrameReader<tokio::io::ReadHalf<DuplexStream>>,
        writer: tokio::io::WriteHalf<DuplexStream>,
        next_id: u64,
    }
    fn fixture_session() -> (FixtureSession, FrameReader<DuplexStream>) {
        let (client, agent) = tokio::io::duplex(64 * 1024);
        let (reader, writer) = tokio::io::split(client);
        (
            FixtureSession {
                reader: FrameReader::new(reader, SERVER_LIMITS),
                writer,
                next_id: 1,
            },
            FrameReader::new(agent, CLIENT_LIMITS),
        )
    }
    async fn exchange_cached(cached: &mut Option<FixtureSession>) -> Result<Reply, Error> {
        exchange_session(cached, |mut session| async move {
            let call = Call {
                id: session.next_id,
                request: Request::StableDisplay,
            };
            session.next_id += 1;
            let reply = exchange(
                &mut session.reader,
                &mut session.writer,
                &call,
                REPLY_TIMEOUT,
            )
            .await?;
            Ok((session, reply))
        })
        .await
    }
    async fn read_call(agent: &mut FrameReader<DuplexStream>) -> Call {
        wire::decode_request(agent.next().await.unwrap().unwrap()).unwrap()
    }
    fn response(call: &Call, reply: Reply) -> Vec<u8> {
        wire::encode_reply(&Response { id: call.id, reply }, call).unwrap()
    }
    fn shown(running: bool) -> Reply {
        Reply::StableDisplay(State {
            running,
            viewer: None,
        })
    }
    #[tokio::test]
    async fn cancelled_partial_frames_discard_the_channel_before_the_next_request() {
        let call = Call {
            id: 1,
            request: Request::StableDisplay,
        };
        let whole = response(&call, shown(false));
        for partial in [vec![], vec![0x82], whole[..whole.len() - 1].to_vec()] {
            let (session, mut agent) = fixture_session();
            let mut cached = Some(session);
            {
                let exchange = exchange_cached(&mut cached);
                tokio::pin!(exchange);
                tokio::select! {
                    result = &mut exchange => panic!("incomplete reply finished: {result:?}"),
                    _ = async {
                        assert_eq!(read_call(&mut agent).await, call);
                        agent.get_mut().write_all(&partial).await.unwrap();
                        tokio::task::yield_now().await;
                    } => {},
                }
            }
            assert!(cached.is_none());
            let (mut transport, buffered) = agent.into_parts();
            assert!(buffered.is_empty());
            let mut rest = Vec::new();
            tokio::time::timeout(Duration::from_secs(1), transport.read_to_end(&mut rest))
                .await
                .unwrap()
                .unwrap();
            assert!(rest.is_empty());
            let (session, mut agent) = fixture_session();
            cached = Some(session);
            let task = tokio::spawn(async move {
                let call = read_call(&mut agent).await;
                agent
                    .get_mut()
                    .write_all(&response(&call, shown(true)))
                    .await
                    .unwrap();
            });
            assert_eq!(exchange_cached(&mut cached).await.unwrap(), shown(true));
            assert!(cached.is_some());
            task.await.unwrap();
        }
    }
    #[tokio::test]
    async fn complete_service_errors_preserve_the_channel_and_advance_identifiers() {
        let (session, mut agent) = fixture_session();
        let mut cached = Some(session);
        let task = tokio::spawn(async move {
            for (id, reply) in [
                (
                    1,
                    Reply::Failed(RemoteError {
                        code: "not_ready".into(),
                        message: "not ready".into(),
                        retry: Retry::Yes,
                    }),
                ),
                (2, shown(true)),
            ] {
                let call = read_call(&mut agent).await;
                assert_eq!(call.id, id);
                agent
                    .get_mut()
                    .write_all(&response(&call, reply))
                    .await
                    .unwrap();
            }
        });
        let error = answer(exchange_cached(&mut cached).await.unwrap()).unwrap_err();
        let Error::Remote(remote) = &error else {
            panic!("Lost the remote failure: {error:?}");
        };
        assert_eq!(remote.code, "not_ready");
        assert_eq!(remote.retry, Retry::Yes);
        assert_eq!(remote.message, "not ready");
        assert_eq!(
            error.source().unwrap().downcast_ref::<RemoteError>(),
            Some(remote)
        );
        assert_eq!(error.to_string(), "not ready");
        assert!(cached.is_some());
        assert_eq!(
            answer(exchange_cached(&mut cached).await.unwrap()).unwrap(),
            shown(true)
        );
        assert!(cached.is_some());
        task.await.unwrap();
    }
    async fn raw_reply(bytes: Vec<u8>) -> (Result<Reply, Error>, bool) {
        let (session, mut agent) = fixture_session();
        let mut cached = Some(session);
        let task = tokio::spawn(async move {
            read_call(&mut agent).await;
            let _ = agent.get_mut().write_all(&bytes).await;
        });
        let result = exchange_cached(&mut cached).await;
        task.await.unwrap();
        (result, cached.is_some())
    }
    #[tokio::test]
    async fn stale_malformed_truncated_and_oversized_replies_close_the_channel() {
        let call = Call {
            id: 2,
            request: Request::StableDisplay,
        };
        let mut oversize = Vec::new();
        newport_protocol::encoding::protobuf::write_frame(
            &mut oversize,
            &vec![0; SERVER_LIMITS.message + 1],
        )
        .unwrap();
        oversize.truncate(5);
        for bytes in [
            response(&call, shown(true)),
            vec![],
            vec![0x82],
            vec![2, 0x0a],
            vec![1, 0],
            oversize,
        ] {
            let (result, reused) = raw_reply(bytes).await;
            let error = result.unwrap_err();
            match &error {
                Error::Decode(_) => {
                    assert!(error.source().unwrap().is::<wire::Error>());
                }
                Error::Transport(FrameError::Truncated | FrameError::TooLarge { .. })
                | Error::Closed => (),
                error => panic!("Unexpected failure category: {error:?}"),
            }
            assert!(!reused);
        }
    }
    #[tokio::test]
    async fn transport_failure_retains_its_io_cause_and_discards_the_channel() {
        let (session, agent) = fixture_session();
        let mut cached = Some(session);
        drop(agent);
        let error = exchange_cached(&mut cached).await.unwrap_err();
        assert!(matches!(&error, Error::Transport(FrameError::Io(_))));
        let io = error
            .source()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap();
        assert_eq!(io.kind(), std::io::ErrorKind::BrokenPipe);
        assert!(cached.is_none());
    }
    #[tokio::test]
    async fn a_stalled_reply_expires_and_discards_the_channel() {
        let (session, mut agent) = fixture_session();
        let mut cached = Some(session);
        let task = tokio::spawn(async move {
            read_call(&mut agent).await;
            let (mut transport, _) = agent.into_parts();
            let mut rest = Vec::new();
            transport.read_to_end(&mut rest).await.unwrap();
        });
        let result = exchange_session(&mut cached, |mut session| async move {
            let call = Call {
                id: 1,
                request: Request::StableDisplay,
            };
            let reply = exchange(
                &mut session.reader,
                &mut session.writer,
                &call,
                Duration::from_millis(50),
            )
            .await?;
            Ok((session, reply))
        })
        .await;
        let error = result.unwrap_err();
        assert!(matches!(
            &error,
            Error::Timeout { duration, .. } if *duration == Duration::from_millis(50)
        ));
        assert!(error.source().unwrap().is::<tokio::time::error::Elapsed>());
        assert!(error.to_string().contains("did not answer"));
        assert!(cached.is_none());
        task.await.unwrap();
    }
    #[test]
    fn only_connection_failures_are_retried_at_once() {
        use crate::agent_runtime::Error;
        for error in [
            Error::Interrupted(crate::agent_runtime::BUSY.into()),
            Error::Interrupted("Needle runtime disconnected".into()),
            Error::Link(newport_ssh::Error::Lost("reset".into())),
            Error::Link(newport_ssh::Error::TimedOut),
            Error::Link(newport_ssh::Error::SessionsDisabled),
        ] {
            assert!(error.transient(), "{error}");
        }
        for error in [
            Error::Unavailable("The Newport agent requires Linux.".into()),
            Error::Unavailable("The Newport agent supports x86_64 and ARM64 Linux servers.".into()),
            Error::NotInstalled,
        ] {
            assert!(!error.transient(), "{error}");
        }
        assert_eq!(
            unavailable(
                &Request::Metrics,
                Error::Unavailable("The Newport agent requires Linux.".into()),
            )
            .to_string(),
            "Monitoring needs the Newport agent, which can’t run on this server: The Newport agent requires Linux."
        );
    }
}
