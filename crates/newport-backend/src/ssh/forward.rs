//! Port forwards over a server's link. A forward never logs in by itself:
//! its clients, destination checks and remote listeners ask the link, and
//! its state follows the link's status.
use super::{auth, health, relay, reverse};
use crate::model::{
    ConnectionState, DestinationHealth, DestinationStatus, Direction, Status, Tunnel,
};
use newport_ssh::{Error, Fate, Link, Login, Open, Reach, Retry};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{Semaphore, watch},
    task::JoinSet,
};

/// How often a local forward checks that the server reaches its destinations.
const CHECK_INTERVAL: Duration = Duration::from_secs(30);
/// How long a destination check may take once the link is ready.
const CHECK_TIMEOUT: Duration = Duration::from_secs(3);
/// Destination checks in flight at once for one forward.
const CHECK_SLOTS: usize = 4;
/// How often a refused remote forward is asked for again before the forward
/// fails. The server may hold the port for a connection it hasn't noticed is
/// gone.
const DENIED_RETRIES: u32 = 5;
const BACKOFF_MAX: Duration = Duration::from_secs(30);
const LOST: &str = "The connection to the server was lost.";

/// A tunnel's listeners and checks, running until it is shut down or fails.
pub struct Forward {
    state: watch::Receiver<ConnectionState>,
    health: health::Health,
    tasks: JoinSet<()>,
}

impl Forward {
    /// Binds every local port first, so a port in use fails the forward
    /// before anything waits on the link. `asked` when the person connects
    /// it, rather than Newport restoring it.
    pub async fn start(link: Link, tunnel: &Tunnel, asked: bool) -> Result<Self, String> {
        let pairs = tunnel.pairs()?;
        let mut listeners = Vec::new();
        if tunnel.direction == Direction::Local {
            for (local, _) in &pairs {
                let listener = TcpListener::bind(("127.0.0.1", *local))
                    .await
                    .map_err(|error| format!("Cannot bind local port {local}: {error}"))?;
                listeners.push(listener);
            }
        }
        // Connecting a tunnel is asking again; the person connecting it also
        // replaces a login whose prompt may be hidden.
        let retried = matches!(link.status().borrow().reach, Reach::Failed(_));
        if retried && asked {
            link.retry_fresh();
        } else if retried {
            link.retry();
        }
        let health: health::Health = Arc::new(Mutex::new(
            pairs
                .iter()
                .map(|(local, remote)| DestinationHealth {
                    local_port: *local,
                    remote_port: *remote,
                    status: DestinationStatus::Checking,
                    message: None,
                    checked_at: None,
                })
                .collect(),
        ));
        let unbound = match tunnel.direction {
            Direction::Local => 0,
            Direction::Remote => pairs.len(),
        };
        let context = Arc::new(Context {
            link,
            unbound: watch::Sender::new(unbound),
            failure: watch::Sender::new(None),
            fate: Mutex::default(),
            listener_lost: Mutex::default(),
            recheck: watch::Sender::new(()),
        });
        let (state, receiver) = watch::channel(ConnectionState::new(Status::Connecting));
        let mut tasks = JoinSet::new();
        let slots = Arc::new(Semaphore::new(CHECK_SLOTS));
        for (listener, (_, remote)) in listeners.into_iter().zip(&pairs) {
            let host = tunnel.remote_host.clone();
            tasks.spawn(serve(context.clone(), listener, host.clone(), *remote));
            tasks.spawn(check(
                context.clone(),
                host,
                *remote,
                health.clone(),
                slots.clone(),
            ));
        }
        if tunnel.direction == Direction::Remote {
            for (local, remote) in &pairs {
                tasks.spawn(listen(
                    context.clone(),
                    tunnel.remote_host.clone(),
                    *remote,
                    *local,
                ));
                tasks.spawn(health::monitor_local(*local, *remote, health.clone()));
            }
        }
        tasks.spawn(follow(
            context,
            tunnel.auto_reconnect,
            retried,
            health.clone(),
            state,
        ));
        Ok(Self {
            state: receiver,
            health,
            tasks,
        })
    }

    /// The tunnel's state. An error is final: the forward has stopped.
    pub fn state(&self) -> watch::Receiver<ConnectionState> {
        self.state.clone()
    }

