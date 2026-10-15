//! Needle management remains in the existing Integration surface. Confirmed
//! requests retain their identity; navigation observes backend lifecycle state.
use crate::integration::{description, unfinished_operation};
use crate::*;

#[cfg(test)]
#[path = "integration_removal_tests.rs"]
mod tests;

#[derive(Clone)]
pub(super) struct Review {
    server: Value,
    scope: Value,
    build: String,
    id: String,
    action: Option<bool>, // false = removal, true = restoration
}

pub(super) fn key(server: &str) -> String {
    format!("integration:removal:{server}")
}

pub(super) fn operation(state: &Value) -> Option<&Value> {
    state["removals"]
        .as_array()?
        .iter()
        .rev()
        .find(|r| r["phase"] != "restored" && r["operationId"].is_string())
}

fn valid_build(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Newport {
    fn removal_scope_matches(&self, review: &Review) -> bool {
        self.selected_server().is_some_and(|s| {
            s["id"] == review.server["id"]
                && newport_domain::connection_scope::connection(&s, &self.snapshot) == review.scope
        })
    }

    fn review_needle_removal(&mut self, cx: &mut Context<Self>) {
        if !self.configuration_available()
            || self.server_form.is_some()
            || self.integration_busy(&self.server)
        {
            return;
        }
        let Some(server) = self.selected_server() else {
            return;
        };
        let state = &self.snapshot["runtime"]["agentOperations"][&self.server];
        if state["error"].is_string()
            || unfinished_operation(state).is_some()
            || crate::integration_helpers::pending(state).is_some()
            || operation(state).is_some()
        {
            return;
        }
        let status = &self.snapshot["runtime"]["agentStatus"][&self.server];
        let Some(build) = status["installedHash"].as_str().filter(|b| valid_build(b)) else {
            return;
        };
        if status["installed"] != true || status["error"].is_string() {
            return;
        }
        self.needle_removal_reviews.insert(
            self.server.clone(),
            Review {
                scope: newport_domain::connection_scope::connection(&server, &self.snapshot),
                server,
                build: build.into(),
                id: uuid::Uuid::new_v4().to_string(),
                action: None,
            },
        );
        cx.notify();
    }

    fn request_needle_removal(
        &mut self,
        mut review: Review,
        restore: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.configuration_available()
            || self.server_form.is_some()
            || self.integration_busy(&self.server)
            || !self.removal_scope_matches(&review)
        {
            return;
        }
        let state = &self.snapshot["runtime"]["agentOperations"][&self.server];
        if state["error"].is_string()
            || unfinished_operation(state).is_some()
            || crate::integration_helpers::pending(state).is_some()
        {
            return;
        }
        if let Some(record) = operation(state) {
            if record["operationId"] != review.id
                || record["build"] != review.build
                || (!restore && matches!(record["phase"].as_str(), Some("removed" | "restoring")))
            {
                return;
            }
        } else {
            if !self
                .needle_removal_reviews
                .get(&self.server)
                .is_some_and(|current| current.id == review.id && current.action == review.action)
                || restore
                || review.action.is_none() && {
                    let status = &self.snapshot["runtime"]["agentStatus"][&self.server];
                    status["installedHash"] != review.build
                        || status["installed"] != true
                        || status["error"].is_string()
                }
            {
                return;
            }
            // A completed historical ID is never a new confirmation.
            if state["removals"]
                .as_array()
                .is_some_and(|r| r.iter().any(|r| r["operationId"] == review.id))
            {
                return;
            }
        }
        review.action = Some(restore);
        let mut args = json!({"expectedConnection":review.server, "operationId":review.id});
        if !restore {
            args["build"] = json!(review.build);
        }
        self.needle_removal_reviews
            .insert(self.server.clone(), review);
        self.request(
            &key(&self.server),
            if restore {
                "restore_removed_agent"
            } else {
                "remove_agent"
            },
            args,
        );
        cx.notify();
    }

    pub(super) fn needle_management(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(server) = self.selected_server() else {
            return div().into_any_element();
        };
        let state = &self.snapshot["runtime"]["agentOperations"][&self.server];
        let status = &self.snapshot["runtime"]["agentStatus"][&self.server];
        let request = self.resources.get(&key(&self.server));
        let disabled = self.integration_busy(&self.server)
            || !self.configuration_available()
            || self.server_form.is_some();
        let mut content = div()
            .id("needle-management")
            .flex()
            .flex_col()
            .gap_2()
            .mt_3()
            .min_w_0();
        if crate::integration_helpers::pending(state).is_some() {
            return content.into_any_element();
        }
        if let Some(error) = state["error"].as_str() {
            return content
                .child(description("needle-journal-error", error, cx))
                .into_any_element();
        }
        if let Some(record) = operation(state) {
            let review = Review {
                scope: newport_domain::connection_scope::connection(&server, &self.snapshot),
                server,
                build: record["build"].as_str().unwrap_or_default().into(),
                id: record["operationId"].as_str().unwrap_or_default().into(),
                action: None,
            };
            let pending =
                request.is_some_and(|r| r.pending) || state["activeOperationId"] == review.id;
            let restoring = record["phase"] == "restoring"
                || pending
                    && self
                        .needle_removal_reviews
                        .get(&self.server)
                        .is_some_and(|r| r.id == review.id && r.action == Some(true));
            let removed = record["phase"] == "removed";
            let title = if pending {
                if restoring {
                    "Restoring Needle…"
                } else {
                    "Removing Needle…"
                }
            } else if removed {
                "Needle removed"
            } else if restoring {
                "Restore unfinished"
            } else {
                "Removal unfinished"
            };
            let details = if pending {
                "You can leave this page. Newport will keep the operation running."
            } else if removed {
                "The executable and backups are retained so you can restore Needle. Your integration preferences are saved."
            } else {
                "Resume the recorded operation, or restore Needle. Changed files are preserved when they conflict with recovery."
            };
            content = content
                .child(status_line(title, pending, cx))
                .child(description("needle-removal-details", details, cx));
            if let Some(error) = request
                .and_then(|r| r.error.as_deref())
                .or_else(|| record["error"].as_str())
            {
                content = content.child(error_line(error, cx));
            }
            let mut actions = div().flex().flex_wrap().gap_2();
            if !removed && !restoring {
                let resume = review.clone();
                actions = actions.child(
                    crate::controls::button("needle-removal-resume")
                        .label("Resume removal")
                        .disabled(disabled || pending)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.request_needle_removal(resume.clone(), false, cx)
                        })),
                );
            }
            actions = actions.child(
                crate::controls::button("needle-removal-restore")
                    .label(if restoring {
                        "Retry restore"
                    } else if removed {
                        "Restore Needle"
                    } else {
                        "Restore or cancel removal"
                    })
                    .disabled(disabled || pending)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.request_needle_removal(review.clone(), true, cx)
                    })),
            );
            if record["phase"] == "restoring" {
                let server_id = self.server.clone();
                let operation_id = record["operationId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                content = content.child(self.integration_file_review(
                    &server_id,
                    &operation_id,
                    disabled || pending,
                    cx,
                ));
                actions = actions.child(
                    crate::controls::button("needle-removal-review-files")
                        .label("Review files")
                        .disabled(disabled || pending)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.integration_review(&server_id, &operation_id, None, cx);
                        })),
                );
            }
            return content.child(actions).into_any_element();
        }

        let review = self.needle_removal_reviews.get(&self.server).filter(|r| {
            !state["removals"]
                .as_array()
                .is_some_and(|records| records.iter().any(|record| record["operationId"] == r.id))
        });
        if let Some(review) = review {
            let stale = !self.removal_scope_matches(review)
                || review.action.is_none()
                    && (status["installedHash"] != review.build
                        || status["installed"] != true
                        || status["error"].is_string());
            let title = if review.action.is_some() {
                "Needle removal requested"
            } else {
                "Remove Needle from this server?"
            };
            content = content.child(status_line(title, request.is_some_and(|r| r.pending), cx));
            let target = format!(
                "{} · {}@{}",
                text(&review.server, "name"),
                text(&review.server, "sshUser"),
                text(&review.server, "sshHost")
            );
            content = content.child(div().id("needle-removal-target").role(Role::Paragraph).test_support().aria_label(target.clone())
                .min_w_0().w_full().truncate().text_size(px(13.)).text_color(cx.theme().muted_foreground)
                .tooltip({ let target = target.clone(); move |window, cx| gpui_kit::component::tooltip::Tooltip::new(target.clone()).build(window, cx) }).child(target))
                .child(description("needle-removal-impact", "Metrics, Git, browser, and clipboard will stop. SSH terminals and port forwarding stay available.", cx))
                .child(description("needle-removal-retention", "Newport-owned shell changes will be restored. The executable and backups are kept so you can restore Needle later.", cx));
            if stale {
                content = content.child(error_line(
                    "The server or installation state changed. Cancel and review removal again.",
                    cx,
                ));
            }
            if let Some(error) = request.and_then(|r| r.error.as_deref()) {
                content = content.child(error_line(error, cx));
            }
            let mut actions = div().flex().flex_wrap().gap_2();
            if review.action.is_none() || stale {
                let id = self.server.clone();
                let op = review.id.clone();
                actions = actions.child(
                    crate::controls::button("needle-removal-cancel")
                        .label(if review.action.is_none() {
                            "Cancel"
                        } else {
                            "Dismiss"
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.server == id
                                && this.needle_removal_reviews.get(&id).is_some_and(|r| {
                                    r.id == op
                                        && (r.action.is_none() || !this.removal_scope_matches(r))
                                })
                            {
                                this.needle_removal_reviews.remove(&id);
                                cx.notify();
                            }
                        })),
                );
            }
            let confirm = review.clone();
            actions = actions.child(
                crate::controls::button("needle-removal-confirm")
                    .label(if review.action.is_some() {
                        "Retry removal"
                    } else {
                        "Remove Needle"
                    })
                    .danger()
                    .disabled(disabled || stale || unfinished_operation(state).is_some())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.request_needle_removal(confirm.clone(), false, cx)
                    })),
            );
            return content.child(actions).into_any_element();
        }

        let id = self.server.clone();
        let blocked = disabled || unfinished_operation(state).is_some();
        let actions = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .child(
                crate::controls::button("integration-reinstall")
                    .label(if status["updateAvailable"] == true {
                        "Update Needle"
                    } else {
                        "Reinstall Needle"
                    })
                    .disabled(blocked)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.integration_command(&id, None, cx)),
                    ),
            )
            .when(
                status["installed"] == true
                    && status["installedHash"].as_str().is_some_and(valid_build),
                |actions| {
                    actions.child(
                        crate::controls::button("needle-removal-review")
                            .label("Remove Needle…")
                            .disabled(blocked || status["error"].is_string())
                            .on_click(cx.listener(|this, _, _, cx| this.review_needle_removal(cx))),
                    )
                },
            );
        content.child(actions).into_any_element()
    }
}

fn status_line(title: &str, pending: bool, cx: &App) -> AnyElement {
    div()
        .id("needle-removal-state")
        .role(Role::Status)
        .test_support()
        .aria_label(title.to_owned())
        .flex()
        .items_center()
        .gap_2()
        .text_size(px(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .when(pending, |row| {
            row.child(gpui_kit::component::spinner::Spinner::new().small())
        })
        .text_color(cx.theme().foreground)
        .child(title.to_owned())
        .into_any_element()
}

fn error_line(error: &str, cx: &App) -> AnyElement {
    div()
        .id("needle-removal-error")
        .role(Role::Alert)
        .test_support()
        .aria_label(error.to_owned())
        .text_size(px(13.))
        .line_height(relative(1.6))
        .text_color(cx.theme().danger)
        .child(error.to_owned())
        .into_any_element()
}
