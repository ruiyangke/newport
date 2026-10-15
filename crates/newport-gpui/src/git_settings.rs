//! Repository-local preferences with explicit review after concurrent edits.
use super::*;
use newport_client::git_configuration::{
    settings_action as action, valid_configuration as valid_config,
};

const READ_KEYS: [&str; 3] = ["git-list", "git-remote-status", "git-repository-settings"];

#[derive(Default)]
struct ReadFeedback {
    pending: bool,
    errors: Vec<String>,
}
fn read_feedback(
    resources: &newport_client::resources::Resources,
    list_error: Option<&str>,
) -> ReadFeedback {
    let mut feedback = ReadFeedback::default();
    for key in READ_KEYS {
        let Some(resource) = resources.get(key) else {
            feedback.pending = true;
            continue;
        };
        feedback.pending |= resource.pending;
        if let Some(error) = &resource.error
            && !feedback.errors.contains(error)
        {
            feedback.errors.push(error.clone());
        }
    }
    if let Some(error) = list_error
        && !feedback.errors.iter().any(|message| message == error)
    {
        feedback.errors.push(error.to_owned());
    }
    feedback
}
fn read_feedback_view(
    feedback: &ReadFeedback,
    has_form: bool,
    retry: Button,
    cx: &App,
) -> AnyElement {
    div()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_2()
        .when(
            feedback.errors.is_empty() && feedback.pending && !has_form,
            |view| view.child(project_loading::resource("git-repository-settings", 6)),
        )
        .children(feedback.errors.iter().enumerate().map(|(index, message)| {
            div()
                .id(("settings-read-error", index))
                .role(accesskit::Role::Alert)
                .aria_label(message.clone())
                .test_support()
                .min_w_0()
                .p_3()
                .bg(cx.theme().danger.opacity(0.12))
                .text_color(cx.theme().danger)
                .child(message.clone())
        }))
        .when(!feedback.errors.is_empty(), |view| {
            view.child(
                div()
                    .debug_selector(|| "settings-read-retry".into())
                    .child(retry),
            )
        })
        .into_any_element()
}

pub(super) struct Form {
    id: uuid::Uuid,
    initial: Value,
    reviewed: Value,
    snapshot: String,
    pub(super) needs_review: bool,
    name: Entity<InputState>,
    email: Entity<InputState>,
    prune: bool,
    _subscriptions: Vec<Subscription>,
}
impl Form {
    pub(super) fn initial_focus(&self, cx: &App) -> FocusHandle {
        self.name.read(cx).focus_handle(cx)
    }