    pub fn destination_health(&self) -> Vec<DestinationHealth> {
        self.health.lock().unwrap().clone()
    }

    /// Closes the listeners and every client.
    pub async fn shutdown(mut self) {
        self.tasks.shutdown().await;
    }
}

struct Context {
    link: Link,
    /// Remote forwards not registered on the server right now.
    unbound: watch::Sender<usize>,
    /// A failure of this tunnel alone, which ends it.
    failure: watch::Sender<Option<String>>,
    /// The connection a local forward last used, to say why it was lost.
    fate: Mutex<Option<Fate>>,
    /// Why a remote forward's connection was lost.
    listener_lost: Mutex<Option<Error>>,
    /// Runs destination checks again: after a loss they bring the link back,
    /// and once it is ready they refresh what it reaches.
    recheck: watch::Sender<()>,
}

impl Context {
    fn fail(&self, message: String) {
        self.failure.send_if_modified(|failure| {
            let first = failure.is_none();
            if first {
                *failure = Some(message);
            }
            first
        });
    }
    fn lost(&self) -> Option<Error> {
        let fate = self.fate.lock().unwrap().as_ref().and_then(Fate::error);
        fate.or_else(|| self.listener_lost.lock().unwrap().clone())
    }
}

/// What the tunnel shows: `bound` when every remote forward is registered,
/// `connected` once the tunnel has been connected, and `lost` why its
/// connection closed.
fn tunnel_state(
    status: &newport_ssh::Status,
    bound: bool,
    connected: bool,
    auto_reconnect: bool,
    lost: Option<&Error>,
) -> ConnectionState {
    if let Reach::Failed(error) = &status.reach {
        return ConnectionState::error(wording(error));
    }
    let ready = matches!(status.reach, Reach::Ready { .. });
    if ready && bound {
        return ConnectionState::new(Status::Connected);
    }
    let login_failure = match &status.login {
        Some(Login::Failed(error)) => Some(error),
        _ => None,
    };
    if !auto_reconnect && connected {
        return ConnectionState::error(lost.or(login_failure).map_or_else(|| LOST.into(), wording));
    }
    if !auto_reconnect
        && !ready
        && let Some(error) = login_failure
    {
        return ConnectionState::error(wording(error));
    }
    let (status, reconnect_attempt) = match status.reach {
        Reach::Reconnecting { attempt, .. } => (Status::Reconnecting, attempt),
        _ if connected => (Status::Reconnecting, 0),
        _ => (Status::Connecting, 0),
    };
    ConnectionState {
        status,
        error_message: login_failure.map(wording),
        reconnect_attempt,
    }
}

/// A link error as the person reads it: a failure they resolve in their SSH
/// agent says what to do there.
fn wording(error: &Error) -> String {
    match error {
        Error::ApprovalRequired => auth::AgentApprovalRequired.to_string(),
        Error::AgentUnavailable => auth::SelectedAgentUnavailable.to_string(),
        other => other.to_string(),
    }
}

fn same(a: &ConnectionState, b: &ConnectionState) -> bool {
    a.status == b.status
        && a.error_message == b.error_message
        && a.reconnect_attempt == b.reconnect_attempt
}

/// Publishes the tunnel's state as the link and its listeners change, until
/// it fails.
async fn follow(
    context: Arc<Context>,
    auto_reconnect: bool,
    mut retried: bool,
    health: health::Health,
    state: watch::Sender<ConnectionState>,
) {
    let mut status = context.link.status();
    let mut unbound = context.unbound.subscribe();
    let mut failure = context.failure.subscribe();
    let mut connected = false;
    let mut ready = false;
    loop {
        let current = status.borrow_and_update().clone();
        let bound = *unbound.borrow_and_update() == 0;
        let next = if let Some(message) = failure.borrow_and_update().clone() {
            ConnectionState::error(message)
        } else if retried && matches!(current.reach, Reach::Failed(_)) {
            // The failure the retry clears, until the link takes it.
            ConnectionState::new(Status::Connecting)
        } else {
            tunnel_state(
                &current,
                bound,
                connected,
                auto_reconnect,
                context.lost().as_ref(),
            )
        };
        let now_ready = matches!(current.reach, Reach::Ready { .. });
        if now_ready != ready {
            if !now_ready {
                for destination in health.lock().unwrap().iter_mut() {
                    destination.status = DestinationStatus::Checking;
                    destination.message = None;
                }
            }
            context.recheck.send_replace(());
        }
        ready = now_ready;
        connected |= next.status == Status::Connected;
        let failed = next.status == Status::Error;
        state.send_if_modified(|published| {
            let changed = !same(published, &next);
            *published = next;
            changed
        });
        if failed {
            return;
        }
        tokio::select! {
            changed = status.changed() => {
                if changed.is_err() {
                    return;
                }
                retried = false;
            }
            _ = unbound.changed() => {}
            _ = failure.changed() => {}
        }
    }
}

