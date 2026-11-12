//! An in-process SSH server with OpenSSH's session limit and remote forwards,
//! a dialer that can sever its connections, and an authenticator the test
//! scripts.
#![allow(dead_code)]
pub mod proxy;

use newport_ssh::{
    Abort, Authenticator, BoxFuture, Dialed, Dialer, Error, Handler, HostKeys, Limits, Link, Parts,
    Target,
};
use russh::{
    Channel, ChannelId, ChannelOpenFailure, keys,
    server::{self, Auth, ChannelOpenHandle, Msg, Server as _, Session},
};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::{Notify, watch},
    task::JoinHandle,
};

pub static HOST_A: LazyLock<String> = LazyLock::new(|| key(1));
pub static HOST_B: LazyLock<String> = LazyLock::new(|| key(2));
pub static CLIENT: LazyLock<String> = LazyLock::new(|| key(3));

/// A test key in OpenSSH form, made from a fixed seed so the repository keeps
/// no key files.
fn key(seed: u8) -> String {
    let pair = keys::ssh_key::private::Ed25519Keypair::from_seed(&[seed; 32]);
    keys::PrivateKey::from(pair)
        .to_openssh(keys::ssh_key::LineEnding::LF)
        .unwrap()
        .to_string()
}

/// How the server behaves.
#[derive(Clone)]
pub struct Behaviour {
    /// Sessions each connection may hold, as OpenSSH's `MaxSessions`.
    pub max_sessions: usize,
    /// The connection, by arrival order from 0, that refuses every session.
    pub refuse_sessions_on: Option<usize>,
    /// How long the server waits before confirming a session.
    pub confirm_after: Duration,
    /// Refuse every `tcpip-forward`.
    pub refuse_forwards: bool,
    /// Open a `forwarded-tcpip` channel to a port as soon as a forward asking
    /// for port 0 binds it, right behind the reply.
    pub connect_on_bind: bool,
}

impl Default for Behaviour {
    fn default() -> Self {
        Self {
            max_sessions: 10,
            refuse_sessions_on: None,
            confirm_after: Duration::ZERO,
            refuse_forwards: false,
            connect_on_bind: false,
        }
    }
}

/// What the server saw.
#[derive(Default)]
pub struct Seen {
    pub connections: AtomicUsize,
    /// Sessions open on the server right now, across connections.
    pub sessions: AtomicUsize,
    pub refusals: AtomicUsize,
    /// Refuse every session from now on.
    pub refusing: AtomicBool,
    /// What `connect_on_bind` opened: the channel, or why the client refused it.
    pub on_bind: Mutex<Vec<Result<Channel<Msg>, russh::Error>>>,
    /// Remote forwards held right now, by bound port, with the connection
    /// that holds each.
    pub forwards: Mutex<HashMap<u32, server::Handle>>,
    /// The next port a forward asking for port 0 gets.
    next_port: AtomicU32,
}

impl Seen {
    pub fn forwarded(&self, port: u32) -> bool {
        self.forwards.lock().unwrap().contains_key(&port)
    }
}

pub struct TestServer {
    pub port: u16,
    pub seen: Arc<Seen>,
}

impl TestServer {
    /// A connection arriving at a forwarded port: the server opens a
    /// `forwarded-tcpip` channel to the client that holds it, which resolves
    /// once the client answers.
    pub fn connect(&self, port: u32) -> JoinHandle<Result<Channel<Msg>, russh::Error>> {
        let handle = self.seen.forwards.lock().unwrap().get(&port).cloned();
        tokio::spawn(async move {
            handle
                .ok_or(russh::Error::RequestDenied)?
                .channel_open_forwarded_tcpip("127.0.0.1", port, "127.0.0.1", 50000)
                .await
        })
    }
}

pub async fn server(behaviour: Behaviour, host_key: &str) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Seen {
        next_port: AtomicU32::new(40000),
        ..Seen::default()
    });
    let config = Arc::new(server::Config {
        keys: vec![keys::PrivateKey::from_openssh(host_key).unwrap()],
        auth_rejection_time: Duration::from_millis(1),
        ..Default::default()
    });
    let mut accepting = Accepting {
        behaviour,
        seen: seen.clone(),
    };
    tokio::spawn(async move {
        let _ = accepting.run_on_socket(config, &listener).await;
    });
    TestServer { port, seen }
}

struct Accepting {
    behaviour: Behaviour,
    seen: Arc<Seen>,
}

impl server::Server for Accepting {
    type Handler = Connection;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Connection {
        let index = self.seen.connections.fetch_add(1, Ordering::SeqCst);
        Connection {
            index,
            behaviour: self.behaviour.clone(),
            seen: self.seen.clone(),
            sessions: Arc::default(),
            forwards: Arc::default(),
        }
    }
}

