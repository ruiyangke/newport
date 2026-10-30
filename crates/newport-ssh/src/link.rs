//! The link: one task that owns every connection to a server, places each
//! requested channel, logs in one connection at a time, and publishes the
//! server's status. Everything else talks to it through messages.
use crate::{
    Abort, Authenticator, Channel, Dialer, Error, Handler, Login, Parts, Reach, Status,
    handler::HostKey,
    listener::{BACKLOG, Forward, Forwards, Listen, Listener, Routes, forward_in_order},
};
use russh::{ChannelId, ChannelOpenFailure, client};
use std::{
    collections::{HashSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};

pub(crate) type ConnectionId = u64;

/// Where and as whom.
#[derive(Clone, Debug)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub user: String,
}

/// Deadlines and sizes. The defaults are the design's; tests shorten them.
#[derive(Clone, Debug)]
pub struct Limits {
    pub max_connections: usize,
    pub max_waiting: usize,
    /// A new connection's session limit until a refusal says otherwise.
    pub default_sessions: usize,
    pub dial: Duration,
    pub handshake: Duration,
    /// Network time a login may take; time waiting for approval doesn't count.
    pub login: Duration,
    /// From placing a request on a ready connection to its channel opening.
    pub open: Duration,
    /// A connection with no channels for this long closes.
    pub idle: Duration,
    pub probe: Duration,
    pub backoff_min: Duration,
    pub backoff_max: Duration,
    pub keepalive: Duration,
    pub keepalive_max: usize,
    /// A connection Ready this long ends a run of login failures.
    pub healthy: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_connections: 5,
            max_waiting: 256,
            default_sessions: 10,
            dial: Duration::from_secs(10),
            handshake: Duration::from_secs(20),
            login: Duration::from_secs(20),
            open: Duration::from_secs(10),
            idle: Duration::from_secs(60),
            probe: Duration::from_secs(3),
            backoff_min: Duration::from_secs(2),
            backoff_max: Duration::from_secs(30),
            keepalive: Duration::from_secs(15),
            keepalive_max: 2,
            healthy: Duration::from_secs(60),
        }
    }
}

/// A channel to ask for, in SSH's terms.
#[derive(Clone, Debug)]
pub enum Open {
    /// Counts against the server's `MaxSessions`.
    Session,
    DirectTcpip {
        host: String,
        port: u16,
    },
    DirectStreamLocal {
        path: String,
    },
}

/// What a request waits for.
#[derive(Clone, Debug)]
enum Want {
    Open(Open),
    Listen { address: String, port: u32 },
}

impl Want {
    fn session(&self) -> bool {
        matches!(self, Want::Open(Open::Session))
    }
}

/// Everything Newport has open to one server. Cloning shares the link;
/// dropping the last clone closes its connections.
#[derive(Clone)]
pub struct Link {
    commands: mpsc::UnboundedSender<Command>,
    status: watch::Receiver<Status>,
    pressure: watch::Receiver<u64>,
    requests: Arc<AtomicU64>,
}

impl Link {
    pub fn new(target: Target, parts: Parts, limits: Limits) -> Self {
        let (link, actor) = Actor::new(target, parts, limits);
        tokio::spawn(actor.run());
        link
    }

    /// A channel on the oldest ready connection with room, logging in another
    /// connection when none has room. Cancel-safe: dropping the future gives
    /// back its place, and a channel confirmed after that is closed.
    pub async fn open(&self, open: Open) -> Result<Channel, Error> {
        self.request(Want::Open(open), Patience::Retries, Reply::Channel)
            .await
    }

    /// Opens like [`Link::open`], but while the link has stopped retrying
    /// ([`Reach::Failed`]) it fails at once with the error that stopped it
    /// instead of waiting for Retry, including when the link stops while the
    /// request waits. Requests and retries are taken in the order they are
    /// sent, so after [`Link::retry`] or [`Link::retry_fresh`] this never
    /// sees the failure the retry cleared.
    pub async fn open_unless_stopped(&self, open: Open) -> Result<Channel, Error> {
        self.request(Want::Open(open), Patience::UntilStopped, Reply::Channel)
            .await
    }

    /// Opens like [`Link::open_unless_stopped`], and also fails with a
    /// login's error when a login fails while the request waits, though the
    /// link goes on trying. A failure from before the request doesn't count.
    pub async fn open_unless_failing(&self, open: Open) -> Result<Channel, Error> {
        self.request(Want::Open(open), Patience::OneLogin, Reply::Channel)
            .await
    }

    /// A remote forward on the oldest ready connection, which stays open
    /// while the listener lives. Cancel-safe like [`Link::open`].
    pub async fn listen_tcp(&self, address: &str, port: u32) -> Result<Listener, Error> {
        let want = Want::Listen {
            address: address.into(),
            port,
        };
        self.request(want, Patience::Retries, Reply::Listener).await
    }

    async fn request<T>(
        &self,
        want: Want,
        patience: Patience,
        reply: fn(oneshot::Sender<Result<T, Error>>) -> Reply,
    ) -> Result<T, Error> {
        let id = self.requests.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        let request = Request {
            id,
            want,
            patience,
            reply: Some(reply(sender)),
        };
        self.commands
            .send(Command::Request(request))
            .map_err(|_| Error::Closed)?;
        let mut cancel = CancelOnDrop {
            commands: &self.commands,
            id,
            armed: true,
        };
        let result = receiver.await.map_err(|_| Error::Closed)?;
        cancel.armed = false;
        result
    }

    pub fn status(&self) -> watch::Receiver<Status> {
        self.status.clone()
    }

    /// Bumped once for each session request that has to wait for a slot on
    /// the ready connections, so idle long-lived sessions can give theirs up.
    pub fn pressure(&self) -> watch::Receiver<u64> {
        self.pressure.clone()
    }

