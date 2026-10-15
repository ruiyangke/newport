use super::{Content, FolderTree, Navigate};
use gpui::base::test_support::{ElementSnapshot, snapshots};
use gpui::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, Role, Styled, TestAppContext,
    VisualTestContext, Window, div, px, size,
};
use gpui_kit as gpui;
use newport_client::{backend::Backend, file_browser::folders::Folders};
use newport_platform::ipc::protocol::{Request, Response, VERSION};
use newport_platform::ipc::{Listener, Stream};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    io::{Read, Write},
    path::PathBuf,
    rc::Rc,
    thread,
    time::{Duration, Instant},
};

/// Strict local RPC peer. The tree never discovers the installed host or reaches
/// a real server; each directory response is released explicitly by its test.
pub(crate) struct Host {
    path: PathBuf,
    listener: Listener,
    requests: RefCell<Vec<Request>>,
    connection: Value,
}
impl Host {
    fn new() -> Self {
        Self::with_connection(connection())
    }
    pub(crate) fn with_connection(connection: Value) -> Self {
        let path =
            std::env::temp_dir().join(format!("np-tree-{}.sock", uuid::Uuid::new_v4().simple()));
        let listener = Listener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        Self {
            path,
            listener,
            requests: RefCell::default(),
            connection,
        }
    }
    fn client(&self) -> Folders {
        Folders::new(
            Backend::at_path(self.path.clone()),
            "fixture".into(),
            self.connection.clone(),
        )
    }
    pub(crate) fn backend(&self) -> Backend {
        Backend::at_path(self.path.clone())
    }
    pub(crate) fn try_receive(&self) -> Option<(Stream, Request)> {
        let (mut stream, _) = match self.listener.accept() {
            Ok(pair) => pair,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return None,
            Err(error) => panic!("{error}"),
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut header = [0; 4];
        stream.read_exact(&mut header).unwrap();
        let size = u32::from_be_bytes(header) as usize;
        assert!(size < 1024 * 1024);
        let mut bytes = vec![0; size];
        stream.read_exact(&mut bytes).unwrap();
        let request: Request = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(request.version, VERSION);
        assert!(matches!(
            request.method.as_str(),
            "files_list" | "files_cancel"
        ));
        if request.method == "files_list" {
            assert_eq!(request.args["id"], "fixture");
            assert_eq!(request.args["expectedConnection"], self.connection);
        }
        uuid::Uuid::parse_str(request.args["operation"].as_str().unwrap()).unwrap();
        self.requests
            .borrow_mut()
            .push(rmp_serde::from_slice(&bytes).unwrap());
        Some((stream, request))
    }
    fn receive(&self, method: &str) -> (Stream, Request) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some((stream, request)) = self.try_receive() {
                assert_eq!(request.method, method);
                return (stream, request);
            }
            assert!(Instant::now() < deadline, "missing {method} RPC");
            thread::sleep(Duration::from_millis(1));
        }
    }
    pub(crate) fn listing(&self, path: &str) -> Stream {
        let (stream, request) = self.receive("files_list");
        assert_eq!(request.args["path"], path);
        stream
    }
    fn quiet(&self) {
        let deadline = Instant::now() + Duration::from_millis(20);
        while Instant::now() < deadline {
            assert!(
                self.try_receive().is_none(),
                "unexpected RPC without an explicit expansion"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }
    fn reads(&self) -> usize {
        self.requests
            .borrow()
            .iter()
            .filter(|request| request.method == "files_list")
            .count()
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
fn connection() -> Value {
    json!({"host":"fixture.invalid","port":22,"username":"fixture"})
}
pub(crate) fn reply(mut stream: Stream, result: Result<Value, &str>) {
    let response = match result {
        Ok(result) => Response {
            version: VERSION,
            result,
            error: None,
        },
        Err(error) => Response {
            version: VERSION,
            result: Value::Null,
            error: Some(json!(error)),
        },
    };
    let bytes = rmp_serde::to_vec_named(&response).unwrap();
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&bytes).unwrap();
}
fn listing(path: &str, entries: Vec<Value>) -> Value {
    json!({"path":path,"entries":entries})
}
fn entry(parent: &str, name: &str, kind: &str) -> Value {
    json!({"name":name,"path":format!("{}/{name}", parent.trim_end_matches('/')),"kind":kind})
}
struct Shell(Entity<FolderTree>);
impl Render for Shell {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .font_family(crate::theme::UI_FONT)
            .text_size(px(13.))
            .child(self.0.clone())
    }
}
fn fixture(
    cx: &mut TestAppContext,
    width: f32,
    height: f32,
) -> (Entity<FolderTree>, &mut VisualTestContext, Host) {
    let host = Host::new();
    let client = host.client();
    cx.update(|cx| {
        gpui::component::init(cx);
        crate::theme::install(false, cx);
        cx.set_reduce_motion(true);
    });
    let mut tree = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let content = cx.new(|cx| {
            let mut tree = FolderTree::new(client);
            tree.configure(
                Some("/home/fixture".into()),
                "/home/fixture".into(),
                false,
                true,
                cx,
            );
            tree
        });
        tree = Some(content.clone());
        let shell = cx.new(|_| Shell(content));
        gpui::base::Root::new(shell, window, cx)
    });
    cx.update(|window, _| window.activate_window());
    cx.simulate_resize(size(px(width), px(height)));
    draw(cx);
    (tree.unwrap(), cx, host)
}
fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
}
fn settle(tree: &Entity<FolderTree>, cx: &mut VisualTestContext, host: &Host) {
    for _ in 0..300 {
        tree.update(cx, |tree, cx| tree.poll(cx));
        while let Some((stream, request)) = host.try_receive() {
            assert_eq!(
                request.method, "files_cancel",
                "reads must be explicitly observed by the test"
            );
            reply(stream, Ok(Value::Null));
        }
        draw(cx);
        if !tree.read_with(cx, |tree, _| tree.client.has_pending()) {
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!("folder tree request did not settle");
}
fn elements(cx: &mut VisualTestContext) -> Vec<ElementSnapshot> {
    cx.update(|window, _| snapshots(window))
}
fn element(cx: &mut VisualTestContext, label: &str) -> ElementSnapshot {
    elements(cx)
        .into_iter()
        .find(|element| element.label() == Some(label))
        .unwrap_or_else(|| panic!("missing {label}"))
}
fn has_element(cx: &mut VisualTestContext, label: &str) -> bool {
    elements(cx)
        .iter()
        .any(|element| element.label() == Some(label))
}
fn click(cx: &mut VisualTestContext, label: &str) {
    let element = element(cx, label);
    assert!(element.visible(), "{label} must be visible before clicking");
    cx.simulate_click(element.bounds().center(), Default::default());
    draw(cx);
}
fn navigation_events(
    tree: &Entity<FolderTree>,
    cx: &mut VisualTestContext,
) -> (Rc<RefCell<Vec<String>>>, gpui::Subscription) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let collected = events.clone();
    let subscription = cx.update(|_, cx| {
        cx.subscribe(tree, move |_, event: &Navigate, _| {
            collected.borrow_mut().push(event.0.clone())
        })
    });
    (events, subscription)
}

#[gpui::test]
fn folder_hover_covers_the_whole_row_and_preserves_selection(cx: &mut TestAppContext) {
    let (_, cx, host) = fixture(cx, 240., 300.);
    for dark in [false, true] {
        cx.update(|_, cx| crate::theme::install(dark, cx));
        draw(cx);
        let home = cx.debug_bounds("folder-row-0").unwrap();
        let server = cx.debug_bounds("folder-row-1").unwrap();
        assert_eq!(home.left(), px(crate::controls::PAGE_PADDING));
        assert_eq!(home.size.height, px(32.));
        assert_eq!(server.left(), home.left());
        assert_eq!(server.right(), home.right());
        for label in [
            "Open Server folder",
            "Expand Server",
            "Open Home folder",
            "Expand Home",
        ] {
            let target = element(cx, label).bounds().center();
            cx.simulate_mouse_move(target, None, Default::default());
            cx.update(|window, cx| {
                window.draw(cx).clear(cx);
                let quads = window.painted_quads();
                assert!(
                    quads
                        .iter()
                        .any(|q| q.background == crate::theme::git_selection(dark).into())
                );
                assert_eq!(
                    quads
                        .iter()
                        .filter(|q| q.background == crate::theme::row_hover(cx).into())
                        .count(),
                    usize::from(label.contains("Server")),
                    "one full-row hover only on an unselected folder: {label}"
                );
            });
            assert_eq!(cx.debug_bounds("folder-row-0").unwrap(), home);
            assert_eq!(cx.debug_bounds("folder-row-1").unwrap(), server);
        }
    }
    host.quiet();
}

#[gpui::test]
fn roots_are_idle_and_expansion_only_reads_scoped_directories(cx: &mut TestAppContext) {
    let (tree, cx, host) = fixture(cx, 180., 300.);
    let (events, _subscription) = navigation_events(&tree, cx);
    assert!(has_element(cx, "Open Home folder"));
    assert!(has_element(cx, "Open Server folder"));
    assert!(!tree.read_with(cx, |tree, _| tree.client.has_pending()));
    host.quiet();

    click(cx, "Expand Home");
    assert!(has_element(cx, "Loading folders"));
    assert!(events.borrow().is_empty());
    let stream = host.listing("/home/fixture");
    host.quiet();
    reply(
        stream,
        Ok(listing(
            "/home/fixture",
            vec![
                entry("/home/fixture", "docs", "directory"),
                entry("/home/fixture", ".private", "directory"),
                entry("/home/fixture", "notes.txt", "file"),
                entry("/home/fixture", "linked", "symlink"),
            ],
        )),
    );
    settle(&tree, cx, &host);
    assert!(!has_element(cx, "Loading folders"));
    assert!(has_element(cx, "Open docs folder"));
    assert!(!has_element(cx, "Open .private folder"));
    assert!(!has_element(cx, "Open notes.txt folder"));
    assert!(!has_element(cx, "Open linked folder"));

    tree.update(cx, |tree, cx| {
        tree.configure(
            Some("/home/fixture".into()),
            "/home/fixture".into(),
            true,
            true,
            cx,
        )
    });
    draw(cx);
    assert!(has_element(cx, "Open .private folder"));
    host.quiet();
    assert_eq!(host.reads(), 1);
    click(cx, "Open docs folder");
    assert_eq!(events.borrow().as_slice(), ["/home/fixture/docs"]);
    assert_eq!(
        host.reads(),
        1,
        "navigation is an event for the host, not a tree read"
    );
}

#[gpui::test]
fn live_scope_guard_blocks_retained_controls_before_host_reconfiguration(cx: &mut TestAppContext) {
    let (tree, cx, host) = fixture(cx, 180., 300.);
    let (events, _subscription) = navigation_events(&tree, cx);
    let allowed = Rc::new(Cell::new(true));
    let scope = allowed.clone();
    tree.update(cx, |tree, _| {
        tree.scope_guard = Some(Rc::new(move |_| scope.get()));
    });
    draw(cx);
    let revision = tree.read_with(cx, |tree, _| tree.revision);

    // A route or server change can invalidate the host without configuring,
    // polling, or repainting the tree. Its previously painted controls remain.
    allowed.set(false);
    click(cx, "Expand Home");
    click(cx, "Open Home folder");
    host.quiet();
    assert!(events.borrow().is_empty());
    tree.read_with(cx, |tree, _| {
        assert!(tree.active);
        assert_eq!(tree.revision, revision);
        assert!(tree.expanded.is_empty());
        assert!(!tree.client.has_pending());
    });

    // Re-entering the same scope permits the current controls, without needing
    // a new configure call or weakening their independent revision guards.
    allowed.set(true);
    click(cx, "Open Home folder");
    assert_eq!(events.borrow().as_slice(), ["/home/fixture"]);
    events.borrow_mut().clear();
    click(cx, "Expand Home");
    reply(host.listing("/home/fixture"), Err("Permission denied"));
    settle(&tree, cx, &host);
    assert!(has_element(cx, "Retry"));
    let revision = tree.read_with(cx, |tree, _| tree.revision);

    allowed.set(false);
    click(cx, "Retry");
    click(cx, "Open Home folder");
    click(cx, "Collapse Home");
    host.quiet();
    assert!(events.borrow().is_empty());
    assert_eq!(host.reads(), 1);
    tree.read_with(cx, |tree, _| {
        assert!(tree.active);
        assert_eq!(tree.revision, revision);
        assert_eq!(tree.expanded.len(), 1);
        assert!(!tree.client.has_pending());
    });

    allowed.set(true);
    click(cx, "Retry");
    reply(
        host.listing("/home/fixture"),
        Ok(listing(
            "/home/fixture",
            vec![entry("/home/fixture", "docs", "directory")],
        )),
    );
    settle(&tree, cx, &host);
    assert!(!has_element(cx, "Retry"));
    click(cx, "Open docs folder");
    assert_eq!(events.borrow().as_slice(), ["/home/fixture/docs"]);
    host.quiet();
    assert_eq!(host.reads(), 2);
}

#[gpui::test]
fn failed_expansion_retries_explicitly_and_empty_or_truncated_results_are_distinct(
    cx: &mut TestAppContext,
) {
    let (tree, cx, host) = fixture(cx, 180., 300.);
    click(cx, "Expand Server");
    reply(host.listing("/"), Err("Permission denied"));
    settle(&tree, cx, &host);
    assert_eq!(element(cx, "Permission denied").role(), Some(Role::Alert));
    assert!(has_element(cx, "Retry"));
    draw(cx);
    host.quiet();
    assert_eq!(host.reads(), 1, "errors do not retry themselves");

    click(cx, "Retry");
    assert!(has_element(cx, "Loading folders"));
    reply(
        host.listing("/"),
        Ok(listing("/", vec![entry("/", "readme", "file")])),
    );
    settle(&tree, cx, &host);
    assert!(!has_element(cx, "Retry"));
    assert!(tree.read_with(cx, |tree, _| {
        tree.rows.iter().any(|row| row.content == Content::Empty)
    }));
    assert_eq!(host.reads(), 2);

    click(cx, "Collapse Server");
    click(cx, "Expand Server");
    let mut result = listing("/", Vec::new());
    result["truncated"] = json!(true);
    reply(host.listing("/"), Ok(result));
    settle(&tree, cx, &host);
    assert!(tree.read_with(cx, |tree, _| {
        tree.rows
            .iter()
            .any(|row| row.content == Content::Truncated)
    }));
    assert!(!tree.read_with(cx, |tree, _| {
        tree.rows.iter().any(|row| row.content == Content::Empty)
    }));
}

#[gpui::test]
fn collapsing_or_suspending_invalidates_pending_rows_and_their_captured_callbacks(
    cx: &mut TestAppContext,
) {
    let (tree, cx, host) = fixture(cx, 180., 300.);
    let (events, _subscription) = navigation_events(&tree, cx);
    click(cx, "Expand Home");
    let (held, request) = host.receive("files_list");
    let (captured, revision) = tree.read_with(cx, |tree, _| (tree.rows[0].clone(), tree.revision));
    click(cx, "Collapse Home");
    assert!(!has_element(cx, "Loading folders"));
    let (cancel, cancellation) = host.receive("files_cancel");
    assert_eq!(cancellation.args["operation"], request.args["operation"]);
    reply(cancel, Ok(Value::Null));
    reply(
        held,
        Ok(listing(
            "/home/fixture",
            vec![entry("/home/fixture", "stale", "directory")],
        )),
    );
    settle(&tree, cx, &host);
    tree.update(cx, |tree, cx| {
        tree.toggle(&captured, revision, cx);
        tree.navigate(&captured, revision, cx);
    });
    draw(cx);
    assert!(has_element(cx, "Expand Home"));
    assert!(!has_element(cx, "Open stale folder"));
    assert!(events.borrow().is_empty());
    host.quiet();

    click(cx, "Expand Home");
    let held = host.listing("/home/fixture");
    let (captured, revision) = tree.read_with(cx, |tree, _| (tree.rows[0].clone(), tree.revision));
    let notifications = Rc::new(Cell::new(0));
    let observed = notifications.clone();
    let _observer =
        cx.update(|_, cx| cx.observe(&tree, move |_, _| observed.set(observed.get() + 1)));
    tree.update(cx, |tree, cx| {
        tree.configure(
            Some("/home/fixture".into()),
            "/home/fixture".into(),
            false,
            false,
            cx,
        )
    });
    draw(cx);
    assert_eq!(
        notifications.get(),
        1,
        "deactivation must disable mounted controls"
    );
    notifications.set(0);
    let (cancel, _) = host.receive("files_cancel");
    reply(cancel, Ok(Value::Null));
    reply(
        held,
        Ok(listing(
            "/home/fixture",
            vec![entry("/home/fixture", "late", "directory")],
        )),
    );
    settle(&tree, cx, &host);
    assert_eq!(
        notifications.get(),
        0,
        "inactive cancellation replies must stay quiet"
    );
    tree.update(cx, |tree, cx| {
        tree.toggle(&captured, revision, cx);
        tree.navigate(&captured, revision, cx);
    });
    draw(cx);
    assert!(events.borrow().is_empty());
    click(cx, "Expand Home");
    click(cx, "Open Home folder");
    assert!(events.borrow().is_empty());
    assert!(!has_element(cx, "Open late folder"));
    host.quiet();
}

#[gpui::test]
fn hiding_dot_folders_cancels_child_reads_without_reopening_them_on_return(
    cx: &mut TestAppContext,
) {
    let (tree, cx, host) = fixture(cx, 180., 300.);
    let (events, _subscription) = navigation_events(&tree, cx);
    tree.update(cx, |tree, cx| {
        tree.configure(
            Some("/home/fixture".into()),
            "/home/fixture".into(),
            true,
            true,
            cx,
        )
    });
    draw(cx);
    click(cx, "Expand Home");
    reply(
        host.listing("/home/fixture"),
        Ok(listing(
            "/home/fixture",
            vec![
                entry("/home/fixture", ".private", "directory"),
                entry("/home/fixture", "public", "directory"),
            ],
        )),
    );
    settle(&tree, cx, &host);
    click(cx, "Expand .private");
    let (held, request) = host.receive("files_list");
    assert_eq!(request.args["path"], "/home/fixture/.private");
    let (captured, revision) = tree.read_with(cx, |tree, _| {
        (
            tree.rows
                .iter()
                .find(|row| row.content == Content::Folder(".private".into()))
                .unwrap()
                .clone(),
            tree.revision,
        )
    });
    tree.update(cx, |tree, cx| {
        tree.configure(
            Some("/home/fixture".into()),
            "/home/fixture".into(),
            false,
            true,
            cx,
        )
    });
    draw(cx);
    let (cancel, cancellation) = host.receive("files_cancel");
    assert_eq!(cancellation.args["operation"], request.args["operation"]);
    reply(cancel, Ok(Value::Null));
    reply(
        held,
        Ok(listing(
            "/home/fixture/.private",
            vec![entry("/home/fixture/.private", "late", "directory")],
        )),
    );
    settle(&tree, cx, &host);
    assert!(!has_element(cx, "Open .private folder"));
    assert!(has_element(cx, "Open public folder"));
    tree.update(cx, |tree, cx| {
        tree.navigate(&captured, revision, cx);
        tree.toggle(&captured, revision, cx);
        tree.configure(
            Some("/home/fixture".into()),
            "/home/fixture".into(),
            true,
            true,
            cx,
        );
    });
    draw(cx);
    assert!(events.borrow().is_empty());
    assert!(has_element(cx, "Expand .private"));
    assert!(!has_element(cx, "Open late folder"));
    host.quiet();
    assert_eq!(host.reads(), 2);
}

#[gpui::test]
fn depth_limit_disables_expansion_without_disabling_folder_navigation(cx: &mut TestAppContext) {
    let (tree, cx, host) = fixture(cx, 600., 640.);
    let (events, _subscription) = navigation_events(&tree, cx);
    let mut path = "/home/fixture".to_owned();
    let mut label = "Home".to_owned();
    for depth in 1..=16 {
        click(cx, &format!("Expand {label}"));
        let name = format!("d{depth}");
        reply(
            host.listing(&path),
            Ok(listing(&path, vec![entry(&path, &name, "directory")])),
        );
        settle(&tree, cx, &host);
        path.push('/');
        path.push_str(&name);
        label = name;
    }
    click(cx, "Expand d16");
    host.quiet();
    assert_eq!(host.reads(), 16);
    assert!(events.borrow().is_empty());
    click(cx, "Open d16 folder");
    assert_eq!(events.borrow().as_slice(), [path.clone()]);
    for width in [180., 140.] {
        cx.simulate_resize(size(px(width), px(640.)));
        draw(cx);
        let nav = cx.debug_bounds("file-tree").unwrap();
        let scroll = cx.debug_bounds("file-tree-scroll").unwrap();
        assert!(element(cx, "Open d16 folder").bounds().size.width >= px(60.));
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: scroll.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(-10_000.), px(0.))),
            ..Default::default()
        });
        draw(cx);
        let leaf = element(cx, "Open d16 folder").bounds();
        assert!(
            leaf.left() >= nav.left() && leaf.right() <= nav.right(),
            "deep folder control {leaf:?} must be reachable by horizontal scrolling in {nav:?}"
        );
        assert_eq!(cx.debug_bounds("file-tree").unwrap(), nav);
        events.borrow_mut().clear();
        click(cx, "Open d16 folder");
        assert_eq!(events.borrow().as_slice(), [path.clone()]);
    }
    host.quiet();
    assert_eq!(host.reads(), 16);
}

