//! Remote/configuration write planning.
//! UI drafts stay with their form; these rules never read display URLs back into
//! configuration or silently replace a captured configuration token.
use newport_protocol::git::{Action, Remote, Reply, RepositoryConfiguration, Request};

impl crate::backend::Backend {
    /// Validate the settings read before a UI caches it as successfully loaded.
    /// Keep protocol errors intact so retry/recovery can distinguish their codes.
    pub fn repository_settings(
        &self,
        server: &str,
        repo: &str,
    ) -> Result<Reply, crate::request_error::RequestError> {
        let server = uuid::Uuid::parse_str(server).map_err(|error| error.to_string())?;
        let reply = self.git(
            server,
            Request::Remotes {
                repo_id: repo.into(),
            },
            false,
        )?;
        match &reply {
            Reply::Remotes(remotes) if valid_configuration(&remotes.configuration) => Ok(reply),
            _ => Err("Repository settings could not be read. Retry settings.".into()),
        }
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
/// remote must not retarget an existing draft. Adding a remote reviews no
/// existing configuration, so it accepts `Some(None)`.
pub fn reviewed_remote(
    kind: RemoteEdit,
    captured: Option<&Remote>,
    detail: Option<&Remote>,
) -> Option<Option<Remote>> {
    if kind == RemoteEdit::Add {
        return Some(None);
    }
    let captured = captured.filter(|captured| !captured.name.is_empty())?;
    detail
        .filter(|detail| detail.name == captured.name && !detail.token.is_empty())
        .map(|detail| Some(detail.clone()))
}

pub fn remote_action(
    kind: RemoteEdit,
    remote: Option<&Remote>,
    name: &str,
    url: &str,
) -> Option<Action> {
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
        return Some(Action::RemoteAdd {
            name: name.into(),
            url: url.into(),
        });
    }
    let remote = remote.filter(|remote| !remote.name.is_empty() && !remote.token.is_empty())?;
    let target = remote.name.clone();
    let token = remote.token.clone();
    Some(match kind {
        RemoteEdit::Rename => Action::RemoteRename {
            name: target,
            new_name: name.into(),
            expected_token: token,
        },
        RemoteEdit::Url => Action::RemoteSetUrl {
            name: target,
            url: url.into(),
            expected_token: token,
        },
        RemoteEdit::Remove => Action::RemoteRemove {
            name: target,
            expected_token: token,
        },
        RemoteEdit::Add => return None,
    })
}

/// A configuration read can be reviewed only with its write token.
pub fn valid_configuration(config: &RepositoryConfiguration) -> bool {
    !config.token.is_empty()
}

/// Send only fields changed by the user, against the explicitly reviewed token.
/// Unedited fields must not overwrite concurrent changes discovered on refresh.
pub fn settings_action(
    initial: &RepositoryConfiguration,
    reviewed: &RepositoryConfiguration,
    name: &str,
    email: &str,
    prune: bool,
) -> Option<Action> {
    if !valid_configuration(initial) || !valid_configuration(reviewed) {
        return None;
    }
    let changed = |initial: &Option<String>, value: &str| -> Option<Option<String>> {
        let value = value.trim();
        if value == initial.as_deref().unwrap_or_default() {
            return Some(None);
        }
        if value.is_empty()
            || value.len() > 4096
            || value.starts_with('-')
            || value.chars().any(char::is_control)
        {
            return None;
        }
        Some(Some(value.into()))
    };
    let user_name = changed(&initial.user_name, name)?;
    let user_email = changed(&initial.user_email, email)?;
    let fetch_prune = (initial.fetch_prune != prune).then_some(prune);
    (user_name.is_some() || user_email.is_some() || fetch_prune.is_some()).then(|| {
        Action::ConfigSet {
            expected_token: reviewed.token.clone(),
            user_name,
            user_email,
            fetch_prune,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{Remote, RemoteEdit as Kind, reviewed_remote};
    fn remote(name: &str, token: &str) -> Remote {
        Remote {
            name: name.into(),
            url: None,
            push_url: None,
            token: token.into(),
        }
    }
    fn action(
        kind: Kind,
        remote: Option<&Remote>,
        name: &str,
        url: &str,
    ) -> Option<serde_json::Value> {
        super::remote_action(kind, remote, name, url)
            .map(|value| serde_json::to_value(value).unwrap())
    }
    use serde_json::json;
    #[test]
    fn remote_review_rejects_missing_identity_and_never_submits_display_urls() {
        assert!(reviewed_remote(Kind::Remove, None, Some(&remote("", "token"))).is_none());
        let mut captured = remote("origin", "captured");
        captured.url = Some("https://[redacted]@host/repo".into());
        assert!(action(Kind::Url, Some(&captured), "", "").is_none());
        assert_eq!(
            action(Kind::Url, Some(&captured), "", " git@host:团队/repo.git ").unwrap()["url"],
            "git@host:团队/repo.git"
        );
        for name in ["", "-option", "two names", "bad\nname"] {
            assert!(action(Kind::Add, None, name, "/repo").is_none());
        }
        assert!(action(Kind::Add, None, "origin", "/repo\0extra").is_none());
        assert!(action(Kind::Add, None, "origin", "/repo\nextra").is_none());
        assert_eq!(
            action(Kind::Add, None, " upstream ", "/path with spaces/repo").unwrap()["url"],
            "/path with spaces/repo"
        );
        assert_eq!(reviewed_remote(Kind::Add, None, None), Some(None));
    }
    #[test]
    fn refreshed_review_keeps_original_remote_identity() {
        let captured = remote("upstream", "old");
        let mut page = remote("upstream", "new");
        page.url = Some("/new".into());
        let current = reviewed_remote(Kind::Url, Some(&captured), Some(&page))
            .unwrap()
            .unwrap();
        assert_eq!(current.token, "new");
        assert_eq!(
            action(Kind::Url, Some(&current), "", "/draft").unwrap()["expectedToken"],
            "new"
        );
        assert!(
            reviewed_remote(
                Kind::Remove,
                Some(&captured),
                Some(&remote("origin", "wrong"))
            )
            .is_none()
        );
        assert!(
            reviewed_remote(Kind::Rename, Some(&captured), Some(&remote("upstream", ""))).is_none()
        );
    }
    #[test]
    fn remote_edits_require_reviewed_identity_and_keep_exact_target() {
        let captured = remote("upstream", "captured-config");
        assert_eq!(
            action(Kind::Rename, Some(&captured), "renamed", ""),
            Some(
                json!({"kind":"remote.rename","name":"upstream","newName":"renamed","expectedToken":"captured-config"})
            )
        );
        assert_eq!(
            action(Kind::Url, Some(&captured), "other", "/server/repository").unwrap()["name"],
            "upstream"
        );
        assert!(action(Kind::Remove, Some(&remote("upstream", "")), "", "").is_none());
        assert!(action(Kind::Remove, None, "", "").is_none());
        assert!(action(Kind::Add, None, "-bad", "url").is_none());
        assert!(action(Kind::Url, Some(&captured), "", "").is_none());
        assert!(action(Kind::Add, None, "origin", "/server/repository").is_some());
    }
}

#[cfg(test)]
mod settings_tests {
    use super::RepositoryConfiguration;
    fn action(
        initial: &RepositoryConfiguration,
        reviewed: &RepositoryConfiguration,
        name: &str,
        email: &str,
        prune: bool,
    ) -> Option<serde_json::Value> {
        super::settings_action(initial, reviewed, name, email, prune).map(|value| {
            let mut value = serde_json::to_value(value).unwrap();
            value
                .as_object_mut()
                .unwrap()
                .retain(|_, value| !value.is_null());
            value
        })
    }
    fn config(
        token: &str,
        name: Option<&str>,
        email: Option<&str>,
        prune: bool,
    ) -> RepositoryConfiguration {
        RepositoryConfiguration {
            fetch_prune: prune,
            user_name: name.map(Into::into),
            user_email: email.map(Into::into),
            token: token.into(),
        }
    }
    use serde_json::json;
    #[test]
    fn settings_reject_unreviewable_config_and_allow_prune_without_identity() {
        let config = config("configuration", None, None, false);
        assert_eq!(
            action(&config, &config, "", "", true),
            Some(json!({"kind":"config.set","expectedToken":"configuration","fetchPrune":true}))
        );
        assert!(action(&config, &config, "", "", false).is_none());
        assert!(action(&config, &config, "Name\nextra", "", true).is_none());
        assert!(action(&config, &config, "Valid name", "-invalid", true).is_none());
        assert!(action(&config, &config, &"x".repeat(4097), "", true).is_none());
        let mut tokenless = config.clone();
        tokenless.token.clear();
        assert!(action(&tokenless, &config, "", "", true).is_none());
        assert!(action(&config, &tokenless, "", "", true).is_none());
    }
    #[test]
    fn settings_write_only_user_changes_after_review() {
        let original = config("old", Some("Original"), Some("old@example.test"), false);
        let current = config("new", Some("External"), Some("new@example.test"), false);
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
        let unread = config("", None, None, false);
        assert!(action(&original, &unread, "Edited", "old@example.test", true).is_none());
    }
}
