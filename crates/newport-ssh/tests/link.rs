mod support;

use newport_ssh::{Channel, Error, Link, Login, Open, Reach};
use std::{sync::atomic::Ordering, time::Duration};
use support::*;
use tokio::sync::Notify;

async fn sessions(link: &Link, count: usize) -> Vec<Channel> {
    let opening: Vec<_> = (0..count)
        .map(|_| {
            let link = link.clone();
            tokio::spawn(async move { link.open(Open::Session).await })
        })
        .collect();
    let mut leases = Vec::new();
    for task in opening {
        leases.push(task.await.unwrap().expect("a session"));
    }
    leases
}

fn connections(leases: &[Channel]) -> usize {
    let mut ids: Vec<_> = leases.iter().map(Channel::connection).collect();
    ids.sort();
    ids.dedup();
    ids.len()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_unless_failing_fails_with_the_login_it_waited_for_only() {
    let server = server(Behaviour::default(), &HOST_A).await;
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    // The first login fails; the next reaches the server.
    let link = link(
        TestDialer::new(&[closed, server.port]),
        TestAuthenticator::accepting(),
        limits(),
    );
    let failed = tokio::time::timeout(
        Duration::from_secs(5),
        link.open_unless_failing(Open::Session),
    )
    .await
    .expect("the failed login answers the request");
    assert!(matches!(failed.err(), Some(Error::Unreachable(_))));
    // The earlier failure is still the link's last login, but doesn't count
    // against a request made after it.
    let mut session = tokio::time::timeout(
        Duration::from_secs(5),
        link.open_unless_failing(Open::Session),
    )
    .await
    .expect("the next login serves the request")
    .unwrap();
    round_trip(&mut session).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sessions_share_one_connection() {
    let server = server(Behaviour::default(), &HOST_A).await;
    let authenticator = TestAuthenticator::accepting();
    let link = link(
        TestDialer::new(&[server.port]),
        authenticator.clone(),
        limits(),
    );
    let mut leases = sessions(&link, 3).await;
    for lease in &mut leases {
        round_trip(lease).await;
    }
    assert_eq!(connections(&leases), 1);
    assert_eq!(server.seen.connections.load(Ordering::SeqCst), 1);
    assert_eq!(authenticator.attempts.load(Ordering::SeqCst), 1);
    assert_eq!(
        link.status().borrow().reach,
        Reach::Ready { connections: 1 }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn thirty_two_sessions_on_a_ten_session_server_log_in_four_times_one_at_a_time() {
    let server = server(Behaviour::default(), &HOST_A).await;
    let authenticator = TestAuthenticator::accepting();
    let link = link(
        TestDialer::new(&[server.port]),
        authenticator.clone(),
        limits(),
    );
    let leases = sessions(&link, 32).await;
    assert_eq!(connections(&leases), 4);
    assert_eq!(authenticator.attempts.load(Ordering::SeqCst), 4);
    assert_eq!(authenticator.most_at_once.load(Ordering::SeqCst), 1);
    assert_eq!(server.seen.sessions.load(Ordering::SeqCst), 32);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_learned_limit_moves_sessions_to_the_next_connection() {
    let server = server(
        Behaviour {
            max_sessions: 2,
            ..Behaviour::default()
        },
        &HOST_A,
    )
    .await;
    let authenticator = TestAuthenticator::accepting();
    let link = link(
        TestDialer::new(&[server.port]),
        authenticator.clone(),
        limits(),
    );
    let leases = sessions(&link, 7).await;
    assert_eq!(connections(&leases), 4);
    assert_eq!(authenticator.most_at_once.load(Ordering::SeqCst), 1);
    assert_eq!(server.seen.sessions.load(Ordering::SeqCst), 7);
}

#[tokio::test(flavor = "multi_thread")]
async fn at_the_connection_limit_a_request_waits_for_a_free_session() {
    let server = server(
        Behaviour {
            max_sessions: 1,
            ..Behaviour::default()
        },
        &HOST_A,
    )
    .await;
    let limits = newport_ssh::Limits {
        max_connections: 2,
        ..limits()
    };
    let link = link(
        TestDialer::new(&[server.port]),
        TestAuthenticator::accepting(),
        limits,
    );
    let mut leases = sessions(&link, 2).await;
    let waiting = {
        let link = link.clone();
        tokio::spawn(async move { link.open(Open::Session).await })
    };
    let status = link.status();
    eventually("a waiting request", || status.borrow().waiting == 1).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!waiting.is_finished());
    drop(leases.pop());
    let mut lease = tokio::time::timeout(Duration::from_secs(5), waiting)
        .await
        .expect("the waiting request gets the freed session")
        .unwrap()
        .unwrap();
    round_trip(&mut lease).await;
    assert_eq!(server.seen.connections.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_open_leaves_no_session_on_the_server() {
    let server = server(
        Behaviour {
            confirm_after: Duration::from_millis(500),
            ..Behaviour::default()
        },
        &HOST_A,
    )
    .await;
    let link = link(
        TestDialer::new(&[server.port]),
        TestAuthenticator::accepting(),
        limits(),
    );
    // Log in first, so the cancellation lands while the server confirms.
    drop(
        link.open(Open::DirectTcpip {
            host: "x".into(),
            port: 1,
        })
        .await
        .unwrap(),
    );
    let cancelled =
        tokio::time::timeout(Duration::from_millis(100), link.open(Open::Session)).await;
    assert!(cancelled.is_err());
    eventually("the server to count the session", || {
        server.seen.sessions.load(Ordering::SeqCst) == 1
    })
    .await;
    eventually("the late session to be closed", || {
        server.seen.sessions.load(Ordering::SeqCst) == 0
    })
    .await;
    assert_eq!(link.status().borrow().waiting, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_without_sessions_still_forwards() {
    let server = server(
        Behaviour {
            max_sessions: 0,
            ..Behaviour::default()
        },
        &HOST_A,
    )
    .await;
    let link = link(
        TestDialer::new(&[server.port]),
        TestAuthenticator::accepting(),
        limits(),
    );
    assert_eq!(
        link.open(Open::Session).await.err(),
        Some(Error::SessionsDisabled)
    );
    let mut forward = link
        .open(Open::DirectTcpip {
            host: "localhost".into(),
            port: 5290,
        })
        .await
        .unwrap();
    round_trip(&mut forward).await;
    assert_eq!(
        link.open(Open::Session).await.err(),
        Some(Error::SessionsDisabled)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_connection_that_refuses_sessions_is_not_retried() {
    let server = server(
        Behaviour {
            max_sessions: 1,
            refuse_sessions_on: Some(1),
            ..Behaviour::default()
        },
        &HOST_A,
    )
    .await;
    let link = link(
        TestDialer::new(&[server.port]),
        TestAuthenticator::accepting(),
        limits(),
    );
    let first = link.open(Open::Session).await.unwrap();
    let second = {
        let link = link.clone();
        tokio::spawn(async move { link.open(Open::Session).await })
    };
    eventually("the second connection's refusal", || {
        server.seen.refusals.load(Ordering::SeqCst) >= 2
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(server.seen.connections.load(Ordering::SeqCst), 2);
    assert_eq!(link.status().borrow().waiting, 1);
    drop(first);
    let mut lease = tokio::time::timeout(Duration::from_secs(5), second)
        .await
        .expect("the request takes the freed session")
        .unwrap()
        .unwrap();
    round_trip(&mut lease).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_login_waiting_for_approval_shows_in_the_status() {
    let server = server(Behaviour::default(), &HOST_A).await;
    let approve = std::sync::Arc::new(Notify::new());
    let authenticator = TestAuthenticator::new(vec![Step::Approve(approve.clone())]);
    let link = link(TestDialer::new(&[server.port]), authenticator, limits());
    let opening = {
        let link = link.clone();
        tokio::spawn(async move { link.open(Open::Session).await })
    };
    let status = link.status();
    eventually("the approval wait", || {
        status.borrow().login == Some(Login::WaitingForApproval)
    })
    .await;
    assert_eq!(status.borrow().reach, Reach::Connecting);
    // The login deadline (20 s of network time) doesn't run while approving.
    approve.notify_one();
    opening.await.unwrap().unwrap();
    assert_eq!(status.borrow().reach, Reach::Ready { connections: 1 });
    assert_eq!(status.borrow().login, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_declined_second_login_leaves_the_server_ready() {
    let server = server(
        Behaviour {
            max_sessions: 1,
            ..Behaviour::default()
        },
        &HOST_A,
    )
    .await;
    let authenticator = TestAuthenticator::new(vec![Step::Accept, Step::Decline, Step::Accept]);
    let link = link(
        TestDialer::new(&[server.port]),
        authenticator.clone(),
        limits(),
    );
    let mut first = link.open(Open::Session).await.unwrap();
    let second = {
        let link = link.clone();
        tokio::spawn(async move { link.open(Open::Session).await })
    };
    let status = link.status();
    eventually("the declined login", || {
        status.borrow().login == Some(Login::Failed(Error::ApprovalRequired))
    })
    .await;
    assert_eq!(status.borrow().reach, Reach::Ready { connections: 1 });
    assert_eq!(status.borrow().waiting, 1);
    round_trip(&mut first).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        authenticator.attempts.load(Ordering::SeqCst),
        2,
        "no automatic retry"
    );
    link.retry();
    let mut lease = tokio::time::timeout(Duration::from_secs(5), second)
        .await
        .expect("Retry logs in again")
        .unwrap()
        .unwrap();
    round_trip(&mut lease).await;
    assert_eq!(status.borrow().reach, Reach::Ready { connections: 2 });
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lost_connection_reaches_its_leases_and_the_link_reconnects_on_demand() {
    let server = server(Behaviour::default(), &HOST_A).await;
    let dialer = TestDialer::new(&[server.port]);
    let link = link(dialer.clone(), TestAuthenticator::accepting(), limits());
    let lease = link.open(Open::Session).await.unwrap();
    dialer.sever();
    let lost = tokio::time::timeout(Duration::from_secs(5), lease.lost())
        .await
        .expect("the lease learns its connection is lost");
    assert!(matches!(lost, Error::Lost(_)), "{lost:?}");
    let mut lease = link.open(Open::Session).await.unwrap();
    round_trip(&mut lease).await;
    assert_eq!(dialer.dials.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_host_key_stops_new_connections() {
    let first = server(
        Behaviour {
            max_sessions: 1,
            ..Behaviour::default()
        },
        &HOST_A,
    )
    .await;
    let impostor = server(Behaviour::default(), &HOST_B).await;
    let dialer = TestDialer::new(&[first.port, impostor.port]);
    let link = link(dialer.clone(), TestAuthenticator::accepting(), limits());
    let mut lease = link.open(Open::Session).await.unwrap();
    let _second = {
        let link = link.clone();
        tokio::spawn(async move { link.open(Open::Session).await })
    };
    let status = link.status();
    eventually("the host key change", || {
        matches!(
            status.borrow().login,
            Some(Login::Failed(Error::HostKeyChanged { .. }))
        )
    })
    .await;
    assert_eq!(status.borrow().reach, Reach::Ready { connections: 1 });
    // Only the person can vouch for a new host key: no change on this
    // computer lifts the stop.
    link.wake();
    link.network_changed();
    link.unlocked();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        dialer.dials.load(Ordering::SeqCst),
        2,
        "no more connections"
    );
    round_trip(&mut lease).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn forwarding_channels_use_no_session_slots() {
    let server = server(
        Behaviour {
            max_sessions: 1,
            ..Behaviour::default()
        },
        &HOST_A,
    )
    .await;
    let link = link(
        TestDialer::new(&[server.port]),
        TestAuthenticator::accepting(),
        limits(),
    );
    let mut leases = vec![link.open(Open::Session).await.unwrap()];
    for port in 1..=3 {
        leases.push(
            link.open(Open::DirectTcpip {
                host: "localhost".into(),
                port,
            })
            .await
            .unwrap(),
        );
    }
    for lease in &mut leases {
        round_trip(lease).await;
    }
    assert_eq!(connections(&leases), 1);
    assert_eq!(server.seen.connections.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unused_connection_closes_after_the_idle_time() {
    let server = server(Behaviour::default(), &HOST_A).await;
    let limits = newport_ssh::Limits {
        idle: Duration::from_millis(300),
        ..limits()
    };
    let link = link(
        TestDialer::new(&[server.port]),
        TestAuthenticator::accepting(),
        limits,
    );
    let mut lease = link.open(Open::Session).await.unwrap();
    round_trip(&mut lease).await;
    drop(lease);
    let status = link.status();
    eventually("the link to go idle", || {
        status.borrow().reach == Reach::Idle
    })
    .await;
    eventually("the server's session to close", || {
        server.seen.sessions.load(Ordering::SeqCst) == 0
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_server_is_retried_with_backoff() {
    let closed = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    };
    let server = server(Behaviour::default(), &HOST_A).await;
    let dialer = TestDialer::new(&[closed, server.port]);
    let link = link(dialer.clone(), TestAuthenticator::accepting(), limits());
    let opening = {
        let link = link.clone();
        tokio::spawn(async move { link.open(Open::Session).await })
    };
    let status = link.status();
    eventually("the failed first attempt", || {
        matches!(
            status.borrow().reach,
            Reach::Reconnecting { attempt: 1, .. }
        ) && matches!(
            status.borrow().login,
            Some(Login::Failed(Error::Unreachable(_)))
        )
    })
    .await;
    let mut lease = tokio::time::timeout(Duration::from_secs(5), opening)
        .await
        .expect("the retry reaches the server")
        .unwrap()
        .unwrap();
    round_trip(&mut lease).await;
    assert_eq!(dialer.dials.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lease_dropped_on_another_thread_still_closes_its_session() {
    let server = server(Behaviour::default(), &HOST_A).await;
    let link = link(
        TestDialer::new(&[server.port]),
        TestAuthenticator::accepting(),
        limits(),
    );
    let lease = link.open(Open::Session).await.unwrap();
    std::thread::spawn(move || drop(lease)).join().unwrap();
    eventually("the server's session to close", || {
        server.seen.sessions.load(Ordering::SeqCst) == 0
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_without_sessions_is_recognised_after_a_failed_first_dial() {
    let closed = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    };
    let server = server(
        Behaviour {
            max_sessions: 0,
            ..Behaviour::default()
        },
        &HOST_A,
    )
    .await;
    let link = link(
        TestDialer::new(&[closed, server.port]),
        TestAuthenticator::accepting(),
        limits(),
    );
    let refused = tokio::time::timeout(Duration::from_secs(5), link.open(Open::Session))
        .await
        .expect("an answer, not a wait");
    assert_eq!(refused.err(), Some(Error::SessionsDisabled));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_key_the_verifier_refuses_stops_the_link() {
    struct Refuse;
    impl newport_ssh::HostKeys for Refuse {
        fn verify(
            &self,
            target: &newport_ssh::Target,
            _: &russh::keys::PublicKeyOrCertificate,
        ) -> Result<(), Error> {
            assert_eq!(target.user, "fixture", "verified for the target");
            Err(Error::HostKeyRejected(
                "host certificates aren't accepted".into(),
            ))
        }
    }
    let server = server(Behaviour::default(), &HOST_A).await;
    let dialer = TestDialer::new(&[server.port]);
    let link = link_with(
        dialer.clone(),
        TestAuthenticator::accepting(),
        std::sync::Arc::new(Refuse),
        limits(),
    );
    let _opening = {
        let link = link.clone();
        tokio::spawn(async move { link.open(Open::Session).await })
    };
    let status = link.status();
    eventually("the refusal", || {
        status.borrow().reach
            == Reach::Failed(Error::HostKeyRejected(
                "host certificates aren't accepted".into(),
            ))
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(dialer.dials.load(Ordering::SeqCst), 1, "no automatic retry");
}

#[tokio::test(flavor = "multi_thread")]
async fn each_request_that_waits_for_a_slot_raises_the_pressure_once() {
    let server = server(
        Behaviour {
            max_sessions: 1,
            ..Behaviour::default()
        },
        &HOST_A,
    )
    .await;
    let limits = newport_ssh::Limits {
        max_connections: 1,
        ..limits()
    };
    let link = link(
        TestDialer::new(&[server.port]),
        TestAuthenticator::accepting(),
        limits,
    );
    let pressure = link.pressure();
    let first = link.open(Open::Session).await.unwrap();
    assert_eq!(
        *pressure.borrow(),
        0,
        "waiting for the login is no pressure"
    );
    let waiting = |link: &Link| {
        let link = link.clone();
        tokio::spawn(async move { link.open(Open::Session).await })
    };
    let second = waiting(&link);
    eventually("the pressure to rise", || *pressure.borrow() == 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let third = waiting(&link);
    eventually("the pressure to rise again", || *pressure.borrow() == 2).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(*pressure.borrow(), 2, "once per request");
    drop(first);
    let mut served = tokio::time::timeout(Duration::from_secs(5), second)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    round_trip(&mut served).await;
    drop(served);
    tokio::time::timeout(Duration::from_secs(5), third)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(*pressure.borrow(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lost_connection_ends_a_session_refusal_it_caused() {
    let server = server(
        Behaviour {
            max_sessions: 1,
            refuse_sessions_on: Some(1),
            ..Behaviour::default()
        },
        &HOST_A,
    )
    .await;
    let dialer = TestDialer::new(&[server.port]);
    let link = link(dialer.clone(), TestAuthenticator::accepting(), limits());
    let _first = link.open(Open::Session).await.unwrap();
    let second = {
        let link = link.clone();
        tokio::spawn(async move { link.open(Open::Session).await })
    };
    eventually("the second connection's refusal", || {
        server.seen.refusals.load(Ordering::SeqCst) >= 2
    })
    .await;
    // The first connection's session made the refusal look secondary.
    dialer.sever();
    let mut lease = tokio::time::timeout(Duration::from_secs(5), second)
        .await
        .expect("a new connection serves the request")
        .unwrap()
        .unwrap();
    round_trip(&mut lease).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_connection_that_refuses_sessions_after_holding_some_is_not_asked_in_a_loop() {
    let server = server(Behaviour::default(), &HOST_A).await;
    let link = link(
        TestDialer::new(&[server.port]),
        TestAuthenticator::accepting(),
        limits(),
    );
    let mut first = link.open(Open::Session).await.unwrap();
    round_trip(&mut first).await;
    drop(first);
    eventually("the server's session to close", || {
        server.seen.sessions.load(Ordering::SeqCst) == 0
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    server.seen.refusing.store(true, Ordering::SeqCst);
    let _waiting = {
        let link = link.clone();
        tokio::spawn(async move { link.open(Open::Session).await })
    };
    eventually("the refusal", || {
        server.seen.refusals.load(Ordering::SeqCst) >= 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let refusals = server.seen.refusals.load(Ordering::SeqCst);
    assert!(refusals < 10, "{refusals} refusals: asked again in a loop");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_without_sessions_is_recognised_after_a_forwarding_connection_is_lost() {
    let server = server(
        Behaviour {
            max_sessions: 0,
            ..Behaviour::default()
        },
        &HOST_A,
    )
    .await;
    let dialer = TestDialer::new(&[server.port]);
    let authenticator = TestAuthenticator::accepting();
    let link = link(dialer.clone(), authenticator.clone(), limits());
    let forward = link
        .open(Open::DirectTcpip {
            host: "localhost".into(),
            port: 5290,
        })
        .await
        .unwrap();
    dialer.sever();
    tokio::time::timeout(Duration::from_secs(5), forward.lost())
        .await
        .expect("the forward learns its connection is lost");
    let refused = tokio::time::timeout(Duration::from_secs(5), link.open(Open::Session))
        .await
        .expect("an answer, not a login every backoff");
    assert_eq!(refused.err(), Some(Error::SessionsDisabled));
    assert_eq!(authenticator.attempts.load(Ordering::SeqCst), 2);

    let refusals = server.seen.refusals.load(Ordering::SeqCst);
    assert_eq!(
        link.open(Open::Session).await.err(),
        Some(Error::SessionsDisabled)
    );
    assert_eq!(
        server.seen.refusals.load(Ordering::SeqCst),
        refusals,
        "known without asking"
    );
    link.retry();
    assert_eq!(
        link.open(Open::Session).await.err(),
        Some(Error::SessionsDisabled)
    );
    assert_eq!(
        server.seen.refusals.load(Ordering::SeqCst),
        refusals + 1,
        "Retry asks the server again"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_healthy_connection_ends_a_run_of_login_failures() {
    let closed = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    };
    let server = server(
        Behaviour {
            max_sessions: 1,
            ..Behaviour::default()
        },
        &HOST_A,
    )
    .await;
    let dialer = TestDialer::new(&[closed, closed, server.port, closed]);
    let limits = newport_ssh::Limits {
        healthy: Duration::from_millis(300),
        backoff_min: Duration::from_millis(200),
        backoff_max: Duration::from_secs(60),
        ..limits()
    };
    let link = link(dialer.clone(), TestAuthenticator::accepting(), limits);
    // Two failures before the login that works.
    let _first = link.open(Open::Session).await.unwrap();
    assert_eq!(dialer.dials.load(Ordering::SeqCst), 3);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _second = {
        let link = link.clone();
        tokio::spawn(async move { link.open(Open::Session).await })
    };
    eventually("the next failed dial", || {
        dialer.dials.load(Ordering::SeqCst) == 4
    })
    .await;
    // A first failure waits at most the shortest backoff; a third, twice
    // as long at least.
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert!(
        dialer.dials.load(Ordering::SeqCst) >= 5,
        "the backoff kept growing"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn retry_accepts_a_host_key_the_verifier_now_accepts() {
    let first = server(
        Behaviour {
            max_sessions: 1,
            ..Behaviour::default()
        },
        &HOST_A,
    )
    .await;
    let rekeyed = server(Behaviour::default(), &HOST_B).await;
    let dialer = TestDialer::new(&[first.port, rekeyed.port]);
    let link = link(dialer.clone(), TestAuthenticator::accepting(), limits());
    let _lease = link.open(Open::Session).await.unwrap();
    let second = {
        let link = link.clone();
        tokio::spawn(async move { link.open(Open::Session).await })
    };
    let status = link.status();
    eventually("the host key change", || {
        matches!(
            status.borrow().login,
            Some(Login::Failed(Error::HostKeyChanged { .. }))
        )
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(dialer.dials.load(Ordering::SeqCst), 2, "no automatic retry");
    link.retry();
    let mut lease = tokio::time::timeout(Duration::from_secs(5), second)
        .await
        .expect("Retry checks the key afresh")
        .unwrap()
        .unwrap();
    round_trip(&mut lease).await;
    assert_eq!(rekeyed.seen.sessions.load(Ordering::SeqCst), 1);
}
