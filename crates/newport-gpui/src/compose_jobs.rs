//! Compose controls and review drafts. The shared client validates and owns
//! commands; this module only presents choices and retains unsent form state.
use super::*;
use gpui_kit::component::input::InputEvent;
use newport_domain::compose::{ComposeProject, service_replicas, service_rows};

#[cfg(test)]
#[path = "compose_controls_tests.rs"]
mod tests;

#[derive(Clone, PartialEq)]
pub(super) enum ComposeReview {
    Down {
        project: String,
        volumes: bool,
        orphans: bool,
    },
    Scale {
        project: String,
        service: String,
        current: usize,
    },
}
impl ComposeReview {
    fn project(&self) -> &str {
        match self {
            Self::Down { project, .. } | Self::Scale { project, .. } => project,
        }
    }
}
pub(super) struct ComposeState {
    pub proposed: Option<ComposeReview>,
    pub show_activity_for: Option<String>,
    typed: Entity<InputState>,
    replicas: Entity<InputState>,
    confirm_focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}
impl ComposeState {
    pub(super) fn new(window: &mut Window, cx: &mut Context<ContainersPanel>) -> Self {
        let typed = cx.new(|cx| InputState::new(window, cx).placeholder("Project name"));
        let replicas = cx.new(|cx| {
            InputState::new(window, cx)
                .min(0.)
                .max(20.)
                .step(1.)
                .default_value("1")
        });
        let subscriptions = [&typed, &replicas]
            .into_iter()
            .map(|field| {
                cx.subscribe(field, |_, _, event, cx| {
                    if matches!(event, InputEvent::Change) {
                        cx.notify();
                    }
                })
            })
            .collect();
        Self {
            proposed: None,
            show_activity_for: None,
            typed,
            replicas,
            confirm_focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }
}
impl ContainersPanel {
    pub(crate) fn compose_project_selected(&self, project: &str) -> bool {
        !self.retired
            && self.client.selected_container().is_none()
            && self.active_selection.as_deref() == Some(domain::project_key(project).as_str())
            && self.client.selected_project() == Some(project)
    }
    /// Ordinary Compose commands start directly.
    /// Destructive options and scale have a separate explicit review below.
    pub(crate) fn propose_compose(&mut self, action: Value) {
        if self.has_review_dialog() {
            return;
        }
        let Some(project) = self.client.selected_project().map(str::to_owned) else {
            return;
        };
        if self.compose_project_selected(&project)
            && self.client.compose_run(project.clone(), action)
        {
            self.review = None;
            self.compose_state.show_activity_for = Some(project);
            self.compose_state.proposed = None;
        }
    }
    fn propose_down(&mut self, project: String, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx)
            || !self.compose_project_selected(&project)
            || !self.client.can_compose_run(&project)
        {
            return;
        }
        self.review = None;
        self.compose_state
            .typed
            .update(cx, |field, cx| field.set_value("", window, cx));
        self.compose_state.proposed = Some(ComposeReview::Down {
            project,
            volumes: false,
            orphans: false,
        });
        self.open_compose_review(window, cx);
        cx.notify();
    }
    pub(crate) fn propose_scale(
        &mut self,
        project: String,
        service: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx)
            || !self.compose_project_selected(&project)
            || !self.client.can_compose_run(&project)
        {
            return;
        }
        let Some(snapshot) = self.client.snapshot() else {
            return;
        };
        let groups = domain::group_containers(&snapshot.containers);
        let Some(group) = groups.projects.iter().find(|p| p.name == project) else {
            return;
        };
        let detail: Option<ComposeProject> = serde_json::from_value(self.compose_value()).ok();
        if !service_rows(group, detail.as_ref())
            .iter()
            .any(|row| row.service == service)
        {
            return;
        }
        let current = service_replicas(group, &service);
        self.review = None;
        self.compose_state.replicas.update(cx, |field, cx| {
            field.set_value(current.max(1).to_string(), window, cx)
        });
        self.compose_state.proposed = Some(ComposeReview::Scale {
            project,
            service,
            current,
        });
        self.open_compose_review(window, cx);
        cx.notify();
    }
    fn compose_review_action(&self, cx: &App) -> Option<Value> {
        let review = self.compose_state.proposed.as_ref()?;
        if !self.compose_project_selected(review.project())
            || !self.client.can_compose_run(review.project())
        {
            return None;
        }
        match review {
            ComposeReview::Down {
                project,
                volumes,
                orphans,
            } => {
                if *volumes && self.compose_state.typed.read(cx).value().as_ref() != project {
                    return None;
                }
                Some(json!({"kind":"down","volumes":volumes,"removeOrphans":orphans}))
            }
            ComposeReview::Scale { service, .. } => {
                let replicas = self
                    .compose_state
                    .replicas
                    .read(cx)
                    .value()
                    .parse::<u32>()
                    .ok()
                    .filter(|n| *n <= 20)?;
                Some(json!({"kind":"scale","service":service,"replicas":replicas}))
            }
        }
    }

    fn open_compose_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(review) = self.compose_state.proposed.as_ref() else {
            return;
        };
        let title = match review {
            ComposeReview::Down { project, .. } => format!("Take {project} down?"),
            ComposeReview::Scale { service, .. } => format!("Scale {service}"),
        };
        let focus = match review {
            ComposeReview::Scale { .. } => self.compose_state.replicas.read(cx).focus_handle(cx),
            ComposeReview::Down { .. } => self.review_focus.clone(),
        };
        let token = self.next_review_dialog_token();
        let owner = cx.weak_entity();
        // Popup menu focus disappears as it closes. Keep a stable return target.
        self.detail_focus.focus(window, cx);
        window.open_dialog(cx, move |dialog, window, cx| {
            let cancel = owner.clone();
            let close = owner.clone();
            let submit = owner.clone();
            let (body, footer) = owner
                .update(cx, |this, cx| this.compose_review_parts(token, window, cx))
                .unwrap_or_else(|_| (div().into_any_element(), div().into_any_element()));
            let compact =
                window.viewport_size().width < px(540.) || window.viewport_size().height < px(560.);
            dialog
                .title(
                    div()
                        .min_w_0()
                        .pr(px(24.))
                        .whitespace_normal()
                        .text_size(px(17.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title.clone()),
                )
                .w(px(480.))
                .max_w((window.viewport_size().width - px(32.)).max(px(240.)))
                .p(px(if compact { 18. } else { 24. }))
                .rounded(px(crate::ui::RADIUS))
                .border_0()
                .centered(true)
                .overlay_closable(false)
                .on_cancel(move |_, _, cx| {
                    cancel
                        .upgrade()
                        .is_some_and(|panel| panel.read(cx).review_dialog_current(token))
                })
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |this, cx| {
                        if this.review_dialog_matches(token) {
                            this.compose_state.proposed = None;
                            this.forget_review_dialog(token);
                            cx.notify();
                        }
                    });
                })
                // Only the explicit focused confirmation button submits. Bare
                // Enter in a field must not approve volume deletion or scale.
                .on_ok(move |_, window, cx| {
                    let _ = submit.update(cx, |this, cx| {
                        if !this.review_dialog_current(token) {
                            return;
                        }
                        if this.review_focus.is_focused(window) {
                            this.dismiss_compose_review(token, window, cx);
                        } else if this.compose_state.confirm_focus.is_focused(window)
                            && let Some(review) = this.compose_state.proposed.clone()
                        {
                            this.confirm_compose_review(token, &review, window, cx);
                        }
                    });
                    false
                })
                .child(
                    div()
                        .id("compose-dialog-body")
                        .max_h((window.viewport_size().height - px(190.)).max(px(80.)))
                        .overflow_y_scroll()
                        .child(body),
                )
                .footer(footer)
        });
        self.own_review_dialog(token, true, window, cx);
        let owner = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            if owner
                .upgrade()
                .is_some_and(|panel| panel.read(cx).review_dialog_current(token))
            {
                focus.focus(window, cx);
            }
        });
    }

    fn dismiss_compose_review(&mut self, token: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.review_dialog_matches(token) {
            self.compose_state.proposed = None;
            self.close_review_dialog(token, window, cx);
            cx.notify();
        }
    }

    fn confirm_compose_review(
        &mut self,
        token: u64,
        review: &ComposeReview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.review_dialog_current(token)
            && self.compose_state.proposed.as_ref() == Some(review)
            && let Some(action) = self.compose_review_action(cx)
        {
            let project = review.project().to_owned();
            if self.client.compose_run(project.clone(), action) {
                self.compose_state.show_activity_for = Some(project);
                self.dismiss_compose_review(token, window, cx);
            }
            cx.notify();
        }
    }
    pub(super) fn compose_job_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(project) = self.client.selected_project().map(str::to_owned) else {
            return div().into_any_element();
        };
        let Some(snapshot) = self.client.snapshot() else {
            return div().into_any_element();
        };
        let groups = domain::group_containers(&snapshot.containers);
        let Some(group) = groups.projects.iter().find(|p| p.name == project) else {
            return div().into_any_element();
        };
        let detail: Option<ComposeProject> = serde_json::from_value(self.compose_value()).ok();
        let cli = detail.as_ref().and_then(|p| p.cli.as_ref());
        let blocked = !self.client.can_compose_run(&project);
        let missing = compose_services::missing_count(group, detail.as_ref()) > 0;
        let builds = detail
            .as_ref()
            .and_then(|p| p.services.as_ref())
            .is_some_and(|s| s.iter().any(|s| s.build));
        let ids: Vec<_> = group.services.iter().map(|c| c.id.clone()).collect();
        let action = |verb| Action::Project {
            project: project.clone(),
            verb,
            expected_ids: ids.clone(),
        };
        let mut controls = div().flex().flex_wrap().items_center().gap(px(6.));
        if cli.is_some() {
            let project = project.clone();
            controls = controls.child(crate::controls::button("compose-up").label("Up").icon(IconName::ArrowUpFromLine)
                .when(group.running < group.services.len() || missing, |b| b.primary())
                .disabled(blocked).hover_hint(format!("Create missing containers and recreate what changed from the Compose files. Restart and Stop act on the existing containers. Compose {}.", cli.map(|cli| cli.version.as_str()).unwrap_or_default()))
                .on_click(cx.listener(move |this, _, _, cx| { if this.compose_project_selected(&project) {this.propose_compose(up(false,false,false)); cx.notify();} })));
        } else if group.running < group.services.len() {
            let action = action(ProjectVerb::Start);
            controls = controls.child(
                crate::controls::button("project-start")
                    .primary()
                    .icon(IconName::Play)
                    .label(if group.running == 0 {
                        "Start all"
                    } else {
                        "Start the rest"
                    })
                    .disabled(!self.client.can_action(&action))
                    .loading(self.action_running(&action))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.run_container_action(action.clone(), cx)
                    })),
            );
        }
        if group.running > 0 {
            for (verb, label, icon) in [
                (ProjectVerb::Restart, "Restart all", IconName::RotateCw),
                (ProjectVerb::Stop, "Stop all", IconName::Square),
            ] {
                let action = action(verb);
                controls = controls.child(
                    crate::controls::button(SharedString::from(format!(
                        "project-{}",
                        verb.as_str()
                    )))
                    .label(label)
                    .icon(icon)
                    .disabled(!self.client.can_action(&action))
                    .loading(self.action_running(&action))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.compose_state.proposed = None;
                        this.review_action(action.clone(), window, cx);
                    })),
                );
            }
        }
        if cli.is_some() {
            let weak = cx.weak_entity();
            let project = project.clone();
            let start = (group.running < group.services.len()).then(|| action(ProjectVerb::Start));
            controls = controls.child(
                crate::controls::icon_button("compose-more")
                    .ghost()
                    .icon(IconName::Ellipsis)
                    .accessibility_label(format!("More actions for {project}"))
                    .disabled(blocked)
                    .dropdown_menu(move |mut menu, _, cx| {
                        let enabled = weak.upgrade().is_some_and(|p| {
                            let p = p.read(cx);
                            p.compose_project_selected(&project)
                                && p.client.can_compose_run(&project)
                        });
                        if let Some(action) = start.clone() {
                            let weak = weak.clone();
                            menu = menu
                                .item(
                                    PopupMenuItem::new("Start existing containers")
                                        .disabled(!enabled)
                                        .on_click(move |_, _, cx| {
                                            let _ = weak.update(cx, |this, cx| {
                                                this.run_container_action(action.clone(), cx)
                                            });
                                        }),
                                )
                                .separator();
                        }
                        for (label, action) in [
                            ("Pull images & up", Some(up(false, true, false))),
                            ("Rebuild & up", builds.then(|| up(true, false, false))),
                            (
                                "Pull images only",
                                Some(json!({"kind":"pull","services":[]})),
                            ),
                            (
                                "Build images only",
                                builds
                                    .then(|| json!({"kind":"build","services":[],"noCache":false})),
                            ),
                        ] {
                            if let Some(action) = action {
                                let weak = weak.clone();
                                let project = project.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(label).disabled(!enabled).on_click(
                                        move |_, _, cx| {
                                            let _ = weak.update(cx, |this, cx| {
                                                if this.compose_project_selected(&project) {
                                                    this.propose_compose(action.clone());
                                                    cx.notify();
                                                }
                                            });
                                        },
                                    ),
                                );
                            }
                        }
                        let weak = weak.clone();
                        let project = project.clone();
                        menu.separator().item(
                            PopupMenuItem::element(move |_, cx| {
                                div()
                                    .id("compose-down-menu")
                                    .role(accesskit::Role::MenuItem)
                                    .aria_label("Down…")
                                    .text_color(if enabled {
                                        cx.theme().danger
                                    } else {
                                        cx.theme().muted_foreground
                                    })
                                    .child("Down…")
                                    .test_support()
                            })
                            .icon(Icon::new(IconName::Trash).text_color(if enabled {
                                cx.theme().danger
                            } else {
                                cx.theme().muted_foreground
                            }))
                            .disabled(!enabled)
                            .on_click(move |_, window, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    this.propose_down(project.clone(), window, cx)
                                });
                            }),
                        )
                    }),
            );
        }
        controls = controls.child(self.project_finished_jobs_button(group, cx));
        // Up explains itself in its tooltip; only a missing CLI needs words.
        let helper = (cli.is_none() && !self.pending("compose-detail")).then(|| {
            format!(
                "These act on the project’s existing containers. Up, Pull, Rebuild and Down need the Compose CLI on the server{}",
                detail
                    .as_ref()
                    .and_then(|p| p.cli_error.as_deref())
                    .or_else(|| self
                        .client
                        .resource("compose-detail")
                        .and_then(|r| r.error.as_deref()))
                    .map(|e| format!(": {e}"))
                    .unwrap_or_else(|| ".".into())
            )
        });
        let mut view = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(controls)
            .when_some(helper, |view, helper| {
                view.child(quiet(helper, cx).text_size(px(crate::ui::FONT_SIZE)))
            });
        if self.running_compose_job().is_none()
            && cli.is_some()
            && let Some(reason) = self.client.compose_run_blocked_reason(&project)
        {
            view = view.child(
                quiet(reason, cx)
                    .id("compose-blocked-reason")
                    .role(accesskit::Role::Status)
                    .aria_label(reason),
            );
        }
        view.when(
            self.client
                .resource("compose-start")
                .is_some_and(|r| r.error.is_some()),
            |v| v.child(self.status("compose-start", cx)),
        )
        .into_any_element()
    }
    fn compose_review_parts(
        &self,
        token: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (AnyElement, AnyElement) {
        let Some(review) = self.compose_state.proposed.clone() else {
            return (div().into_any_element(), div().into_any_element());
        };
        let blocked = !self.review_dialog_current(token)
            || !self.compose_project_selected(review.project())
            || !self.client.can_compose_run(review.project());
        let mut body = div()
            .id("compose-review")
            .debug_selector(|| "compose-review".into())
            .role(accesskit::Role::Group)
            .flex()
            .flex_col()
            .pt(px(8.))
            .gap(px(10.));
        let label;
        match &review {
            ComposeReview::Down {
                project,
                volumes,
                orphans,
            } => {
                label = if *volumes {
                    "Down & delete volumes".to_owned()
                } else {
                    "Down".into()
                };
                body = body
                    .aria_label(format!("Take {project} down?"))
                    .child(action_description(
                        "The project’s containers and networks are removed. Images stay, and Up creates everything again from the compose files.",
                        cx,
                    ))
                    .child(
                        crate::controls::switch("compose-down-orphans")
                            .checked(*orphans)
                            .disabled(blocked)
                            .label("Also remove containers for services no longer in the files")
                            .on_click(cx.listener(move |this, value, _, cx| {
                                if !this.review_dialog_current(token) { return; }
                                if let Some(ComposeReview::Down { orphans, .. }) = &mut this.compose_state.proposed {
                                    *orphans = *value;
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        crate::controls::switch("compose-down-volumes")
                            .checked(*volumes)
                            .disabled(blocked)
                            .label("Delete the project’s volumes")
                            .on_click(cx.listener(move |this, value, _, cx| {
                                if !this.review_dialog_current(token) { return; }
                                if let Some(ComposeReview::Down { volumes, .. }) = &mut this.compose_state.proposed {
                                    *volumes = *value;
                                    cx.notify();
                                }
                            })),
                    );
                if *volumes {
                    body=body.child(alert("compose-volume-warning",format!("Databases and other data in them are deleted for good. Type {project} to confirm."),cx))
                        .child(crate::controls::form_input(&self.compose_state.typed).aria_label("Project name").disabled(blocked));
                }
            }
            ComposeReview::Scale {
                service, current, ..
            } => {
                let valid = self
                    .compose_state
                    .replicas
                    .read(cx)
                    .value()
                    .parse::<u32>()
                    .ok()
                    .filter(|n| *n <= 20);
                label = valid
                    .map(|n| format!("Run {n}"))
                    .unwrap_or_else(|| "Scale".into());
                body=body.aria_label(format!("Scale {service}"))
                    .child(action_description(format!("How many {service} containers should run? It runs {current} now. Services that publish a fixed host port can only run one."),cx))
                    .child(self.compose_scale_input(blocked, window, cx))
                    .when(valid.is_none(), |v|v.child(alert("compose-scale-invalid","Choose 0 to 20 containers.",cx)));
            }
        }
        let enabled = !blocked && self.compose_review_action(cx).is_some();
        let footer = div()
            .flex()
            .flex_wrap()
            .justify_end()
            .gap(px(8.))
            .child(
                crate::controls::form_button("dismiss-compose")
                    .debug_selector(|| "dismiss-compose".into())
                    .label("Cancel")
                    .track_focus(&self.review_focus)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.dismiss_compose_review(token, window, cx);
                    })),
            )
            .child(
                crate::controls::form_button("confirm-compose")
                    .debug_selector(|| "confirm-compose".into())
                    .track_focus(&self.compose_state.confirm_focus)
                    .label(label)
                    .when(matches!(review, ComposeReview::Down { .. }), |b| b.danger())
                    .when(matches!(review, ComposeReview::Scale { .. }), |b| {
                        b.primary()
                    })
                    .disabled(!enabled)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.confirm_compose_review(token, &review, window, cx);
                    })),
            );
        (body.into_any_element(), footer.into_any_element())
    }

    fn compose_scale_input(&self, blocked: bool, window: &Window, cx: &App) -> AnyElement {
        use gpui_kit::component::ThemeStyled;

        let input_height = crate::controls::FORM_HEIGHT;
        let foreground = cx.theme().muted_foreground;
        let hover = cx.theme().muted;
        let focused = !blocked
            && self
                .compose_state
                .replicas
                .read(cx)
                .focus_handle(cx)
                .is_focused(window);
        let stepper = gpui_kit::base::NumberInput::new(&self.compose_state.replicas)
            .controls_right()
            .disabled(blocked)
            .size_full()
            .input(
                Styled::h(
                    crate::controls::form_input(&self.compose_state.replicas),
                    px(input_height - 2.),
                )
                .min_h(px(input_height - 2.))
                .max_h(px(input_height - 2.))
                .appearance(false)
                .aria_label("Containers")
                .disabled(blocked),
            )
            .increment_button(move |button| {
                button
                    .accessibility_label("Increase containers")
                    .w(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(foreground)
                    .hover(move |style| style.bg(hover))
                    .child(Icon::new(IconName::ChevronUp).size(px(12.)))
            })
            .decrement_button(move |button| {
                button
                    .accessibility_label("Decrease containers")
                    .w(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(foreground)
                    .hover(move |style| style.bg(hover))
                    .child(Icon::new(IconName::ChevronDown).size(px(12.)))
            });
        div()
            .id("compose-scale-input")
            .debug_selector(|| "compose-scale-input".into())
            .w(px(96.))
            .h(px(input_height))
            .min_h(px(input_height))
            .max_h(px(input_height))
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(if focused {
                cx.theme().ring
            } else {
                cx.theme().input
            })
            .bg(cx.theme().input_background())
            .when(blocked, |view| view.opacity(0.5))
            .when(focused, |view| view.focus_ring_style(window, cx))
            .child(stepper)
            .into_any_element()
    }
}
fn up(build: bool, pull: bool, force_recreate: bool) -> Value {
    json!({"kind":"up","services":[],"build":build,"pull":pull,"forceRecreate":force_recreate})
}