pub struct Connection {
    index: usize,
    behaviour: Behaviour,
    seen: Arc<Seen>,
    sessions: Arc<Mutex<HashSet<ChannelId>>>,
    forwards: Arc<Mutex<HashSet<u32>>>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        let open = self.sessions.lock().unwrap().len();
        self.seen.sessions.fetch_sub(open, Ordering::SeqCst);
        let mut forwards = self.seen.forwards.lock().unwrap();
        for port in self.forwards.lock().unwrap().drain() {
            forwards.remove(&port);
        }
    }
}

fn echo(channel: Channel<Msg>) {
    tokio::spawn(async move {
        let mut stream = channel.into_stream();
        let mut bytes = [0; 65536];
        while let Ok(n) = stream.read(&mut bytes).await {
            if n == 0 || stream.write_all(&bytes[..n]).await.is_err() {
                break;
            }
        }
    });
}

impl server::Handler for Connection {
    type Error = russh::Error;

    async fn auth_publickey_offered(
        &mut self,
        _: &str,
        _: server::PublicKeyOffer<'_>,
    ) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn auth_publickey(
        &mut self,
        _: &str,
        _: server::VerifiedPublicKey<'_>,
    ) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        let refused = self.behaviour.refuse_sessions_on == Some(self.index)
            || self.seen.refusing.load(Ordering::SeqCst)
            || self.sessions.lock().unwrap().len() >= self.behaviour.max_sessions;
        if refused {
            self.seen.refusals.fetch_add(1, Ordering::SeqCst);
            reply.reject(ChannelOpenFailure::ConnectFailed).await;
            return Ok(());
        }
        // Counted from the open, as OpenSSH counts a session from its open.
        self.sessions.lock().unwrap().insert(channel.id());
        self.seen.sessions.fetch_add(1, Ordering::SeqCst);
        let delay = self.behaviour.confirm_after;
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            reply.accept().await;
            echo(channel);
        });
        Ok(())
    }

    async fn channel_close(&mut self, id: ChannelId, _: &mut Session) -> Result<(), Self::Error> {
        if self.sessions.lock().unwrap().remove(&id) {
            self.seen.sessions.fetch_sub(1, Ordering::SeqCst);
        }
        Ok(())
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        _: &str,
        _: u32,
        _: &str,
        _: u32,
        reply: ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        echo(channel);
        Ok(())
    }

    async fn tcpip_forward(
        &mut self,
        _: &str,
        port: &mut u32,
        session: &mut Session,
    ) -> Result<bool, Self::Error> {
        if self.behaviour.refuse_forwards {
            return Ok(false);
        }
        if *port == 0 {
            *port = self.seen.next_port.fetch_add(1, Ordering::SeqCst);
            if self.behaviour.connect_on_bind {
                // Sent after this handler returns, so after the reply.
                let (handle, seen, bound) = (session.handle(), self.seen.clone(), *port);
                tokio::spawn(async move {
                    let opened = handle
                        .channel_open_forwarded_tcpip("127.0.0.1", bound, "127.0.0.1", 50000)
                        .await;
                    seen.on_bind.lock().unwrap().push(opened);
                });
            }
        }
        let mut forwards = self.seen.forwards.lock().unwrap();
        // As OpenSSH, a port another forward holds can't be bound again.
        if forwards.contains_key(port) {
            return Ok(false);
        }
        forwards.insert(*port, session.handle());
        self.forwards.lock().unwrap().insert(*port);
        Ok(true)
    }

    async fn cancel_tcpip_forward(
        &mut self,
        _: &str,
        port: u32,
        _: &mut Session,
    ) -> Result<bool, Self::Error> {
        if !self.forwards.lock().unwrap().remove(&port) {
            return Ok(false);
        }
        self.seen.forwards.lock().unwrap().remove(&port);
        Ok(true)
    }
}

/// A port no connection reaches. Every platform refuses it at once, where a
/// closed port takes Windows about two seconds to refuse.
pub const REFUSED: u16 = 0;

/// Dials test servers in turn (the last one repeats) and can sever every
/// connection it made, as a network failure would.
#[derive(Default)]
pub struct TestDialer {
    pub ports: Mutex<Vec<u16>>,
    pub dials: AtomicUsize,
    aborts: Mutex<Vec<Abort>>,
}

impl TestDialer {
    pub fn new(ports: &[u16]) -> Arc<Self> {
        Arc::new(Self {
            ports: Mutex::new(ports.to_vec()),
            ..Default::default()
        })
    }
    pub fn sever(&self) {
        for abort in self.aborts.lock().unwrap().drain(..) {
            abort.abort();
        }
    }
}

impl Dialer for TestDialer {
    fn dial<'a>(
        &'a self,
        target: &'a Target,
        approval: &'a watch::Sender<bool>,
    ) -> BoxFuture<'a, Result<Dialed, Error>> {
        Box::pin(async move {
            let index = self.dials.fetch_add(1, Ordering::SeqCst);
            let port = {
                let ports = self.ports.lock().unwrap();
                ports[index.min(ports.len() - 1)]
            };
            let target = Target {
                port,
                ..target.clone()
            };
            let dialed = newport_ssh::TcpDialer.dial(&target, approval).await?;
            self.aborts.lock().unwrap().push(dialed.abort.clone());
            Ok(dialed)
        })
    }
}

