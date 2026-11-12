//! Launching applications on the display.
use serde::{Deserialize, Serialize};

pub const MAX_PENDING: usize = 32;
/// Encoded size of one request's target, which must fit a client message
/// with the rest of its frame.
pub const MAX_REQUEST_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    /// Literal argv, with no implicit shell expansion.
    Command { argv: Vec<String> },
    Desktop {
        id: String,
        #[serde(default)]
        uris: Vec<String>,
        #[serde(default)]
        action: Option<String>,
    },
}
impl Target {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Command { argv } => {
                if argv.is_empty()
                    || argv.len() > 128
                    || argv[0].is_empty()
                    || argv
                        .iter()
                        .any(|arg| arg.len() > 8192 || arg.contains('\0'))
                    || argv.iter().map(String::len).sum::<usize>() > 16384
                {
                    return Err("Invalid application arguments".into());
                }
            }
            Self::Desktop { id, uris, action } => {
                crate::remote_apps::apps::validate_id(id)?;
                crate::remote_apps::apps::validate_open_uris(uris)?;
                if let Some(action) = action {
                    crate::remote_apps::apps::validate_action(action)?;
                    if !uris.is_empty() {
                        return Err("Desktop actions do not take Open URIs".into());
                    }
                }
            }
        }
        if super::size::launch(self) > MAX_REQUEST_BYTES {
            return Err("Application request exceeds the wire limit".into());
        }
        Ok(())
    }
}

/// Acceptance means an owned launcher started, not that a window has opened.
/// Window events remain authoritative, including single-instance activation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum Outcome {
    Accepted,
    Failed { message: String },
}
impl Outcome {
    pub fn validate(&self) -> Result<(), String> {
        if let Self::Failed { message } = self
            && (message.is_empty() || message.len() > 8192 || message.contains('\0'))
        {
            return Err("Invalid application launch result".into());
        }
        Ok(())
    }
}

#[cfg(all(test, feature = "protobuf"))]
mod tests {
    use super::*;
    #[test]
    fn request_size_limit_is_exact() {
        // URIs within their own limits, together over the request's.
        let uri = |length: usize| format!("file:///{}", "a".repeat(length - 8));
        let sized = |last: usize| Target::Desktop {
            id: "app.desktop".into(),
            uris: vec![uri(8192), uri(8192), uri(8192), uri(last)],
            action: None,
        };
        let encoded = |target: &Target| {
            prost::Message::encoded_len(
                &crate::encoding::protobuf::generated::newport::remote::apps::LaunchTarget::from(
                    target,
                ),
            )
        };
        let fits = (9..=8192)
            .rev()
            .find(|&last| encoded(&sized(last)) <= MAX_REQUEST_BYTES)
            .unwrap();
        assert!(fits < 8192);
        sized(fits).validate().unwrap();
        assert_eq!(
            sized(fits + 1).validate().unwrap_err(),
            "Application request exceeds the wire limit"
        );
    }
    #[test]
    fn literal_arguments_and_desktop_actions_are_bounded() {
        let target = Target::Command {
            argv: vec!["app".into(), "$(touch /tmp/no); 'quoted'".into()],
        };
        target.validate().unwrap();
        assert_eq!(
            serde_json::from_slice::<Target>(&serde_json::to_vec(&target).unwrap()).unwrap(),
            target
        );
        for argv in [
            vec![],
            vec![String::new()],
            vec!["x\0y".into()],
            vec!["x".repeat(8193)],
            vec!["x".into(); 129],
            vec!["\n".repeat(8000); 3],
        ] {
            assert!(Target::Command { argv }.validate().is_err());
        }
        assert!(
            Target::Desktop {
                id: "../bad.desktop".into(),
                uris: vec![],
                action: None
            }
            .validate()
            .is_err()
        );
        assert!(
            Target::Desktop {
                id: "app.desktop".into(),
                uris: vec!["file:///tmp/test".into()],
                action: Some("New".into())
            }
            .validate()
            .is_err()
        );
        assert!(
            serde_json::from_str::<Target>(r#"{"kind":"command","argv":["app"],"display":":1"}"#)
                .is_err()
        );
    }
}
