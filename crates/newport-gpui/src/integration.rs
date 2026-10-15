//! Native counterpart of IntegrationPanel, backed by the shared snapshot/commands.
use crate::*;
use gpui_kit::component::tooltip::Tooltip;

#[path = "integration_shell.rs"]
mod shell;
#[path = "integration_view.rs"]
mod view;

fn terminal_setup_status(operations: &Value, removed: bool) -> &'static str {
    if removed {
        return "Unavailable";
    }
    if let Some(operation) = operations["operations"].as_array().and_then(|records| {
        records
            .iter()
            .rev()
            .find(|record| record["intent"]["integrations"] == true)
    }) {
        return match operation["phase"].as_str() {
            Some("ready") if operation["error"].is_string() => "Manual setup needed",
            Some("ready") => "Setup completed",
            Some("rolledBack") => "Setup not confirmed",
            _ => "Needs attention",
        };
    }
    // Standalone history proves changes were applied, but older receipts do
    // not say whether every shell file was configured. Keep that distinction.
    let history = &operations["helperHistory"];
    if history["error"].is_null()
        && history["supported"] == true
        && history["history"]["operations"]
            .as_array()
            .is_some_and(|records| {
                records.iter().any(|record| {
                    record["state"] == "applied"
                        && record["origin"] == "standalone"
                        && !operations["helperRecoveries"]
                            .as_array()
                            .is_some_and(|recoveries| {
                                recoveries.iter().any(|recovery| {
                                    recovery["operationId"] == record["operationId"]
                                        && recovery["phase"] == "restored"
                                })
                            })
                })
            })
    {
        return "Setup recorded";
    }
    "Setup not confirmed"
}

// These durable errors describe missing prerequisites, not transient SSH errors.
// Do not offer the same failed resume command as a recovery action.
fn resume_blocker(operation: &Value) -> Option<&'static str> {
    match operation["error"].as_str()? {
        "The recorded Needle build is no longer bundled; resolve this operation before installing a different build"
        | "Recorded Needle candidate is missing; inspect this operation before proceeding" => Some(
            "This installation needs a build that is no longer available. Restore the previous state before starting a new installation. Restoring may stop agent services; edited files are preserved.",
        ),
        _ => None,
    }
}
pub(super) use shell::ShellCode;

const SHELL_CONFIGURATION: &str = "if [ -x \"$HOME/.local/bin/newport-agent\" ]; then\n  if _newport_env=$(\"$HOME/.local/bin/newport-agent\" env 2>/dev/null); then\n    eval \"$_newport_env\"\n  fi\n  unset _newport_env\nfi";

fn action_key(server: &str, reinstall: bool) -> String {
    format!(
        "integration:{}:{server}",
        if reinstall { "reinstall" } else { "feature" }
    )
}

fn disable_key(server: &str, feature: &str) -> String {
    format!("integration:disable:{feature}:{server}")
}

pub(super) fn description(id: &'static str, content: &str, cx: &App) -> AnyElement {
    div()
        .id(id)
        .role(Role::Paragraph)
        .test_support()
        .aria_label(content.to_owned())
        .text_size(px(13.))
        .line_height(relative(1.6))
        .text_color(cx.theme().muted_foreground)
        .child(content.to_owned())
        .into_any_element()
}

fn connection_label(state: &Value) -> String {
    let status = state["status"].as_str().unwrap_or("disconnected");
    match status {
        "reconnecting" if state["reconnectAttempt"].as_u64().unwrap_or(0) > 0 => {
            format!("Reconnecting · attempt {}", state["reconnectAttempt"])
        }
        "connected" => "Connected".into(),
        "connecting" => "Connecting".into(),
        "reconnecting" => "Reconnecting".into(),
        "error" => "Error".into(),
        _ => "Disconnected".into(),
    }
}
fn feature_status<'a>(
    feature: &str,
    enabled: bool,
    permissions: &'a Value,
    connected: bool,
) -> &'a str {
    if !enabled {
        return "Off";
    }
    if permissions["effective"][feature] == true {
        return "Active";
    }
    if let Some(error) = permissions["error"].as_str() {
        return error;
    }
    if connected {
        "Applying permission…"
    } else {
        "Waiting for connection"
    }
}

