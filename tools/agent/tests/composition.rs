use newport_agent::runtime_protocol as wire;
use std::{
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{OnceLock, mpsc},
    time::{Duration, Instant},
};

/// Newport waits this long for Needle's startup and replies; a slow runner
/// can exceed a tighter limit without anything being wrong.
const REPLY: Duration = Duration::from_secs(25);

fn binary() -> &'static Path {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY.get_or_init(|| {
        let source = tempfile::tempdir().unwrap();
        let agent = Path::new(env!("CARGO_MANIFEST_DIR"));
        let target = agent.join("target/composition-consumer");
        std::fs::create_dir(source.path().join("src")).unwrap();
        std::fs::create_dir(source.path().join("src/bin")).unwrap();
        std::fs::write(source.path().join("src/bin/full-product.rs"), include_str!("fixtures/full_product.rs")).unwrap();
        std::fs::write(source.path().join("src/main.rs"), include_str!("fixtures/composed.rs")).unwrap();
        std::fs::write(source.path().join("Cargo.toml"), format!(
            "[package]\nname = 'composition-consumer'\nversion = '0.0.0'\nedition = '2021'\n[workspace]\n[dependencies]\nnewport-agent = {{ path = {} }}\n",
            serde_json::to_string(agent.to_str().unwrap()).unwrap(),
        )).unwrap();
        // Keep dependency selection identical to core; only the temporary root
        // package entry changes. Offline proves this consumer needs no private repo.
        std::fs::copy(agent.join("Cargo.lock"), source.path().join("Cargo.lock")).unwrap();
        let output = Command::new(env!("CARGO"))
            .args(["build", "--bins", "--offline", "--quiet", "--manifest-path"])
            .arg(source.path().join("Cargo.toml"))
            .env("CARGO_TARGET_DIR", &target).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        target.join("debug/composition-consumer")
    })
}

