//! Newport's own notifications on a language server stream. The agent's
//! worker sends them between whole server messages; they are not part of the
//! Language Server Protocol, so the agent and the desktop share these types.
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

/// A worker notification and the method it is sent as.
pub trait Notification: Serialize + DeserializeOwned {
    const METHOD: &'static str;
}

/// How far the worker got preparing the server's environment. `path` is the
/// `.envrc` that applies to the worktree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Environment {
    /// No `.envrc` applies, so the server starts in the login environment.
    #[serde(rename = "none")]
    NoEnvrc,
    /// The user chose to start without the `.envrc`.
    Skipped {
        path: String,
    },
    /// An `.envrc` applies, but the host has no direnv to load it.
    Unavailable {
        path: String,
    },
    /// direnv hasn't allowed the `.envrc`.
    Blocked {
        path: String,
    },
    /// direnv has denied the `.envrc`.
    Denied {
        path: String,
    },
    Loading {
        path: String,
    },
    Loaded {
        path: String,
    },
    /// direnv couldn't check or load the `.envrc`; `message` is what it said.
    Failed {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// The loaded `.envrc`, or a file it watches, changed.
    Changed,
}
impl Notification for Environment {
    const METHOD: &'static str = "$/newport/environment";
}

/// The server process. `server` is the id the desktop asked for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Server {
    /// None of the server's commands is on the environment's `PATH`.
    Missing {
        server: String,
    },
    Started {
        server: String,
        command: String,
    },
    /// The process ended, or couldn't start, which only `error` reports.
    Exited {
        server: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signal: Option<i32>,
        /// The end of the server's standard error.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stderr: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}
impl Notification for Server {
    const METHOD: &'static str = "$/newport/server";
}

/// What the server starts with, sent before it starts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Configuration {
    /// The server's settings: the answer to `workspace/configuration` and the
    /// `settings` of `workspace/didChangeConfiguration`.
    pub settings: Value,
    /// The `initializationOptions` of `initialize`.
    pub initialization_options: Value,
    /// Why the worktree's editor settings couldn't be read, in which case the
    /// server starts with its defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings_error: Option<String>,
    /// Whether the worker watches the worktree and sends [`FilesChanged`].
    pub file_watch: bool,
    /// Why the worktree couldn't be watched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_error: Option<String>,
}
impl Notification for Configuration {
    const METHOD: &'static str = "$/newport/configuration";
}

/// Files that changed in the worktree since the last batch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesChanged {
    pub changes: Vec<FileChange>,
}
impl Notification for FilesChanged {
    const METHOD: &'static str = "$/newport/filesChanged";
}

/// One changed file; `uri` is a `file:` URI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub uri: String,
    #[serde(rename = "type")]
    pub kind: FileChangeKind,
}

/// Numbered as the Language Server Protocol's `FileChangeType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub enum FileChangeKind {
    Created,
    Changed,
    Deleted,
}
impl From<FileChangeKind> for u8 {
    fn from(kind: FileChangeKind) -> Self {
        match kind {
            FileChangeKind::Created => 1,
            FileChangeKind::Changed => 2,
            FileChangeKind::Deleted => 3,
        }
    }
}
impl TryFrom<u8> for FileChangeKind {
    type Error = String;
    fn try_from(kind: u8) -> Result<Self, String> {
        match kind {
            1 => Ok(Self::Created),
            2 => Ok(Self::Changed),
            3 => Ok(Self::Deleted),
            _ => Err(format!("unknown file change type {kind}")),
        }
    }
}

/// Watching the worktree failed or lost events, so the server may have missed
/// changes. `error` says what happened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchError {
    pub error: String,
}
impl Notification for WatchError {
    const METHOD: &'static str = "$/newport/watchError";
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn round_trip<N: Notification + PartialEq + std::fmt::Debug>(value: N, expected: Value) {
        assert_eq!(serde_json::to_value(&value).unwrap(), expected);
        assert_eq!(serde_json::from_value::<N>(expected).unwrap(), value);
    }