    pub(super) fn draft(&self, cx: &App) -> (uuid::Uuid, String, String, bool) {
        (
            self.id,
            self.name.read(cx).value().to_string(),
            self.email.read(cx).value().to_string(),
            self.prune,
        )
    }
}
impl Newport {
    pub(super) fn repository_settings_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let feedback = read_feedback(&self.resources, self.git.error.as_deref());
        let retry = crate::controls::button("retry-modal-repository-settings")
            .label("Retry settings")
            .on_click(cx.listener(|this, _, _, cx| {
                // Refresh every prerequisite, retaining any existing form draft.
                // A retained draft still requires acceptance of the new review.
                this.git_refresh();
                cx.notify();
            }));
        div()
            .min_w_0()
            .flex()
            .flex_col()
            .child(read_feedback_view(
                &feedback,
                self.git.settings.is_some(),
                retry,
                cx,
            ))
            .child(self.repository_settings_view(cx))
            .into_any_element()
    }
    fn can_configure_repository(&self) -> bool {
        self.git.tab == 6
            && !self.git_mutation_blocks_write()
            && self.git.repo["capabilities"]["readOnly"] == false
            && self.git.error.is_none()
            && ["git-list", "git-remote-status"].iter().all(|key| {
                self.resources
                    .get(key)
                    .is_some_and(|r| !r.pending && r.error.is_none())
            })
            && self
                .resources
                .get("git-repository-settings")
                .is_some_and(|r| !r.pending && r.error.is_none())
            && valid_config(&self.value("git-repository-settings")["configuration"])
            && self.value("git-remote-status")["snapshot"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
    }
    fn open_repository_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_configure_repository()
            || self.git.remote_edit.is_some()
            || self.git.pull_form.is_some()
            || self.git.settings.is_some()
        {
            return;
        }
        let config = self.value("git-repository-settings")["configuration"].clone();
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Author name"));
        let email = cx.new(|cx| InputState::new(window, cx).placeholder("Author email"));
        name.update(cx, |input, cx| {
            input.set_value(
                config["userName"].as_str().unwrap_or_default().to_owned(),
                window,
                cx,
            )
        });
        email.update(cx, |input, cx| {
            input.set_value(
                config["userEmail"].as_str().unwrap_or_default().to_owned(),
                window,
                cx,
            )
        });
        let subscriptions = vec![
            Self::project_form_input(
                &name,
                project_dialog::Kind::Settings,
                Self::save_repository_settings,
                cx,
            ),
            Self::project_form_input(
                &email,
                project_dialog::Kind::Settings,
                Self::save_repository_settings,
                cx,
            ),
        ];
        self.git.settings_open = false;
        let form_id = uuid::Uuid::new_v4();
        self.git.settings = Some(Form {
            id: form_id,
            initial: config.clone(),
            reviewed: config.clone(),
            snapshot: text(&self.value("git-remote-status"), "snapshot"),
            needs_review: false,
            name,
            email,
            prune: config["fetchPrune"] == true,
            _subscriptions: subscriptions,
        });
        // The dialog may already be showing its loading placeholder. Its
        // opening-frame autofocus cannot target inputs that did not exist yet.
        if self.project_dialog_open {
            let owner = cx.weak_entity();
            window.on_next_frame(move |window, cx| {
                let _ = owner.update(cx, |this, cx| {
                    if this.project_dialog_open
                        && this.page == Page::Projects
                        && this.git.tab == 6
                        && let Some(form) = &this.git.settings
                        && form.id == form_id
                    {
                        form.initial_focus(cx).focus(window, cx);
                    }
                });
            });
        }
        cx.notify();
    }
    fn save_repository_settings(&mut self, cx: &mut Context<Self>) {
        if !self.can_configure_repository() {
            return;
        }
        let Some(form) = &self.git.settings else {
            return;
        };
        if form.needs_review {
            return;
        }
        let draft = form.draft(cx);
        let Some(action) = action(&form.initial, &form.reviewed, &draft.1, &draft.2, draft.3)
        else {
            return;
        };
        let snapshot = form.snapshot.clone();
        self.start_git_mutation(
            self.git_repository_id(),
            "Update repository settings".into(),
            move |backend, server, repo, id| {
                backend.apply_reviewed_action(server, repo, &snapshot, action, id)
            },
            cx,
        );
        if let Some(state) = self.git_mutation.as_mut() {
            state.settings_draft = Some(draft);
        }
    }
    fn review_repository_settings(&mut self, cx: &mut Context<Self>) {
        if !self.can_configure_repository() {
            return;
        }
        let snapshot = text(&self.value("git-remote-status"), "snapshot");
        let configuration = self.value("git-repository-settings")["configuration"].clone();
        if let Some(form) = self.git.settings.as_mut() {
            form.reviewed = configuration;
            form.snapshot = snapshot;
            form.needs_review = false;
        }
        cx.notify();
    }
    pub(super) fn repository_settings_button(&self, cx: &mut Context<Self>) -> AnyElement {
        crate::controls::button("repository-settings")
            .label("Repository settings")
            .disabled(
                self.git_mutation_blocks_write()
                    || self.git.repo["capabilities"]["readOnly"] != false
                    || self.git.settings_open
                    || self.git.settings.is_some()
                    || self.git.remote_edit.is_some()
                    || self.git.pull_form.is_some(),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.git.settings_open = true;
                this.git.detail_open = true;
                this.resources.invalidate("git-repository-settings");
                this.git_load_repository_settings();
                cx.notify();
            }))
            .into_any_element()
    }
    pub(super) fn repository_settings_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.git.settings else {
            return div().into_any_element();
        };
        let busy = self.git_mutation_blocks_write();
        let draft = form.draft(cx);
        let configuration = self.value("git-repository-settings");
        let latest = &configuration["configuration"];
        div()
            .flex()
            .flex_col()
            .gap_2()
            .px_4()
            .py_2()
            .child(
                div()
                    .id("repository-settings-title")
                    .role(accesskit::Role::Heading)
                    .aria_label("Repository settings")
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Repository settings"),
            )
            .when(form.needs_review, |view| {
                view.child(project_dialog::description(
                    "repository-settings-review-required",
                    "Your draft was kept. Review the current settings before saving again.",
                ))
                .when(self.can_configure_repository(), |view| {
                    view.child(project_dialog::description(
                        "repository-settings-current",
                        format!(
                            "Current identity: {} <{}> · Prune on fetch: {}",
                            text(latest, "userName"),
                            text(latest, "userEmail"),
                            if latest["fetchPrune"] == true {
                                "On"
                            } else {
                                "Off"
                            }
                        ),
                    ))
                })
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            crate::controls::button("refresh-settings-review")
                                .label("Refresh settings")
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.git_refresh();
                                    cx.notify();
                                })),
                        )
                        .child(
                            crate::controls::button("accept-settings-review")
                                .label("Use reviewed settings")
                                .disabled(!self.can_configure_repository())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.review_repository_settings(cx)
                                })),
                        ),
                )
            })
            .child(label("Commit author"))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div().flex_1().min_w_0().child(label("Name")).child(
                            crate::controls::input(&form.name)
                                .aria_label("Git author name")
                                .disabled(busy),
                        ),
                    )
                    .child(
                        div().flex_1().min_w_0().child(label("Email")).child(
                            crate::controls::input(&form.email)
                                .aria_label("Git author email")
                                .disabled(busy),
                        ),
                    ),
            )
            .child(
                crate::controls::checkbox("git-fetch-prune")
                    .label("Prune deleted branches on fetch")
                    .checked(form.prune)
                    .disabled(busy)
                    .on_click(cx.listener(|this, value: &bool, _, cx| {
                        if let Some(form) = this.git.settings.as_mut() {
                            form.prune = *value;
                        }
                        cx.notify();
                    })),
            )
            .child(
                project_dialog::description(
                    "repository-settings-prune-description",
                    "Removes remote-tracking branches that no longer exist upstream when fetching.",
                )
                .text_xs(),
            )
            .child(
                project_dialog::actions()
                    .child(
                        crate::controls::button("cancel-repository-settings")
                            .label("Cancel")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.git.settings = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        crate::controls::button("save-repository-settings")
                            .loading(self.git_write_pending())
                            .label("Save settings")
                            .disabled(
                                !self.can_configure_repository()
                                    || form.needs_review
                                    || action(
                                        &form.initial,
                                        &form.reviewed,
                                        &draft.1,
                                        &draft.2,
                                        draft.3,
                                    )
                                    .is_none(),
                            )
                            .on_click(
                                cx.listener(|this, _, _, cx| this.save_repository_settings(cx)),
                            ),
                    ),
            )
            .into_any_element()
    }
}