    /// Try again now: clear a login failure that stopped automatic attempts
    /// and the backoff. A login waiting for approval keeps its prompt. The
    /// host key is checked afresh, and sessions are asked for again. The
    /// dialer passes it on, so a jump host retries too.
    pub fn retry(&self) {
        let _ = self.commands.send(Command::Retry { fresh: false });
    }

    /// The person asked to try again: as [`Link::retry`], and a login waiting
    /// for approval, whose prompt may be hidden or stale, is replaced with a
    /// fresh one.
    pub fn retry_fresh(&self) {
        let _ = self.commands.send(Command::Retry { fresh: true });
    }

    /// The computer is going to sleep: start no logins until it wakes.
    pub fn sleep(&self) {
        let _ = self.commands.send(Command::Sleep);
    }

    /// The computer woke: check every connection, give up a login started
    /// before sleep, and stop waiting out the backoff.
    pub fn wake(&self) {
        let _ = self.commands.send(Command::Wake);
    }

    /// The network changed: check every connection, and stop waiting out the
    /// backoff.
    pub fn network_changed(&self) {
        let _ = self.commands.send(Command::NetworkChanged);
    }
}

struct CancelOnDrop<'a> {
    commands: &'a mpsc::UnboundedSender<Command>,
    id: u64,
    armed: bool,
}

impl Drop for CancelOnDrop<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.commands.send(Command::Cancel(self.id));
        }
    }
}

pub(crate) enum Reply {
    Channel(oneshot::Sender<Result<Channel, Error>>),
    Listener(oneshot::Sender<Result<Listener, Error>>),
}

impl Reply {
    fn is_closed(&self) -> bool {
        match self {
            Reply::Channel(reply) => reply.is_closed(),
            Reply::Listener(reply) => reply.is_closed(),
        }
    }
    fn fail(self, error: Error) {
        match self {
            Reply::Channel(reply) => drop(reply.send(Err(error))),
            Reply::Listener(reply) => drop(reply.send(Err(error))),
        }
    }
}

enum Command {
    Request(Request),
    Cancel(u64),
    /// `fresh` replaces a login waiting for approval.
    Retry {
        fresh: bool,
    },
    Sleep,
    Wake,
    NetworkChanged,
}

/// How long a request waits for a connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Patience {
    /// Through failures and stops, until a connection is ready.
    Retries,
    /// Fails while automatic logins are stopped.
    UntilStopped,
    /// Fails as `UntilStopped`, and when a login fails while it waits.
    OneLogin,
}

pub(crate) struct Request {
    id: u64,
    want: Want,
    patience: Patience,
    /// Taken when the requester was answered early, for example with a timeout.
    pub(crate) reply: Option<Reply>,
}

impl Request {
    fn waiting(&self) -> bool {
        self.reply.as_ref().is_some_and(|reply| !reply.is_closed())
    }
    pub(crate) fn fail(mut self, error: Error) {
        self.fail_early(error);
    }
    /// Answers the requester now; the request's outcome is still handled.
    pub(crate) fn fail_early(&mut self, error: Error) {
        if let Some(reply) = self.reply.take() {
            reply.fail(error);
        }
    }
}

pub(crate) enum Event {
    Approval(ConnectionId, bool),
    LoggedIn(ConnectionId, Arc<client::Handle<Handler>>, Abort),
    LoginFailed(ConnectionId, Error),
    Lost(ConnectionId, String),
    ChannelClosed(ConnectionId, ChannelId),
    /// The server opened a channel that a listener took.
    Accepted(ConnectionId, ChannelId),
    /// A listener is gone, or its forward was never set up.
    Unlisten(ConnectionId),
    Opened {
        connection: ConnectionId,
        channel: ChannelId,
        session: bool,
    },
    Refused {
        connection: ConnectionId,
        request: Request,
        reason: ChannelOpenFailure,
    },
    OpenFailed {
        connection: ConnectionId,
        request: Request,
    },
    OpenTimedOut,
    ProbeFailed(ConnectionId),
}

enum State {
    Connecting { approval: bool },
    Ready(Arc<client::Handle<Handler>>),
}

struct Connection {
    id: ConnectionId,
    state: State,
    /// Sessions requested or open here, counted until the server closes them.
    sessions: usize,
    forwarding: usize,
    /// Listeners pinned here, including forwards still being set up.
    listeners: usize,
    routes: Routes,
    limit: usize,
    /// The most sessions this connection has held at once: its limit is at
    /// least this, whatever order refusals and closes arrive in.
    most_sessions: usize,
    session_channels: HashSet<ChannelId>,
    forwarding_channels: HashSet<ChannelId>,
    /// Closes that arrived before their open was recorded.
    closed_early: HashSet<ChannelId>,
    idle_since: Option<Instant>,
    ready_since: Option<Instant>,
    lost: watch::Sender<Option<Error>>,
    abort: Option<Abort>,
    /// Set once ready.
    forwards: Option<Forwards>,
    tasks: Vec<JoinHandle<()>>,
}

impl Connection {
    fn ready(&self) -> Option<&Arc<client::Handle<Handler>>> {
        match &self.state {
            State::Ready(handle) => Some(handle),
            State::Connecting { .. } => None,
        }
    }
    /// What keeps the connection from closing as idle.
    fn leases(&self) -> usize {
        self.sessions + self.forwarding + self.listeners
    }
    fn close(&mut self, error: Error) {
        let _ = self.lost.send(Some(error));
        if let Some(abort) = &self.abort {
            abort.abort();
        }
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }
}

