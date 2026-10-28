use super::{Preferences, Selection, ServicesPanel, tests};
use gpui_kit::{
    self as gpui, AppContext, Context, Entity, IntoElement, ParentElement, Render, ScrollDelta,
    ScrollWheelEvent, Styled, TestAppContext, VisualTestContext, Window, div, point, px, size,
};
use newport_domain::services::{ServiceUnit, ServicesSnapshot};

fn many_services() -> ServicesSnapshot {
    let mut snapshot = tests::snapshot();
    snapshot.services = (0..1000)
        .map(|i| ServiceUnit {
            name: format!("app-{i:04}.service"),
            load: "loaded".into(),
            active: "active".into(),
            sub: "running".into(),
            ..Default::default()
        })
        .collect();
    snapshot
}

fn restored_fixture(
    cx: &mut TestAppContext,
) -> (
    Entity<ServicesPanel>,
    &mut VisualTestContext,
    tests::FakeBackend,
) {
    let backend = tests::FakeBackend::new(many_services());
    let client = backend.client();
    cx.update(|cx| {
        gpui::component::init(cx);
        crate::theme::install(false, cx);
        cx.set_reduce_motion(true);
    });
    let mut panel = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let content = cx.new(|cx| {
            let mut panel = ServicesPanel::with_client(client, window, cx);
            panel.restore_preferences(
                Preferences {
                    query: String::new(),
                    selected: Some(Selection::Unit("system:app-0900.service".into())),
                    system_open: false,
                    collapsed: Default::default(),
                    tab: 0,
                    log_level: "all".into(),
                    live: false,
                    log_query: String::new(),
                },
                window,
                cx,
            );
            panel
        });
        panel = Some(content.clone());
        let shell = cx.new(|_| FixtureShell(content));
        gpui::base::Root::new(shell, window, cx)
    });
    cx.update(|window, _| window.activate_window());
    cx.simulate_resize(size(px(898.), px(720.)));
    tests::draw(cx);
    (panel.unwrap(), cx, backend)
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
fn restored_deep_selection_is_revealed_after_initial_snapshot(cx: &mut TestAppContext) {
    let (panel, cx, backend) = restored_fixture(cx);
    // Rendering the initial skeleton must not consume the pending reveal.
    assert!(panel.read_with(cx, |panel, _| panel.client.snapshot().is_none()));
    assert!(cx.debug_bounds("services-row-901").is_none());
    tests::settle(&panel, cx);

    let row = cx.debug_bounds("services-row-901").unwrap();
    let viewport = cx.debug_bounds("services-master").unwrap();
    assert!(
        row.top() >= viewport.top(),
        "restored row is above the viewport"
    );
    assert!(
        row.bottom() <= viewport.bottom(),
        "restored row is below the viewport"
    );
    assert!(
        cx.debug_bounds("services-row-499").is_none(),
        "master remains virtualized"
    );
    assert_eq!(backend.count("service_action"), 0);
}

#[gpui::test]
fn refreshed_snapshot_preserves_scroll_away_from_restored_selection(cx: &mut TestAppContext) {
    let (panel, cx, backend) = restored_fixture(cx);
    tests::settle(&panel, cx);
    let selected = cx.debug_bounds("services-row-901").unwrap();
    let original = panel.read_with(cx, |panel, _| {
        panel.list_scroll.0.borrow().base_handle.offset()
    });
    cx.simulate_event(ScrollWheelEvent {
        position: selected.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(1400.))),
        ..Default::default()
    });
    tests::draw(cx);
    let scrolled = panel.read_with(cx, |panel, _| {
        panel.list_scroll.0.borrow().base_handle.offset()
    });
    assert!(
        scrolled.y > original.y,
        "wheel scrolling must move the master"
    );
    assert!(cx.debug_bounds("services-row-901").is_none());

    let mut fresh = many_services();
    fresh.services[900].description = "Updated after restoring the selection".into();
    backend.set_snapshot(fresh);
    panel.update(cx, |panel, cx| panel.refresh(cx));
    tests::settle(&panel, cx);
    let after = panel.read_with(cx, |panel, _| {
        panel.list_scroll.0.borrow().base_handle.offset()
    });
    assert_eq!(
        after, scrolled,
        "fresh readings must not recenter the master"
    );
    assert!(cx.debug_bounds("services-row-901").is_none());
    assert_eq!(
        panel.read_with(cx, |panel, _| panel.selected.clone()),
        Some(Selection::Unit("system:app-0900.service".into()))
    );
    assert_eq!(backend.count("services_snapshot"), 2);
    assert_eq!(backend.count("service_action"), 0);
}
