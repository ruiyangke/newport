//! Native text-input v3: fenced inline preedit/commit; revisions order geometry.
use crate::remote_apps::v1::WindowId;
use serde::{Deserialize, Serialize};
pub const MAX_TEXT_BYTES: usize = 16 * 1024;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub window: WindowId,
    pub revision: u64,
    /// First published revision of this focus lifetime; geometry keeps it stable.
    pub focus_epoch: u64,
    pub preedit: bool,
    /// Remote surface pixel coordinates at the caret bottom edge / XIM baseline.
    /// None means no reliable caret location is available.
    pub caret: Option<[i32; 2]>,
}
impl Context {
    pub fn same_focus(&self, other: &Self) -> bool {
        self.window == other.window
            && self.focus_epoch == other.focus_epoch
            && self.preedit == other.preedit
    }
    /// A captured composition may lag geometry, but never a focus boundary.
    pub fn accepts(&self, captured: &Self) -> bool {
        self.same_focus(captured)
            && captured.revision >= self.focus_epoch
            && captured.revision <= self.revision
    }

    pub fn validate(&self) -> Result<(), String> {
        self.window.validate()?;
        if self.focus_epoch == 0 || self.revision < self.focus_epoch {
            return Err("Invalid text-input revision".into());
        }
        if self
            .caret
            .is_some_and(|p| p.iter().any(|n| !(-65536..=65536).contains(n)))
        {
            return Err("Invalid text-input caret".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Client {
    Preedit {
        request: u64,
        context: Context,
        text: String,
        cursor: u32,
    },
    Commit {
        request: u64,
        context: Context,
        text: String,
    },
}
impl Client {
    pub fn context(&self) -> &Context {
        match self {
            Self::Commit { context, .. } | Self::Preedit { context, .. } => context,
        }
    }
    pub fn request(&self) -> u64 {
        match self {
            Self::Commit { request, .. } | Self::Preedit { request, .. } => *request,
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        self.context().validate()?;
        let (text, preedit) = match self {
            Self::Commit { text, .. } => (text, false),
            Self::Preedit {
                text,
                cursor,
                context,
                ..
            } => {
                if !context.preedit || *cursor as usize > text.chars().count() {
                    return Err("Invalid preedit context or cursor".into());
                }
                (text, true)
            }
        };
        if self.request() == 0
            || (!preedit && text.is_empty())
            || text.len() > MAX_TEXT_BYTES
            || text
                .chars()
                .any(|c| c.is_control() && (preedit || !matches!(c, '\t' | '\n')))
        {
            return Err("Invalid text-input update".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Server {
    Focus { context: Option<Context> },
    Result { request: u64, applied: bool },
}
impl Server {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Focus {
                context: Some(context),
            } => context.validate(),
            Self::Result { request: 0, .. } => Err("Invalid text-input result".into()),
            _ => Ok(()),
        }
    }
}
#[cfg(all(test, feature = "protobuf"))]
mod tests {
    use super::*;
    #[test]
    fn preedit_requires_support_and_scalar_cursor_but_allows_cancellation() {
        let context = Context {
            window: WindowId {
                id: 1,
                generation: 1,
            },
            revision: 1,
            focus_epoch: 1,
            preedit: true,
            caret: None,
        };
        for (text, cursor, valid) in [
            ("", 0, true),
            ("拼音👋", 3, true),
            ("拼音👋", 4, false),
            ("\0", 0, false),
        ] {
            let message = Client::Preedit {
                request: 1,
                context: context.clone(),
                text: text.into(),
                cursor,
            };
            assert_eq!(message.validate().is_ok(), valid);
            assert_eq!(
                serde_json::from_slice::<Client>(&serde_json::to_vec(&message).unwrap()).unwrap(),
                message
            );
        }
        assert!(
            Client::Preedit {
                request: 1,
                context: Context {
                    preedit: false,
                    ..context
                },
                text: "x".into(),
                cursor: 1,
            }
            .validate()
            .is_err()
        );
    }
    #[test]
    fn v2_requires_a_valid_focus_epoch_and_rejects_future_geometry() {
        let old = Context {
            window: WindowId {
                id: 1,
                generation: 1,
            },
            revision: 3,
            focus_epoch: 3,
            preedit: false,
            caret: None,
        };
        let moved = Context {
            revision: 4,
            caret: Some([20, 30]),
            ..old.clone()
        };
        assert!(moved.accepts(&old));
        assert!(!old.accepts(&moved));
        assert!(
            !Context {
                focus_epoch: 4,
                ..moved.clone()
            }
            .accepts(&old)
        );
        assert!(
            Context {
                focus_epoch: 0,
                ..old.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            Context {
                focus_epoch: 5,
                ..moved.clone()
            }
            .validate()
            .is_err()
        );
        let mut without_focus_epoch = serde_json::to_value(&old).unwrap();
        without_focus_epoch
            .as_object_mut()
            .unwrap()
            .remove("focus_epoch");
        assert!(serde_json::from_value::<Context>(without_focus_epoch).is_err());
    }
    #[test]
    fn commits_roundtrip_unicode_and_fit_the_client_line_bound() {
        let context = Context {
            window: WindowId {
                id: 1,
                generation: 2,
            },
            revision: 3,
            focus_epoch: 3,
            preedit: false,
            caret: Some([10, 20]),
        };
        for text in ["你好 👋 e\u{301}".to_owned(), "\n".repeat(MAX_TEXT_BYTES)] {
            let message = Client::Commit {
                request: 1,
                context: context.clone(),
                text,
            };
            message.validate().unwrap();
            let framed = crate::remote_apps::v1::ClientMessage::TextInput {
                session: 1,
                message: message.clone(),
            };
            let bytes = crate::remote_apps::wire::encode_client(&framed).unwrap();
            let crate::remote_apps::v1::ClientMessage::TextInput { message: read, .. } =
                crate::remote_apps::wire::read_client(&mut bytes.as_slice())
                    .unwrap()
                    .0
            else {
                panic!()
            };
            assert_eq!(read, message);
        }
        for text in ["".to_owned(), "\0".into(), "x".repeat(MAX_TEXT_BYTES + 1)] {
            assert!(
                Client::Commit {
                    request: 1,
                    context: context.clone(),
                    text,
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            Context {
                revision: 0,
                ..context
            }
            .validate()
            .is_err()
        );
    }
}
