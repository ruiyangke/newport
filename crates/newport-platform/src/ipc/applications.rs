//! Desktop application intent and opaque generation-scoped receipts. This is
//! local IPC data, independent of remote GUI protocol and presentation ownership.
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Runtime {
    pub instance: Uuid,
    pub build_sha256: String,
    pub transport_generation: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub runtime: Runtime,
    pub session: Uuid,
    pub worker: Uuid,
    pub display_generation: u64,
}
/// Execute literal arguments or activate an installed desktop entry. The
/// application adapter validates the target before admitting it to its queue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    Command {
        argv: Vec<String>,
    },
    Desktop {
        id: String,
        uris: Vec<String>,
        action: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "message", rename_all = "snake_case")]
pub enum Outcome {
    /// The remote launcher started. This does not promise a new window: an
    /// application may activate an existing instance or run without windows.
    Accepted,
    /// The remote side explicitly refused this request.
    Rejected(String),
    /// Delivery or completion is uncertain. Never retry automatically, even
    /// after a replacement connection becomes available.
    Unknown(String),
}
/// A launch identity belongs to one exact display, never to a process name or
/// PID. Keeping or serializing it holds no display/transport demand.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Application {
    pub display: Identity,
    pub request: NonZeroU64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Pending,
    /// The launcher is owned, but its first window has not been presented.
    Opening,
    /// It still runs, but opened no window within the remote startup budget.
    /// A window may still open.
    Background,
    Running,
    Stopping,
    /// The owned group ended; an activated existing instance may still run.
    /// Normal completion remains observable for the last 64 retired displays
    /// on this owner. Older observations expire instead of adopting a new one.
    Ended,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Launched {
    pub application: Application,
    pub outcome: Outcome,
}
/// Why an ended application had no window, when the remote side said.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum Ending {
    /// The launch failed, with what it reported.
    Failed(String),
    /// It ended without opening a window, with the end of what it reported.
    NoWindow(Option<String>),
    /// An instance of the app that was already running took the launch.
    HandedOff,
    /// Its display stopped on the server, and its windows closed with it.
    DisplayStopped,
}
/// Something the remote side said about a running application that needs no
/// action and changes nothing about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "display", rename_all = "snake_case")]
pub enum Notice {
    /// Its file dialogs open on another display of the server.
    DialogsElsewhere(Elsewhere),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Elsewhere {
    /// The server's own screen, where someone is signed in.
    ServerScreen,
    /// Another Mac using this server with the same account.
    OtherDisplay,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub application: Application,
    pub state: Option<State>,
    pub error: Option<String>,
    /// Only on an Ended state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ending: Option<Ending>,
    /// Only while it runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice: Option<Notice>,
}
impl Target {
    /// Bound and validate local intent before starting any remote work. The
    /// selected graphical adapter remains responsible for its own validation.
    pub fn validate(&self) -> Result<(), String> {
        let words: Vec<&str> = match self {
            Self::Command { argv } => {
                if argv.is_empty() || argv[0].is_empty() || argv.len() > 128 {
                    return Err("Enter a program and at most 127 arguments.".into());
                }
                argv.iter().map(String::as_str).collect()
            }
            Self::Desktop { id, uris, action } => {
                if id.is_empty() || uris.len() > 64 || action.as_ref().is_some_and(String::is_empty)
                {
                    return Err("Invalid application entry or action.".into());
                }
                std::iter::once(id.as_str())
                    .chain(uris.iter().map(String::as_str))
                    .chain(action.as_deref())
                    .collect()
            }
        };
        if words
            .iter()
            .any(|word| word.contains('\0') || word.len() > 8192)
            || words.iter().map(|word| word.len()).sum::<usize>() > 16384
        {
            return Err("Application arguments are invalid or too long.".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(ending: Option<serde_json::Value>) -> serde_json::Value {
        let mut row = json!({
            "application": {
                "display": {
                    "runtime": {
                        "instance": Uuid::nil(),
                        "build_sha256": "a",
                        "transport_generation": 1,
                    },
                    "session": Uuid::nil(),
                    "worker": Uuid::nil(),
                    "display_generation": 1,
                },
                "request": 1,
            },
            "state": "ended",
            "error": null,
        });
        if let Some(ending) = ending {
            row["ending"] = ending;
        }
        row
    }

    #[test]
    fn observations_carry_an_optional_ending() {
        let plain: Observation = serde_json::from_value(row(None)).unwrap();
        assert_eq!(plain.ending, None);
        assert!(
            serde_json::to_value(&plain)
                .unwrap()
                .get("ending")
                .is_none()
        );
        for ending in [
            Ending::Failed("boom".into()),
            Ending::NoWindow(Some("done".into())),
            Ending::NoWindow(None),
            Ending::HandedOff,
            Ending::DisplayStopped,
        ] {
            let value = serde_json::to_value(&ending).unwrap();
            let observation: Observation = serde_json::from_value(row(Some(value))).unwrap();
            assert_eq!(observation.ending, Some(ending));
        }
        assert_eq!(
            serde_json::to_value(Ending::Failed("boom".into())).unwrap(),
            json!({"kind": "failed", "message": "boom"})
        );
        assert!(
            serde_json::from_value::<Observation>(row(Some(json!({"kind": "presented"})))).is_err()
        );
    }

    #[test]
    fn observations_carry_an_optional_notice() {
        let plain: Observation = serde_json::from_value(row(None)).unwrap();
        assert_eq!(plain.notice, None);
        assert!(
            serde_json::to_value(&plain)
                .unwrap()
                .get("notice")
                .is_none()
        );
        for (notice, wire) in [
            (
                Notice::DialogsElsewhere(Elsewhere::ServerScreen),
                json!({"kind": "dialogs_elsewhere", "display": "server_screen"}),
            ),
            (
                Notice::DialogsElsewhere(Elsewhere::OtherDisplay),
                json!({"kind": "dialogs_elsewhere", "display": "other_display"}),
            ),
        ] {
            assert_eq!(serde_json::to_value(notice).unwrap(), wire);
            let mut value = row(None);
            value["state"] = json!("running");
            value["notice"] = wire;
            let observation: Observation = serde_json::from_value(value).unwrap();
            assert_eq!(observation.notice, Some(notice));
        }
        let mut unknown = row(None);
        unknown["notice"] = json!({"kind": "dialogs_elsewhere", "display": "tv"});
        assert!(serde_json::from_value::<Observation>(unknown).is_err());
    }

    #[test]
    fn observations_carry_running_without_a_window() {
        let mut background = row(None);
        background["state"] = json!("background");
        let observation: Observation = serde_json::from_value(background).unwrap();
        assert_eq!(observation.state, Some(State::Background));
        assert_eq!(
            serde_json::to_value(Ending::HandedOff).unwrap(),
            json!({"kind": "handed_off"})
        );
    }
}
