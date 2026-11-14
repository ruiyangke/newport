use crate::page_perf_tests::frame;
use crate::project_library_state::typed_fixture::*;
use crate::project_library_state::typed_test_owner as owner;
use crate::snapshot_fixture as fixture;
use crate::{Newport, Page, theme};
use gpui::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, TestAppContext,
    VisualTestContext, Window, div, px, size,
};
use gpui_kit as gpui;
use std::sync::mpsc::Receiver;

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

/// A server whose Needle was found missing, with setup skipped on Overview.
fn fixture(
    cx: &mut TestAppContext,
) -> (
    Entity<Newport>,
    &mut VisualTestContext,
    Receiver<crate::Work>,
) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        theme::install(false, cx);
        cx.set_reduce_motion(true);
    });
    let mut view = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let entity = cx.new(|cx| {
            let mut v = owner(window, cx);
            v.page = Page::Overview;
            fixture::set(
                &mut v.snapshot_mut().runtime.agent_status,
                fixture::SERVER,
                fixture::installed(false),
            );
            v
        });
        view = Some(entity.clone());
        ScreenHost(entity)
    });
    visual.simulate_resize(size(px(900.), px(800.)));
    let view = view.unwrap();
    let (work, requests) = std::sync::mpsc::sync_channel(64);
    view.update(visual, |v, _| {
        v.work = work.into();
        v.poll_setup();
    });
    let inspect = requests.try_recv().unwrap();
    assert_eq!(kind(&inspect), Some(Kind::ConnectionSetupInspect));
    view.update(visual, |v, cx| {
        crate::snapshot_fixture::settle_connection(
            v,
            &inspect.key,
            inspect.generation,
            newport_protocol::desktop::connection::Reply::SetupInspect(Box::new(
                newport_protocol::desktop::connection::SetupInspection {
                    os: "Ubuntu".into(),
                    architecture: "x86_64".into(),
                    cores: None,
                    ..crate::snapshot_fixture::inspection(1)
                },
            )),
        );
        v.poll_setup();
        cx.notify();
    });
    frame(visual);
    click(visual, "Skip");
    requests.try_iter().for_each(drop);
    (view, visual, requests)
}

fn labels(visual: &mut VisualTestContext) -> Vec<String> {
    visual.update(|window, _| {
        gpui_kit::base::test_support::snapshots(window)
            .iter()
            .filter_map(|n| n.label().map(str::to_owned))
            .collect()
    })
}

#[gpui::test]
fn failed_inventory_keeps_details_and_saved_forwards_without_claiming_no_ports(
    cx: &mut TestAppContext,
) {
    use crate::connections::fixtures::{observation, put};
    use newport_protocol::monitor::ports::discovery as p;
    let (view, visual, _requests) = fixture(cx);
    for saved in [false, true] {
        view.update(visual, |v, cx| {
            fixture::set(
                &mut v.snapshot_mut().runtime.agent_status,
                fixture::SERVER,
                fixture::installed(true),
            );
            v.snapshot_mut().config.tunnels = if saved {
                vec![fixture::saved_forward(fixture::SERVER)]
            } else {
                Vec::new()
            };
            v.navigate(Page::Ports, cx);
            let mut value = observation([]);
            let current = &mut value.data.as_mut().unwrap().current;
            for report in [&mut current.coverage.ipv4, &mut current.coverage.ipv6] {
                *report = p::Report {
                    outcome: p::Outcome::Unavailable,
                    issues: vec![p::Issue {
                        kind: p::IssueKind::PermissionDenied,
                        count: 1,
                        message: "SECRET-REMOTE-ERROR".into(),
                    }],
                };
            }
            value.error = Some("No port inventory source was available.".into());
            value.stale = true;
            value.updated_at = None;
            put(v, value);
            assert!(!v.resources.get("ports").unwrap().has_value());
            assert!(v.resources.get("ports").unwrap().error.is_some());
            assert!(!v.listeners_known(cx));
            cx.notify();
        });
        frame(visual);
        let text = labels(visual);
        assert!(
            !text
                .iter()
                .any(|label| label.starts_with("Nothing to forward yet")),
            "{text:?}"
        );
        assert!(
            !text
                .iter()
                .any(|label| label.contains("last available reading")),
            "{text:?}"
        );
        assert!(text.iter().any(|label| label == "Discovery details"));
        if saved {
            assert!(
                text.iter().any(|label| label.contains("Saved forward")),
                "{text:?}"
            );
            assert!(visual.debug_bounds("connections-discovery-error").is_some());
        } else {
            assert!(
                text.iter()
                    .any(|label| label.starts_with("Port discovery isn’t available")),
                "{text:?}"
            );
        }
        click(visual, "Discovery details");
        let text = labels(visual);
        assert!(visual.debug_bounds("discovery-diagnostics").is_some());
        assert!(
            visual
                .debug_bounds("discovery-diagnostic-IPv4 ports")
                .is_some()
        );
        assert!(
            visual
                .debug_bounds("discovery-diagnostic-IPv6 ports")
                .is_some()
        );
        assert!(!text.iter().any(|label| label.contains("SECRET")));
        visual.simulate_keystrokes("escape");
        frame(visual);
    }
}

