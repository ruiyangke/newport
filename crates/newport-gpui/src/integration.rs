//! Native counterpart of IntegrationPanel, backed by the shared snapshot/commands.
use crate::*;

#[path = "integration_shell.rs"]
mod shell;
#[path = "integration_view.rs"]
mod view;

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
        .text_size(px(crate::ui::FONT_SIZE))
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
        return "A signed update is available. Newport installs it automatically while connected, or you can update now.";
    }
    match state["buildRelation"].as_str() {
        Some("bundled") => {
            "Needle matches this Newport build. Newer signed versions update automatically on connection and are checked hourly while connected."
        }
        Some("different") => "A different Needle build is installed. Its release order is unknown.",
        Some("newer") => {
            "Needle is newer than this Newport release. Update Newport before replacing it."
        }
        Some("sameRelease") => "A different Needle build from the same release is installed.",
        Some("missing") => "Needle isn’t set up on this server.",
        _ => "Needle hasn’t been checked. Newport checks when connected.",
    }
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
                .get(&crate::integration_needle::key(server))
                .is_some_and(|r| r.pending)
    }

    pub(super) fn integration_installing(&self, server: &str) -> bool {
        matches!(
            self.snapshot["runtime"]["agentInstallation"][server]["stage"].as_str(),
            Some(
                "preparing"
                    | "uploading"
                    | "installing"
                    | "verifying"
                    | "activating"
                    | "uninstalling"
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
            crate::terminal_scope::connection(current, &self.snapshot) == *expected
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
            crate::terminal_scope::connection(self.selected_server().unwrap(), &self.snapshot);
        if let Some((feature @ ("browser" | "clipboard"), false)) = feature {
            self.request(
                &disable_key(server, feature),
                "set_integration_enabled",
                json!({"id":server,"feature":feature,"enabled":false,"expectedConnection":expected_connection}),
            );
            cx.notify();
            return;
        }
        if self.integration_busy(server) || self.integration_installing(server) {
            return;
        }
        let (method, args) = match feature {
            Some((feature, enabled)) => (
                "set_integration_enabled",
                json!({"id":server,"feature":feature,"enabled":enabled,"expectedConnection":expected_connection}),
            ),
            None => ("reinstall_agent", json!({"id":server})),
        };
        self.request(&action_key(server, feature.is_none()), method, args);
        cx.notify();
    }

    pub(super) fn integration_completed(&mut self, key: &str, server: &str) {
        if server == self.server
            && let Some(resource) = self.resources.get(key)
        {
            self.notification = Some(match &resource.error {
                Some(error) => Notification::error(error.clone()),
                None => Notification::success(if key == crate::integration_needle::key(server) {
                    resource.value.as_str().unwrap_or("Needle uninstalled")
                } else if key == action_key(server, true) {
                    resource.value.as_str().unwrap_or("Needle installed")
                } else {
                    "Integration preference saved"
                }),
            });
        }
        self.integration_refresh_pending.insert(server.to_owned());
        // An in-flight snapshot may have been captured before this mutation.
        self.resources.fence("snapshot");
        self.request("snapshot", "snapshot", json!({}));
    }
}

#[cfg(test)]
mod tests {
    use super::{connection_label, feature_status, needle_status};
    use serde_json::{Value, json};
    #[test]
    fn needle_status_does_not_confuse_unknown_different_or_missing() {
        assert!(needle_status(&Value::Null).contains("hasn’t been checked"));
        assert!(
            needle_status(&json!({"buildRelation":"different", "installed":true}))
                .contains("release order is unknown")
        );
        assert!(
            needle_status(&json!({"buildRelation":"missing", "installed":false}))
                .contains("isn’t set up")
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
