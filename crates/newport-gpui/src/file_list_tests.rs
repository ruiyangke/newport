//! Render the real Files page against a strict local listing peer.
use crate::file_test_backend::{Host, reply};
use crate::{Newport, Page};
use gpui_kit as gpui;
use gpui_kit::accesskit::Role;
use gpui_kit::{
    AppContext, Context, Entity, IntoElement, Render, TestAppContext, VisualTestContext, Window,
    div, prelude::*, px, size,
};
use serde_json::{Value, json};
use std::{thread, time::Duration};

struct FilesHost(Entity<Newport>);
impl Render for FilesHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = self.0.update(cx, |view, cx| {
            view.content_width = f32::from(window.viewport_size().width);
            view.files(window, cx)
        });
        div().size_full().flex().flex_col().child(content)
    }
}

fn fixture(cx: &mut TestAppContext, width: f32) -> (Entity<Newport>, &mut VisualTestContext, Host) {
    cx.update(|cx| {
        gpui::component::init(cx);
        crate::theme::install(false, cx);
        cx.set_reduce_motion(true);
    });
    let mut owner = None;
    let mut host = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| {
            let mut view = crate::page_perf_tests::owner(window, cx);
            view.page = Page::Files;
            view.prepare_file_browser(window, cx);
            let browser = view.file_browsers.get_mut("fixture").unwrap();
            let peer = Host::with_connection(browser.connection.clone());
            browser.reads = newport_client::file_browser::Browser::new(
                peer.backend(),
                "fixture".into(),
                browser.connection.clone(),
            );
            browser.active = true;
            host = Some(peer);
            view
        });
        owner = Some(view.clone());
        gpui::base::Root::new(cx.new(|_| FilesHost(view)), window, cx)
    });
    visual.simulate_resize(size(px(width), px(720.)));
    (owner.unwrap(), visual, host.unwrap())
}