fn click(visual: &mut VisualTestContext, label: &str) {
    let bounds = visual.update(|window, _| {
        gpui_kit::base::test_support::snapshots(window)
            .iter()
            .find(|n| n.label() == Some(label))
            .unwrap_or_else(|| panic!("Missing {label}"))
            .bounds()
    });
    visual.simulate_click(bounds.center(), Default::default());
    frame(visual);
}

#[gpui::test]
fn every_page_that_runs_on_needle_says_so_the_same_way_and_asks_nothing(cx: &mut TestAppContext) {
    let (view, visual, requests) = fixture(cx);
    for (page, feature) in [
        (Page::Overview, "Monitoring"),
        (Page::Projects, "Git"),
        (Page::Services, "Services"),
        (Page::Containers, "Containers"),
        (Page::Ports, "Port discovery"),
        (Page::RemoteApps, "Remote apps"),
    ] {
        view.update(visual, |v, cx| {
            v.navigate(page, cx);
            v.tick(cx);
        });
        frame(visual);
        let labels = labels(visual);
        let expected = format!(
            "Needle isn’t set up. {feature} runs on Needle, Newport’s agent on this server. Terminals, files and port forwarding work without it."
        );
        assert!(labels.contains(&expected), "{feature}: {labels:?}");
        assert!(labels.iter().any(|l| l == "Set up Needle"), "{feature}");
        assert!(
            !labels.iter().any(|l| l.contains("Needle is not installed")),
            "{feature} shows the backend's error: {labels:?}"
        );
        let sent: Vec<_> = requests
            .try_iter()
            .map(|r| kind(&r))
            // Local reads: the snapshot and saved project bookmarks.
            .filter(|kind| {
                !matches!(
                    kind,
                    Some(Kind::ConfigurationSnapshot | Kind::PreferencesProjectsList)
                )
            })
            .collect();
        assert!(sent.is_empty(), "{feature} asked the server: {sent:?}");
    }
}

#[gpui::test]
fn saved_forwards_stay_listed_with_the_shared_notice(cx: &mut TestAppContext) {
    let (view, visual, _requests) = fixture(cx);
    view.update(visual, |v, cx| {
        v.snapshot_mut().config.tunnels = vec![fixture::saved_forward(fixture::SERVER)];
        v.navigate(Page::Ports, cx);
    });
    frame(visual);
    let labels = labels(visual);
    assert!(
        labels
            .iter()
            .any(|l| l.starts_with("Needle isn’t set up. Port discovery runs on Needle")),
        "{labels:?}"
    );
    assert!(
        !labels
            .iter()
            .any(|l| l.starts_with("Couldn’t discover ports"))
    );
    // It leads the page's centred column, its action beside the text.
    let (text, button) = visual.update(|window, _| {
        let nodes = gpui_kit::base::test_support::snapshots(window);
        let find = |label: &str| {
            nodes
                .iter()
                .find(|n| n.label().is_some_and(|l| l.starts_with(label)))
                .unwrap()
                .bounds()
        };
        (find("Needle isn’t set up"), find("Set up Needle"))
    });
    let column = visual.debug_bounds("connections-detail").unwrap();
    assert!(text.left() > column.left() && button.right() <= column.right());
    assert!(button.left() > text.right());
    assert!(button.top() < text.bottom() && button.bottom() > text.top());
}

#[gpui::test]
fn set_up_needle_opens_server_setup_from_the_start(cx: &mut TestAppContext) {
    let (view, visual, requests) = fixture(cx);
    view.update(visual, |v, cx| v.navigate(Page::Containers, cx));
    frame(visual);
    click(visual, "Set up Needle");
    assert!(view.read_with(visual, |v, _| v.page == Page::Overview));
    assert!(
        requests
            .try_iter()
            .any(|r| kind(&r) == Some(Kind::ConnectionSetupInspect))
    );
    assert!(!labels(visual).iter().any(|l| l == "Set up Needle"));
}
