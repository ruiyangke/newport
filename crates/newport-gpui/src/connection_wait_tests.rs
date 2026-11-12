use crate::connectivity::{self, Phase, ServerState, Snapshot};
use crate::page_perf_tests::{frame, owner};
use crate::{Newport, Page, theme};
use gpui::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, TestAppContext,
    VisualTestContext, Window, div,
};
use gpui_kit as gpui;
use serde_json::json;

/// What the window shows, as the app renders it.
struct ScreenHost(Entity<Newport>);
impl Render for ScreenHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = self.0.update(cx, |view, cx| {
            view.sync_connections(window, cx);
            view.screen(window, cx)
        });
        div().size_full().flex().flex_col().child(content)
    }
}

fn labels(visual: &mut VisualTestContext) -> Vec<String> {
    visual.update(|window, _| {
        gpui::base::test_support::snapshots(window)
            .into_iter()
            .filter_map(|node| node.label().map(str::to_owned))
            .collect()
    })
}

/// A server whose connection is in `phase`, with `page` open.
fn fixture(
    cx: &mut TestAppContext,
    page: Page,
    phase: Phase,
) -> (Entity<crate::Newport>, &mut VisualTestContext) {
    let id = uuid::Uuid::new_v4();
    let instance = uuid::Uuid::new_v4();
    cx.update(|cx| {
        gpui::component::init(cx);
        theme::install(false, cx);
    });
    let mut view = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let entity = cx.new(|cx| {
            let mut v = owner(window, cx);
            v.page = page;
            v.server = id.to_string();
            v.snapshot["instanceId"] = json!(instance);
            v.snapshot["config"]["servers"][0]["id"] = json!(id);
            v.snapshot["config"]["servers"][0]["name"] = json!("Box");
            v
        });
        view = Some(entity.clone());
        ScreenHost(entity)
    });
    visual.update(|_, cx| {
        connectivity::publish(
            Snapshot {
                instance_id: instance,
                revision: 1,
                servers: [(
                    id,
                    ServerState {
                        phase,
                        ..Default::default()
                    },
                )]
                .into(),
            },
            cx,
        )
    });
    let view = view.unwrap();
    view.update(visual, |_, cx| cx.notify());
    frame(visual);
    (view, visual)
}

#[gpui::test]
fn a_page_with_nothing_read_says_it_waits_for_the_connection(cx: &mut TestAppContext) {
    for (page, feature) in [
        (Page::Containers, "Containers"),
        (Page::Services, "Services"),
    ] {
        let mut cx = cx.clone();
        let (_, visual) = fixture(&mut cx, page, Phase::Reconnecting);
        let shown = labels(visual);
        let expected = format!("{feature} appear once Box is connected.");
        assert!(
            shown.iter().any(|l| l.contains(&expected)),
            "{feature}: {shown:?}"
        );
    }
}

#[gpui::test]
fn connections_note_that_discovery_waits_for_the_connection(cx: &mut TestAppContext) {
    let (_, visual) = fixture(cx, Page::Ports, Phase::Reconnecting);
    assert!(visual.debug_bounds("connections-updating").is_some());
}

#[gpui::test]
fn a_connected_server_shows_no_waiting_note(cx: &mut TestAppContext) {
    let (_, visual) = fixture(cx, Page::Ports, Phase::Connected);
    assert!(visual.debug_bounds("connections-updating").is_none());
    let mut other = cx.clone();
    let (_, visual) = fixture(&mut other, Page::Containers, Phase::Connected);
    assert!(!labels(visual).iter().any(|l| l.contains("appear once")));
}