#[gpui::test]
fn home_and_server_aliases_share_a_read_without_collapsing_open_descendants(
    cx: &mut TestAppContext,
) {
    let (tree, cx, host) = fixture(cx, 180., 300.);
    tree.update(cx, |tree, cx| {
        tree.configure(Some("/".into()), "/work".into(), false, true, cx)
    });
    draw(cx);
    click(cx, "Expand Home");
    reply(
        host.listing("/"),
        Ok(listing("/", vec![entry("/", "work", "directory")])),
    );
    settle(&tree, cx, &host);
    click(cx, "Expand work");
    reply(
        host.listing("/work"),
        Ok(listing("/work", vec![entry("/work", "leaf", "directory")])),
    );
    settle(&tree, cx, &host);
    assert!(has_element(cx, "Open leaf folder"));

    click(cx, "Expand Server");
    host.quiet();
    assert_eq!(
        host.reads(),
        2,
        "opening another root alias reuses its already expanded path"
    );
    assert!(!has_element(cx, "Loading folders"));
    assert!(has_element(cx, "Open leaf folder"));
    tree.read_with(cx, |tree, _| {
        assert!(
            tree.rows
                .iter()
                .any(|row| row.node.root == 0 && row.node.path == "/work/leaf")
        );
        assert!(
            tree.rows
                .iter()
                .any(|row| row.node.root == 1 && row.node.path == "/work")
        );
        assert!(
            tree.expanded
                .iter()
                .any(|node| node.root == 0 && node.path == "/work")
        );
    });

    click(cx, "Collapse Server");
    host.quiet();
    assert!(
        has_element(cx, "Open leaf folder"),
        "collapsing the alias keeps the other root open"
    );
    assert_eq!(host.reads(), 2);
}

