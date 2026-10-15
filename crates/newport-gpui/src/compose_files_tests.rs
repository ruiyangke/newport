use super::ContainersPanel;
use crate::containers::tests::{
    FakeBackend, click, compose_project, draw, fixture, select, settle, snapshot,
};
use gpui_kit as gpui;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, point, px, size};
use serde_json::{Value, json};

const FIRST: &str = "/srv/stack/compose.yml";
const SECOND: &str = "/srv/override/compose.yml";

fn files(items: Value) -> Value {
    let mut project = compose_project();
    project["files"] = items;
    project
}
fn refresh(panel: &Entity<ContainersPanel>, cx: &mut VisualTestContext) {
    panel.update(cx, |p, _| p.client.retry_compose());
    settle(panel, cx);
}
fn open(panel: &Entity<ContainersPanel>, cx: &mut VisualTestContext) {
    select(panel, cx, "p:stack");
    refresh(panel, cx);
    click(cx, "Compose files");
    settle(panel, cx);
}
fn text(panel: &Entity<ContainersPanel>, cx: &VisualTestContext) -> String {
    panel.read_with(cx, |p, cx| p.files.editor.read(cx).value().to_string())
}
fn choose(panel: &Entity<ContainersPanel>, cx: &mut VisualTestContext, path: &str) {
    panel.update_in(cx, |p, window, cx| {
        p.files.choose("stack", path, window, cx)
    });
    draw(cx);
}
fn has_label(cx: &mut VisualTestContext, label: &str) -> bool {
    cx.update(|window, _| {
        gpui_kit::base::test_support::snapshots(window)
            .iter()
            .any(|node| node.label() == Some(label))
    })
}
fn click_tab(cx: &mut VisualTestContext, path: &str) {
    let at = cx.update(|window, _| {
        gpui_kit::base::test_support::snapshots(window)
            .into_iter()
            .find(|node| {
                node.role() == Some(gpui::accesskit::Role::Tab) && node.label() == Some(path)
            })
            .unwrap_or_else(|| panic!("missing file tab {path}"))
            .bounds()
            .center()
    });
    cx.simulate_click(at, Default::default());
    draw(cx);
}
fn populate(backend: &FakeBackend) {
    backend.set_compose_project(Ok(files(json!([
        {"path":FIRST,"text":"services: first","error":null},
        {"path":SECOND,"text":"services: second","error":null}
    ]))));
}

