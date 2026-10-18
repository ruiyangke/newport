//! Container commands and modal review. The shared client validates observed
//! identities and owns operations; this module owns controls and focus only.
use super::*;
use newport_domain::compose::{ComposeCli, ComposeService};
use serde::Deserialize;

#[cfg(test)]
#[path = "container_compose_actions_tests.rs"]
mod tests;

#[derive(Clone)]
struct ContainerComposeTarget {
    id: String,
    name: String,
    project: String,
    service: String,
}
impl ContainerComposeTarget {
    fn from_container(container: &Container) -> Option<Self> {
        let labels = container.compose.as_ref().filter(|labels| !labels.oneoff)?;
        Some(Self {
            id: container.id.clone(),
            name: container.name.clone(),
            project: labels.project.clone(),
            service: labels.service.clone(),
        })
    }
}

#[derive(Clone, Copy)]
enum ContainerComposeAction {
    Recreate,
    PullRecreate,
    Rebuild,
}
impl ContainerComposeAction {
    fn label(self) -> &'static str {
        match self {
            Self::Recreate => "Recreate",
            Self::PullRecreate => "Pull & recreate",
            Self::Rebuild => "Rebuild",
        }
    }
    fn action(self, service: &str) -> Value {
        json!({"kind":"up", "services":[service], "build":matches!(self, Self::Rebuild),
            "pull":matches!(self, Self::PullRecreate), "forceRecreate":true})
    }
}

