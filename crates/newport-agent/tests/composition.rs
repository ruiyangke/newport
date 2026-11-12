#[path = "support/runtime_reply.rs"]
mod runtime_reply;
use newport_protocol::runtime::{self as wire, wire as codec};
use runtime_reply::Expected;
#[path = "fixtures/service_io.rs"]
mod service_io;
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
        std::fs::write(source.path().join("src/service_io.rs"), include_str!("fixtures/service_io.rs")).unwrap();
        std::fs::write(source.path().join("src/main.rs"), include_str!("fixtures/composed.rs")).unwrap();
        std::fs::write(source.path().join("Cargo.toml"), format!(
            "[package]\nname = 'composition-consumer'\nversion = '0.0.0'\nedition = '2021'\n[workspace]\n[dependencies]\nnewport-agent = {{ path = {} }}\nnewport-protocol = {{ path = {} }}\nserde = '1'\nserde_json = '1'\n[profile.dev.package.sha2]\nopt-level = 3\n",
            serde_json::to_string(agent.to_str().unwrap()).unwrap(),
            serde_json::to_string(agent.join("../../crates/newport-protocol").to_str().unwrap()).unwrap(),
        )).unwrap();
        // Keep dependency selection identical to core; only the temporary root
        // package entry changes. Offline proves this consumer needs no private repo.
        // Its own workspace doesn't inherit the agent's profile, which hashes
        // fast enough for Needle's self-checksum on a busy runner.
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
    use newport_protocol::product::{Identity, Variant};
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
    for (exe, identity) in [(core, &core_id), (full.as_path(), &full_id)] {
        assert_eq!(identity.version.as_deref(), Some(newport_agent::VERSION));
        let output = run(exe, &["--version"]);
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("newport-agent {}\n", newport_agent::VERSION)
        );
    }
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
    let helpers = install(
        &core_path,
        &core_id,
        &[
            "--keep-default",
            "--integrations",
            "--expect-current",
            &full_id.build_sha256,
            "--expect-variant",
            "full",
        ],
    );
    assert!(
        helpers.status.success(),
        "{}",
        String::from_utf8_lossy(&helpers.stderr)
    );
    let outcome: serde_json::Value = serde_json::from_slice(&helpers.stdout).unwrap();
    assert_eq!(outcome["build"], core_id.build_sha256);
    assert_eq!(outcome["active"], false);
    assert_eq!(std::fs::read_link(&active).unwrap(), full_path);
    for helper in ["newport-browser", "xdg-open"] {
        assert_eq!(
            std::fs::read_link(home.path().join(".local/bin").join(helper)).unwrap(),
            core_path
        );
    }
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

