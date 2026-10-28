use super::{Selection, ServicesPanel, master_rows};
use gpui::{Context, IntoElement, ParentElement, Render, Styled, Window, div};
use gpui_kit as gpui;
use gpui_kit::base::test_support::{ElementSnapshot, snapshots};
use gpui_kit::{
    AppContext, Entity, KeyDownEvent, Keystroke, TestAppContext, VisualTestContext, px, size,
};
use newport_client::{backend::Backend, services::Services};
use newport_domain::services::{
    JournalEntry, JournalPage, Privilege, Scope, ServiceUnit, ServicesSnapshot, UserAvailability,
};
use newport_platform::ipc::{
    Listener, Stream,
    protocol::{Request, Response, VERSION},
};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

/// A strict local RPC peer. Tests never discover the installed app or server.
/// Action sockets can be held to exercise view lifetime and physical ownership.
pub(crate) struct FakeBackend {
    path: PathBuf,
    calls: Arc<Mutex<Vec<Request>>>,
    snapshot: Arc<Mutex<ServicesSnapshot>>,
    held: Arc<Mutex<Vec<Stream>>>,
    hold: Arc<AtomicBool>,
    hold_reads: Arc<AtomicBool>,
    pending_reads: Arc<Mutex<Vec<(String, Stream, Value)>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl FakeBackend {
    pub(crate) fn new(snapshot: ServicesSnapshot) -> Self {
        let path = std::env::temp_dir().join(format!("np-services-{}.sock", uuid::Uuid::new_v4()));
        let listener = Listener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let calls = Arc::new(Mutex::new(vec![]));
        let source = Arc::new(Mutex::new(snapshot));
        let held = Arc::new(Mutex::new(vec![]));
        let hold = Arc::new(AtomicBool::new(false));
        let hold_reads = Arc::new(AtomicBool::new(false));
        let pending_reads = Arc::new(Mutex::new(vec![]));
        let (read_gate, read_streams) = (hold_reads.clone(), pending_reads.clone());
        let stop = Arc::new(AtomicBool::new(false));
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
                let mut header = [0u8; 4];
                stream.read_exact(&mut header).unwrap();
                let len = u32::from_be_bytes(header) as usize;
                assert!(len < 1024 * 1024);
                let mut bytes = vec![0; len];
                stream.read_exact(&mut bytes).unwrap();
                let request: Request = rmp_serde::from_slice(&bytes).unwrap();
                assert_eq!(request.version, VERSION);
                assert_eq!(request.args["id"], "fixture");
                let result = match request.method.as_str() {
                    "services_snapshot" => serde_json::to_value(&*data.lock().unwrap()).unwrap(),
                    "service_unit_file" => {
                        json!({"text":"[Unit]\nDescription=Fixture service\n[Service]\nExecStart=/usr/bin/true\n"})
                    }
                    "service_logs" => serde_json::to_value(JournalPage {
                        entries: (0..if request.args["unit"]=="nixos-upgrade.service" {200}else{40})
                            .map(|i| JournalEntry {
                                at: Some(1_700_000_000_000 + i * 1000),
                                priority: Some(if i % 3 == 0 { 3 } else { 6 }),
                                pid: Some(99),
                                identifier: Some("fixture".into()),
                                message: if request.args["unit"]=="nixos-upgrade.service" {format!("Log entry {i}: {}", "/nix/store/c7f69b19837d3a1f83-server-system-runtime/bin/nixos-rebuild --upgrade --flake /etc/nixos#sample-server ".repeat(10))} else {format!("Log entry {i}")},
                            })
                            .collect(),
                        cursor: Some("tail".into()),
                    })
                    .unwrap(),
                    "service_action" => json!({"ok":true}),
                    other => panic!("unexpected request {other}"),
                };
                let action = request.method == "service_action";
                let method = request.method.clone();
                call_store.lock().unwrap().push(request);
                if action && should_hold.load(Ordering::Acquire) {
                    held_streams.lock().unwrap().push(stream);
                } else if !action && read_gate.load(Ordering::Acquire) {
                    read_streams.lock().unwrap().push((method, stream, result));
                } else {
                    reply(&mut stream, result);
                }
            }
        });
        Self {
            path,
            calls,
            snapshot: source,
            held,
            hold,
            hold_reads,
            pending_reads,
            stop,
            worker: Some(worker),
        }
    }
    pub(crate) fn client(&self) -> Services {
        Services::new(Backend::at_path(self.path.clone()), "fixture".into())
    }
    fn hold_reads(&self) {
        self.hold_reads.store(true, Ordering::Release);
    }
    fn held_reads(&self, method: &str) -> usize {
        self.pending_reads
            .lock()
            .unwrap()
            .iter()
            .filter(|(key, _, _)| key == method)
            .count()
    }
    fn release_read(&self, method: &str) {
        let mut pending = self.pending_reads.lock().unwrap();
        if let Some(index) = pending.iter().position(|(key, _, _)| key == method) {
            let (_, mut stream, value) = pending.remove(index);
            reply(&mut stream, value);
        }
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
    pub(crate) fn set_snapshot(&self, snapshot: ServicesSnapshot) {
        *self.snapshot.lock().unwrap() = snapshot;
    }
}
fn reply(stream: &mut Stream, result: Value) {
    let bytes = rmp_serde::to_vec_named(&Response {
        version: VERSION,
        result,
        error: None,
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
        if let Some(worker) = self.worker.take() {
            let result = worker.join();
            if !thread::panicking() {
                result.unwrap();
            }
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

pub(crate) fn snapshot() -> ServicesSnapshot {
    ServicesSnapshot {
        services: vec![
            ServiceUnit {
                name: "app.service".into(),
                scope: Scope::System,
                description: "Customer application".into(),
                load: "loaded".into(),
                active: "active".into(),
                sub: "running".into(),
                enabled: Some("enabled".into()),
                main_pid: Some(42),
                user: Some("app".into()),
                memory: Some(1024),
                cpu: Some(2.5),
                ..Default::default()
            },
            ServiceUnit {
                name: "worker.service".into(),
                scope: Scope::User,
                description: "Background jobs".into(),
                load: "loaded".into(),
                active: "inactive".into(),
                sub: "dead".into(),
                enabled: Some("disabled".into()),
                ..Default::default()
            },
            ServiceUnit {
                name: "systemd-logind.service".into(),
                load: "loaded".into(),
                active: "active".into(),
                sub: "running".into(),
                ..Default::default()
            },
        ],
        timers: vec![],
        user: UserAvailability {
            available: true,
            reason: None,
        },
        privilege: Privilege::Sudo,
    }
}
pub(crate) fn fixture(
    cx: &mut TestAppContext,
) -> (Entity<ServicesPanel>, &mut VisualTestContext, FakeBackend) {
    fixture_with_snapshot(cx, snapshot())
}
fn fixture_with_snapshot(
    cx: &mut TestAppContext,
    snapshot: ServicesSnapshot,
) -> (Entity<ServicesPanel>, &mut VisualTestContext, FakeBackend) {
    fixture_with_size(cx, snapshot, 898., 720.)
}
fn fixture_with_size(
    cx: &mut TestAppContext,
    snapshot: ServicesSnapshot,
    width: f32,
    height: f32,
) -> (Entity<ServicesPanel>, &mut VisualTestContext, FakeBackend) {
    fixture_staged(cx, snapshot, width, height, false)
}
fn fixture_staged(
    cx: &mut TestAppContext,
    snapshot: ServicesSnapshot,
    width: f32,
    height: f32,
    staged: bool,
) -> (Entity<ServicesPanel>, &mut VisualTestContext, FakeBackend) {
    let backend = FakeBackend::new(snapshot);
    if staged {
        backend.hold_reads();
    }
    let client = backend.client();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        crate::theme::install(false, cx);
        cx.set_reduce_motion(true);
    });
    let mut panel = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let content = cx.new(|cx| ServicesPanel::with_client(client, window, cx));
        panel = Some(content.clone());
        let shell = cx.new(|_| FixtureShell(content));
        gpui_kit::base::Root::new(shell, window, cx)
    });
    cx.update(|window, _| window.activate_window());
    cx.simulate_resize(size(px(width), px(height)));
    let panel = panel.unwrap();
    if staged {
        draw(cx);
    } else {
        settle(&panel, cx);
    }
    (panel, cx, backend)
}
struct FixtureShell(Entity<ServicesPanel>);
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

