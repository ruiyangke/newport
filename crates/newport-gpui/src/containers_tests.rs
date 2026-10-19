use super::{ContainersPanel, Stopped, master_rows};

const EVERY: Stopped = Stopped {
    shown: true,
    open: true,
};
use gpui::{Context, IntoElement, ParentElement, Render, Styled, Window, div};
use gpui_kit as gpui;
use gpui_kit::base::test_support::{ElementSnapshot, snapshots};
use gpui_kit::{
    AppContext, Entity, KeyDownEvent, Keystroke, TestAppContext, VisualTestContext, px, size,
};
use newport_client::{
    backend::Backend,
    containers::{Action, Containers},
};
use newport_domain::containers::{
    ComposeLabels, Container, ContainerInspect, ContainerLogEntry, ContainerLogPage,
    ContainerState, ContainerVerb, ContainersSnapshot, DiskTotals, DockerDisk, DockerImage,
    DockerTotals, DockerVolume, EngineInfo, EnvVar, PruneTarget,
};
use newport_platform::ipc::{
    Listener, Stream,
    protocol::{Request, Response, VERSION},
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    io::{Read, Write},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

type FixtureResponse = Result<Value, String>;
type HeldDiskRead = (Stream, FixtureResponse);
type HeldOutputRead = (String, Stream, FixtureResponse);

/// Strict local RPC peer: never discovers the installed app or reaches a server.
pub(crate) struct FakeBackend {
    path: PathBuf,
    calls: Arc<Mutex<Vec<Request>>>,
    snapshot: Arc<Mutex<ContainersSnapshot>>,
    disk: Arc<Mutex<FixtureResponse>>,
    /// Answers volume measurements when set; otherwise they read `disk`.
    disk_volumes: Arc<Mutex<Option<FixtureResponse>>>,
    logs: Arc<Mutex<Option<FixtureResponse>>>,
    inspect: Arc<Mutex<Option<FixtureResponse>>>,
    hold_inspects: Arc<AtomicBool>,
    held_inspects: Arc<Mutex<Vec<HeldDiskRead>>>,
    compose_project: Arc<Mutex<FixtureResponse>>,
    compose_jobs: Arc<Mutex<FixtureResponse>>,
    compose_outputs: Arc<Mutex<HashMap<String, FixtureResponse>>>,
    hold_outputs: Arc<AtomicBool>,
    held_outputs: Arc<Mutex<Vec<HeldOutputRead>>>,
    held_disks: Arc<Mutex<Vec<HeldDiskRead>>>,
    hold_disk: Arc<AtomicBool>,
    held: Arc<Mutex<Vec<Stream>>>,
    hold: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl FakeBackend {
    pub(crate) fn new(snapshot: ContainersSnapshot) -> Self {
        let path = std::env::temp_dir().join(format!("npc-{}.sock", uuid::Uuid::new_v4().simple()));
        let listener = Listener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let calls = Arc::new(Mutex::new(vec![]));
        let source = Arc::new(Mutex::new(snapshot));
        let held = Arc::new(Mutex::new(vec![]));
        let hold = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let disk = Arc::new(Mutex::new(Ok(
            serde_json::to_value(disk_snapshot()).unwrap()
        )));
        let held_disks = Arc::new(Mutex::new(Vec::new()));
        let hold_disk = Arc::new(AtomicBool::new(false));
        let (disk_data, disk_streams, disk_gate) =
            (disk.clone(), held_disks.clone(), hold_disk.clone());
        let disk_volumes = Arc::new(Mutex::new(None::<FixtureResponse>));
        let volume_data = disk_volumes.clone();
        let logs = Arc::new(Mutex::new(None));
        let log_data = logs.clone();
        let inspect = Arc::new(Mutex::new(None::<FixtureResponse>));
        let hold_inspects = Arc::new(AtomicBool::new(false));
        let held_inspects = Arc::new(Mutex::new(Vec::new()));
        let (inspect_data, inspect_gate, inspect_streams) = (
            inspect.clone(),
            hold_inspects.clone(),
            held_inspects.clone(),
        );
        let compose_project = Arc::new(Mutex::new(Ok(compose_project())));
        let project_data = compose_project.clone();
        let compose_jobs = Arc::new(Mutex::new(Ok(json!({"jobs":[]}))));
        let compose_outputs = Arc::new(Mutex::new(HashMap::<String, FixtureResponse>::new()));
        let hold_outputs = Arc::new(AtomicBool::new(false));
        let held_outputs = Arc::new(Mutex::new(Vec::new()));
        let (jobs_data, output_data, output_gate, output_streams) = (
            compose_jobs.clone(),
            compose_outputs.clone(),
            hold_outputs.clone(),
            held_outputs.clone(),
        );
        let (call_store, data, held_streams, should_hold, done) = (
            calls.clone(),
            source.clone(),
            held.clone(),
            hold.clone(),
            stop.clone(),
        );
        let worker = thread::spawn(move || {
            while !done.load(Ordering::Acquire) {
                let (mut stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut header = [0; 4];
                stream.read_exact(&mut header).unwrap();
                let length = u32::from_be_bytes(header) as usize;
                assert!(length < 1024 * 1024);
                let mut bytes = vec![0; length];
                stream.read_exact(&mut bytes).unwrap();
                let request: Request = rmp_serde::from_slice(&bytes).unwrap();
                assert_eq!(request.version, VERSION);
                assert_eq!(request.args["id"], "fixture");
                let action = matches!(
                    request.method.as_str(),
                    "container_action"
                        | "project_action"
                        | "containers_remove"
                        | "docker_prune"
                        | "compose_run"
                        | "compose_cancel"
                );
                let result = if request.method == "compose_project" {
                    project_data.lock().unwrap().clone()
                } else if request.method == "compose_jobs" {
                    jobs_data.lock().unwrap().clone()
                } else if request.method == "compose_job" {
                    let id = request.args["job"].as_str().unwrap();
                    output_data
                        .lock()
                        .unwrap()
                        .get(id)
                        .cloned()
                        .unwrap_or_else(|| Err(format!("Unknown Compose job {id}")))
                        .map(|mut value| {
                            // Match the agent's incremental suffix, respecting UTF-8 boundaries.
                            let end = value["offset"].as_u64().unwrap();
                            let output = value["output"].as_str().unwrap();
                            let start = end.saturating_sub(output.len() as u64);
                            let requested = request.args["offset"].as_u64().unwrap_or(0);
                            let mut from =
                                requested.saturating_sub(start).min(output.len() as u64) as usize;
                            while !output.is_char_boundary(from) {
                                from += 1;
                            }
                            value["output"] = json!(&output[from..]);
                            value
                        })
                } else if request.method == "docker_disk" {
                    volume_data
                        .lock()
                        .unwrap()
                        .clone()
                        .filter(|_| request.args["part"] == "volumes")
                        .unwrap_or_else(|| disk_data.lock().unwrap().clone())
                } else if request.method == "container_logs"
                    && let Some(result) = log_data.lock().unwrap().clone()
                {
                    result
                } else if request.method == "container_inspect"
                    && let Some(result) =
                        inspect_data
                            .lock()
                            .unwrap()
                            .clone()
                            .filter(|result| match result {
                                Ok(value) => value["id"] == request.args["container"],
                                Err(_) => true,
                            })
                {
                    result
                } else {
                    Ok(match request.method.as_str() {
                        "containers_snapshot" => {
                            serde_json::to_value(&*data.lock().unwrap()).unwrap()
                        }
                        "container_inspect" => {
                            let data = data.lock().unwrap();
                            let container = data
                                .containers
                                .iter()
                                .find(|container| container.id == request.args["container"])
                                .expect("inspection must target a current concrete ID");
                            serde_json::to_value(ContainerInspect {
                                id: container.id.clone(),
                                name: container.name.clone(),
                                image: container.image.clone(),
                                entrypoint: Some(vec!["/usr/bin/app".into()]),
                                cmd: Some(vec!["serve".into()]),
                                env: vec![
                                    EnvVar {
                                        name: "MODE".into(),
                                        value: "fixture-public-value".into(),
                                    },
                                    EnvVar {
                                        name: "API_TOKEN".into(),
                                        value: "fixture-secret-value".into(),
                                    },
                                ],
                                ..Default::default()
                            })
                            .unwrap()
                        }
                        "container_logs" => serde_json::to_value(ContainerLogPage {
                            entries: vec![ContainerLogEntry {
                                message: format!(
                                    "Logs for {}",
                                    request.args["container"].as_str().unwrap_or("project")
                                ),
                                ..Default::default()
                            }],
                            cursor: Some(format!(
                                "cursor-{}",
                                request.args["container"].as_str().unwrap_or("project")
                            )),
                        })
                        .unwrap(),
                        "compose_run" => json!({"job":"fixture-job"}),
                        "container_action" | "project_action" | "containers_remove"
                        | "docker_prune" | "compose_cancel" => json!({"ok":true}),
                        other => panic!("unexpected request {other}"),
                    })
                };
                let is_disk = request.method == "docker_disk";
                let is_inspect = request.method == "container_inspect";
                let output_job = (request.method == "compose_job")
                    .then(|| request.args["job"].as_str().unwrap().to_owned());
                call_store.lock().unwrap().push(request);
                if action && should_hold.load(Ordering::Acquire) {
                    held_streams.lock().unwrap().push(stream);
                } else if is_disk && disk_gate.load(Ordering::Acquire) {
                    disk_streams.lock().unwrap().push((stream, result));
                } else if is_inspect && inspect_gate.load(Ordering::Acquire) {
                    inspect_streams.lock().unwrap().push((stream, result));
                } else if let Some(job) = output_job.filter(|_| output_gate.load(Ordering::Acquire))
                {
                    output_streams.lock().unwrap().push((job, stream, result));
                } else {
                    reply_result(&mut stream, result);
                }
            }
        });
        Self {
            path,
            calls,
            snapshot: source,
            disk,
            disk_volumes,
            logs,
            inspect,
            hold_inspects,
            held_inspects,
            compose_project,
            compose_jobs,
            compose_outputs,
            hold_outputs,
            held_outputs,
            held_disks,
            hold_disk,
            held,
            hold,
            stop,
            worker: Some(worker),
        }
    }
    pub(crate) fn client(&self) -> Containers {
        Containers::new(Backend::at_path(self.path.clone()), "fixture".into())
    }
    pub(super) fn set_logs(&self, result: Result<ContainerLogPage, String>) {
        *self.logs.lock().unwrap() = Some(result.map(|page| serde_json::to_value(page).unwrap()));
    }
    pub(super) fn set_inspect(&self, result: Result<ContainerInspect, String>) {
        *self.inspect.lock().unwrap() =
            Some(result.map(|data| serde_json::to_value(data).unwrap()));
    }
    pub(super) fn clear_inspect(&self) {
        *self.inspect.lock().unwrap() = None;
    }
    pub(super) fn hold_inspects(&self, hold: bool) {
        self.hold_inspects.store(hold, Ordering::Release);
    }
    pub(super) fn held_inspects(&self) -> usize {
        self.held_inspects.lock().unwrap().len()
    }
    pub(super) fn release_inspects(&self) {
        self.hold_inspects(false);
        for (mut stream, result) in self.held_inspects.lock().unwrap().drain(..) {
            reply_result(&mut stream, result);
        }
    }
    pub(super) fn set_compose_project(&self, result: FixtureResponse) {
        *self.compose_project.lock().unwrap() = result;
    }
    pub(super) fn set_compose_jobs(&self, result: FixtureResponse) {
        *self.compose_jobs.lock().unwrap() = result;
    }
    pub(super) fn set_compose_output(&self, job: &str, result: FixtureResponse) {
        self.compose_outputs
            .lock()
            .unwrap()
            .insert(job.into(), result);
    }
    pub(super) fn hold_outputs(&self, hold: bool) {
        self.hold_outputs.store(hold, Ordering::Release);
    }
    pub(super) fn held_outputs(&self, job: &str) -> usize {
        self.held_outputs
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _, _)| id == job)
            .count()
    }
    pub(super) fn release_output(&self, job: &str) {
        let mut held = self.held_outputs.lock().unwrap();
        let index = held
            .iter()
            .position(|(id, _, _)| id == job)
            .expect("held output");
        let (_, mut stream, result) = held.remove(index);
        reply_result(&mut stream, result);
    }
    pub(crate) fn count(&self, method: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method == method)
            .count()
    }
    pub(crate) fn hold_actions(&self) {
        self.hold.store(true, Ordering::Release);
    }
    pub(crate) fn held_actions(&self) -> usize {
        self.held.lock().unwrap().len()
    }
    pub(crate) fn release_action(&self, result: Value) {
        for mut stream in self.held.lock().unwrap().drain(..) {
            reply(&mut stream, result.clone());
        }
    }
    fn set_disk(&self, disk: DockerDisk) {
        *self.disk.lock().unwrap() = Ok(serde_json::to_value(disk).unwrap());
    }
    fn fail_disk(&self, error: &str) {
        *self.disk.lock().unwrap() = Err(error.into());
    }
    fn set_disk_volumes(&self, result: FixtureResponse) {
        *self.disk_volumes.lock().unwrap() = Some(result);
    }
    fn hold_disk(&self) {
        self.hold_disk.store(true, Ordering::Release);
    }
    fn held_disks(&self) -> usize {
        self.held_disks.lock().unwrap().len()
    }
    fn release_disk(&self) {
        self.hold_disk.store(false, Ordering::Release);
        for (mut stream, result) in self.held_disks.lock().unwrap().drain(..) {
            reply_result(&mut stream, result);
        }
    }
    pub(super) fn fail_action(&self, error: &str) {
        for mut stream in self.held.lock().unwrap().drain(..) {
            reply_result(&mut stream, Err(error.into()));
        }
    }
    pub(crate) fn set_snapshot(&self, snapshot: ContainersSnapshot) {
        *self.snapshot.lock().unwrap() = snapshot;
    }
    pub(super) fn requests(&self, method: &str) -> Vec<Value> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method == method)
            .map(|r| r.args.clone())
            .collect()
    }
    pub(crate) fn writes(&self) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|r| {
                matches!(
                    r.method.as_str(),
                    "container_action"
                        | "project_action"
                        | "containers_remove"
                        | "docker_prune"
                        | "compose_run"
                        | "compose_cancel"
                )
            })
            .count()
    }
}
fn reply(stream: &mut Stream, result: Value) {
    reply_result(stream, Ok(result));
}
fn reply_result(stream: &mut Stream, result: FixtureResponse) {
    let (result, error) = match result {
        Ok(value) => (value, None),
        Err(error) => (Value::Null, Some(json!(error))),
    };
    let bytes = rmp_serde::to_vec_named(&Response {
        version: VERSION,
        result,
        error,
    })
    .unwrap();
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&bytes).unwrap();
}
impl Drop for FakeBackend {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.held.lock().unwrap().clear();
        self.held_disks.lock().unwrap().clear();
        self.held_outputs.lock().unwrap().clear();
        if let Some(worker) = self.worker.take() {
            let result = worker.join();
            if !thread::panicking() {
                result.unwrap();
            }
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

fn disk_snapshot() -> DockerDisk {
    DockerDisk {
        volumes_measured: true,
        totals: DockerTotals {
            images: DiskTotals {
                count: 2,
                size: 3_000_000,
                reclaimable: 1_000_000,
            },
            containers: DiskTotals {
                count: 4,
                size: 1_000_000,
                reclaimable: 500_000,
            },
            volumes: DiskTotals {
                count: 2,
                size: 2_000_000,
                reclaimable: 1_000_000,
            },
            build_cache: DiskTotals {
                count: 3,
                size: 2_000_000,
                reclaimable: 1_500_000,
            },
        },
        images: vec![
            DockerImage {
                id: format!("sha256:{}", "a".repeat(64)),
                tags: vec!["fixture/web:1".into()],
                size: 2_000_000,
                shared_size: Some(500_000),
                created: 1_700_000_000_000,
                containers: 1,
                dangling: false,
            },
            DockerImage {
                id: format!("sha256:{}", "b".repeat(64)),
                tags: vec![],
                size: 1_000_000,
                shared_size: None,
                created: 1_700_000_001_000,
                containers: 0,
                dangling: true,
            },
        ],
        volumes: vec![
            DockerVolume {
                name: "stack-data".into(),
                driver: "local".into(),
                size: Some(1_000_000),
                ref_count: Some(1),
                anonymous: false,
                compose: Some("stack".into()),
            },
            DockerVolume {
                name: "c".repeat(64),
                driver: "local".into(),
                size: None,
                ref_count: None,
                anonymous: true,
                compose: None,
            },
        ],
    }
}

pub(super) fn compose_project() -> Value {
    json!({"workingDir":"/srv/stack","cli":{"kind":"plugin","version":"2.40.1"},"cliError":null,"configFiles":["/srv/stack/compose.yml"],"configError":null,"services":[{"name":"web","image":"fixture/web:1","build":false,"profiles":[]},{"name":"worker","image":"fixture/worker:1","build":true,"profiles":[]}],"files":[{"path":"/srv/stack/compose.yml","text":"services: {}","error":null}]})
}
pub(crate) fn snapshot() -> ContainersSnapshot {
    let service = |id: &str, name: &str, service: &str, state, exit_code| Container {
        id: id.into(),
        name: name.into(),
        image: format!("fixture/{service}:1"),
        state,
        status: state.to_string(),
        exit_code,
        compose: Some(ComposeLabels {
            project: "stack".into(),
            service: service.into(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut job = service(
        "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        "stack-run-1",
        "run",
        ContainerState::Exited,
        Some(0),
    );
    job.compose.as_mut().unwrap().oneoff = true;
    ContainersSnapshot {
        engine: EngineInfo {
            version: "27.0.0".into(),
            ..Default::default()
        },
        containers: vec![
            service(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "stack-web-1",
                "web",
                ContainerState::Running,
                None,
            ),
            service(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "stack-worker-1",
                "worker",
                ContainerState::Exited,
                Some(1),
            ),
            job,
            Container {
                id: "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd".into(),
                name: "cache".into(),
                image: "redis:7".into(),
                state: ContainerState::Running,
                ..Default::default()
            },
        ],
        truncated: false,
    }
}
pub(crate) fn fixture(
    cx: &mut TestAppContext,
) -> (Entity<ContainersPanel>, &mut VisualTestContext, FakeBackend) {
    fixture_with_size(cx, 980., 1000.)
}
fn fixture_with_size(
    cx: &mut TestAppContext,
    width: f32,
    height: f32,
) -> (Entity<ContainersPanel>, &mut VisualTestContext, FakeBackend) {
    fixture_with_snapshot(cx, snapshot(), width, height)
}
fn fixture_with_snapshot(
    cx: &mut TestAppContext,
    data: ContainersSnapshot,
    width: f32,
    height: f32,
) -> (Entity<ContainersPanel>, &mut VisualTestContext, FakeBackend) {
    let backend = FakeBackend::new(data);
    let client = backend.client();
    cx.update(|cx| {
        gpui::component::init(cx);
        crate::theme::install(false, cx);
        cx.set_reduce_motion(true);
    });
    let mut panel = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let content = cx.new(|cx| ContainersPanel::with_client(client, window, cx));
        panel = Some(content.clone());
        let shell = cx.new(|_| FixtureShell(content));
        gpui::base::Root::new(shell, window, cx)
    });
    cx.update(|window, _| window.activate_window());
    cx.simulate_resize(size(px(width), px(height)));
    let panel = panel.unwrap();
    settle(&panel, cx);
    (panel, cx, backend)
}
struct FixtureShell(Entity<ContainersPanel>);
impl Render for FixtureShell {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .font_family(crate::theme::UI_FONT)
            .text_size(px(13.))
            .child(self.0.clone())
    }
}
pub(crate) fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
}
pub(crate) fn settle(panel: &Entity<ContainersPanel>, cx: &mut VisualTestContext) {
    for _ in 0..300 {
        panel.update(cx, |panel, cx| panel.poll(true, cx));
        draw(cx);
        if !panel.read_with(cx, |p, _| p.has_pending()) {
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!("fixture request did not settle");
}
pub(super) fn element(cx: &mut VisualTestContext, label: &str) -> ElementSnapshot {
    cx.update(|window, _| {
        snapshots(window)
            .into_iter()
            .find(|e| e.label() == Some(label))
            .unwrap_or_else(|| panic!("missing {label}"))
    })
}
fn has_element(cx: &mut VisualTestContext, label: &str) -> bool {
    cx.update(|window, _| snapshots(window).iter().any(|e| e.label() == Some(label)))
}
/// Remove lives in the selected container's "…" menu.
fn click_remove(cx: &mut VisualTestContext) {
    let more = cx.update(|window, _| {
        snapshots(window)
            .into_iter()
            .find(|e| {
                e.label()
                    .is_some_and(|l| l.starts_with("More actions for "))
            })
            .expect("container actions menu")
            .bounds()
            .center()
    });
    cx.simulate_click(more, Default::default());
    draw(cx);
    let item = cx.update(|window, _| {
        snapshots(window)
            .into_iter()
            .find(|e| {
                e.role() == Some(gpui_kit::accesskit::Role::MenuItem)
                    && e.label() == Some("Remove…")
            })
            .expect("Remove menu item")
            .bounds()
            .center()
    });
    cx.simulate_click(item, Default::default());
    draw(cx);
}
pub(super) fn click(cx: &mut VisualTestContext, label: &str) {
    let at = element(cx, label).bounds().center();
    cx.simulate_click(at, Default::default());
    draw(cx);
}
fn key(cx: &mut VisualTestContext, key: &str) {
    cx.simulate_event(KeyDownEvent {
        physical_key: None,
        key_metadata: None,
        keystroke: Keystroke::parse(key).unwrap(),
        is_held: false,
        prefer_character_input: false,
    });
    draw(cx);
}
pub(super) fn select(panel: &Entity<ContainersPanel>, cx: &mut VisualTestContext, key: &str) {
    cx.update(|window, cx| panel.update(cx, |panel, cx| panel.select(key.into(), window, cx)));
    settle(panel, cx);
}
/// Projects open on Services; these tests read the Logs tab.
pub(super) fn select_logs(panel: &Entity<ContainersPanel>, cx: &mut VisualTestContext, key: &str) {
    select(panel, cx, key);
    panel.update(cx, |panel, cx| {
        panel.tab = 0;
        panel.sync_selection(cx);
        cx.notify();
    });
    settle(panel, cx);
}
pub(crate) fn restart_web(panel: &Entity<ContainersPanel>, cx: &mut VisualTestContext) {
    panel.update(cx, |panel, _| {
        assert!(panel.client.action(Action::Container {
            container: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            name: "stack-web-1".into(),
            verb: ContainerVerb::Restart
        }))
    });
}

#[gpui::test]
fn grouped_list_renders_each_container_once_and_search_opens_collapsed_matches(
    cx: &mut TestAppContext,
) {
    let (panel, cx, backend) = fixture(cx);
    let rows = panel.read_with(cx, |p, cx| p.rows(cx));
    let keys: Vec<_> = rows.iter().filter_map(|r| r.key.as_deref()).collect();
    assert_eq!(
        keys.iter()
            .filter(|key| **key == "c:stack-worker-1")
            .count(),
        1,
        "problem containers stay in their project"
    );
    assert_eq!(
        keys.iter().filter(|key| key.starts_with("c:")).count(),
        3,
        "one-off jobs start folded"
    );
    for (index, row) in rows.iter().enumerate().filter(|(_, r)| r.key.is_some()) {
        let bounds = cx
            .debug_bounds(Box::leak(
                format!("containers-row-{index}").into_boxed_str(),
            ))
            .unwrap_or_else(|| panic!("row {} should render", row.name));
        let master = cx.debug_bounds("containers-master").unwrap();
        assert!(
            (bounds.right() - (master.right() - px(1.))).abs() <= px(2.),
            "row {} must fill its pane, not shrink to its text",
            row.name
        );
        assert!(bounds.left() >= master.left());
    }
    assert!(
        cx.debug_bounds("container-job-badge-c:stack-run-1")
            .is_none()
    );
    click(cx, "1 one-off job");
    let job = element(cx, "stack-run-1: one-off job, Completed");
    let badge = cx
        .debug_bounds("container-job-badge-c:stack-run-1")
        .expect("expanded one-off rows must show their job badge");
    assert!(badge.left() >= job.bounds().left() && badge.right() <= job.bounds().right());
    assert_eq!(job.bounds().size.height, px(28.));
    assert!(
        cx.debug_bounds("container-job-badge-c:stack-web-1")
            .is_none()
    );
    click(cx, "stack-run-1: one-off job, Completed");
    assert_eq!(
        panel.read_with(cx, |p, _| p.selected.clone()),
        Some("c:stack-run-1".into())
    );
    click(cx, "1 one-off job");
    assert!(
        cx.debug_bounds("container-job-badge-c:stack-run-1")
            .is_none()
    );
    click(cx, "Fold stack");
    assert!(!panel.read_with(cx, |p, cx| {
        p.rows(cx)
            .iter()
            .any(|r| r.key.as_deref() == Some("c:stack-web-1"))
    }));
    use gpui::Focusable;
    let search = panel.read_with(cx, |p, _| p.search.clone());
    cx.update(|window, cx| search.read(cx).focus_handle(cx).focus(window, cx));
    cx.simulate_input("fixture/web");
    settle(&panel, cx);
    assert!(panel.read_with(cx, |p, cx| {
        p.rows(cx)
            .iter()
            .any(|r| r.key.as_deref() == Some("c:stack-web-1"))
    }));
    assert_eq!(backend.writes(), 0);
    let all = snapshot();
    let rows = master_rows(
        &all.containers,
        "",
        &HashSet::new(),
        &HashSet::from(["stack".into()]),
        EVERY,
        0,
    );
    let keys: Vec<_> = rows
        .iter()
        .filter_map(|r| r.key.as_ref())
        .filter(|key| key.starts_with("c:"))
        .collect();
    assert_eq!(keys.len(), all.containers.len());
    assert_eq!(keys.iter().collect::<HashSet<_>>().len(), keys.len());
}

#[test]
fn oneoff_only_projects_do_not_display_a_zero_service_fraction() {
    let mut source = snapshot();
    source.containers[1].exit_code = Some(0);
    let all_rows = master_rows(
        &source.containers,
        "",
        &HashSet::new(),
        &HashSet::new(),
        EVERY,
        0,
    );
    assert_eq!(
        all_rows
            .iter()
            .find(|row| row.key.as_deref() == Some("p:stack"))
            .unwrap()
            .note,
        "1/2 running"
    );
    let jobs = source
        .containers
        .into_iter()
        .filter(|container| {
            container
                .compose
                .as_ref()
                .is_some_and(|labels| labels.oneoff)
        })
        .collect::<Vec<_>>();
    let rows = master_rows(&jobs, "", &HashSet::new(), &HashSet::new(), EVERY, 0);
    let project = rows
        .iter()
        .find(|row| row.key.as_deref() == Some("p:stack"))
        .unwrap();
    assert!(project.note.is_empty());
    assert!(
        rows.iter()
            .any(|row| row.jobs && row.name == "1 one-off job")
    );
}

#[gpui::test]
fn recreated_name_preserves_selection_and_resets_reveals_and_log_target(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "c:stack-web-1");
    assert_eq!(
        panel.read_with(cx, |p, _| p.client.logs().unwrap().cursor.clone()),
        Some("cursor-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into())
    );
    panel.update(cx, |p, _| {
        p.reveal_values = true;
        p.revealed.insert("API_TOKEN".into());
        p.labels_open = true;
        p.review = Some(Action::Container {
            container: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            name: "stack-web-1".into(),
            verb: ContainerVerb::Stop,
        });
    });
    let mut fresh = snapshot();
    fresh.containers[0].id =
        "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".into();
    backend.set_snapshot(fresh);
    panel.update(cx, |p, cx| p.refresh(cx));
    settle(&panel, cx);
    panel.read_with(cx, |p, _| {
        assert_eq!(p.selected.as_deref(), Some("c:stack-web-1"));
        assert_eq!(
            p.inspected_id.as_deref(),
            Some("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee")
        );
        assert!(!p.reveal_values);
        assert!(p.revealed.is_empty());
        assert!(!p.labels_open);
        assert!(p.review.is_none());
        assert_eq!(
            p.client.inspect().unwrap().id,
            "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
        );
    });
    let reads = backend.requests("container_logs");
    let first = reads
        .iter()
        .find(|args| {
            args["container"] == "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
        })
        .unwrap();
    assert!(
        first["cursor"].is_null(),
        "recreated containers must start a new journal cursor"
    );
    assert_eq!(
        backend.requests("container_inspect").last().unwrap()["container"],
        "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
    );
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn inspector_keeps_secrets_hidden_until_individual_reveal(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "c:stack-web-1");
    click(cx, "Inspect");
    assert!(has_element(cx, "Show MODE"));
    assert!(has_element(cx, "Show API_TOKEN"));
    click(cx, "Show values");
    assert!(!has_element(cx, "Show MODE"));
    assert!(has_element(cx, "Show API_TOKEN"));
    click(cx, "Show API_TOKEN");
    assert!(!has_element(cx, "Show API_TOKEN"));
    assert!(panel.read_with(cx, |p, _| p.revealed.contains("API_TOKEN")));
    click(cx, "Hide values");
    assert!(has_element(cx, "Show MODE"));
    assert!(has_element(cx, "Show API_TOKEN"));
    assert!(panel.read_with(cx, |p, _| p.revealed.is_empty()));
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn keyboard_navigation_and_tabs_only_read_with_one_selected_tab_stop(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "c:stack-web-1");
    key(cx, "end");
    assert_eq!(
        panel.read_with(cx, |p, _| p.selected.clone()),
        Some("c:cache".into())
    );
    // Disk usage lives in the toolbar now; the list starts with projects.
    key(cx, "home");
    assert_eq!(
        panel.read_with(cx, |p, _| p.selected.clone()),
        Some("p:stack".into())
    );
    select(&panel, cx, "c:stack-web-1");
    click(cx, "Logs");
    cx.update(|window, cx| assert!(panel.read(cx).tab_focus[0].is_focused(window)));
    cx.update(|window, cx| window.focus_next(cx));
    cx.update(|window, cx| {
        assert!(!panel.read(cx).tab_focus[1].is_focused(window));
        window.focus_prev(cx);
    });
    cx.update(|window, cx| assert!(panel.read(cx).tab_focus[0].is_focused(window)));
    for (stroke, tab) in [
        ("right", 1),
        ("left", 0),
        ("end", 1),
        ("enter", 1),
        ("space", 1),
        ("home", 0),
    ] {
        key(cx, stroke);
        cx.update(|window, cx| {
            assert_eq!(panel.read(cx).tab, tab);
            assert!(panel.read(cx).tab_focus[tab].is_focused(window));
        });
    }
    settle(&panel, cx);
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn arrow_keys_enter_the_list_from_a_selection_it_does_not_show(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    for (stroke, expected) in [
        ("down", "p:stack"),
        ("home", "p:stack"),
        ("up", "c:cache"),
        ("end", "c:cache"),
    ] {
        select(&panel, cx, "disk");
        key(cx, stroke);
        assert_eq!(
            panel.read_with(cx, |p, _| p.selected.clone()),
            Some(expected.into()),
            "{stroke} from Disk usage"
        );
    }
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn default_selection_skips_containers_the_running_filter_hides(cx: &mut TestAppContext) {
    let container = |id: char, name: &str, state| Container {
        id: id.to_string().repeat(64),
        name: name.into(),
        image: "redis:7".into(),
        state,
        exit_code: (state == ContainerState::Exited).then_some(0),
        ..Default::default()
    };
    let data = ContainersSnapshot {
        containers: vec![
            container('a', "old", ContainerState::Exited),
            container('b', "cache", ContainerState::Running),
        ],
        ..snapshot()
    };
    let (panel, cx, backend) = fixture_with_snapshot(cx, data, 980., 1000.);
    assert_eq!(
        panel.read_with(cx, |p, cx| p.effective_selection(cx)),
        Some("c:cache".into()),
        "the stopped container is first but hidden"
    );
    assert_eq!(
        panel.read_with(cx, |p, _| p.active_selection.clone()),
        Some("c:cache".into())
    );
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn compact_navigation_moves_focus_between_list_and_detail(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture_with_size(cx, 560., 900.);
    assert!(cx.debug_bounds("containers-master").is_some());
    assert!(cx.debug_bounds("containers-detail").is_none());
    select(&panel, cx, "c:stack-web-1");
    assert!(cx.debug_bounds("containers-master").is_none());
    assert!(cx.debug_bounds("containers-detail").is_some());
    cx.update(|window, cx| assert!(panel.read(cx).detail_focus.is_focused(window)));
    click(cx, "All containers");
    assert!(cx.debug_bounds("containers-master").is_some());
    assert!(cx.debug_bounds("containers-detail").is_none());
    cx.update(|window, cx| assert!(panel.read(cx).list_focus.is_focused(window)));
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn filtering_a_fallback_project_discards_its_compose_review_without_writing(
    cx: &mut TestAppContext,
) {
    let (panel, cx, backend) = fixture(cx);
    let mut fresh = snapshot();
    fresh.containers[1].exit_code = Some(0);
    let mut other = fresh.containers[0].clone();
    other.id = "f".repeat(64);
    other.name = "zoo-web-1".into();
    other.compose.as_mut().unwrap().project = "zoo".into();
    fresh.containers.push(other);
    backend.set_snapshot(fresh);
    panel.update(cx, |p, cx| p.refresh(cx));
    settle(&panel, cx);
    panel.read_with(cx, |p, _| {
        assert!(
            p.selected.is_none(),
            "test must exercise fallback selection"
        );
        assert_eq!(p.client.selected_project(), Some("stack"));
        assert!(p.client.selected_container().is_none());
    });
    click(cx, "More Compose actions");
    click(cx, "Down…");
    assert!(panel.read_with(cx, |p, _| p.compose_state.proposed.is_some()));
    assert!(cx.debug_bounds("compose-review").is_some());
    let search = panel.read_with(cx, |p, _| p.search.clone());
    // A model-driven filter change can retire a fallback selection even while
    // the modal correctly keeps keyboard input out of the background search.
    cx.update(|window, cx| search.update(cx, |search, cx| search.set_value("zoo", window, cx)));
    panel.update(cx, |panel, cx| {
        panel.sync_selection(cx);
        cx.notify();
    });
    settle(&panel, cx);
    panel.read_with(cx, |p, _| {
        assert!(p.selected.is_none());
        assert_eq!(p.client.selected_project(), Some("zoo"));
        assert!(p.compose_state.proposed.is_none());
    });
    assert!(cx.debug_bounds("compose-review").is_none());
    assert_eq!(backend.writes(), 0);
}

fn click_selector(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("missing {selector}"));
    cx.simulate_click(bounds.center(), Default::default());
    draw(cx);
}

#[gpui::test]
fn published_port_links_keep_exact_bindings_range_offsets_and_inactive_states(
    cx: &mut TestAppContext,
) {
    use newport_client::tunnel_operations::Operations;
    use newport_domain::containers::{ContainerPort, PortProtocol};
    use std::{cell::RefCell, rc::Rc};
    let (panel, cx, backend) = fixture(cx);
    let mut data = snapshot();
    let port = ContainerPort {
        ip: Some("0.0.0.0".into()),
        host_port: Some(3003),
        container_port: 80,
        protocol: PortProtocol::Tcp,
    };
    data.containers[0].ports = vec![
        ContainerPort {
            ip: Some("::".into()),
            ..port.clone()
        },
        port.clone(),
        ContainerPort {
            protocol: PortProtocol::Udp,
            ..port.clone()
        },
        ContainerPort {
            host_port: Some(3004),
            ip: Some("10.0.0.4".into()),
            ..port.clone()
        },
        ContainerPort {
            host_port: None,
            protocol: PortProtocol::Udp,
            container_port: 53,
            ..port.clone()
        },
    ];
    backend.set_snapshot(data.clone());
    panel.update(cx, |p, cx| p.refresh(cx));
    settle(&panel, cx);
    select(&panel, cx, "c:stack-web-1");
    let server = uuid::Uuid::new_v4();
    let mut tunnel =
        newport_domain::forwarding::discovered_tunnel(&[], server, "127.0.0.1", 3000).unwrap();
    tunnel.remote_port_end = Some(3005);
    tunnel.local_port = 9000;
    tunnel.local_port_end = Some(9005);
    let mut config = json!({"config":{"tunnels":[tunnel]},"runtime":{"tunnels":{tunnel.id.to_string():{"status":"connected"}}}});
    let operations = Operations::new(Backend::at_path(backend.path.clone()));
    panel.update(cx, |p, cx| {
        p.sync_port_links(&server.to_string(), Some(&config), &operations, cx)
    });
    draw(cx);
    let requests = Rc::new(RefCell::new(Vec::new()));
    let captured = requests.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&panel, move |_, event: &super::ContainersEvent, _| {
            if let super::ContainersEvent::OpenPort(target) = event {
                captured.borrow_mut().push(target.clone());
            }
        })
    });
    assert!(has_element(cx, "Open port 3003→80"));
    assert!(
        cx.debug_bounds("container-port-1").is_none(),
        "UDP is not a link"
    );
    assert!(
        cx.debug_bounds("container-port-2").is_none(),
        "private binding is not a link"
    );
    let chip = cx.debug_bounds("container-port-0").unwrap();
    assert_eq!(chip.size.height, px(24.));
    panel.read_with(cx, |p, _| {
        let forward = p.ports.rows[0].forward.as_ref().unwrap();
        assert_eq!(forward.local_port, 9003);
    });
    click_selector(cx, "container-port-0");
    assert_eq!(
        requests.borrow().as_slice(),
        &[super::PortTarget {
            container: "a".repeat(64),
            name: "stack-web-1".into(),
            port: port.clone(),
        }]
    );
    config["runtime"]["tunnels"][tunnel.id.to_string()]["status"] = json!("reconnecting");
    panel.update(cx, |p, cx| {
        p.sync_port_links(&server.to_string(), Some(&config), &operations, cx)
    });
    draw(cx);
    click_selector(cx, "container-port-0");
    assert_eq!(
        requests.borrow().len(),
        1,
        "reconnecting target stays inactive"
    );
    config["runtime"]["tunnels"][tunnel.id.to_string()]["status"] = json!("disconnected");
    panel.update(cx, |p, cx| {
        p.sync_port_links(&server.to_string(), Some(&config), &operations, cx)
    });
    draw(cx);
    assert!(has_element(cx, "Connect and open port 3003→80"));
    panel.update(cx, |p, cx| {
        p.sync_port_links(&server.to_string(), None, &operations, cx)
    });
    draw(cx);
    click_selector(cx, "container-port-0");
    assert_eq!(
        requests.borrow().len(),
        1,
        "configuration outage stays inactive"
    );
    data.containers[0].state = ContainerState::Exited;
    backend.set_snapshot(data);
    panel.update(cx, |p, cx| p.refresh(cx));
    settle(&panel, cx);
    panel.update(cx, |p, cx| {
        p.sync_port_links(&server.to_string(), Some(&config), &operations, cx)
    });
    draw(cx);
    click_selector(cx, "container-port-0");
    assert_eq!(
        requests.borrow().len(),
        1,
        "stopped container cannot initiate a forward"
    );
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn compact_container_ports_wrap_and_failures_stay_inline_without_replaying(
    cx: &mut TestAppContext,
) {
    use newport_client::tunnel_operations::Operations;
    use newport_domain::containers::{ContainerPort, PortProtocol};
    let (panel, cx, backend) = fixture_with_size(cx, 560., 1000.);
    let mut data = snapshot();
    data.containers[0].ports = (8000..8008)
        .map(|port| ContainerPort {
            ip: Some("0.0.0.0".into()),
            host_port: Some(port),
            container_port: port,
            protocol: PortProtocol::Tcp,
        })
        .collect();
    backend.set_snapshot(data.clone());
    panel.update(cx, |p, cx| p.refresh(cx));
    settle(&panel, cx);
    select(&panel, cx, "c:stack-web-1");
    let server = uuid::Uuid::new_v4().to_string();
    let config = json!({"config":{"tunnels":[]},"runtime":{}});
    // This transport cannot discover the real backend or mutate a real server.
    let mut operations = Operations::new(Backend::at_path(
        std::env::temp_dir().join(format!("missing-{}.sock", uuid::Uuid::new_v4())),
    ));
    panel.update(cx, |p, cx| {
        p.sync_port_links(&server, Some(&config), &operations, cx)
    });
    draw(cx);
    let row = cx.debug_bounds("container-port-links").unwrap();
    let first = cx.debug_bounds("container-port-0").unwrap();
    let last = cx.debug_bounds("container-port-7").unwrap();
    assert!(
        last.top() > first.top(),
        "port controls wrap in compact detail"
    );
    for selector in [
        "container-port-0",
        "container-port-1",
        "container-port-2",
        "container-port-3",
        "container-port-4",
        "container-port-5",
        "container-port-6",
        "container-port-7",
    ] {
        let bounds = cx.debug_bounds(selector).unwrap();
        assert!(bounds.left() >= row.left() && bounds.right() <= row.right());
        assert_eq!(bounds.size.height, px(24.));
    }
    let target = super::PortTarget {
        container: "a".repeat(64),
        name: "stack-web-1".into(),
        port: data.containers[0].ports[0].clone(),
    };
    panel.update(cx, |p, cx| {
        p.open_port_link(&server, Value::Null, &target, &mut operations, cx);
        p.open_port_link(&server, Value::Null, &target, &mut operations, cx);
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        panel.update(cx, |p, cx| {
            p.sync_port_links(&server, Some(&config), &operations, cx)
        });
        if panel.read_with(cx, |p, _| {
            p.ports.attempts.first().is_some_and(|a| a.result.is_some())
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "port handover did not finish"
        );
        thread::sleep(Duration::from_millis(2));
    }
    draw(cx);
    assert!(
        cx.debug_bounds("container-port-error-0").is_some(),
        "failed handover must stay visible beside the port"
    );
    panel.read_with(cx, |p, _| {
        assert_eq!(p.ports.attempts.len(), 1);
        assert!(p.ports.attempts[0].result.as_ref().unwrap().is_err());
    });
    let outcomes = operations.poll();
    assert!(outcomes.len() <= 1);
    assert!(operations.poll().is_empty());
    select(&panel, cx, "c:cache");
    panel.update(cx, |p, cx| {
        p.sync_port_links(&server, Some(&config), &operations, cx)
    });
    draw(cx);
    assert!(cx.debug_bounds("container-port-links").is_none());
    assert!(panel.read_with(cx, |p, _| p.ports.attempts.is_empty()));
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn shell_handover_uses_the_observed_container_and_pause_lives_in_the_menu(cx: &mut TestAppContext) {
    use std::{cell::RefCell, rc::Rc};
    let (panel, cx, backend) = fixture(cx);
    let shells = Rc::new(RefCell::new(Vec::new()));
    let captured = shells.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&panel, move |_, event: &super::ContainersEvent, _| {
            if let super::ContainersEvent::OpenShell { command, title } = event {
                captured.borrow_mut().push((command.clone(), title.clone()));
            }
        })
    });
    select(&panel, cx, "c:stack-web-1");
    assert!(has_element(cx, "Shell"));
    assert!(!has_element(cx, "Pause"));
    assert!(!has_element(cx, "Remove…"));
    click(cx, "Shell");
    assert_eq!(shells.borrow().len(), 1);
    assert_eq!(shells.borrow()[0].1, "web");
    assert!(shells.borrow()[0].0.contains(&"a".repeat(64)));
    assert!(!shells.borrow()[0].0.contains("stack-web-1"));
    assert_eq!(backend.writes(), 0);

    backend.hold_actions();
    click(cx, "More actions for stack-web-1");
    click(cx, "Pause");
    wait_for_held_action(&backend);
    let calls = backend.requests("container_action");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["container"], "a".repeat(64));
    assert_eq!(calls[0]["verb"], "pause");
    click(cx, "Shell");
    assert_eq!(
        shells.borrow().len(),
        1,
        "pending action must block shell handover"
    );
    backend.release_action(json!({"ok":true}));
    settle(&panel, cx);

    select(&panel, cx, "c:stack-worker-1");
    click(cx, "Shell");
    assert_eq!(shells.borrow().len(), 1, "stopped container has no shell");
    let mut paused = snapshot();
    paused.containers[1].state = ContainerState::Paused;
    backend.set_snapshot(paused);
    panel.update(cx, |p, cx| p.refresh(cx));
    settle(&panel, cx);
    assert!(has_element(cx, "Resume"));
    assert!(!has_element(cx, "Shell"));
    assert!(!has_element(cx, "Remove…"));
    assert_eq!(backend.writes(), 1);
}

#[gpui::test]
fn removal_review_cancels_retries_and_keeps_its_original_target_while_navigating(
    cx: &mut TestAppContext,
) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "c:stack-worker-1");
    click_remove(cx);
    assert!(cx.debug_bounds("container-action-review").is_some());
    cx.update(|window, cx| assert!(panel.read(cx).review_focus.is_focused(window)));
    assert_eq!(backend.writes(), 0);
    click(cx, "Cancel");
    assert!(panel.read_with(cx, |p, _| p.review.is_none()));
    assert_eq!(backend.writes(), 0);

    backend.hold_actions();
    click_remove(cx);
    click_selector(cx, "confirm-container-action");
    wait_for_held_action(&backend);
    click_selector(cx, "confirm-container-action");
    click(cx, "Cancel");
    assert!(panel.read_with(cx, |p, _| p.review.is_some()));
    assert_eq!(backend.count("containers_remove"), 1);
    assert_eq!(
        backend.requests("containers_remove")[0]["containers"],
        json!(["b".repeat(64)])
    );
    backend.fail_action("Fixture engine refused removal");
    settle(&panel, cx);
    assert!(panel.read_with(cx, |p, _| p.review.is_some()));
    assert!(cx.debug_bounds("container-review-error").is_some());
    assert_eq!(
        panel.read_with(cx, |p, _| p
            .client
            .action_status(&p.review.as_ref().unwrap().target())
            .unwrap()
            .error
            .clone()),
        Some("Fixture engine refused removal".into())
    );
    click_selector(cx, "confirm-container-action");
    wait_for_held_action(&backend);
    cx.update(|window, cx| panel.update(cx, |p, cx| p.select("c:cache".into(), window, cx)));
    draw(cx);
    assert!(panel.read_with(cx, |p, _| p.review.is_none()));
    let mut removed = snapshot();
    removed.containers.retain(|c| c.id != "b".repeat(64));
    backend.set_snapshot(removed);
    backend.release_action(json!({"removed":1}));
    settle(&panel, cx);
    assert_eq!(backend.count("containers_remove"), 2);
    assert!(
        backend
            .requests("containers_remove")
            .iter()
            .all(|args| args["containers"] == json!(["b".repeat(64)]))
    );
    panel.read_with(cx, |p, _| {
        assert_eq!(p.client.selected_container().unwrap().name, "cache");
        assert!(
            !p.client
                .snapshot()
                .unwrap()
                .containers
                .iter()
                .any(|c| c.name == "stack-worker-1")
        );
    });
}

#[gpui::test]
fn removal_review_rejects_a_restarted_or_recreated_container(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "c:stack-worker-1");
    click_remove(cx);
    let mut changed = snapshot();
    changed.containers[1].state = ContainerState::Running;
    backend.set_snapshot(changed);
    panel.update(cx, |p, cx| p.refresh(cx));
    settle(&panel, cx);
    click_selector(cx, "confirm-container-action");
    assert_eq!(backend.writes(), 0);
    assert!(cx.debug_bounds("container-review-stale").is_some());
    click(cx, "Cancel");

    backend.set_snapshot(snapshot());
    panel.update(cx, |p, cx| p.refresh(cx));
    settle(&panel, cx);
    click_remove(cx);
    let mut changed = snapshot();
    changed.containers[1].id = "f".repeat(64);
    backend.set_snapshot(changed);
    panel.update(cx, |p, cx| p.refresh(cx));
    settle(&panel, cx);
    assert!(panel.read_with(cx, |p, _| p.review.is_none()));
    assert!(cx.debug_bounds("confirm-container-action").is_none());
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn compact_removal_review_keeps_cancel_visible_and_focused(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture_with_size(cx, 560., 480.);
    select(&panel, cx, "c:stack-worker-1");
    click_remove(cx);
    let detail = cx.update(|window, _| window.viewport_size());
    let cancel = cx.debug_bounds("cancel-container-review").unwrap();
    assert!(
        cancel.top() >= px(0.) && cancel.bottom() <= detail.height,
        "{cancel:?} outside {detail:?}"
    );
    cx.update(|window, cx| assert!(panel.read(cx).review_focus.is_focused(window)));
    key(cx, "enter");
    cx.simulate_event(gpui::KeyUpEvent {
        physical_key: None,
        key_metadata: None,
        keystroke: Keystroke::parse("enter").unwrap(),
    });
    draw(cx);
    assert!(panel.read_with(cx, |p, _| p.review.is_none()));
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn lifecycle_dialog_blocks_background_pointer_and_keyboard_until_explicit_confirmation(
    cx: &mut TestAppContext,
) {
    use gpui::component::WindowExt;
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "c:stack-worker-1");
    let start = element(cx, "Start").bounds().center();
    click_remove(cx);
    assert!(cx.debug_bounds("dialog-layer").is_some());
    assert_eq!(
        element(cx, "The stopped container and its logs are deleted. Its image and volumes stay. Compose creates it again on the next up.").role(),
        Some(gpui::Role::Label)
    );
    cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
    panel.update(cx, |p, cx| p.poll(true, cx));
    draw(cx);
    assert!(panel.read_with(cx, |p, _| p.has_review_dialog()));
    cx.simulate_click(start, Default::default());
    draw(cx);
    assert_eq!(
        backend.writes(),
        0,
        "the modal must occlude background Start"
    );
    let focus = cx.update(|window, cx| window.active_dialog_focus_handle(cx).unwrap());
    cx.update(|window, cx| focus.focus(window, cx));
    key(cx, "enter");
    assert_eq!(
        backend.writes(),
        0,
        "bare Enter cannot approve a destructive action"
    );
    assert!(panel.read_with(cx, |p, _| p.review.is_some()));
    backend.hold_actions();
    click_selector(cx, "confirm-container-action");
    wait_for_held_action(&backend);
    key(cx, "escape");
    click(cx, "Cancel");
    assert!(panel.read_with(cx, |p, _| p.has_review_dialog()));
    assert_eq!(backend.count("containers_remove"), 1);
    backend.release_action(json!({"removed":1}));
    settle(&panel, cx);
    assert!(!panel.read_with(cx, |p, _| p.has_review_dialog()));
    cx.update(|window, cx| {
        assert!(!window.has_active_dialog(cx));
        assert!(panel.read(cx).detail_focus.is_focused(window));
    });
}

#[gpui::test]
fn lifecycle_dialog_scope_cleanup_closes_only_its_owner_even_when_covered(cx: &mut TestAppContext) {
    use gpui::component::WindowExt;
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "c:stack-worker-1");
    click_remove(cx);
    panel.update(cx, |p, cx| p.poll(false, cx));
    draw(cx);
    assert!(!panel.read_with(cx, |p, _| p.has_review_dialog()));
    cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    click_remove(cx);
    let unrelated = cx.update(|window, cx| {
        window.open_dialog(cx, |dialog, _, _| dialog.title("Unrelated dialog"));
        window.active_dialog_focus_handle(cx).unwrap()
    });
    assert!(
        !panel.read_with(cx, |p, _| p.has_review_dialog()),
        "an unrelated top dialog must suspend page polling"
    );
    panel.update(cx, |p, cx| p.retire(cx));
    draw(cx);
    cx.update(|window, cx| {
        assert_eq!(
            window.active_dialog_focus_handle(cx).as_ref(),
            Some(&unrelated)
        );
        window.close_dialog(cx);
        assert!(
            !window.has_active_dialog(cx),
            "retired confirmation must not return after closing the covering dialog"
        );
    });
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn hidden_container_reads_publish_once_on_return_without_losing_data(cx: &mut TestAppContext) {
    use std::{cell::Cell, rc::Rc};
    let (panel, cx, backend) = fixture(cx);
    let notifications = Rc::new(Cell::new(0));
    let observed = notifications.clone();
    let _observer =
        cx.update(|_, cx| cx.observe(&panel, move |_, _| observed.set(observed.get() + 1)));
    let mut next = snapshot();
    next.engine.version = "28.0.0".into();
    backend.set_snapshot(next);
    panel.update(cx, |panel, _| panel.client.refresh());
    for _ in 0..300 {
        panel.update(cx, |panel, cx| panel.poll(false, cx));
        cx.run_until_parked();
        if !panel.read_with(cx, |panel, _| panel.has_pending()) {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert!(!panel.read_with(cx, |panel, _| panel.has_pending()));
    assert_eq!(
        notifications.get(),
        0,
        "hidden reads should not repaint Containers"
    );
    assert_eq!(
        panel.read_with(cx, |panel, _| panel
            .client
            .snapshot()
            .unwrap()
            .engine
            .version
            .clone()),
        "28.0.0"
    );
    let requests = backend.count("containers_snapshot");
    panel.update(cx, |panel, cx| panel.poll(true, cx));
    draw(cx);
    assert_eq!(notifications.get(), 1, "publish retained changes on return");
    for _ in 0..3 {
        panel.update(cx, |panel, cx| panel.poll(true, cx));
    }
    cx.run_until_parked();
    assert_eq!(notifications.get(), 1);
    assert_eq!(
        backend.count("containers_snapshot"),
        requests,
        "return must reuse fresh data"
    );
}

#[gpui::test]
fn hidden_container_action_delivers_one_receipt_without_repainting_panel(cx: &mut TestAppContext) {
    use super::ContainersEvent;
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };
    let (panel, cx, backend) = fixture(cx);
    backend.hold_actions();
    select(&panel, cx, "c:stack-worker-1");
    click(cx, "Start");
    wait_for_held_action(&backend);
    let notifications = Rc::new(Cell::new(0));
    let observed = notifications.clone();
    let receipts = Rc::new(RefCell::new(Vec::new()));
    let received = receipts.clone();
    let _observer =
        cx.update(|_, cx| cx.observe(&panel, move |_, _| observed.set(observed.get() + 1)));
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&panel, move |_, event, _| {
            if let ContainersEvent::Notification { message } = event {
                received.borrow_mut().push(message.clone());
            }
        })
    });
    backend.release_action(json!({"ok":true}));
    for _ in 0..300 {
        panel.update(cx, |panel, cx| panel.poll(false, cx));
        cx.run_until_parked();
        if !panel.read_with(cx, |panel, _| panel.has_pending()) {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert!(!panel.read_with(cx, |panel, _| panel.has_pending()));
    assert_eq!(*receipts.borrow(), ["stack-worker-1: operation completed."]);
    assert_eq!(notifications.get(), 0);
    settle(&panel, cx);
    assert_eq!(
        *receipts.borrow(),
        ["stack-worker-1: operation completed."],
        "return must not repeat the receipt"
    );
    assert_eq!(backend.writes(), 1);
}

#[gpui::test]
fn lifecycle_dialog_rejects_retained_confirmation_after_external_close_without_polling(
    cx: &mut TestAppContext,
) {
    use gpui::component::WindowExt;
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "c:stack-worker-1");
    click_remove(cx);
    let (token, action) = panel.read_with(cx, |p, _| {
        (p.review_dialog_token, p.review.clone().unwrap())
    });
    let replacement = cx.update(|window, cx| {
        window.close_dialog(cx);
        window.open_dialog(cx, |dialog, _, _| dialog.title("Replacement dialog"));
        window.active_dialog_focus_handle(cx).unwrap()
    });
    // This is the same guarded entry point retained by the painted confirm
    // callback. No controller poll runs between replacement and activation.
    panel.update(cx, |p, cx| p.confirm_action_review(token, &action, cx));
    assert_eq!(backend.writes(), 0);
    assert!(!panel.read_with(cx, |p, _| p.has_review_dialog()));
    panel.update(cx, |p, cx| p.poll(false, cx));
    draw(cx);
    cx.update(|window, cx| {
        assert_eq!(
            window.active_dialog_focus_handle(cx).as_ref(),
            Some(&replacement)
        );
    });
}

#[gpui::test]
fn lifecycle_dialog_keyboard_confirmation_keeps_project_ids_and_submits_once(
    cx: &mut TestAppContext,
) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "p:stack");
    backend.hold_actions();
    click(cx, "Restart all");
    assert!(cx.debug_bounds("project-action-members").is_some());
    let focus = panel.read_with(cx, |p, _| p.review_confirm_focus.clone());
    cx.update(|window, cx| focus.focus(window, cx));
    key(cx, "enter");
    wait_for_held_action(&backend);
    key(cx, "enter");
    assert_eq!(backend.count("project_action"), 1);
    let request = backend.requests("project_action");
    assert_eq!(request[0]["project"], "stack");
    assert_eq!(request[0]["verb"], "restart");
    assert_eq!(
        request[0]["expectedIds"],
        json!(["a".repeat(64), "b".repeat(64)])
    );
    backend.fail_action("fixture restart failed");
    settle(&panel, cx);
    assert!(cx.debug_bounds("container-review-error").is_some());
}