struct Actor {
    target: Target,
    dialer: Arc<dyn Dialer>,
    authenticator: Arc<dyn Authenticator>,
    pressure: watch::Sender<u64>,
    /// Session requests that waited for a slot at the last pump.
    pressured: HashSet<u64>,
    limits: Limits,
    commands: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<Event>,
    event_receiver: mpsc::UnboundedReceiver<Event>,
    status: watch::Sender<Status>,
    /// In the order they were opened: the oldest first.
    connections: Vec<Connection>,
    next_connection: ConnectionId,
    queue: VecDeque<Request>,
    /// The one connection being logged in.
    attempt: Option<ConnectionId>,
    login_error: Option<Error>,
    /// A failure that stops automatic attempts until Retry.
    blocked: Option<Error>,
    failures: u32,
    backoff_until: Option<Instant>,
    ever_ready: bool,
    sleeping: bool,
    /// Counts sleeps, so a wake can tell a login from before sleep.
    sleep_epoch: u64,
    /// The sleep epoch the login in progress started in.
    attempt_epoch: Option<u64>,
    /// A requester gave up since the last pump.
    abandoned: bool,
    had_session: bool,
    sessions_disabled: bool,
    /// The highest session limit a refusal taught in this run.
    learned_limit: Option<usize>,
    /// A connection refused its first session while another holds sessions:
    /// open no more for sessions until none does.
    secondary_refused: bool,
    host_key: Arc<HostKey>,
}

impl Actor {
    fn new(target: Target, parts: Parts, limits: Limits) -> (Link, Actor) {
        let (commands, receiver) = mpsc::unbounded_channel();
        let (pressure, pressure_receiver) = watch::channel(0);
        let (events, event_receiver) = mpsc::unbounded_channel();
        let (status, status_receiver) = watch::channel(Status::default());
        let host_key = Arc::new(HostKey::new(target.clone(), parts.host_keys));
        let actor = Actor {
            target,
            dialer: parts.dialer,
            authenticator: parts.authenticator,
            pressure,
            pressured: HashSet::new(),
            limits,
            commands: receiver,
            events,
            event_receiver,
            status,
            connections: Vec::new(),
            next_connection: 0,
            queue: VecDeque::new(),
            attempt: None,
            login_error: None,
            blocked: None,
            failures: 0,
            backoff_until: None,
            ever_ready: false,
            sleeping: false,
            sleep_epoch: 0,
            attempt_epoch: None,
            abandoned: false,
            had_session: false,
            sessions_disabled: false,
            learned_limit: None,
            secondary_refused: false,
            host_key,
        };
        let link = Link {
            commands,
            status: status_receiver,
            pressure: pressure_receiver,
            requests: Arc::default(),
        };
        (link, actor)
    }

