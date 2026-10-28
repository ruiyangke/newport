use super::*;
use gpui_kit::component::{input::Editor, tab::Tab};
impl ServicesPanel {
    pub(super) fn detail(&self, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.effective_selection(cx);
        let snapshot = self.client.snapshot();
        let service = snapshot.and_then(|s| {
            s.services
                .iter()
                .find(|u| selected == Selection::Unit(u.key()))
        });
        let timer = snapshot.and_then(|s| {
            s.timers
                .iter()
                .find(|u| selected == Selection::Unit(u.key()))
        });
        // Unit tabs size to the pane as last laid out. When the list beside
        // it is resized, lay out again with the new width.
        let laid_out = self.detail_scroll.bounds().size.width;
        div()
            .id("services-detail")
            .debug_selector(|| "services-detail".into())
            .role(accesskit::Role::Region)
            .aria_label("Service detail")
            .track_focus(&self.detail_focus)
            .tab_stop(false)
            .min_w_0()
            .min_h_0()
            .flex_1()
            .h_full()
            .overflow_y_scroll()
            .track_scroll(&self.detail_scroll)
            .child(
                canvas(
                    move |bounds, window, _| {
                        if bounds.size.width != laid_out {
                            window.refresh();
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full()
                .left_0()
                .top_0(),
            )
            .child(
                div()
                    .min_h_full()
                    .p(px(crate::controls::PAGE_PADDING))
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .when(self.width <= 600., |v| {
                        v.child(
                            crate::controls::compact_button("services-back")
                                .label("All services")
                                .icon(IconName::ArrowLeft)
                                .ghost()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.selected = None;
                                    this.review = None;
                                    this.list_focus.focus(window, cx);
                                    this.sync_selection(cx);
                                    cx.notify();
                                })),
                        )
                    })
                    .when(snapshot.is_none(), |v| v.child(detail_skeleton()))
                    .when(snapshot.is_some(), |v| {
                        if let Some(unit) = service {
                            v.child(self.service(unit, cx))
                        } else if let Some(timer) = timer {
                            v.child(self.timer(timer, cx))
                        } else if selected == Selection::Journal {
                            v.child(self.journal(cx))
                        } else {
                            v.child(crate::empty::pane(IconName::Server, "Service unavailable", "That service is no longer loaded. Choose another service or refresh the list.", cx))
                        }
                    }),
            )
            .into_any_element()
    }
    fn journal(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex_1()
            .min_h(px(410.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_end()
                    .justify_between()
                    .gap(px(10.))
                    .child(div().child(title("System journal")))
                    .child(self.log_toolbar(cx)),
            )
            .child(self.log_view("Journal", cx))
            .into_any_element()
    }
    fn service(&self, unit: &ServiceUnit, cx: &mut Context<Self>) -> AnyElement {
        let state = domain::state_of(unit);
        let pending = self
            .client
            .action_status(&unit.name, unit.scope)
            .filter(|s| s.pending)
            .map(|s| s.verb);
        let matching: Vec<_> = self
            .client
            .snapshot()
            .into_iter()
            .flat_map(|s| s.timers.iter())
            .filter(|t| t.scope == unit.scope && t.unit.as_deref() == Some(&unit.name))
            .collect();
        let mut triggered = unit.triggered_by.clone();
        for t in &matching {
            if !triggered.contains(&t.name) {
                triggered.push(t.name.clone());
            }
        }
        let restarts = unit.restarts.filter(|n| *n > 0);
        let has_facts = unit.main_pid.is_some() || restarts.is_some() || !triggered.is_empty();
        // Identity and boot behaviour that rarely change ride the status line.
        let boot_switch = {
            let verbs = domain::available_verbs(unit);
            verbs.enable || verbs.disable
        };
        let meta = [
            unit.main_pid.map(|pid| format!("PID {pid}")),
            Some(format!(
                "runs as {}",
                unit.user.clone().unwrap_or(
                    if unit.scope == Scope::User {
                        "you"
                    } else {
                        "root"
                    }
                    .into()
                )
            )),
            (!boot_switch)
                .then(|| match unit.enabled.as_deref() {
                    Some("static") => Some("started by another unit".to_owned()),
                    Some("masked") => Some("masked".to_owned()),
                    Some("enabled") => Some("starts at boot".to_owned()),
                    Some("disabled") => Some("doesn’t start at boot".to_owned()),
                    _ => None,
                })
                .flatten(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        let facts = div()
            .flex()
            .flex_wrap()
            .gap_x(px(28.))
            .gap_y(px(10.))
            .when(unit.main_pid.is_some(), |v| {
                v.child(fact(
                    "CPU",
                    unit.cpu.map(|n| format!("{n:.1}%")).unwrap_or("—".into()),
                    None,
                    cx,
                ))
                .child(fact(
                    "Memory",
                    unit.memory.map(|n| bytes(&json!(n))).unwrap_or("—".into()),
                    None,
                    cx,
                ))
            })
            .when_some(restarts, |v, restarts| {
                v.child(fact("Restarts", restarts.to_string(), None, cx))
            })
            .when(!triggered.is_empty(), |v| {
                v.child(
                    div()
                        .min_w_0()
                        .child(quiet("Triggered by", cx).text_size(px(crate::ui::FONT_SIZE)))
                        .children(triggered.iter().map(|name| {
                            if let Some(timer) = matching.iter().find(|timer| timer.name == *name) {
                                let key = timer.key();
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .child(
                                        crate::controls::compact_button(SharedString::from(
                                            format!("timer-link-{key}"),
                                        ))
                                        .link()
                                        .px_0()
                                        .label(name.clone())
                                        .child(
                                            Icon::new(IconName::ArrowRight)
                                                .size(px(crate::controls::ICON_SIZE)),
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.select(Selection::Unit(key.clone()), cx)
                                            }),
                                        ),
                                    )
                                    .child(
                                        quiet(format!(" · next {}", self.clock.timer(timer)), cx)
                                            .text_size(px(crate::ui::FONT_SIZE)),
                                    )
                            } else {
                                div()
                                    .text_size(px(crate::ui::FONT_SIZE))
                                    .child(name.clone())
                            }
                        })),
                )
            });
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.))
                            .child(title(unit.name.clone()))
                            .when(unit.scope == Scope::User, |v| {
                                v.child(crate::controls::tag("user service", cx))
                            }),
                    )
                    .child(
                        quiet(unit.description.clone(), cx)
                            .text_size(px(crate::ui::FONT_SIZE))
                            .mt(px(2.)),
                    )
                    .child(
                        div()
                            .mt(px(8.))
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.))
                            .text_size(px(crate::ui::FONT_SIZE))
                            .child(
                                div()
                                    .id("service-state")
                                    .role(accesskit::Role::Status)
                                    .aria_label(pending.map(progress).unwrap_or(state.label))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(tone_color(
                                        if pending.is_some() {
                                            Tone::Muted
                                        } else {
                                            state.tone
                                        },
                                        cx,
                                    ))
                                    .child(pending.map(progress).unwrap_or(state.label)),
                            )
                            .when_some(unit.since.filter(|_| pending.is_none()), |v, since| {
                                v.child(
                                    quiet(
                                        format!(
                                            "{} {}{}",
                                            if unit.active == "active" && unit.sub == "running" {
                                                "for"
                                            } else {
                                                "since"
                                            },
                                            self.clock.elapsed(since),
                                            if unit.active == "active" && unit.sub == "running" {
                                                ""
                                            } else {
                                                " ago"
                                            }
                                        ),
                                        cx,
                                    )
                                    .id("service-since")
                                    .hover_hint(date(since)),
                                )
                            })
                            .when(!meta.is_empty(), |v| {
                                v.child(quiet(format!("· {meta}"), cx).id("service-meta"))
                            }),
                    ),
            )
            .when_some(domain::failure_reason(unit), |v, reason| {
                v.child(
                    div()
                        .id("service-failure")
                        .role(accesskit::Role::Alert)
                        .aria_label(reason.clone())
                        .text_color(tone_color(state.tone, cx))
                        .text_size(px(crate::ui::FONT_SIZE))
                        .flex()
                        .items_start()
                        .gap(px(8.))
                        .child(
                            Icon::new(if unit.active == "failed" {
                                IconName::OctagonAlert
                            } else {
                                IconName::TriangleAlert
                            })
                            .size(px(15.))
                            .mt(px(1.))
                            .text_color(tone_color(state.tone, cx)),
                        )
                        .child(div().flex_1().min_w_0().child(reason)),
                )
            })
            .child(self.service_actions(unit, cx))
            .when(has_facts, |v| {
                v.child(
                    facts
                        .py(px(10.))
                        .border_t_1()
                        .border_b_1()
                        .border_color(cx.theme().border),
                )
            })
            .child(self.unit_tabs(
                &unit.name,
                true,
                unit.fragment.as_deref(),
                unit.drop_ins.len(),
                cx,
            ))
            .into_any_element()
    }
    fn timer(&self, timer: &TimerUnit, cx: &mut Context<Self>) -> AnyElement {
        let target = self.client.snapshot().and_then(|s| {
            s.services
                .iter()
                .find(|u| Some(u.name.as_str()) == timer.unit.as_deref() && u.scope == timer.scope)
        });
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(
                div().child(title(timer.name.clone())).child(
                    quiet(timer.description.clone(), cx)
                        .text_size(px(crate::ui::FONT_SIZE))
                        .mt(px(2.)),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_x(px(28.))
                    .gap_y(px(10.))
                    .child(fact(
                        "Next run",
                        timer
                            .next
                            .map(|at| self.clock.until(at))
                            .unwrap_or("Not scheduled".into()),
                        timer.next.map(date),
                        cx,
                    ))
                    .child(fact(
                        "Last run",
                        timer
                            .last
                            .map(|at| format!("{} ago", self.clock.elapsed(at)))
                            .unwrap_or("Never".into()),
                        timer.last.map(date),
                        cx,
                    ))
                    .child(
                        div()
                            .child(quiet("Runs", cx).text_size(px(crate::ui::FONT_SIZE)))
                            .child(if let Some(target) = target {
                                let key = target.key();
                                let label = format!(
                                    "{} · {}",
                                    target.name,
                                    domain::state_of(target).label.to_lowercase()
                                );
                                div()
                                    .flex()
                                    .items_start()
                                    .child(
                                        crate::controls::compact_button("timer-service-link")
                                            .text()
                                            .px_0()
                                            .group("timer-service-link")
                                            .accessibility_label(label.clone())
                                            .child(
                                                div()
                                                    .text_size(px(crate::ui::FONT_SIZE))
                                                    .text_color(cx.theme().foreground)
                                                    .group_hover("timer-service-link", |style| {
                                                        style.text_decoration_1()
                                                    })
                                                    .child(label),
                                            )
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.select(Selection::Unit(key.clone()), cx)
                                            })),
                                    )
                                    .into_any_element()
                            } else {
                                div()
                                    .text_size(px(crate::ui::FONT_SIZE))
                                    .child(timer.unit.clone().unwrap_or("—".into()))
                                    .into_any_element()
                            }),
                    ),
            )
            .child(self.unit_tabs(&timer.name, timer.unit.is_some(), None, 0, cx))
            .into_any_element()
    }
    fn service_actions(&self, unit: &ServiceUnit, cx: &mut Context<Self>) -> AnyElement {
        let verbs = domain::available_verbs(unit);
        let running = !verbs.start;
        let blocked = self
            .client
            .snapshot()
            .and_then(|s| domain::action_blocked_reason(unit.scope, s.privilege));
        let status = self.client.action_status(&unit.name, unit.scope);
        let review = self
            .review
            .as_ref()
            .filter(|r| r.unit == unit.name && r.scope == unit.scope);
        let handover = status
            .and_then(|s| s.result.as_ref())
            .filter(|r| r.needs_password == Some(true))
            .and_then(|r| r.command.clone());
        let action = |verb: ServiceVerb,
                      id: &'static str,
                      primary: bool,
                      this: &Self,
                      cx: &mut Context<Self>| {
            let name = unit.name.clone();
            let scope = unit.scope;
            let busy = status.is_some_and(|s| s.pending && s.verb == verb);
            crate::controls::button(id)
                .label(verb.label())
                .when(
                    matches!(
                        verb,
                        ServiceVerb::Start | ServiceVerb::Stop | ServiceVerb::Restart
                    ),
                    |b| {
                        b.icon(match verb {
                            ServiceVerb::Start => IconName::Play,
                            ServiceVerb::Stop => IconName::Square,
                            _ => IconName::RotateCw,
                        })
                    },
                )
                .when(primary, |b| b.primary())
                .disabled(!this.client.can_action(&name, scope, verb))
                .loading(busy)
                .on_click(cx.listener(move |this, _, window, cx| {
                    if matches!(verb, ServiceVerb::Stop | ServiceVerb::Disable) {
                        this.open_review(
                            Review {
                                unit: name.clone(),
                                scope,
                                verb,
                            },
                            window,
                            cx,
                        );
                    } else {
                        this.start_action(name.clone(), scope, verb, cx);
                    }
                }))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .child(action(
                        if running {
                            ServiceVerb::Restart
                        } else {
                            ServiceVerb::Start
                        },
                        "service-primary",
                        !running,
                        self,
                        cx,
                    ))
                    .when(verbs.stop, |v| {
                        v.child(action(ServiceVerb::Stop, "service-stop", false, self, cx))
                    })
                    .when(!running && verbs.restart && unit.active == "failed", |v| {
                        v.child(action(
                            ServiceVerb::Restart,
                            "service-restart",
                            false,
                            self,
                            cx,
                        ))
                    })
                    .when(verbs.reload, |v| v.child(self.more_actions(unit, cx)))
                    .when(verbs.enable || verbs.disable, |v| {
                        let name = unit.name.clone();
                        let scope = unit.scope;
                        let next = if unit.enabled.as_deref() == Some("enabled") {
                            ServiceVerb::Disable
                        } else {
                            ServiceVerb::Enable
                        };
                        v.child(
                            div().ml_auto().child(
                                crate::controls::switch("service-at-boot")
                                    .label("Start at boot")
                                    .checked(unit.enabled.as_deref() == Some("enabled"))
                                    .disabled(!self.client.can_action(&name, scope, next))
                                    .on_click(cx.listener(move |this, checked, window, cx| {
                                        let verb = if *checked {
                                            ServiceVerb::Enable
                                        } else {
                                            ServiceVerb::Disable
                                        };
                                        if verb == ServiceVerb::Disable {
                                            this.open_review(
                                                Review {
                                                    unit: name.clone(),
                                                    scope,
                                                    verb,
                                                },
                                                window,
                                                cx,
                                            );
                                        } else {
                                            this.start_action(name.clone(), scope, verb, cx);
                                        }
                                    })),
                            ),
                        )
                    }),
            )
            .when_some(blocked, |v, reason| v.child(quiet(reason, cx)))
            .when(review.is_none(), |v| {
                v.when_some(status.and_then(|s| s.error.clone()), |v, error| {
                    v.child(alert("service-action-error", error, cx))
                })
            })
            .when_some(handover, |v, command| {
                v.child(
                    div()
                        .id("service-handover")
                        .role(accesskit::Role::Alert)
                        .rounded(px(crate::ui::RADIUS))
                        .border_1()
                        .border_color(tone_color(Tone::Warning, cx).opacity(0.3))
                        .bg(tone_color(Tone::Warning, cx).opacity(0.05))
                        .p(px(12.))
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(div().text_size(px(crate::ui::FONT_SIZE)).child(
                            "This needs your sudo password. Run this command in the Terminal.",
                        ))
                        .child(
                            div()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(px(crate::ui::FONT_SIZE))
                                .child(command.clone()),
                        )
                        .child(
                            crate::controls::button("service-open-terminal")
                                .label("Open in Terminal")
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(ServicesEvent::OpenTerminal {
                                        command: command.clone(),
                                    })
                                })),
                        ),
                )
            })
            .into_any_element()
    }
    /// Stop and Disable are confirmed in a modal dialog, like Containers'
    /// reviews. Enter never confirms: it only activates the focused button.
    fn open_review(&mut self, review: Review, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) || self.retired {
            return;
        }
        self.review = Some(review);
        self.review_token += 1;
        let token = self.review_token;
        let owner = cx.weak_entity();
        // Kit restores the focus captured when opening, including after Escape.
        self.detail_focus.focus(window, cx);
        window.open_dialog(cx, move |dialog, window, cx| {
            let Some(panel) = owner.upgrade() else {
                return dialog;
            };
            let (title, body, footer, pending) =
                panel.update(cx, |panel, cx| panel.review_parts(token, cx));
            let cancel = owner.clone();
            let close = owner.clone();
            let confirm = owner.clone();
            dialog
                .title(crate::controls::dialog_title(title))
                .w(px(480.))
                .max_w((window.viewport_size().width - px(32.)).max(px(240.)))
                .p(px(24.))
                .rounded(px(crate::ui::RADIUS))
                .border_0()
                .centered(true)
                .overlay_closable(false)
                .keyboard(!pending)
                .close_button(!pending)
                .on_ok(move |_, window, cx| {
                    confirm
                        .update(cx, |panel, cx| {
                            if !panel.review_dialog_current(token)
                                || panel.retired
                                || panel.review_pending()
                            {
                                return false;
                            }
                            if panel.review_cancel_focus.is_focused(window) {
                                return true;
                            }
                            if panel.review_confirm_focus.is_focused(window) {
                                panel.confirm_review(token, cx);
                            }
                            false
                        })
                        .unwrap_or(false)
                })
                .on_cancel(move |_, _, cx| {
                    cancel.upgrade().is_some_and(|panel| {
                        let panel = panel.read(cx);
                        panel.review_dialog_current(token) && !panel.review_pending()
                    })
                })
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |panel, cx| {
                        if panel
                            .review_dialog
                            .as_ref()
                            .is_some_and(|dialog| dialog.token == token)
                        {
                            panel.review = None;
                            panel.review_dialog = None;
                            cx.notify();
                        }
                    });
                })
                .child(body)
                .footer(footer)
        });
        self.review_dialog = window.active_dialog_handle(cx).map(|handle| ReviewDialog {
            token,
            window: window.window_handle(),
            handle,
        });
        let owner = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            let focus = owner.upgrade().and_then(|panel| {
                let panel = panel.read(cx);
                panel
                    .review_dialog_current(token)
                    .then(|| panel.review_cancel_focus.clone())
            });
            if let Some(focus) = focus {
                focus.focus(window, cx);
            }
        });
        cx.notify();
    }
    fn review_pending(&self) -> bool {
        self.review.as_ref().is_some_and(|review| {
            self.client
                .action_status(&review.unit, review.scope)
                .is_some_and(|status| status.pending)
        })
    }
    fn confirm_review(&mut self, token: u64, cx: &mut Context<Self>) {
        if let Some(review) = self.review.clone()
            && self.review_dialog_current(token)
        {
            self.start_action(review.unit, review.scope, review.verb, cx);
        }
    }
    fn dismiss_review(&mut self, token: u64, window: &mut Window, cx: &mut Context<Self>) {
        if !self.review_dialog_current(token) || self.review_pending() {
            return;
        }
        self.review = None;
        if let Some(dialog) = self.review_dialog.take() {
            window.close_dialog_if(dialog.handle.focus_handle(), cx);
        }
        cx.notify();
    }
    fn review_parts(
        &self,
        token: u64,
        cx: &mut Context<Self>,
    ) -> (String, AnyElement, AnyElement, bool) {
        let Some(Review {
            unit: name,
            scope,
            verb,
        }) = self.review.clone()
        else {
            return (
                String::new(),
                div().into_any_element(),
                div().into_any_element(),
                false,
            );
        };
        let title = if verb == ServiceVerb::Stop {
            format!("Stop {}?", domain::short_name(&name))
        } else {
            format!("Don’t start at boot {}?", domain::short_name(&name))
        };
        let description = if verb == ServiceVerb::Stop {
            format!(
                "This stops {name}{}. Anything relying on it stops working until it is started again.",
                if scope == Scope::User {
                    " (user service)"
                } else {
                    ""
                }
            )
        } else {
            format!(
                "{name} will no longer start when the server boots. It keeps running until it is stopped."
            )
        };
        let status = self.client.action_status(&name, scope);
        let pending = status.is_some_and(|status| status.pending);
        let allowed = self.review_dialog_current(token)
            && !self.retired
            && self.client.can_action(&name, scope, verb);
        let body = div()
            .id("service-action-review")
            .role(accesskit::Role::Region)
            .aria_label(title.clone())
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                quiet(description.clone(), cx)
                    .id("service-review-description")
                    .role(accesskit::Role::Label)
                    .aria_label(description),
            )
            .when_some(status.and_then(|s| s.error.clone()), |v, error| {
                v.child(alert("service-review-error", error, cx))
            })
            .into_any_element();
        let footer = div()
            .flex()
            .w_full()
            .justify_end()
            .gap(px(8.))
            .child(
                crate::controls::form_button("service-review-cancel")
                    .min_w(px(76.))
                    .track_focus(&self.review_cancel_focus)
                    .label("Cancel")
                    .disabled(pending)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.dismiss_review(token, window, cx)
                    })),
            )
            .child(
                crate::controls::form_button("service-review-confirm")
                    .min_w(px(76.))
                    .track_focus(&self.review_confirm_focus)
                    .label(verb.label())
                    .accessibility_label(format!("Confirm {}", verb.as_str()))
                    .danger()
                    .disabled(!allowed)
                    .loading(pending)
                    .on_click(cx.listener(move |this, _, _, cx| this.confirm_review(token, cx))),
            )
            .into_any_element();
        (title, body, footer, pending)
    }
    /// Rare verbs live behind "…": today only Reload.
    fn more_actions(&self, unit: &ServiceUnit, cx: &mut Context<Self>) -> AnyElement {
        let (name, scope) = (unit.name.clone(), unit.scope);
        let weak = cx.weak_entity();
        crate::controls::icon_button("service-more")
            .ghost()
            .icon(IconName::Ellipsis)
            .accessibility_label(format!(
                "More actions for {}",
                domain::short_name(&unit.name)
            ))
            .dropdown_menu(move |menu, _, cx| {
                let enabled = weak.upgrade().is_some_and(|panel| {
                    panel
                        .read(cx)
                        .client
                        .can_action(&name, scope, ServiceVerb::Reload)
                });
                let (weak, name) = (weak.clone(), name.clone());
                menu.item(
                    PopupMenuItem::new("Reload configuration")
                        .icon(IconName::RotateCw)
                        .disabled(!enabled)
                        .on_click(move |_, _, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                this.start_action(name.clone(), scope, ServiceVerb::Reload, cx)
                            });
                        }),
                )
            })
            .into_any_element()
    }
    fn unit_tabs(
        &self,
        name: &str,
        has_logs: bool,
        fragment: Option<&str>,
        drop_ins: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = if has_logs { self.tab } else { 1 };
        // Measured, since the list beside the detail pane is resizable. Until
        // the first layout, assume the default list width.
        let pane = f32::from(self.detail_scroll.bounds().size.width);
        let detail_width = if pane > 0. {
            pane - 32.
        } else if self.width <= 600. {
            self.width - 32.
        } else {
            self.width - 312.
        };
        let stacked = detail_width <= 720.;
        let choices: &[(usize, &str)] = if has_logs {
            &[(0, "Logs"), (1, "Unit file")]
        } else {
            &[(1, "Unit file")]
        };
        let tabs = choices.iter().map(|&(index, label)| {
            Tab::new()
                .label(label)
                .track_focus(
                    &self.detail_tab_focus[index]
                        .clone()
                        .tab_stop(index == active),
                )
                .focus_visible(|style| crate::controls::inset_focus_ring(style, cx))
                .on_key_down(cx.listener(move |this, event, window, cx| {
                    this.detail_tab_key(index, has_logs, event, window, cx)
                }))
        });
        div()
            .flex_1()
            .min_h(px(300.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .id("service-detail-bar")
                    .debug_selector(|| "service-detail-bar".into())
                    .flex()
                    .flex_shrink_0()
                    .min_h(px(if stacked {
                        36. + if detail_width < 320. {
                            100.
                        } else if detail_width < 540. {
                            64.
                        } else {
                            28.
                        }
                    } else {
                        35.
                    }))
                    .items_end()
                    .when(stacked, |v| v.flex_col().items_stretch())
                    .gap_x(px(12.))
                    .gap_y(px(8.))
                    .when(!stacked, |v| v.border_b_1().border_color(cx.theme().border))
                    .child(
                        crate::controls::tabs("service-detail-tabs")
                            .underline()
                            .when(stacked, |v| v.w_full())
                            .selected_index(if has_logs { active } else { 0 })
                            .children(tabs)
                            .on_click(cx.listener(move |this, index, window, cx| {
                                this.activate_detail_tab(
                                    if has_logs { *index } else { 1 },
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        div()
                            .id("service-detail-controls")
                            .debug_selector(|| "service-detail-controls".into())
                            .min_w_0()
                            .flex()
                            .items_center()
                            .justify_end()
                            .min_h(px(if stacked && detail_width < 320. {
                                100.
                            } else if stacked && detail_width < 540. {
                                64.
                            } else {
                                28.
                            }))
                            .when(!stacked, |v| v.flex_1().pb(px(6.)))
                            .when(stacked, |v| v.w_full().flex_shrink_0())
                            .when(active == 0, |v| v.child(self.log_toolbar(cx)))
                            .when_some(
                                fragment
                                    .filter(|path| !path.is_empty())
                                    .filter(|_| active == 1),
                                |v, path| {
                                    let full_path = path.to_string();
                                    v.child(
                                        quiet(
                                            format!(
                                                "{}{}",
                                                path,
                                                if drop_ins > 0 {
                                                    format!(
                                                        " + {drop_ins} drop-in{}",
                                                        if drop_ins == 1 { "" } else { "s" }
                                                    )
                                                } else {
                                                    String::new()
                                                }
                                            ),
                                            cx,
                                        )
                                        .id("service-unit-path")
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .min_w_0()
                                        .text_right()
                                        .truncate()
                                        .hover_path("Unit file", full_path.clone()),
                                    )
                                },
                            ),
                    ),
            )
            .child(if active == 0 {
                self.log_view(&format!("{name} logs"), cx)
            } else {
                self.unit_file(name, cx)
            })
            .into_any_element()
    }
    fn activate_detail_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.detail_tab_focus[index].focus(window, cx);
        if self.tab != index {
            self.tab = index;
            self.sync_selection(cx);
            cx.notify();
        }
    }
    fn detail_tab_key(
        &mut self,
        index: usize,
        has_logs: bool,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.modifiers.modified()
            || window.has_active_dialog(cx)
            || !self.detail_tab_focus[index].is_focused(window)
        {
            return;
        }
        let next = match event.keystroke.key.as_str() {
            "left" | "right" => {
                if has_logs {
                    1 - index
                } else {
                    1
                }
            }
            "home" => usize::from(!has_logs),
            "end" => 1,
            "enter" | "space" => index,
            _ => return,
        };
        self.activate_detail_tab(next, window, cx);
        window.prevent_default();
        cx.stop_propagation();
    }
    fn unit_file(&self, name: &str, cx: &mut Context<Self>) -> AnyElement {
        let loading = self.client.unit_file().is_none() && self.error("unit-file").is_none();
        div()
            .flex_1()
            .min_h(px(280.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .when_some(self.error("unit-file"), |v, error| {
                v.child(alert("service-file-error", error, cx)).child(
                    crate::controls::button("service-file-retry")
                        .label("Try again")
                        .accessibility_label("Try reading the unit file again")
                        .disabled(self.pending("unit-file"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.client.retry_unit_file();
                            cx.notify();
                        })),
                )
            })
            .when(loading, |v| v.child(loading::lines("unit-file", 12)))
            .when(!loading && self.client.unit_file().is_some(), |v| {
                v.child(
                    Editor::new(&self.logs.editor)
                        .readonly(true)
                        .aria_label(format!("{name} unit file"))
                        .h_full()
                        .text_size(px(crate::ui::FONT_SIZE)),
                )
            })
            .into_any_element()
    }
}
/// A detail pane's title: the shared heading.
fn title(value: impl Into<SharedString>) -> Stateful<Div> {
    let value = value.into();
    crate::controls::detail_title(
        SharedString::from(format!("services-heading-{value}")),
        value,
    )
    .debug_selector(|| "services-detail-heading".into())
}
fn fact(label: &str, value: String, detail: Option<String>, cx: &App) -> Stateful<Div> {
    let accessible = format!(
        "{label}: {value}{}",
        detail
            .as_ref()
            .map(|d| format!(", {d}"))
            .unwrap_or_default()
    );
    div()
        .id(SharedString::from(format!("service-fact-{label}")))
        .role(accesskit::Role::Group)
        .aria_label(accessible)
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(quiet(label.to_owned(), cx).text_size(px(crate::ui::FONT_SIZE)))
        .child(div().text_size(px(crate::ui::FONT_SIZE)).child(value))
        .when_some(detail, |v, text| {
            v.child(quiet(text, cx).text_size(px(crate::ui::FONT_SIZE)))
        })
}
fn date(at: i64) -> String {
    crate::localized_time::detail_date(at)
}
fn progress(verb: ServiceVerb) -> &'static str {
    match verb {
        ServiceVerb::Start => "Starting…",
        ServiceVerb::Stop => "Stopping…",
        ServiceVerb::Restart => "Restarting…",
        ServiceVerb::Reload => "Reloading…",
        ServiceVerb::Enable => "Enabling at boot…",
        ServiceVerb::Disable => "Disabling at boot…",
    }
}
fn detail_skeleton() -> AnyElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(Skeleton::new().w(px(220.)).h(px(18.)))
        .child(Skeleton::new().w(px(160.)).h(px(10.)))
        .child(Skeleton::new().w(px(260.)).h(px(11.)))
        .child(
            div()
                .flex()
                .gap(px(6.))
                .child(Skeleton::new().w(px(84.)).h(px(28.)))
                .child(Skeleton::new().w(px(64.)).h(px(28.))),
        )
        .child(
            div()
                .flex()
                .gap(px(28.))
                .py(px(10.))
                .children((0..3).map(|_| {
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(Skeleton::new().w(px(50.)).h(px(10.)))
                        .child(Skeleton::new().w(px(80.)).h(px(14.)))
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
                .children((0..8).map(|i| {
                    Skeleton::new()
                        .h(px(10.))
                        .w(relative([0.72, 0.55, 0.85, 0.63][i % 4]))
                })),
        )
        .into_any_element()
}
