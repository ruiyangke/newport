use super::*;
use gpui_kit::component::tab::Tab;

#[cfg(test)]
#[path = "containers_detail_tests.rs"]
mod tests;

impl ContainersPanel {
    pub(super) fn detail(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.effective_selection(cx).as_deref() == Some(domain::DISK) {
            return self.disk_detail(cx);
        }
        let container = self.client.selected_container();
        let project = self.client.selected_project();
        div().id("containers-detail").debug_selector(||"containers-detail".into()).role(accesskit::Role::Region).aria_label("Container detail").track_focus(&self.detail_focus).tab_stop(false)
            .flex_1().min_w_0().min_h_0().h_full().overflow_y_scroll().track_scroll(&self.detail_scroll)
            .child(self.measure_detail(cx))
            .child(div().min_h_full().flex().flex_col().p(px(crate::controls::PAGE_PADDING)).gap(px(12.))
            .when(self.width<=600.,|v|v.child(div().flex().items_start().child(crate::controls::compact_button("all-containers").ghost().icon(IconName::ArrowLeft).label("All containers").on_click(cx.listener(|this,_,window,cx|{this.selected=None;this.sync_selection(cx);this.list_focus.focus(window,cx);cx.notify();})))))
            .when(self.client.snapshot().is_none(),|v|v.child(detail_skeleton()))
            .when_some(container,|v,c|v.child(self.container_header(c,cx)).child(self.container_actions(c,cx)).child(self.container_facts(c,cx)))
            .when(container.is_none(),|v|v.when_some(project,|v,name|v.child(self.project_header(name,cx))))
            .when(self.client.snapshot().is_some()&&container.is_none()&&project.is_none(),|v|v.child(crate::empty::pane(IconName::Boxes, if self.selected.is_some(){"Container unavailable"}else{"Choose a container"}, if self.selected.is_some(){"It may have been removed or recreated. Choose another container or refresh the list."}else{"Select a container to inspect its status, logs, and configuration."},cx)))
            .when(container.is_some()||project.is_some(),|v|v
                .when(self.review.is_none()&&self.error("action").is_some(),|v|v.child(self.status("action",cx)))
                .when(container.is_none(),|v|v.child(self.compose_job_controls(cx)))
                .child(div().flex().flex_col().flex_1().min_h(px(360.))
                    .child(self.detail_bar(container.is_some(),cx))
                    .child(if container.is_none()&&self.tab==PROJECT_SERVICES {self.project_services(window,cx)}else if self.tab==0 {self.logs_view(cx)}else if container.is_some(){self.inspect_view(cx)}else if self.tab==1 {self.compose_activity_view(cx)}else{self.files.view(cx)}))))
            .into_any_element()
    }
    fn container_header(&self, c: &Container, cx: &mut Context<Self>) -> AnyElement {
        use gpui_kit::component::spinner::Spinner;
        use newport_domain::container_status;
        let state = domain::state_of(c);
        let pending = self.client.pending_container_action(&c.id);
        let composing = self.running_compose_job();
        let progress = pending
            .map(|status| match &status.action {
                Action::Container { verb, .. } => {
                    container_status::action_progress(*verb).to_owned()
                }
                Action::Remove { .. } => "Removing…".to_owned(),
                _ => unreachable!("single-container progress excludes project and prune actions"),
            })
            .or_else(|| composing.map(|job| format!("Compose is working on {}…", job.project)));
        let lifecycle = if progress.is_none() {
            container_status::lifecycle_context(c, self.updated.unwrap_or_default())
        } else {
            None
        };
        let status_label = progress.as_deref().unwrap_or(state.label);
        let status_description = lifecycle.as_ref().map_or_else(
            || status_label.to_owned(),
            |context| format!("{status_label} {context}"),
        );
        let problem = container_status::problem_context(c, self.client.inspect());
        // Configuration that rarely changes rides along the status line.
        let meta = format!(
            "· created {} ago{}",
            newport_domain::services::duration(
                self.updated.unwrap_or(c.created).saturating_sub(c.created)
            ),
            c.restart_policy
                .as_deref()
                .filter(|policy| *policy != "no")
                .map(|policy| format!(" · restart {policy}"))
                .unwrap_or_default()
        );
        let (observed_id, observed_name) = (c.id.clone(), c.name.clone());
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child(heading(domain::display_name(c).to_owned(), 2))
                    .when_some(c.compose.as_ref(), |v, compose| {
                        let key = domain::project_key(&compose.project);
                        let (id, name) = (observed_id.clone(), observed_name.clone());
                        v.child(
                            crate::controls::compact_button("container-project-link")
                                .ghost()
                                .bg(cx.theme().muted)
                                .label(compose.project.clone())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if !this.retired && this.selected_container_is(&id, &name) {
                                        this.select(key.clone(), window, cx)
                                    }
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .mt(px(2.))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(4.))
                    .when(
                        c.compose.as_ref().is_some_and(|labels| !labels.oneoff),
                        |v| {
                            v.child(
                                quiet(format!("{} ·", c.name), cx)
                                    .min_w_0()
                                    .max_w_full()
                                    .whitespace_normal(),
                            )
                        },
                    )
                    .child(
                        quiet(c.image.clone(), cx)
                            .min_w_0()
                            .max_w_full()
                            .whitespace_normal()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(crate::ui::FONT_SIZE)),
                    ),
            )
            .child(
                div()
                    .id("container-state")
                    .debug_selector(|| "container-state".into())
                    .role(accesskit::Role::Status)
                    .aria_label(status_description)
                    .test_support()
                    .mt(px(8.))
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .items_center()
                    .when(progress.is_some(), |view| {
                        view.child(
                            Spinner::new()
                                .with_size(px(13.))
                                .color(cx.theme().muted_foreground),
                        )
                    })
                    .child(
                        div()
                            .text_size(px(crate::ui::FONT_SIZE))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if progress.is_some() {
                                cx.theme().muted_foreground
                            } else {
                                tone_color(state.tone, cx)
                            })
                            .child(status_label.to_owned()),
                    )
                    .when_some(composing.filter(|_| pending.is_none()), |view, job| {
                        let (id, name, project, job) = (
                            observed_id.clone(),
                            observed_name.clone(),
                            job.project.clone(),
                            job.job.clone(),
                        );
                        view.child(
                            crate::controls::compact_button("container-compose-output")
                                .link()
                                .px_0()
                                .label("View output")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_container_activity(
                                        (&id, &name),
                                        &project,
                                        &job,
                                        window,
                                        cx,
                                    );
                                })),
                        )
                    })
                    .when_some(lifecycle, |view, text| view.child(quiet(text, cx)))
                    .child(quiet(meta, cx)),
            )
            .when_some(problem, |v, problem| {
                let description = format!(
                    "{}.{}",
                    problem.reason,
                    problem
                        .restarts
                        .as_ref()
                        .map(|text| format!(" {text}"))
                        .unwrap_or_default()
                );
                let detail = problem.health_output.as_deref().unwrap_or_default();
                let warning_tone = if state.tone == domain::Tone::Danger {
                    domain::Tone::Danger
                } else {
                    domain::Tone::Warning
                };
                v.child(
                    div()
                        .id("container-problem")
                        .debug_selector(|| "container-problem".into())
                        .role(accesskit::Role::Alert)
                        .aria_label(format!("{description} {detail}").trim().to_owned())
                        .test_support()
                        .mt(px(8.))
                        .text_color(tone_color(warning_tone, cx))
                        .flex()
                        .items_start()
                        .gap(px(8.))
                        .child(
                            Icon::new(if warning_tone == domain::Tone::Danger {
                                IconName::OctagonAlert
                            } else {
                                IconName::TriangleAlert
                            })
                            .size(px(15.))
                            .text_color(tone_color(warning_tone, cx)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .text_size(px(crate::ui::FONT_SIZE))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(description),
                                )
                                .when(!detail.is_empty(), |v| {
                                    v.child(
                                        quiet(detail.to_owned(), cx)
                                            .mt(px(2.))
                                            .min_w_0()
                                            .max_w_full()
                                            .whitespace_normal()
                                            .when(problem.health_output.is_some(), |view| {
                                                view.font_family(
                                                    cx.theme().mono_font_family.clone(),
                                                )
                                                .text_size(px(crate::ui::FONT_SIZE))
                                            }),
                                    )
                                }),
                        ),
                )
            })
            .into_any_element()
    }
    /// A retained click must never follow the mutable selection or a recreated
    /// container. Opening output changes navigation without dispatching a command.
    pub(super) fn open_container_activity(
        &mut self,
        (id, name): (&str, &str),
        project: &str,
        job: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.retired
            || !self.selected_container_is(id, name)
            || self
                .active_selection
                .as_deref()
                .and_then(|key| key.strip_prefix("c:"))
                != Some(name)
            || self
                .client
                .selected_container()
                .and_then(|c| c.compose.as_ref())
                .is_none_or(|labels| labels.project != project)
        {
            return false;
        }
        self.open_compose_activity(project, job, window, cx)
    }
    fn container_facts(&self, c: &Container, cx: &mut Context<Self>) -> AnyElement {
        let history = self.client.stats_history(&c.id);
        let running = c.state == domain::ContainerState::Running;
        let ports = self.port_links(c, cx);
        if !running && c.restart_count == 0 && ports.is_none() {
            return div().children(self.port_errors(c, cx)).into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .py(px(10.))
            .border_t_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .id("container-facts")
                    .debug_selector(|| "container-facts".into())
                    .flex()
                    .flex_wrap()
                    .gap_x(px(28.))
                    .gap_y(px(10.))
                    .when(running, |v| {
                        v.child(
                            fact(
                                "CPU",
                                c.cpu.map(domain::format_cpu).unwrap_or_else(|| "—".into()),
                                cx,
                            )
                            .when_some(
                                history.and_then(|history| spark::cpu(history.cpu(), cx)),
                                |view, spark| view.child(spark),
                            ),
                        )
                        .child(
                            fact(
                                "Memory",
                                format!(
                                    "{}{}",
                                    c.memory
                                        .map(|memory| bytes(&json!(memory)))
                                        .unwrap_or_else(|| "—".into()),
                                    c.memory_limit
                                        .map(|limit| format!(" of {}", bytes(&json!(limit))))
                                        .unwrap_or_default()
                                ),
                                cx,
                            )
                            .when_some(
                                history.and_then(|history| {
                                    spark::memory(history.memory(), c.memory_limit, cx)
                                }),
                                |view, spark| view.child(spark),
                            ),
                        )
                    })
                    .when(c.restart_count > 0, |v| {
                        v.child(fact("Restarts", c.restart_count.to_string(), cx))
                    })
                    .children(ports),
            )
            .children(self.port_errors(c, cx))
            .into_any_element()
    }
    fn project_header(&self, name: &str, cx: &mut Context<Self>) -> AnyElement {
        use gpui_kit::component::spinner::Spinner;

        let Some(snapshot) = self.client.snapshot() else {
            return div().into_any_element();
        };
        let groups = domain::group_containers(&snapshot.containers);
        let Some(project) = groups.projects.iter().find(|p| p.name == name) else {
            return quiet("That project is no longer on this server.", cx).into_any_element();
        };
        let state = domain::project_state(project);
        let detail: Option<newport_domain::compose::ComposeProject> =
            serde_json::from_value(self.compose_value()).ok();
        let missing = compose_services::missing_count(project, detail.as_ref());
        let job = self.running_compose_job();
        let progress = self
            .client
            .action_status(&ActionTarget::Project(name.to_owned()))
            .filter(|status| status.pending)
            .and_then(|status| match &status.action {
                Action::Project { verb, .. } => Some(match verb {
                    ProjectVerb::Start => "Starting the project…",
                    ProjectVerb::Stop => "Stopping the project…",
                    ProjectVerb::Restart => "Restarting the project…",
                }),
                _ => None,
            })
            .or_else(|| {
                if job.is_some_and(|job| self.client.compose_cancelling(&job.job)) {
                    Some("Cancelling…")
                } else if job.is_some() || self.pending("compose-start") {
                    Some("Running a Compose command…")
                } else {
                    None
                }
            });
        let status = progress.unwrap_or(state.label);
        let count = format!(
            "{} of {} {} running{}",
            project.running,
            project.services.len(),
            if project.services.len() == 1 {
                "service"
            } else {
                "services"
            },
            if missing > 0 {
                format!(" · {missing} not created")
            } else {
                String::new()
            },
        );
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("compose-project-title")
                    .debug_selector(|| "compose-project-title".into())
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child(heading(name.to_owned(), 2))
                    .child(crate::controls::tag("Compose project", cx)),
            )
            .when_some(project.working_dir, |v, path| {
                v.child(
                    quiet(path.to_owned(), cx)
                        .id("compose-project-path")
                        .debug_selector(|| "compose-project-path".into())
                        .mt(px(2.))
                        .min_w_0()
                        .max_w_full()
                        .whitespace_normal()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(px(crate::ui::FONT_SIZE)),
                )
            })
            .child(
                div()
                    .id("compose-project-status")
                    .debug_selector(|| "compose-project-status".into())
                    .role(accesskit::Role::Status)
                    .aria_label(format!("{status} · {count}"))
                    .test_support()
                    .mt(px(8.))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(crate::ui::FONT_SIZE))
                    .when(progress.is_some(), |view| {
                        view.child(
                            Spinner::new()
                                .with_size(px(13.))
                                .color(cx.theme().muted_foreground),
                        )
                    })
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if progress.is_some() {
                                cx.theme().muted_foreground
                            } else {
                                tone_color(state.tone, cx)
                            })
                            .child(status),
                    )
                    .child(quiet(count, cx))
                    .when_some(job.filter(|_| self.tab != 1), |view, job| {
                        let (project, job) = (job.project.clone(), job.job.clone());
                        view.child(
                            crate::controls::compact_button("compose-view-output")
                                .link()
                                .px_0()
                                .label("View output")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.show_compose_output(&project, job.clone(), cx);
                                })),
                        )
                    }),
            )
            .into_any_element()
    }
    fn project_services(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(snapshot) = self.client.snapshot() else {
            return div().into_any_element();
        };
        let groups = domain::group_containers(&snapshot.containers);
        let Some(group) = groups
            .projects
            .iter()
            .find(|p| Some(p.name) == self.client.selected_project())
        else {
            return div().into_any_element();
        };
        let detail: Option<newport_domain::compose::ComposeProject> =
            serde_json::from_value(self.compose_value()).ok();
        self.compose_services_table(group, detail.as_ref(), window, cx)
    }
    /// Records the detail pane's width after layout, so content sized to it
    /// follows the resizable list rather than an assumed list width.
    pub(super) fn measure_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let owner = cx.weak_entity();
        canvas(
            move |bounds, window, cx| {
                let width = f32::from(bounds.size.width);
                let _ = owner.update(cx, |this, cx| {
                    if this.detail_width != width {
                        this.detail_width = width;
                        window.refresh();
                        cx.notify();
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full()
        .left_0()
        .top_0()
    }
    /// The detail pane's content width, inside its padding. Until the first
    /// layout measures it, assume the default 280px list.
    pub(super) fn detail_content_width(&self) -> f32 {
        let padding = 2. * crate::controls::PAGE_PADDING;
        if self.detail_width > 0. {
            self.detail_width - padding
        } else if self.width > 600. {
            self.width - 280. - padding
        } else {
            self.width - padding
        }
    }
    fn detail_bar(&self, container: bool, cx: &mut Context<Self>) -> AnyElement {
        // Stack log controls when the detail pane is narrow. Other tabs have
        // no controls row to reserve.
        let detail_width = self.detail_content_width();
        let stacked = detail_width <= 840.;
        div()
            .id("container-detail-bar")
            .debug_selector(|| "container-detail-bar".into())
            .flex()
            .flex_shrink_0()
            .min_w_0()
            .mb(px(8.))
            .min_h(px(if stacked && self.tab == 0 {
                36. + if detail_width < 480. { 64. } else { 28. }
            } else {
                35.
            }))
            .gap_x(px(12.))
            .when(stacked, |view| view.flex_col().items_stretch())
            .when(!stacked, |view| {
                view.items_end()
                    .border_b_1()
                    .border_color(cx.theme().border)
            })
            .child(
                div()
                    .id("container-detail-tabs-wrap")
                    .debug_selector(|| "container-detail-tabs-wrap".into())
                    .flex_shrink_0()
                    .child(self.detail_tabs(container, cx)),
            )
            .when(self.tab == 0, |view| {
                view.child(
                    div()
                        .id("container-detail-log-controls")
                        .debug_selector(|| "container-detail-log-controls".into())
                        .min_w_0()
                        .min_h(px(if stacked && detail_width < 480. {
                            72.
                        } else if stacked {
                            36.
                        } else {
                            34.
                        }))
                        .flex()
                        .when(stacked, |view| view.pt(px(8.)).justify_end())
                        .when(!stacked, |view| view.flex_1().pb(px(6.)).justify_end())
                        .child(self.log_toolbar(cx)),
                )
            })
            .into_any_element()
    }
    fn detail_tabs(&self, container: bool, cx: &mut Context<Self>) -> AnyElement {
        // Display order → retained tab index.
        let choices: &[(&str, usize)] = if container {
            &[("Logs", 0), ("Inspect", 1)]
        } else {
            &[
                ("Services", PROJECT_SERVICES),
                ("Logs", 0),
                ("Activity", 1),
                ("Compose files", 2),
            ]
        };
        let order: Rc<Vec<usize>> = Rc::new(choices.iter().map(|(_, tab)| *tab).collect());
        let count = choices.len();
        let selected = order.iter().position(|tab| *tab == self.tab).unwrap_or(0);
        let clicked = order.clone();
        crate::controls::tabs("container-detail-tabs")
            .underline()
            .label_size(px(crate::ui::FONT_SIZE))
            .accessibility_label("Container details")
            .selected_index(selected)
            .children(
                choices
                    .iter()
                    .copied()
                    .enumerate()
                    .map(|(position, (name, tab))| {
                        let order = order.clone();
                        Tab::new()
                            .label(name)
                            .track_focus(&self.tab_focus[tab].clone().tab_stop(tab == self.tab))
                            .on_key_down(cx.listener(
                                move |this, event: &KeyDownEvent, window, cx| {
                                    if event.keystroke.modifiers.modified()
                                        || window.has_active_dialog(cx)
                                    {
                                        return;
                                    }
                                    let next = order[match event.keystroke.key.as_str() {
                                        "left" => (position + count - 1) % count,
                                        "right" => (position + 1) % count,
                                        "home" => 0,
                                        "end" => count - 1,
                                        "enter" | "space" => position,
                                        _ => return,
                                    }];
                                    this.compose_state.show_activity_for = None;
                                    this.tab = next;
                                    this.client.revalidate_details();
                                    this.tab_focus[next].focus(window, cx);
                                    this.sync_selection(cx);
                                    window.prevent_default();
                                    cx.stop_propagation();
                                    cx.notify();
                                },
                            ))
                    }),
            )
            .on_click(cx.listener(move |this, position: &usize, window, cx| {
                let tab = clicked[*position];
                this.compose_state.show_activity_for = None;
                this.tab = tab;
                this.client.revalidate_details();
                this.tab_focus[tab].focus(window, cx);
                this.sync_selection(cx);
                cx.notify();
            }))
            .into_any_element()
    }
}
fn fact(name: &str, value: impl Into<SharedString>, cx: &App) -> Div {
    let value = value.into();
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(quiet(name.to_owned(), cx).text_size(px(crate::ui::FONT_SIZE)))
        .child(
            div()
                .id(SharedString::from(format!(
                    "container-fact-{}-value",
                    name.to_lowercase().replace(' ', "-")
                )))
                .role(accesskit::Role::Label)
                .aria_label(value.clone())
                .text_size(px(crate::ui::FONT_SIZE))
                .child(value)
                .test_support(),
        )
}

fn detail_skeleton() -> AnyElement {
    div()
        .id("container-detail-loading")
        .role(accesskit::Role::Status)
        .aria_label("Loading container details")
        .a11y_synthetic_children(|tree| tree.parent_node().set_busy())
        .flex_1()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(Skeleton::new().w(px(220.)).h(px(18.)))
        .child(Skeleton::new().w(px(180.)).h(px(11.)))
        .child(Skeleton::new().w(px(130.)).h(px(12.)))
        .child(
            div()
                .flex()
                .gap(px(6.))
                .children([84., 64., 64.].map(|width| Skeleton::new().w(px(width)).h(px(28.)))),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(28.))
                .py(px(10.))
                .children((0..4).map(|_| {
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(Skeleton::new().w(px(50.)).h(px(10.)))
                        .child(Skeleton::new().w(px(70.)).h(px(14.)))
                })),
        )
        .child(
            div()
                .flex()
                .gap(px(16.))
                .child(Skeleton::new().w(px(36.)).h(px(16.)))
                .child(Skeleton::new().w(px(52.)).h(px(16.))),
        )
        .child(
            div()
                .flex_1()
                .min_h(px(180.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .py(px(12.))
                .children(
                    [0.8, 0.65, 0.9, 0.55, 0.7]
                        .map(|width| Skeleton::new().w(relative(width)).h(px(10.))),
                ),
        )
        .into_any_element()
}
