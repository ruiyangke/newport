use super::*;
#[cfg(test)]
#[path = "containers_inspect_tests.rs"]
mod tests;

enum InspectAction {
    Retry,
    CopyCommand(String),
    ToggleValues,
    Reveal(String),
    ToggleLabels,
}
impl ContainersPanel {
    fn inspect_action(&mut self, id: &str, action: InspectAction, cx: &mut Context<Self>) -> bool {
        // Mouse/keyboard callbacks retain the painted target. A new selection,
        // recreated name, tab change or retired connection invalidates them.
        if self.retired
            || self.tab != 1
            || self.inspected_id.as_deref() != Some(id)
            || self
                .client
                .selected_container()
                .is_none_or(|container| container.id != id)
        {
            return false;
        }
        match action {
            InspectAction::Retry => self.client.retry_inspect(),
            InspectAction::CopyCommand(command) => {
                if self.client.inspect().is_none_or(|data| data.id != id) || command.is_empty() {
                    return false;
                }
                cx.write_to_clipboard(ClipboardItem::new_string(command));
                cx.emit(ContainersEvent::Notification {
                    message: "Copied command.".into(),
                });
            }
            InspectAction::ToggleValues => {
                self.reveal_values = !self.reveal_values;
                self.revealed.clear();
            }
            InspectAction::Reveal(name) => {
                if self.client.inspect().is_none_or(|data| {
                    data.id != id || !data.env.iter().any(|env| env.name == name)
                }) {
                    return false;
                }
                self.revealed.insert(name);
            }
            InspectAction::ToggleLabels => self.labels_open = !self.labels_open,
        }
        cx.notify();
        true
    }
    pub(super) fn inspect_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let data = self.client.inspect();
        let retry_id = self.inspected_id.clone();
        let body = div()
            .id("container-inspect")
            .debug_selector(|| "container-inspect".into())
            .role(accesskit::Role::Region)
            .aria_label("Container inspection")
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .gap(px(18.))
            .when(self.error("container-inspect").is_some(), |v| {
                v.child(self.status("container-inspect", cx))
            })
            .when(self.error("container-inspect").is_some(), |v| {
                v.child(
                    crate::controls::button("retry-container-inspect")
                        .label("Try again")
                        .accessibility_label("Try inspecting again")
                        .disabled(self.pending("container-inspect"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(id) = &retry_id {
                                this.inspect_action(id, InspectAction::Retry, cx);
                            }
                        })),
                )
            })
            .when(
                data.is_none() && self.error("container-inspect").is_none(),
                |v| v.child(loading::lines("container-inspect", 9)),
            );
        let Some(data) = data else {
            return body.into_any_element();
        };
        let command = data
            .entrypoint
            .iter()
            .flatten()
            .chain(data.cmd.iter().flatten())
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        let copy = command.clone();
        let copy_id = data.id.clone();
        let values_id = data.id.clone();
        let labels_id = data.id.clone();
        body.child(
            section("Command", None)
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .gap(px(8.))
                        .child(
                            mono(
                                cx,
                                if command.is_empty() {
                                    "—".into()
                                } else {
                                    command
                                },
                            )
                            .flex_1()
                            .min_w_0()
                            .debug_selector(|| "inspect-command".into())
                            .bg(cx.theme().muted)
                            .rounded(px(crate::ui::RADIUS))
                            .px(px(8.))
                            .py(px(6.)),
                        )
                        .child(
                            crate::controls::compact_icon_button("copy-container-command")
                                .icon(IconName::Copy)
                                .accessibility_label("Copy container command")
                                .disabled(copy.is_empty())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.inspect_action(
                                        &copy_id,
                                        InspectAction::CopyCommand(copy.clone()),
                                        cx,
                                    );
                                })),
                        ),
                )
                .child(
                    quiet(
                        format!(
                            "{}as {}{}{}",
                            data.working_dir
                                .as_ref()
                                .filter(|s| !s.is_empty())
                                .map(|s| format!("in {s} · "))
                                .unwrap_or_default(),
                            data.user
                                .as_deref()
                                .filter(|s| !s.is_empty())
                                .unwrap_or("root"),
                            data.hostname
                                .as_ref()
                                .filter(|s| !s.is_empty())
                                .map(|s| format!(" · hostname {s}"))
                                .unwrap_or_default(),
                            data.restart_policy
                                .as_ref()
                                .filter(|s| !s.is_empty())
                                .map(|s| format!(" · restart: {s}"))
                                .unwrap_or_default()
                        ),
                        cx,
                    )
                    .min_w_0()
                    .max_w_full()
                    .whitespace_normal(),
                ),
        )
        .child(
            section("Environment", Some(data.env.len()))
                .when(!data.env.is_empty(), |v| {
                    v.child(
                        div().flex().items_start().child(
                            crate::controls::compact_button("reveal-container-env")
                                .ghost()
                                .icon(if self.reveal_values {
                                    IconName::EyeOff
                                } else {
                                    IconName::Eye
                                })
                                .label(if self.reveal_values {
                                    "Hide values"
                                } else {
                                    "Show values"
                                })
                                .toggled(self.reveal_values)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.inspect_action(
                                        &values_id,
                                        InspectAction::ToggleValues,
                                        cx,
                                    );
                                })),
                        ),
                    )
                })
                .when(data.env.is_empty(), |v| v.child(quiet("None set.", cx)))
                .children(data.env.iter().enumerate().map(|(index, env)| {
                    let visible = (self.reveal_values && !domain::secret_env_name(&env.name))
                        || self.revealed.contains(&env.name);
                    let name = env.name.clone();
                    let id = data.id.clone();
                    row(cx)
                        .child(
                            mono(cx, name.clone())
                                .debug_selector(move || format!("inspect-env-name-{index}"))
                                .w(relative(0.38))
                                .flex_shrink_0()
                                .text_color(cx.theme().muted_foreground),
                        )
                        .child(if visible {
                            mono(
                                cx,
                                if env.value.is_empty() {
                                    "(empty)".into()
                                } else {
                                    env.value.clone()
                                },
                            )
                            .flex_1()
                            .min_w_0()
                            .debug_selector(move || format!("inspect-env-value-{index}"))
                            .into_any_element()
                        } else {
                            crate::controls::compact_button(("reveal-env", index))
                                .ghost()
                                .px_0()
                                .label(if self.reveal_values {
                                    "•••••• (secret)"
                                } else {
                                    "••••••"
                                })
                                .accessibility_label(format!("Show {name}"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.inspect_action(
                                        &id,
                                        InspectAction::Reveal(name.clone()),
                                        cx,
                                    );
                                }))
                                .into_any_element()
                        })
                })),
        )
        .child(
            section("Mounts", Some(data.mounts.len()))
                .when(data.mounts.is_empty(), |v| v.child(quiet("None.", cx)))
                .children(data.mounts.iter().map(|mount| {
                    row(cx)
                        .child(
                            mono(
                                cx,
                                format!(
                                    "{} ← {}",
                                    mount.destination,
                                    if mount.kind == "volume" {
                                        format!(
                                            "volume {}",
                                            mount
                                                .name
                                                .as_deref()
                                                .or(mount.source.as_deref())
                                                .unwrap_or_default()
                                        )
                                    } else {
                                        mount.source.as_deref().unwrap_or(&mount.kind).into()
                                    }
                                ),
                            )
                            .flex_1()
                            .min_w_0(),
                        )
                        .child(
                            quiet(if mount.rw { "read-write" } else { "read-only" }, cx)
                                .text_size(px(crate::ui::FONT_SIZE))
                                .flex_shrink_0(),
                        )
                })),
        )
        .child(
            section("Networks", Some(data.networks.len()))
                .when(data.networks.is_empty(), |v| v.child(quiet("None.", cx)))
                .children(data.networks.iter().map(|network| {
                    row(cx)
                        .flex_wrap()
                        .child(
                            div()
                                .min_w_0()
                                .max_w_full()
                                .whitespace_normal()
                                .font_weight(FontWeight::MEDIUM)
                                .child(network.name.clone()),
                        )
                        .when_some(network.ip_address.clone(), |v, ip| v.child(mono(cx, ip)))
                        .when(!network.aliases.is_empty(), |v| {
                            v.child(
                                quiet(format!("reachable as {}", network.aliases.join(", ")), cx)
                                    .min_w_0()
                                    .max_w_full()
                                    .whitespace_normal(),
                            )
                        })
                })),
        )
        .when_some(data.healthcheck.as_ref(), |v, check| {
            let command = health_command(&check.test);
            v.child(
                section("Health check", None)
                    .child(
                        mono(cx, command.clone())
                            .id("inspect-health-command")
                            .role(accesskit::Role::Label)
                            .aria_label(command)
                            .test_support()
                            .bg(cx.theme().muted)
                            .rounded(px(crate::ui::RADIUS))
                            .px(px(8.))
                            .py(px(6.)),
                    )
                    .when_some(
                        check.interval.filter(|interval| *interval > 0),
                        |v, interval| {
                            let interval = format!(
                                "Every {}",
                                newport_domain::services::duration(
                                    i64::try_from(interval).unwrap_or(i64::MAX)
                                )
                            );
                            v.child(
                                quiet(interval.clone(), cx)
                                    .id("inspect-health-interval")
                                    .role(accesskit::Role::Label)
                                    .aria_label(interval)
                                    .test_support(),
                            )
                        },
                    )
                    .children(data.last_health.iter().enumerate().map(|(index, check)| {
                        let output = if check.output.trim().is_empty() {
                            "—"
                        } else {
                            check.output.trim()
                        };
                        row(cx)
                            .child(
                                quiet(crate::localized_time::updated_time(check.at), cx)
                                    .flex_shrink_0(),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_color(if check.exit_code == 0 {
                                        theme::success(cx.theme().is_dark())
                                    } else {
                                        theme::error(cx.theme().is_dark())
                                    })
                                    .child(if check.exit_code == 0 {
                                        "passed".into()
                                    } else {
                                        format!("failed ({})", check.exit_code)
                                    }),
                            )
                            .child(
                                mono(cx, output.to_owned())
                                    .flex_1()
                                    .min_w_0()
                                    .id(("inspect-health-output", index))
                                    .role(accesskit::Role::Label)
                                    .aria_label(output.to_owned())
                                    .test_support(),
                            )
                    })),
            )
        })
        .child(
            section("Labels", Some(data.labels.len()))
                .child(
                    div().flex().items_start().child(
                        crate::controls::compact_button("container-labels")
                            .ghost()
                            .label(if self.labels_open {
                                "Hide labels"
                            } else {
                                "Show labels"
                            })
                            .icon(if self.labels_open {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.inspect_action(&labels_id, InspectAction::ToggleLabels, cx);
                            })),
                    ),
                )
                .when(self.labels_open, |v| {
                    v.children(data.labels.iter().map(|(name, value)| {
                        row(cx)
                            .child(
                                mono(cx, name.clone())
                                    .w(relative(0.45))
                                    .flex_shrink_0()
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .child(mono(cx, value.clone()).flex_1().min_w_0())
                    }))
                }),
        )
        .into_any_element()
    }
}
fn section(title: &str, count: Option<usize>) -> gpui_kit::base::ObservedElement<Stateful<Div>> {
    div()
        .id(SharedString::from(format!("inspect-section-{title}")))
        .role(accesskit::Role::Region)
        .aria_label(title.to_owned())
        .test_support()
        .flex()
        .flex_col()
        .w_full()
        .min_w_0()
        .gap(px(6.))
        .child(
            div()
                .text_size(px(crate::ui::FONT_SIZE))
                .font_weight(FontWeight::SEMIBOLD)
                .child(format!(
                    "{title}{}",
                    count.map(|n| format!("  {n}")).unwrap_or_default()
                )),
        )
}
fn health_command(test: &[String]) -> String {
    // Only Docker's first execution sentinel is metadata; later arguments
    // with the same spelling are literal command content.
    test.iter()
        .skip(usize::from(test.first().is_some_and(|part| {
            matches!(part.as_str(), "CMD" | "CMD-SHELL")
        })))
        .cloned()
        .collect::<Vec<_>>()
        .join(" ")
}
fn row(cx: &App) -> Div {
    div()
        .flex()
        .w_full()
        .min_w_0()
        .items_start()
        .gap(px(12.))
        .border_b_1()
        .border_color(cx.theme().border)
        .py(px(5.))
        .text_size(px(crate::ui::FONT_SIZE))
}
fn mono(cx: &App, value: impl Into<SharedString>) -> Div {
    div()
        .min_w_0()
        .max_w_full()
        .whitespace_normal()
        .font_family(cx.theme().mono_font_family.clone())
        .text_size(px(crate::ui::FONT_SIZE))
        .child(value.into())
}
