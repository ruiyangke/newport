//! Bounded messages for a waiting local GUI launch. Arguments stay in the process.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io};

#[path = "gui_protocol/attachment.rs"]
pub mod attachment;
#[path = "gui_protocol/environment.rs"]
pub mod environment;
pub use environment::SharedGuiEnvironment;

pub const VERSIONS: &[u32] = &[1, 2, 3, 4, 5, 6];

pub const MAX_MESSAGE: usize = 32 * 1024;
pub const MAX_PENDING: usize = 32;
pub const WAIT_SECONDS: u64 = 15 * 60;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: uuid::Uuid,
    pub terminal: uuid::Uuid,
    pub application: String,
}
impl Request {
    pub fn validate(&self) -> io::Result<()> {
        if self.id.is_nil()
            || self.terminal.is_nil()
            || self.application.is_empty()
            || self.application.len() > 256
            || self.application.chars().any(char::is_control)
        {
            return Err(io::Error::other("Invalid GUI launch request"));
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub id: uuid::Uuid,
    pub environment: Option<BTreeMap<String, Option<String>>>,
    pub error: Option<String>,
}

/// Version two is explicitly negotiated. Keep the strict version-one records
/// above unchanged for clients already waiting on an older integration worker.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedRequest {
    pub id: uuid::Uuid,
    pub terminal: uuid::Uuid,
    pub epoch: uuid::Uuid,
    pub application: String,
    /// Only version-three helpers request a conditional owned-bus fallback.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub session_bus: bool,
    /// Version-four helpers can explicitly clear both X11 endpoints when the
    /// server session uses only native Wayland. Omit for strict older readers.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub native_wayland: bool,
    /// Version-five helpers accept the display's application policy
    /// (`SharedGuiEnvironment::application`). Omit for strict older readers.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub application_environment: bool,
    /// Version-six helpers accept schema and accessibility application policy.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub application_environment_v2: bool,
}
impl SharedRequest {
    /// Restrict a validated display policy to the profile this helper requested.
    pub fn project_application(&self, environment: &mut SharedGuiEnvironment) {
        if !self.application_environment {
            environment.application.clear();
        } else if !self.application_environment_v2 {
            environment
                .application
                .retain(|name, _| environment::contract::application_v1::allowed(name));
        }
    }
    pub fn validate(&self) -> io::Result<()> {
        if self.application_environment_v2 && !self.application_environment {
            return Err(io::Error::other(
                "Expanded GUI application environment requires its base profile",
            ));
        }
        Request {
            id: self.id,
            terminal: self.terminal,
            application: self.application.clone(),
        }
        .validate()?;
        if self.epoch.is_nil() {
            return Err(io::Error::other("Invalid terminal shell instance"));
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedReply {
    pub id: uuid::Uuid,
    pub epoch: uuid::Uuid,
    pub environment: Option<SharedGuiEnvironment>,
    pub error: Option<String>,
}
impl SharedReply {
    pub fn accept(
        self,
        request: &SharedRequest,
        session: uuid::Uuid,
    ) -> io::Result<SharedGuiEnvironment> {
        request.validate()?;
        if self.id != request.id || self.epoch != request.epoch {
            return Err(io::Error::other(
                "GUI reply belongs to another launch or shell",
            ));
        }
        match (self.environment, self.error) {
            (Some(environment), None) if environment.session == session => {
                if environment.fallback_bus.is_some() && !request.session_bus {
                    return Err(io::Error::other("Unrequested GUI session bus"));
                }
                if environment.native_wayland() && !request.native_wayland {
                    return Err(io::Error::other(
                        "Native Wayland GUI handoff was not negotiated",
                    ));
                }
                if !environment.application.is_empty() && !request.application_environment {
                    return Err(io::Error::other("Unrequested GUI application environment"));
                }
                environment.validate().map_err(io::Error::other)?;
                if !request.application_environment_v2 {
                    environment::contract::application_v1::validate(&environment.application)
                        .map_err(io::Error::other)?;
                }
                Ok(environment)
            }
            (None, Some(error)) => Err(io::Error::other(error)),
            _ => Err(io::Error::other("Invalid shared GUI launch reply")),
        }
    }
}

pub fn encode(value: &impl Serialize) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_MESSAGE {
        return Err(io::Error::other("GUI message too large"));
    }
    Ok(bytes)
}
pub fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> io::Result<T> {
    if bytes.len() > MAX_MESSAGE {
        return Err(io::Error::other("GUI message too large"));
    }
    Ok(serde_json::from_slice(bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_environment_requires_negotiation_and_old_requests_keep_their_shape() {
        let request = SharedRequest {
            id: uuid::Uuid::new_v4(),
            terminal: uuid::Uuid::new_v4(),
            epoch: uuid::Uuid::new_v4(),
            application: "fixture".into(),
            session_bus: false,
            native_wayland: false,
            application_environment: false,
            application_environment_v2: false,
        };
        let wire = serde_json::to_value(&request).unwrap();
        assert!(wire.get("applicationEnvironment").is_none());
        assert!(wire.get("nativeWayland").is_none());
        assert!(wire.get("sessionBus").is_none());
        assert!(
            !serde_json::from_value::<SharedRequest>(wire)
                .unwrap()
                .native_wayland
        );
        let session = uuid::Uuid::new_v4();
        for native in [false, true] {
            for negotiated in [false, true] {
                let request = SharedRequest {
                    native_wayland: negotiated,
                    ..request.clone()
                };
                let reply = SharedReply {
                    id: request.id,
                    epoch: request.epoch,
                    error: None,
                    environment: Some(SharedGuiEnvironment {
                        session,
                        display: (!native).then(|| ":75".into()),
                        authority: (!native).then(|| "/tmp/owned/Xauthority".into()),
                        wayland_socket: "/tmp/owned/wayland-0".into(),
                        fallback_bus: None,
                        toolkit: BTreeMap::new(),
                        application: BTreeMap::new(),
                    }),
                };
                assert_eq!(
                    reply.accept(&request, session).is_ok(),
                    !native || negotiated
                );
            }
        }
    }

    #[test]
    fn application_environment_requires_version_five_request() {
        let session = uuid::Uuid::new_v4();
        for requested in [false, true] {
            let request = SharedRequest {
                id: uuid::Uuid::new_v4(),
                terminal: uuid::Uuid::new_v4(),
                epoch: uuid::Uuid::new_v4(),
                application: "fixture".into(),
                session_bus: true,
                native_wayland: true,
                application_environment: requested,
                application_environment_v2: false,
            };
            let wire = serde_json::to_value(&request).unwrap();
            assert_eq!(wire.get("applicationEnvironment").is_some(), requested);
            for policy in [false, true] {
                let reply = SharedReply {
                    id: request.id,
                    epoch: request.epoch,
                    error: None,
                    environment: Some(SharedGuiEnvironment {
                        session,
                        display: None,
                        authority: None,
                        wayland_socket: "/tmp/owned/wayland-0".into(),
                        fallback_bus: None,
                        toolkit: BTreeMap::new(),
                        application: if policy {
                            [("XDG_CURRENT_DESKTOP".into(), Some("X-Newport".into()))].into()
                        } else {
                            BTreeMap::new()
                        },
                    }),
                };
                assert_eq!(
                    reply.accept(&request, session).is_ok(),
                    !policy || requested
                );
            }
        }
    }
    #[test]
    fn application_profiles_are_negotiated_and_projected_per_helper() {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct VersionFiveRequest {
            id: uuid::Uuid,
            terminal: uuid::Uuid,
            epoch: uuid::Uuid,
            application: String,
            session_bus: bool,
            native_wayland: bool,
            application_environment: bool,
        }
        let session = uuid::Uuid::new_v4();
        let environment = SharedGuiEnvironment {
            session,
            display: None,
            authority: None,
            wayland_socket: "/tmp/owned/wayland-0".into(),
            fallback_bus: None,
            toolkit: BTreeMap::new(),
            application: [
                ("XDG_CURRENT_DESKTOP".into(), Some("X-Newport".into())),
                (
                    "GSETTINGS_SCHEMA_DIR".into(),
                    Some("/usr/share/glib-2.0/schemas".into()),
                ),
                ("GTK_A11Y".into(), Some("none".into())),
                ("NO_AT_BRIDGE".into(), Some("1".into())),
            ]
            .into(),
        };
        for version in [4, 5, 6] {
            let request = SharedRequest {
                id: uuid::Uuid::new_v4(),
                terminal: uuid::Uuid::new_v4(),
                epoch: uuid::Uuid::new_v4(),
                application: "fixture".into(),
                session_bus: true,
                native_wayland: true,
                application_environment: version >= 5,
                application_environment_v2: version >= 6,
            };
            request.validate().unwrap();
            let wire = serde_json::to_value(&request).unwrap();
            assert_eq!(wire.get("applicationEnvironmentV2").is_some(), version == 6);
            if version == 5 {
                let old: VersionFiveRequest = serde_json::from_value(wire.clone()).unwrap();
                assert_eq!(
                    (old.id, old.terminal, old.epoch),
                    (request.id, request.terminal, request.epoch)
                );
                assert_eq!(old.application, "fixture");
                assert!(old.session_bus && old.native_wayland && old.application_environment);
            } else if version == 6 {
                assert!(serde_json::from_value::<VersionFiveRequest>(wire).is_err());
            }
            let reply = |environment| SharedReply {
                id: request.id,
                epoch: request.epoch,
                error: None,
                environment: Some(environment),
            };
            assert_eq!(
                reply(environment.clone()).accept(&request, session).is_ok(),
                version == 6
            );
            let mut projected = environment.clone();
            request.project_application(&mut projected);
            assert_eq!(
                projected.application.len(),
                match version {
                    4 => 0,
                    5 => 1,
                    _ => 4,
                }
            );
            assert_eq!(projected.wayland_socket, environment.wayland_socket);
            reply(projected).accept(&request, session).unwrap();
            let mut invalid = environment.clone();
            invalid
                .application
                .insert("GTK_A11Y".into(), Some("atspi".into()));
            assert!(reply(invalid).accept(&request, session).is_err());
            let mut inconsistent = request;
            inconsistent.application_environment = false;
            inconsistent.application_environment_v2 = true;
            assert!(inconsistent.validate().is_err());
            assert!(
                SharedReply {
                    id: inconsistent.id,
                    epoch: inconsistent.epoch,
                    error: None,
                    environment: Some(environment.clone()),
                }
                .accept(&inconsistent, session)
                .is_err()
            );
        }
    }
}