#[gpui::test]
fn initial_failed_service_heading_stays_inside_detail_viewport(cx: &mut TestAppContext) {
    use newport_domain::services::TimerUnit;
    let now = chrono::Local::now().timestamp_millis();
    let mut data = snapshot();
    data.services = vec![ServiceUnit {
        name: "nixos-upgrade.service".into(),
        description: "NixOS Upgrade".into(),
        load: "loaded".into(),
        active: "failed".into(),
        sub: "failed".into(),
        enabled: Some("linked".into()),
        since: Some(now - 58_380_000),
        exit_code: Some(1),
        exit_kind: Some("exited".into()),
        result: Some("exit-code".into()),
        triggered_by: vec!["nixos-upgrade.timer".into()],
        ..Default::default()
    }];
    data.timers = vec![TimerUnit {
        name: "nixos-upgrade.timer".into(),
        scope: Scope::System,
        description: "NixOS Upgrade Timer".into(),
        active: "active".into(),
        next: Some(now + 3_600_000),
        last: Some(now - 58_380_000),
        unit: Some("nixos-upgrade.service".into()),
    }];
    let (panel, cx, backend) = fixture_staged(cx, data, 921., 730., true);
    draw(cx);
    for _ in 0..100 {
        if backend.held_reads("services_snapshot") == 1 {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(backend.held_reads("services_snapshot"), 1);
    assert!(cx.debug_bounds("services-detail-heading").is_none());
    backend.release_read("services_snapshot");
    for _ in 0..100 {
        panel.update(cx, |p, cx| p.poll(true, cx));
        draw(cx);
        if backend.held_reads("service_logs") == 1 {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(backend.held_reads("service_logs"), 1);
    let loading_heading = cx.debug_bounds("services-detail-heading").unwrap();
    let loading_viewport = cx.debug_bounds("services-detail").unwrap();
    assert!(loading_heading.top() >= loading_viewport.top());
    backend.release_read("service_logs");
    settle(&panel, cx);
    let viewport = cx.debug_bounds("services-detail").unwrap();
    let heading = cx.debug_bounds("services-detail-heading").unwrap();
    let offset = panel.read_with(cx, |p, _| p.detail_scroll.offset());
    assert_eq!(
        panel.read_with(cx, |p, _| p.client.logs().unwrap().entries.len()),
        200
    );
    assert!(
        heading.size.height >= px(20.),
        "heading collapsed: {heading:?}"
    );
    assert_eq!(offset.y, px(0.), "initial detail scroll moved: {offset:?}");
    assert!(
        heading.top() >= viewport.top(),
        "heading {heading:?} clipped by detail viewport {viewport:?}, offset={offset:?}"
    );
    let timer = cx.update(|window, _| {
        snapshots(window)
            .into_iter()
            .find(|element| {
                element
                    .label()
                    .is_some_and(|label| label == "nixos-upgrade.timer")
            })
            .unwrap()
    });
    assert!(timer.bounds().left() >= viewport.left() + px(16.));
    assert!(timer.bounds().right() <= viewport.right() - px(16.));
    assert!(
        timer.bounds().size.width < viewport.size.width / 2.,
        "timer link should keep its natural width: {:?}",
        timer.bounds()
    );
}
pub(crate) fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
}
pub(crate) fn settle(panel: &Entity<ServicesPanel>, cx: &mut VisualTestContext) {
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
fn element(cx: &mut VisualTestContext, label: &str) -> ElementSnapshot {
    cx.update(|window, _| {
        snapshots(window)
            .into_iter()
            .find(|e| e.label() == Some(label))
            .unwrap_or_else(|| panic!("missing {label}"))
    })
}
fn click(cx: &mut VisualTestContext, label: &str) {
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

#[gpui::test]
fn reconnect_restores_choices_without_carrying_remote_state(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.selected = Some(Selection::Unit("system:app.service".into()));
            panel.system_open = true;
            panel.tab = 1;
            panel
                .search
                .update(cx, |input, cx| input.set_value("worker", window, cx));
            panel
                .logs
                .restore_preferences("warning".into(), false, "entry 2".into(), window, cx);
        })
    });
    settle(&panel, cx);
    let choices = panel.read_with(cx, |panel, cx| panel.preferences(cx));
    assert_eq!(choices.query, "worker");
    assert_eq!(choices.log_query, "entry 2");
    let replacement = FakeBackend::new(ServicesSnapshot::default());
    replacement.hold_reads();
    let restored = cx.update(|window, cx| {
        cx.new(|cx| {
            let mut panel = ServicesPanel::with_client(replacement.client(), window, cx);
            panel.restore_preferences(choices.clone(), window, cx);
            panel
        })
    });
    restored.read_with(cx, |panel, cx| {
        assert_eq!(panel.preferences(cx), choices);
        assert!(panel.client.snapshot().is_none());
        assert!(panel.client.logs().is_none());
        assert!(panel.review.is_none());
        assert_eq!(panel.snapshot_revision, 0);
        assert_eq!(panel.active_data, super::ActiveData::None);
    });
    assert_eq!(backend.count("service_action"), 0);
    assert_eq!(replacement.count("service_action"), 0);
}

#[gpui::test]
fn desktop_geometry_filter_retains_selected_detail_and_reads_no_actions(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    let master = cx.debug_bounds("services-master").unwrap();
    let detail = cx.debug_bounds("services-detail").unwrap();
    assert!(master.size.width >= px(210.) && master.size.width <= px(280.));
    assert_eq!(detail.left(), master.right());
    assert!(detail.size.width > px(500.));
    panel.update(cx, |panel, cx| {
        panel.select(Selection::Unit("system:app.service".into()), cx)
    });
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel
                .search
                .update(cx, |input, cx| input.set_value("worker", window, cx))
        })
    });
    draw(cx);
    assert_eq!(
        panel.read_with(cx, |p, cx| p.effective_selection(cx)),
        Selection::Unit("system:app.service".into())
    );
    assert!(element(cx, "Restart").bounds().size.width > px(0.));
    assert_eq!(backend.count("service_action"), 0);
}
#[gpui::test]
fn compact_list_detail_back_and_arrow_navigation_are_operable(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    cx.simulate_resize(size(px(540.), px(720.)));
    draw(cx);
    assert!(cx.debug_bounds("services-master").is_some());
    assert!(cx.debug_bounds("services-detail").is_none());
    cx.update(|window, cx| panel.read(cx).list_focus.clone().focus(window, cx));
    key(cx, "down");
    assert_eq!(
        panel.read_with(cx, |p, _| p.selected.clone()),
        Some(Selection::Unit("user:worker.service".into()))
    );
    assert!(cx.debug_bounds("services-master").is_none());
    assert!(cx.debug_bounds("services-detail").is_some());
    assert!(cx.update(|window, cx| panel.read(cx).detail_focus.is_focused(window)));
    key(cx, "tab");
    assert_eq!(element(cx, "All services").focused(), Some(true));
    click(cx, "All services");
    assert!(panel.read_with(cx, |p, _| p.selected.is_none()));
    assert!(cx.debug_bounds("services-master").is_some());
    assert!(cx.update(|window, cx| panel.read(cx).list_focus.is_focused(window)));
    assert_eq!(backend.count("service_action"), 0);
}
#[gpui::test]
fn system_services_reveal_from_the_list_and_the_filter(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    assert!(!panel.read_with(cx, |p, _| p.system_open));
    click(cx, "1 system service · Show all");
    assert!(panel.read_with(cx, |p, _| p.system_open));
    assert!(cx.update(|window, _| {
        !snapshots(window)
            .iter()
            .any(|e| e.label() == Some("1 system service · Show all"))
    }));
    let choice = |cx: &mut VisualTestContext, name: &str| {
        let at = cx.update(|window, _| {
            snapshots(window)
                .into_iter()
                .find(|e| {
                    e.label()
                        .is_some_and(|l| l.starts_with(&format!("{name} (")))
                })
                .unwrap_or_else(|| panic!("missing {name}"))
                .bounds()
                .center()
        });
        cx.simulate_click(at, Default::default());
        draw(cx);
    };
    choice(cx, "Yours");
    assert!(!panel.read_with(cx, |p, _| p.system_open));
    choice(cx, "All");
    assert!(panel.read_with(cx, |p, _| p.system_open));
    click(cx, "System journal");
    assert_eq!(
        panel.read_with(cx, |p, _| p.selected.clone()),
        Some(Selection::Journal)
    );
    assert_eq!(backend.count("service_action"), 0);
}
#[gpui::test]
fn destructive_action_requires_review_and_retains_one_write_owner(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    backend.hold_actions();
    click(cx, "Stop");
    assert!(panel.read_with(cx, |p, _| p.review.is_some()));
    assert_eq!(backend.count("service_action"), 0);
    click(cx, "Cancel");
    assert!(panel.read_with(cx, |p, _| p.review.is_none()));
    click(cx, "Stop");
    let review = cx.update(|window, _| {
        snapshots(window)
            .into_iter()
            .rfind(|e| e.label() == Some("Confirm stop"))
            .unwrap()
            .bounds()
    });
    cx.simulate_click(review.center(), Default::default());
    draw(cx);
    for _ in 0..100 {
        if backend.held_actions() == 1 {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(backend.count("service_action"), 1);
    assert!(panel.read_with(cx, |p, _| p.pending("action")));
    // A second explicit attempt while the first RPC is retained cannot dispatch.
    panel.update(cx, |p, cx| {
        p.start_action(
            "app.service".into(),
            Scope::System,
            newport_domain::services::ServiceVerb::Stop,
            cx,
        )
    });
    assert_eq!(backend.count("service_action"), 1);
    backend.release_action(json!({"ok":true}));
    settle(&panel, cx);
    assert!(panel.read_with(cx, |p, _| p.review.is_none()));
}
#[test]
fn groups_search_system_visibility_and_uniform_row_shape() {
    let snapshot = snapshot();
    let rows = master_rows(&snapshot, "", false, &Default::default(), 0);
    assert!(!rows.iter().any(|r| r.name == "System"));
    assert!(!rows.iter().any(|r| r.name == "systemd-logind"));
    assert!(
        rows.iter()
            .any(|r| r.reveal && r.name == "1 system service · Show all" && r.key.is_none())
    );
    assert!(
        !rows.iter().any(|r| r.key == Some(Selection::Journal)),
        "the journal opens from the toolbar"
    );
    let rows = master_rows(&snapshot, "", true, &Default::default(), 0);
    assert!(
        rows.iter()
            .any(|r| r.heading && r.name == "System" && r.count == 1)
    );
    assert!(rows.iter().any(|r| r.name == "systemd-logind"));
    assert!(!rows.iter().any(|r| r.reveal));
    let rows = master_rows(&snapshot, "logind", false, &Default::default(), 0);
    assert!(rows.iter().any(|r| r.name == "systemd-logind"));
    let folded = std::collections::BTreeSet::from(["Services".to_owned()]);
    let rows = master_rows(&snapshot, "", false, &folded, 0);
    assert!(
        rows.iter()
            .any(|r| r.heading && r.name == "Services" && !r.open)
    );
    assert!(!rows.iter().any(|r| r.name == "app"));
    let rows = master_rows(&snapshot, "app", false, &folded, 0);
    assert!(
        rows.iter().any(|r| r.name == "app"),
        "a search opens folded groups"
    );

    let empty = ServicesSnapshot::default();
    for query in ["", "  "] {
        let rows = master_rows(&empty, query, false, &Default::default(), 0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "No services or timers here.");
    }
    for data in [&snapshot, &empty] {
        let rows = master_rows(data, "missing-service", false, &Default::default(), 0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "No services or timers match.");
        assert_eq!(rows[0].key, None, "empty copy is not a selectable service");
    }
}

#[gpui::test]
fn unit_file_is_read_only_cached_and_resets_editor_for_another_unit(cx: &mut TestAppContext) {
    use gpui::Focusable;
    let (panel, cx, backend) = fixture(cx);
    click(cx, "Unit file");
    settle(&panel, cx);
    let editor = panel.read_with(cx, |p, _| p.logs.editor.clone());
    let before = editor.read_with(cx, |editor, _| editor.value().to_string());
    assert!(before.contains("ExecStart"));
    assert_eq!(backend.count("service_unit_file"), 1);
    cx.update(|window, cx| editor.read(cx).focus_handle(cx).focus(window, cx));
    cx.simulate_input("should not be inserted");
    draw(cx);
    assert_eq!(
        editor.read_with(cx, |editor, _| editor.value().to_string()),
        before
    );
    for _ in 0..4 {
        panel.update(cx, |p, cx| p.poll(true, cx));
        draw(cx);
    }
    assert_eq!(backend.count("service_unit_file"), 1);
    panel.update(cx, |p, cx| {
        p.select(Selection::Unit("user:worker.service".into()), cx)
    });
    settle(&panel, cx);
    let next = panel.read_with(cx, |p, _| p.logs.editor.clone());
    assert_ne!(editor.entity_id(), next.entity_id());
    assert_eq!(
        next.read_with(cx, |editor, _| editor.value().to_string()),
        before
    );
    assert_eq!(backend.count("service_action"), 0);
}
#[gpui::test]
fn large_master_virtualizes_and_keyboard_end_reveals_selection(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    let mut data = snapshot();
    data.services = (0..1000)
        .map(|i| ServiceUnit {
            name: format!("app-{i:04}.service"),
            load: "loaded".into(),
            active: "active".into(),
            sub: "running".into(),
            ..Default::default()
        })
        .collect();
    backend.set_snapshot(data);
    panel.update(cx, |p, cx| p.refresh(cx));
    settle(&panel, cx);
    assert!(cx.debug_bounds("services-row-1").is_some());
    assert!(cx.debug_bounds("services-row-499").is_none());
    assert!(cx.debug_bounds("services-row-1000").is_none());
    cx.update(|window, cx| panel.read(cx).list_focus.clone().focus(window, cx));
    key(cx, "end");
    assert_eq!(
        panel.read_with(cx, |p, _| p.selected.clone()),
        Some(Selection::Unit("system:app-0999.service".into()))
    );
    let last = cx.debug_bounds("services-row-1000").unwrap();
    let viewport = cx.debug_bounds("services-master").unwrap();
    assert!(last.bottom() <= viewport.bottom());
    key(cx, "home");
    assert_eq!(
        panel.read_with(cx, |p, _| p.selected.clone()),
        Some(Selection::Unit("system:app-0000.service".into()))
    );
    assert_eq!(backend.count("service_action"), 0);
}

#[gpui::test]
fn pending_action_keeps_other_unit_controls_usable_and_receipts_scoped(cx: &mut TestAppContext) {
    use super::ServicesEvent;
    use std::{cell::RefCell, rc::Rc};
    let (panel, cx, backend) = fixture(cx);
    backend.hold_actions();
    let notifications = Rc::new(RefCell::new(Vec::new()));
    let received = notifications.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&panel, move |_, event, _| {
            if let ServicesEvent::Notification { message } = event {
                received.borrow_mut().push(message.clone());
            }
        })
    });
    click(cx, "Restart");
    for _ in 0..100 {
        if backend.held_actions() == 1 {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(backend.held_actions(), 1);
    panel.update(cx, |p, cx| {
        p.select(Selection::Unit("user:worker.service".into()), cx)
    });
    draw(cx);
    assert_ne!(element(cx, "Start").disabled(), Some(true));
    click(cx, "Start");
    for _ in 0..100 {
        if backend.held_actions() == 2 {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(backend.held_actions(), 2);
    assert_eq!(backend.count("service_action"), 2);
    backend.release_action(json!({"ok":true}));
    settle(&panel, cx);
    let mut notices = notifications.borrow().clone();
    notices.sort();
    assert_eq!(notices, vec!["Restarted app", "Started worker"]);
    panel.update(cx, |p, cx| p.poll(true, cx));
    draw(cx);
    assert_eq!(
        notifications.borrow().len(),
        2,
        "completion receipts must not toast twice"
    );
}

#[gpui::test]
fn hidden_initial_service_snapshot_defers_log_selection(cx: &mut TestAppContext) {
    hidden_initial_snapshot_defers_details(cx, 0, "service_logs");
}

#[gpui::test]
fn hidden_initial_service_snapshot_defers_file_selection(cx: &mut TestAppContext) {
    hidden_initial_snapshot_defers_details(cx, 1, "service_unit_file");
}

fn hidden_initial_snapshot_defers_details(cx: &mut TestAppContext, tab: usize, method: &str) {
    use std::{cell::Cell, rc::Rc};
    let (panel, cx, backend) = fixture_staged(cx, snapshot(), 898., 720., true);
    panel.update(cx, |panel, _| panel.tab = tab);
    let list_key = panel.read_with(cx, |panel, _| panel.list_cache.borrow().key.clone());
    let notifications = Rc::new(Cell::new(0));
    let observed = notifications.clone();
    let _observer =
        cx.update(|_, cx| cx.observe(&panel, move |_, _| observed.set(observed.get() + 1)));
    for _ in 0..300 {
        if backend.held_reads("services_snapshot") == 1 {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(backend.held_reads("services_snapshot"), 1);
    backend.release_read("services_snapshot");
    for _ in 0..300 {
        panel.update(cx, |panel, cx| panel.poll(false, cx));
        cx.run_until_parked();
        if panel.read_with(cx, |panel, _| panel.client.snapshot().is_some()) {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panel.read_with(cx, |panel, _| {
        assert!(
            panel.client.snapshot().is_some(),
            "cache the completed read"
        );
        assert_eq!(panel.active_data, super::ActiveData::None);
        assert!(
            !panel.has_pending(),
            "hidden selection must not dispatch details"
        );
        assert_eq!(
            panel.list_cache.borrow().key,
            list_key,
            "defer list projection too"
        );
    });
    assert_eq!(notifications.get(), 0);
    assert_eq!(backend.count("service_logs"), 0);
    assert_eq!(backend.count("service_unit_file"), 0);

    panel.update(cx, |panel, cx| panel.poll(true, cx));
    draw(cx);
    for _ in 0..300 {
        if backend.held_reads(method) == 1 {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        backend.held_reads(method),
        1,
        "load the visible detail on return"
    );
    assert_eq!(
        backend.count("services_snapshot"),
        1,
        "reuse the fresh snapshot"
    );
    backend.release_read(method);
    settle(&panel, cx);
    assert_eq!(
        backend.count(method),
        1,
        "do not duplicate the detail request"
    );
    assert!(!panel.read_with(cx, |panel, _| panel.has_pending()));
}

#[gpui::test]
fn hidden_service_reads_publish_once_on_return_without_losing_data(cx: &mut TestAppContext) {
    use std::{cell::Cell, rc::Rc};
    let (panel, cx, backend) = fixture(cx);
    let notifications = Rc::new(Cell::new(0));
    let observed = notifications.clone();
    let _observer =
        cx.update(|_, cx| cx.observe(&panel, move |_, _| observed.set(observed.get() + 1)));
    let mut next = snapshot();
    next.services[0].description = "Updated while hidden".into();
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
        "hidden reads should not repaint Services"
    );
    assert_eq!(
        panel.read_with(cx, |panel, _| panel.client.snapshot().unwrap().services[0]
            .description
            .clone()),
        "Updated while hidden"
    );
    let requests = backend.count("services_snapshot");
    panel.update(cx, |panel, cx| panel.poll(true, cx));
    draw(cx);
    assert_eq!(notifications.get(), 1, "publish retained changes on return");
    for _ in 0..3 {
        panel.update(cx, |panel, cx| panel.poll(true, cx));
    }
    cx.run_until_parked();
    assert_eq!(notifications.get(), 1);
    assert_eq!(
        backend.count("services_snapshot"),
        requests,
        "return must reuse fresh data"
    );
}

#[gpui::test]
fn visible_service_clock_advances_without_a_query_completion(cx: &mut TestAppContext) {
    use std::{cell::Cell, rc::Rc};
    let mut data = snapshot();
    data.services[0].since = Some(chrono::Local::now().timestamp_millis() - 10_000);
    let (panel, cx, backend) = fixture_with_snapshot(cx, data);
    let calls = backend.count("services_snapshot");
    let notifications = Rc::new(Cell::new(0));
    let observed = notifications.clone();
    let _observer =
        cx.update(|_, cx| cx.observe(&panel, move |_, _| observed.set(observed.get() + 1)));
    thread::sleep(Duration::from_millis(1_100));
    panel.update(cx, |panel, cx| panel.poll(false, cx));
    cx.run_until_parked();
    assert_eq!(notifications.get(), 0, "hidden clocks must not repaint");
    panel.update(cx, |panel, cx| panel.poll(true, cx));
    cx.run_until_parked();
    assert_eq!(
        notifications.get(),
        1,
        "a visible elapsed label needs a local update"
    );
    assert_eq!(
        backend.count("services_snapshot"),
        calls,
        "clock repaint must not refetch"
    );
}

#[gpui::test]
fn offscreen_service_clocks_do_not_invalidate_the_visible_list(cx: &mut TestAppContext) {
    use std::{cell::Cell, rc::Rc};
    let mut data = snapshot();
    let since = chrono::Local::now().timestamp_millis() - 10_000;
    data.services = (0..1000)
        .map(|i| ServiceUnit {
            name: format!("app-{i:04}.service"),
            load: "loaded".into(),
            active: "active".into(),
            sub: "running".into(),
            since: (i >= 500).then_some(since),
            ..Default::default()
        })
        .collect();
    let (panel, cx, backend) = fixture_with_snapshot(cx, data);
    assert!(cx.debug_bounds("services-row-2").is_some());
    assert!(cx.debug_bounds("services-row-502").is_none());
    let calls = backend.count("services_snapshot");
    let notifications = Rc::new(Cell::new(0));
    let observed = notifications.clone();
    let _observer =
        cx.update(|_, cx| cx.observe(&panel, move |_, _| observed.set(observed.get() + 1)));
    thread::sleep(Duration::from_millis(1_100));
    panel.update(cx, |panel, cx| panel.poll(true, cx));
    cx.run_until_parked();
    assert_eq!(
        notifications.get(),
        0,
        "offscreen elapsed labels stay quiet"
    );
    assert_eq!(backend.count("services_snapshot"), calls);
}

#[gpui::test]
fn hidden_service_action_delivers_one_receipt_without_repainting_panel(cx: &mut TestAppContext) {
    use super::ServicesEvent;
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };
    let (panel, cx, backend) = fixture(cx);
    backend.hold_actions();
    click(cx, "Restart");
    for _ in 0..300 {
        if backend.held_actions() == 1 {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(backend.held_actions(), 1);
    let notifications = Rc::new(Cell::new(0));
    let observed = notifications.clone();
    let receipts = Rc::new(RefCell::new(Vec::new()));
    let received = receipts.clone();
    let _observer =
        cx.update(|_, cx| cx.observe(&panel, move |_, _| observed.set(observed.get() + 1)));
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&panel, move |_, event, _| {
            if let ServicesEvent::Notification { message } = event {
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
    assert_eq!(*receipts.borrow(), ["Restarted app"]);
    assert_eq!(notifications.get(), 0);
    settle(&panel, cx);
    assert_eq!(
        *receipts.borrow(),
        ["Restarted app"],
        "return must not repeat the receipt"
    );
    assert_eq!(backend.count("service_action"), 1);
}

/// Match the production column shell: the Services page flexes below a fixed
/// title bar, beside a sidebar, rather than filling a standalone test window.
struct ConstrainedServicesShell(Entity<ServicesPanel>);
impl Render for ConstrainedServicesShell {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .font_family(crate::theme::UI_FONT)
            .text_size(px(13.))
            .child(div().w(px(240.)).h_full().flex_shrink_0())
            .child(
                div()
                    .w_full()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(div().h(px(52.)).flex_shrink_0())
                    .child(self.0.clone()),
            )
    }
}

#[gpui::test]
fn failed_service_heading_survives_real_shell_constraints_and_post_load_resize(
    cx: &mut TestAppContext,
) {
    use newport_domain::services::TimerUnit;
    let now = chrono::Local::now().timestamp_millis();
    let mut data = snapshot();
    data.services = vec![ServiceUnit {
        name: "nixos-upgrade.service".into(),
        description: "NixOS Upgrade".into(),
        load: "loaded".into(),
        active: "failed".into(),
        sub: "failed".into(),
        enabled: Some("linked".into()),
        since: Some(now - 58_380_000),
        exit_code: Some(1),
        exit_kind: Some("exited".into()),
        result: Some("exit-code".into()),
        triggered_by: vec!["nixos-upgrade.timer".into()],
        ..Default::default()
    }];
    data.timers = vec![TimerUnit {
        name: "nixos-upgrade.timer".into(),
        scope: Scope::System,
        description: "NixOS Upgrade Timer".into(),
        active: "active".into(),
        next: Some(now + 3_600_000),
        last: Some(now - 58_380_000),
        unit: Some("nixos-upgrade.service".into()),
    }];
    let backend = FakeBackend::new(data);
    backend.hold_reads();
    let client = backend.client();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        crate::theme::install(false, cx);
        cx.set_reduce_motion(true);
    });
    let mut panel = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        window.set_rem_size(px(14.));
        let content = cx.new(|cx| ServicesPanel::with_client(client, window, cx));
        panel = Some(content.clone());
        let shell = cx.new(|_| ConstrainedServicesShell(content));
        gpui_kit::base::Root::new(shell, window, cx)
    });
    let panel = panel.unwrap();
    cx.update(|window, _| window.activate_window());
    cx.simulate_resize(size(px(1161.), px(782.)));
    draw(cx);
    for _ in 0..100 {
        if backend.held_reads("services_snapshot") == 1 {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(backend.held_reads("services_snapshot"), 1);
    backend.release_read("services_snapshot");
    for _ in 0..100 {
        panel.update(cx, |p, cx| p.poll(true, cx));
        draw(cx);
        if backend.held_reads("service_logs") == 1 {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(backend.held_reads("service_logs"), 1);
    let mut failures = Vec::new();
    let mut check = |phase: &str, cx: &mut VisualTestContext| {
        let viewport = cx.debug_bounds("services-detail").unwrap();
        let heading = cx.debug_bounds("services-detail-heading").unwrap();
        let (offset, maximum) = panel.read_with(cx, |p, _| {
            (p.detail_scroll.offset(), p.detail_scroll.max_offset())
        });
        if offset.y != px(0.) || heading.top() < viewport.top() || heading.size.height < px(20.) {
            failures.push(format!("{phase}: viewport={viewport:?}, heading={heading:?}, offset={offset:?}, max={maximum:?}"));
        }
    };
    check("snapshot accepted / logs loading", cx);
    backend.release_read("service_logs");
    settle(&panel, cx);
    check("logs loaded", cx);
    for (width, height) in [
        (1100., 660.),
        (960., 610.),
        (1161., 660.),
        (1161., 540.),
        (1280., 850.),
    ] {
        cx.simulate_resize(size(px(width), px(height)));
        draw(cx);
        check(&format!("loaded resize {width}x{height}"), cx);
    }
    assert!(
        failures.is_empty(),
        "service heading clipped without user scrolling:\n{}",
        failures.join("\n")
    );
}

#[gpui::test]
fn scrolling_journal_does_not_scroll_the_service_heading(cx: &mut TestAppContext) {
    use gpui::{ScrollDelta, ScrollWheelEvent, point};
    let (panel, cx, _backend) = fixture_with_size(cx, snapshot(), 898., 500.);
    let viewport = cx.debug_bounds("services-detail").unwrap();
    let logs = cx.debug_bounds("services-log-region").unwrap();
    assert!(
        panel.read_with(cx, |p, _| p.detail_scroll.max_offset().y) > px(22.),
        "fixture must leave the detail scrollable"
    );
    let position = point(logs.center().x, logs.top().max(viewport.top()) + px(10.));
    assert!(position.y < viewport.bottom(), "journal must be visible");
    cx.simulate_mouse_move(position, None, Default::default());
    // Leave the tail first, so the journal can consume the following downward
    // wheel rather than treating it as an intentional boundary overscroll.
    cx.simulate_event(ScrollWheelEvent {
        position,
        delta: ScrollDelta::Pixels(point(px(0.), px(80.))),
        ..Default::default()
    });
    draw(cx);
    assert!(
        panel.read_with(cx, |p, _| !p.logs.is_following_tail()),
        "wheel must have moved the nested journal off its tail"
    );
    assert_eq!(
        panel.read_with(cx, |p, _| p.detail_scroll.offset().y),
        px(0.)
    );
    cx.simulate_event(ScrollWheelEvent {
        position,
        delta: ScrollDelta::Pixels(point(px(0.), px(-22.))),
        ..Default::default()
    });
    draw(cx);
    let heading = cx.debug_bounds("services-detail-heading").unwrap();
    let offset = panel.read_with(cx, |p, _| p.detail_scroll.offset());
    assert_eq!(
        offset.y,
        px(0.),
        "journal wheel leaked to detail scrolling: heading={heading:?}, viewport={viewport:?}, offset={offset:?}"
    );
    assert!(heading.top() >= viewport.top());

    // Repeated gestures at either journal boundary stay in that scroll area.
    for delta in [-10_000., -22., 10_000., 22.] {
        cx.simulate_event(ScrollWheelEvent {
            position,
            delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
            ..Default::default()
        });
        draw(cx);
        assert_eq!(
            panel.read_with(cx, |p, _| p.detail_scroll.offset().y),
            px(0.),
            "journal boundary wheel leaked to the detail (delta={delta})"
        );
    }

    // The page remains independently scrollable outside the nested journal.
    let outside = heading.center();
    cx.simulate_mouse_move(outside, None, Default::default());
    cx.simulate_event(ScrollWheelEvent {
        position: outside,
        delta: ScrollDelta::Pixels(point(px(0.), px(-22.))),
        ..Default::default()
    });
    draw(cx);
    assert_eq!(
        panel.read_with(cx, |p, _| p.detail_scroll.offset().y),
        px(-22.),
        "wheel outside the journal must still scroll the detail"
    );
}

#[gpui::test]
fn service_and_system_logs_fill_available_height_across_loading_and_resize(
    cx: &mut TestAppContext,
) {
    let (panel, cx, backend) = fixture_staged(cx, snapshot(), 1100., 800., true);
    for _ in 0..100 {
        if backend.held_reads("services_snapshot") == 1 {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    backend.release_read("services_snapshot");
    for _ in 0..100 {
        panel.update(cx, |p, cx| p.poll(true, cx));
        draw(cx);
        if backend.held_reads("service_logs") == 1 {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(backend.held_reads("service_logs"), 1);
    let loading = cx.debug_bounds("services-log-region").unwrap();
    let heading = cx.debug_bounds("services-detail-heading").unwrap();
    backend.hold_reads.store(false, Ordering::Release);
    backend.release_read("service_logs");
    settle(&panel, cx);
    let loaded = cx.debug_bounds("services-log-region").unwrap();
    assert_eq!(
        loading, loaded,
        "accepting log lines must not resize their viewport"
    );
    assert_eq!(cx.debug_bounds("services-detail-heading").unwrap(), heading);

    for selection in [
        Selection::Unit("system:app.service".into()),
        Selection::Journal,
    ] {
        panel.update(cx, |p, cx| p.select(selection.clone(), cx));
        backend.release_read("service_logs");
        settle(&panel, cx);
        let mut heights = Vec::new();
        for height in [800., 1100.] {
            cx.simulate_resize(size(px(1100.), px(height)));
            draw(cx);
            let detail = cx.debug_bounds("services-detail").unwrap();
            let logs = cx.debug_bounds("services-log-region").unwrap();
            let footer = cx.debug_bounds("services-log-footer").unwrap();
            assert!(
                (detail.bottom() - px(16.) - footer.bottom()).abs() <= px(1.),
                "{selection:?} footer must reach the padded detail bottom: detail={detail:?}, footer={footer:?}"
            );
            assert!(
                (footer.top() - logs.bottom() - px(6.)).abs() <= px(1.),
                "{selection:?} logs must fill the space above the footer: logs={logs:?}, footer={footer:?}"
            );
            assert_eq!(
                panel.read_with(cx, |p, _| p.detail_scroll.offset().y),
                px(0.)
            );
            heights.push(logs.size.height);
        }
        assert!(
            (heights[1] - heights[0] - px(300.)).abs() <= px(2.),
            "{selection:?} logs must consume added window height: {heights:?}"
        );
        cx.simulate_resize(size(px(1100.), px(480.)));
        draw(cx);
        let logs = cx.debug_bounds("services-log-region").unwrap();
        assert!(
            logs.size.height >= px(260.),
            "{selection:?} short windows retain a readable minimum viewport: {logs:?}"
        );
        assert_eq!(
            panel.read_with(cx, |p, _| p.detail_scroll.offset().y),
            px(0.),
            "short windows must keep the detail heading visible until the user scrolls"
        );
    }
    assert_eq!(backend.count("service_action"), 0);
}

#[gpui::test]
#[ignore = "manual Services CPU frame benchmark; excludes GPU presentation and SSH"]
fn benchmark_services_frames(cx: &mut TestAppContext) {
    use std::time::Instant;
    for count in [50, 1000] {
        let mut data = snapshot();
        let template = data.services[0].clone();
        data.services = (0..count)
            .map(|i| ServiceUnit {
                name: format!("app-{i:04}.service"),
                description: format!("Application worker {i}"),
                ..template.clone()
            })
            .collect();
        let (panel, cx, _backend) = fixture_with_snapshot(cx, data);
        draw(cx);
        let start = Instant::now();
        for _ in 0..30 {
            panel.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
        eprintln!(
            "Services {count} units at 898x720: {:.3} ms/CPU frame",
            start.elapsed().as_secs_f64() * 1000. / 30.
        );
    }
}

#[gpui::test]
fn prepared_list_reuses_redraws_and_tracks_search_collapse_and_refresh(cx: &mut TestAppContext) {
    use std::rc::Rc;
    let (panel, cx, backend) = fixture(cx);
    let original = panel.read_with(cx, |p, cx| p.rows(cx));
    draw(cx);
    assert!(Rc::ptr_eq(
        &original,
        &panel.read_with(cx, |p, cx| p.rows(cx))
    ));
    panel.update(cx, |p, cx| {
        p.system_open = true;
        cx.notify();
    });
    let opened = panel.read_with(cx, |p, cx| p.rows(cx));
    assert!(!Rc::ptr_eq(&original, &opened));
    assert!(opened.len() > original.len());
    cx.update(|window, cx| {
        panel.update(cx, |p, cx| {
            p.search
                .update(cx, |input, cx| input.set_value("WORKER", window, cx));
        })
    });
    let filtered = panel.read_with(cx, |p, cx| p.rows(cx));
    assert!(!Rc::ptr_eq(&opened, &filtered));
    assert_eq!(
        panel.read_with(cx, |p, cx| p.effective_selection(cx)),
        Selection::Unit("user:worker.service".into())
    );
    let mut data = snapshot();
    data.services.retain(|unit| unit.name != "worker.service");
    backend.set_snapshot(data);
    panel.update(cx, |p, cx| p.refresh(cx));
    // Starting a refresh retains the successful snapshot and its list.
    assert!(Rc::ptr_eq(
        &filtered,
        &panel.read_with(cx, |p, cx| p.rows(cx))
    ));
    settle(&panel, cx);
    assert!(!Rc::ptr_eq(
        &filtered,
        &panel.read_with(cx, |p, cx| p.rows(cx))
    ));
    assert_eq!(
        panel.read_with(cx, |p, cx| p.effective_selection(cx)),
        Selection::Journal
    );
}

#[test]
fn cached_rows_keep_service_and_timer_clocks_current() {
    use newport_domain::services::TimerUnit;
    let mut data = snapshot();
    data.services[0].since = Some(1000);
    data.timers.push(TimerUnit {
        name: "job.timer".into(),
        next: Some(61000),
        active: "active".into(),
        ..Default::default()
    });
    let rows = master_rows(&data, "", false, &Default::default(), 1000);
    let service = rows.iter().find(|row| row.name == "app").unwrap();
    let timer = rows
        .iter()
        .find(|row| row.name == "job.timer" || row.name == "job")
        .unwrap();
    assert_eq!(service.note_at(31000), "up 30s");
    assert_eq!(timer.note_at(31000), "in 30s");
    assert!(timer.accessible_at(31000).ends_with("in 30s"));
    assert_eq!(timer.note_at(62000), "waiting");
    assert!(timer.accessible_at(62000).ends_with("waiting"));
}