#[test]
fn product_identity_fences_activation_and_requires_explicit_downgrade() {
    use newport_agent::product::{Identity, Variant};
    use std::os::unix::fs::PermissionsExt;
    let core = binary();
    let full = core.with_file_name("full-product");
    let home = tempfile::tempdir().unwrap();
    let run = |exe: &Path, args: &[&str]| {
        Command::new(exe)
            .args(args)
            .env("HOME", home.path())
            .output()
            .unwrap()
    };
    let identity = |exe: &Path| -> Identity {
        let output = run(exe, &["product", "--json"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let identity: Identity = serde_json::from_slice(&output.stdout).unwrap();
        identity.validate().unwrap();
        identity
    };
    let core_id = identity(core);
    let full_id = identity(&full);
    assert_eq!(core_id.product.variant, Variant::Core);
    assert_eq!(full_id.product.variant, Variant::Full);
    let stage = |exe: &Path, id: &Identity| {
        assert!(
            run(exe, &["deployment", "prepare", &id.build_sha256])
                .status
                .success()
        );
        home.path()
            .join(".local/share/newport/needle/builds")
            .join(&id.build_sha256)
            .join("newport-agent")
    };
    let core_path = stage(core, &core_id);
    let full_path = stage(&full, &full_id);
    let active = home.path().join(".local/bin/newport-agent");
    let install = |exe: &Path, id: &Identity, extra: &[&str]| {
        let mut args = vec!["deployment", "install", &id.build_sha256];
        args.extend_from_slice(extra);
        run(exe, &args)
    };
    assert!(
        install(&core_path, &core_id, &["--expect-current", "missing"])
            .status
            .success()
    );
    assert!(
        install(
            &full_path,
            &full_id,
            &[
                "--expect-current",
                &core_id.build_sha256,
                "--expect-variant",
                "core"
            ]
        )
        .status
        .success()
    );
    assert_eq!(std::fs::read_link(&active).unwrap(), full_path);
    let downgrade = install(&core_path, &core_id, &[]);
    assert!(!downgrade.status.success());
    assert!(
        String::from_utf8_lossy(&downgrade.stderr).contains("--allow-downgrade"),
        "unexpected downgrade error: {}",
        String::from_utf8_lossy(&downgrade.stderr)
    );
    assert_eq!(std::fs::read_link(&active).unwrap(), full_path);
    let stale = install(
        &core_path,
        &core_id,
        &[
            "--allow-downgrade",
            "--expect-current",
            &core_id.build_sha256,
        ],
    );
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("changed during deployment"));
    let mismatch = install(
        &core_path,
        &core_id,
        &[
            "--allow-downgrade",
            "--expect-current",
            &full_id.build_sha256,
            "--expect-variant",
            "core",
        ],
    );
    assert!(!mismatch.status.success());
    assert!(String::from_utf8_lossy(&mismatch.stderr).contains("expected variant"));
    assert!(
        install(
            &core_path,
            &core_id,
            &[
                "--allow-downgrade",
                "--expect-current",
                &full_id.build_sha256,
                "--expect-variant",
                "full"
            ]
        )
        .status
        .success()
    );
    assert_eq!(std::fs::read_link(&active).unwrap(), core_path);
    // A malformed response must not be mistaken for legacy core, even when
    // a caller supplies a previous variant and explicitly requests downgrade.
    std::fs::remove_file(&active).unwrap();
    std::fs::write(&active, "#!/bin/sh\n# newport-agent/1\nprintf '{}\\n'\n").unwrap();
    std::fs::set_permissions(&active, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        !install(&core_path, &core_id, &["--allow-downgrade"])
            .status
            .success()
    );
    assert!(!std::fs::symlink_metadata(&active).unwrap().is_symlink());
}

struct Process(Child);
impl Process {
    fn exit(&mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "process did not stop");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn frames(&mut self) -> mpsc::Receiver<serde_json::Value> {
        let mut output = self.0.stdout.take().unwrap();
        let (send, frames) = mpsc::sync_channel(8);
        std::thread::spawn(move || {
            while let Ok(value) = wire::read(&mut output) {
                if send.send(value).is_err() {
                    break;
                }
            }
        });
        frames
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn next(frames: &mpsc::Receiver<serde_json::Value>) -> serde_json::Value {
    frames
        .recv_timeout(Duration::from_secs(10))
        .expect("protocol progress")
}
struct Runtime {
    process: Process,
    input: Option<ChildStdin>,
    frames: mpsc::Receiver<serde_json::Value>,
    hello: wire::Hello,
    session: String,
    home: tempfile::TempDir,
}
impl Runtime {
    fn new() -> Self {
        let executable = binary();
        let home = tempfile::Builder::new()
            .prefix("np-composition-")
            .tempdir_in("/tmp")
            .unwrap();
        let mut process = Process(
            Command::new(executable)
                .args(["runtime", "--stdio"])
                .env("HOME", home.path())
                .env("XDG_RUNTIME_DIR", home.path())
                .env("PATH", "")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        let input = process.0.stdin.take();
        let frames = process.frames();
        let hello: wire::Hello = serde_json::from_value(next(&frames)).unwrap();
        hello.validate().unwrap();
        let mut runtime = Self {
            process,
            input,
            frames,
            hello,
            session: uuid::Uuid::new_v4().to_string(),
            home,
        };
        let reply = runtime.call(wire::Operation::Initialize {
            version: wire::VERSION,
            session_id: runtime.session.clone(),
            transport_generation: 1,
        });
        assert!(reply.error.is_none());
        runtime
    }
    fn call(&mut self, operation: wire::Operation) -> wire::Reply {
        let id = uuid::Uuid::new_v4().to_string();
        wire::write(
            self.input.as_mut().unwrap(),
            &wire::Request {
                id: id.clone(),
                runtime_id: self.hello.instance_id.clone(),
                operation,
            },
        )
        .unwrap();
        let reply: wire::Reply = serde_json::from_value(next(&self.frames)).unwrap();
        assert_eq!(reply.id, id);
        reply
    }
    fn register(&mut self) -> String {
        self.register_service("fixture", 7)
    }
    fn register_service(&mut self, service: &str, version: u32) -> String {
        let worker_id = uuid::Uuid::new_v4().to_string();
        assert!(
            self.call(wire::Operation::Register {
                worker_id: worker_id.clone(),
                service: service.into(),
                version,
            })
            .error
            .is_none()
        );
        worker_id
    }
    fn worker(&self, worker: &str, build: &str, args: &[&str]) -> Process {
        self.worker_service(worker, build, "fixture", args)
    }
    fn worker_service(&self, worker: &str, build: &str, service: &str, args: &[&str]) -> Process {
        Process(
            Command::new(binary())
                .args([
                    "worker",
                    build,
                    &self.hello.instance_id,
                    &self.hello.worker_socket,
                    worker,
                    service,
                ])
                .args(args)
                .env("HOME", self.home.path())
                .env("XDG_RUNTIME_DIR", self.home.path())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }
    fn identity(&self, worker: &str) -> serde_json::Value {
        serde_json::json!([
            self.session,
            self.hello.instance_id,
            worker,
            self.hello.build_sha256
        ])
    }
}

#[test]
fn service_admission_checks_availability_build_version_and_one_time_registration() {
    let mut runtime = Runtime::new();
    assert!(runtime.hello.capabilities.contains_key("integration"));
    assert!(!runtime.hello.capabilities["missing-fixture"].available);
    for (service, version) in [("missing-fixture", 7), ("fixture", 8), ("unknown", 7)] {
        assert!(
            runtime
                .call(wire::Operation::Register {
                    worker_id: uuid::Uuid::new_v4().to_string(),
                    service: service.into(),
                    version
                })
                .error
                .is_some()
        );
    }
    let ticket = runtime.register();
    assert!(
        !runtime
            .worker(&ticket, &"0".repeat(64), &[])
            .exit()
            .success()
    );
    let mut worker = runtime.worker(
        &ticket,
        &runtime.hello.build_sha256,
        &["hello world", "'literal'", "$(false)"],
    );
    let frames = worker.frames();
    assert_eq!(next(&frames), runtime.identity(&ticket));
    assert_eq!(
        next(&frames),
        serde_json::json!(["hello world", "'literal'", "$(false)"])
    );
    assert!(worker.exit().success());
    assert!(
        !runtime
            .worker(&ticket, &runtime.hello.build_sha256, &[])
            .exit()
            .success()
    );
}

#[test]
fn helper_reexecution_and_worker_crashes_leave_the_control_runtime_usable() {
    let mut runtime = Runtime::new();
    for (mode, code) in [("helper", 0), ("crash", 23)] {
        let ticket = runtime.register();
        let mut worker = runtime.worker(&ticket, &runtime.hello.build_sha256, &[mode]);
        let frames = worker.frames();
        assert_eq!(next(&frames), runtime.identity(&ticket));
        if mode == "helper" {
            assert_eq!(next(&frames), "helper\n");
        }
        assert_eq!(worker.exit().code(), Some(code));
        assert!(runtime.call(wire::Operation::Ping).error.is_none());
    }
}

#[test]
fn control_disconnection_revokes_only_its_registered_workers() {
    let mut first = Runtime::new();
    let mut second = Runtime::new();
    let ticket = first.register();
    assert!(
        !second
            .worker(&ticket, &second.hello.build_sha256, &[])
            .exit()
            .success()
    );
    let mut worker = first.worker(&ticket, &first.hello.build_sha256, &["hold"]);
    let frames = worker.frames();
    assert_eq!(next(&frames), first.identity(&ticket));
    // Worker stdin remains open. Only losing the control lease cancels it.
    first.input.take();
    assert!(first.process.exit().success());
    assert_eq!(next(&frames), "cancelled");
    assert!(worker.exit().success());
    assert!(second.call(wire::Operation::Ping).error.is_none());
}

#[test]
fn composed_worker_and_command_helper_share_runtime_clipboard_ownership() {
    use newport_agent::{integration_control as control, wire as integration};
    let mut runtime = Runtime::new();
    let host_id = runtime.register_service("integration", control::VERSION);
    let mut host = runtime.worker_service(
        &host_id,
        &runtime.hello.build_sha256,
        "integration",
        &["clipboard-ipc"],
    );
    let mut output = host.0.stdout.take().unwrap();
    let (send, events) = mpsc::sync_channel(16);
    std::thread::spawn(move || {
        while let Ok(frame) = integration::read(&mut output) {
            if send.send(frame).is_err() {
                break;
            }
        }
    });
    let event = |kind: u8| {
        let frame = events.recv_timeout(REPLY).unwrap();
        assert_eq!(frame.0, kind, "{}", String::from_utf8_lossy(&frame.1));
        frame.1
    };
    control::decode::<control::Hello>(&event(b'R'))
        .unwrap()
        .validate()
        .unwrap();
    let viewer = runtime.register();
    let mut empty = runtime.worker(&viewer, &runtime.hello.build_sha256, &["clipboard-offer"]);
    let frames = empty.frames();
    assert_eq!(next(&frames), runtime.identity(&viewer));
    assert_eq!(next(&frames)["formats"], serde_json::json!([]));
    assert!(empty.exit().success());

    let applied = runtime.call(wire::Operation::Permissions {
        worker_id: host_id.clone(),
        permissions: control::Permissions {
            revision: 1,
            clipboard: true,
            browser: false,
        },
    });
    assert!(applied.error.is_none(), "{:?}", applied.error);
    assert!(applied.applied.unwrap().permissions.clipboard);
    integration::write(
        host.0.stdin.as_mut().unwrap(),
        b'M',
        &rmp_serde::to_vec(&(17i64, vec!["image/png"])).unwrap(),
    )
    .unwrap();
    event(b'A');
    assert!(
        events.try_recv().is_err(),
        "metadata fetched a host payload"
    );

    let viewer = runtime.register();
    let mut display = runtime.worker(
        &viewer,
        &runtime.hello.build_sha256,
        &["clipboard", "image/png"],
    );
    let frames = display.frames();
    assert_eq!(next(&frames), runtime.identity(&viewer));
    let offer = next(&frames);
    assert_eq!(offer["runtime"], runtime.hello.instance_id);
    assert_eq!(offer["formats"], serde_json::json!(["image/png"]));
    let mut helper = Process(
        Command::new(binary())
            .args(["clipboard", "-o", "-t", "image/png"])
            .env("HOME", runtime.home.path())
            .env("XDG_RUNTIME_DIR", runtime.home.path())
            .env("NEWPORT_SESSION", &runtime.session)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let (request, source_revision, format): (u64, i64, String) =
        rmp_serde::from_slice(&event(b'C')).unwrap();
    assert_eq!((source_revision, format.as_str()), (17, "image/png"));
    let image = include_bytes!("pixel.png");
    let reply = rmp_serde::to_vec(&(
        request,
        source_revision,
        0u8,
        true,
        serde_bytes::Bytes::new(image),
    ))
    .unwrap();
    integration::write(host.0.stdin.as_mut().unwrap(), b'D', &reply).unwrap();
    assert_eq!(next(&frames), serde_json::json!(image.as_slice()));
    assert!(display.exit().success());
    assert!(helper.exit().success());
    let mut copied = vec![];
    std::io::Read::read_to_end(&mut helper.0.stdout.take().unwrap(), &mut copied).unwrap();
    assert_eq!(copied, image);
    assert!(
        events.recv_timeout(Duration::from_millis(80)).is_err(),
        "helper opened another host fetch"
    );

    let watcher = runtime.register();
    let mut watching = runtime.worker(
        &watcher,
        &runtime.hello.build_sha256,
        &["clipboard-watch-empty"],
    );
    let frames = watching.frames();
    assert_eq!(next(&frames), runtime.identity(&watcher));
    assert_eq!(next(&frames)["formats"], serde_json::json!(["image/png"]));
    host.0.kill().unwrap();
    let _ = host.exit();
    assert_eq!(next(&frames)["formats"], serde_json::json!([]));
    assert!(watching.exit().success());
    assert!(runtime.call(wire::Operation::Ping).error.is_none());
}

#[test]
fn runtime_control_assigns_and_retires_a_native_provider_without_enabling_exports() {
    use newport_agent::clipboard_service::Route;
    let mut runtime = Runtime::new();
    runtime.hello.require("clipboard-service", 1).unwrap();
    let product: newport_agent::product::Identity = serde_json::from_slice(
        &Command::new(binary())
            .args(["product", "--json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let advertised = newport_agent::product::Product::from_capabilities(
        &runtime.hello.versions,
        runtime.hello.capabilities.iter().map(|(name, capability)| {
            (
                name.as_str(),
                capability.versions.as_slice(),
                capability.available,
            )
        }),
    )
    .unwrap();
    assert_eq!(
        advertised, product.product,
        "runtime and packaged product protocols disagree"
    );
    let native_id = runtime.register();
    let mut native = runtime.worker(
        &native_id,
        &runtime.hello.build_sha256,
        &["native-clipboard"],
    );
    let frames = native.frames();
    assert_eq!(next(&frames), runtime.identity(&native_id));
    assert_eq!(next(&frames), false);
    let route = Route {
        revision: 1,
        worker_id: Some(native_id.clone()),
        exports: false,
        focused: false,
    };
    assert!(
        runtime
            .call(wire::Operation::ClipboardRoute {
                route: route.clone()
            })
            .error
            .is_none()
    );
    let offer = next(&frames);
    assert_eq!(offer["origin"]["kind"], "native");
    assert_eq!(offer["origin"]["worker"], native_id);
    let reader_id = runtime.register();
    let mut reader = runtime.worker(
        &reader_id,
        &runtime.hello.build_sha256,
        &["clipboard", "text/plain"],
    );
    let reads = reader.frames();
    assert_eq!(next(&reads), runtime.identity(&reader_id));
    assert_eq!(next(&reads), offer);
    assert_eq!(
        next(&reads),
        serde_json::json!(b"native through runtime".as_slice())
    );
    assert!(reader.exit().success());
    let mut conflict = route;
    conflict.exports = true;
    conflict.focused = true;
    assert!(
        runtime
            .call(wire::Operation::ClipboardRoute { route: conflict })
            .error
            .is_some()
    );
    assert!(
        runtime
            .call(wire::Operation::ClipboardRoute {
                route: Route {
                    revision: 2,
                    worker_id: None,
                    exports: false,
                    focused: false
                }
            })
            .error
            .is_none()
    );
    let reader_id = runtime.register();
    let mut reader = runtime.worker(
        &reader_id,
        &runtime.hello.build_sha256,
        &["clipboard-offer"],
    );
    let reads = reader.frames();
    assert_eq!(next(&reads), runtime.identity(&reader_id));
    let offer = next(&reads);
    assert_eq!(offer["origin"]["kind"], "empty");
    assert_eq!(offer["formats"], serde_json::json!([]));
    assert!(reader.exit().success());
    assert!(runtime.call(wire::Operation::Ping).error.is_none());
    assert!(
        native.0.try_wait().unwrap().is_none(),
        "retiring the route terminated the provider worker"
    );
    runtime.input.take();
    assert!(runtime.process.exit().success());
    assert_eq!(next(&frames), "cancelled");
    assert!(native.exit().success());
}

#[test]
fn typed_provider_retirement_preserves_runtime_and_legacy_control_contract() {
    use wire::clipboard::{Route, RouteOutcome};
    let mut runtime = Runtime::new();
    runtime.hello.require("clipboard-service", 2).unwrap();
    let worker = runtime.register();
    let mut native = runtime.worker(&worker, &runtime.hello.build_sha256, &["native-clipboard"]);
    let frames = native.frames();
    assert_eq!(next(&frames), runtime.identity(&worker));
    assert_eq!(next(&frames), false);
    let mut route = Route {
        revision: 1,
        worker_id: Some(worker.clone()),
        exports: false,
        focused: false,
    };
    let ready = runtime.call(wire::Operation::ClipboardRouteV2 {
        route: route.clone(),
    });
    assert!(ready.error.is_none());
    assert_eq!(
        ready.clipboard_route.unwrap().outcome,
        RouteOutcome::Applied
    );
    assert_eq!(next(&frames)["origin"]["worker"], worker);
    native.0.kill().unwrap();
    let _ = native.exit();
    // Worker EOF is consumed independently from runtime control. Bound this
    // observation without presuming either thread gets scheduled first.
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        route.revision += 1;
        let reply = runtime.call(wire::Operation::ClipboardRouteV2 {
            route: route.clone(),
        });
        assert!(reply.error.is_none());
        let reply = reply.clipboard_route.unwrap();
        assert_eq!(reply.revision, route.revision);
        assert_eq!(reply.worker_id.as_deref(), Some(worker.as_str()));
        if reply.outcome == RouteOutcome::Retired {
            break;
        }
        assert!(Instant::now() < deadline, "worker EOF not retired");
        std::thread::sleep(Duration::from_millis(2));
    }
    let legacy_id = uuid::Uuid::new_v4().to_string();
    wire::write(
        runtime.input.as_mut().unwrap(),
        &wire::Request {
            id: legacy_id.clone(),
            runtime_id: runtime.hello.instance_id.clone(),
            operation: wire::Operation::ClipboardRoute {
                route: route.clone(),
            },
        },
    )
    .unwrap();
    let legacy = next(&runtime.frames);
    assert_eq!(legacy["id"], legacy_id);
    assert!(
        legacy.get("clipboardRoute").is_none(),
        "v2 field leaked to strict v1 reader"
    );
    assert!(legacy.get("workerStream").is_none());
    assert_eq!(
        legacy["error"],
        "Native clipboard provider must be an admitted non-integration worker"
    );
    route.worker_id = Some(uuid::Uuid::new_v4().to_string());
    let unknown = runtime.call(wire::Operation::ClipboardRouteV2 { route });
    assert!(unknown.error.is_some());
    assert!(unknown.clipboard_route.is_none());
    assert!(runtime.call(wire::Operation::Ping).error.is_none());
}

#[test]
fn a_starting_runtime_gives_open_shells_their_gui_launchers_back() {
    let home = tempfile::Builder::new()
        .prefix("np-launchers-")
        .tempdir_in("/tmp")
        .unwrap();
    let applications = home.path().join(".local/share/applications");
    std::fs::create_dir_all(&applications).unwrap();
    std::fs::write(
        applications.join("viewer.desktop"),
        "[Desktop Entry]\nType=Application\nName=Viewer\nExec=/bin/sleep %U\n",
    )
    .unwrap();
    let mut process = Process(
        Command::new(binary())
            .args(["runtime", "--stdio"])
            .env("HOME", home.path())
            .env("XDG_RUNTIME_DIR", home.path())
            .env("XDG_DATA_DIRS", home.path().join("none"))
            .env("PATH", "")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let frames = process.frames();
    next(&frames);
    let launcher = home.path().join(".cache/newport/gui-bin/sleep");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !launcher.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let source = std::fs::read_to_string(&launcher).expect("launcher restored");
    assert!(
        source.contains(" gui --program '/bin/sleep' -- "),
        "{source}"
    );
}
