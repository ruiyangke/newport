mod support;

use newport_ssh::{
    BoxFuture, CommandDialer, Dialed, Dialer, Error, HostKeys, JumpDialer, Limits, Link, Login,
    Open, Parts, Reach, Retry, Target,
};
use russh::keys::{self, HashAlg, PublicKeyOrCertificate};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use support::{proxy::*, *};
use tokio::{sync::Notify, sync::watch};

fn through(dialer: Arc<dyn Dialer>, port: u16, host_keys: Arc<dyn HostKeys>) -> Link {
    through_with(dialer, port, host_keys, limits())
}

fn through_with(
    dialer: Arc<dyn Dialer>,
    port: u16,
    host_keys: Arc<dyn HostKeys>,
    limits: Limits,
) -> Link {
    Link::new(
        Target { port, ..target() },
        Parts {
            dialer,
            authenticator: TestAuthenticator::accepting(),
            host_keys,
        },
        limits,
    )
}

#[cfg(unix)]
fn no_approval() -> watch::Sender<bool> {
    watch::channel(false).0
}

/// Counts the dials it passes on.
struct Counted<D> {
    inner: D,
    dials: AtomicUsize,
}

impl<D: Dialer> Dialer for Counted<D> {
    fn dial<'a>(
        &'a self,
        target: &'a Target,
        approval: &'a watch::Sender<bool>,
    ) -> BoxFuture<'a, Result<Dialed, Error>> {
        self.dials.fetch_add(1, Ordering::SeqCst);
        self.inner.dial(target, approval)
    }

    fn deadline(&self, default: Duration) -> Option<Duration> {
        self.inner.deadline(default)
    }

    fn retry(&self) {
        self.inner.retry();
    }

    fn retry_fresh(&self) {
        self.inner.retry_fresh();
    }
}

impl<D> Counted<D> {
    fn new(inner: D) -> Arc<Self> {
        Arc::new(Self {
            inner,
            dials: AtomicUsize::new(0),
        })
    }
}

fn has_nc() -> bool {
    std::process::Command::new("sh")
        .args(["-c", "command -v nc"])
        .output()
        .is_ok_and(|output| output.status.success())
}

/// A file the proxy command writes its process ID to.
#[cfg(unix)]
fn pid_file(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "newport-ssh-proxy-{}-{name}.pid",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

#[cfg(unix)]
async fn pid_in(path: &std::path::Path) -> String {
    eventually("the proxy command to start", || {
        std::fs::read_to_string(path).is_ok_and(|pid| pid.ends_with('\n'))
    })
    .await;
    std::fs::read_to_string(path).unwrap().trim().to_owned()
}

/// Whether the process exists, as a zombie too.
#[cfg(unix)]
fn exists(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

async fn failed_login(link: &Link) -> Error {
    let opening = {
        let link = link.clone();
        tokio::spawn(async move { link.open(Open::Session).await })
    };
    let status = link.status();
    eventually("a failed login", || {
        matches!(status.borrow().login, Some(Login::Failed(_)))
    })
    .await;
    opening.abort();
    let login = status.borrow().login.clone();
    match login {
        Some(Login::Failed(error)) => error,
        other => panic!("no failed login: {other:?}"),
    }
}

/// Records which target each host key was checked for.
#[derive(Default)]
struct Recorded(Mutex<Vec<(u16, String)>>);

impl HostKeys for Recorded {
    fn verify(&self, target: &Target, key: &PublicKeyOrCertificate) -> Result<(), Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            panic!("a plain host key");
        };
        self.0
            .lock()
            .unwrap()
            .push((target.port, key.fingerprint(HashAlg::Sha256).to_string()));
        Ok(())
    }
}

fn fingerprint(host_key: &str) -> String {
    keys::PrivateKey::from_openssh(host_key)
        .unwrap()
        .public_key()
        .fingerprint(HashAlg::Sha256)
        .to_string()
}

#[test]
fn tokens_expand_as_ssh_does() {
    let target = Target {
        host: "10.0.0.5".into(),
        port: 2222,
        user: "deploy".into(),
    };
    let command = "ssh -W %h:%p %r@bastion # %n at 100%%";
    assert_eq!(
        CommandDialer::new(command)
            .original_host("web")
            .command_for(&target)
            .unwrap(),
        "ssh -W 10.0.0.5:2222 deploy@bastion # web at 100%"
    );
    assert_eq!(
        CommandDialer::new(command).command_for(&target).unwrap(),
        "ssh -W 10.0.0.5:2222 deploy@bastion # 10.0.0.5 at 100%"
    );
    assert_eq!(
        CommandDialer::new("nc %%h").command_for(&target).unwrap(),
        "nc %h"
    );
}