    #[test]
    fn environment_states_keep_their_json() {
        assert_eq!(Environment::METHOD, "$/newport/environment");
        let path = || "/w/.envrc".to_owned();
        round_trip(Environment::NoEnvrc, json!({"state": "none"}));
        round_trip(Environment::Changed, json!({"state": "changed"}));
        for (value, state) in [
            (Environment::Skipped { path: path() }, "skipped"),
            (Environment::Unavailable { path: path() }, "unavailable"),
            (Environment::Blocked { path: path() }, "blocked"),
            (Environment::Denied { path: path() }, "denied"),
            (Environment::Loading { path: path() }, "loading"),
            (Environment::Loaded { path: path() }, "loaded"),
        ] {
            round_trip(value, json!({"state": state, "path": "/w/.envrc"}));
        }
        round_trip(
            Environment::Failed {
                path: path(),
                message: Some("direnv: bad".into()),
            },
            json!({"state": "failed", "path": "/w/.envrc", "message": "direnv: bad"}),
        );
        round_trip(
            Environment::Failed {
                path: path(),
                message: None,
            },
            json!({"state": "failed", "path": "/w/.envrc"}),
        );
    }

    #[test]
    fn server_states_keep_their_json() {
        assert_eq!(Server::METHOD, "$/newport/server");
        round_trip(
            Server::Missing {
                server: "gopls".into(),
            },
            json!({"state": "missing", "server": "gopls"}),
        );
        round_trip(
            Server::Started {
                server: "gopls".into(),
                command: "/bin/gopls".into(),
            },
            json!({"state": "started", "server": "gopls", "command": "/bin/gopls"}),
        );
        round_trip(
            Server::Exited {
                server: "gopls".into(),
                status: Some(1),
                signal: Some(9),
                stderr: Some("panic\n".into()),
                error: Some("bad frame".into()),
            },
            json!({"state": "exited", "server": "gopls", "status": 1, "signal": 9,
                "stderr": "panic\n", "error": "bad frame"}),
        );
        round_trip(
            Server::Exited {
                server: "gopls".into(),
                status: None,
                signal: None,
                stderr: None,
                error: Some("couldn't start".into()),
            },
            json!({"state": "exited", "server": "gopls", "error": "couldn't start"}),
        );
        assert_eq!(
            serde_json::from_value::<Server>(json!({"state": "exited", "server": "gopls",
                "status": null, "signal": null, "stderr": "", "error": null}))
            .unwrap(),
            Server::Exited {
                server: "gopls".into(),
                status: None,
                signal: None,
                stderr: Some(String::new()),
                error: None,
            }
        );
    }

    #[test]
    fn configuration_keeps_its_json() {
        assert_eq!(Configuration::METHOD, "$/newport/configuration");
        round_trip(
            Configuration {
                settings: json!({"rust-analyzer": {"check": {"command": "clippy"}}}),
                initialization_options: json!({"files": {"watcher": "server"}}),
                settings_error: Some("bad settings".into()),
                file_watch: false,
                watch_error: Some("too many watches".into()),
            },
            json!({"settings": {"rust-analyzer": {"check": {"command": "clippy"}}},
                "initializationOptions": {"files": {"watcher": "server"}},
                "settingsError": "bad settings", "fileWatch": false,
                "watchError": "too many watches"}),
        );
        round_trip(
            Configuration {
                settings: json!({}),
                initialization_options: json!({}),
                settings_error: None,
                file_watch: true,
                watch_error: None,
            },
            json!({"settings": {}, "initializationOptions": {}, "fileWatch": true}),
        );
    }

    #[test]
    fn file_changes_keep_their_json() {
        assert_eq!(FilesChanged::METHOD, "$/newport/filesChanged");
        let change = |uri: &str, kind| FileChange {
            uri: uri.into(),
            kind,
        };
        round_trip(
            FilesChanged {
                changes: vec![
                    change("file:///w/a", FileChangeKind::Created),
                    change("file:///w/b", FileChangeKind::Changed),
                    change("file:///w/c", FileChangeKind::Deleted),
                ],
            },
            json!({"changes": [
                {"uri": "file:///w/a", "type": 1},
                {"uri": "file:///w/b", "type": 2},
                {"uri": "file:///w/c", "type": 3},
            ]}),
        );
        assert_eq!(WatchError::METHOD, "$/newport/watchError");
        round_trip(
            WatchError {
                error: "File watching failed".into(),
            },
            json!({"error": "File watching failed"}),
        );
    }

    #[test]
    fn malformed_notifications_are_errors() {
        assert!(serde_json::from_value::<Environment>(json!({"state": "paused"})).is_err());
        assert!(serde_json::from_value::<Server>(json!({"state": "paused"})).is_err());
        for kind in [0, 4] {
            let changes = json!({"changes": [{"uri": "file:///w/a", "type": kind}]});
            assert!(serde_json::from_value::<FilesChanged>(changes).is_err());
        }
        assert!(serde_json::from_value::<WatchError>(json!({})).is_err());
        assert!(
            serde_json::from_value::<Configuration>(json!({"settings": {},
                "initializationOptions": {}}))
            .is_err()
        );
    }
}
