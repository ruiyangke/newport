use super::*;
impl Convert for crate::desktop::applications::Runtime {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::Runtime;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::applications::Runtime {
                instance: Some(self.instance.to_string()),
                build_sha256: Some(self.build_sha256.clone()),
                transport_generation: Some(self.transport_generation),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            instance: uuid(required(value.instance)?)?,
            build_sha256: required(value.build_sha256)?,
            transport_generation: required(value.transport_generation)?,
        })
    }
}

impl Convert for crate::desktop::applications::Identity {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::Identity;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::applications::Identity {
                runtime: Some(self.runtime.to_proto()?),
                session: Some(self.session.to_string()),
                worker: Some(self.worker.to_string()),
                display_generation: Some(self.display_generation),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            runtime: <crate::desktop::applications::Runtime as Convert>::from_proto(required(
                value.runtime,
            )?)?,
            session: uuid(required(value.session)?)?,
            worker: uuid(required(value.worker)?)?,
            display_generation: required(value.display_generation)?,
        })
    }
}

impl Convert for crate::desktop::applications::Command {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::Command;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::applications::Command {
                argv: self
                    .argv
                    .iter()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .collect::<Result<_, _>>()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            argv: value
                .argv
                .into_iter()
                .map(|v| -> Result<_, Error> { Ok(v) })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl Convert for crate::desktop::applications::Desktop {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::Desktop;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::applications::Desktop {
                id: Some(self.id.clone()),
                uris: self
                    .uris
                    .iter()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .collect::<Result<_, _>>()?,
                action: self
                    .action
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            id: required(value.id)?,
            uris: value
                .uris
                .into_iter()
                .map(|v| -> Result<_, Error> { Ok(v) })
                .collect::<Result<_, _>>()?,
            action: value
                .action
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::applications::Target {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::Target;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::applications::Target { value: Some(match self { crate::desktop::applications::Target::Command(v) => crate::encoding::protobuf::generated::newport::desktop::applications::target::Value::Command(Box::new(v.as_ref().to_proto()?)), crate::desktop::applications::Target::Desktop(v) => crate::encoding::protobuf::generated::newport::desktop::applications::target::Value::Desktop(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::applications::target::Value::Command(v) => crate::desktop::applications::Target::Command(Box::new(<crate::desktop::applications::Command as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::applications::target::Value::Desktop(v) => crate::desktop::applications::Target::Desktop(Box::new(<crate::desktop::applications::Desktop as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::applications::Outcome {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::Outcome;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::applications::Outcome { value: Some(match self { crate::desktop::applications::Outcome::Accepted(v) => crate::encoding::protobuf::generated::newport::desktop::applications::outcome::Value::Accepted(Box::new(v.as_ref().to_proto()?)), crate::desktop::applications::Outcome::Rejected(v) => crate::encoding::protobuf::generated::newport::desktop::applications::outcome::Value::Rejected(v.as_ref().clone()), crate::desktop::applications::Outcome::Unknown(v) => crate::encoding::protobuf::generated::newport::desktop::applications::outcome::Value::Unknown(v.as_ref().clone()) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::applications::outcome::Value::Accepted(v) => crate::desktop::applications::Outcome::Accepted(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::applications::outcome::Value::Rejected(v) => crate::desktop::applications::Outcome::Rejected(Box::new(v)), crate::encoding::protobuf::generated::newport::desktop::applications::outcome::Value::Unknown(v) => crate::desktop::applications::Outcome::Unknown(Box::new(v)) })
    }
}

impl Convert for crate::desktop::applications::Application {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::Application;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::applications::Application {
                display: Some(self.display.to_proto()?),
                request: Some(self.request.get()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            display: <crate::desktop::applications::Identity as Convert>::from_proto(required(
                value.display,
            )?)?,
            request: std::num::NonZeroU64::new(required(value.request)?)
                .ok_or(Invalid("nonzero identifier"))?,
        })
    }
}

impl Convert for crate::desktop::applications::State {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::Pending => 1,
            Self::Opening => 2,
            Self::Background => 3,
            Self::Running => 4,
            Self::Stopping => 5,
            Self::Ended => 6,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::Pending,
            2 => Self::Opening,
            3 => Self::Background,
            4 => Self::Running,
            5 => Self::Stopping,
            6 => Self::Ended,
            _ => return Err(Invalid("State").into()),
        })
    }
}

impl Convert for crate::desktop::applications::Launched {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::Launched;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::applications::Launched {
                application: Some(self.application.to_proto()?),
                outcome: Some(self.outcome.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            application: <crate::desktop::applications::Application as Convert>::from_proto(
                required(value.application)?,
            )?,
            outcome: <crate::desktop::applications::Outcome as Convert>::from_proto(required(
                value.outcome,
            )?)?,
        })
    }
}

impl Convert for crate::desktop::applications::OptionalDetail {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::applications::OptionalDetail;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::applications::OptionalDetail {
                detail: self
                    .detail
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            detail: value
                .detail
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::applications::Ending {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::Ending;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::applications::Ending { value: Some(match self { crate::desktop::applications::Ending::Failed(v) => crate::encoding::protobuf::generated::newport::desktop::applications::ending::Value::Failed(v.as_ref().clone()), crate::desktop::applications::Ending::NoWindow(v) => crate::encoding::protobuf::generated::newport::desktop::applications::ending::Value::NoWindow(Box::new(v.as_ref().to_proto()?)), crate::desktop::applications::Ending::HandedOff(v) => crate::encoding::protobuf::generated::newport::desktop::applications::ending::Value::HandedOff(Box::new(v.as_ref().to_proto()?)), crate::desktop::applications::Ending::DisplayStopped(v) => crate::encoding::protobuf::generated::newport::desktop::applications::ending::Value::DisplayStopped(Box::new(v.as_ref().to_proto()?)), crate::desktop::applications::Ending::ShownElsewhere(v) => crate::encoding::protobuf::generated::newport::desktop::applications::ending::Value::ShownElsewhere(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::applications::ending::Value::Failed(v) => crate::desktop::applications::Ending::Failed(Box::new(v)), crate::encoding::protobuf::generated::newport::desktop::applications::ending::Value::NoWindow(v) => crate::desktop::applications::Ending::NoWindow(Box::new(<crate::desktop::applications::OptionalDetail as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::applications::ending::Value::HandedOff(v) => crate::desktop::applications::Ending::HandedOff(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::applications::ending::Value::DisplayStopped(v) => crate::desktop::applications::Ending::DisplayStopped(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::applications::ending::Value::ShownElsewhere(v) => crate::desktop::applications::Ending::ShownElsewhere(Box::new(<crate::desktop::applications::OptionalDetail as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::applications::Observation {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::Observation;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::applications::Observation {
                application: Some(self.application.to_proto()?),
                state: self
                    .state
                    .as_ref()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .transpose()?,
                error: self
                    .error
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                ending: self
                    .ending
                    .as_ref()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            application: <crate::desktop::applications::Application as Convert>::from_proto(
                required(value.application)?,
            )?,
            state: value
                .state
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::applications::State as Convert>::from_proto(v)
                })
                .transpose()?,
            error: value
                .error
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            ending: value
                .ending
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::applications::Ending as Convert>::from_proto(v)
                })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::applications::Observations {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::Observations;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::applications::Observations {
                observations: self
                    .observations
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            observations: value
                .observations
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::applications::Observation as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl Convert for crate::desktop::applications::Shown {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::Shown;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::applications::Shown {
                app_id: Some(self.app_id.clone()),
                windows: Some(self.windows),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            app_id: required(value.app_id)?,
            windows: required(value.windows)?,
        })
    }
}

impl Convert for crate::desktop::applications::ShownOn {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::ShownOn;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::applications::ShownOn {
                computer: self
                    .computer
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                session: self
                    .session
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            computer: value
                .computer
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            session: value
                .session
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::applications::Display {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::Display;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::applications::Display {
                shown_on: self
                    .shown_on
                    .as_ref()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .transpose()?,
                applications: self
                    .applications
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            shown_on: value
                .shown_on
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::applications::ShownOn as Convert>::from_proto(v)
                })
                .transpose()?,
            applications: value
                .applications
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::applications::Shown as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl Convert for crate::desktop::applications::Stopped {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::Stopped;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::applications::Stopped {
                stopped: Some(self.stopped),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            stopped: required(value.stopped)?,
        })
    }
}

impl Convert for crate::desktop::applications::Requirement {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::Dbus => 1,
            Self::Xwayland => 2,
            Self::Fonts => 3,
            Self::Gles => 4,
            Self::Portal => 5,
            Self::Audio => 6,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::Dbus,
            2 => Self::Xwayland,
            3 => Self::Fonts,
            4 => Self::Gles,
            5 => Self::Portal,
            6 => Self::Audio,
            _ => return Err(Invalid("Requirement").into()),
        })
    }
}

impl Convert for crate::desktop::applications::Inactive {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::Root => 1,
            Self::Stopped => 2,
            Self::Failed => 3,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::Root,
            2 => Self::Stopped,
            3 => Self::Failed,
            _ => return Err(Invalid("Inactive").into()),
        })
    }
}

impl Convert for crate::desktop::applications::Finding {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::Finding;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::applications::Finding { value: Some(match self { crate::desktop::applications::Finding::Missing(v) => crate::encoding::protobuf::generated::newport::desktop::applications::finding::Value::Missing(Box::new(v.as_ref().to_proto()?)), crate::desktop::applications::Finding::Outdated(v) => crate::encoding::protobuf::generated::newport::desktop::applications::finding::Value::Outdated(v.as_ref().clone()), crate::desktop::applications::Finding::Inactive(v) => crate::encoding::protobuf::generated::newport::desktop::applications::finding::Value::Inactive(v.as_ref().to_proto()?), crate::desktop::applications::Finding::Unverified(v) => crate::encoding::protobuf::generated::newport::desktop::applications::finding::Value::Unverified(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::applications::finding::Value::Missing(v) => crate::desktop::applications::Finding::Missing(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::applications::finding::Value::Outdated(v) => crate::desktop::applications::Finding::Outdated(Box::new(v)), crate::encoding::protobuf::generated::newport::desktop::applications::finding::Value::Inactive(v) => crate::desktop::applications::Finding::Inactive(Box::new(<crate::desktop::applications::Inactive as Convert>::from_proto(v)?)), crate::encoding::protobuf::generated::newport::desktop::applications::finding::Value::Unverified(v) => crate::desktop::applications::Finding::Unverified(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::applications::Gap {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::Gap;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::applications::Gap {
                requirement: Some(self.requirement.to_proto()?),
                finding: Some(self.finding.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            requirement: <crate::desktop::applications::Requirement as Convert>::from_proto(
                required(value.requirement)?,
            )?,
            finding: <crate::desktop::applications::Finding as Convert>::from_proto(required(
                value.finding,
            )?)?,
        })
    }
}

impl Convert for crate::desktop::applications::Elevation {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::Root => 1,
            Self::Sudo => 2,
            Self::Doas => 3,
            Self::Unavailable => 4,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::Root,
            2 => Self::Sudo,
            3 => Self::Doas,
            4 => Self::Unavailable,
            _ => return Err(Invalid("Elevation").into()),
        })
    }
}

impl Convert for crate::desktop::applications::Updater {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::RpmOstree => 1,
            Self::TransactionalUpdate => 2,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::RpmOstree,
            2 => Self::TransactionalUpdate,
            _ => return Err(Invalid("Updater").into()),
        })
    }
}

impl Convert for crate::desktop::applications::Host {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::Host;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::applications::Host {
                elevation: Some(self.elevation.to_proto()?),
                updater: self
                    .updater
                    .as_ref()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .transpose()?,
                user_services: Some(self.user_services),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            elevation: <crate::desktop::applications::Elevation as Convert>::from_proto(required(
                value.elevation,
            )?)?,
            updater: value
                .updater
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::applications::Updater as Convert>::from_proto(v)
                })
                .transpose()?,
            user_services: required(value.user_services)?,
        })
    }
}

impl Convert for crate::desktop::applications::LegacyAgent {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::LegacyAgent;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::applications::LegacyAgent {
                revision: Some(self.revision.clone()),
                agent: Some(self.agent.clone()),
                built_at: self
                    .built_at
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            revision: required(value.revision)?,
            agent: required(value.agent)?,
            built_at: value
                .built_at
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::applications::SelectedRuntime {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::applications::SelectedRuntime;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::applications::SelectedRuntime {
                build_sha256: Some(self.build_sha256.clone()),
                product: Some(self.product.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            build_sha256: required(value.build_sha256)?,
            product: <crate::product::Variant as Convert>::from_proto(required(value.product)?)?,
        })
    }
}

impl Convert for crate::desktop::applications::Availability {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::Availability;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::applications::Availability {
                agent: self
                    .agent
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                legacy_agent: self
                    .legacy_agent
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                legacy_revision: self
                    .legacy_revision
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                legacy_agents: self
                    .legacy_agents
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
                system: self
                    .system
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                architecture: self
                    .architecture
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                gaps: self
                    .gaps
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
                host: Some(self.host.to_proto()?),
                os_release: Some(self.os_release.clone()),
                expected_server: Some(self.expected_server.to_proto()?),
                bundled_variant: Some(self.bundled_variant.to_proto()?),
                bundled_revision: self
                    .bundled_revision
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                total: self
                    .total
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
                installed_variant: self
                    .installed_variant
                    .as_ref()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .transpose()?,
                revision: self
                    .revision
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                build_relation: Some(self.build_relation.to_proto()?),
                installation_error: self
                    .installation_error
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                selected_runtime: self
                    .selected_runtime
                    .as_ref()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .transpose()?,
                agent_error: self
                    .agent_error
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                automatic_update: self
                    .automatic_update
                    .as_ref()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            agent: value
                .agent
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            legacy_agent: value
                .legacy_agent
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            legacy_revision: value
                .legacy_revision
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            legacy_agents: value
                .legacy_agents
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::applications::LegacyAgent as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
            system: value
                .system
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            architecture: value
                .architecture
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            gaps: value
                .gaps
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::applications::Gap as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
            host: <crate::desktop::applications::Host as Convert>::from_proto(required(
                value.host,
            )?)?,
            os_release: required(value.os_release)?,
            expected_server: <crate::desktop::configuration::Server as Convert>::from_proto(
                required(value.expected_server)?,
            )?,
            bundled_variant: <crate::product::Variant as Convert>::from_proto(required(
                value.bundled_variant,
            )?)?,
            bundled_revision: value
                .bundled_revision
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            total: value
                .total
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            installed_variant: value
                .installed_variant
                .map(|v| -> Result<_, Error> {
                    <crate::product::Variant as Convert>::from_proto(v)
                })
                .transpose()?,
            revision: value
                .revision
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            build_relation: <crate::desktop::configuration::BuildRelation as Convert>::from_proto(
                required(value.build_relation)?,
            )?,
            installation_error: value
                .installation_error
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            selected_runtime: value
                .selected_runtime
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::applications::SelectedRuntime as Convert>::from_proto(v)
                })
                .transpose()?,
            agent_error: value
                .agent_error
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            automatic_update: value
                .automatic_update
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::UpdateOutcome as Convert>::from_proto(v)
                })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::applications::StatusArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::StatusArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::applications::StatusArgs {
                server: Some(self.server.to_string()),
                refresh: Some(self.refresh),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            refresh: value.refresh.unwrap_or(false),
        })
    }
}

impl Convert for crate::desktop::applications::CatalogueArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::applications::CatalogueArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::applications::CatalogueArgs {
                server: Some(self.server.to_string()),
                refresh: Some(self.refresh),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            refresh: value.refresh.unwrap_or(false),
        })
    }
}

impl Convert for crate::desktop::applications::DisplayArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::DisplayArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::applications::DisplayArgs {
                server: Some(self.server.to_string()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
        })
    }
}

impl Convert for crate::desktop::applications::LaunchArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::LaunchArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::applications::LaunchArgs {
                server: Some(self.server.to_string()),
                expected: Some(self.expected.to_proto()?),
                target: Some(self.target.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            expected: <crate::desktop::configuration::ConnectionScope as Convert>::from_proto(
                required(value.expected)?,
            )?,
            target: <crate::desktop::applications::Target as Convert>::from_proto(required(
                value.target,
            )?)?,
        })
    }
}

