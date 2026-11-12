//! Startup results, independent of owned application groups. Ordinary
//! launch acceptance proves no presentation. No result authorizes replay.
use crate::remote_apps::v1::WindowId;
use serde::{Deserialize, Serialize};

pub const MAX_RETIRED: usize = 64;
pub const MAX_DIAGNOSTIC: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Preparing,
    ServiceLookup,
    ServiceRegistration,
    ActivationCall,
    Command,
    AwaitingWindow,
    AwaitingPresentation,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Window {
    #[serde(with = "window_id")]
    pub window: WindowId,
    pub surface: u64,
    /// The launch finished on a window of an instance already running.
    pub existing_instance: bool,
}
impl Window {
    pub fn validate(self) -> Result<(), String> {
        self.window.validate()?;
        if self.surface == 0 {
            return Err("Invalid application surface identity".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Outcome {
    Presented(Window),
    /// All owned descendants completed before an exact presentation ACK. A
    /// transient created/offered window may have existed. User-facing wording:
    /// "Finished before a window was shown"; this is never Presented. `output`
    /// is the end of what the launch wrote, if anything.
    CompletedWithoutPresentation {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    /// Another process took the launch: an instance that was already running
    /// accepted its D-Bus activation and showed no window of its own for it, or
    /// a window outside the launch's processes presented its startup token.
    /// Never Presented. A struct variant, so unknown fields are rejected.
    HandedOff {},
    Failed {
        stage: Stage,
        message: String,
        outcome_unknown: bool,
    },
    TimedOut {
        stage: Stage,
        outcome_unknown: bool,
    },
    Cancelled {
        outcome_unknown: bool,
    },
}
impl Outcome {
    pub fn validate(&self) -> Result<(), String> {
        let text = match self {
            Self::Presented(window) => return window.validate(),
            Self::Failed { message, .. } => Some(message),
            Self::CompletedWithoutPresentation { output } => output.as_ref(),
            _ => None,
        };
        if text.is_some_and(|text| {
            text.is_empty() || text.len() > MAX_DIAGNOSTIC || text.contains('\0')
        }) {
            return Err("Invalid application startup diagnostic".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum State {
    Starting {
        stage: Stage,
    },
    /// The startup budget passed while the launch's own processes still run,
    /// and none of them has a window yet. Not terminal: a window or a result
    /// may follow. Nothing is stopped for it.
    Background {
        waited_ms: u64,
    },
    Window(Window),
    Finished {
        outcome: Outcome,
    },
}
impl State {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Starting { .. } => Ok(()),
            Self::Background { waited_ms: 0 } => Err("Invalid application startup wait".into()),
            Self::Background { .. } => Ok(()),
            Self::Window(window) => window.validate(),
            Self::Finished { outcome } => outcome.validate(),
        }
    }
    pub fn window(&self) -> Option<Window> {
        match self {
            Self::Window(window)
            | Self::Finished {
                outcome: Outcome::Presented(window),
            } => Some(*window),
            _ => None,
        }
    }
    pub fn terminal(&self) -> bool {
        matches!(self, Self::Finished { .. })
    }
}

/// New strict envelope. Receipt Accepted precedes revision 1. Revisions are
/// contiguous per request; terminal results precede inventory removal and are
/// immutable. `outcome_unknown` is uncertainty about invocation/startup, never a
/// guarantee of no side effects when false. No outcome permits automatic replay.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    pub session: u64,
    pub request: u64,
    pub revision: u64,
    pub state: State,
}
impl Update {
    pub fn validate(&self) -> Result<(), String> {
        if self.session == 0 || self.request == 0 || self.revision == 0 {
            return Err("Invalid application lifecycle identity/revision".into());
        }
        self.state.validate()
    }
}
/// The host has installed complete backing for the exact offered surface. This
/// proves local application, not scanout, and does not require a fresh frame if
/// the same current backing was already installed before the offer arrived.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Presented {
    pub session: u64,
    pub request: u64,
    #[serde(with = "window_id")]
    pub window: WindowId,
    pub surface: u64,
}
impl Presented {
    pub fn validate(self) -> Result<(), String> {
        if self.session == 0 || self.request == 0 || self.surface == 0 {
            return Err("Invalid application presentation identity".into());
        }
        self.window.validate()
    }
}

// Keep the established WindowId wire shape, but reject unknown fields in these
// new strict messages without changing older messages' deserialization policy.
mod window_id {
    use super::*;
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Strict {
        id: u64,
        generation: u64,
    }
    pub fn serialize<S: serde::Serializer>(
        id: &WindowId,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        id.serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<WindowId, D::Error> {
        let Strict { id, generation } = Strict::deserialize(deserializer)?;
        Ok(WindowId { id, generation })
    }
}

#[cfg(all(test, feature = "protobuf"))]
mod tests {
    use super::*;
    use crate::remote_apps::v1::{ClientMessage, ServerMessage};
    fn window() -> Window {
        Window {
            window: WindowId {
                id: 3,
                generation: 7,
            },
            surface: 2,
            existing_instance: true,
        }
    }
    fn update(state: State) -> Update {
        Update {
            session: 9,
            request: 4,
            revision: 1,
            state,
        }
    }
    #[test]
    fn strict_new_envelopes_round_trip_and_reject_unknown_fields() {
        for state in [
            State::Starting {
                stage: Stage::Preparing,
            },
            State::Window(window()),
            State::Finished {
                outcome: Outcome::Presented(window()),
            },
            State::Finished {
                outcome: Outcome::CompletedWithoutPresentation { output: None },
            },
            State::Finished {
                outcome: Outcome::CompletedWithoutPresentation {
                    output: Some("done".into()),
                },
            },
            State::Background { waited_ms: 30_000 },
            State::Finished {
                outcome: Outcome::HandedOff {},
            },
            State::Finished {
                outcome: Outcome::TimedOut {
                    stage: Stage::Command,
                    outcome_unknown: true,
                },
            },
        ] {
            let value =
                serde_json::to_value(ServerMessage::ApplicationLifecycle(update(state))).unwrap();
            assert_eq!(
                serde_json::to_value(
                    serde_json::from_value::<ServerMessage>(value.clone()).unwrap()
                )
                .unwrap(),
                value
            );
            let mut extra = value.clone();
            extra["pid"] = 12.into();
            assert!(serde_json::from_value::<ServerMessage>(extra).is_err());
            let mut extra = value;
            extra["state"]["pid"] = 12.into();
            assert!(serde_json::from_value::<ServerMessage>(extra).is_err());
        }
        let message = ClientMessage::ApplicationPresented(Presented {
            session: 9,
            request: 4,
            window: window().window,
            surface: 2,
        });
        let value = serde_json::to_value(message).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"type":"application_presented","session":9,"request":4,"window":{"id":3,"generation":7},"surface":2})
        );
        assert!(serde_json::from_value::<ClientMessage>(value.clone()).is_ok());
        let mut extra = value.clone();
        extra["pid"] = 12.into();
        assert!(serde_json::from_value::<ClientMessage>(extra).is_err());
        let mut extra = value;
        extra["window"]["pid"] = 12.into();
        assert!(serde_json::from_value::<ClientMessage>(extra).is_err());
        let mut extra = serde_json::to_value(ServerMessage::ApplicationLifecycle(update(
            State::Window(window()),
        )))
        .unwrap();
        extra["state"]["window"]["pid"] = 12.into();
        assert!(serde_json::from_value::<ServerMessage>(extra).is_err());
    }
    #[test]
    fn identities_diagnostics_payload_and_capability_prerequisites_are_bounded() {
        let base = update(State::Starting {
            stage: Stage::Preparing,
        });
        for invalid in [
            Update {
                session: 0,
                ..base.clone()
            },
            Update {
                request: 0,
                ..base.clone()
            },
            Update {
                revision: 0,
                ..base.clone()
            },
        ] {
            assert!(invalid.validate().is_err());
        }
        assert!(
            Window {
                surface: 0,
                ..window()
            }
            .validate()
            .is_err()
        );
        for message in [String::new(), "x".repeat(MAX_DIAGNOSTIC + 1), "a\0b".into()] {
            assert!(
                Outcome::Failed {
                    stage: Stage::Preparing,
                    message: message.clone(),
                    outcome_unknown: false
                }
                .validate()
                .is_err()
            );
            assert!(
                Outcome::CompletedWithoutPresentation {
                    output: Some(message)
                }
                .validate()
                .is_err()
            );
        }
        assert!(State::Background { waited_ms: 0 }.validate().is_err());
        assert!(!State::Background { waited_ms: 1 }.terminal());
        assert!(
            State::Finished {
                outcome: Outcome::HandedOff {}
            }
            .terminal()
        );
        // Windowless completion keeps its earlier shape when nothing was written.
        assert_eq!(
            serde_json::to_value(Outcome::CompletedWithoutPresentation { output: None }).unwrap(),
            serde_json::json!({"kind":"completed_without_presentation"})
        );
        assert!(
            serde_json::from_value::<Outcome>(serde_json::json!({"kind":"handed_off","pid":1}))
                .is_err()
        );
    }
    #[test]
    fn message_shapes_are_stable_and_older_readers_reject_new_variants() {
        let launch = ClientMessage::Launch {
            session: 9,
            request: 4,
            target: crate::remote_apps::session_launch::Target::Command {
                argv: vec!["app".into()],
            },
        };
        assert_eq!(
            serde_json::to_value(launch).unwrap(),
            serde_json::json!({"type":"launch","session":9,"request":4,"target":{"kind":"command","argv":["app"]}})
        );
        assert_eq!(
            serde_json::to_value(ServerMessage::LaunchResult {
                session: 9,
                request: 4,
                outcome: crate::remote_apps::session_launch::Outcome::Accepted
            })
            .unwrap(),
            serde_json::json!({"type":"launch_result","session":9,"request":4,"outcome":{"status":"accepted"}})
        );
        assert_eq!(
            serde_json::to_value(ClientMessage::StopApplication {
                session: 9,
                request: 4
            })
            .unwrap(),
            serde_json::json!({"type":"stop_application","session":9,"request":4})
        );
        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        enum OlderReader {
            Launch {
                session: u64,
                request: u64,
                target: crate::remote_apps::session_launch::Target,
            },
        }
        let OlderReader::Launch {
            session,
            request,
            target,
        } = serde_json::from_value::<OlderReader>(serde_json::json!(
            {"type":"launch","session":9,"request":4,"target":{"kind":"command","argv":["app"]}}
        ))
        .unwrap();
        assert_eq!((session, request), (9, 4));
        target.validate().unwrap();
        let value = serde_json::to_value(ClientMessage::ApplicationPresented(Presented {
            session: 9,
            request: 4,
            window: window().window,
            surface: 2,
        }))
        .unwrap();
        assert!(serde_json::from_value::<OlderReader>(value).is_err());
    }
}
