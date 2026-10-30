mod support;

use newport_ssh::{Channel, Error, Incoming, Limits, Link, Listener, Open, Reach, Retry};
use russh::{ChannelMsg, ChannelOpenFailure, server::Msg};
use std::{sync::atomic::Ordering, time::Duration};
use support::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    task::JoinHandle,
};

type Remote = JoinHandle<Result<russh::Channel<Msg>, russh::Error>>;

fn short_idle() -> Limits {
    Limits {
        idle: Duration::from_millis(300),
        ..limits()
    }
}

async fn incoming(listener: &mut Listener) -> Incoming {
    tokio::time::timeout(Duration::from_secs(3), listener.accept())
        .await
        .expect("a forwarded channel in time")
        .expect("a forwarded channel")
}

async fn accept(listener: &mut Listener) -> Channel {
    incoming(listener).await.accept().await
}

/// The server's end of a forwarded channel, once the client answered.
async fn opened(remote: Remote) -> Result<russh::Channel<Msg>, russh::Error> {
    tokio::time::timeout(Duration::from_secs(3), remote)
        .await
        .expect("the client answers in time")
        .unwrap()
}

/// Echo what the server sends over a forwarded channel.
fn echo(mut channel: Channel) {
    tokio::spawn(async move {
        while let Some(message) = channel.wait().await {
            if let ChannelMsg::Data { data } = message
                && channel.data(&data[..]).await.is_err()
            {
                break;
            }
        }
    });
}

/// Once the client accepted the channel, send bytes from the server's end
/// and read them back.
async fn server_round_trip(remote: Remote) {
    echoes(
        opened(remote)
            .await
            .expect("the client accepted the channel"),
    )
    .await;
}

/// Send bytes from the server's end of a forwarded channel and read them back.
async fn echoes(remote: russh::Channel<Msg>) {
    let mut stream = remote.into_stream();
    stream.write_all(b"hello").await.unwrap();
    let mut echoed = [0; 5];
    tokio::time::timeout(Duration::from_secs(3), stream.read_exact(&mut echoed))
        .await
        .expect("an echo in time")
        .unwrap();
    assert_eq!(&echoed, b"hello");
}

