//! Against a real OpenSSH server, run with `--ignored`. Set
//! NEWPORT_TRANSPORT_OPENSSH_PORT and NEWPORT_TRANSPORT_OPENSSH_KEY (a private
//! key the server's `fixture` user accepts), for example for the
//! `tests/remote` container.
mod support;

use newport_ssh::{
    Authenticator, BoxFuture, Channel, Error, Handler, Limits, Link, Open, Parts, Reach, Target,
    TcpDialer,
};
use russh::{ChannelMsg, keys};
use std::{sync::Arc, time::Duration};
use tokio::sync::watch;

struct KeyFile(keys::PrivateKey);

impl Authenticator for KeyFile {
    fn authenticate<'a>(
        &'a self,
        handle: &'a mut russh::client::Handle<Handler>,
        target: &'a Target,
        _: &'a watch::Sender<bool>,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let accepted = handle
                .authenticate_publickey(
                    target.user.clone(),
                    keys::PrivateKeyWithHashAlg::new(Arc::new(self.0.clone()), None),
                )
                .await
                .map_err(|error| Error::Unreachable(error.to_string()))?;
            if accepted.success() {
                Ok(())
            } else {
                Err(Error::AuthRefused)
            }
        })
    }
}

fn openssh() -> Link {
    let port = std::env::var("NEWPORT_TRANSPORT_OPENSSH_PORT")
        .expect("NEWPORT_TRANSPORT_OPENSSH_PORT")
        .parse()
        .unwrap();
    let key =
        std::env::var("NEWPORT_TRANSPORT_OPENSSH_KEY").expect("NEWPORT_TRANSPORT_OPENSSH_KEY");
    let target = Target {
        host: "127.0.0.1".into(),
        port,
        user: "fixture".into(),
    };
    let key = keys::load_secret_key(key, None).unwrap();
    Link::new(
        target,
        Parts {
            dialer: Arc::new(TcpDialer),
            authenticator: Arc::new(KeyFile(key)),
            host_keys: Arc::new(support::AnyHostKey),
        },
        Limits::default(),
    )
}

async fn command(link: &Link, command: &str) -> Channel {
    let lease = link.open(Open::Session).await.expect("a session");
    lease.exec(true, command).await.unwrap();
    lease
}

async fn echo(lease: &mut Channel) -> Duration {
    let started = std::time::Instant::now();
    support::round_trip(lease).await;
    started.elapsed()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs an OpenSSH server"]
async fn eleven_sessions_need_a_second_connection() {
    let link = openssh();
    let mut leases = Vec::new();
    for _ in 0..11 {
        leases.push(command(&link, "cat").await);
    }
    for lease in &mut leases {
        echo(lease).await;
    }
    let mut connections: Vec<_> = leases.iter().map(Channel::connection).collect();
    connections.dedup();
    assert_eq!(
        connections.len(),
        2,
        "MaxSessions 10 learned from OpenSSH's refusal"
    );
    assert_eq!(
        link.status().borrow().reach,
        Reach::Ready { connections: 2 }
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs an OpenSSH server"]
async fn an_unread_flood_leaves_the_other_sessions_working() {
    let link = openssh();
    let mut flood = command(&link, "cat /dev/zero").await;
    while !matches!(flood.wait().await, Some(ChannelMsg::Data { .. })) {}
    // The flood is no longer read; its window closes, nothing else does.
    let mut echoing = command(&link, "cat").await;
    assert_eq!(echoing.connection(), flood.connection());
    tokio::time::sleep(Duration::from_secs(2)).await;
    for _ in 0..5 {
        assert!(echo(&mut echoing).await < Duration::from_millis(500));
    }
    let mut another = command(&link, "cat").await;
    echo(&mut another).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs an OpenSSH server"]
async fn closed_sessions_free_their_slots() {
    let mut used = Vec::new();
    let link = openssh();
    for round in 0..3 {
        let mut leases = Vec::new();
        for _ in 0..10 {
            leases.push(command(&link, "cat").await);
        }
        for lease in &mut leases {
            echo(lease).await;
        }
        used.extend(leases.iter().map(Channel::connection));
        drop(leases);
        // OpenSSH confirms each close within milliseconds.
        tokio::time::sleep(Duration::from_millis(300)).await;
        println!("round {round}: {:?}", link.status().borrow().reach);
    }
    used.dedup();
    assert_eq!(
        used.len(),
        1,
        "every round fits the slots the last one freed"
    );
}
