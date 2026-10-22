//! Automatic, bounded status reads for the visible project library.
use super::*;
use newport_client::project_checks::Advance;
pub(super) use newport_client::project_checks::Batch;

impl Newport {
    fn project_library_visible(&self) -> bool {
        self.page == Page::Projects
            && self.selection.is_none()
            && self.project_folder.is_none()
            && self.project_add.is_none()
            && self.project_create.is_none()
            && self.project_edit.is_none()
            && self.window_visible
            && self.configuration_available()
    }

    pub(super) fn project_status_visible(&self, key: &str) -> bool {
        self.project_library_visible()
            && self.visible_project_rows().iter().any(|project| {
                project["serverId"].as_str() == Some(&self.server)
                    && project_library::status_key(project) == key
            })
    }

    pub(super) fn poll_project_checks(&mut self, cx: &mut Context<Self>) {
        self.poll_project_checks_at(Instant::now(), cx);
    }
    fn poll_project_checks_at(&mut self, now: Instant, cx: &mut Context<Self>) {
        let library = self.project_library_visible() && !self.needle_missing();
        if !library {
            self.project_checks = None;
            return;
        }
        if self.project_checks.is_none() {
            let next = self
                .visible_project_rows()
                .iter()
                .find(|project| {
                    project["serverId"].as_str() == Some(&self.server)
                        && self
                            .resources
                            .get(&project_library::status_key(project))
                            .is_none_or(|r| {
                                (!r.pending && r.finished_at.is_none())
                                    || r.poll_due(now, Duration::from_secs(30), true)
                            })
                })
                .cloned();
            if let Some(project) = next {
                self.project_checks = Some(Batch::new(
                    self.server.clone(),
                    self.project_query(cx),
                    vec![project],
                ));
            }
        }
        let query = self.project_query(cx);
        let Some(batch) = self.project_checks.as_mut() else {
            return;
        };
        let next = batch.advance(&self.resources, &self.server, &query, library);
        match next {
            Advance::Read { key, arguments } => {
                let presentation = |view: &Self| {
                    view.resources.get(&key).map(|row| {
                        (
                            row.display_pending(),
                            row.error.clone(),
                            row.presentation_revision,
                        )
                    })
                };
                let previous = presentation(self);
                let generation = self.resources.begin_refresh(&key, false);
                self.dispatch_request(&key, "git_request", arguments, generation);
                if previous != presentation(self) {
                    cx.notify();
                }
            }
            Advance::Waiting => {}
            Advance::Finished | Advance::Cancelled => {
                self.project_checks = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Page;
    use gpui_kit::{ListOffset, TestAppContext, px};
    use serde_json::json;

    #[gpui_kit::test]
    fn completed_checks_only_notify_the_visible_project_rows(cx: &mut TestAppContext) {
        use gpui_kit::{AppContext, Context, IntoElement, Render};
        struct IdleHost;
        impl Render for IdleHost {
            fn render(
                &mut self,
                _: &mut gpui_kit::Window,
                _: &mut Context<Self>,
            ) -> impl IntoElement {
                gpui_kit::div()
            }
        }
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
        });
        let mut view = None;
        let (_, visual) = cx.add_window_view(|window, cx| {
            view = Some(cx.new(|cx| crate::page_perf_tests::owner(window, cx)));
            IdleHost
        });
        let view = view.unwrap();
        crate::page_perf_tests::settle_update_check(&view, visual);
        let (work, _requests) = std::sync::mpsc::sync_channel(32);
        let (replies, inbox) = std::sync::mpsc::channel();
        let projects: Vec<_> = (0..3)
            .map(|id| {
                json!({
                    "id":id.to_string(), "serverId":"fixture", "name":id.to_string(),
                    "path":{"display":format!("/{id}"), "bytesB64":id.to_string()}
                })
            })
            .collect();
        let key = super::project_library::status_key(&projects[0]);
        view.update(visual, |view, _| {
            view.work = work.into();
            view.replies = inbox;
            view.project_library_state().list.sync(&projects, 900.);
            for project in &projects {
                let key = super::project_library::status_key(project);
                let generation = view.resources.begin(&key).unwrap();
                view.resources
                    .complete(&key, generation, Ok(json!({"totalEntries":0})));
            }
        });
        let notices = visual.new(|cx: &mut Context<usize>| {
            cx.observe(&view, |count, _, _| *count += 1).detach();
            0
        });
        // A completed query must remain available even when its presentation
        // has moved away. Returning to the row reads the retained result.
        let mut changes = 0;
        for (case, expected) in [
            ("offscreen", 0),
            ("other page", 0),
            ("hidden", 0),
            ("other server", 0),
            ("repository", 0),
            ("configuration unavailable", 0),
            ("visible", 1),
            ("identical", 0),
            ("visible error", 1),
            ("hidden recovery", 0),
        ] {
            let generation = view.update(visual, |view, _| {
                view.page = if case == "other page" {
                    Page::Integration
                } else {
                    Page::Projects
                };
                view.window_visible = !matches!(case, "hidden" | "hidden recovery");
                view.selection = (case == "repository").then(|| projects[0].clone());
                if case == "configuration unavailable" {
                    view.snapshot["loadError"] = json!("Profile unavailable");
                } else {
                    view.snapshot.as_object_mut().unwrap().remove("loadError");
                }
                view.server = if case == "other server" {
                    "other"
                } else {
                    "fixture"
                }
                .into();
                view.project_library_state()
                    .list
                    .state
                    .scroll_to(ListOffset {
                        item_ix: if case == "offscreen" { 2 } else { 0 },
                        offset_in_item: px(0.),
                    });
                view.resources.begin_refresh(&key, false).unwrap()
            });
            let result = if case == "visible error" {
                Err("Offline".to_owned())
            } else {
                if case != "identical" {
                    changes += 1;
                }
                Ok(json!({"totalEntries":changes, "truncated":false}))
            };
            let before = notices.read_with(visual, |count, _| *count);
            replies
                .send(crate::Completion {
                    server: "fixture".into(),
                    key: key.clone(),
                    generation,
                    result: result.clone().map_err(Into::into),
                    image: None,
                    pdf: None,
                    pdf_text: None,
                })
                .unwrap();
            view.update(visual, |view, cx| view.tick(cx));
            visual.run_until_parked();
            assert_eq!(
                notices.read_with(visual, |count, _| *count) - before,
                expected,
                "{case}"
            );
            view.read_with(visual, |view, _| {
                let resource = view.resources.get(&key).unwrap();
                assert!(!resource.pending);
                match result {
                    Ok(value) => assert_eq!(resource.value, value),
                    Err(error) => assert_eq!(resource.error.as_deref(), Some(error.as_str())),
                }
            });
        }
    }

    #[gpui_kit::test]
    fn unchanged_visible_library_checks_do_not_redraw_on_admission_completion_or_batch_end(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::{AppContext, Context};
        use std::time::{Duration, Instant};
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (owner, visual) = cx.add_window_view(crate::page_perf_tests::owner);
        let (work, requests) = std::sync::mpsc::sync_channel(16);
        let project = json!({"id":"one", "serverId":"fixture", "name":"One",
            "path":{"display":"/one","bytesB64":"L29uZQ=="}});
        let status = json!({"truncated":false,"totalEntries":0,
            "head":{"name":{"display":"refs/heads/main"}}});
        let key = newport_client::project_checks::status_key(&project);
        owner.update(visual, |this, _| {
            this.work = work.into();
            this.page = Page::Projects;
            this.window_visible = true;
            this.project_library_state().list.sync(&[project], 900.);
            let generation = this.resources.begin(&key).unwrap();
            this.resources
                .complete(&key, generation, Ok(status.clone()));
        });
        visual.run_until_parked();
        let notices = visual.new(|cx: &mut Context<usize>| {
            cx.observe(&owner, |count, _, _| *count += 1).detach();
            0
        });
        for _ in 0..3 {
            owner.update(visual, |this, cx| {
                this.poll_project_checks_at(Instant::now() + Duration::from_secs(31), cx);
            });
            visual.run_until_parked();
            let request = requests.try_recv().unwrap();
            assert_eq!(request.key, key);
            // Use the same resource settlement/presentation decision as the
            // shell; the batch's next poll must not add a redundant redraw.
            owner.update(visual, |this, _| {
                let previous = this.resources.get(&key).unwrap().presentation_revision;
                this.resources
                    .complete(&key, request.generation, Ok(status.clone()));
                assert_eq!(
                    this.resources.get(&key).unwrap().presentation_revision,
                    previous
                );
            });
            owner.update(visual, |this, cx| this.poll_project_checks(cx));
            visual.run_until_parked();
            assert!(requests.try_recv().is_err());
            assert_eq!(notices.read_with(visual, |count, _| *count), 0);
        }
    }

    #[gpui_kit::test]
    fn automatic_status_reads_are_visible_sequential_cached_and_scoped(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (owner, cx) = cx.add_window_view(crate::page_perf_tests::owner);
        owner.update(cx, |this, cx| {
            let (work, requests) = std::sync::mpsc::sync_channel(32);
            this.work = work.into();
            this.page = Page::Projects;
            this.window_visible = true;
            let projects: Vec<_> = (0..1000)
                .map(|id| {
                    json!({
                        "id":id.to_string(),"serverId":"fixture", "name":id.to_string(),
                        "path":{"display":format!("/{id}"),"bytesB64":id.to_string()}
                    })
                })
                .collect();
            this.project_library_state().list.sync(&projects, 900.);
            // No viewport is laid out yet: preload only its first row.
            this.poll_project_checks(cx);
            let first = requests.try_recv().unwrap();
            assert_eq!(first.args["request"]["method"], "repo.status_summary");
            assert_eq!(first.args["request"]["params"]["path"], projects[0]["path"]);
            this.poll_project_checks(cx);
            assert!(requests.try_recv().is_err(), "pending reads must coalesce");
            let status = json!({"truncated":false,"totalEntries":0,"head":{"name":{"display":"refs/heads/main"}}});
            this.resources.complete(&first.key, first.generation, Ok(status.clone()));
            let later = std::time::Instant::now() + std::time::Duration::from_secs(31);
            this.poll_project_checks_at(later, cx);
            this.poll_project_checks_at(later, cx);
            let refresh = requests.try_recv().unwrap();
            assert_eq!(refresh.key, first.key);
            assert_eq!(this.resources.get(&first.key).unwrap().value, status);
            this.resources.complete(&refresh.key, refresh.generation, Err("Offline".into()));
            let later = std::time::Instant::now() + std::time::Duration::from_secs(31);
            this.poll_project_checks_at(later, cx);
            this.poll_project_checks_at(later, cx);
            let retry = requests.try_recv().unwrap();
            let reading = newport_client::project_bookmark::reading(this.resources.get(&first.key));
            assert!(!reading.pending && reading.previous);
            assert_eq!(reading.error, Some("Offline"));
            assert_eq!(reading.status.unwrap().changes, 0);
            this.resources
                .complete(&retry.key, retry.generation, Err("Read failed".into()));
            this.poll_project_checks(cx);
            this.poll_project_checks(cx);
            assert!(
                requests.try_recv().is_err(),
                "failed reads must respect refresh interval"
            );
            this.project_library_state().list.state.scroll_to(ListOffset {
                item_ix: 40,
                offset_in_item: px(0.),
            });
            this.poll_project_checks(cx);
            let next = requests.try_recv().unwrap();
            assert_eq!(next.args["request"]["params"]["path"], projects[40]["path"]);
            assert!(
                requests.try_recv().is_err(),
                "offscreen repositories must not be swept"
            );
            this.resources
                .complete(&next.key, next.generation, Ok(json!({})));
            this.poll_project_checks(cx);
            this.project_library_state().list.state.scroll_to(ListOffset {
                item_ix: 80,
                offset_in_item: px(0.),
            });
            this.window_visible = false;
            this.poll_project_checks(cx);
            this.window_visible = true;
            this.page = Page::Files;
            this.poll_project_checks(cx);
            assert!(
                requests.try_recv().is_err(),
                "hidden and other pages must not schedule"
            );
        });
    }
}