/// Relays each client of a local port over its own `DirectTcpip` channel.
/// While the link is down a client waits in the link's queue; the relay
/// refuses it when no channel comes in time.
async fn serve(context: Arc<Context>, listener: TcpListener, host: String, port: u16) {
    let open = |_| {
        let context = context.clone();
        let host = host.clone();
        async move {
            let channel = context
                .link
                .open(Open::DirectTcpip { host, port })
                .await
                .ok()?;
            *context.fate.lock().unwrap() = Some(channel.fate());
            Some(channel.into_stream())
        }
    };
    if let Err(error) = relay::serve(listener, open, relay::Policy::TUNNEL).await {
        context.fail(format!("Local listener failed: {error}"));
    }
}

/// Opens and closes a channel to the destination now and then. A check made
/// while the link is down waits for it, which is what brings an idle link up
/// for a local forward.
async fn check(
    context: Arc<Context>,
    host: String,
    port: u16,
    health: health::Health,
    slots: Arc<Semaphore>,
) {
    let mut recheck = context.recheck.subscribe();
    loop {
        recheck.mark_unchanged();
        let Ok(permit) = slots.acquire().await else {
            return;
        };
        let open = context.link.open(Open::DirectTcpip {
            host: host.clone(),
            port,
        });
        let mut status = context.link.status();
        let deadline = async {
            let _ = status
                .wait_for(|status| matches!(status.reach, Reach::Ready { .. }))
                .await;
            tokio::time::sleep(CHECK_TIMEOUT).await;
        };
        let result = tokio::select! {
            result = open => Some(result),
            () = deadline => None,
        };
        let (status, message) = match result {
            Some(Ok(channel)) => {
                *context.fate.lock().unwrap() = Some(channel.fate());
                (DestinationStatus::Reachable, None)
            }
            Some(Err(error)) => {
                let (status, message) = health::destination(Some(&error));
                (status, Some(message))
            }
            None => {
                let (status, message) = health::destination(None);
                (status, Some(message))
            }
        };
        health::record(&health, port, status, message);
        drop(permit);
        // A server that forbids every destination of the forward makes it useless.
        if health
            .lock()
            .unwrap()
            .iter()
            .all(|destination| destination.status == DestinationStatus::Blocked)
        {
            context.fail(health::BLOCKED.into());
            return;
        }
        tokio::select! {
            () = tokio::time::sleep(CHECK_INTERVAL) => {}
            _ = recheck.changed() => {}
        }
    }
}

/// Keeps a remote forward registered: again whenever its connection is lost,
/// once the link is ready, and after a pause when the server refuses it.
async fn listen(context: Arc<Context>, address: String, remote: u16, local: u16) {
    let mut denied = 0;
    let mut failures = 0;
    loop {
        match context.link.listen_tcp(&address, u32::from(remote)).await {
            Ok(mut listener) => {
                denied = 0;
                failures = 0;
                *context.listener_lost.lock().unwrap() = None;
                context.unbound.send_modify(|unbound| *unbound -= 1);
                let error = reverse::serve(&mut listener, local).await;
                *context.listener_lost.lock().unwrap() = Some(error);
                context.unbound.send_modify(|unbound| *unbound += 1);
            }
            Err(Error::RequestDenied) if denied < DENIED_RETRIES => {
                denied += 1;
                tokio::time::sleep(backoff(denied)).await;
            }
            Err(error) if error.retry() == Retry::Backoff && error != Error::RequestDenied => {
                failures += 1;
                tokio::time::sleep(backoff(failures)).await;
            }
            Err(error) => {
                context.fail(format!(
                    "Cannot open port {remote} on the server: {}",
                    wording(&error)
                ));
                return;
            }
        }
    }
}

