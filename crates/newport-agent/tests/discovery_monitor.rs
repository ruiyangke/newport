#![cfg(target_os = "linux")]
use newport_protocol::monitor::{self, Call, Reply, Request, ports::discovery, wire};
use std::{
    io::{BufReader, Write},
    net::TcpListener,
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

#[test]
fn monitor_returns_verified_holders_and_new_generations_for_refresh() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let deadline = Instant::now() + Duration::from_secs(50);
    let agent = std::env::var_os("NEWPORT_DISCOVERY_TEST_AGENT")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_newport-agent").into());
    let mut child = Command::new(agent)
        .args(["monitor", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let calls = [false, true]
        .into_iter()
        .enumerate()
        .map(|(index, refresh)| Call {
            id: u64::try_from(index + 1).unwrap(),
            request: Request::Ports(discovery::Request { refresh }),
        })
        .collect::<Vec<_>>();
    let mut input = child.stdin.take().unwrap();
    for call in &calls {
        input
            .write_all(&wire::encode_request(call).unwrap())
            .unwrap();
    }
    drop(input);
    let output = child.stdout.take().unwrap();
    let (sent, receive) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut output = BufReader::new(output);
        let result = calls
            .iter()
            .map(|call| wire::read_reply(&mut output, call))
            .collect::<Result<Vec<_>, _>>();
        let _ = sent.send(result);
    });
    let result = receive.recv_timeout(deadline.saturating_duration_since(Instant::now()));
    if result.is_err() {
        let _ = child.kill();
    }
    let exit = loop {
        if let Some(exit) = child.try_wait().unwrap() {
            break exit;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            break child.wait().unwrap();
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    reader.join().unwrap();
    assert!(exit.success(), "monitor failed: {exit}");
    let responses = result.expect("bounded monitor scans").unwrap();
    let mut previous = 0;
    for response in responses {
        let Reply::Ports(snapshot) = response.reply else {
            panic!("{response:?}");
        };
        assert!(snapshot.generation > previous);
        previous = snapshot.generation;
        let socket = snapshot
            .sockets
            .iter()
            .find(|socket| socket.endpoint.port == port)
            .expect("live listener");
        assert!(
            snapshot
                .processes
                .iter()
                .any(|process| socket.holders.contains(&process.id)
                    && process.identity.pid == std::process::id())
        );
        assert_eq!(snapshot.coverage.ipv4.outcome, discovery::Outcome::Complete);
        assert!(snapshot.diagnostics.ipv4_source.is_some());
        assert!(snapshot.diagnostics.ipv6_source.is_some());
        println!(
            "generation={} source={:?}/{:?} elevation={:?}",
            snapshot.generation,
            snapshot.diagnostics.ipv4_source,
            snapshot.diagnostics.ipv6_source,
            snapshot.coverage.elevation.outcome,
        );
        assert!(discovery::wire::fits_response(&snapshot).unwrap());
        assert_eq!(monitor::VERSION, 1);
    }
}