#[gpui::test]
fn narrow_folder_navigation_bounds_and_virtualizes_long_directory_lists(cx: &mut TestAppContext) {
    let (tree, cx, host) = fixture(cx, 180., 280.);
    click(cx, "Expand Home");
    let name = |index: usize| format!("folder-{index:04}-a-long-directory-name");
    reply(
        host.listing("/home/fixture"),
        Ok(listing(
            "/home/fixture",
            (0..2_000)
                .map(|index| entry("/home/fixture", &name(index), "directory"))
                .collect(),
        )),
    );
    settle(&tree, cx, &host);
    assert_eq!(tree.read_with(cx, |tree, _| tree.rows.len()), 2_002);
    for width in [180., 140.] {
        cx.simulate_resize(size(px(width), px(280.)));
        tree.update(cx, |tree, cx| {
            tree.scroll.scroll_to_item(0, gpui::ScrollStrategy::Top);
            cx.notify();
        });
        draw(cx);
        let nav = cx.debug_bounds("file-tree").unwrap();
        assert_eq!(nav.size.width, px(width));
        assert!(has_element(cx, &format!("Open {} folder", name(0))));
        assert!(!has_element(cx, &format!("Open {} folder", name(1_999))));
        let mounted = elements(cx);
        let buttons: Vec<_> = mounted
            .iter()
            .filter(|element| element.role() == Some(Role::Button))
            .collect();
        assert!(buttons.len() < 40, "offscreen rows must stay unmounted");
        for button in buttons.iter().filter(|button| button.visible()) {
            assert!(button.bounds().left() >= nav.left());
            assert!(
                button.bounds().right() <= nav.right(),
                "{button:?} overflows {nav:?}"
            );
        }
        tree.update(cx, |tree, cx| {
            tree.scroll.scroll_to_item(1_500, gpui::ScrollStrategy::Top);
            cx.notify();
        });
        draw(cx);
        assert_eq!(
            cx.debug_bounds("file-tree").unwrap(),
            nav,
            "scroll stays inside navigation"
        );
        assert!(has_element(cx, &format!("Open {} folder", name(1_499))));
        assert!(!has_element(cx, &format!("Open {} folder", name(0))));
    }
    host.quiet();
    assert_eq!(host.reads(), 1);
}