fn needle_status(state: &Value) -> &'static str {
    if state["error"].is_string() {
        return if state["installed"] == true {
            "Needle was detected. The latest check failed; Newport will check again while connected."
        } else {
            "Couldn’t check Needle. Newport will check again while connected."
        };
    }
    if state["updateAvailable"] == true {
        return "An update is available. Update Needle below.";
    }
    match state["buildRelation"].as_str() {
        Some("bundled") => "Needle matches this Newport build. Checked hourly while connected.",
        Some("different") => "A different Needle build is installed. Its release order is unknown.",
        Some("newer") => {
            "Needle is newer than this Newport release. Update Newport before replacing it."
        }
        Some("sameRelease") => "A different Needle build from the same release is installed.",
        Some("missing") => "Needle is not installed. Set it up from Overview.",
        _ => "Needle hasn’t been checked. Newport checks when connected.",
    }
}

pub(super) fn unfinished_operation(state: &Value) -> Option<&Value> {
    state["operations"].as_array()?.iter().find(|r| {
        r["phase"]
            .as_str()
            .is_some_and(|phase| !matches!(phase, "ready" | "rolledBack"))
            && r["intent"]["operationId"].is_string()
    })
}

impl Newport {
    pub(super) fn integration_busy(&self, server: &str) -> bool {
        self.integration_refresh_pending.contains(server)
            || ["browser", "clipboard"]
                .iter()
                .any(|feature| self.integration_disabling(server, feature))
            || self.integration_pending(server, false)
            || self.integration_pending(server, true)
            || self
                .resources
                .get(&format!("integration:recovery:{server}"))
                .is_some_and(|r| r.pending)
            || self
                .resources
                .get(&format!("integration:review:{server}"))
                .is_some_and(|r| r.pending)
            || self
                .resources
                .get(&crate::integration_removal::key(server))
                .is_some_and(|r| r.pending)
            || ["helper-restore", "helper-inspect"].iter().any(|action| {
                self.resources
                    .get(&format!("integration:{action}:{server}"))
                    .is_some_and(|r| r.pending)
            })
            || self.snapshot["runtime"]["agentOperations"][server]["activeOperationId"].is_string()
    }

    fn integration_installing(&self, server: &str) -> bool {
        matches!(
            self.snapshot["runtime"]["agentInstallation"][server]["stage"].as_str(),
            Some(
                "preparing"
                    | "uploading"
                    | "installing"
                    | "verifying"
                    | "activating"
                    | "recovering"
                    | "configuring"
                    | "removing"
                    | "restoring"
                    | "restoring-helpers"
            )
        )
    }

    fn integration_disabling(&self, server: &str, feature: &str) -> bool {
        self.resources
            .get(&disable_key(server, feature))
            .is_some_and(|r| r.pending)
    }

    fn integration_pending(&self, server: &str, reinstall: bool) -> bool {
        self.resources
            .get(&action_key(server, reinstall))
            .is_some_and(|r| r.pending)
    }

    pub(super) fn integration_scoped_command(
        &mut self,
        server: &str,
        expected: &Value,
        feature: (&str, bool),
        cx: &mut Context<Self>,
    ) {
        if self.selected_server().is_some_and(|current| {
            crate::terminal_scope::connection(&current, &self.snapshot) == *expected
        }) {
            self.integration_command(server, Some(feature), cx);
        }
    }