    async fn run(mut self) {
        loop {
            let wake = self.next_wake();
            tokio::select! {
                command = self.commands.recv() => match command {
                    Some(command) => self.command(command),
                    None => break,
                },
                Some(event) = self.event_receiver.recv() => self.event(event),
                () = tokio::time::sleep_until(wake.into()) => {}
            }
            self.pump();
        }
        for connection in &mut self.connections {
            connection.close(Error::Closed);
        }
        for request in self.queue.drain(..) {
            request.fail(Error::Closed);
        }
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::Request(request) => {
                if self.queue.len() >= self.limits.max_waiting {
                    request.fail(Error::Busy);
                } else {
                    self.queue.push_back(request);
                }
            }
            Command::Cancel(id) => {
                let waiting = self.queue.len();
                self.queue.retain(|request| request.id != id);
                self.abandoned |= self.queue.len() < waiting;
            }
            Command::Retry { fresh } => {
                // First, so a jump host has its own Retry before this link's
                // cancelled dial withdraws from it.
                if fresh {
                    self.dialer.retry_fresh();
                    if self.approving() {
                        self.cancel_attempt();
                    }
                } else {
                    self.dialer.retry();
                }
                // The person may have fixed `known_hosts`.
                self.host_key.unpin();
                self.blocked = None;
                self.login_error = None;
                self.secondary_refused = false;
                self.sessions_disabled = false;
                self.failures = 0;
                self.backoff_until = None;
            }
            Command::Sleep => {
                self.sleeping = true;
                self.sleep_epoch += 1;
            }
            Command::Wake => {
                self.sleeping = false;
                // A prompt shown before sleep is almost certainly gone.
                if self
                    .attempt_epoch
                    .is_some_and(|epoch| epoch < self.sleep_epoch)
                {
                    self.cancel_attempt();
                }
                self.backoff_until = None;
                self.probe_all();
            }
            Command::NetworkChanged => {
                self.backoff_until = None;
                self.probe_all();
            }
        }
    }

    fn event(&mut self, event: Event) {
        match event {
            // A login replaced by Retry, wake or a request served elsewhere
            // may still report before its task ends.
            Event::Approval(id, waiting) => {
                if self.attempt == Some(id)
                    && let Some(connection) = self.connection(id)
                {
                    connection.state = State::Connecting { approval: waiting };
                }
            }
            Event::LoggedIn(id, handle, abort) => {
                if self.attempt == Some(id) {
                    self.logged_in(id, handle, abort);
                } else {
                    abort.abort();
                }
            }
            Event::LoginFailed(id, error) => {
                if self.attempt == Some(id) {
                    self.login_failed(id, error);
                }
            }
            Event::Lost(id, cause) => {
                // A failed handshake also disconnects; its attempt reports it.
                if self.connection(id).is_some_and(|c| c.ready().is_some()) {
                    self.lose(id, Error::Lost(cause));
                }
            }
            Event::ProbeFailed(id) => {
                self.lose(id, Error::Lost("the server didn't answer a probe".into()))
            }
            Event::ChannelClosed(id, channel) => self.channel_closed(id, channel),
            Event::Accepted(id, channel) => {
                if let Some(connection) = self.connection(id) {
                    connection.forwarding += 1;
                    connection.idle_since = None;
                }
                self.opened(id, channel, false);
            }
            Event::Unlisten(id) => {
                if let Some(connection) = self.connection(id) {
                    connection.listeners -= 1;
                }
            }
            Event::Opened {
                connection,
                channel,
                session,
            } => self.opened(connection, channel, session),
            Event::Refused {
                connection,
                request,
                reason,
            } => self.refused(connection, request, reason),
            Event::OpenFailed {
                connection,
                request,
            } => {
                // Only a broken connection fails an open without refusing it.
                self.queue.push_front(request);
                self.lose(connection, Error::Lost("a channel couldn't open".into()));
            }
            Event::OpenTimedOut => self.probe_all(),
        }
    }

    fn connection(&mut self, id: ConnectionId) -> Option<&mut Connection> {
        self.connections.iter_mut().find(|c| c.id == id)
    }

    fn logged_in(&mut self, id: ConnectionId, handle: Arc<client::Handle<Handler>>, abort: Abort) {
        self.attempt = None;
        self.attempt_epoch = None;
        self.backoff_until = None;
        self.login_error = None;
        self.ever_ready = true;
        let closed = handle.is_closed();
        let Some(connection) = self.connection(id) else {
            abort.abort();
            return;
        };
        let (forwards, queued) = mpsc::unbounded_channel();
        connection
            .tasks
            .push(tokio::spawn(forward_in_order(handle.clone(), queued)));
        connection.forwards = Some(forwards);
        connection.state = State::Ready(handle);
        connection.ready_since = Some(Instant::now());
        connection.abort = Some(abort);
        connection.idle_since = Some(Instant::now());
        // Its handler's report came before it was ready, and was passed over.
        if closed {
            self.lose(id, Error::Lost("the connection closed".into()));
        }
    }

    fn login_failed(&mut self, id: ConnectionId, error: Error) {
        self.attempt = None;
        self.attempt_epoch = None;
        if let Some(index) = self.connections.iter().position(|c| c.id == id) {
            let mut connection = self.connections.remove(index);
            connection.close(error.clone());
        }
        log::info!("Login to {} failed: {error}", self.target.host);
        for request in std::mem::take(&mut self.queue) {
            if request.patience == Patience::OneLogin {
                request.fail(error.clone());
            } else {
                self.queue.push_back(request);
            }
        }
        match error.retry() {
            crate::Retry::Backoff => {
                self.failures += 1;
                self.backoff_until = Some(Instant::now() + self.backoff(self.failures));
            }
            crate::Retry::OnRetry | crate::Retry::Never => self.blocked = Some(error.clone()),
        }
        self.login_error = Some(error);
    }

    fn backoff(&self, failures: u32) -> Duration {
        let doubled = self
            .limits
            .backoff_min
            .saturating_mul(1 << failures.saturating_sub(1).min(16));
        doubled
            .min(self.limits.backoff_max)
            .mul_f64(0.5 + fastrand::f64() / 2.)
    }

    /// A ready connection closed: its leases learn at once, and the others
    /// are checked, since they likely share its network.
    fn lose(&mut self, id: ConnectionId, error: Error) {
        let Some(index) = self.connections.iter().position(|c| c.id == id) else {
            return;
        };
        let mut connection = self.connections.remove(index);
        log::info!("Connection {id} to {} lost: {error}", self.target.host);
        if connection
            .ready_since
            .is_some_and(|since| since.elapsed() >= self.limits.healthy)
        {
            self.failures = 0;
        }
        // The sessions that made a refusal secondary may have been here.
        self.secondary_refused = false;
        connection.close(error);
        self.probe_all();
    }

    /// Whether the login in progress waits for the person to approve it.
    fn approving(&self) -> bool {
        self.attempt
            .and_then(|id| self.connections.iter().find(|c| c.id == id))
            .is_some_and(|c| matches!(c.state, State::Connecting { approval: true }))
    }

    /// Give up the login in progress; its agent request is withdrawn.
    fn cancel_attempt(&mut self) {
        let Some(id) = self.attempt.take() else {
            return;
        };
        self.attempt_epoch = None;
        if let Some(index) = self.connections.iter().position(|c| c.id == id) {
            let mut connection = self.connections.remove(index);
            connection.close(Error::Closed);
        }
    }

    fn probe_all(&mut self) {
        for connection in &mut self.connections {
            let Some(handle) = connection.ready().cloned() else {
                continue;
            };
            let id = connection.id;
            let events = self.events.clone();
            let limit = self.limits.probe;
            connection.tasks.retain(|task| !task.is_finished());
            connection.tasks.push(tokio::spawn(async move {
                // A ping also returns when the connection drops, so only a
                // reply on a still-open connection counts.
                let answered = tokio::time::timeout(limit, handle.send_ping()).await;
                if answered.is_err() || handle.is_closed() {
                    let _ = events.send(Event::ProbeFailed(id));
                }
            }));
        }
    }

    fn opened(&mut self, id: ConnectionId, channel: ChannelId, session: bool) {
        if session {
            self.had_session = true;
        }
        let Some(connection) = self.connection(id) else {
            return;
        };
        if connection.closed_early.remove(&channel) {
            if session {
                connection.sessions -= 1;
            } else {
                connection.forwarding -= 1;
            }
        } else if session {
            connection.session_channels.insert(channel);
            connection.most_sessions = connection
                .most_sessions
                .max(connection.session_channels.len());
        } else {
            connection.forwarding_channels.insert(channel);
        }
    }

    fn channel_closed(&mut self, id: ConnectionId, channel: ChannelId) {
        let Some(connection) = self.connection(id) else {
            return;
        };
        if connection.session_channels.remove(&channel) {
            connection.sessions -= 1;
            self.secondary_refused = false;
        } else if connection.forwarding_channels.remove(&channel) {
            connection.forwarding -= 1;
        } else {
            connection.closed_early.insert(channel);
        }
    }

    fn refused(&mut self, id: ConnectionId, request: Request, reason: ChannelOpenFailure) {
        let had_session = self.had_session;
        let Some(connection) = self.connection(id) else {
            self.queue.push_front(request);
            return;
        };
        if !request.want.session() {
            connection.forwarding -= 1;
            request.fail(Error::ChannelRefused { reason });
            return;
        }
        connection.sessions -= 1;
        if connection.sessions > 0 {
            // OpenSSH refuses a session over `MaxSessions` this way. With
            // several opens in flight the first refusals overcount, so the
            // limit comes down towards the true one, but never below what
            // this connection has held at once.
            let counted = connection.sessions.max(connection.most_sessions);
            connection.limit = connection.limit.min(counted);
            log::info!(
                "Connection {id} holds at most {} sessions",
                connection.limit
            );
            let limit = connection.limit;
            self.learned_limit = Some(self.learned_limit.map_or(limit, |l| l.max(limit)));
            self.queue.push_front(request);
            return;
        }
        let never_opened = !had_session && connection.closed_early.is_empty();
        let prohibited = matches!(
            reason,
            ChannelOpenFailure::ConnectFailed | ChannelOpenFailure::AdministrativelyProhibited
        );
        if never_opened && prohibited {
            self.sessions_disabled = true;
            request.fail(Error::SessionsDisabled);
        } else {
            // A server may refuse a connection's sessions while others hold
            // theirs, or for a while: drop the connection, and wait for a
            // slot elsewhere or try again later.
            self.queue.push_front(request);
            self.discard(id);
            if self.connections.iter().any(|c| c.sessions > 0) {
                self.secondary_refused = true;
            } else {
                self.failures += 1;
                self.backoff_until = Some(Instant::now() + self.backoff(self.failures));
            }
        }
    }

    /// Close a connection nobody should use, keeping its forwarding channels
    /// and listeners if it has any.
    fn discard(&mut self, id: ConnectionId) {
        let Some(index) = self.connections.iter().position(|c| c.id == id) else {
            return;
        };
        if self.connections[index].forwarding + self.connections[index].listeners > 0 {
            self.connections[index].limit = 0;
        } else {
            let connection = self.connections.remove(index);
            self.close_gracefully(connection);
        }
    }

    /// Place what fits, log in when something doesn't, close idle
    /// connections, and publish the status.
    fn pump(&mut self) {
        if self
            .backoff_until
            .is_some_and(|until| until <= Instant::now())
        {
            self.backoff_until = None;
        }
        if self.connections.iter().any(|c| {
            c.ready_since
                .is_some_and(|since| since.elapsed() >= self.limits.healthy)
        }) {
            self.failures = 0;
        }
        let waiting = self.queue.len();
        self.queue.retain(Request::waiting);
        let abandoned = std::mem::take(&mut self.abandoned) || self.queue.len() < waiting;
        let mut waiting = VecDeque::new();
        while let Some(request) = self.queue.pop_front() {
            if request.want.session() && self.sessions_disabled {
                request.fail(Error::SessionsDisabled);
                continue;
            }
            match (self.place(&request.want), &self.blocked) {
                (Some(index), _) => self.start(index, request),
                (None, Some(error)) if request.patience != Patience::Retries => {
                    request.fail(error.clone())
                }
                (None, _) => waiting.push_back(request),
            }
        }
        self.queue = waiting;
        self.raise_pressure();
        if !self.connections.iter().any(|c| c.sessions > 0) {
            self.secondary_refused = false;
        }
        if self.queue.is_empty() && self.attempt.is_some() {
            // Asking again by itself would bring back a prompt the person
            // walked away from.
            if abandoned && self.approving() {
                self.blocked = Some(Error::ApprovalRequired);
                self.login_error = Some(Error::ApprovalRequired);
            }
            self.cancel_attempt();
        }
        if self.wants_connection() {
            self.start_attempt();
        }
        self.close_idle();
        self.publish();
    }

    /// Counts each session request that starts waiting while connections are
    /// ready: it waits for a slot, not only for a login.
    fn raise_pressure(&mut self) {
        let waiting: HashSet<u64> = self
            .queue
            .iter()
            .filter(|r| r.want.session())
            .map(|r| r.id)
            .collect();
        self.pressured.retain(|id| waiting.contains(id));
        if !self.connections.iter().any(|c| c.ready().is_some()) {
            return;
        }
        let started = waiting
            .into_iter()
            .filter(|id| self.pressured.insert(*id))
            .count() as u64;
        if started > 0 {
            self.pressure.send_modify(|pressure| *pressure += started);
        }
    }

    /// First fit by age.
    fn place(&self, want: &Want) -> Option<usize> {
        self.connections.iter().position(|connection| {
            connection.ready().is_some()
                && (!want.session() || connection.sessions < connection.limit)
        })
    }

    fn wants_connection(&self) -> bool {
        if self.queue.is_empty()
            || self.sleeping
            || self.attempt.is_some()
            || self.blocked.is_some()
            || self
                .backoff_until
                .is_some_and(|until| Instant::now() < until)
            || self.connections.len() >= self.limits.max_connections
        {
            return false;
        }
        // Forwarding channels and listeners wait only while no connection is
        // ready.
        let forwarding = self.queue.iter().any(|request| !request.want.session());
        forwarding || !self.secondary_refused
    }

    fn start(&mut self, index: usize, request: Request) {
        match request.want.clone() {
            Want::Open(open) => self.start_open(index, open, request),
            Want::Listen { address, port } => self.start_listen(index, address, port, request),
        }
    }

    fn start_open(&mut self, index: usize, open: Open, mut request: Request) {
        let session = matches!(open, Open::Session);
        let deadline = self.limits.open;
        let events = self.events.clone();
        let connection = &mut self.connections[index];
        let handle = connection
            .ready()
            .expect("placed on a ready connection")
            .clone();
        if session {
            connection.sessions += 1;
        } else {
            connection.forwarding += 1;
        }
        connection.idle_since = None;
        let id = connection.id;
        let lost = connection.lost.subscribe();
        tokio::spawn(async move {
            let opening = open_channel(&handle, &open);
            tokio::pin!(opening);
            // The open runs to its end even if the requester gives up, so a
            // channel the server confirms late is closed, not leaked.
            let result = tokio::select! {
                result = &mut opening => result,
                () = tokio::time::sleep(deadline) => {
                    request.fail_early(Error::TimedOut);
                    let _ = events.send(Event::OpenTimedOut);
                    opening.await
                }
            };
            match result {
                Ok(channel) => {
                    let _ = events.send(Event::Opened {
                        connection: id,
                        channel: channel.id(),
                        session,
                    });
                    // An unanswered channel is dropped here, which closes it.
                    let channel = Channel::new(channel, id, lost);
                    if let Some(Reply::Channel(reply)) = request.reply.take() {
                        let _ = reply.send(Ok(channel));
                    }
                }
                Err(russh::Error::ChannelOpenFailure(reason)) => {
                    let _ = events.send(Event::Refused {
                        connection: id,
                        request,
                        reason,
                    });
                }
                Err(_) => {
                    let _ = events.send(Event::OpenFailed {
                        connection: id,
                        request,
                    });
                }
            }
        });
    }

    /// Pins a listener to the connection at once, so it can't close as idle
    /// while the server answers.
    fn start_listen(&mut self, index: usize, address: String, port: u32, request: Request) {
        let deadline = self.limits.open;
        let events = self.events.clone();
        let connection = &mut self.connections[index];
        let forwards = connection
            .forwards
            .clone()
            .expect("placed on a ready connection");
        let routes = connection.routes.clone();
        let (sender, channels) = mpsc::channel(BACKLOG);
        // A port asked for by number is routed before the server answers, so
        // a connection arriving right after the reply isn't refused.
        if port != 0 {
            let mut routes = routes.lock().unwrap();
            if routes
                .ports
                .get(&port)
                .is_some_and(|route| !route.is_closed())
            {
                request.fail(Error::RequestDenied);
                return;
            }
            routes.ports.insert(port, sender.clone());
        }
        connection.listeners += 1;
        connection.idle_since = None;
        // Queued behind the connection's earlier forward requests, such as
        // the cancel of a listener on the same port dropped just before.
        let _ = forwards.send(Forward::Listen(Listen {
            request,
            address,
            port,
            sender,
            channels,
            deadline,
            connection: connection.id,
            routes,
            lost: connection.lost.subscribe(),
            events,
            forwards: forwards.clone(),
        }));
    }

    fn start_attempt(&mut self) {
        let id = self.next_connection;
        self.next_connection += 1;
        self.attempt = Some(id);
        self.attempt_epoch = Some(self.sleep_epoch);
        let (lost, _) = watch::channel(None);
        let routes = Routes::default();
        let handler = Handler {
            connection: id,
            events: self.events.clone(),
            host_key: self.host_key.clone(),
            routes: routes.clone(),
            lost: lost.subscribe(),
        };
        let task = tokio::spawn(login(
            id,
            self.target.clone(),
            self.dialer.clone(),
            self.authenticator.clone(),
            self.limits.clone(),
            handler,
            self.events.clone(),
        ));
        self.connections.push(Connection {
            id,
            state: State::Connecting { approval: false },
            sessions: 0,
            forwarding: 0,
            listeners: 0,
            routes,
            limit: self.learned_limit.unwrap_or(self.limits.default_sessions),
            most_sessions: 0,
            session_channels: HashSet::new(),
            forwarding_channels: HashSet::new(),
            closed_early: HashSet::new(),
            idle_since: None,
            ready_since: None,
            lost,
            abort: None,
            forwards: None,
            tasks: vec![task],
        });
    }

    fn in_use(&self) -> bool {
        !self.queue.is_empty()
            || self.attempt.is_some()
            || self.connections.iter().any(|c| c.leases() > 0)
    }

    /// A connection without channels or listeners closes after the idle time,
    /// unless it is the last one and the link is in use.
    fn close_idle(&mut self) {
        let now = Instant::now();
        let idle = self.limits.idle;
        let in_use = self.in_use();
        let mut ready = self
            .connections
            .iter()
            .filter(|c| c.ready().is_some())
            .count();
        let mut closed = Vec::new();
        for connection in &mut self.connections {
            if connection.ready().is_none() || connection.leases() > 0 {
                continue;
            }
            let since = *connection.idle_since.get_or_insert(now);
            if now.duration_since(since) >= idle && !(ready == 1 && in_use) {
                closed.push(connection.id);
                ready -= 1;
            }
        }
        for id in closed {
            if let Some(index) = self.connections.iter().position(|c| c.id == id) {
                let connection = self.connections.remove(index);
                self.close_gracefully(connection);
            }
        }
    }

    /// Say goodbye to the server, then cut the socket whatever happens.
    fn close_gracefully(&self, mut connection: Connection) {
        let abort = connection.abort.take();
        let handle = connection.ready().cloned();
        connection.close(Error::Closed);
        tokio::spawn(async move {
            if let Some(handle) = handle {
                let goodbye = handle.disconnect(russh::Disconnect::ByApplication, "", "");
                let _ = tokio::time::timeout(Duration::from_secs(1), goodbye).await;
            }
            if let Some(abort) = abort {
                abort.abort();
            }
        });
    }

    fn next_wake(&self) -> Instant {
        let now = Instant::now();
        let mut wake = now + Duration::from_secs(3600);
        if let Some(until) = self.backoff_until {
            wake = wake.min(until);
        }
        if self.failures > 0 {
            for since in self.connections.iter().filter_map(|c| c.ready_since) {
                let healthy = since + self.limits.healthy;
                if healthy > now {
                    wake = wake.min(healthy);
                }
            }
        }
        for connection in &self.connections {
            if let Some(since) = connection.idle_since.filter(|_| connection.leases() == 0) {
                let due = since + self.limits.idle;
                if due > now {
                    wake = wake.min(due);
                }
            }
        }
        wake
    }

    fn publish(&self) {
        let ready = self
            .connections
            .iter()
            .filter(|c| c.ready().is_some())
            .count();
        let reach = if ready > 0 {
            Reach::Ready { connections: ready }
        } else if let Some(error) = &self.blocked {
            Reach::Failed(error.clone())
        } else if !self.in_use() {
            Reach::Idle
        } else if self.ever_ready || self.failures > 0 {
            Reach::Reconnecting {
                attempt: self.failures,
                next: self.backoff_until,
            }
        } else {
            Reach::Connecting
        };
        let login = match self
            .attempt
            .and_then(|id| self.connections.iter().find(|c| c.id == id))
        {
            Some(Connection {
                state: State::Connecting { approval: true },
                ..
            }) => Some(Login::WaitingForApproval),
            Some(_) => Some(Login::InProgress),
            None => self.login_error.clone().map(Login::Failed),
        };
        let status = Status {
            reach,
            login,
            waiting: self.queue.len(),
        };
        self.status.send_if_modified(|current| {
            let changed = *current != status;
            *current = status;
            changed
        });
    }
}