async fn go_idle(link: &Link) {
    let status = link.status();
    eventually("the link to go idle", || {
        status.borrow().reach == Reach::Idle
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_listener_receives_forwarded_connections_and_echoes_over_them() {
    let server = server(Behaviour::default(), &HOST_A).await;
    let link = link(
        TestDialer::new(&[server.port]),
        TestAuthenticator::accepting(),
        limits(),
    );
    let mut listener = link.listen_tcp("127.0.0.1", 8080).await.unwrap();
    assert_eq!(listener.port(), 8080);
    assert!(server.seen.forwarded(8080));
    for _ in 0..3 {
        let remote = server.connect(8080);
        echo(accept(&mut listener).await);
        server_round_trip(remote).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn port_zero_reports_the_port_the_server_bound() {
    let server = server(Behaviour::default(), &HOST_A).await;
    let link = link(
        TestDialer::new(&[server.port]),
        TestAuthenticator::accepting(),
        limits(),
    );
    let mut listener = link.listen_tcp("127.0.0.1", 0).await.unwrap();
    let port = listener.port();
    assert_ne!(port, 0);
    assert!(server.seen.forwarded(port));
    let remote = server.connect(port);
    echo(accept(&mut listener).await);
    server_round_trip(remote).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_listener_cancels_the_forward_on_the_server() {
    let server = server(Behaviour::default(), &HOST_A).await;
    let link = link(
        TestDialer::new(&[server.port]),
        TestAuthenticator::accepting(),
        limits(),
    );
    let listener = link.listen_tcp("127.0.0.1", 8080).await.unwrap();
    drop(listener);
    eventually("the server to cancel the forward", || {
        !server.seen.forwarded(8080)
    })
    .await;
    let mut listener = link.listen_tcp("127.0.0.1", 8080).await.unwrap();
    let remote = server.connect(8080);
    echo(accept(&mut listener).await);
    server_round_trip(remote).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lost_connection_ends_the_listener_and_a_new_one_can_listen_again() {
    let server = server(Behaviour::default(), &HOST_A).await;
    let dialer = TestDialer::new(&[server.port]);
    let link = link(dialer.clone(), TestAuthenticator::accepting(), limits());
    let mut listener = link.listen_tcp("127.0.0.1", 8080).await.unwrap();
    let pinned = listener.connection();
    dialer.sever();
    let ended = tokio::time::timeout(Duration::from_secs(3), listener.accept())
        .await
        .expect("the listener ends in time");
    assert!(ended.is_none());
    assert!(matches!(listener.lost().await, Error::Lost(_)));
    drop(listener);
    // The server may still hold the port for the old connection.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut listener = loop {
        match link.listen_tcp("127.0.0.1", 8080).await {
            Ok(listener) => break listener,
            Err(error) => {
                assert_eq!(error.retry(), Retry::Backoff, "{error}");
                assert!(tokio::time::Instant::now() < deadline, "{error}");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    };
    assert_ne!(listener.connection(), pinned);
    let remote = server.connect(8080);
    echo(accept(&mut listener).await);
    server_round_trip(remote).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_connection_with_a_listener_is_not_closed_as_idle() {
    let server = server(Behaviour::default(), &HOST_A).await;
    let link = link(
        TestDialer::new(&[server.port]),
        TestAuthenticator::accepting(),
        short_idle(),
    );
    let listener = link.listen_tcp("127.0.0.1", 8080).await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        link.status().borrow().reach,
        Reach::Ready { connections: 1 }
    );
    assert!(server.seen.forwarded(8080));
    assert_eq!(server.seen.connections.load(Ordering::SeqCst), 1);
    drop(listener);
    go_idle(&link).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_forwarded_channel_keeps_its_connection_until_it_closes() {
    let server = server(Behaviour::default(), &HOST_A).await;
    let link = link(
        TestDialer::new(&[server.port]),
        TestAuthenticator::accepting(),
        short_idle(),
    );
    let mut listener = link.listen_tcp("127.0.0.1", 8080).await.unwrap();
    let remote = server.connect(8080);
    let channel = accept(&mut listener).await;
    let _remote = opened(remote).await.unwrap();
    drop(listener);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        link.status().borrow().reach,
        Reach::Ready { connections: 1 }
    );
    drop(channel);
    go_idle(&link).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_forward_is_request_denied_and_unpins_the_connection() {
    let server = server(
        Behaviour {
            refuse_forwards: true,
            ..Behaviour::default()
        },
        &HOST_A,
    )
    .await;
    let link = link(
        TestDialer::new(&[server.port]),
        TestAuthenticator::accepting(),
        short_idle(),
    );
    let error = link.listen_tcp("127.0.0.1", 8080).await.err().unwrap();
    assert_eq!(error, Error::RequestDenied);
    assert_eq!(error.retry(), Retry::Backoff);
    go_idle(&link).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn channels_beyond_the_backlog_are_refused_without_blocking_the_connection() {
    let server = server(Behaviour::default(), &HOST_A).await;
    let link = link(
        TestDialer::new(&[server.port]),
        TestAuthenticator::accepting(),
        limits(),
    );
    let mut listener = link.listen_tcp("127.0.0.1", 8080).await.unwrap();
    let mut remotes: Vec<Remote> = (0..130).map(|_| server.connect(8080)).collect();
    // The opens arrive in any order; whichever come after the backlog fills
    // are refused, and the rest wait for an answer.
    eventually("the refusals", || {
        remotes.iter().filter(|remote| remote.is_finished()).count() == 2
    })
    .await;
    let (refused, waiting): (Vec<_>, Vec<_>) =
        remotes.drain(..).partition(|remote| remote.is_finished());
    for remote in refused {
        match opened(remote).await {
            Err(russh::Error::ChannelOpenFailure(ChannelOpenFailure::ResourceShortage)) => {}
            other => panic!("expected a refusal, got {:?}", other.map(|c| c.id())),
        }
    }
    remotes = waiting;
    let mut session = link.open(Open::Session).await.unwrap();
    round_trip(&mut session).await;
    echo(accept(&mut listener).await);
    eventually("the accepted channel to open", || {
        remotes.iter().any(|remote| remote.is_finished())
    })
    .await;
    let index = remotes.iter().position(|remote| remote.is_finished());
    server_round_trip(remotes.remove(index.unwrap())).await;
    // A freed place takes the next connection, behind the ones waiting.
    let remote = server.connect(8080);
    for _ in 0..127 {
        drop(incoming(&mut listener).await);
    }
    echo(accept(&mut listener).await);
    server_round_trip(remote).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_channel_is_refused_with_the_reason_its_listener_gives() {
    let server = server(Behaviour::default(), &HOST_A).await;
    let link = link(
        TestDialer::new(&[server.port]),
        TestAuthenticator::accepting(),
        short_idle(),
    );
    let mut listener = link.listen_tcp("127.0.0.1", 8080).await.unwrap();
    let remote = server.connect(8080);
    incoming(&mut listener)
        .await
        .reject(ChannelOpenFailure::ConnectFailed)
        .await;
    match opened(remote).await {
        Err(russh::Error::ChannelOpenFailure(ChannelOpenFailure::ConnectFailed)) => {}
        other => panic!("expected a refusal, got {:?}", other.map(|c| c.id())),
    }
    let remote = server.connect(8080);
    drop(incoming(&mut listener).await);
    match opened(remote).await {
        Err(russh::Error::ChannelOpenFailure(ChannelOpenFailure::AdministrativelyProhibited)) => {}
        other => panic!("expected a refusal, got {:?}", other.map(|c| c.id())),
    }
    // Refused channels hold nothing open.
    drop(listener);
    go_idle(&link).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_port_listened_on_again_right_after_a_drop_is_not_refused() {
    let server = server(Behaviour::default(), &HOST_A).await;
    let link = link(
        TestDialer::new(&[server.port]),
        TestAuthenticator::accepting(),
        limits(),
    );
    let mut listener = link.listen_tcp("127.0.0.1", 8080).await.unwrap();
    for _ in 0..20 {
        drop(listener);
        listener = link.listen_tcp("127.0.0.1", 8080).await.unwrap();
    }
    assert!(server.seen.forwarded(8080));
    let remote = server.connect(8080);
    echo(accept(&mut listener).await);
    server_round_trip(remote).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_channel_right_behind_the_port_zero_reply_reaches_the_listener() {
    let server = server(
        Behaviour {
            connect_on_bind: true,
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
    let mut listener = link.listen_tcp("127.0.0.1", 0).await.unwrap();
    echo(accept(&mut listener).await);
    eventually("the server's channel to open", || {
        !server.seen.on_bind.lock().unwrap().is_empty()
    })
    .await;
    let remote = server.seen.on_bind.lock().unwrap().pop().unwrap();
    echoes(remote.expect("the client accepted the channel")).await;
}