    pub(super) fn integration_command(
        &mut self,
        server: &str,
        feature: Option<(&str, bool)>,
        cx: &mut Context<Self>,
    ) {
        if self.server_form.is_some()
            || !self.configuration_available()
            || self.server != server
            || self.selected_server().is_none()
        {
            return;
        }
        let expected_connection =
            crate::terminal_scope::connection(&self.selected_server().unwrap(), &self.snapshot);
        if let Some((feature @ ("browser" | "clipboard"), false)) = feature {
            self.request(
                &disable_key(server, feature),
                "set_integration_enabled",
                json!({"id":server,"feature":feature,"enabled":false,"expectedConnection":expected_connection}),
            );
            cx.notify();
            return;
        }
        if self.integration_busy(server)
            || self.integration_installing(server)
            || crate::integration_removal::operation(
                &self.snapshot["runtime"]["agentOperations"][server],
            )
            .is_some()
        {
            return;
        }
        let (method, args) = match feature {
            Some((feature, enabled)) => (
                "set_integration_enabled",
                json!({"id":server,"feature":feature,"enabled":enabled,"expectedConnection":expected_connection}),
            ),
            None => {
                if crate::integration_helpers::pending(
                    &self.snapshot["runtime"]["agentOperations"][server],
                )
                .is_some()
                {
                    return;
                }
                ("reinstall_agent", json!({"id":server}))
            }
        };
        self.request(&action_key(server, feature.is_none()), method, args);
        cx.notify();
    }

    pub(super) fn integration_recovery(
        &mut self,
        server: &str,
        operation: &str,
        restore: bool,
        cx: &mut Context<Self>,
    ) {
        if self.server != server
            || self.integration_busy(server)
            || self.server_form.is_some()
            || !self.configuration_available()
        {
            return;
        }
        let current = unfinished_operation(&self.snapshot["runtime"]["agentOperations"][server]);
        if !current.is_some_and(|record| {
            record["intent"]["operationId"] == operation
                && (restore
                    || (record["phase"] != "rollingBack" && resume_blocker(record).is_none()))
        }) {
            return;
        }
        self.resources
            .invalidate(&format!("integration:review:{server}"));
        self.request(
            &format!("integration:recovery:{server}"),
            if restore {
                "recover_agent"
            } else {
                "resume_agent"
            },
            json!({"id":server,"operationId":operation}),
        );
        cx.notify();
    }

    pub(super) fn integration_completed(&mut self, key: &str, server: &str) {
        if key == format!("integration:review:{server}") {
            return;
        }
        if server == self.server
            && let Some(resource) = self.resources.get(key)
        {
            self.notification = Some(match &resource.error {
                Some(error) => Notification::error(error.clone()),
                None => Notification::success(
                    if key == format!("integration:recovery:{server}")
                        || key == format!("integration:helper-restore:{server}")
                        || key == crate::integration_removal::key(server)
                    {
                        resource
                            .value
                            .as_str()
                            .unwrap_or("Needle operation resolved")
                    } else if key == format!("integration:helper-inspect:{server}") {
                        "Needle check finished"
                    } else if key == action_key(server, true) {
                        "Agent reinstalled"
                    } else {
                        "Integration preference saved"
                    },
                ),
            });
        }
        if server == self.server
            && key == format!("integration:recovery:{server}")
            && self.resources.get(key).is_some_and(|r| r.error.is_none())
        {
            self.setup_flow = None;
        }
        self.integration_refresh_pending.insert(server.to_owned());
        // An in-flight snapshot may have been captured before this mutation.
        self.resources.invalidate("snapshot");
        self.request("snapshot", "snapshot", json!({}));
    }