#[gpui::test]
fn container_and_project_logs_fill_available_height_and_contain_scrolling(cx: &mut TestAppContext) {
    use gpui::{ScrollDelta, ScrollWheelEvent, point};
    let (panel, cx, backend) = fixture_with_size(cx, 980., 900.);
    let output = "Container stack-web-1 started\n";
    let mut job = json!({
        "job": "layout", "project": "stack", "command": "docker compose up -d",
        "state": "succeeded", "exitCode": 0, "startedAt": 1000, "finishedAt": 3000,
        "output": output, "offset": output.len(),
    });
    backend.set_compose_output("layout", Ok(job.clone()));
    job["output"] = json!("");
    job["offset"] = json!(0);
    backend.set_compose_jobs(Ok(json!({"jobs": [job]})));
    panel.update(cx, |p, _| p.client.retry_jobs());
    settle(&panel, cx);
    for (selection, tab, viewport_id, footer_id) in [
        (
            "c:stack-web-1",
            0,
            "container-log-viewport",
            "container-log-footer",
        ),
        (
            "p:stack",
            0,
            "container-log-viewport",
            "container-log-footer",
        ),
        (
            "p:stack",
            1,
            "compose-output-viewport",
            "compose-output-footer",
        ),
    ] {
        cx.simulate_resize(size(px(980.), px(900.)));
        select(&panel, cx, selection);
        panel.update(cx, |p, cx| {
            p.tab = tab;
            p.sync_selection(cx);
            cx.notify();
        });
        settle(&panel, cx);
        let before = cx.debug_bounds(viewport_id).unwrap();
        cx.simulate_resize(size(px(980.), px(1200.)));
        draw(cx);
        let after = cx.debug_bounds(viewport_id).unwrap();
        let detail = cx.debug_bounds("containers-detail").unwrap();
        let footer = cx.debug_bounds(footer_id).unwrap();
        assert!(
            (after.size.height - before.size.height - px(300.)).abs() <= px(1.),
            "{selection}: additional window height should belong to logs: {before:?} -> {after:?}"
        );
        assert!(
            (detail.bottom() - footer.bottom() - px(16.)).abs() <= px(1.),
            "{selection}: footer must reach the detail bottom: {detail:?}, {footer:?}"
        );
        assert_eq!(
            panel.read_with(cx, |p, _| p.detail_scroll.offset().y),
            px(0.)
        );

        // A short window may scroll the surrounding details, but wheel events
        // inside the log viewport must never move the service/project heading.
        cx.simulate_resize(size(px(980.), px(440.)));
        draw(cx);
        let detail = cx.debug_bounds("containers-detail").unwrap();
        let logs = cx.debug_bounds(viewport_id).unwrap();
        assert!(logs.size.height >= px(200.));
        panel.update(cx, |p, cx| {
            let reveal = (logs.top() - detail.top() - px(40.)).max(px(0.));
            p.detail_scroll
                .set_offset(point(px(0.), -reveal.min(p.detail_scroll.max_offset().y)));
            cx.notify();
        });
        draw(cx);
        let detail = cx.debug_bounds("containers-detail").unwrap();
        let logs = cx.debug_bounds(viewport_id).unwrap();
        let parent_offset = panel.read_with(cx, |p, _| p.detail_scroll.offset());
        let position = point(logs.center().x, logs.top().max(detail.top()) + px(4.));
        assert!(
            position.y < detail.bottom(),
            "short window must show the log area"
        );
        cx.simulate_mouse_move(position, None, Default::default());
        cx.simulate_event(ScrollWheelEvent {
            position,
            delta: ScrollDelta::Pixels(point(px(0.), px(-80.))),
            ..Default::default()
        });
        draw(cx);
        assert_eq!(
            panel.read_with(cx, |p, _| p.detail_scroll.offset()),
            parent_offset
        );
    }
    assert_eq!(backend.writes(), 0);
}
fn wait_for_held_action(backend: &FakeBackend) {
    for _ in 0..300 {
        if backend.held_actions() == 1 {
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!("cleanup did not reach isolated fixture");
}

#[gpui::test]
fn disk_navigation_renders_loading_then_typed_totals_without_writes(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    backend.hold_disk();
    cx.update(|window, cx| panel.update(cx, |p, cx| p.select("disk".into(), window, cx)));
    for _ in 0..300 {
        panel.update(cx, |p, cx| p.poll(true, cx));
        draw(cx);
        if backend.held_disks() == 1 {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(backend.held_disks(), 1);
    assert!(cx.debug_bounds("containers-disk").is_some());
    assert!(cx.debug_bounds("docker-disk-skeleton").is_some());
    assert!(!has_element(cx, "Clean up"));
    backend.release_disk();
    settle(&panel, cx);
    assert!(cx.debug_bounds("docker-disk-skeleton").is_none());
    assert!(has_element(cx, "Clean up"));
    assert!(has_element(cx, "Remove stopped"));
    assert!(has_element(cx, "Remove unused"));
    assert!(has_element(cx, "Clear"));
    assert!(cx.debug_bounds("docker-image-0").is_some());
    panel.read_with(cx, |p, _| {
        let disk = p.client.disk().unwrap();
        assert_eq!(disk.totals.images.size, 3_000_000);
        assert_eq!(disk.totals.build_cache.reclaimable, 1_500_000);
        assert!(disk.volumes[1].size.is_none());
        assert!(p.client.selected_container().is_none());
    });
    select(&panel, cx, "c:stack-web-1");
    select(&panel, cx, "disk");
    assert_eq!(
        backend.count("docker_disk"),
        1,
        "navigation should retain the fresh reading"
    );
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn every_cleanup_captures_its_target_and_requires_confirmation(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "disk");
    backend.hold_actions();
    for (index, (menu, target)) in [
        ("Remove stopped", PruneTarget::Containers),
        ("Remove dangling images (1)", PruneTarget::DanglingImages),
        ("Remove all unused images", PruneTarget::UnusedImages),
        ("Remove unused", PruneTarget::Volumes),
        ("Clear", PruneTarget::BuildCache),
    ]
    .into_iter()
    .enumerate()
    {
        if matches!(
            target,
            PruneTarget::DanglingImages | PruneTarget::UnusedImages
        ) {
            click(cx, "Clean up");
        }
        click(cx, menu);
        assert!(cx.debug_bounds("disk-review").is_some());
        assert!(cx.debug_bounds("dialog-layer").is_some());
        assert!(panel.read_with(cx, |p, _| p.has_review_dialog()));
        let warning = element(cx, super::disk::prune_copy(target).2);
        assert_eq!(warning.role(), Some(gpui::Role::Label));
        assert!(warning.visible());
        assert_eq!(
            backend.count("docker_prune"),
            index,
            "opening a review cannot mutate Docker"
        );
        click(cx, "Cancel");
        assert!(cx.debug_bounds("disk-review").is_none());
        assert_eq!(backend.count("docker_prune"), index);
        if matches!(
            target,
            PruneTarget::DanglingImages | PruneTarget::UnusedImages
        ) {
            click(cx, "Clean up");
        }
        click(cx, menu);
        click_selector(cx, "disk-confirm-prune");
        wait_for_held_action(&backend);
        click(cx, "Cancel");
        assert!(
            cx.debug_bounds("disk-review").is_some(),
            "Cancel cannot dismiss a running cleanup"
        );
        click_selector(cx, "disk-confirm-prune");
        assert_eq!(
            backend.count("docker_prune"),
            index + 1,
            "pending confirmation cannot duplicate a write"
        );
        assert_eq!(
            backend.requests("docker_prune").last().unwrap()["what"],
            target.as_str()
        );
        let disk_reads = backend.count("docker_disk");
        backend.release_action(json!({"removed":1,"reclaimed":1_000_000}));
        settle(&panel, cx);
        assert!(cx.debug_bounds("disk-review").is_none());
        assert!(
            backend.count("docker_disk") > disk_reads,
            "successful cleanup refreshes disk usage"
        );
    }
    assert_eq!(backend.writes(), 5);
}

#[gpui::test]
fn disk_review_closes_on_selection_change_and_pending_cleanup_stays_serialized(
    cx: &mut TestAppContext,
) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "disk");
    click(cx, "Clear");
    assert!(cx.debug_bounds("disk-review").is_some());
    select(&panel, cx, "c:stack-web-1");
    select(&panel, cx, "disk");
    assert!(cx.debug_bounds("disk-review").is_none());
    assert_eq!(backend.writes(), 0);
    backend.hold_actions();
    click(cx, "Clear");
    click_selector(cx, "disk-confirm-prune");
    wait_for_held_action(&backend);
    cx.update(|window, cx| panel.update(cx, |p, cx| p.select("c:stack-web-1".into(), window, cx)));
    draw(cx);
    assert!(panel.read_with(cx, |p, _| p.client.resource("action").unwrap().pending));
    click(cx, "Restart");
    assert_eq!(backend.count("container_action"), 0);
    cx.update(|window, cx| panel.update(cx, |p, cx| p.select("disk".into(), window, cx)));
    draw(cx);
    for label in ["Clean up", "Remove stopped", "Remove unused", "Clear"] {
        click(cx, label);
        assert!(
            cx.debug_bounds("disk-review").is_none(),
            "{label} must wait for the physical cleanup"
        );
    }
    assert_eq!(backend.requests("docker_prune")[0]["what"], "build-cache");
    backend.release_action(json!({"removed":3,"reclaimed":1_500_000}));
    settle(&panel, cx);
    assert_ne!(element(cx, "Clear").disabled(), Some(true));
    assert_eq!(backend.writes(), 1);
}

#[gpui::test]
fn failed_disk_read_has_an_explicit_retry_without_starting_cleanup(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    backend.fail_disk("Fixture disk read denied");
    select(&panel, cx, "disk");
    assert!(cx.debug_bounds("docker-disk-skeleton").is_none());
    assert!(!has_element(cx, "Clean up"));
    assert!(has_element(cx, "Retry disk usage"));
    assert_eq!(
        panel.read_with(cx, |p, _| p
            .client
            .resource("docker-disk")
            .unwrap()
            .error
            .clone()),
        Some("Fixture disk read denied".into())
    );
    let requests = backend.count("docker_disk");
    for _ in 0..5 {
        panel.update(cx, |p, cx| p.poll(true, cx));
        draw(cx);
    }
    assert_eq!(
        backend.count("docker_disk"),
        requests,
        "an unchanged failed pane must not retry on every poll"
    );
    backend.set_disk(disk_snapshot());
    click(cx, "Retry disk usage");
    settle(&panel, cx);
    assert!(has_element(cx, "Clean up"));
    assert_eq!(backend.count("docker_disk"), requests + 1);
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn failed_volume_measurement_stops_measuring_and_retries_only_volumes(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    let measured = disk_snapshot();
    let mut quick = measured.clone();
    quick.volumes_measured = false;
    quick.totals.volumes.size = 0;
    quick.totals.volumes.reclaimable = 0;
    for volume in &mut quick.volumes {
        volume.size = None;
    }
    backend.set_disk(quick);
    backend.set_disk_volumes(Err("Fixture volume scan timed out".into()));
    select(&panel, cx, "disk");
    assert!(panel.read_with(cx, |p, _| {
        p.client
            .resource("docker-disk-volumes")
            .is_some_and(|r| r.error.is_some() && !r.pending)
    }));
    let tile = element(cx, "Volumes: Unknown, 2 volumes");
    assert!(tile.bounds().size.width > px(0.));
    assert!(!has_element(cx, "Volumes: Measuring…, 2 volumes"));
    assert!(has_element(cx, "Retry measuring volumes"));
    let quick_reads = backend.count("docker_disk");
    backend.set_disk_volumes(Ok(serde_json::to_value(measured).unwrap()));
    click(cx, "Retry measuring volumes");
    settle(&panel, cx);
    assert_eq!(backend.count("docker_disk"), quick_reads + 1);
    assert_eq!(
        backend.requests("docker_disk").last().unwrap()["part"],
        "volumes"
    );
    assert!(!has_element(cx, "Retry measuring volumes"));
    // Sizes come from the measurement; totals are recomputed from them.
    assert!(has_element(cx, "Volumes: 976.6 KiB, 2 volumes"));
}

#[gpui::test]
fn failed_cleanup_keeps_its_review_and_retries_the_captured_target(cx: &mut TestAppContext) {
    use newport_client::containers::ActionTarget;
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "disk");
    backend.hold_actions();
    click(cx, "Clear");
    click_selector(cx, "disk-confirm-prune");
    wait_for_held_action(&backend);
    backend.fail_action("Fixture cleanup refused");
    settle(&panel, cx);
    assert!(cx.debug_bounds("disk-review").is_some());
    assert_eq!(
        panel.read_with(cx, |p, _| p
            .client
            .action_status(&ActionTarget::Prune(PruneTarget::BuildCache))
            .unwrap()
            .error
            .clone()),
        Some("Fixture cleanup refused".into())
    );
    assert_ne!(element(cx, "Clear cache").disabled(), Some(true));
    click_selector(cx, "disk-confirm-prune");
    wait_for_held_action(&backend);
    assert_eq!(
        backend
            .requests("docker_prune")
            .iter()
            .map(|args| args["what"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["build-cache", "build-cache"]
    );
    backend.release_action(json!({"removed":3,"reclaimed":1_500_000}));
    settle(&panel, cx);
    assert!(cx.debug_bounds("disk-review").is_none());
    assert_eq!(backend.writes(), 2);
}

#[gpui::test]
fn disk_image_inventory_keeps_rendered_rows_bounded(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    let mut disk = disk_snapshot();
    disk.images = (0..2_000)
        .map(|index| DockerImage {
            id: format!("sha256:{index:064x}"),
            tags: vec![format!("fixture/image-{index}:1")],
            size: 2_000_000 - index,
            created: 1_700_000_000_000,
            ..Default::default()
        })
        .collect();
    disk.totals.images.count = disk.images.len() as u64;
    backend.set_disk(disk);
    select(&panel, cx, "disk");
    assert_eq!(
        panel.read_with(cx, |p, _| p.client.disk().unwrap().images.len()),
        2_000
    );
    assert!(cx.debug_bounds("docker-image-0").is_some());
    assert!(
        cx.debug_bounds("docker-image-1999").is_none(),
        "far-away rows must stay unmounted"
    );
    panel.update(cx, |p, cx| {
        p.disk_view.list.scroll_to(gpui::ListOffset {
            item_ix: 1002,
            offset_in_item: px(0.),
        });
        cx.notify();
    });
    draw(cx);
    assert!(cx.debug_bounds("docker-image-1000").is_some());
    assert!(cx.debug_bounds("docker-image-0").is_none());
    panel.update(cx, |p, cx| {
        p.disk_view.list.scroll_to(gpui::ListOffset {
            item_ix: 2003,
            offset_in_item: px(0.),
        });
        cx.notify();
    });
    draw(cx);
    assert!(cx.debug_bounds("docker-volume-0").is_some());
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn compact_disk_confirmation_is_visible_focused_and_enter_dispatches_once(cx: &mut TestAppContext) {
    use gpui::{ScrollDelta, ScrollWheelEvent, point};
    let (panel, cx, backend) = fixture_with_size(cx, 560., 360.);
    select(&panel, cx, "disk");
    backend.hold_actions();
    let viewport = cx.debug_bounds("containers-disk").unwrap();
    // The summary is taller than this deliberately short viewport. Reveal the
    // opener through the inventory's real scroll path before activating it.
    cx.simulate_event(ScrollWheelEvent {
        position: viewport.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(-100.))),
        ..Default::default()
    });
    draw(cx);
    let opener = element(cx, "Remove stopped").bounds();
    assert!(opener.top() >= viewport.top() && opener.bottom() <= viewport.bottom());
    click(cx, "Remove stopped");
    let viewport = cx
        .update(|window, _| gpui::Bounds::new(gpui::point(px(0.), px(0.)), window.viewport_size()));
    let review = cx.debug_bounds("disk-review").unwrap();
    let confirm = cx.debug_bounds("disk-confirm-prune").unwrap();
    assert!(
        review.top() >= viewport.top(),
        "review heading must be visible: {review:?} in {viewport:?}"
    );
    assert!(
        confirm.top() >= viewport.top() && confirm.bottom() <= viewport.bottom(),
        "confirmation must fit the short viewport: {confirm:?} in {viewport:?}"
    );
    cx.update(|window, cx| assert!(panel.read(cx).review_focus.is_focused(window)));
    assert!(cx.debug_bounds("dialog-layer").is_some());
    let focus = panel.read_with(cx, |p, _| p.review_confirm_focus.clone());
    cx.update(|window, cx| focus.focus(window, cx));
    assert_eq!(backend.writes(), 0);
    let keystroke = Keystroke::parse("enter").unwrap();
    cx.simulate_event(KeyDownEvent {
        physical_key: None,
        key_metadata: None,
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    cx.simulate_event(gpui::KeyUpEvent {
        physical_key: None,
        key_metadata: None,
        keystroke,
    });
    draw(cx);
    wait_for_held_action(&backend);
    let keystroke = Keystroke::parse("enter").unwrap();
    cx.simulate_event(KeyDownEvent {
        physical_key: None,
        key_metadata: None,
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    cx.simulate_event(gpui::KeyUpEvent {
        physical_key: None,
        key_metadata: None,
        keystroke,
    });
    draw(cx);
    click(cx, "Cancel");
    assert!(cx.debug_bounds("disk-review").is_some());
    assert_eq!(backend.requests("docker_prune").len(), 1);
    assert_eq!(backend.requests("docker_prune")[0]["what"], "containers");
    backend.release_action(json!({"removed":2,"reclaimed":500_000}));
    settle(&panel, cx);
    assert!(cx.debug_bounds("disk-review").is_none());
    assert_eq!(
        panel.read_with(cx, |p, _| p.disk_view.list.logical_scroll_top().item_ix),
        0
    );
    assert_eq!(backend.writes(), 1);
}

#[gpui::test]
fn disk_refresh_preserves_visible_image_anchor_after_sorting_and_insertion(
    cx: &mut TestAppContext,
) {
    let (panel, cx, backend) = fixture(cx);
    let mut disk = disk_snapshot();
    disk.images = (0..80)
        .map(|index| DockerImage {
            id: format!("sha256:{index:064x}"),
            tags: vec![format!("fixture/image-{index}:1")],
            size: 2_000_000 - index,
            created: 1_700_000_000_000,
            ..Default::default()
        })
        .collect();
    disk.totals.images.count = 80;
    backend.set_disk(disk.clone());
    select(&panel, cx, "disk");
    panel.update(cx, |p, cx| {
        p.disk_view.list.scroll_to(gpui::ListOffset {
            item_ix: 42,
            offset_in_item: px(5.),
        });
        cx.notify();
    });
    draw(cx);
    let anchor = cx.debug_bounds("docker-image-40").unwrap();
    let before = panel.read_with(cx, |p, _| p.disk_view.list.logical_scroll_top());
    assert_eq!(before.item_ix, 42);
    disk.images[65].size = 4_000_000;
    disk.images.push(DockerImage {
        id: format!("sha256:{}", "f".repeat(64)),
        tags: vec!["fixture/new-image:1".into()],
        size: 3_000_000,
        created: 1_700_000_000_000,
        ..Default::default()
    });
    disk.totals.images.count = 81;
    backend.set_disk(disk);
    panel.update(cx, |p, _| p.client.retry_disk());
    settle(&panel, cx);
    let after = panel.read_with(cx, |p, _| p.disk_view.list.logical_scroll_top());
    assert_eq!(
        after.item_ix, 44,
        "the same image moved two rows after sorting and insertion"
    );
    assert_eq!(after.offset_in_item, before.offset_in_item);
    assert_eq!(
        cx.debug_bounds("docker-image-40").unwrap().top(),
        anchor.top()
    );
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn disk_header_and_table_columns_fill_the_inventory_width(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "disk");
    let viewport = cx.debug_bounds("docker-inventory-table").unwrap();
    let image = cx.debug_bounds("docker-image-0").unwrap();
    let volume = cx.debug_bounds("docker-volume-0").unwrap();
    for selector in ["disk-header", "disk-columns-images", "disk-columns-volumes"] {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("missing {selector}"));
        assert_eq!(
            bounds.left(),
            viewport.left(),
            "{selector} should align with the pane"
        );
        assert_eq!(
            bounds.size.width, image.size.width,
            "{selector} should fill inventory width"
        );
        assert_eq!(
            bounds.size.width, viewport.size.width,
            "{selector} should not shrink to its content"
        );
    }
    assert_eq!(image.size.width, volume.size.width);
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn cleanup_modal_blocks_background_and_escape_cancels_without_writes(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "disk");
    let background = element(cx, "Remove stopped").bounds().center();
    let offset = panel.read_with(cx, |p, _| p.disk_view.list.logical_scroll_top());
    click(cx, "Clear");
    assert!(cx.debug_bounds("dialog-layer").is_some());
    cx.simulate_click(background, Default::default());
    draw(cx);
    assert_eq!(
        panel.read_with(cx, |p, _| p.review.clone()),
        Some(Action::Prune {
            what: PruneTarget::BuildCache
        })
    );
    assert_eq!(backend.writes(), 0);
    key(cx, "escape");
    assert!(!panel.read_with(cx, |p, _| p.has_review_dialog()));
    assert!(cx.debug_bounds("disk-review").is_none());
    let after = panel.read_with(cx, |p, _| p.disk_view.list.logical_scroll_top());
    assert_eq!(after.item_ix, offset.item_ix);
    assert_eq!(after.offset_in_item, offset.offset_in_item);
    cx.update(|window, cx| assert!(panel.read(cx).detail_focus.is_focused(window)));
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
#[ignore = "manual Containers CPU frame benchmark; excludes GPU presentation and SSH"]
fn benchmark_containers_frames(cx: &mut TestAppContext) {
    use std::time::Instant;
    for (count, per_project) in [(50, 10), (1000, 10), (1000, 1000)] {
        let mut data = snapshot();
        let template = data.containers[0].clone();
        data.containers = (0..count)
            .map(|i| Container {
                id: format!("{i:064x}"),
                name: format!("worker-{i:04}"),
                compose: Some(ComposeLabels {
                    project: format!("project-{:03}", i / per_project),
                    service: format!("worker-{}", i % per_project),
                    ..Default::default()
                }),
                ..template.clone()
            })
            .collect();
        let (panel, cx, _backend) = fixture_with_snapshot(cx, data, 980., 720.);
        draw(cx);
        let start = Instant::now();
        for _ in 0..30 {
            panel.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
        eprintln!(
            "Containers {count} containers, {per_project} per project at 980x720: {:.3} ms/CPU frame",
            start.elapsed().as_secs_f64() * 1000. / 30.
        );
    }
}

#[gpui::test]
fn prepared_list_tracks_refresh_search_and_project_expansion(cx: &mut TestAppContext) {
    use std::rc::Rc;
    let (panel, cx, backend) = fixture(cx);
    let initial = panel.read_with(cx, |p, cx| p.rows(cx));
    draw(cx);
    assert!(Rc::ptr_eq(
        &initial,
        &panel.read_with(cx, |p, cx| p.rows(cx))
    ));
    panel.update(cx, |p, cx| {
        p.jobs_open.insert("stack".into());
        cx.notify();
    });
    let jobs = panel.read_with(cx, |p, cx| p.rows(cx));
    assert!(!Rc::ptr_eq(&initial, &jobs));
    assert!(
        jobs.iter()
            .any(|row| row.key.as_deref() == Some("c:stack-run-1"))
    );
    panel.update(cx, |p, cx| {
        p.collapsed.insert("stack".into());
        cx.notify();
    });
    let collapsed = panel.read_with(cx, |p, cx| p.rows(cx));
    assert!(!Rc::ptr_eq(&jobs, &collapsed));
    assert!(collapsed.len() < jobs.len());
    cx.update(|window, cx| {
        panel.update(cx, |p, cx| {
            p.search
                .update(cx, |input, cx| input.set_value("fixture/web", window, cx));
        })
    });
    let searched = panel.read_with(cx, |p, cx| p.rows(cx));
    assert!(
        searched
            .iter()
            .any(|row| row.key.as_deref() == Some("c:stack-web-1"))
    );
    let mut data = snapshot();
    data.containers
        .retain(|container| container.name != "stack-web-1");
    backend.set_snapshot(data);
    panel.update(cx, |p, cx| p.refresh(cx));
    assert!(Rc::ptr_eq(
        &searched,
        &panel.read_with(cx, |p, cx| p.rows(cx))
    ));
    settle(&panel, cx);
    let refreshed = panel.read_with(cx, |p, cx| p.rows(cx));
    assert!(!Rc::ptr_eq(&searched, &refreshed));
    assert!(
        !refreshed
            .iter()
            .any(|row| row.key.as_deref() == Some("c:stack-web-1"))
    );
}

#[test]
fn sections_fold_and_a_search_opens_them() {
    let source = snapshot();
    let folded = HashSet::from([
        "section:Projects".to_owned(),
        "section:Containers".to_owned(),
    ]);
    let rows = master_rows(&source.containers, "", &folded, &HashSet::new(), EVERY, 0);
    assert!(rows.iter().all(|row| row.group && row.project.is_none()));
    assert!(rows.iter().any(|row| row.name == "Projects" && !row.open));
    let rows = master_rows(
        &source.containers,
        "web",
        &folded,
        &HashSet::new(),
        EVERY,
        0,
    );
    assert!(
        rows.iter()
            .any(|row| row.key.as_deref() == Some("c:stack-web-1"))
    );
}
