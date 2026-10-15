//! Poll the real retained browser through strict, delayed local IPC replies.
use crate::file_tree::tests::{Host, reply};
use crate::{Newport, Page, file_tree};
use gpui_kit::{
    AppContext, Context, Entity, IntoElement, Render, TestAppContext, VisualTestContext, Window,
    div,
};
use serde_json::{Value, json};
use std::{thread, time::Duration};

struct IdleHost;
impl Render for IdleHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn fixture(cx: &mut TestAppContext) -> (Entity<Newport>, &mut VisualTestContext, Host) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        crate::theme::install(false, cx);
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
            browser.tree = cx.new(|_| {
                file_tree::FolderTree::new(newport_client::file_browser::folders::Folders::new(
                    peer.backend(),
                    "fixture".into(),
                    browser.connection.clone(),
                ))
            });
            browser.active = true;
            host = Some(peer);
            view
        });
        owner = Some(view);
        // This host intentionally does not run Newport's unrelated startup or
        // periodic workers. Every browser poll below is driven by the test.
        IdleHost
    });
    (owner.unwrap(), visual, host.unwrap())
}

fn settle(view: &Entity<Newport>, cx: &mut VisualTestContext, host: &Host) -> bool {
    let mut changed = false;
    for _ in 0..1000 {
        changed |= view.update(cx, |view, cx| view.poll_file_browsers(cx));
        while let Some((stream, request)) = host.try_receive() {
            assert_eq!(request.method, "files_cancel");
            reply(stream, Ok(Value::Null));
        }
        cx.run_until_parked();
        if !view.read_with(cx, |view, _| {
            view.file_browsers["fixture"].reads.has_pending()
        }) {
            return changed;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("file browser did not settle");
}

#[gpui_kit::test]
fn background_file_results_update_cache_without_invalidating_other_pages(cx: &mut TestAppContext) {
    let (view, cx, host) = fixture(cx);
    for visibility in ["other page", "other server", "inactive", "visible"] {
        view.update(cx, |view, cx| {
            view.page = if visibility == "other page" {
                Page::Overview
            } else {
                Page::Files
            };
            view.server = if visibility == "other server" {
                "other"
            } else {
                "fixture"
            }
            .into();
            let browser = view.file_browsers.get_mut("fixture").unwrap();
            browser.active = visibility != "inactive";
            browser.reads.navigate(format!("/{visibility}"));
            view.poll_file_browsers(cx);
        });
        let stream = host.listing(&format!("/{visibility}"));
        reply(
            stream,
            Ok(json!({"path":format!("/{visibility}"),"entries":[
                {"name":"retained.txt","path":format!("/{visibility}/retained.txt"),"kind":"file"}
            ]})),
        );
        assert_eq!(
            settle(&view, cx, &host),
            visibility == "visible",
            "{visibility}"
        );
        view.read_with(cx, |view, _| {
            let browser = &view.file_browsers["fixture"];
            assert_eq!(browser.directory, format!("/{visibility}"));
            assert_eq!(
                browser.reads.listing().unwrap()["entries"][0]["name"],
                "retained.txt"
            );
        });
    }
    // The visible browser must still deliver errors and keep the last listing.
    view.update(cx, |view, cx| {
        view.file_browsers
            .get_mut("fixture")
            .unwrap()
            .reads
            .navigate("/denied".into());
        view.poll_file_browsers(cx);
    });
    reply(host.listing("/denied"), Err("Permission denied"));
    assert!(settle(&view, cx, &host));
    view.read_with(cx, |view, _| {
        let browser = &view.file_browsers["fixture"];
        assert_eq!(browser.reads.listing_error(), Some("Permission denied"));
        assert_eq!(browser.directory, "/visible");
    });
}

#[gpui_kit::test]
fn leaving_files_drains_cancelled_reads_without_redrawing_or_replacing_cache(
    cx: &mut TestAppContext,
) {
    let (view, cx, host) = fixture(cx);
    view.update(cx, |view, cx| {
        view.file_browsers
            .get_mut("fixture")
            .unwrap()
            .reads
            .navigate("/late".into());
        view.poll_file_browsers(cx);
    });
    let pending = host.listing("/late");
    view.update(cx, |view, _| {
        view.page = Page::Overview;
        let browser = view.file_browsers.get_mut("fixture").unwrap();
        browser.active = false;
        browser.reads.suspend();
    });
    reply(pending, Ok(json!({"path":"/late","entries":[]})));
    assert!(!settle(&view, cx, &host));
    view.read_with(cx, |view, _| {
        let browser = &view.file_browsers["fixture"];
        assert!(browser.reads.listing().is_none());
        assert_eq!(browser.directory, ".");
    });
}

#[gpui_kit::test]
fn file_filter_notifications_follow_the_captured_server_and_connection(cx: &mut TestAppContext) {
    use gpui_kit::component::input::InputEvent;
    use std::{cell::Cell, rc::Rc};
    let (view, cx, _host) = fixture(cx);
    let notifications = Rc::new(Cell::new(0));
    let observed = notifications.clone();
    let _subscription =
        cx.update(|_, cx| cx.observe(&view, move |_, _| observed.set(observed.get() + 1)));
    let (filter, connection) = view.read_with(cx, |view, _| {
        let browser = &view.file_browsers["fixture"];
        (browser.filter.clone(), browser.connection.clone())
    });
    for scope in [
        "other page",
        "other server",
        "replacement",
        "inactive",
        "visible",
    ] {
        view.update(cx, |view, _| {
            view.page = if scope == "other page" {
                Page::Overview
            } else {
                Page::Files
            };
            view.server = if scope == "other server" {
                "other"
            } else {
                "fixture"
            }
            .into();
            let browser = view.file_browsers.get_mut("fixture").unwrap();
            browser.active = scope != "inactive";
            browser.page = 2;
            browser.connection = if scope == "replacement" {
                json!({"replacement":true})
            } else {
                connection.clone()
            };
        });
        filter.update(cx, |_, cx| cx.emit(InputEvent::Change));
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view.file_browsers["fixture"].page),
            if scope == "visible" { 0 } else { 2 },
            "{scope}"
        );
        assert_eq!(
            notifications.get(),
            usize::from(scope == "visible"),
            "{scope}"
        );
    }
}