impl ContainersPanel {
    pub(super) fn container_actions(&self, c: &Container, cx: &mut Context<Self>) -> AnyElement {
        let mut commands = div().flex().flex_wrap().items_center().gap(px(6.));
        for verb in domain::available_container_verbs(c)
            .iter()
            .copied()
            .filter(|verb| *verb != ContainerVerb::Pause)
        {
            let action = Action::Container {
                container: c.id.clone(),
                name: c.name.clone(),
                verb,
            };
            commands = commands.child(
                crate::controls::button(SharedString::from(format!("container-{}", verb.as_str())))
                    .label(verb_label(verb))
                    .icon(match verb {
                        ContainerVerb::Start | ContainerVerb::Unpause => IconName::Play,
                        ContainerVerb::Restart => IconName::RefreshCw,
                        ContainerVerb::Stop => IconName::Square,
                        ContainerVerb::Pause => IconName::Pause,
                    })
                    .when(
                        matches!(verb, ContainerVerb::Start | ContainerVerb::Restart),
                        |b| b.primary(),
                    )
                    .loading(self.action_running(&action))
                    .disabled(!self.client.can_action(&action))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if verb == ContainerVerb::Stop {
                            this.review_action(action.clone(), window, cx);
                        } else {
                            this.run_container_action(action.clone(), cx);
                        }
                    })),
            );
        }
        if c.state != domain::ContainerState::Paused {
            let (id, name, title) = (
                c.id.clone(),
                c.name.clone(),
                domain::display_name(c).to_owned(),
            );
            commands = commands.child(
                crate::controls::button("container-shell")
                    .icon(IconName::Terminal)
                    .label("Shell")
                    .when(c.state != domain::ContainerState::Running, |button| {
                        button.hover_hint("Start the container to open a shell in it")
                    })
                    .disabled(self.client.shell_command(&id, &name).is_err())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_container_shell(&id, &name, &title, cx);
                    })),
            );
        }
        if domain::can_remove_container(c) {
            let (id, name) = (c.id.clone(), c.name.clone());
            let action = Action::Remove {
                containers: vec![id.clone()],
                names: vec![name.clone()],
            };
            commands = commands.child(
                crate::controls::button("container-remove")
                    .ghost()
                    .icon(IconName::Trash)
                    .label("Remove")
                    .loading(self.action_running(&action))
                    .disabled(!self.client.can_action(&action))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.selected_container_is(&id, &name) {
                            this.review_action(action.clone(), window, cx);
                        }
                    })),
            );
        }
        let compose_target = ContainerComposeTarget::from_container(c);
        let compose_builds = compose_target
            .as_ref()
            .and_then(|target| self.container_compose_builds(target));
        if c.state == domain::ContainerState::Running || compose_builds.is_some() {
            let pause = c.state == domain::ContainerState::Running;
            let action = Action::Container {
                container: c.id.clone(),
                name: c.name.clone(),
                verb: ContainerVerb::Pause,
            };
            let weak = cx.weak_entity();
            commands = commands.child(
                crate::controls::icon_button("container-more-actions")
                    .ghost()
                    .icon(IconName::Ellipsis)
                    .accessibility_label(format!("More actions for {}", c.name))
                    .dropdown_menu(move |menu, _, cx| {
                        let mut menu = menu;
                        if pause {
                            let enabled = weak.upgrade().is_some_and(|panel| {
                                let panel = panel.read(cx);
                                panel.action_matches_selection(&action)
                                    && panel.client.can_action(&action)
                            });
                            let (weak, action) = (weak.clone(), action.clone());
                            menu = menu.item(
                                PopupMenuItem::new("Pause")
                                    .icon(IconName::Pause)
                                    .disabled(!enabled)
                                    .on_click(move |_, _, cx| {
                                        let _ = weak.update(cx, |this, cx| {
                                            this.run_container_action(action.clone(), cx)
                                        });
                                    }),
                            );
                        }
                        if let (Some(target), Some(builds)) = (&compose_target, compose_builds) {
                            if pause {
                                menu = menu.separator();
                            }
                            let mut actions = vec![
                                ContainerComposeAction::Recreate,
                                ContainerComposeAction::PullRecreate,
                            ];
                            if builds {
                                actions.push(ContainerComposeAction::Rebuild);
                            }
                            for action in actions {
                                let enabled = weak.upgrade().is_some_and(|panel| {
                                    panel.read(cx).can_container_compose(target, action)
                                });
                                let (weak, target) = (weak.clone(), target.clone());
                                menu = menu.item(
                                    PopupMenuItem::new(action.label())
                                        .disabled(!enabled)
                                        .on_click(move |_, _, cx| {
                                            let _ = weak.update(cx, |this, cx| {
                                                this.run_container_compose(&target, action, cx);
                                            });
                                        }),
                                );
                            }
                        }
                        menu
                    }),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(commands)
            .when(
                c.compose.as_ref().is_some_and(|labels| !labels.oneoff)
                    && self.error("compose-start").is_some(),
                |view| view.child(self.status("compose-start", cx)),
            )
            .into_any_element()
    }

    /// Capability belongs to the selected project's retained read; identity
    /// remains the exact container whose menu was opened, including its service.
    fn container_compose_builds(&self, target: &ContainerComposeTarget) -> Option<bool> {
        if self.retired
            || !self.client.selected_container().is_some_and(|container| {
                container.id == target.id
                    && container.name == target.name
                    && container.compose.as_ref().is_some_and(|labels| {
                        !labels.oneoff
                            && labels.project == target.project
                            && labels.service == target.service
                    })
            })
            || self.client.selected_project() != Some(target.project.as_str())
        {
            return None;
        }
        // Read only capability fields: Compose file contents can be several
        // megabytes and are unrelated to this menu's presentation.
        let detail = self.client.compose_project()?;
        ComposeCli::deserialize(detail.get("cli")?).ok()?;
        Some(
            detail
                .get("services")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|service| ComposeService::deserialize(service).ok())
                .any(|service| service.name == target.service && service.build),
        )
    }

    fn can_container_compose(
        &self,
        target: &ContainerComposeTarget,
        action: ContainerComposeAction,
    ) -> bool {
        self.container_compose_builds(target)
            .is_some_and(|builds| !matches!(action, ContainerComposeAction::Rebuild) || builds)
            && self.client.can_compose_run(&target.project)
    }

    fn run_container_compose(
        &mut self,
        target: &ContainerComposeTarget,
        action: ContainerComposeAction,
        cx: &mut Context<Self>,
    ) {
        if self.can_container_compose(target, action)
            && self
                .client
                .compose_run(target.project.clone(), action.action(&target.service))
        {
            self.review = None;
            self.compose_state.proposed = None;
            // Single-container actions keep the current detail/tab, matching
            // Compose start(action, false) in the reference interface.
            self.compose_state.show_activity_for = None;
            cx.notify();
        }
    }

    pub(super) fn project_finished_jobs_button(
        &self,
        group: &domain::ProjectGroup<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let jobs: Vec<_> = group
            .jobs
            .iter()
            .filter(|container| domain::can_remove_container(container))
            .collect();
        if jobs.is_empty() {
            return div().into_any_element();
        }
        let project = group.name.to_owned();
        let action = Action::Remove {
            containers: jobs.iter().map(|container| container.id.clone()).collect(),
            names: jobs
                .iter()
                .map(|container| container.name.clone())
                .collect(),
        };
        crate::controls::button("project-remove-finished-jobs")
            .debug_selector(|| "project-remove-finished-jobs".into())
            .ghost()
            .icon(IconName::Trash)
            .label(format!(
                "Remove {} finished {}",
                jobs.len(),
                if jobs.len() == 1 { "job" } else { "jobs" }
            ))
            .loading(self.action_running(&action))
            .disabled(!self.action_matches_selection(&action) || !self.client.can_action(&action))
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.active_selection.as_deref() == Some(domain::project_key(&project).as_str())
                {
                    this.review_action(action.clone(), window, cx);
                }
            }))
            .into_any_element()
    }

    pub(super) fn action_running(&self, action: &Action) -> bool {
        self.client
            .action_status(&action.target())
            .is_some_and(|status| status.pending && status.action == *action)
    }

    pub(super) fn selected_container_is(&self, id: &str, name: &str) -> bool {
        self.client
            .selected_container()
            .is_some_and(|c| c.id == id && c.name == name)
    }

    fn action_matches_selection(&self, action: &Action) -> bool {
        match action {
            Action::Container {
                container, name, ..
            } => self.selected_container_is(container, name),
            Action::Remove { containers, names } => {
                if containers.len() == 1
                    && names.len() == 1
                    && self.selected_container_is(&containers[0], &names[0])
                {
                    return true;
                }
                let Some(project) = self.client.selected_project() else {
                    return false;
                };
                if self.active_selection.as_deref() != Some(domain::project_key(project).as_str())
                    || containers.is_empty()
                    || containers.len() != names.len()
                {
                    return false;
                }
                let Some(snapshot) = self.client.snapshot() else {
                    return false;
                };
                let mut current: Vec<_> = snapshot
                    .containers
                    .iter()
                    .filter(|container| {
                        container
                            .compose
                            .as_ref()
                            .is_some_and(|labels| labels.project == project && labels.oneoff)
                            && domain::can_remove_container(container)
                    })
                    .map(|container| (&container.id, &container.name))
                    .collect();
                let mut captured: Vec<_> = containers.iter().zip(names).collect();
                current.sort_unstable();
                captured.sort_unstable();
                current == captured
            }
            Action::Project { project, .. } => {
                self.client.selected_container().is_none()
                    && self.client.selected_project() == Some(project.as_str())
            }
            Action::Prune { .. } => {
                self.active_selection.as_deref() == Some(domain::DISK)
                    && self.client.disk().is_some()
            }
        }
    }

    fn open_container_shell(&mut self, id: &str, name: &str, title: &str, cx: &mut Context<Self>) {
        if !self.selected_container_is(id, name) {
            return;
        }
        match self.client.shell_command(id, name) {
            Ok(command) => cx.emit(ContainersEvent::OpenShell {
                command,
                title: title.to_owned(),
            }),
            Err(message) => cx.emit(ContainersEvent::Notification {
                message: message.to_owned(),
            }),
        }
    }

    pub(super) fn run_container_action(&mut self, action: Action, cx: &mut Context<Self>) {
        if self.action_matches_selection(&action) && self.client.action(action) {
            self.compose_state.proposed = None;
            cx.notify();
        }
    }

    pub(super) fn review_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx)
            || self.retired
            || !self.action_matches_selection(&action)
            || !self.client.can_action(&action)
        {
            return;
        }
        self.compose_state.proposed = None;
        self.review = Some(action);
        let token = self.next_review_dialog_token();
        let owner = cx.weak_entity();
        // Kit restores the focus captured when opening, including after Escape.
        self.detail_focus.focus(window, cx);
        window.open_dialog(cx, move |dialog, window, cx| {
            let Some(panel) = owner.upgrade() else {
                return dialog;
            };
            let (title, body, footer, pending) = panel.update(cx, |panel, cx| {
                let pending = panel
                    .review
                    .as_ref()
                    .is_some_and(|action| panel.action_running(action));
                let (title, body, footer) = panel.action_review_parts(token, cx);
                (title, body, footer, pending)
            });
            let cancel = owner.clone();
            let close = owner.clone();
            let confirm = owner.clone();
            dialog
                .aria_label(title.clone())
                .title(
                    div()
                        .text_size(px(17.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .w(px(480.))
                .max_w((window.viewport_size().width - px(32.)).max(px(240.)))
                .p(px(24.))
                .rounded(px(14.))
                .border_0()
                .centered(true)
                .overlay_closable(false)
                .keyboard(!pending)
                .close_button(!pending)
                .on_ok(move |_, window, cx| {
                    confirm
                        .update(cx, |panel, cx| {
                            if !panel.review_dialog_current(token) || panel.retired {
                                return false;
                            }
                            let Some(action) = panel.review.clone() else {
                                return false;
                            };
                            if panel.action_running(&action) {
                                return false;
                            }
                            if panel.review_focus.is_focused(window) {
                                return true;
                            }
                            if panel.review_confirm_focus.is_focused(window) {
                                panel.confirm_action_review(token, &action, cx);
                            }
                            false
                        })
                        .unwrap_or(false)
                })
                .on_cancel(move |_, _, cx| {
                    cancel.upgrade().is_some_and(|panel| {
                        let panel = panel.read(cx);
                        panel.review_dialog_current(token)
                            && !panel
                                .review
                                .as_ref()
                                .is_some_and(|action| panel.action_running(action))
                    })
                })
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |panel, cx| {
                        if panel.review_dialog_matches(token) {
                            panel.review = None;
                            panel.forget_review_dialog(token);
                            cx.notify();
                        }
                    });
                })
                .child(body)
                .footer(footer)
        });
        self.own_review_dialog(token, false, window, cx);
        let owner = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            if let Some(panel) = owner.upgrade() {
                let panel = panel.read(cx);
                let focus = (panel.review_dialog_matches(token) && panel.has_review_dialog())
                    .then(|| panel.review_focus.clone());
                if let Some(focus) = focus {
                    focus.focus(window, cx);
                }
            }
        });
        cx.notify();
    }

    fn dismiss_action_review(&mut self, token: u64, window: &mut Window, cx: &mut Context<Self>) {
        if !self.review_dialog_current(token)
            || self
                .review
                .as_ref()
                .is_some_and(|action| self.action_running(action))
        {
            return;
        }
        self.review = None;
        self.close_review_dialog(token, window, cx);
        cx.notify();
    }

    pub(super) fn confirm_action_review(
        &mut self,
        token: u64,
        action: &Action,
        cx: &mut Context<Self>,
    ) {
        if self.review_dialog_current(token) && self.review.as_ref() == Some(action) {
            self.run_container_action(action.clone(), cx);
        }
    }

    fn action_review_parts(
        &self,
        token: u64,
        cx: &mut Context<Self>,
    ) -> (String, AnyElement, AnyElement) {
        let Some(action) = self.review.clone() else {
            return (
                String::new(),
                div().into_any_element(),
                div().into_any_element(),
            );
        };
        let (message, label, body) = match &action {
            Action::Container { name, verb, .. } => {
                let restart = self
                    .client
                    .selected_container()
                    .is_some_and(|c| c.restart_policy.as_deref() == Some("always"));
                (
                    format!("{} {name}?", verb_label(*verb)),
                    verb_label(*verb),
                    format!(
                        "Anything using it stops working until it is started again.{}",
                        if restart {
                            " Its restart policy is “always”, so Docker starts it again when the daemon restarts."
                        } else {
                            ""
                        }
                    ),
                )
            }
            Action::Remove { names, .. } if self.client.selected_container().is_none() => (
                format!(
                    "Remove {} finished {} from {}?",
                    names.len(),
                    if names.len() == 1 { "job" } else { "jobs" },
                    self.client.selected_project().unwrap_or("this project")
                ),
                "Remove",
                "The stopped one-off containers from docker compose run and their logs are deleted. Services, images and volumes stay.".into(),
            ),
            Action::Remove { names, .. } => {
                let compose = self
                    .client
                    .selected_container()
                    .and_then(|c| c.compose.as_ref())
                    .is_some_and(|c| !c.oneoff);
                (
                    format!("Remove {}?", names.join(", ")),
                    "Remove",
                    format!(
                        "The stopped container and its logs are deleted. Its image and volumes stay.{}",
                        if compose {
                            " Compose creates it again on the next up."
                        } else {
                            ""
                        }
                    ),
                )
            }
            Action::Project { project, verb, .. } => {
                let (label, body) = match verb {
                    ProjectVerb::Start => (
                        "Start all",
                        "These existing service containers are started. Compose files are not applied.",
                    ),
                    ProjectVerb::Stop => (
                        "Stop all",
                        "These service containers are stopped. Anything using them stays unavailable until they start again.",
                    ),
                    ProjectVerb::Restart => (
                        "Restart all",
                        "These service containers are restarted and their connections may drop. Stopped containers may start too.",
                    ),
                };
                (format!("{label} services in {project}?"), label, body.into())
            }
            Action::Prune { what } => {
                let (title, label, body, _) = super::disk::prune_copy(*what);
                (title.into(), label, body.into())
            },
        };
        let prune = matches!(action, Action::Prune { .. });
        let destructive = match &action {
            Action::Prune { what } => super::disk::prune_copy(*what).3,
            _ => true,
        };
        let review_id = if prune {
            "disk-review"
        } else {
            "container-action-review"
        };
        let confirm_id = if prune {
            "disk-confirm-prune"
        } else {
            "confirm-container-action"
        };
        let cancel_id = if prune {
            "disk-cancel-prune"
        } else {
            "cancel-container-review"
        };
        let error_id = if prune {
            "disk-prune-error"
        } else {
            "container-review-error"
        };
        let pending = self.action_running(&action);
        let allowed = self.review_dialog_current(token)
            && !self.retired
            && self.action_matches_selection(&action)
            && self.client.can_action(&action);
        let affected: Vec<String> = match &action {
            Action::Project { expected_ids, .. } => expected_ids
                .iter()
                .map(|id| {
                    self.client
                        .snapshot()
                        .and_then(|snapshot| {
                            snapshot
                                .containers
                                .iter()
                                .find(|container| &container.id == id)
                        })
                        .map(|container| {
                            format!("{} ({})", domain::display_name(container), container.name)
                        })
                        .unwrap_or_else(|| id.clone())
                })
                .collect(),
            Action::Remove { names, .. } if self.client.selected_container().is_none() => {
                names.clone()
            }
            _ => Vec::new(),
        };
        let error = self
            .client
            .action_status(&action.target())
            .and_then(|s| s.error.clone());
        let body = div().id(review_id)
            .debug_selector(move || review_id.into())
            .role(accesskit::Role::Region).aria_label(message.clone())
            .flex().flex_col().gap(px(10.))
            .child(action_description(body, cx))
            .when(!affected.is_empty(), |view| view.child(
                div().id("project-action-members").debug_selector(|| "project-action-members".into())
                    .role(accesskit::Role::List).aria_label("Affected containers")
                    .max_h(px(160.)).overflow_y_scroll().flex().flex_col().gap(px(4.))
                    .children(affected.into_iter().enumerate().map(|(index, name)| {
                        div().id(("project-action-member", index)).role(accesskit::Role::ListItem)
                            .aria_label(name.clone()).text_size(px(crate::controls::FONT_SIZE)).child(name)
                    }))
            ))
            .when_some(error, |view, error| view.child(alert(error_id, error, cx)
                .debug_selector(move || error_id.into())))
            .when(!pending && !allowed, |view| {
                let message = "Container state has changed. Cancel and review the current reading before continuing.";
                view.child(quiet(message, cx).id("container-review-stale")
                    .debug_selector(|| "container-review-stale".into())
                    .role(accesskit::Role::Status).aria_label(message))
            })
            .into_any_element();
        let cancel = crate::controls::form_button(cancel_id)
            .debug_selector(move || cancel_id.into())
            .track_focus(&self.review_focus)
            .label("Cancel")
            .disabled(pending)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.dismiss_action_review(token, window, cx);
            }));
        let confirm = crate::controls::form_button(confirm_id)
            .debug_selector(move || confirm_id.into())
            .track_focus(&self.review_confirm_focus)
            .label(label)
            .when(destructive, |b| b.danger())
            .when(!destructive, |b| b.primary())
            .disabled(!allowed)
            .loading(pending)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.confirm_action_review(token, &action, cx);
            }));
        let footer = div()
            .flex()
            .w_full()
            .justify_end()
            .gap(px(8.))
            .child(cancel)
            .child(confirm)
            .into_any_element();
        (message, body, footer)
    }
}

fn verb_label(verb: ContainerVerb) -> &'static str {
    match verb {
        ContainerVerb::Start => "Start",
        ContainerVerb::Stop => "Stop",
        ContainerVerb::Restart => "Restart",
        ContainerVerb::Pause => "Pause",
        ContainerVerb::Unpause => "Resume",
    }
}