#[gpui::test]
#[ignore = "manual Files folder-tree CPU benchmark; excludes GPU presentation and SSH"]
fn benchmark_files_tree_frames(cx: &mut TestAppContext) {
    for count in [50, 2000] {
        let (tree, cx, host) = fixture(cx, 240., 600.);
        click(cx, "Expand Home");
        reply(
            host.listing("/home/fixture"),
            Ok(listing(
                "/home/fixture",
                (0..count)
                    .map(|i| entry("/home/fixture", &format!("folder-{i:04}"), "directory"))
                    .collect(),
            )),
        );
        settle(&tree, cx, &host);
        let start = std::time::Instant::now();
        for _ in 0..30 {
            tree.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
        eprintln!(
            "Files tree {count} folders: {:.3} ms/CPU frame",
            start.elapsed().as_secs_f64() * 1000. / 30.
        );
        host.quiet();
        assert_eq!(host.reads(), 1);
    }
}

#[gpui::test]
fn cached_folders_survive_loading_and_failed_refresh(cx: &mut TestAppContext) {
    let (tree, cx, host) = fixture(cx, 240., 400.);
    click(cx, "Expand Server");
    reply(
        host.listing("/"),
        Ok(listing("/", vec![entry("/", "docs", "directory")])),
    );
    settle(&tree, cx, &host);
    click(cx, "Collapse Server");
    click(cx, "Expand Server");
    assert!(tree.read_with(cx, |tree, _| {
        tree.rows
            .iter()
            .any(|row| row.content == Content::Folder("docs".into()))
    }));
    assert!(has_element(cx, "Refreshing folders"));
    assert!(!has_element(cx, "Loading folders"));
    reply(host.listing("/"), Err("Offline"));
    settle(&tree, cx, &host);
    assert!(tree.read_with(cx, |tree, _| {
        tree.rows
            .iter()
            .any(|row| row.content == Content::Folder("docs".into()))
    }));
    assert!(has_element(cx, "Offline Showing cached folders."));
    click(cx, "Retry");
    assert!(has_element(cx, "Offline Showing cached folders."));
    reply(host.listing("/"), Ok(listing("/", vec![])));
    settle(&tree, cx, &host);
    assert!(tree.read_with(cx, |tree, _| tree.client.get("/").unwrap().loaded));
    click(cx, "Collapse Server");
    click(cx, "Expand Server");
    assert!(
        !has_element(cx, "Loading folders"),
        "cached empty folders are not initial loads"
    );
    reply(host.listing("/"), Ok(listing("/", vec![])));
    settle(&tree, cx, &host);
}
