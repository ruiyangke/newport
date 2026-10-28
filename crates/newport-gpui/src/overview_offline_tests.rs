use crate::connectivity::{self, Phase, ServerState, Snapshot};
use crate::page_perf_tests::{PageHost, frame, owner};
use crate::{Page, theme};
use gpui::{AppContext, TestAppContext, VisualTestContext};
use gpui_kit as gpui;
use serde_json::json;

const RETIRED: &str = "Server session changed. Reconnect before retrying.";

fn labels(visual: &mut VisualTestContext) -> Vec<String> {
    visual.update(|window, _| {
        gpui::base::test_support::snapshots(window)
            .into_iter()
            .filter_map(|node| node.label().map(str::to_owned))
            .collect()
    })
}

#[gpui::test]
fn a_dropped_connection_shows_the_last_reading_without_an_error(cx: &mut TestAppContext) {
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
            v.page = Page::Overview;
            v.server = id.to_string();
            v.snapshot["instanceId"] = json!(instance);
            v.snapshot["config"]["servers"][0]["id"] = json!(id);
            v
        });
        view = Some(entity.clone());
        PageHost(entity)
    });
    let view = view.unwrap();
    view.update(visual, |v, _| {
        let generation = v.resources.begin_refresh("page", false).unwrap();
        v.resources
            .complete("page", generation, Ok(json!({"cpu": {"usage": 1.0}})));
        let generation = v.resources.begin_refresh("page", false).unwrap();
        v.resources
            .complete("page", generation, Err(RETIRED.into()));
    });
    // The application re-renders on connectivity through its subscription.
    let publish = |visual: &mut VisualTestContext, revision, phase| {
        visual.update(|_, cx| {
            connectivity::publish(
                Snapshot {
                    instance_id: instance,
                    revision,
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
        view.update(visual, |_, cx| cx.notify());
        frame(visual);
    };
    publish(visual, 1, Phase::Reconnecting);
    assert!(visual.debug_bounds("overview-offline").is_some());
    assert!(visual.debug_bounds("overview-error-0").is_none());
    let shown = labels(visual);
    assert!(
        !shown.iter().any(|l| l.contains("Server session changed")),
        "{shown:?}"
    );
    // Connected again, a failure is the page's own and says so.
    publish(visual, 2, Phase::Connected);
    assert!(visual.debug_bounds("overview-offline").is_none());
    assert!(visual.debug_bounds("overview-error-0").is_some());
}