#[test]
fn an_unknown_token_is_refused() {
    for command in ["nc %x %p", "nc %h %"] {
        let error = CommandDialer::new(command)
            .command_for(&target())
            .unwrap_err();
        assert!(
            matches!(error, Error::InvalidProxy(_)),
            "{command}: {error:?}"
        );
        assert_eq!(error.retry(), Retry::Never);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_proxy_command_that_cant_be_built_stops_the_link() {
    let dialer = Counted::new(CommandDialer::new("nc %x %p"));
    let link = through(dialer.clone(), 22, Arc::new(AnyHostKey));
    let error = failed_login(&link).await;
    assert_eq!(error, Error::InvalidProxy("unknown token %x".into()));
    let _waiting = {
        let link = link.clone();
        tokio::spawn(async move { link.open(Open::Session).await })
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(link.status().borrow().reach, Reach::Failed(error));
    assert_eq!(dialer.dials.load(Ordering::SeqCst), 1, "no automatic retry");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_through_a_proxy_command_opens_sessions_that_echo() {
    if !has_nc() {
        eprintln!("skipped: nc isn't installed");
        return;
    }
    let server = server(Behaviour::default(), &HOST_A).await;
    let link = through(
        Arc::new(CommandDialer::new("nc 127.0.0.1 %p")),
        server.port,
        Arc::new(AnyHostKey),
    );
    for _ in 0..3 {
        let mut session = link.open(Open::Session).await.unwrap();
        round_trip(&mut session).await;
    }
    let mut forward = link
        .open(Open::DirectTcpip {
            host: "localhost".into(),
            port: 5290,
        })
        .await
        .unwrap();
    round_trip(&mut forward).await;
    assert_eq!(server.seen.connections.load(Ordering::SeqCst), 1);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_proxy_command_that_exits_early_fails_with_its_output() {
    use tokio::io::AsyncReadExt;
    let pid = pid_file("early");
    let command = format!(
        "sh -c 'echo $$ > {}; echo \"bastion: Name or service not known\" >&2; exit 255'",
        pid.display()
    );
    let dialer = CommandDialer::new(command);

    let mut dialed = dialer.dial(&target(), &no_approval()).await.unwrap();
    let mut bytes = [0; 64];
    let error = dialed.stream.read(&mut bytes).await.unwrap_err();
    let error = error
        .get_ref()
        .and_then(|error| error.downcast_ref::<Error>());
    assert_eq!(
        error,
        Some(&Error::ProxyFailed {
            output: "bastion: Name or service not known".into()
        })
    );
    let process = pid_in(&pid).await;
    eventually("the command to be reaped", || !exists(&process)).await;

    let link = through(Arc::new(dialer), 22, Arc::new(AnyHostKey));
    let error = failed_login(&link).await;
    assert_eq!(
        error,
        Error::ProxyFailed {
            output: "bastion: Name or service not known".into()
        }
    );
    assert_eq!(error.retry(), newport_ssh::Retry::Backoff);
    let _ = std::fs::remove_file(pid);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_silent_proxy_command_reports_how_it_ended() {
    use tokio::io::AsyncReadExt;
    let mut dialed = CommandDialer::new("sh -c 'exit 3'")
        .dial(&target(), &no_approval())
        .await
        .unwrap();
    let mut bytes = [0; 64];
    let error = dialed.stream.read(&mut bytes).await.unwrap_err();
    assert!(error.to_string().contains("exit status: 3"), "{error}");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn abort_and_drop_kill_the_proxy_command() {
    let pid = pid_file("abort");
    let dialer = CommandDialer::new(format!(
        "sh -c 'echo $$ > {}; exec sleep 30'",
        pid.display()
    ));
    let dialed = dialer.dial(&target(), &no_approval()).await.unwrap();
    let process = pid_in(&pid).await;
    assert!(exists(&process));
    dialed.abort.abort();
    eventually("abort to kill and reap the command", || !exists(&process)).await;
    drop(dialed);

    let dialed = dialer.dial(&target(), &no_approval()).await.unwrap();
    let process = pid_in(&pid).await;
    let _ = std::fs::remove_file(&pid);
    drop(dialed);
    eventually("drop to kill and reap the command", || !exists(&process)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_through_a_jump_host_opens_sessions_on_the_target() {
    let jump = jump_server(&HOST_A).await;
    let server = server(Behaviour::default(), &HOST_B).await;
    let jump_link = link(
        TestDialer::new(&[jump.port]),
        TestAuthenticator::accepting(),
        limits(),
    );
    let host_keys = Arc::new(Recorded::default());
    let link = through(
        Arc::new(JumpDialer::new(jump_link.clone())),
        server.port,
        host_keys.clone(),
    );
    let mut sessions = Vec::new();
    for _ in 0..3 {
        let mut session = link.open(Open::Session).await.unwrap();
        round_trip(&mut session).await;
        sessions.push(session);
    }
    assert_eq!(server.seen.sessions.load(Ordering::SeqCst), 3);
    assert_eq!(jump.seen.sessions.load(Ordering::SeqCst), 0);
    assert_eq!(jump.seen.open.load(Ordering::SeqCst), 1);
    assert_eq!(
        jump_link.status().borrow().reach,
        Reach::Ready { connections: 1 }
    );
    assert_eq!(
        *host_keys.0.lock().unwrap(),
        [(server.port, fingerprint(&HOST_B))]
    );

    drop(sessions);
    drop(link);
    eventually("the forwarding channel to close", || {
        jump.seen.open.load(Ordering::SeqCst) == 0
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_jump_forward_is_an_unreachable_target() {
    let jump = jump_server(&HOST_A).await;
    let jump_link = link(
        TestDialer::new(&[jump.port]),
        TestAuthenticator::accepting(),
        limits(),
    );
    let link = through(
        Arc::new(JumpDialer::new(jump_link)),
        REFUSED,
        Arc::new(AnyHostKey),
    );
    let error = failed_login(&link).await;
    let Error::Unreachable(reason) = &error else {
        panic!("not unreachable: {error:?}");
    };
    assert!(
        reason.contains(&format!(
            "the jump host couldn't connect to 127.0.0.1:{REFUSED}"
        )),
        "{reason}"
    );
    assert!(jump.seen.refused.load(Ordering::SeqCst) >= 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_two_hop_chain_reaches_the_target() {
    let first = jump_server(&HOST_A).await;
    let second = jump_server(&HOST_B).await;
    let server = server(Behaviour::default(), &HOST_A).await;
    let first_link = link(
        TestDialer::new(&[first.port]),
        TestAuthenticator::accepting(),
        limits(),
    );
    let second_link = through(
        Arc::new(JumpDialer::new(first_link)),
        second.port,
        Arc::new(AnyHostKey),
    );
    let link = through(
        Arc::new(JumpDialer::new(second_link)),
        server.port,
        Arc::new(AnyHostKey),
    );
    let mut session = link.open(Open::Session).await.unwrap();
    round_trip(&mut session).await;
    assert_eq!(first.seen.open.load(Ordering::SeqCst), 1);
    assert_eq!(second.seen.open.load(Ordering::SeqCst), 1);
    assert_eq!(server.seen.sessions.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_busy_jump_host_is_retried_with_backoff() {
    let jump = jump_server(&HOST_A).await;
    let jump_link = link(
        TestDialer::new(&[jump.port]),
        TestAuthenticator::accepting(),
        Limits {
            max_waiting: 0,
            ..limits()
        },
    );
    let link = through(
        Arc::new(JumpDialer::new(jump_link)),
        22,
        Arc::new(AnyHostKey),
    );
    let error = failed_login(&link).await;
    assert!(matches!(error, Error::Unreachable(_)), "{error:?}");
    assert_eq!(error.retry(), Retry::Backoff);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_jump_host_that_cant_be_reached_fails_the_dial_and_says_so() {
    let jump_link = link(
        TestDialer::new(&[REFUSED]),
        TestAuthenticator::accepting(),
        limits(),
    );
    let link = through(
        Arc::new(JumpDialer::new(jump_link)),
        22,
        Arc::new(AnyHostKey),
    );
    let error = failed_login(&link).await;
    assert!(
        matches!(&error, Error::Unreachable(cause) if cause.contains("jump host")),
        "{error:?}"
    );
    assert_eq!(error.retry(), Retry::Backoff);
}

/// A link to `server` through a jump host whose logins follow `steps`, with
/// a dial limit far shorter than the jump host's approval wait.
async fn through_an_approving_jump_host(
    steps: Vec<Step>,
) -> (Link, Arc<TestAuthenticator>, support::TestServer) {
    let jump = jump_server(&HOST_A).await;
    let server = server(Behaviour::default(), &HOST_B).await;
    let authenticator = TestAuthenticator::new(steps);
    let jump_link = link(
        TestDialer::new(&[jump.port]),
        authenticator.clone(),
        limits(),
    );
    let link = through_with(
        Arc::new(JumpDialer::new(jump_link)),
        server.port,
        Arc::new(AnyHostKey),
        Limits {
            dial: Duration::from_millis(200),
            ..limits()
        },
    );
    (link, authenticator, server)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_jump_host_waiting_for_approval_holds_the_dial_and_shows_the_wait() {
    let approve = Arc::new(Notify::new());
    let (link, _, _server) =
        through_an_approving_jump_host(vec![Step::Approve(approve.clone())]).await;
    let opening = {
        let link = link.clone();
        tokio::spawn(async move { link.open(Open::Session).await })
    };
    let status = link.status();
    eventually("the jump host's approval wait", || {
        status.borrow().login == Some(Login::WaitingForApproval)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(status.borrow().login, Some(Login::WaitingForApproval));
    approve.notify_one();
    let mut session = tokio::time::timeout(Duration::from_secs(5), opening)
        .await
        .expect("the approved jump host carries the dial")
        .unwrap()
        .unwrap();
    round_trip(&mut session).await;
    assert_eq!(status.borrow().login, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_jump_host_slower_to_log_in_than_the_dial_limit_still_carries_the_dial() {
    let jump = jump_server(&HOST_A).await;
    let server = server(Behaviour::default(), &HOST_B).await;
    // The jump host's first dial fails, and its backoff outlasts the dial
    // limit below.
    let jump_link = link(
        TestDialer::new(&[REFUSED, jump.port]),
        TestAuthenticator::accepting(),
        Limits {
            backoff_min: Duration::from_millis(600),
            ..limits()
        },
    );
    let dialer = Counted::new(JumpDialer::new(jump_link));
    let link = through_with(
        dialer.clone(),
        server.port,
        Arc::new(AnyHostKey),
        Limits {
            dial: Duration::from_millis(100),
            ..limits()
        },
    );
    let mut session = tokio::time::timeout(Duration::from_secs(5), link.open(Open::Session))
        .await
        .expect("the jump host's own deadlines apply")
        .unwrap();
    round_trip(&mut session).await;
    // The jump host's failed login fails the first dial; the second waits out
    // its backoff, far past this link's dial limit.
    assert_eq!(dialer.dials.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn retry_reaches_the_jump_host() {
    let never_approved = Step::Approve(Arc::new(Notify::new()));
    let (link, authenticator, _server) =
        through_an_approving_jump_host(vec![never_approved, Step::Accept]).await;
    let opening = {
        let link = link.clone();
        tokio::spawn(async move { link.open(Open::Session).await })
    };
    let status = link.status();
    eventually("the jump host's approval wait", || {
        status.borrow().login == Some(Login::WaitingForApproval)
    })
    .await;
    link.retry_fresh();
    let mut session = tokio::time::timeout(Duration::from_secs(5), opening)
        .await
        .expect("the jump host logs in afresh")
        .unwrap()
        .unwrap();
    round_trip(&mut session).await;
    assert!(authenticator.attempts.load(Ordering::SeqCst) >= 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_jump_host_that_stopped_retrying_fails_the_dial_until_retry() {
    let (link, authenticator, _server) =
        through_an_approving_jump_host(vec![Step::Decline, Step::Accept]).await;
    let error = failed_login(&link).await;
    assert_eq!(error, Error::ApprovalRequired);
    let opening = {
        let link = link.clone();
        tokio::spawn(async move { link.open(Open::Session).await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(authenticator.attempts.load(Ordering::SeqCst), 1);
    link.retry();
    let mut session = tokio::time::timeout(Duration::from_secs(5), opening)
        .await
        .expect("Retry logs in to the jump host again")
        .unwrap()
        .unwrap();
    round_trip(&mut session).await;
}