impl Newport {
    pub(super) fn git_load_repository_settings(&mut self) {
        // The current agent exposes repository preferences through repo.remotes.
        // Keep this legacy read demand-driven, separate from name pagination.
        self.request(
            "git-repository-settings",
            "native.repository_settings",
            json!({"serverId":self.server,"repoId":self.git_repository_id()}),
        );
    }
    pub(super) fn sync_repository_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page == Page::Projects
            && self.git.tab == 6
            && self.git.settings_open
            && self.can_configure_repository()
        {
            self.open_repository_settings(window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{READ_KEYS, read_feedback, read_feedback_view};
    use gpui_kit::base::test_support::snapshots;
    use gpui_kit::component::input::InputState;
    use gpui_kit::{
        self as gpui, Context, Entity, IntoElement, Render, Window, div, prelude::*, px, size,
    };
    use newport_client::resources::Resources;
    use serde_json::json;

    struct FeedbackHost {
        resources: Resources,
        has_form: bool,
        draft: Entity<InputState>,
    }
    impl Render for FeedbackHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .w_full()
                .flex()
                .flex_col()
                .child(read_feedback_view(
                    &read_feedback(&self.resources, None),
                    self.has_form,
                    crate::controls::button("retry-settings-test").label("Retry settings"),
                    cx,
                ))
                .when(self.has_form, |view| {
                    view.child(crate::controls::input(&self.draft).aria_label("Git author name"))
                })
        }
    }
    fn settled(resources: &mut Resources, key: &str, error: Option<&str>) {
        let generation = resources.begin(key).unwrap();
        resources.complete(
            key,
            generation,
            error.map_or(Ok(json!({"loaded":true})), |message| Err(message.into())),
        );
    }
    #[gpui::test]
    fn settings_dependency_feedback_handles_each_read_and_preserves_a_visible_draft(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui::init(cx);
            crate::theme::install(false, cx);
        });
        let (view, visual) = cx.add_window_view(|window, cx| FeedbackHost {
            resources: Resources::default(),
            has_form: false,
            draft: cx.new(|cx| {
                let mut input = InputState::new(window, cx);
                input.set_value("Unsaved author", window, cx);
                input
            }),
        });
        visual.simulate_resize(size(px(400.), px(500.)));
        for key in READ_KEYS {
            view.update(visual, |view, cx| {
                view.has_form = false;
                view.resources = Resources::default();
                for ready in READ_KEYS.into_iter().filter(|ready| *ready != key) {
                    settled(&mut view.resources, ready, None);
                }
                view.resources.begin(key);
                cx.notify();
            });
            visual.update(|window, cx| window.draw(cx).clear(cx));
            assert!(
                visual.debug_bounds("settings-loading-identity-0").is_some(),
                "waiting for {key} must show the form skeleton"
            );
            view.update(visual, |view, cx| {
                view.resources.invalidate(key);
                settled(&mut view.resources, key, Some("Repository read failed"));
                cx.notify();
            });
            visual.update(|window, cx| {
                window.draw(cx).clear(cx);
                assert!(
                    snapshots(window)
                        .iter()
                        .any(|node| node.role() == Some(gpui::Role::Alert)
                            && node.label() == Some("Repository read failed"))
                );
            });
            assert!(visual.debug_bounds("settings-read-retry").is_some());
            assert!(visual.debug_bounds("settings-loading-identity-0").is_none());
            view.update(visual, |view, cx| {
                view.resources.invalidate(key);
                settled(&mut view.resources, key, None);
                view.has_form = true;
                cx.notify();
            });
            visual.update(|window, cx| window.draw(cx).clear(cx));
            assert!(visual.debug_bounds("settings-read-retry").is_none());
            view.update(visual, |view, cx| {
                view.resources.begin(key);
                assert_eq!(view.draft.read(cx).value().as_ref(), "Unsaved author");
                cx.notify();
            });
            visual.update(|window, cx| window.draw(cx).clear(cx));
            assert!(
                visual.debug_bounds("settings-loading-identity-0").is_none(),
                "refresh must not add a skeleton beside the existing form"
            );
        }
    }
    #[test]
    fn settings_dependency_errors_include_listing_validation_without_duplicates() {
        let mut resources = Resources::default();
        for key in READ_KEYS {
            settled(&mut resources, key, None);
        }
        assert!(!read_feedback(&resources, None).pending);
        assert_eq!(
            read_feedback(&resources, Some("Invalid remote list")).errors,
            ["Invalid remote list"]
        );
        resources.invalidate("git-list");
        settled(&mut resources, "git-list", Some("Invalid remote list"));
        assert_eq!(
            read_feedback(&resources, Some("Invalid remote list")).errors,
            ["Invalid remote list"]
        );
    }
}
