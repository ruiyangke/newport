use super::*;
use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

#[derive(Clone, Default)]
struct TestHost {
    dispatched: Arc<AtomicUsize>,
    terminals: Arc<AtomicUsize>,
    pending: Arc<AtomicUsize>,
}
struct Terminal(Arc<AtomicUsize>);
impl Drop for Terminal {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
impl Host for TestHost {
    async fn dispatch(&self, method: &str, args: Value) -> Result<Value, Value> {
        self.dispatched.fetch_add(1, Ordering::SeqCst);
        match method {
            "fixture.echo" => Ok(args),
            "files_write_text" => Ok(json!({"bytes":args["content"].as_str().unwrap().len()})),
            "fixture.large" => Ok(json!("x".repeat(MAX_RESPONSE))),
            "set_integration_enabled" | "set_clipboard_enabled" => {
                if args["hold"] == true {
                    self.pending.fetch_add(1, Ordering::SeqCst);
                    let _guard = Terminal(self.pending.clone());
                    std::future::pending().await
                } else {
                    Ok(args)
                }
            }
            "fixture.pending" => {
                self.pending.fetch_add(1, Ordering::SeqCst);
                let _guard = Terminal(self.pending.clone());
                std::future::pending().await
            }
            _ => Err(json!({"code":"UNKNOWN_METHOD", "message":"Unknown fixture method"})),
        }
    }
    async fn open_terminal(&self, _: TerminalOpen) -> Result<Box<dyn Send>, String> {
        self.terminals.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Terminal(self.terminals.clone())))
    }
}
struct Fixture {
    directory: PathBuf,
    endpoint: PathBuf,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
    host: TestHost,
    events: EventLog,
}
impl Fixture {
    fn start() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "np-rpc-{}",
            &Uuid::new_v4().simple().to_string()[..12]
        ));
        newport_platform::filesystem::create_private_directory(&directory).unwrap();
        let endpoint = directory.join("native-client/desktop.sock");
        let server = Server::bind(&endpoint).unwrap();
        let events = server.events();
        let host = TestHost::default();
        let task = tokio::spawn(server.run(host.clone()));
        Self {
            directory,
            endpoint,
            task,
            host,
            events,
        }
    }
    async fn stop(self) {
        self.task.abort();
        assert!(self.task.await.unwrap_err().is_cancelled());
        std::fs::remove_dir_all(self.directory).unwrap();
    }
    async fn call(&self, method: &'static str, args: Value) -> Result<Value, String> {
        let endpoint = self.endpoint.clone();
        tokio::task::spawn_blocking(move || {
            newport_client::backend::Backend::at_path(endpoint).call_with_timeout(
                method,
                args,
                Duration::from_secs(2),
            )
        })
        .await
        .unwrap()
    }
}
fn connection(path: PathBuf) -> newport_platform::ipc::Stream {
    let stream = newport_platform::ipc::Stream::connect(path).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
}
fn request(stream: &mut newport_platform::ipc::Stream, method: &str, args: Value) {
    let bytes = rmp_serde::to_vec_named(&Request {
        version: VERSION,
        method: method.into(),
        args,
    })
    .unwrap();
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&bytes).unwrap();
}
fn response(stream: &mut newport_platform::ipc::Stream) -> Response {
    let mut header = [0; 4];
    stream.read_exact(&mut header).unwrap();
    let mut bytes = vec![0; u32::from_be_bytes(header) as usize];
    stream.read_exact(&mut bytes).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn actual_client_uses_host_and_bounded_event_history() {
    let fixture = Fixture::start();
    assert_eq!(
        fixture
            .call("fixture.echo", json!({"scope":"one"}))
            .await
            .unwrap(),
        json!({"scope":"one"})
    );
    assert!(
        fixture
            .call("fixture.unknown", json!({}))
            .await
            .unwrap_err()
            .contains("Unknown fixture method")
    );
    for index in 0..125 {
        fixture.events.record(json!({"index":index}));
    }
    let history = fixture.call("$events", json!({"after":0})).await.unwrap();
    assert_eq!(history["cursor"], 125);
    assert_eq!(history["entries"].as_array().unwrap().len(), 100);
    assert_eq!(history["entries"][0]["index"], 25);
    let recent = fixture.call("$events", json!({"after":124})).await.unwrap();
    assert_eq!(recent["entries"], json!([{"index":124}]));
    assert_eq!(fixture.host.dispatched.load(Ordering::SeqCst), 2);
    fixture.stop().await;
}

#[tokio::test]
async fn malformed_frames_never_dispatch_and_do_not_kill_the_listener() {
    let fixture = Fixture::start();
    let invalid_version = rmp_serde::to_vec_named(&Request {
        version: VERSION + 1,
        method: "fixture.echo".into(),
        args: json!({}),
    })
    .unwrap();
    let malformed = [
        vec![0, 0, 0, 0],
        ((MAX_REQUEST + 1) as u32).to_be_bytes().to_vec(),
        vec![0, 0, 0, 1, 0xc1],
        [
            (invalid_version.len() as u32).to_be_bytes().as_slice(),
            &invalid_version,
        ]
        .concat(),
    ];
    for bytes in malformed {
        let path = fixture.endpoint.clone();
        tokio::task::spawn_blocking(move || {
            let mut stream = connection(path);
            stream.write_all(&bytes).unwrap();
            let result = stream.read(&mut [0]);
            assert!(matches!(result, Ok(0) | Err(_)));
        })
        .await
        .unwrap();
    }
    assert_eq!(fixture.host.dispatched.load(Ordering::SeqCst), 0);
    assert!(fixture.call("fixture.echo", json!({})).await.is_ok());
    fixture.stop().await;
}

#[tokio::test]
async fn closing_a_real_terminal_lease_releases_only_its_guard() {
    let fixture = Fixture::start();
    let mut leases = Vec::new();
    for _ in 0..2 {
        let path = fixture.endpoint.clone();
        let lease = tokio::task::spawn_blocking(move || {
            let mut stream = connection(path);
            request(
                &mut stream,
                protocol::TERMINAL_OPEN_LEASE,
                json!({"id":Uuid::new_v4(),"session":Uuid::new_v4(),"cols":80,"rows":24}),
            );
            assert!(response(&mut stream).error.is_none());
            stream
        })
        .await
        .unwrap();
        leases.push(lease);
    }
    assert_eq!(fixture.host.terminals.load(Ordering::SeqCst), 2);
    for expected in [1, 0] {
        drop(leases.pop());
        tokio::time::timeout(Duration::from_secs(2), async {
            while fixture.host.terminals.load(Ordering::SeqCst) != expected {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
    }
    fixture.stop().await;
}

#[tokio::test]
async fn invalid_terminal_open_does_not_acquire_ownership() {
    let fixture = Fixture::start();
    let path = fixture.endpoint.clone();
    tokio::task::spawn_blocking(move || {
        let mut stream = connection(path);
        request(
            &mut stream,
            protocol::TERMINAL_OPEN_LEASE,
            json!({"id":"bad","unexpected":true}),
        );
        assert!(response(&mut stream).error.is_some());
    })
    .await
    .unwrap();
    assert_eq!(fixture.host.terminals.load(Ordering::SeqCst), 0);
    fixture.stop().await;
}

#[tokio::test]
async fn shutdown_drains_open_leases_pending_commands_and_incomplete_frames() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint = directory.path().join("desktop.sock");
    let server = Server::bind(&endpoint).unwrap();
    let host = TestHost::default();
    let (shutdown, wait) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(server.run_until(host.clone(), async {
        let _ = wait.await;
    }));
    let path = endpoint.clone();
    let streams = tokio::task::spawn_blocking(move || {
        let mut lease = connection(path.clone());
        request(
            &mut lease,
            protocol::TERMINAL_OPEN_LEASE,
            json!({"id":Uuid::new_v4(),"session":Uuid::new_v4(),"cols":80,"rows":24}),
        );
        assert!(response(&mut lease).error.is_none());
        let mut pending = connection(path.clone());
        request(&mut pending, "fixture.pending", json!({}));
        let mut incomplete = connection(path);
        incomplete.write_all(&[0, 0]).unwrap();
        [lease, pending, incomplete]
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while host.pending.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(host.terminals.load(Ordering::SeqCst), 1);
    shutdown.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    // Shutdown completion proves owned work ended, not just that accept stopped.
    assert_eq!(host.pending.load(Ordering::SeqCst), 0);
    assert_eq!(host.terminals.load(Ordering::SeqCst), 0);
    tokio::task::spawn_blocking(move || {
        for mut stream in streams {
            assert!(matches!(stream.read(&mut [0]), Ok(0)));
        }
        assert!(newport_platform::ipc::Stream::connect(endpoint).is_err());
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn aborting_listener_does_not_detach_terminal_lease_tasks() {
    let fixture = Fixture::start();
    let path = fixture.endpoint.clone();
    let stream = tokio::task::spawn_blocking(move || {
        let mut stream = connection(path);
        request(
            &mut stream,
            protocol::TERMINAL_OPEN_LEASE,
            json!({"id":Uuid::new_v4(),"session":Uuid::new_v4(),"cols":80,"rows":24}),
        );
        assert!(response(&mut stream).error.is_none());
        stream
    })
    .await
    .unwrap();
    let active = fixture.host.terminals.clone();
    fixture.stop().await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while active.load(Ordering::SeqCst) != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(stream);
}

#[tokio::test]
async fn integration_revocation_has_bounded_admission_separate_from_slow_commands() {
    for ordinary_saturated in [false, true] {
        let fixture = Fixture::start();
        let path = fixture.endpoint.clone();
        let streams = tokio::task::spawn_blocking(move || {
            let mut streams = Vec::new();
            if ordinary_saturated {
                for _ in 0..32 {
                    let mut stream = connection(path.clone());
                    request(&mut stream, "fixture.pending", json!({}));
                    streams.push(stream);
                }
            }
            streams
        })
        .await
        .unwrap();
        if ordinary_saturated {
            tokio::time::timeout(Duration::from_secs(3), async {
                while fixture.host.pending.load(Ordering::SeqCst) < 32 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert!(fixture.call("fixture.echo", json!({})).await.is_err());
            assert!(
                fixture
                    .call(
                        "set_integration_enabled",
                        json!({"feature":"browser","enabled":true})
                    )
                    .await
                    .is_err()
            );
            assert!(
                fixture
                    .call(
                        "set_integration_enabled",
                        json!({"feature":"unknown","enabled":false})
                    )
                    .await
                    .is_err()
            );
        }
        for (method, feature) in [
            ("set_integration_enabled", "browser"),
            ("set_clipboard_enabled", "clipboard"),
        ] {
            assert_eq!(
                fixture
                    .call(method, json!({"feature":feature,"enabled":false}))
                    .await
                    .unwrap()["enabled"],
                false
            );
        }
        let path = fixture.endpoint.clone();
        let reserved = tokio::task::spawn_blocking(move || {
            (0..4)
                .map(|_| {
                    let mut stream = connection(path.clone());
                    request(
                        &mut stream,
                        "set_integration_enabled",
                        json!({"feature":"clipboard","enabled":false,"hold":true}),
                    );
                    stream
                })
                .collect::<Vec<_>>()
        })
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            let target = if ordinary_saturated { 36 } else { 4 };
            while fixture.host.pending.load(Ordering::SeqCst) < target {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            fixture
                .call(
                    "set_integration_enabled",
                    json!({"feature":"browser","enabled":false})
                )
                .await
                .is_err()
        );
        if !ordinary_saturated {
            assert_eq!(
                fixture
                    .call("fixture.echo", json!({"ordinary":"available"}))
                    .await
                    .unwrap()["ordinary"],
                "available"
            );
        }
        let pending = fixture.host.pending.clone();
        fixture.stop().await;
        tokio::time::timeout(Duration::from_secs(3), async {
            while pending.load(Ordering::SeqCst) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        drop(streams);
        drop(reserved);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_response_returns_an_error_without_closing_before_the_header() {
    let fixture = Fixture::start();
    let endpoint = fixture.endpoint.clone();
    let error = tokio::task::spawn_blocking(move || {
        newport_client::backend::Backend::at_path(endpoint)
            .call_detailed("fixture.large", json!({}))
            .unwrap_err()
    })
    .await
    .unwrap();
    assert_eq!(error.code.as_deref(), Some("RESPONSE_TOO_LARGE"));
    assert!(error.message.contains("too large"));
    assert!(!error.message.contains("interrupted"));
    assert_eq!(fixture.call("fixture.echo", json!(42)).await.unwrap(), 42);
    assert_eq!(fixture.host.dispatched.load(Ordering::SeqCst), 2);
    fixture.stop().await;
}

#[tokio::test]
async fn only_file_saves_accept_large_native_payloads() {
    let fixture = Fixture::start();
    let content = "x".repeat(12 * 1024 * 1024);
    let result = fixture
        .call("files_write_text", json!({"content":content}))
        .await
        .unwrap();
    assert_eq!(result["bytes"], 12 * 1024 * 1024);
    let result = fixture
        .call(
            "fixture.echo",
            json!({"content":"x".repeat(protocol::DEFAULT_MAX_REQUEST)}),
        )
        .await;
    assert!(result.unwrap_err().contains("too large"));
    // A client bypassing its local guard is also rejected before dispatch.
    let endpoint = fixture.endpoint.clone();
    let before = fixture.host.dispatched.load(Ordering::SeqCst);
    tokio::task::spawn_blocking(move || {
        let mut stream = connection(endpoint);
        request(
            &mut stream,
            "fixture.echo",
            json!({"content":"x".repeat(protocol::DEFAULT_MAX_REQUEST)}),
        );
        let mut byte = [0];
        assert!(!matches!(stream.read(&mut byte), Ok(n) if n > 0));
    })
    .await
    .unwrap();
    assert_eq!(fixture.host.dispatched.load(Ordering::SeqCst), before);
    fixture.stop().await;
}