fn backoff(attempt: u32) -> Duration {
    Duration::from_secs(2u64.pow(attempt.min(5))).min(BACKOFF_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_ssh::Status as LinkStatus;

    fn link(reach: Reach, login: Option<Login>) -> LinkStatus {
        LinkStatus {
            reach,
            login,
            waiting: 0,
        }
    }
    fn ready() -> LinkStatus {
        link(Reach::Ready { connections: 1 }, None)
    }
    fn reconnecting(attempt: u32, login: Option<Login>) -> LinkStatus {
        link(
            Reach::Reconnecting {
                attempt,
                next: None,
            },
            login,
        )
    }

    #[test]
    fn a_ready_link_connects_only_a_bound_tunnel() {
        let state = tunnel_state(&ready(), true, false, true, None);
        assert_eq!(state.status, Status::Connected);
        assert_eq!(state.error_message, None);
        assert_eq!(
            tunnel_state(&ready(), false, false, true, None).status,
            Status::Connecting
        );
        let registering = tunnel_state(&ready(), false, true, true, None);
        assert_eq!(registering.status, Status::Reconnecting);
        assert_eq!(registering.reconnect_attempt, 0);
    }

    #[test]
    fn a_tunnel_follows_the_link_back_with_its_attempt_and_login_failure() {
        for reach in [Reach::Idle, Reach::Connecting] {
            let state = tunnel_state(&link(reach.clone(), None), true, false, true, None);
            assert_eq!(state.status, Status::Connecting);
            let state = tunnel_state(&link(reach, None), true, true, true, None);
            assert_eq!(state.status, Status::Reconnecting);
        }
        let approving = link(Reach::Connecting, Some(Login::WaitingForApproval));
        assert_eq!(
            tunnel_state(&approving, true, true, true, None).status,
            Status::Reconnecting
        );
        let failed = reconnecting(3, Some(Login::Failed(Error::TimedOut)));
        let state = tunnel_state(&failed, true, false, true, None);
        assert_eq!(state.status, Status::Reconnecting);
        assert_eq!(state.reconnect_attempt, 3);
        assert_eq!(
            state.error_message.as_deref(),
            Some("The server didn't answer in time.")
        );
    }

    #[test]
    fn a_failed_link_fails_the_tunnel_with_the_agent_wording() {
        let state = tunnel_state(
            &link(Reach::Failed(Error::ApprovalRequired), None),
            true,
            true,
            true,
            None,
        );
        assert_eq!(state.status, Status::Error);
        assert_eq!(
            state.error_message,
            Some(auth::AgentApprovalRequired.to_string())
        );
        let state = tunnel_state(
            &link(Reach::Failed(Error::AuthRefused), None),
            false,
            false,
            true,
            None,
        );
        assert_eq!(
            state.error_message.as_deref(),
            Some("The server refused the login.")
        );
    }

    #[test]
    fn without_auto_reconnect_a_loss_or_failed_login_stops_the_tunnel() {
        let lost = Error::Lost("reset by peer".into());
        let state = tunnel_state(&reconnecting(0, None), true, true, false, Some(&lost));
        assert_eq!(state.status, Status::Error);
        assert_eq!(state.error_message, Some(lost.to_string()));
        let state = tunnel_state(&link(Reach::Idle, None), true, true, false, None);
        assert_eq!(state.error_message.as_deref(), Some(LOST));
        // A remote forward whose listener was lost, on a link that is ready again.
        assert_eq!(
            tunnel_state(&ready(), false, true, false, Some(&lost)).status,
            Status::Error
        );
        let refused = link(Reach::Connecting, Some(Login::Failed(Error::TimedOut)));
        assert_eq!(
            tunnel_state(&refused, true, false, false, None).status,
            Status::Error
        );
        assert_eq!(
            tunnel_state(&link(Reach::Connecting, None), true, false, false, None).status,
            Status::Connecting
        );
    }

    #[test]
    fn refused_remote_forwards_back_off_to_the_limit() {
        assert_eq!(backoff(1), Duration::from_secs(2));
        assert_eq!(backoff(4), Duration::from_secs(16));
        assert_eq!(backoff(DENIED_RETRIES), BACKOFF_MAX);
        assert_eq!(backoff(u32::MAX), BACKOFF_MAX);
    }
}
