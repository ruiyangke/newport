//! Session choices are keyed by server, not by connection resources.
use super::{
    ContainersPanel, Preferences,
    tests::{click, fixture, select, settle},
};
use gpui::{AppContext, TestAppContext};
use gpui_kit as gpui;
use newport_domain::containers_log::Level;
use std::collections::HashSet;

#[gpui::test]
fn detail_and_project_tabs_survive_selection_and_disk_navigation(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "c:stack-web-1");
    click(cx, "Inspect");
    select(&panel, cx, "p:stack");
    assert_eq!(panel.read_with(cx, |p, _| p.tab), super::PROJECT_SERVICES);
    click(cx, "Compose files");
    select(&panel, cx, "c:cache");
    assert_eq!(panel.read_with(cx, |p, _| p.tab), 1);
    select(&panel, cx, "disk");
    select(&panel, cx, "p:stack");
    assert_eq!(panel.read_with(cx, |p, _| p.tab), 2);
    select(&panel, cx, "c:stack-worker-1");
    assert_eq!(panel.read_with(cx, |p, _| p.tab), 1);
    let preferences = panel.read_with(cx, |p, cx| p.preferences(cx));
    assert_eq!((preferences.detail_tab, preferences.project_tab), (1, 2));
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn replacement_panel_restores_only_session_preferences(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    let preferences = Preferences {
        query: "stack".into(),
        selected: Some("p:stack".into()),
        collapsed: HashSet::from(["stack".into()]),
        detail_tab: 1,
        project_tab: 2,
        live: false,
        log_level: Level::Warnings,
        log_query: "connection".into(),
    };
    cx.update(|window, cx| {
        panel.update(cx, |p, cx| {
            p.restore_preferences(preferences.clone(), window, cx);
            p.jobs_open.insert("stack".into());
            p.reveal_values = true;
            p.revealed.insert("API_TOKEN".into());
            p.labels_open = true;
            p.review = Some(newport_client::containers::Action::Remove {
                containers: vec!["captured-id".into()],
                names: vec!["captured-name".into()],
            });
        })
    });
    let saved = panel.read_with(cx, |p, cx| p.preferences(cx));
    assert_eq!(saved, preferences);
    let replacement = cx.update(|window, cx| {
        cx.new(|cx| {
            let mut p = ContainersPanel::with_client(backend.client(), window, cx);
            p.restore_preferences(saved, window, cx);
            p
        })
    });
    settle(&replacement, cx);
    replacement.read_with(cx, |p, cx| {
        assert_eq!(p.preferences(cx), preferences);
        assert_eq!(p.tab, 2);
        assert!(p.jobs_open.is_empty());
        assert!(!p.reveal_values);
        assert!(p.revealed.is_empty());
        assert!(!p.labels_open);
        assert!(p.review.is_none());
        assert!(p.compose_state.proposed.is_none());
        assert!(p.client.selected_job().is_none());
    });
    let other = cx.update(|window, cx| {
        cx.new(|cx| ContainersPanel::with_client(backend.client(), window, cx))
    });
    assert_eq!(
        other.read_with(cx, |p, cx| p.preferences(cx)),
        Preferences::default()
    );
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn default_selection_changes_restore_the_matching_tab_and_clamp_restored_indices(
    cx: &mut TestAppContext,
) {
    let (panel, cx, backend) = fixture(cx);
    cx.update(|window, cx| {
        panel.update(cx, |p, cx| {
            p.restore_preferences(
                Preferences {
                    query: "stack".into(),
                    detail_tab: usize::MAX,
                    project_tab: usize::MAX,
                    ..Default::default()
                },
                window,
                cx,
            );
        })
    });
    settle(&panel, cx);
    assert_eq!(
        panel.read_with(cx, |p, _| (p.active_selection.clone(), p.tab)),
        (Some("c:stack-worker-1".into()), 1)
    );
    cx.update(|window, cx| {
        panel.update(cx, |p, cx| {
            p.search
                .update(cx, |search, cx| search.set_value("fixture/web", window, cx));
            p.sync_selection(cx);
        })
    });
    settle(&panel, cx);
    assert_eq!(
        panel.read_with(cx, |p, _| (p.active_selection.clone(), p.tab)),
        (Some("p:stack".into()), super::PROJECT_SERVICES)
    );
    let saved = panel.read_with(cx, |p, cx| p.preferences(cx));
    assert!(saved.live);
    assert_eq!(
        (saved.detail_tab, saved.project_tab),
        (1, super::PROJECT_SERVICES)
    );
    assert_eq!(backend.writes(), 0);
}
