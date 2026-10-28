//! Read-only validation of an existing terminal attachment. A successful
//! response is a point-in-time observation, never a lease or ownership receipt.
use super::SharedGuiEnvironment;
use serde::{Deserialize, Serialize};
use std::io;
use uuid::Uuid;

pub const CAPABILITY: &str = "gui-attachment-check";
pub const VERSION: u32 = 1;
pub const FEATURE: &str = "gui-attachment-check-v1";
pub const ENABLE: u8 = b'c';
pub const REQUEST: u8 = b'a';
pub const REPLY: u8 = b'b';
// Integration events have a size ceiling. Leave room for JSON escaping.
pub const MAX_MESSAGE: usize = 8192;
pub const WAIT_SECONDS: u64 = 5;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Endpoints {
    #[serde(deserialize_with = "explicit_option")]
    pub display: Option<String>,
    #[serde(deserialize_with = "explicit_option")]
    pub authority: Option<String>,
    /// Already normalized to an absolute path; no ambient environment or fd.
    pub wayland_socket: String,
}
fn explicit_option<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Deserialize::deserialize(d)
}
impl Endpoints {
    pub fn matches(&self, environment: &SharedGuiEnvironment) -> bool {
        self.display == environment.display
            && self.authority == environment.authority
            && self.wayland_socket == environment.wayland_socket
    }
    fn validate(&self) -> io::Result<()> {
        super::environment::contract::shared_attachment_v2::validate(
            self.display.as_deref(),
            self.authority.as_deref(),
            &self.wayland_socket,
            None,
            &Default::default(),
        )
        .map_err(io::Error::other)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Check {
    pub id: Uuid,
    pub session: Uuid,
    pub terminal: Uuid,
    pub epoch: Uuid,
    pub endpoints: Endpoints,
}
impl Check {
    pub fn validate(&self) -> io::Result<()> {
        identities([self.id, self.session, self.terminal, self.epoch])?;
        self.endpoints.validate()?;
        encode(self).map(|_| ())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Reason {
    SessionMismatch,
    StaleShell,
    NoSharedDisplay,
    EndpointMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase", deny_unknown_fields)]
pub enum Outcome {
    Current {},
    /// This does not prove that the claimed endpoints belong to Newport.
    NotCurrent {
        reason: Reason,
    },
    /// Permission, transport and display validation failures stay errors.
    Error {
        message: String,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Checked {
    pub id: Uuid,
    pub session: Uuid,
    pub terminal: Uuid,
    pub epoch: Uuid,
    pub outcome: Outcome,
}
impl Checked {
    pub fn new(request: &Check, outcome: Outcome) -> Self {
        Self {
            id: request.id,
            session: request.session,
            terminal: request.terminal,
            epoch: request.epoch,
            outcome,
        }
    }
    pub fn error(request: &Check, message: &str) -> Self {
        let mut message: String = message
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        while message.len() > 1024 {
            message.pop();
        }
        if message.is_empty() {
            message = "Attachment validation failed".into();
        }
        Self::new(request, Outcome::Error { message })
    }
    pub fn validate(&self) -> io::Result<()> {
        identities([self.id, self.session, self.terminal, self.epoch])?;
        if let Outcome::Error { message } = &self.outcome {
            if message.is_empty() || message.len() > 1024 || message.chars().any(char::is_control) {
                return Err(io::Error::other("Invalid attachment error"));
            }
        }
        encode(self).map(|_| ())
    }
    pub fn accept(&self, request: &Check) -> io::Result<&Outcome> {
        self.validate()?;
        if (self.id, self.session, self.terminal, self.epoch)
            != (request.id, request.session, request.terminal, request.epoch)
        {
            return Err(io::Error::other(
                "Attachment reply belongs to another request or shell",
            ));
        }
        Ok(&self.outcome)
    }
}
fn identities(ids: [Uuid; 4]) -> io::Result<()> {
    if ids.iter().any(Uuid::is_nil) {
        Err(io::Error::other("Invalid attachment identity"))
    } else {
        Ok(())
    }
}
pub fn encode(value: &impl Serialize) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_MESSAGE {
        return Err(io::Error::other("Attachment message too large"));
    }
    Ok(bytes)
}
pub fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> io::Result<T> {
    if bytes.len() > MAX_MESSAGE {
        return Err(io::Error::other("Attachment message too large"));
    }
    Ok(serde_json::from_slice(bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> Check {
        Check {
            id: Uuid::new_v4(),
            session: Uuid::new_v4(),
            terminal: Uuid::new_v4(),
            epoch: Uuid::new_v4(),
            endpoints: Endpoints {
                display: Some(":77".into()),
                authority: Some("/tmp/authority".into()),
                wayland_socket: "/tmp/wayland-0".into(),
            },
        }
    }
    #[test]
    fn strict_identity_endpoints_and_explicit_native_nulls() {
        let request = request();
        request.validate().unwrap();
        let value = serde_json::to_value(&request).unwrap();
        for key in ["id", "session", "terminal", "epoch"] {
            let mut invalid = value.clone();
            invalid[key] = Uuid::nil().to_string().into();
            assert!(
                serde_json::from_value::<Check>(invalid)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        for key in ["display", "authority"] {
            let mut invalid = value.clone();
            invalid["endpoints"].as_object_mut().unwrap().remove(key);
            assert!(serde_json::from_value::<Check>(invalid).is_err());
        }
        for (key, text) in [
            ("display", "host:0"),
            ("authority", "/tmp/../foreign"),
            ("waylandSocket", "relative"),
            ("waylandSocket", "/tmp/socket\0"),
        ] {
            let mut invalid = value.clone();
            invalid["endpoints"][key] = text.into();
            assert!(
                serde_json::from_value::<Check>(invalid)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        for key in ["command", "environment", "generation"] {
            let mut invalid = value.clone();
            invalid[key] = "injected".into();
            assert!(serde_json::from_value::<Check>(invalid).is_err());
        }
        let mut native = request;
        native.endpoints.display = None;
        assert!(native.validate().is_err());
        native.endpoints.authority = None;
        native.validate().unwrap();
    }
    #[test]
    fn bound_encoded_bytes_and_reject_unrelated_or_extended_replies() {
        let request = request();
        let reply = Checked::new(&request, Outcome::Current {});
        assert_eq!(reply.accept(&request).unwrap(), &Outcome::Current {});
        for key in ["id", "session", "terminal", "epoch"] {
            let mut wrong = serde_json::to_value(&reply).unwrap();
            wrong[key] = Uuid::new_v4().to_string().into();
            assert!(
                serde_json::from_value::<Checked>(wrong)
                    .unwrap()
                    .accept(&request)
                    .is_err()
            );
        }
        let mut wrong = serde_json::to_value(&reply).unwrap();
        wrong["outcome"]["environment"] = "unexpected".into();
        assert!(serde_json::from_value::<Checked>(wrong).is_err());
        let mut escaped = request.clone();
        escaped.endpoints.authority = Some(format!("/{}", "\\".repeat(4000)));
        escaped.endpoints.wayland_socket = format!("/{}", "\\".repeat(4000));
        assert!(
            escaped.validate().is_err(),
            "JSON escaping must count toward the event limit"
        );
        assert!(decode::<Check>(&vec![b' '; MAX_MESSAGE + 1]).is_err());
        let error = Checked::error(&request, &"permission revoked\n".repeat(200));
        error.validate().unwrap();
        assert!(matches!(
            error.accept(&request).unwrap(),
            Outcome::Error { .. }
        ));
    }
}