fn settle(view: &Entity<Newport>, cx: &mut VisualTestContext, host: &Host) {
    for _ in 0..1000 {
        view.update(cx, |view, cx| view.poll_file_browsers(cx));
        while let Some((stream, request)) = host.try_receive() {
            assert_eq!(request.method, "files_cancel");
            reply(stream, Ok(Value::Null));
        }
        cx.run_until_parked();
        if !view.read_with(cx, |view, _| {
            view.file_browsers["fixture"].reads.has_pending()
        }) {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("file browser did not settle");
}

fn open(view: &Entity<Newport>, cx: &mut VisualTestContext, host: &Host, listing: Value) {
    let path = listing["path"].as_str().unwrap().to_owned();
    view.update(cx, |view, cx| {
        view.file_browsers
            .get_mut("fixture")
            .unwrap()
            .reads
            .navigate(path.clone());
        view.poll_file_browsers(cx);
    });
    reply(host.listing(&path), Ok(listing));
    settle(view, cx, host);
    view.update(cx, |view, _| {
        let browser = view.file_browsers.get_mut("fixture").unwrap();
        browser.directory = path;
    });
    draw(cx);
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn nodes(cx: &mut VisualTestContext) -> Vec<gpui::base::test_support::ElementSnapshot> {
    cx.update(|window, _| gpui::base::test_support::snapshots(window))
}

fn click(cx: &mut VisualTestContext, role: Role, label: &str) {
    let at = nodes(cx)
        .into_iter()
        .find(|node| node.role() == Some(role) && node.label() == Some(label))
        .unwrap_or_else(|| panic!("missing {role:?}: {label}"))
        .bounds()
        .center();
    cx.simulate_click(at, Default::default());
    draw(cx);
}

fn files(count: usize) -> Value {
    json!({"path":"/work", "entries":(0..count).map(|i| json!({
        "name":format!("file-{i:03}.txt"),"path":format!("/work/file-{i:03}.txt"),
        "kind":"file","size":i * 100,"modified":1_700_000_000 + i as i64
    })).collect::<Vec<_>>()})
}

fn selected(view: &Entity<Newport>, cx: &mut VisualTestContext) -> Option<String> {
    view.read_with(cx, |view, _| view.file_browsers["fixture"].selected.clone())
}

#[gpui::test]
fn keyboard_selection_moves_through_a_virtualized_folder(cx: &mut TestAppContext) {
    let (view, cx, host) = fixture(cx, 1100.);
    open(&view, cx, &host, files(300));
    let rows = nodes(cx)
        .iter()
        .filter(|node| {
            node.role() == Some(Role::Row) && node.label().is_some_and(|l| l.starts_with("file-"))
        })
        .count();
    assert!(
        (10..80).contains(&rows),
        "only the visible window of 300 rows is built: {rows}"
    );
    view.update_in(cx, |view, window, cx| {
        view.file_browsers["fixture"].list_focus.focus(window, cx)
    });
    cx.simulate_keystrokes("down down down");
    draw(cx);
    assert_eq!(selected(&view, cx).as_deref(), Some("/work/file-002.txt"));
    cx.simulate_keystrokes("end");
    draw(cx);
    assert_eq!(selected(&view, cx).as_deref(), Some("/work/file-299.txt"));
    assert!(
        nodes(cx)
            .iter()
            .any(|node| node.label() == Some("file-299.txt") && node.selected() == Some(true)),
        "the selected row scrolls into view and reports its state"
    );
    cx.simulate_keystrokes("up");
    draw(cx);
    assert_eq!(selected(&view, cx).as_deref(), Some("/work/file-298.txt"));
    // Enter opens the selected file in the side preview; Escape closes it.
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert_eq!(
        view.read_with(cx, |view, _| view.file_preview_path.clone())
            .as_deref(),
        Some("/work/file-298.txt")
    );
    assert!(cx.debug_bounds("files-preview-pane").is_some());
    assert!(
        cx.debug_bounds("file-list").is_some(),
        "the list stays beside it"
    );
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(view.read_with(cx, |view, _| view.file_preview_path.is_none()));
    assert!(cx.debug_bounds("files-preview-pane").is_none());
}

#[gpui::test]
fn breadcrumb_collapses_long_paths_navigates_and_turns_into_a_field(cx: &mut TestAppContext) {
    let (view, cx, host) = fixture(cx, 1100.);
    view.update(cx, |view, _| {
        view.file_browsers.get_mut("fixture").unwrap().home = Some("/home/u".into());
    });
    let listing = json!({"path":"/home/u/a/b/c/d/e","entries":[]});
    view.update(cx, |view, cx| {
        view.file_browsers
            .get_mut("fixture")
            .unwrap()
            .reads
            .navigate("/home/u/a/b/c/d/e".into());
        view.poll_file_browsers(cx);
    });
    reply(host.listing("/home/u/a/b/c/d/e"), Ok(listing));
    settle(&view, cx, &host);
    draw(cx);
    let labels: Vec<String> = nodes(cx)
        .iter()
        .filter(|node| node.role() == Some(Role::Button))
        .filter_map(|node| node.label().map(str::to_owned))
        .collect();
    for crumb in ["Home folder", "c", "d", "e"] {
        assert!(
            labels.iter().any(|label| label == crumb),
            "{crumb}: {labels:?}"
        );
    }
    assert!(
        !labels.iter().any(|label| label == "a"),
        "middle folders collapse"
    );
    click(cx, Role::Button, "c");
    assert_eq!(
        view.read_with(cx, |view, _| view.file_browsers["fixture"]
            .requested
            .clone()),
        "/home/u/a/b/c"
    );
    reply(
        host.listing("/home/u/a/b/c"),
        Ok(json!({"path":"/home/u/a/b/c","entries":[]})),
    );
    settle(&view, cx, &host);
    draw(cx);
    let edit = cx.debug_bounds("files-breadcrumb").unwrap();
    cx.simulate_click(
        gpui::point(edit.right() - px(8.), edit.center().y),
        Default::default(),
    );
    draw(cx);
    assert!(view.read_with(cx, |view, _| view.file_browsers["fixture"].editing_path));
    assert!(cx.debug_bounds("files-breadcrumb").is_none());
}

#[gpui::test]
fn places_offer_home_root_and_this_servers_projects(cx: &mut TestAppContext) {
    let (view, cx, host) = fixture(cx, 1100.);
    view.update(cx, |view, _| {
        let generation = view.resources.begin_refresh("file-places", false).unwrap();
        view.resources.complete(
            "file-places",
            generation,
            Ok(json!([
                {"serverId":"fixture","name":"api","path":{"display":"/srv/api"}},
                {"serverId":"other","name":"elsewhere","path":{"display":"/srv/other"}}
            ])),
        );
    });
    open(&view, cx, &host, json!({"path":"/srv/api","entries":[]}));
    let mut places: Vec<_> = nodes(cx)
        .into_iter()
        .filter(|node| node.role() == Some(Role::Link))
        .collect();
    places.sort_by(|a, b| a.bounds().top().partial_cmp(&b.bounds().top()).unwrap());
    let places: Vec<_> = places
        .iter()
        .map(|node| (node.label().unwrap_or_default().to_owned(), node.selected()))
        .collect();
    assert_eq!(
        places,
        [
            ("Home".to_owned(), Some(false)),
            ("Server root".to_owned(), Some(false)),
            ("api".to_owned(), Some(true)),
        ]
    );
    click(cx, Role::Link, "Server root");
    assert_eq!(
        view.read_with(cx, |view, _| view.file_browsers["fixture"]
            .requested
            .clone()),
        "/"
    );
}

#[gpui::test]
fn column_headers_sort_and_announce_their_order(cx: &mut TestAppContext) {
    let (view, cx, host) = fixture(cx, 1100.);
    open(&view, cx, &host, files(5));
    let first = |view: &Entity<Newport>, cx: &mut VisualTestContext| {
        view.read_with(cx, |view, cx| {
            crate::text(&view.file_browsers["fixture"].entries(cx)[0], "name")
        })
    };
    assert_eq!(first(&view, cx), "file-000.txt");
    click(cx, Role::ColumnHeader, "Modified");
    assert_eq!(first(&view, cx), "file-004.txt", "newest first");
    assert!(
        nodes(cx)
            .iter()
            .any(|node| node.label() == Some("Modified, sorted descending"))
    );
    click(cx, Role::ColumnHeader, "Modified, sorted descending");
    assert_eq!(first(&view, cx), "file-000.txt");
}

#[gpui::test]
fn filter_whitespace_neither_narrows_nor_reports_no_matches(cx: &mut TestAppContext) {
    let (view, cx, host) = fixture(cx, 1100.);
    open(&view, cx, &host, files(3));
    let filter = |value: &str, view: &Entity<Newport>, cx: &mut VisualTestContext| {
        let input = view.read_with(cx, |view, _| view.file_browsers["fixture"].filter.clone());
        cx.update(|window, cx| input.update(cx, |input, cx| input.set_value(value, window, cx)));
        draw(cx);
        view.read_with(cx, |view, cx| {
            view.file_browsers["fixture"].entries(cx).len()
        })
    };
    assert_eq!(filter(" file-001 ", &view, cx), 1);
    assert_eq!(filter("   ", &view, cx), 3);
    open(&view, cx, &host, json!({"path":"/empty","entries":[]}));
    assert_eq!(filter("  ", &view, cx), 0);
    assert!(
        nodes(cx)
            .iter()
            .any(|node| node.label() == Some("This folder is empty"))
    );
}

#[gpui::test]
fn loading_and_empty_states_keep_the_column_header(cx: &mut TestAppContext) {
    let (view, cx, host) = fixture(cx, 1100.);
    view.update(cx, |view, cx| {
        view.file_browsers
            .get_mut("fixture")
            .unwrap()
            .reads
            .navigate("/empty".into());
        view.poll_file_browsers(cx);
    });
    draw(cx);
    let header = cx.debug_bounds("file-list-columns").unwrap();
    assert!(cx.debug_bounds("file-list-loading").is_some());
    reply(
        host.listing("/empty"),
        Ok(json!({"path":"/empty","entries":[]})),
    );
    settle(&view, cx, &host);
    draw(cx);
    assert!(cx.debug_bounds("file-list-empty").is_some());
    assert_eq!(cx.debug_bounds("file-list-columns").unwrap(), header);
}

#[gpui::test]
fn narrow_windows_show_the_preview_instead_of_the_list(cx: &mut TestAppContext) {
    let (view, cx, host) = fixture(cx, 620.);
    open(&view, cx, &host, files(3));
    assert!(
        cx.debug_bounds("files-places").is_none(),
        "places yield their width on narrow windows"
    );
    click(cx, Role::Row, "file-001.txt");
    assert!(cx.debug_bounds("files-preview-pane").is_some());
    assert!(cx.debug_bounds("file-list").is_none());
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(cx.debug_bounds("file-list").is_some());
}

fn drag(cx: &mut VisualTestContext, from: gpui::Point<gpui::Pixels>, dx: f32) {
    let left = gpui::MouseButton::Left;
    cx.simulate_mouse_down(from, left, Default::default());
    for step in 1..=4 {
        let to = gpui::point(from.x + px(dx * step as f32 / 4.), from.y);
        cx.simulate_mouse_move(to, Some(left), Default::default());
    }
    cx.simulate_mouse_up(
        gpui::point(from.x + px(dx), from.y),
        left,
        Default::default(),
    );
    draw(cx);
}

#[gpui::test]
fn places_and_preview_resize_by_dragging_their_dividers(cx: &mut TestAppContext) {
    let (_view, cx, host) = fixture(cx, 1100.);
    open(&_view, cx, &host, files(3));
    let places = cx.debug_bounds("files-places").unwrap();
    let list = cx.debug_bounds("file-list").unwrap();
    drag(cx, gpui::point(places.right(), places.center().y), 80.);
    let wider = cx.debug_bounds("files-places").unwrap();
    assert!(
        (wider.size.width - places.size.width - px(80.)).abs() <= px(2.),
        "{places:?} → {wider:?}"
    );
    let narrower = cx.debug_bounds("file-list").unwrap();
    assert!(narrower.size.width < list.size.width - px(70.));
    click(cx, Role::Row, "file-001.txt");
    let pane = cx.debug_bounds("files-preview-pane").unwrap();
    drag(cx, gpui::point(pane.left(), pane.center().y), -60.);
    let wider = cx.debug_bounds("files-preview-pane").unwrap();
    assert!(
        (wider.size.width - pane.size.width - px(60.)).abs() <= px(2.),
        "{pane:?} → {wider:?}"
    );
}