async fn open_channel(
    handle: &client::Handle<Handler>,
    open: &Open,
) -> Result<russh::Channel<client::Msg>, russh::Error> {
    match open {
        Open::Session => handle.channel_open_session().await,
        Open::DirectTcpip { host, port } => {
            handle
                .channel_open_direct_tcpip(host.clone(), u32::from(*port), "127.0.0.1", 0)
                .await
        }
        Open::DirectStreamLocal { path } => {
            handle.channel_open_direct_streamlocal(path.clone()).await
        }
    }
}

/// Dial, handshake and log in one connection, within the deadlines.
async fn login(
    id: ConnectionId,
    target: Target,
    dialer: Arc<dyn Dialer>,
    authenticator: Arc<dyn Authenticator>,
    limits: Limits,
    handler: Handler,
    events: mpsc::UnboundedSender<Event>,
) {
    let host_key = handler.host_key.clone();
    host_key.refused.lock().unwrap().take();
    let (approval, approving) = watch::channel(false);
    let mut approval_wait = ApprovalWait {
        id,
        approving,
        events: events.clone(),
    };
    let result = async {
        let dialed = approval_wait
            .within(
                dialer.dial(&target, &approval),
                dialer.deadline(limits.dial),
            )
            .await?;
        let abort = dialed.abort.clone();
        let config = Arc::new(client::Config {
            keepalive_interval: Some(limits.keepalive),
            keepalive_max: limits.keepalive_max,
            ..Default::default()
        });
        let mut handle = match tokio::time::timeout(
            limits.handshake,
            client::connect_stream(config, dialed.stream, handler),
        )
        .await
        {
            Err(_) => {
                abort.abort();
                return Err(Error::TimedOut);
            }
            Ok(Err(error)) => {
                return Err(host_key
                    .refused
                    .lock()
                    .unwrap()
                    .take()
                    .unwrap_or_else(|| handshake_failure(error)));
            }
            Ok(Ok(handle)) => handle,
        };
        let logging_in = authenticator.authenticate(&mut handle, &target, &approval);
        if let Err(error) = approval_wait.within(logging_in, Some(limits.login)).await {
            abort.abort();
            return Err(error);
        }
        Ok((handle, abort))
    }
    .await;
    let _ = events.send(match result {
        Ok((handle, abort)) => Event::LoggedIn(id, Arc::new(handle), abort),
        Err(error) => Event::LoginFailed(id, error),
    });
}