    pub(super) fn integration_review(
        &mut self,
        server: &str,
        operation: &str,
        keep: Option<(u64, String)>,
        cx: &mut Context<Self>,
    ) {
        if self.server != server
            || self.integration_busy(server)
            || self.server_form.is_some()
            || !self.configuration_available()
        {
            return;
        }
        let state = &self.snapshot["runtime"]["agentOperations"][server];
        let eligible = unfinished_operation(state).is_some_and(|r| {
            r["phase"] == "rollingBack" && r["intent"]["operationId"] == operation
        }) || crate::integration_removal::operation(state)
            .is_some_and(|r| r["phase"] == "restoring" && r["operationId"] == operation)
            || crate::integration_helpers::pending(state)
                .is_some_and(|r| r["operationId"] == operation);
        if !eligible || state["error"].is_string() {
            return;
        }
        if let Some((index, fingerprint)) = &keep {
            let valid = self
                .resources
                .get(&format!("integration:review:{server}"))
                .is_some_and(|r| {
                    !r.pending
                        && r.error.is_none()
                        && r.value["operationId"] == operation
                        && r.value["files"].as_array().is_some_and(|files| {
                            files.iter().any(|f| {
                                f["index"] == *index
                                    && f["status"] == "conflict"
                                    && f["fingerprint"] == *fingerprint
                            })
                        })
                });
            if !valid {
                return;
            }
        }
        let mut args = json!({"id": server, "operationId": operation});
        let method = if let Some((index, fingerprint)) = keep {
            args["index"] = json!(index);
            args["fingerprint"] = json!(fingerprint);
            "keep_agent_file"
        } else {
            "review_agent"
        };
        self.request(&format!("integration:review:{server}"), method, args);
        cx.notify();
    }

