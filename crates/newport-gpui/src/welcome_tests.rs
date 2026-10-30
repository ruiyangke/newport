use crate::page_perf_tests::{frame, owner};
use crate::{Newport, theme};
use gpui::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, TestAppContext,
    Window, div, px, size,
};
use gpui_kit as gpui;
use serde_json::json;

struct ScreenHost(Entity<Newport>);
impl Render for ScreenHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = self.0.update(cx, |view, cx| view.screen(window, cx));
        div().size_full().flex().flex_col().child(content)
    }
}

#[gpui::test]
fn a_host_that_cant_be_imported_shows_its_whole_reason(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        theme::install(false, cx);
        cx.set_reduce_motion(true);
    });
    let mut view = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let entity = cx.new(|cx| {
            let mut v = owner(window, cx);
            v.server.clear();
            v.snapshot["config"]["servers"] = json!([]);
            v
        });
        view = Some(entity.clone());
        ScreenHost(entity)
    });
    visual.simulate_resize(size(px(640.), px(800.)));
    let view = view.unwrap();
    let (work, requests) = std::sync::mpsc::sync_channel(16);
    view.update(visual, |v, _| {
        v.work = work.into();
        v.poll_welcome();
    });
    let hosts = requests.try_recv().unwrap();
    assert_eq!(hosts.method, "ssh_config_hosts");
    view.update(visual, |v, cx| {
        v.resources.complete(
            &hosts.key,
            hosts.generation,
            Ok(json!({"path": "~/.ssh/config", "hosts": [{
                "alias": "orb", "user": "default", "host": "127.0.0.1", "port": 32222,
                "importable": false, "suggested": false,
                "note": "Uses a ProxyCommand, which Newport doesn’t support yet",
            }]})),
        );
        v.poll_welcome();
        cx.notify();
    });
    frame(visual);
    let note = visual.debug_bounds("welcome-host-note-0").unwrap();
    let row = visual.debug_bounds("welcome-host-0").unwrap();
    let card = visual.debug_bounds("welcome-import").unwrap();
    // On its own line under the host, inside the card.
    assert!(
        note.top() > row.top() + px(crate::ui::FONT_SIZE),
        "{note:?}"
    );
    assert!(note.right() <= card.right(), "{note:?} {card:?}");
    // Nothing to import: no Import button.
    let labels: Vec<String> = visual.update(|window, _| {
        gpui::base::test_support::snapshots(window)
            .into_iter()
            .filter_map(|node| node.label().map(str::to_owned))
            .collect()
    });
    assert!(
        !labels.iter().any(|l| l.starts_with("Import ")),
        "{labels:?}"
    );
}

#[gpui::test]
fn an_empty_ssh_config_never_shows_the_import_card(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        theme::install(false, cx);
        cx.set_reduce_motion(true);
    });
    let mut view = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let entity = cx.new(|cx| {
            let mut v = owner(window, cx);
            v.server.clear();
            v.snapshot["config"]["servers"] = json!([]);
            v
        });
        view = Some(entity.clone());
        ScreenHost(entity)
    });
    visual.simulate_resize(size(px(640.), px(800.)));
    let view = view.unwrap();
    let (work, requests) = std::sync::mpsc::sync_channel(16);
    view.update(visual, |v, cx| {
        v.work = work.into();
        v.poll_welcome();
        cx.notify();
    });
    let hosts = requests.try_recv().unwrap();
    frame(visual);
    assert!(visual.debug_bounds("welcome-import").is_none());
    view.update(visual, |v, cx| {
        v.resources.complete(
            &hosts.key,
            hosts.generation,
            Ok(json!({"path": "~/.ssh/config", "hosts": []})),
        );
        v.poll_welcome();
        cx.notify();
    });
    frame(visual);
    assert!(visual.debug_bounds("welcome-import").is_none());
    let labels: Vec<String> = visual.update(|window, _| {
        gpui::base::test_support::snapshots(window)
            .into_iter()
            .filter_map(|node| node.label().map(str::to_owned))
            .collect()
    });
    assert!(labels.iter().any(|l| l == "Add server"), "{labels:?}");
}

#[gpui::test]
fn a_chosen_host_is_imported_after_the_jump_host_it_connects_through(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        theme::install(false, cx);
    });
    let mut view = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let entity = cx.new(|cx| {
            let mut v = owner(window, cx);
            v.server.clear();
            v.snapshot["config"]["servers"] = json!([]);
            v
        });
        view = Some(entity.clone());
        ScreenHost(entity)
    });
    let view = view.unwrap();
    let (work, requests) = std::sync::mpsc::sync_channel(16);
    view.update(visual, |v, _| {
        v.work = work.into();
        v.poll_welcome();
    });
    let hosts = requests.try_recv().unwrap();
    let bastion = uuid::Uuid::new_v4();
    let inner = uuid::Uuid::new_v4();
    view.update(visual, |v, _| {
        v.resources.complete(
            &hosts.key,
            hosts.generation,
            Ok(json!({"path": "~/.ssh/config", "hosts": [
                {"id": bastion, "alias": "ops@bastion.example.com", "user": "ops",
                 "host": "bastion.example.com", "port": 22, "proxy": null, "through": [],
                 "importable": true, "suggested": false, "note": "Jump host for inner"},
                {"id": inner, "alias": "inner", "user": "ops", "host": "10.1.0.2", "port": 22,
                 "proxy": {"jump": bastion}, "through": ["ops@bastion.example.com"],
                 "importable": true, "suggested": true, "note": null},
            ]})),
        );
        v.poll_welcome();
        v.import_chosen();
    });
    let first = requests.try_recv().unwrap();
    assert_eq!(first.method, "save_server");
    assert_eq!(first.args["server"]["id"], json!(bastion));
    assert_eq!(first.args["server"]["proxy"], json!(null));
    view.update(visual, |v, _| {
        v.resources
            .complete(&first.key, first.generation, Ok(json!(null)));
        v.poll_welcome();
    });
    let second = requests.try_recv().unwrap();
    assert_eq!(second.args["server"]["id"], json!(inner));
    assert_eq!(second.args["server"]["proxy"], json!({"jump": bastion}));
}