/// Reports a login's approval wait to the link.
struct ApprovalWait {
    id: ConnectionId,
    approving: watch::Receiver<bool>,
    events: mpsc::UnboundedSender<Event>,
}

impl ApprovalWait {
    /// Runs one step of a login within `limit` of network time: time spent
    /// waiting for the person to approve doesn't count.
    async fn within<T>(
        &mut self,
        step: impl Future<Output = Result<T, Error>>,
        limit: Option<Duration>,
    ) -> Result<T, Error> {
        tokio::pin!(step);
        let tick = Duration::from_millis(100);
        let mut network = Duration::ZERO;
        let result = loop {
            tokio::select! {
                result = &mut step => break result,
                Ok(()) = self.approving.changed() => self.report(),
                () = tokio::time::sleep(tick) => {
                    if !*self.approving.borrow() {
                        network += tick;
                        if limit.is_some_and(|limit| network >= limit) {
                            break Err(Error::TimedOut);
                        }
                    }
                }
            }
        };
        if self.approving.has_changed().unwrap_or(false) {
            self.report();
        }
        result
    }

    fn report(&mut self) {
        let waiting = *self.approving.borrow_and_update();
        let _ = self.events.send(Event::Approval(self.id, waiting));
    }
}

/// Why a handshake failed. A dialer's own error, such as a proxy command's
/// output, arrives inside russh's I/O error and is kept as it is.
fn handshake_failure(error: russh::Error) -> Error {
    if let russh::Error::IO(io) = &error
        && let Some(inner) = io.get_ref().and_then(|inner| inner.downcast_ref::<Error>())
    {
        return inner.clone();
    }
    Error::Unreachable(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BoxFuture, Dialed, HostKeys};
    use russh::keys::{PrivateKey, PublicKeyOrCertificate};
    use std::sync::atomic::AtomicBool;

    /// A dial that never finishes, so a login stays in progress.
    struct Stalled;

    impl Dialer for Stalled {
        fn dial<'a>(
            &'a self,
            _: &'a Target,
            _: &'a watch::Sender<bool>,
        ) -> BoxFuture<'a, Result<Dialed, Error>> {
            Box::pin(std::future::pending())
        }
    }

    impl Authenticator for Stalled {
        fn authenticate<'a>(
            &'a self,
            _: &'a mut client::Handle<Handler>,
            _: &'a Target,
            _: &'a watch::Sender<bool>,
        ) -> BoxFuture<'a, Result<(), Error>> {
            Box::pin(std::future::pending())
        }
    }

    impl HostKeys for Stalled {
        fn verify(&self, _: &Target, _: &PublicKeyOrCertificate) -> Result<(), Error> {
            Ok(())
        }
    }

    struct Server;

    impl russh::server::Handler for Server {
        type Error = russh::Error;
    }

    /// A connection whose handshake is done, as a login reports it, and the
    /// server's end of it.
    async fn handshake(actor: &Actor) -> (Arc<client::Handle<Handler>>, JoinHandle<()>) {
        let (near, far) = tokio::io::duplex(64 * 1024);
        let key = russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&[1; 32]);
        let config = Arc::new(russh::server::Config {
            keys: vec![PrivateKey::from(key)],
            ..Default::default()
        });
        let server = tokio::spawn(async move {
            if let Ok(session) = russh::server::run_stream(config, far, Server).await {
                let _ = session.await;
            }
        });
        let handler = Handler {
            connection: ConnectionId::MAX,
            events: actor.events.clone(),
            host_key: actor.host_key.clone(),
            routes: Routes::default(),
            lost: watch::channel(None).1,
        };
        let config = Arc::new(client::Config::default());
        let handle = client::connect_stream(config, near, handler).await.unwrap();
        (Arc::new(handle), server)
    }

    fn stalled() -> (Link, Actor) {
        let target = Target {
            host: "127.0.0.1".into(),
            port: 22,
            user: "fixture".into(),
        };
        let parts = Parts {
            dialer: Arc::new(Stalled),
            authenticator: Arc::new(Stalled),
            host_keys: Arc::new(Stalled),
        };
        Actor::new(target, parts, Limits::default())
    }

    #[tokio::test]
    async fn a_connection_reports_itself_lost_however_it_ends() {
        let (link, mut actor) = stalled();
        let (handle, _server) = handshake(&actor).await;
        handle
            .disconnect(russh::Disconnect::ByApplication, "", "")
            .await
            .unwrap();
        let event = tokio::time::timeout(Duration::from_secs(5), actor.event_receiver.recv())
            .await
            .expect("the ended connection reports itself")
            .unwrap();
        assert!(matches!(event, Event::Lost(ConnectionId::MAX, _)));
        assert!(handle.is_closed());

        // One that ends before the link records it as ready is lost at once.
        let _opening = {
            let link = link.clone();
            tokio::spawn(async move { link.open(Open::Session).await })
        };
        let request = actor.commands.recv().await.unwrap();
        actor.command(request);
        actor.pump();
        let id = actor.attempt.expect("a login");
        actor.event(Event::LoggedIn(id, handle, Abort::new(|| {})));
        assert!(actor.connections.iter().all(|c| c.id != id));
    }

    #[tokio::test]
    async fn login_events_from_a_replaced_attempt_change_nothing() {
        let target = Target {
            host: "127.0.0.1".into(),
            port: 22,
            user: "fixture".into(),
        };
        let parts = Parts {
            dialer: Arc::new(Stalled),
            authenticator: Arc::new(Stalled),
            host_keys: Arc::new(Stalled),
        };
        let (link, mut actor) = Actor::new(target, parts, Limits::default());
        let _opening = {
            let link = link.clone();
            tokio::spawn(async move { link.open(Open::Session).await })
        };
        let request = actor.commands.recv().await.unwrap();
        actor.command(request);
        actor.pump();
        let replaced = actor.attempt.expect("a login");
        actor.event(Event::Approval(replaced, true));
        actor.command(Command::Retry { fresh: true });
        actor.pump();
        let current = actor.attempt.expect("a fresh login");
        assert_ne!(current, replaced);

        // The replaced login reports before its task ends.
        actor.event(Event::Approval(replaced, true));
        actor.event(Event::LoginFailed(replaced, Error::AuthRefused));
        let aborted = Arc::new(AtomicBool::new(false));
        let abort = {
            let aborted = aborted.clone();
            Abort::new(move || aborted.store(true, Ordering::SeqCst))
        };
        let (handle, _server) = handshake(&actor).await;
        actor.event(Event::LoggedIn(replaced, handle, abort));
        actor.pump();

        assert!(aborted.load(Ordering::SeqCst), "the late connection closes");
        assert_eq!(actor.attempt, Some(current));
        assert_eq!(actor.failures, 0);
        assert!(actor.blocked.is_none());
        assert!(actor.backoff_until.is_none());
        assert!(actor.connections.iter().all(|c| c.ready().is_none()));
        assert_eq!(link.status().borrow().login, Some(Login::InProgress));
    }
}
