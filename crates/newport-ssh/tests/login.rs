//! The login rules learned from Newport's earlier transport: one prompt at a
//! time, an explicit Retry replaces a stale prompt while a routine one keeps
//! it, a prompt the person walked away from doesn't come back by itself,
//! sleep never leaves a stale login behind, and the login's network deadline
//! never counts the approval wait.
mod support;

use newport_ssh::{Error, Limits, Login, Open, Reach};
use std::{sync::Arc, sync::atomic::Ordering, time::Duration};
use support::*;
use tokio::sync::Notify;

fn never_approved() -> Step {
    Step::Approve(Arc::new(Notify::new()))
}

/// A login deadline far shorter than the approval waits below, and a backoff
/// long enough to see.
fn short_login() -> Limits {
    Limits {
        login: Duration::from_millis(300),
        backoff_min: Duration::from_millis(500),
        backoff_max: Duration::from_millis(500),
        ..limits()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn retry_fresh_replaces_a_login_waiting_for_approval() {
    let server = server(Behaviour::default(), &HOST_A).await;
    let authenticator = TestAuthenticator::new(vec![never_approved(), Step::Accept]);
    let link = link(
        TestDialer::new(&[server.port]),
        authenticator.clone(),
        limits(),
    );
    let opening = {
        let link = link.clone();
        tokio::spawn(async move { link.open(Open::Session).await })
    };
    let status = link.status();
    eventually("the approval wait", || {
        status.borrow().login == Some(Login::WaitingForApproval)
    })
    .await;
    link.retry_fresh();
    let mut channel = tokio::time::timeout(Duration::from_secs(5), opening)
        .await
        .expect("the fresh login serves the request")
        .unwrap()
        .unwrap();
    round_trip(&mut channel).await;
    assert_eq!(authenticator.attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn retry_keeps_a_login_waiting_for_approval() {
    let server = server(Behaviour::default(), &HOST_A).await;
    let approve = Arc::new(Notify::new());
    let authenticator = TestAuthenticator::new(vec![Step::Approve(approve.clone()), Step::Accept]);
    let link = link(
        TestDialer::new(&[server.port]),
        authenticator.clone(),
        limits(),
    );
    let opening = {
        let link = link.clone();
        tokio::spawn(async move { link.open(Open::Session).await })
    };
    let status = link.status();
    eventually("the approval wait", || {
        status.borrow().login == Some(Login::WaitingForApproval)
    })
    .await;
    link.retry();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(status.borrow().login, Some(Login::WaitingForApproval));
    assert_eq!(authenticator.attempts.load(Ordering::SeqCst), 1);
    approve.notify_one();
    let mut channel = tokio::time::timeout(Duration::from_secs(5), opening)
        .await
        .expect("the approved login serves the request")
        .unwrap()
        .unwrap();
    round_trip(&mut channel).await;
    assert_eq!(authenticator.attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stalled_login_times_out_and_is_retried_after_a_backoff() {
    let server = server(Behaviour::default(), &HOST_A).await;
    let authenticator = TestAuthenticator::new(vec![Step::Stall, Step::Accept]);
    let link = link(
        TestDialer::new(&[server.port]),
        authenticator.clone(),
        short_login(),
    );
    let opening = {
        let link = link.clone();
        tokio::spawn(async move { link.open(Open::Session).await })
    };
    let status = link.status();
    eventually("the login deadline", || {
        status.borrow().login == Some(Login::Failed(Error::TimedOut))
    })
    .await;
    assert!(
        matches!(
            status.borrow().reach,
            Reach::Reconnecting {
                attempt: 1,
                next: Some(_)
            }
        ),
        "{:?}",
        status.borrow().reach
    );
    assert_eq!(authenticator.attempts.load(Ordering::SeqCst), 1);
    let mut channel = tokio::time::timeout(Duration::from_secs(5), opening)
        .await
        .expect("the next login serves the request")
        .unwrap()
        .unwrap();
    round_trip(&mut channel).await;
    assert_eq!(authenticator.attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn waiting_for_approval_does_not_count_against_the_login_deadline() {
    let server = server(Behaviour::default(), &HOST_A).await;
    let approve = Arc::new(Notify::new());
    let authenticator = TestAuthenticator::new(vec![Step::Approve(approve.clone())]);
    let link = link(
        TestDialer::new(&[server.port]),
        authenticator.clone(),
        short_login(),
    );
    let opening = {
        let link = link.clone();
        tokio::spawn(async move { link.open(Open::Session).await })
    };
    let status = link.status();
    eventually("the approval wait", || {
        status.borrow().login == Some(Login::WaitingForApproval)
    })
    .await;
    tokio::time::sleep(short_login().login * 3).await;
    assert_eq!(status.borrow().login, Some(Login::WaitingForApproval));
    approve.notify_one();
    let mut channel = tokio::time::timeout(Duration::from_secs(5), opening)
        .await
        .expect("the approved login serves the request")
        .unwrap()
        .unwrap();
    round_trip(&mut channel).await;
    assert_eq!(authenticator.attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_login_deadline_resumes_after_approval() {
    let server = server(Behaviour::default(), &HOST_A).await;
    let approve = Arc::new(Notify::new());
    let authenticator = TestAuthenticator::new(vec![Step::ApproveThenStall(approve.clone())]);
    let link = link(
        TestDialer::new(&[server.port]),
        authenticator.clone(),
        short_login(),
    );
    let _opening = {
        let link = link.clone();
        tokio::spawn(async move { link.open(Open::Session).await })
    };
    let status = link.status();
    eventually("the approval wait", || {
        status.borrow().login == Some(Login::WaitingForApproval)
    })
    .await;
    tokio::time::sleep(short_login().login * 3).await;
    assert_eq!(status.borrow().login, Some(Login::WaitingForApproval));
    approve.notify_one();
    eventually("the login deadline", || {
        status.borrow().login == Some(Login::Failed(Error::TimedOut))
    })
    .await;
    assert_eq!(authenticator.attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_login_abandoned_while_approving_waits_for_retry() {
    let server = server(Behaviour::default(), &HOST_A).await;
    let authenticator = TestAuthenticator::new(vec![never_approved(), Step::Accept]);
    let link = link(
        TestDialer::new(&[server.port]),
        authenticator.clone(),
        limits(),
    );
    let status = link.status();
    {
        let opening = link.open(Open::Session);
        tokio::pin!(opening);
        tokio::select! {
            _ = &mut opening => panic!("nothing approved the login"),
            () = eventually("the approval wait", || {
                status.borrow().login == Some(Login::WaitingForApproval)
            }) => {}
        }
        // The requester walks away while the prompt is up.
    }
    eventually("the fence", || {
        status.borrow().login == Some(Login::Failed(Error::ApprovalRequired))
    })
    .await;
    let again = {
        let link = link.clone();
        tokio::spawn(async move { link.open(Open::Session).await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        authenticator.attempts.load(Ordering::SeqCst),
        1,
        "no prompt comes back by itself"
    );
    link.retry();
    tokio::time::timeout(Duration::from_secs(5), again)
        .await
        .expect("Retry logs in")
        .unwrap()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_login_served_elsewhere_is_cancelled_without_a_fence() {
    let server = server(
        Behaviour {
            max_sessions: 1,
            ..Behaviour::default()
        },
        &HOST_A,
    )
    .await;
    let authenticator = TestAuthenticator::new(vec![Step::Accept, never_approved(), Step::Accept]);
    let link = link(
        TestDialer::new(&[server.port]),
        authenticator.clone(),
        limits(),
    );
    let first = link.open(Open::Session).await.unwrap();
    let second = {
        let link = link.clone();
        tokio::spawn(async move { link.open(Open::Session).await })
    };
    let status = link.status();
    eventually("the second login's approval wait", || {
        status.borrow().login == Some(Login::WaitingForApproval)
    })
    .await;
    // The first session closes; the request takes its slot, and the login
    // nobody needs any more is withdrawn without blocking later ones.
    drop(first);
    tokio::time::timeout(Duration::from_secs(5), second)
        .await
        .expect("the freed slot serves the request")
        .unwrap()
        .unwrap();
    eventually("the login to be withdrawn", || {
        status.borrow().login.is_none()
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn no_login_starts_while_asleep() {
    let server = server(Behaviour::default(), &HOST_A).await;
    let authenticator = TestAuthenticator::accepting();
    let link = link(
        TestDialer::new(&[server.port]),
        authenticator.clone(),
        limits(),
    );
    link.sleep();
    let opening = {
        let link = link.clone();
        tokio::spawn(async move { link.open(Open::Session).await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(authenticator.attempts.load(Ordering::SeqCst), 0);
    link.wake();
    tokio::time::timeout(Duration::from_secs(5), opening)
        .await
        .expect("wake logs in")
        .unwrap()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn wake_replaces_a_login_started_before_sleep() {
    let server = server(Behaviour::default(), &HOST_A).await;
    let authenticator = TestAuthenticator::new(vec![never_approved(), Step::Accept]);
    let link = link(
        TestDialer::new(&[server.port]),
        authenticator.clone(),
        limits(),
    );
    let opening = {
        let link = link.clone();
        tokio::spawn(async move { link.open(Open::Session).await })
    };
    let status = link.status();
    eventually("the approval wait", || {
        status.borrow().login == Some(Login::WaitingForApproval)
    })
    .await;
    link.sleep();
    link.wake();
    tokio::time::timeout(Duration::from_secs(5), opening)
        .await
        .expect("a fresh login after wake")
        .unwrap()
        .unwrap();
    assert_eq!(authenticator.attempts.load(Ordering::SeqCst), 2);
    assert_eq!(status.borrow().login, None, "no fence after sleep");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_open_unless_stopped_fails_with_a_declined_login_and_waits_after_retry() {
    let server = server(Behaviour::default(), &HOST_A).await;
    let approve = Arc::new(Notify::new());
    let authenticator = TestAuthenticator::new(vec![Step::Decline, Step::Approve(approve.clone())]);
    let link = link(
        TestDialer::new(&[server.port]),
        authenticator.clone(),
        limits(),
    );
    let declined = tokio::time::timeout(
        Duration::from_secs(5),
        link.open_unless_stopped(Open::Session),
    )
    .await
    .expect("the declined login ends the waiting request");
    assert_eq!(declined.err(), Some(Error::ApprovalRequired));
    let status = link.status();
    eventually("the stopped link", || {
        status.borrow().reach == Reach::Failed(Error::ApprovalRequired)
    })
    .await;
    let stopped = tokio::time::timeout(
        Duration::from_secs(1),
        link.open_unless_stopped(Open::Session),
    )
    .await
    .expect("a stopped link fails the request at once");
    assert_eq!(stopped.err(), Some(Error::ApprovalRequired));
    assert_eq!(authenticator.attempts.load(Ordering::SeqCst), 1);

    link.retry_fresh();
    let opening = {
        let link = link.clone();
        tokio::spawn(async move { link.open_unless_stopped(Open::Session).await })
    };
    eventually("the new login's approval wait", || {
        status.borrow().login == Some(Login::WaitingForApproval)
    })
    .await;
    assert!(
        !opening.is_finished(),
        "the request waits for the new login"
    );
    approve.notify_one();
    let mut channel = tokio::time::timeout(Duration::from_secs(5), opening)
        .await
        .expect("the approved login serves the request")
        .unwrap()
        .unwrap();
    round_trip(&mut channel).await;
    assert_eq!(authenticator.attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_open_unless_stopped_sent_right_after_retry_never_sees_the_cleared_failure() {
    let server = server(Behaviour::default(), &HOST_A).await;
    for _ in 0..20 {
        let authenticator = TestAuthenticator::new(vec![Step::Decline, Step::Accept]);
        let link = link(
            TestDialer::new(&[server.port]),
            authenticator.clone(),
            limits(),
        );
        let declined = link.open_unless_stopped(Open::Session).await;
        assert_eq!(declined.err(), Some(Error::ApprovalRequired));
        link.retry_fresh();
        let mut channel = tokio::time::timeout(
            Duration::from_secs(5),
            link.open_unless_stopped(Open::Session),
        )
        .await
        .expect("the retried login serves the request")
        .expect("the request is taken after the retry");
        round_trip(&mut channel).await;
    }
}