#[gpui_kit::test]
fn projected_file_targets_revalidate_after_navigation_and_connection_changes(
    cx: &mut TestAppContext,
) {
    let (view, cx, host) = fixture(cx);
    view.update(cx, |view, cx| {
        view.file_browsers
            .get_mut("fixture")
            .unwrap()
            .reads
            .navigate("/work".into());
        view.poll_file_browsers(cx);
    });
    let listing = json!({"path":"/work", "entries":(0..400).map(|i| json!({
        "name":format!("file-{i:03}"),"path":format!("/work/{i}"),"kind":"file"
    })).collect::<Vec<_>>()});
    reply(host.listing("/work"), Ok(listing.clone()));
    assert!(settle(&view, cx, &host));
    let targets = view.read_with(cx, |view, cx| {
        let entries = view.file_browsers["fixture"].entries(cx);
        let directory = view.file_action_target("/work").unwrap();
        entries[super::page_range(entries.len(), 9)]
            .iter()
            .map(|entry| directory.for_entry(entry).unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(targets.len(), 40);
    assert_eq!(targets[0].path, "/work/360");
    assert!(view.read_with(cx, |view, _| {
        targets
            .iter()
            .all(|target| view.file_target_current(target))
    }));
    let connection = targets[0].connection.clone();
    for state in [
        "other page",
        "other server",
        "inactive",
        "replacement",
        "restored",
    ] {
        view.update(cx, |view, _| {
            view.page = if state == "other page" {
                Page::Overview
            } else {
                Page::Files
            };
            view.server = if state == "other server" {
                "other"
            } else {
                "fixture"
            }
            .into();
            let browser = view.file_browsers.get_mut("fixture").unwrap();
            browser.active = state != "inactive";
            browser.connection = if state == "replacement" {
                json!(["replacement"])
            } else {
                connection.clone()
            };
        });
        assert_eq!(
            view.read_with(cx, |view, _| view.file_target_current(&targets[0])),
            state == "restored",
            "{state}"
        );
    }
    view.update(cx, |view, cx| {
        view.file_browsers
            .get_mut("fixture")
            .unwrap()
            .reads
            .refresh();
        view.poll_file_browsers(cx);
    });
    assert!(
        view.read_with(cx, |view, _| view.file_action_target("/work").is_none()
            && !view.file_target_current(&targets[0]))
    );
    let mut updated = listing;
    updated["entries"][360]["kind"] = json!("directory");
    updated["entries"].as_array_mut().unwrap().remove(361);
    reply(host.listing("/work"), Ok(updated));
    assert!(settle(&view, cx, &host));
    view.read_with(cx, |view, _| {
        assert!(!view.file_target_current(&targets[0]), "changed kind");
        assert!(!view.file_target_current(&targets[1]), "removed entry");
        assert!(view.file_target_current(&targets[2]), "unchanged entry");
    });
}