/// What a login does, in order; the last entry repeats.
#[derive(Clone)]
pub enum Step {
    Accept,
    /// Waits for approval until `approve` is notified.
    Approve(Arc<Notify>),
    Decline,
    /// Never answers, as a server that stopped responding mid-login.
    Stall,
    /// Waits for approval until notified, then never answers.
    ApproveThenStall(Arc<Notify>),
}

pub struct TestAuthenticator {
    steps: Mutex<Vec<Step>>,
    pub attempts: AtomicUsize,
    running: AtomicUsize,
    /// The most logins that ever ran at once.
    pub most_at_once: AtomicUsize,
}

impl TestAuthenticator {
    pub fn new(steps: Vec<Step>) -> Arc<Self> {
        Arc::new(Self {
            steps: Mutex::new(steps),
            attempts: AtomicUsize::new(0),
            running: AtomicUsize::new(0),
            most_at_once: AtomicUsize::new(0),
        })
    }
    pub fn accepting() -> Arc<Self> {
        Self::new(vec![Step::Accept])
    }
}

impl Authenticator for TestAuthenticator {
    fn authenticate<'a>(
        &'a self,
        handle: &'a mut russh::client::Handle<Handler>,
        target: &'a Target,
        approval: &'a watch::Sender<bool>,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
            let running = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.most_at_once.fetch_max(running, Ordering::SeqCst);
            let step = {
                let steps = self.steps.lock().unwrap();
                steps[attempt.min(steps.len() - 1)].clone()
            };
            // Long enough for overlapping logins to show.
            tokio::time::sleep(Duration::from_millis(30)).await;
            let result = match step {
                Step::Accept => Ok(()),
                Step::Approve(approved) => {
                    let _ = approval.send(true);
                    approved.notified().await;
                    let _ = approval.send(false);
                    Ok(())
                }
                Step::Decline => Err(Error::ApprovalRequired),
                Step::Stall => std::future::pending().await,
                Step::ApproveThenStall(approved) => {
                    let _ = approval.send(true);
                    approved.notified().await;
                    let _ = approval.send(false);
                    std::future::pending().await
                }
            };
            let result = match result {
                Ok(()) => {
                    let key = keys::PrivateKey::from_openssh(CLIENT.as_str()).unwrap();
                    let accepted = handle
                        .authenticate_publickey(
                            target.user.clone(),
                            keys::PrivateKeyWithHashAlg::new(Arc::new(key), None),
                        )
                        .await
                        .map_err(|error| Error::Unreachable(error.to_string()))?;
                    if accepted.success() {
                        Ok(())
                    } else {
                        Err(Error::AuthRefused)
                    }
                }
                Err(error) => Err(error),
            };
            self.running.fetch_sub(1, Ordering::SeqCst);
            result
        })
    }
}

pub fn target() -> Target {
    Target {
        host: "127.0.0.1".into(),
        port: 0,
        user: "fixture".into(),
    }
}

/// Short deadlines, so failure tests finish quickly.
pub fn limits() -> Limits {
    Limits {
        open: Duration::from_secs(2),
        idle: Duration::from_secs(60),
        probe: Duration::from_millis(500),
        backoff_min: Duration::from_millis(100),
        backoff_max: Duration::from_millis(400),
        ..Limits::default()
    }
}

/// Accepts every host key; the link's own pin still applies.
pub struct AnyHostKey;

impl HostKeys for AnyHostKey {
    fn verify(&self, _: &Target, _: &keys::PublicKeyOrCertificate) -> Result<(), Error> {
        Ok(())
    }
}

pub fn link(
    dialer: Arc<TestDialer>,
    authenticator: Arc<TestAuthenticator>,
    limits: Limits,
) -> Link {
    link_with(dialer, authenticator, Arc::new(AnyHostKey), limits)
}

pub fn link_with(
    dialer: Arc<TestDialer>,
    authenticator: Arc<TestAuthenticator>,
    host_keys: Arc<dyn HostKeys>,
    limits: Limits,
) -> Link {
    Link::new(
        target(),
        Parts {
            dialer,
            authenticator,
            host_keys,
        },
        limits,
    )
}

/// Wait until `condition` holds, or fail after 5 s.
pub async fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Write a byte and read it back.
pub async fn round_trip(lease: &mut newport_ssh::Channel) {
    let channel = &mut **lease;
    channel.data(&b"x"[..]).await.unwrap();
    loop {
        match tokio::time::timeout(Duration::from_secs(3), channel.wait()).await {
            Ok(Some(russh::ChannelMsg::Data { data })) if !data.is_empty() => return,
            Ok(Some(_)) => {}
            other => panic!("no echo: {other:?}"),
        }
    }
}