#[gpui::test]
fn file_identity_survives_reorder_and_local_switch_does_not_fetch(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    populate(&backend);
    open(&panel, cx);
    let reads = backend.count("compose_project");
    click_tab(cx, SECOND);
    assert_eq!(text(&panel, cx), "services: second");
    assert_eq!(backend.count("compose_project"), reads);
    let editor = panel.read_with(cx, |p, _| p.files.editor.entity_id());
    backend.set_compose_project(Ok(files(json!([
        {"path":SECOND,"text":"services: second","error":null},
        {"path":FIRST,"text":"services: first","error":null}
    ]))));
    refresh(&panel, cx);
    assert_eq!(
        panel.read_with(cx, |p, _| p.files.editor.entity_id()),
        editor
    );
    assert_eq!(
        panel.read_with(cx, |p, _| p.files.selected_path.clone()),
        Some(SECOND.into())
    );
    backend.set_compose_project(Ok(files(json!([
        {"path":FIRST,"text":"services: first","error":null}
    ]))));
    refresh(&panel, cx);
    assert_ne!(
        panel.read_with(cx, |p, _| p.files.editor.entity_id()),
        editor
    );
    assert_eq!(text(&panel, cx), "services: first");
    assert_eq!(
        panel.read_with(cx, |p, _| p.files.selected_path.clone()),
        Some(FIRST.into())
    );
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn many_file_tabs_reveal_keyboard_selection_without_extra_reads(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    let items: Vec<_> = (0..20)
        .map(|i| {
            json!({
                "path":format!("/srv/project/compose-{i:02}-development.yml"),
                "text":format!("services: {{service_{i}: {{}}}}"),
                "error":null
            })
        })
        .collect();
    backend.set_compose_project(Ok(files(json!(items))));
    open(&panel, cx);
    let reads = backend.count("compose_project");
    click_tab(cx, "/srv/project/compose-00-development.yml");
    cx.simulate_keystrokes("end");
    draw(cx);
    assert_eq!(
        panel.read_with(cx, |p, _| p.files.selected_path.clone()),
        Some("/srv/project/compose-19-development.yml".into())
    );
    assert!(panel.read_with(cx, |p, _| p.files.tab_scroll.offset().x) < px(0.));
    assert_eq!(text(&panel, cx), "services: {service_19: {}}");
    assert_eq!(backend.count("compose_project"), reads);
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn file_errors_keep_readable_content_and_never_leak_previous_text(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    let mut project = files(json!([
        {"path":FIRST,"text":"services: truncated","error":"Preview truncated"},
        {"path":SECOND,"text":null,"error":"Permission denied"}
    ]));
    project["configError"] = json!("Missing environment variable");
    backend.set_compose_project(Ok(project));
    open(&panel, cx);
    assert!(has_label(
        cx,
        "Compose can’t read this project’s configuration."
    ));
    assert!(has_label(cx, "Preview truncated"));
    assert_eq!(text(&panel, cx), "services: truncated");
    backend.set_compose_project(Err("Fixture connection unavailable".into()));
    refresh(&panel, cx);
    assert!(has_label(cx, "Fixture connection unavailable"));
    assert_eq!(text(&panel, cx), "services: truncated");
    choose(&panel, cx, SECOND);
    assert_eq!(text(&panel, cx), "");
    assert!(has_label(cx, "Permission denied"));
    assert!(cx.debug_bounds("compose-file-editor").is_none());
    backend.set_compose_project(Ok(files(json!([
        {"path":SECOND,"text":"","error":null}
    ]))));
    click(cx, "Retry files");
    settle(&panel, cx);
    assert!(!has_label(cx, "Fixture connection unavailable"));
    assert!(
        cx.debug_bounds("compose-file-editor").is_some(),
        "empty readable files still have an editor"
    );
    backend.set_compose_project(Ok(files(json!([]))));
    refresh(&panel, cx);
    assert!(has_label(
        cx,
        "No compose file is recorded for this project."
    ));
    assert!(cx.debug_bounds("compose-file-editor").is_none());
    let mut unavailable = files(json!([]));
    unavailable["configError"] = json!("Permission denied");
    backend.set_compose_project(Ok(unavailable));
    refresh(&panel, cx);
    assert!(has_label(
        cx,
        "Compose can’t read this project’s configuration."
    ));
    assert!(
        !has_label(cx, "No compose file is recorded for this project."),
        "an unreadable configuration is not a successful empty result"
    );
    backend.set_compose_project(Err("Fixture connection unavailable".into()));
    refresh(&panel, cx);
    assert!(!has_label(
        cx,
        "No compose file is recorded for this project."
    ));
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn readonly_file_supports_find_copy_wrap_and_keyboard_tabs(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    populate(&backend);
    open(&panel, cx);
    choose(&panel, cx, FIRST);
    cx.simulate_keystrokes("right");
    draw(cx);
    assert_eq!(text(&panel, cx), "services: second");
    cx.simulate_keystrokes("home");
    draw(cx);
    assert_eq!(text(&panel, cx), "services: first");
    cx.simulate_keystrokes("end");
    draw(cx);
    assert_eq!(text(&panel, cx), "services: second");
    click(cx, "Copy file contents");
    assert_eq!(
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()).unwrap()),
        "services: second"
    );
    click(cx, "Wrap file lines");
    assert!(panel.read_with(cx, |p, _| p.files.wrap));
    let editor = panel.read_with(cx, |p, _| p.files.editor.clone());
    cx.update(|window, cx| editor.update(cx, |editor, cx| editor.focus(window, cx)));
    cx.simulate_input("cannot edit");
    cx.simulate_keystrokes("cmd-v");
    draw(cx);
    assert_eq!(text(&panel, cx), "services: second");
    click(cx, "Find file contents");
    assert!(editor.read_with(cx, |editor, _| editor.search_session().open));
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn refresh_preserves_editor_viewport_and_resize_uses_remaining_height(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    let content: String = (0..2000)
        .map(|i| format!("  service-{i}: {{image: example}}\n"))
        .collect();
    let project = files(json!([{"path":FIRST,"text":content,"error":null}]));
    backend.set_compose_project(Ok(project.clone()));
    open(&panel, cx);
    let editor = panel.read_with(cx, |p, _| p.files.editor.clone());
    editor.update(cx, |editor, cx| {
        editor.set_selected_range(5..10, cx);
        editor.set_scroll_offset(point(px(0.), px(-500.)), cx);
    });
    draw(cx);
    let scroll = editor.read_with(cx, |editor, _| editor.scroll_offset());
    assert!(scroll.y < px(0.));
    refresh(&panel, cx);
    assert_eq!(
        editor.read_with(cx, |editor, _| editor.selected_range()),
        5..10
    );
    assert_eq!(
        editor.read_with(cx, |editor, _| editor.scroll_offset()),
        scroll
    );
    let mut changed = project;
    changed["files"][0]["text"] = json!(format!("{content}  new-service: {{image: new}}\n"));
    backend.set_compose_project(Ok(changed));
    refresh(&panel, cx);
    assert_eq!(
        panel.read_with(cx, |p, _| p.files.editor.entity_id()),
        editor.entity_id()
    );
    assert_eq!(
        editor.read_with(cx, |editor, _| editor.selected_range()),
        5..10
    );
    assert_eq!(
        editor.read_with(cx, |editor, _| editor.scroll_offset()),
        scroll
    );
    let short = cx.debug_bounds("compose-file-editor").unwrap();
    cx.simulate_resize(size(px(980.), px(1300.)));
    draw(cx);
    let tall = cx.debug_bounds("compose-file-editor").unwrap();
    assert!(
        tall.size.height - short.size.height >= px(295.),
        "editor should fill extra available height: {short:?} -> {tall:?}"
    );
    assert!(
        editor.read_with(cx, |editor, _| editor.visible_row_range().unwrap().len()) < 100,
        "large file must render only visible rows"
    );
    let outer = panel.read_with(cx, |p, _| p.detail_scroll.offset());
    cx.update(|window, cx| editor.update(cx, |editor, cx| editor.focus(window, cx)));
    cx.simulate_keystrokes("pagedown");
    draw(cx);
    assert_eq!(panel.read_with(cx, |p, _| p.detail_scroll.offset()), outer);
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn another_project_with_same_path_resets_editor_identity(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    populate(&backend);
    let mut data = snapshot();
    let mut container = data.containers[0].clone();
    container.id = "e".repeat(64);
    container.name = "other-web-1".into();
    container.compose.as_mut().unwrap().project = "other".into();
    data.containers.push(container);
    backend.set_snapshot(data);
    panel.update(cx, |p, _| p.client.refresh());
    settle(&panel, cx);
    open(&panel, cx);
    let editor = panel.read_with(cx, |p, _| p.files.editor.clone());
    editor.update(cx, |editor, cx| {
        editor.set_selected_range(0..5, cx);
        editor.open_search(false, cx);
    });
    select(&panel, cx, "p:other");
    click(cx, "Compose files");
    settle(&panel, cx);
    let next = panel.read_with(cx, |p, _| p.files.editor.clone());
    assert_ne!(next.entity_id(), editor.entity_id());
    assert_eq!(
        next.read_with(cx, |editor, _| editor.selected_range()),
        0..0
    );
    assert!(!next.read_with(cx, |editor, _| editor.search_session().open));
    assert_eq!(backend.writes(), 0);
}