impl Convert for crate::desktop::applications::ShowArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::ShowArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::applications::ShowArgs {
                server: Some(self.server.to_string()),
                expected: Some(self.expected.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            expected: <crate::desktop::configuration::ConnectionScope as Convert>::from_proto(
                required(value.expected)?,
            )?,
        })
    }
}

impl Convert for crate::desktop::applications::StatesArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::StatesArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::applications::StatesArgs {
                server: Some(self.server.to_string()),
                expected: Some(self.expected.to_proto()?),
                applications: self
                    .applications
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            expected: <crate::desktop::configuration::ConnectionScope as Convert>::from_proto(
                required(value.expected)?,
            )?,
            applications: value
                .applications
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::applications::Application as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl Convert for crate::desktop::applications::StopArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::StopArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::applications::StopArgs {
                server: Some(self.server.to_string()),
                expected: Some(self.expected.to_proto()?),
                application: Some(self.application.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            expected: <crate::desktop::configuration::ConnectionScope as Convert>::from_proto(
                required(value.expected)?,
            )?,
            application: <crate::desktop::applications::Application as Convert>::from_proto(
                required(value.application)?,
            )?,
        })
    }
}

impl Convert for crate::desktop::applications::Operation {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::Operation;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::applications::Operation { value: Some(match self { crate::desktop::applications::Operation::Status(v) => crate::encoding::protobuf::generated::newport::desktop::applications::operation::Value::Status(Box::new(v.as_ref().to_proto()?)), crate::desktop::applications::Operation::Catalogue(v) => crate::encoding::protobuf::generated::newport::desktop::applications::operation::Value::Catalogue(Box::new(v.as_ref().to_proto()?)), crate::desktop::applications::Operation::Display(v) => crate::encoding::protobuf::generated::newport::desktop::applications::operation::Value::Display(Box::new(v.as_ref().to_proto()?)), crate::desktop::applications::Operation::Launch(v) => crate::encoding::protobuf::generated::newport::desktop::applications::operation::Value::Launch(Box::new(v.as_ref().to_proto()?)), crate::desktop::applications::Operation::Show(v) => crate::encoding::protobuf::generated::newport::desktop::applications::operation::Value::Show(Box::new(v.as_ref().to_proto()?)), crate::desktop::applications::Operation::States(v) => crate::encoding::protobuf::generated::newport::desktop::applications::operation::Value::States(Box::new(v.as_ref().to_proto()?)), crate::desktop::applications::Operation::Stop(v) => crate::encoding::protobuf::generated::newport::desktop::applications::operation::Value::Stop(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::applications::operation::Value::Status(v) => crate::desktop::applications::Operation::Status(Box::new(<crate::desktop::applications::StatusArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::applications::operation::Value::Catalogue(v) => crate::desktop::applications::Operation::Catalogue(Box::new(<crate::desktop::applications::CatalogueArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::applications::operation::Value::Display(v) => crate::desktop::applications::Operation::Display(Box::new(<crate::desktop::applications::DisplayArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::applications::operation::Value::Launch(v) => crate::desktop::applications::Operation::Launch(Box::new(<crate::desktop::applications::LaunchArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::applications::operation::Value::Show(v) => crate::desktop::applications::Operation::Show(Box::new(<crate::desktop::applications::ShowArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::applications::operation::Value::States(v) => crate::desktop::applications::Operation::States(Box::new(<crate::desktop::applications::StatesArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::applications::operation::Value::Stop(v) => crate::desktop::applications::Operation::Stop(Box::new(<crate::desktop::applications::StopArgs as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::applications::Reply {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::applications::Reply;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::applications::Reply { value: Some(match self { crate::desktop::applications::Reply::Status(v) => crate::encoding::protobuf::generated::newport::desktop::applications::reply::Value::Status(Box::new(v.as_ref().to_proto()?)), crate::desktop::applications::Reply::Catalogue(v) => crate::encoding::protobuf::generated::newport::desktop::applications::reply::Value::Catalogue(Box::new(v.as_ref().to_proto()?)), crate::desktop::applications::Reply::Display(v) => crate::encoding::protobuf::generated::newport::desktop::applications::reply::Value::Display(Box::new(v.as_ref().to_proto()?)), crate::desktop::applications::Reply::Launch(v) => crate::encoding::protobuf::generated::newport::desktop::applications::reply::Value::Launch(Box::new(v.as_ref().to_proto()?)), crate::desktop::applications::Reply::Show(v) => crate::encoding::protobuf::generated::newport::desktop::applications::reply::Value::Show(Box::new(v.as_ref().to_proto()?)), crate::desktop::applications::Reply::States(v) => crate::encoding::protobuf::generated::newport::desktop::applications::reply::Value::States(Box::new(v.as_ref().to_proto()?)), crate::desktop::applications::Reply::Stop(v) => crate::encoding::protobuf::generated::newport::desktop::applications::reply::Value::Stop(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::applications::reply::Value::Status(v) => crate::desktop::applications::Reply::Status(Box::new(<crate::desktop::applications::Availability as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::applications::reply::Value::Catalogue(v) => crate::desktop::applications::Reply::Catalogue(Box::new(<crate::remote_apps::apps::Catalogue as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::applications::reply::Value::Display(v) => crate::desktop::applications::Reply::Display(Box::new(<crate::desktop::applications::Display as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::applications::reply::Value::Launch(v) => crate::desktop::applications::Reply::Launch(Box::new(<crate::desktop::applications::Launched as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::applications::reply::Value::Show(v) => crate::desktop::applications::Reply::Show(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::applications::reply::Value::States(v) => crate::desktop::applications::Reply::States(Box::new(<crate::desktop::applications::Observations as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::applications::reply::Value::Stop(v) => crate::desktop::applications::Reply::Stop(Box::new(<crate::desktop::applications::Stopped as Convert>::from_proto(*v)?)) })
    }
}