    pub(super) fn integration_file_review(
        &self,
        server_id: &str,
        operation_id: &str,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let server_id = server_id.to_owned();
        let operation_id = operation_id.to_owned();
        let mut recovery = div().flex().flex_col().min_w_0();
        if let Some(report) = self
            .resources
            .get(&format!("integration:review:{server_id}"))
        {
            if report.pending {
                recovery = recovery.child(description(
                    "needle-review-pending",
                    "Reviewing file changes…",
                    cx,
                ));
            } else if let Some(error) = &report.error {
                recovery = recovery.child(description("needle-review-error", error, cx));
            } else if report.value["operationId"] == operation_id {
                let mut review = div().flex().flex_col().gap_3().mt_2()
                            .child(description("needle-review-help", "Keep current file leaves it untouched and excludes it from this restore. Its Newport shell configuration will not be changed. Then retry restore for the remaining changes.", cx));
                for file in report.value["files"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|f| f["status"] != "restored")
                {
                    let index = file["index"].as_u64().unwrap_or(0);
                    let path = file["path"].as_str().unwrap_or("Unknown path");
                    let message = file["message"].as_str().unwrap_or("");
                    let mut row = div()
                        .id(("needle-review-file", index))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .min_w_0()
                        .child(
                            div()
                                .id(("needle-review-path", index))
                                .w_full()
                                .min_w_0()
                                .truncate()
                                .role(Role::Paragraph)
                                .test_support()
                                .aria_label(path.to_owned())
                                .tooltip({
                                    let path = path.to_owned();
                                    move |window, cx| Tooltip::new(path.clone()).build(window, cx)
                                })
                                .text_size(px(13.))
                                .child(path.to_owned()),
                        )
                        .child(
                            div()
                                .id(("needle-review-message", index))
                                .role(Role::Paragraph)
                                .test_support()
                                .aria_label(message.to_owned())
                                .text_size(px(13.))
                                .text_color(cx.theme().muted_foreground)
                                .child(message.to_owned()),
                        );
                    if file["status"] == "conflict"
                        && let Some(fingerprint) = file["fingerprint"].as_str()
                    {
                        let id = server_id.clone();
                        let op = operation_id.clone();
                        let fingerprint = fingerprint.to_owned();
                        row = row.child(
                            div().flex().child(
                                crate::controls::button(("needle-keep-file", index))
                                    .label("Keep current file")
                                    .disabled(disabled)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.integration_review(
                                            &id,
                                            &op,
                                            Some((index, fingerprint.clone())),
                                            cx,
                                        )
                                    })),
                            ),
                        );
                    }
                    review = review.child(row);
                }
                recovery = recovery.child(review);
            }
        }
        recovery.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{connection_label, feature_status, needle_status};
    use serde_json::{Value, json};
    #[test]
    fn terminal_setup_distinguishes_completion_skipped_files_and_unverified_history() {
        use super::terminal_setup_status;
        let mut state = json!({"operations":[
            {"phase":"ready", "intent":{"integrations":true}},
            {"phase":"ready", "intent":{"integrations":false}}
        ]});
        assert_eq!(terminal_setup_status(&state, false), "Setup completed");
        assert_eq!(terminal_setup_status(&state, true), "Unavailable");
        state["operations"][0]["error"] = json!("Managed shell file skipped");
        assert_eq!(terminal_setup_status(&state, false), "Manual setup needed");
        state["operations"][0]["phase"] = json!("rolledBack");
        assert_eq!(terminal_setup_status(&state, false), "Setup not confirmed");
        assert_eq!(
            terminal_setup_status(&Value::Null, false),
            "Setup not confirmed"
        );
        let mut standalone = json!({"helperHistory":{"supported":true,"history":{"operations":[
            {"operationId":"test", "origin":"standalone", "state":"applied"}
        ]}}});
        assert_eq!(terminal_setup_status(&standalone, false), "Setup recorded");
        standalone["helperRecoveries"] = json!([{"operationId":"test", "phase":"restored"}]);
        assert_eq!(
            terminal_setup_status(&standalone, false),
            "Setup not confirmed"
        );
    }
    #[test]
    fn needle_status_does_not_confuse_unknown_different_or_missing() {
        assert!(needle_status(&Value::Null).contains("hasn’t been checked"));
        assert!(
            needle_status(&json!({"buildRelation":"different", "installed":true}))
                .contains("release order is unknown")
        );
        assert!(
            needle_status(&json!({"buildRelation":"missing", "installed":false}))
                .contains("not installed")
        );
        assert!(
            needle_status(&json!({"buildRelation":"bundled", "installed":true}))
                .contains("matches")
        );
        assert!(
            needle_status(&json!({"buildRelation":"older", "updateAvailable":true}))
                .contains("update is available")
        );
        assert!(
            needle_status(&json!({"buildRelation":"newer", "installed":true}))
                .contains("Update Newport")
        );
        assert!(
            needle_status(&json!({"buildRelation":"sameRelease", "installed":true}))
                .contains("same release")
        );
        assert!(
            needle_status(&json!({"error":"offline", "installed":true})).contains("was detected")
        );
        assert!(
            !needle_status(&json!({"error":"offline", "installed":null})).contains("not installed")
        );
    }
    #[test]
    fn saved_preference_is_not_reported_as_active_before_acknowledgement() {
        assert_eq!(
            feature_status("clipboard", true, &Value::Null, false),
            "Waiting for connection"
        );
        assert_eq!(
            feature_status("clipboard", true, &Value::Null, true),
            "Applying permission…"
        );
        let active = json!({"effective":{"clipboard":true,"browser":false}});
        assert_eq!(feature_status("clipboard", true, &active, true), "Active");
        assert_eq!(feature_status("browser", false, &active, true), "Off");
        let failed = json!({"error":"Clipboard service unavailable"});
        assert_eq!(
            feature_status("clipboard", true, &failed, true),
            "Clipboard service unavailable"
        );
    }
    #[test]
    fn connection_labels_do_not_imply_a_retry_limit() {
        assert_eq!(connection_label(&Value::Null), "Disconnected");
        assert_eq!(
            connection_label(&json!({"status":"reconnecting","reconnectAttempt":3})),
            "Reconnecting · attempt 3"
        );
        assert_eq!(
            connection_label(&json!({"status":"reconnecting","reconnectAttempt":0})),
            "Reconnecting"
        );
    }
}
