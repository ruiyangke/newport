//! Remote/configuration write planning.
//! UI drafts stay with their form; these rules never read display URLs back into
//! configuration or silently replace a captured configuration token.
use serde_json::{Value, json};

impl crate::backend::Backend {
    /// Validate the settings read before a UI caches it as successfully loaded.
    /// Keep protocol errors intact so retry/recovery can distinguish their codes.
    pub fn repository_settings(
        &self,
        server: &str,
        repo: &str,
    ) -> Result<Value, crate::request_error::RequestError> {
        let value = self.call_detailed(
            "git_request",
            json!({"serverId":server,"request":{"method":"repo.remotes","params":{"repoId":repo}}}),
        )?;
        if !valid_configuration(&value["configuration"]) {
            return Err("Repository settings could not be read. Retry settings.".into());
        }
        Ok(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteEdit {
    Add,
    Rename,
    Url,
    Remove,
}

/// Called only when the user accepts a refreshed review. A different selected
/// remote must not retarget an existing draft.
pub fn reviewed_remote(kind: RemoteEdit, captured: &Value, detail: &Value) -> Option<Value> {
    if kind == RemoteEdit::Add {
        return Some(Value::Null);
    }
    (captured["name"].as_str().is_some_and(|s| !s.is_empty())
        && detail["name"] == captured["name"]
        && detail["token"].as_str().is_some_and(|s| !s.is_empty()))
    .then(|| detail.clone())
}

pub fn remote_action(kind: RemoteEdit, remote: &Value, name: &str, url: &str) -> Option<Value> {
    let name = name.trim();
    let url = url.trim();
    let valid_name = |s: &str| {
        !s.is_empty()
            && s.len() <= 1024
            && !s.starts_with('-')
            && !s.chars().any(|c| c.is_control() || c.is_whitespace())
    };
    if matches!(kind, RemoteEdit::Add | RemoteEdit::Rename) && !valid_name(name) {
        return None;
    }
    if matches!(kind, RemoteEdit::Add | RemoteEdit::Url)
        && (url.is_empty() || url.len() > 4096 || url.chars().any(char::is_control))
    {
        return None;
    }
    if kind == RemoteEdit::Add {
        return Some(json!({"kind":"remote.add","name":name,"url":url}));
    }
    let target = remote["name"].as_str().filter(|s| !s.is_empty())?;
    let token = remote["token"].as_str().filter(|s| !s.is_empty())?;
    Some(match kind {
        RemoteEdit::Rename => {
            json!({"kind":"remote.rename","name":target,"newName":name,"expectedToken":token})
        }
        RemoteEdit::Url => {
            json!({"kind":"remote.set_url","name":target,"url":url,"expectedToken":token})
        }
        RemoteEdit::Remove => json!({"kind":"remote.remove","name":target,"expectedToken":token}),
        RemoteEdit::Add => unreachable!(),
    })
}

pub fn valid_configuration(config: &Value) -> bool {
    config["token"].as_str().is_some_and(|s| !s.is_empty())
        && config["fetchPrune"].is_boolean()
        && ["userName", "userEmail"]
            .iter()
            .all(|key| config[*key].is_null() || config[*key].is_string())
}

/// Send only fields changed by the user, against the explicitly reviewed token.
/// Unedited fields must not overwrite concurrent changes discovered on refresh.
pub fn settings_action(
    initial: &Value,
    reviewed: &Value,
    name: &str,
    email: &str,
    prune: bool,
) -> Option<Value> {
    if !valid_configuration(initial) || !valid_configuration(reviewed) {
        return None;
    }
    let mut result = json!({"kind":"config.set","expectedToken":reviewed["token"]});
    for (key, value) in [("userName", name.trim()), ("userEmail", email.trim())] {
        if value != initial[key].as_str().unwrap_or_default() {
            if value.is_empty()
                || value.len() > 4096
                || value.starts_with('-')
                || value.chars().any(char::is_control)
            {
                return None;
            }
            result[key] = json!(value);
        }
    }
    if initial["fetchPrune"] != prune {
        result["fetchPrune"] = json!(prune);
    }
    (result.as_object()?.len() > 2).then_some(result)
}

#[cfg(test)]
mod tests {
    use super::{RemoteEdit as Kind, remote_action as action, reviewed_remote};
    use serde_json::{Value, json};
    #[test]
    fn remote_review_rejects_missing_identity_and_never_submits_display_urls() {
        assert!(reviewed_remote(Kind::Remove, &Value::Null, &json!({"token":"token"})).is_none());
        let remote =
            json!({"name":"origin","token":"captured","url":"https://[redacted]@host/repo"});
        assert!(action(Kind::Url, &remote, "", "").is_none());
        assert_eq!(
            action(Kind::Url, &remote, "", " git@host:团队/repo.git ").unwrap()["url"],
            "git@host:团队/repo.git"
        );
        for name in ["", "-option", "two names", "bad\nname"] {
            assert!(action(Kind::Add, &Value::Null, name, "/repo").is_none());
        }
        assert!(action(Kind::Add, &Value::Null, "origin", "/repo\0extra").is_none());
        assert!(action(Kind::Add, &Value::Null, "origin", "/repo\nextra").is_none());
        assert_eq!(
            action(
                Kind::Add,
                &Value::Null,
                " upstream ",
                "/path with spaces/repo"
            )
            .unwrap()["url"],
            "/path with spaces/repo"
        );
    }
    #[test]
    fn refreshed_review_keeps_original_remote_identity() {
        let captured = json!({"name":"upstream","token":"old"});
        let page = json!({"name":"upstream","token":"new","url":"/new"});
        let current = reviewed_remote(Kind::Url, &captured, &page).unwrap();
        assert_eq!(current["token"], "new");
        assert_eq!(
            action(Kind::Url, &current, "", "/draft").unwrap()["expectedToken"],
            "new"
        );
        assert!(
            reviewed_remote(
                Kind::Remove,
                &captured,
                &json!({"name":"origin","token":"wrong"})
            )
            .is_none()
        );
        assert!(
            reviewed_remote(
                Kind::Rename,
                &captured,
                &json!({"name":"upstream","token":""})
            )
            .is_none()
        );
    }
    #[test]
    fn remote_edits_require_reviewed_identity_and_keep_exact_target() {
        let remote = json!({"name":"upstream","token":"captured-config"});
        assert_eq!(
            action(Kind::Rename, &remote, "renamed", ""),
            Some(
                json!({"kind":"remote.rename","name":"upstream","newName":"renamed","expectedToken":"captured-config"})
            )
        );
        assert_eq!(
            action(Kind::Url, &remote, "other", "/server/repository").unwrap()["name"],
            "upstream"
        );
        assert!(action(Kind::Remove, &json!({"name":"upstream"}), "", "").is_none());
        assert!(action(Kind::Add, &Value::Null, "-bad", "url").is_none());
        assert!(action(Kind::Url, &remote, "", "").is_none());
        assert!(action(Kind::Add, &Value::Null, "origin", "/server/repository").is_some());
    }
}

#[cfg(test)]
mod settings_tests {
    use super::settings_action as action;
    use serde_json::json;
    #[test]
    fn settings_reject_malformed_config_and_allow_prune_without_identity() {
        let config =
            json!({"token":"configuration","userName":null,"userEmail":null,"fetchPrune":false});
        assert_eq!(
            action(&config, &config, "", "", true),
            Some(json!({"kind":"config.set","expectedToken":"configuration","fetchPrune":true}))
        );
        assert!(action(&config, &config, "", "", false).is_none());
        assert!(action(&config, &config, "Name\nextra", "", true).is_none());
        assert!(action(&config, &config, "Valid name", "-invalid", true).is_none());
        assert!(action(&config, &config, &"x".repeat(4097), "", true).is_none());
        for (key, invalid) in [
            ("userName", json!([])),
            ("userEmail", json!(false)),
            ("fetchPrune", json!("false")),
            ("token", json!("")),
        ] {
            let mut malformed = config.clone();
            malformed[key] = invalid;
            assert!(action(&malformed, &config, "", "", true).is_none());
            assert!(action(&config, &malformed, "", "", true).is_none());
        }
    }
    #[test]
    fn settings_write_only_user_changes_after_review() {
        let original = json!({"token":"old","userName":"Original","userEmail":"old@example.test","fetchPrune":false});
        let current = json!({"token":"new","userName":"External","userEmail":"new@example.test","fetchPrune":false});
        assert_eq!(
            action(&original, &current, "Original", "old@example.test", true),
            Some(json!({"kind":"config.set","expectedToken":"new","fetchPrune":true}))
        );
        assert_eq!(
            action(&original, &current, "Edited", "old@example.test", false).unwrap()["userName"],
            "Edited"
        );
        assert!(action(&original, &current, "Original", "old@example.test", false).is_none());
        assert!(action(&original, &current, "", "old@example.test", true).is_none());
        assert!(action(&original, &json!({}), "Edited", "old@example.test", true).is_none());
    }
}