#[test]
fn installing_replaces_a_full_build_this_build_cannot_use() {
    use newport_protocol::product::{CORE_SERVICES, Identity};
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let full = binary().with_file_name("full-product");
    let home = tempfile::tempdir().unwrap();
    let run = |exe: &Path, args: &[&str]| {
        Command::new(exe)
            .args(args)
            .env("HOME", home.path())
            .output()
            .unwrap()
    };
    let full_id: Identity =
        serde_json::from_slice(&run(&full, &["product", "--json"]).stdout).unwrap();
    assert!(
        run(&full, &["deployment", "prepare", &full_id.build_sha256])
            .status
            .success()
    );
    let builds = home.path().join(".local/share/newport/needle/builds");
    let full_path = builds.join(&full_id.build_sha256).join("newport-agent");
    // A full build speaking a Remote Apps version this build doesn't,
    // installed and active. It reports its own checksum, as a build does.
    let mut services: serde_json::Map<String, serde_json::Value> = CORE_SERVICES
        .iter()
        .map(|name| (name.to_string(), serde_json::json!([1])))
        .collect();
    services.insert("remote-apps".into(), serde_json::json!([2]));
    let product = serde_json::json!({
        "variant": "full",
        "controlVersions": [1],
        "services": services,
    });
    let older = format!(
        "#!/bin/sh\nset -e\ntest \"$*\" = 'product --json'\n\
         h=$( (sha256sum \"$0\" 2>/dev/null || shasum -a 256 \"$0\") | cut -d' ' -f1)\n\
         printf '{{\"schemaVersion\":1,\"buildSha256\":\"%s\",\"product\":%s}}\\n' \"$h\" '{product}'\n"
    );
    let older_hash: String = Sha256::digest(older.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let older_path = builds.join(&older_hash).join("newport-agent");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(older_path.parent().unwrap())
        .unwrap();
    std::fs::write(&older_path, &older).unwrap();
    std::fs::set_permissions(&older_path, std::fs::Permissions::from_mode(0o500)).unwrap();
    let active = home.path().join(".local/bin/newport-agent");
    std::os::unix::fs::symlink(&older_path, &active).unwrap();
    let reported: Identity =
        serde_json::from_slice(&run(&active, &["product", "--json"]).stdout).unwrap();
    assert_eq!(reported.build_sha256, older_hash);
    assert!(reported.validate().is_err(), "this build can't use it");
    // The update Newport sends for it.
    let installed = run(
        &full_path,
        &[
            "deployment",
            "install",
            &full_id.build_sha256,
            "--expect-current",
            &older_hash,
            "--expect-variant",
            "full",
        ],
    );
    assert!(
        installed.status.success(),
        "{}",
        String::from_utf8_lossy(&installed.stderr)
    );
    assert_eq!(std::fs::read_link(&active).unwrap(), full_path);
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
            while let Ok(value) = service_io::read(&mut output) {
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
    frames: mpsc::Receiver<wire::Reply>,
    sequence: u64,
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
        let mut output = process.0.stdout.take().unwrap();
        let (announced, announcement) = mpsc::sync_channel(1);
        let (send, frames) = mpsc::sync_channel(8);
        std::thread::spawn(move || {
            let hello: wire::Hello = codec::read(&mut output).unwrap();
            announced.send(hello).unwrap();
            while let Ok(reply) = codec::read::<wire::Reply>(&mut output) {
                if send.send(reply).is_err() {
                    break;
                }
            }
        });
        let hello = announcement.recv_timeout(REPLY).unwrap();
        hello.validate().unwrap();
        assert_eq!(hello.agent_version.as_deref(), Some(newport_agent::VERSION));
        let mut runtime = Self {
            process,
            input,
            frames,
            sequence: 0,
            hello,
            session: uuid::Uuid::new_v4().to_string(),
            home,
        };
        let reply = runtime.call(wire::Operation::Initialize {
            version: wire::VERSION,
            session_id: runtime.session.clone(),
            transport_generation: 1,
        });
        assert!(reply.result.is_ok());
        runtime
    }
    fn call(&mut self, operation: wire::Operation) -> wire::Reply {
        self.sequence += 1;
        let id = self.sequence;
        codec::write(
            self.input.as_mut().unwrap(),
            &wire::Request {
                id,
                runtime_id: self.hello.instance_id.clone(),
                operation,
            },
        )
        .unwrap();
        let reply = self.frames.recv_timeout(REPLY).unwrap();
        assert_eq!(reply.id, id);
        reply
    }
    fn register(&mut self) -> String {
        self.register_service("fixture", 7)
    }
    fn register_service(&mut self, service: &str, version: u32) -> String {
        let worker_id = uuid::Uuid::new_v4().to_string();
        assert!(
            self.call(wire::Operation::PrepareWorkerStream {
                worker_id: worker_id.clone(),
                service: service.into(),
                contract_id: wire::service_contract_id(service).map(str::to_owned),
                version,
            })
            .result
            .is_ok()
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
    assert!(runtime.hello.services.contains_key("integration"));
    assert!(
        runtime.hello.services["missing-fixture"]
            .unavailable
            .is_some()
    );
    for (service, version) in [("missing-fixture", 7), ("fixture", 8), ("unknown", 7)] {
        assert!(
            runtime
                .call(wire::Operation::PrepareWorkerStream {
                    worker_id: uuid::Uuid::new_v4().to_string(),
                    service: service.into(),
                    contract_id: wire::service_contract_id(service).map(str::to_owned),
                    version
                })
                .result
                .is_err()
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
        assert!(runtime.call(wire::Operation::Ping).result.is_ok());
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
    assert!(second.call(wire::Operation::Ping).result.is_ok());
}

#[test]
fn composed_worker_and_command_helper_share_runtime_clipboard_ownership() {
    use newport_protocol::{
        clipboard::imports,
        integration::{
            self as control, Call, ClientMessage, Request, ServerMessage, wire as integration,
        },
    };
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
    let event = || events.recv_timeout(REPLY).unwrap();
    let ready = Call {
        id: 1,
        request: Request::Heartbeat,
    };
    integration::write(
        host.0.stdin.as_mut().unwrap(),
        &ClientMessage::Call(ready.clone()),
    )
    .unwrap();
    let ServerMessage::Response(response) = event() else {
        panic!("expected heartbeat response")
    };
    response.validate_for(&ready).unwrap();
    let viewer = runtime.register();
    let mut empty = runtime.worker(&viewer, &runtime.hello.build_sha256, &["clipboard-offer"]);
    let frames = empty.frames();
    assert_eq!(next(&frames), runtime.identity(&viewer));
    assert_eq!(next(&frames)["formats"], serde_json::json!([]));
    assert!(empty.exit().success());

    let applied = runtime.call(wire::Operation::Permissions {
        worker_id: host_id.clone(),
        permissions: newport_protocol::runtime::Permissions {
            revision: 1,
            clipboard: true,
            browser: false,
        },
    });
    assert!(
        applied.result.is_ok(),
        "{:?}",
        applied.result.as_ref().err()
    );
    assert!(applied.result.unwrap().permissions().permissions.clipboard);
    integration::write(
        host.0.stdin.as_mut().unwrap(),
        &ClientMessage::Call(Call {
            id: 2,
            request: Request::ClipboardOffer(imports::Offer {
                revision: 17,
                formats: vec!["image/png".into()],
            }),
        }),
    )
    .unwrap();
    assert!(matches!(event(), ServerMessage::Response(_)));
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
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let ServerMessage::ClipboardRequest(request) = event() else {
        panic!("expected clipboard request")
    };
    let (source_revision, format) = (request.revision, request.format.clone());
    assert_eq!((source_revision, format.as_str()), (17, "image/png"));
    let image = include_bytes!("pixel.png");
    let reply = imports::Reply::new(&request, imports::Encoding::Raw, true, image.as_slice());
    integration::write(
        host.0.stdin.as_mut().unwrap(),
        &ClientMessage::Clipboard(reply),
    )
    .unwrap();
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
    assert!(runtime.call(wire::Operation::Ping).result.is_ok());
}

#[test]
fn runtime_control_assigns_and_retires_a_native_provider_without_enabling_exports() {
    use newport_agent::clipboard_service::Route;
    let mut runtime = Runtime::new();
    runtime.hello.require("clipboard-service", 1).unwrap();
    let product: newport_protocol::product::Identity = serde_json::from_slice(
        &Command::new(binary())
            .args(["product", "--json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let advertised = newport_protocol::product::Product {
        variant: runtime.hello.product,
        control_versions: runtime.hello.versions.clone(),
        services: runtime
            .hello
            .services
            .iter()
            .map(|(name, capability)| (name.clone(), capability.versions.clone()))
            .collect(),
    };
    advertised.validate().unwrap();
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
            .call(wire::Operation::ClipboardRoute(route.clone()))
            .result
            .is_ok()
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
            .call(wire::Operation::ClipboardRoute(conflict))
            .result
            .is_err()
    );
    assert!(
        runtime
            .call(wire::Operation::ClipboardRoute(Route {
                revision: 2,
                worker_id: None,
                exports: false,
                focused: false
            }))
            .result
            .is_ok()
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
    assert!(runtime.call(wire::Operation::Ping).result.is_ok());
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
fn provider_retirement_is_reported_and_keeps_the_runtime_running() {
    use newport_protocol::clipboard::{Route, RouteOutcome};
    let mut runtime = Runtime::new();
    runtime.hello.require("clipboard-service", 1).unwrap();
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
    let ready = runtime.call(wire::Operation::ClipboardRoute(route.clone()));
    assert!(ready.result.is_ok());
    assert_eq!(ready.result.unwrap().route().outcome, RouteOutcome::Applied);
    assert_eq!(next(&frames)["origin"]["worker"], worker);
    native.0.kill().unwrap();
    let _ = native.exit();
    // Worker EOF is consumed independently from runtime control. Bound this
    // observation without presuming either thread gets scheduled first.
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        route.revision += 1;
        let reply = runtime.call(wire::Operation::ClipboardRoute(route.clone()));
        assert!(reply.result.is_ok());
        let reply = reply.result.unwrap().route();
        assert_eq!(reply.revision, route.revision);
        assert_eq!(reply.worker_id.as_deref(), Some(worker.as_str()));
        if reply.outcome == RouteOutcome::Retired {
            break;
        }
        assert!(Instant::now() < deadline, "worker EOF not retired");
        std::thread::sleep(Duration::from_millis(2));
    }
    route.worker_id = Some(uuid::Uuid::new_v4().to_string());
    let unknown = runtime.call(wire::Operation::ClipboardRoute(route));
    assert!(unknown.result.is_err());
    assert!(unknown.result.is_err());
    assert!(runtime.call(wire::Operation::Ping).result.is_ok());
}
