//! THESIS: Make recorded shell changes recoverable with visible scope and consequences.
//! OWN-WORLD: Existing GPUI theme, 28px shared controls, restrained status and inline review.
//! STORY: Inspect evidence, select one operation, confirm restoration, resolve edited files.
//! FIRST VIEWPORT: Shell changes follows Needle management; outcome and action share each row.
//! FORM: Local extension of the existing Operate surface; no new visual world or concept seed.
//! FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, and DESIGN.md
use crate::*;
use gpui_kit::ElementId;

#[cfg(test)]
#[path = "integration_helpers_tests.rs"]
mod tests;

#[derive(Clone)]
pub(super) struct Review {
    pub server: Value,
    scope: Value,
    id: String,
    build: String,
    fingerprint: String,
    token: uuid::Uuid,
    submitted: bool,
}
#[derive(Default)]
pub(super) struct State {
    review: Option<Review>,
    visible: usize,
}
impl State {
    pub(super) fn has_review(&self) -> bool {
        self.review.is_some()
    }
}
pub(super) fn pending(state: &Value) -> Option<&Value> {
    state["helperRecoveries"]
        .as_array()?
        .iter()
        .find(|r| r["phase"] == "restoring" && r["operationId"].is_string())
}
fn record<'a>(state: &'a Value, id: &str) -> Option<&'a Value> {
    state["helperRecoveries"]
        .as_array()?
        .iter()
        .find(|r| r["operationId"] == id)
}
fn row<'a>(state: &'a Value, id: &str) -> Option<&'a Value> {
    state["helperHistory"]["history"]["operations"]
        .as_array()?
        .iter()
        .find(|r| r["operationId"] == id)
}
fn selectable(state: &Value, status: &Value, row: &Value) -> bool {
    state["error"].is_null()
        && state["helperHistory"]["error"].is_null()
        && state["helperHistory"]["supported"] == true
        && status["installed"] == true
        && status["error"].is_null()
        && state["helperHistory"]["history"]["buildSha256"] == status["installedHash"]
        && row["origin"] == "standalone"
        && row["state"] != "rolledBack"
        && row["restoreBlocked"].is_null()
        && row["operationId"]
            .as_str()
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok() && record(state, id).is_none())
}
fn note(id: impl Into<ElementId>, text: impl Into<String>, alert: bool, cx: &App) -> AnyElement {
    let text = text.into();
    div()
        .id(id)
        .role(if alert { Role::Alert } else { Role::Paragraph })
        .test_support()
        .aria_label(text.clone())
        .min_w_0()
        .text_size(px(13.))
        .line_height(relative(1.6))
        .text_color(if alert {
            theme::error(cx.theme().is_dark())
        } else {
            cx.theme().muted_foreground
        })
        .child(text)
        .into_any_element()
}
fn short(id: &str) -> String {
    id.chars().take(8).collect()
}

impl Newport {
    fn helper_scope_matches(&self, review: &Review) -> bool {
        self.selected_server().is_some_and(|s| {
            s["id"] == review.server["id"]
                && crate::terminal_scope::connection(&s, &self.snapshot) == review.scope
        })
    }
    fn helper_actions_blocked(&self) -> bool {
        let state = &self.snapshot["runtime"]["agentOperations"][&self.server];
        self.integration_busy(&self.server)
            || self.server_form.is_some()
            || !self.configuration_available()
            || state["error"].is_string()
            || crate::integration::unfinished_operation(state).is_some()
            || crate::integration_removal::operation(state).is_some()
    }
    fn select_helper_restore(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.helper_actions_blocked() {
            return;
        }
        let Some(server) = self.selected_server().cloned() else {
            return;
        };
        let state = &self.snapshot["runtime"]["agentOperations"][&self.server];
        if pending(state).is_some() {
            return;
        }
        let Some(row) = row(state, id).filter(|r| {
            selectable(
                state,
                &self.snapshot["runtime"]["agentStatus"][&self.server],
                r,
            )
        }) else {
            return;
        };
        let Some(fingerprint) = row["fingerprint"].as_str() else {
            return;
        };
        let Some(build) = state["helperHistory"]["history"]["buildSha256"].as_str() else {
            return;
        };
        let review = Review {
            scope: crate::terminal_scope::connection(&server, &self.snapshot),
            server,
            id: id.into(),
            build: build.into(),
            fingerprint: fingerprint.into(),
            token: uuid::Uuid::new_v4(),
            submitted: false,
        };
        self.needle_helper_ui
            .entry(self.server.clone())
            .or_default()
            .review = Some(review);
        cx.notify();
    }
    fn submit_helper_restore(&mut self, mut review: Review, cx: &mut Context<Self>) {
        if self.helper_actions_blocked() || !self.helper_scope_matches(&review) {
            return;
        }
        let state = &self.snapshot["runtime"]["agentOperations"][&self.server];
        if let Some(record) = record(state, &review.id) {
            if record["phase"] != "restoring"
                || record["build"] != review.build
                || record["fingerprint"] != review.fingerprint
            {
                return;
            }
        } else {
            if pending(state).is_some()
                || !self
                    .needle_helper_ui
                    .get(&self.server)
                    .and_then(|s| s.review.as_ref())
                    .is_some_and(|r| r.token == review.token)
            {
                return;
            }
            if !review.submitted
                && !row(state, &review.id).is_some_and(|r| {
                    selectable(
                        state,
                        &self.snapshot["runtime"]["agentStatus"][&self.server],
                        r,
                    ) && r["fingerprint"] == review.fingerprint
                        && state["helperHistory"]["history"]["buildSha256"] == review.build
                })
            {
                return;
            }
        }
        review.submitted = true;
        self.resources
            .invalidate(&format!("integration:review:{}", self.server));
        let args = json!({"expectedConnection":review.server,"operationId":review.id,"build":review.build,"fingerprint":review.fingerprint});
        self.needle_helper_ui
            .entry(self.server.clone())
            .or_default()
            .review = Some(review);
        self.request(
            &format!("integration:helper-restore:{}", self.server),
            "restore_standalone_helpers",
            args,
        );
        cx.notify();
    }
    fn inspect_helper_history(&mut self, server: Value, scope: Value, cx: &mut Context<Self>) {
        if self.helper_actions_blocked()
            || !self.selected_server().is_some_and(|s| {
                s["id"] == server["id"]
                    && crate::terminal_scope::connection(&s, &self.snapshot) == scope
            })
        {
            return;
        }
        self.request(
            &format!("integration:helper-inspect:{}", self.server),
            "inspect_agent",
            json!({"expectedConnection":server}),
        );
        cx.notify();
    }
    pub(super) fn standalone_helper_history(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(server) = self.selected_server().cloned() else {
            return div().into_any_element();
        };
        let state = &self.snapshot["runtime"]["agentOperations"][&self.server];
        let status = &self.snapshot["runtime"]["agentStatus"][&self.server];
        let history = &state["helperHistory"];
        let ui = self.needle_helper_ui.get(&self.server);
        let blocked = self.helper_actions_blocked();
        let request = self
            .resources
            .get(&format!("integration:helper-restore:{}", self.server));
        let mut content = div()
            .id("needle-helper-history")
            .mt_4()
            .flex()
            .flex_col()
            .gap_2()
            .min_w_0()
            .child(
                div()
                    .id("needle-helper-heading")
                    .role(Role::Heading)
                    .test_support()
                    .aria_level(3)
                    .aria_label("Shell changes")
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Shell changes"),
            );
        let durable = pending(state);
        let review = durable
            .map(|r| Review {
                scope: crate::terminal_scope::connection(&server, &self.snapshot),
                server: server.clone(),
                id: text(r, "operationId"),
                build: text(r, "build"),
                fingerprint: text(r, "fingerprint"),
                token: uuid::Uuid::nil(),
                submitted: true,
            })
            .or_else(|| {
                ui.and_then(|s| s.review.clone())
                    .filter(|r| record(state, &r.id).is_none())
            });
        if let Some(review) = review {
            let running =
                request.is_some_and(|r| r.pending) || state["activeOperationId"] == review.id;
            let stale = !self.helper_scope_matches(&review)
                || !review.submitted
                    && !row(state, &review.id).is_some_and(|r| {
                        selectable(state, status, r)
                            && r["fingerprint"] == review.fingerprint
                            && history["history"]["buildSha256"] == review.build
                    });
            let title = if running {
                "Restoring shell changes…"
            } else if durable.is_some() {
                "Shell restoration unfinished"
            } else if review.submitted {
                "Restoration requested"
            } else {
                "Restore these shell changes?"
            };
            let target = format!(
                "{} · {}@{} · {}",
                text(&review.server, "name"),
                text(&review.server, "sshUser"),
                text(&review.server, "sshHost"),
                short(&review.id)
            );
            content = content.child(div().id("needle-helper-state").role(Role::Status).test_support().aria_label(title).flex().items_center().gap_2().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD)
                .when(running, |r| r.child(gpui_kit::component::spinner::Spinner::new().small())).child(title))
                .child(div().id("needle-helper-target").role(Role::Paragraph).test_support().aria_label(target.clone())
                    .min_w_0().w_full().whitespace_normal().text_size(px(13.)).text_color(cx.theme().muted_foreground)
                    .child(target))
                .child(note("needle-helper-impact", if running { "You can leave this page. Newport will keep the restoration running." } else { "Restore the files recorded by this shell setup. Changed files will need review. Shell commands may stop using Newport; Needle, metrics, and Git stay available." },false,cx));
            if stale {
                content = content.child(note("needle-helper-stale", "The server or recorded ownership changed. Dismiss this review and inspect the history again.",true,cx));
            }
            if let Some(error) = request
                .and_then(|r| r.error.as_deref())
                .or_else(|| durable.and_then(|r| r["error"].as_str()))
            {
                content = content.child(note("needle-helper-error", error, true, cx));
            }
            let mut actions = div().flex().flex_wrap().gap_2();
            if durable.is_none() && !running {
                let token = review.token;
                let id = self.server.clone();
                actions = actions.child(
                    crate::controls::button("needle-helper-cancel")
                        .label(if review.submitted || stale {
                            "Dismiss"
                        } else {
                            "Cancel"
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.server == id
                                && let Some(s) = this.needle_helper_ui.get_mut(&id)
                                && s.review.as_ref().is_some_and(|r| r.token == token)
                            {
                                s.review = None;
                                cx.notify();
                            }
                        })),
                );
            }
            if durable.is_some() {
                let id = self.server.clone();
                let op = review.id.clone();
                actions = actions.child(
                    crate::controls::button("needle-helper-review")
                        .label("Review files")
                        .disabled(blocked || running)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.integration_review(&id, &op, None, cx)
                        })),
                );
                content = content.child(self.integration_file_review(
                    &self.server,
                    &review.id,
                    blocked || running,
                    cx,
                ));
            }
            actions = actions.child(
                crate::controls::button("needle-helper-confirm")
                    .label(if review.submitted {
                        "Retry restore"
                    } else {
                        "Restore shell changes"
                    })
                    .disabled(blocked || running || stale)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.submit_helper_restore(review.clone(), cx)
                    })),
            );
            return content.child(actions).into_any_element();
        }
        let rows: Vec<_> = history["history"]["operations"]
            .as_array()
            .into_iter()
            .flatten()
            .rev()
            .collect();
        if rows.is_empty() && history["error"].is_null() && history["supported"] != false {
            return div().into_any_element();
        }
        content = content.child(note(
            "needle-helper-description",
            "Shell setup recorded outside Newport. Restore only the changes you want to undo.",
            false,
            cx,
        ));
        if let Some(error) = history["error"].as_str() {
            content = content.child(note(
                "needle-helper-history-error",
                format!("History could not be refreshed. {error}"),
                true,
                cx,
            ));
        }
        if history["supported"] == false {
            let explanation = if status["installed"] == true {
                "This Needle build does not report shell history. Update Needle to inspect recorded changes."
            } else {
                "Shell history is unavailable until a compatible Needle installation can be inspected."
            };
            content = content.child(note(
                "needle-helper-history-unsupported",
                explanation,
                false,
                cx,
            ));
        }
        let count = ui.map_or(5, |s| s.visible.max(5));
        for (index, row) in rows.iter().take(count).enumerate() {
            let Some(id) = row["operationId"].as_str() else {
                continue;
            };
            let restored = row["state"] == "rolledBack"
                || record(state, id).is_some_and(|r| r["phase"] == "restored");
            let outcome = if restored {
                "Restored"
            } else if row["origin"] == "unattributed" {
                "Ownership not recorded"
            } else {
                match row["state"].as_str() {
                    Some("applied") => "Applied",
                    Some("prepared") => "Interrupted",
                    Some("rollingBack") => "Restoration interrupted",
                    _ => "Needs review",
                }
            };
            let mut item = div()
                .py_2()
                .border_t_1()
                .border_color(cx.theme().border)
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(note(
                            ("needle-helper-summary", index),
                            format!(
                                "{} · {} files · {outcome}",
                                short(id),
                                row["fileCount"].as_u64().unwrap_or(0)
                            ),
                            false,
                            cx,
                        ))
                        .when(!restored, |item| {
                            let id = id.to_owned();
                            item.child(
                                crate::controls::button(("needle-helper-select", index))
                                    .label("Restore changes…")
                                    .accessibility_label(format!("Restore changes for {id}"))
                                    .disabled(blocked || !selectable(state, status, row))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.select_helper_restore(&id, cx)
                                    })),
                            )
                        }),
                );
            if !restored && let Some(reason) = row["restoreBlocked"].as_str() {
                item = item.child(note(("needle-helper-blocked", index), reason, false, cx));
            }
            content = content.child(item);
        }
        let mut actions = div().flex().flex_wrap().gap_2();
        if rows.len() > count {
            let id = self.server.clone();
            actions = actions.child(
                crate::controls::button("needle-helper-more")
                    .label("Show older changes")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.server == id {
                            this.needle_helper_ui.entry(id.clone()).or_default().visible =
                                count + 10;
                            cx.notify();
                        }
                    })),
            );
        }
        let scope = crate::terminal_scope::connection(&server, &self.snapshot);
        actions = actions.child(
            crate::controls::button("needle-helper-refresh")
                .label("Refresh history")
                .disabled(blocked)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.inspect_helper_history(server.clone(), scope.clone(), cx)
                })),
        );
        content.child(actions).into_any_element()
    }
}
