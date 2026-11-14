use super::*;
impl Convert for crate::desktop::stream::TerminalOpenArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::TerminalOpenArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::stream::TerminalOpenArgs {
                server: Some(self.server.to_string()),
                session: Some(self.session.to_string()),
                columns: Some(u32::from(self.columns)),
                rows: Some(u32::from(self.rows)),
                persist: Some(self.persist),
                attach: Some(self.attach),
                colors: self
                    .colors
                    .as_ref()
                    .map(|v| -> Result<_, Error> {
                        Ok(bytes::Bytes::from(
                            v.iter().flatten().copied().collect::<Vec<_>>(),
                        ))
                    })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            session: uuid(required(value.session)?)?,
            columns: u16::try_from(required(value.columns)?).map_err(|_| Invalid("u16 range"))?,
            rows: u16::try_from(required(value.rows)?).map_err(|_| Invalid("u16 range"))?,
            persist: value.persist.unwrap_or(false),
            attach: value.attach.unwrap_or(false),
            colors: value.colors.map(rgb3).transpose()?,
        })
    }
}

impl Convert for crate::desktop::stream::CommandOpenArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::CommandOpenArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::stream::CommandOpenArgs {
                server: Some(self.server.to_string()),
                command: Some(self.command.clone()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            command: required(value.command)?,
        })
    }
}

impl Convert for crate::desktop::stream::RemoteAppsOpenArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::RemoteAppsOpenArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::stream::RemoteAppsOpenArgs {
                server: Some(self.server.to_string()),
                arguments: self
                    .arguments
                    .iter()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .collect::<Result<_, _>>()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            arguments: value
                .arguments
                .into_iter()
                .map(|v| -> Result<_, Error> { Ok(v) })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl Convert for crate::desktop::stream::LspOpenArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::LspOpenArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::stream::LspOpenArgs {
                server: Some(self.server.to_string()),
                expected: Some(self.expected.to_proto()?),
                root: Some(self.root.clone()),
                server_name: Some(self.server_name.clone()),
                envrc: Some(self.envrc),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            expected: <crate::desktop::configuration::ConnectionScope as Convert>::from_proto(
                required(value.expected)?,
            )?,
            root: required(value.root)?,
            server_name: required(value.server_name)?,
            envrc: required(value.envrc)?,
        })
    }
}

impl Convert for crate::desktop::stream::TerminalOpened {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::TerminalOpened;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::stream::TerminalOpened {
                version: Some(self.version),
                session: Some(self.session.to_string()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            version: required(value.version)?,
            session: uuid(required(value.session)?)?,
        })
    }
}

impl Convert for crate::desktop::stream::CommandOpened {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::CommandOpened;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::stream::CommandOpened {
                version: Some(self.version),
                server: Some(self.server.to_string()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            version: required(value.version)?,
            server: uuid(required(value.server)?)?,
        })
    }
}

impl Convert for crate::desktop::stream::RemoteAppsOpened {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::RemoteAppsOpened;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::stream::RemoteAppsOpened {
                version: Some(self.version),
                server: Some(self.server.to_string()),
                service_version: Some(self.service_version),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            version: required(value.version)?,
            server: uuid(required(value.server)?)?,
            service_version: required(value.service_version)?,
        })
    }
}

impl Convert for crate::desktop::stream::LspOpened {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::LspOpened;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::stream::LspOpened {
                version: Some(self.version),
                server: Some(self.server.to_string()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            version: required(value.version)?,
            server: uuid(required(value.server)?)?,
        })
    }
}

impl Convert for crate::desktop::stream::Opened {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::Opened;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::stream::Opened { value: Some(match self { crate::desktop::stream::Opened::Terminal(v) => crate::encoding::protobuf::generated::newport::desktop::stream::opened::Value::Terminal(Box::new(v.as_ref().to_proto()?)), crate::desktop::stream::Opened::Command(v) => crate::encoding::protobuf::generated::newport::desktop::stream::opened::Value::Command(Box::new(v.as_ref().to_proto()?)), crate::desktop::stream::Opened::RemoteApps(v) => crate::encoding::protobuf::generated::newport::desktop::stream::opened::Value::RemoteApps(Box::new(v.as_ref().to_proto()?)), crate::desktop::stream::Opened::Lsp(v) => crate::encoding::protobuf::generated::newport::desktop::stream::opened::Value::Lsp(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::stream::opened::Value::Terminal(v) => crate::desktop::stream::Opened::Terminal(Box::new(<crate::desktop::stream::TerminalOpened as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::stream::opened::Value::Command(v) => crate::desktop::stream::Opened::Command(Box::new(<crate::desktop::stream::CommandOpened as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::stream::opened::Value::RemoteApps(v) => crate::desktop::stream::Opened::RemoteApps(Box::new(<crate::desktop::stream::RemoteAppsOpened as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::stream::opened::Value::Lsp(v) => crate::desktop::stream::Opened::Lsp(Box::new(<crate::desktop::stream::LspOpened as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::stream::Ending {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::Exited => 1,
            Self::Stopped => 2,
            Self::Refused => 3,
            Self::Lost => 4,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::Exited,
            2 => Self::Stopped,
            3 => Self::Refused,
            4 => Self::Lost,
            _ => return Err(Invalid("Ending").into()),
        })
    }
}

impl Convert for crate::desktop::stream::CommandCompletion {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::CommandCompletion;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::stream::CommandCompletion {
                status: self
                    .status
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
                signal: self
                    .signal
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                ending: Some(self.ending.to_proto()?),
                stderr: Some(self.stderr.clone()),
                error: self
                    .error
                    .as_ref()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            status: value
                .status
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            signal: value
                .signal
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            ending: <crate::desktop::stream::Ending as Convert>::from_proto(required(
                value.ending,
            )?)?,
            stderr: required(value.stderr)?,
            error: value
                .error
                .map(|v| -> Result<_, Error> { <crate::common::Error as Convert>::from_proto(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::stream::LspCompletion {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::LspCompletion;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::stream::LspCompletion {
                error: self
                    .error
                    .as_ref()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            error: value
                .error
                .map(|v| -> Result<_, Error> { <crate::common::Error as Convert>::from_proto(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::stream::Completion {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::Completion;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::stream::Completion { value: Some(match self { crate::desktop::stream::Completion::Command(v) => crate::encoding::protobuf::generated::newport::desktop::stream::completion::Value::Command(Box::new(v.as_ref().to_proto()?)), crate::desktop::stream::Completion::Lsp(v) => crate::encoding::protobuf::generated::newport::desktop::stream::completion::Value::Lsp(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::stream::completion::Value::Command(v) => crate::desktop::stream::Completion::Command(Box::new(<crate::desktop::stream::CommandCompletion as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::stream::completion::Value::Lsp(v) => crate::desktop::stream::Completion::Lsp(Box::new(<crate::desktop::stream::LspCompletion as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::stream::OpaqueClient {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::OpaqueClient;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::stream::OpaqueClient { value: Some(match self { crate::desktop::stream::OpaqueClient::Data(v) => crate::encoding::protobuf::generated::newport::desktop::stream::opaque_client::Value::Data(v.as_ref().clone()), crate::desktop::stream::OpaqueClient::InputComplete(v) => crate::encoding::protobuf::generated::newport::desktop::stream::opaque_client::Value::InputComplete(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::stream::opaque_client::Value::Data(v) => crate::desktop::stream::OpaqueClient::Data(Box::new(v)), crate::encoding::protobuf::generated::newport::desktop::stream::opaque_client::Value::InputComplete(v) => crate::desktop::stream::OpaqueClient::InputComplete(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::stream::OpaqueServer {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::OpaqueServer;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::stream::OpaqueServer { value: Some(match self { crate::desktop::stream::OpaqueServer::Data(v) => crate::encoding::protobuf::generated::newport::desktop::stream::opaque_server::Value::Data(v.as_ref().clone()), crate::desktop::stream::OpaqueServer::Completion(v) => crate::encoding::protobuf::generated::newport::desktop::stream::opaque_server::Value::Completion(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::stream::opaque_server::Value::Data(v) => crate::desktop::stream::OpaqueServer::Data(Box::new(v)), crate::encoding::protobuf::generated::newport::desktop::stream::opaque_server::Value::Completion(v) => crate::desktop::stream::OpaqueServer::Completion(Box::new(<crate::desktop::stream::Completion as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::stream::Input {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::Input;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::stream::Input {
                epoch: Some(self.epoch),
                data: Some(self.data.clone()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            epoch: required(value.epoch)?,
            data: required(value.data)?,
        })
    }
}

impl Convert for crate::desktop::stream::Resize {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::Resize;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::stream::Resize {
                columns: Some(u32::from(self.columns)),
                rows: Some(u32::from(self.rows)),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            columns: u16::try_from(required(value.columns)?).map_err(|_| Invalid("u16 range"))?,
            rows: u16::try_from(required(value.rows)?).map_err(|_| Invalid("u16 range"))?,
        })
    }
}

impl Convert for crate::desktop::stream::Ack {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::Ack;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::stream::Ack {
                bytes: Some(self.bytes),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            bytes: required(value.bytes)?,
        })
    }
}

impl Convert for crate::desktop::stream::TerminalClient {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::TerminalClient;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::stream::TerminalClient { value: Some(match self { crate::desktop::stream::TerminalClient::Input(v) => crate::encoding::protobuf::generated::newport::desktop::stream::terminal_client::Value::Input(Box::new(v.as_ref().to_proto()?)), crate::desktop::stream::TerminalClient::Resize(v) => crate::encoding::protobuf::generated::newport::desktop::stream::terminal_client::Value::Resize(Box::new(v.as_ref().to_proto()?)), crate::desktop::stream::TerminalClient::Ack(v) => crate::encoding::protobuf::generated::newport::desktop::stream::terminal_client::Value::Ack(Box::new(v.as_ref().to_proto()?)), crate::desktop::stream::TerminalClient::End(v) => crate::encoding::protobuf::generated::newport::desktop::stream::terminal_client::Value::End(Box::new(v.as_ref().to_proto()?)), crate::desktop::stream::TerminalClient::Activate(v) => crate::encoding::protobuf::generated::newport::desktop::stream::terminal_client::Value::Activate(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::stream::terminal_client::Value::Input(v) => crate::desktop::stream::TerminalClient::Input(Box::new(<crate::desktop::stream::Input as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::stream::terminal_client::Value::Resize(v) => crate::desktop::stream::TerminalClient::Resize(Box::new(<crate::desktop::stream::Resize as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::stream::terminal_client::Value::Ack(v) => crate::desktop::stream::TerminalClient::Ack(Box::new(<crate::desktop::stream::Ack as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::stream::terminal_client::Value::End(v) => crate::desktop::stream::TerminalClient::End(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::stream::terminal_client::Value::Activate(v) => crate::desktop::stream::TerminalClient::Activate(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::stream::Output {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::Output;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::stream::Output {
                data: Some(self.data.clone()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            data: required(value.data)?,
        })
    }
}

impl Convert for crate::desktop::stream::Taken {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::Taken;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::stream::Taken {
                bytes: Some(self.bytes),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            bytes: required(value.bytes)?,
        })
    }
}

impl Convert for crate::desktop::stream::Restore {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::Restore;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::stream::Restore {
                state: Some(self.state.clone()),
                prompted: Some(self.prompted),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            state: required(value.state)?,
            prompted: required(value.prompted)?,
        })
    }
}

impl Convert for crate::desktop::stream::Reconnecting {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::Reconnecting;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::stream::Reconnecting {
                attempt: Some(self.attempt),
                message: Some(self.message.clone()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            attempt: required(value.attempt)?,
            message: required(value.message)?,
        })
    }
}

impl Convert for crate::desktop::stream::NotHeld {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::NotHeld;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::stream::NotHeld {
                reason: Some(self.reason.clone()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            reason: required(value.reason)?,
        })
    }
}

impl Convert for crate::desktop::stream::Exit {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::Exit;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::stream::Exit {
                code: self
                    .code
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            code: value
                .code
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::stream::Active {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::Active;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::stream::Active {
                active: Some(self.active),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            active: required(value.active)?,
        })
    }
}

impl Convert for crate::desktop::stream::TerminalEvent {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::TerminalEvent;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::stream::TerminalEvent { value: Some(match self { crate::desktop::stream::TerminalEvent::Ready(v) => crate::encoding::protobuf::generated::newport::desktop::stream::terminal_event::Value::Ready(Box::new(v.as_ref().to_proto()?)), crate::desktop::stream::TerminalEvent::Persistent(v) => crate::encoding::protobuf::generated::newport::desktop::stream::terminal_event::Value::Persistent(Box::new(v.as_ref().to_proto()?)), crate::desktop::stream::TerminalEvent::Reconnecting(v) => crate::encoding::protobuf::generated::newport::desktop::stream::terminal_event::Value::Reconnecting(Box::new(v.as_ref().to_proto()?)), crate::desktop::stream::TerminalEvent::Exit(v) => crate::encoding::protobuf::generated::newport::desktop::stream::terminal_event::Value::Exit(Box::new(v.as_ref().to_proto()?)), crate::desktop::stream::TerminalEvent::Error(v) => crate::encoding::protobuf::generated::newport::desktop::stream::terminal_event::Value::Error(Box::new(v.as_ref().to_proto()?)), crate::desktop::stream::TerminalEvent::Ended(v) => crate::encoding::protobuf::generated::newport::desktop::stream::terminal_event::Value::Ended(Box::new(v.as_ref().to_proto()?)), crate::desktop::stream::TerminalEvent::Active(v) => crate::encoding::protobuf::generated::newport::desktop::stream::terminal_event::Value::Active(Box::new(v.as_ref().to_proto()?)), crate::desktop::stream::TerminalEvent::DisplayLost(v) => crate::encoding::protobuf::generated::newport::desktop::stream::terminal_event::Value::DisplayLost(Box::new(v.as_ref().to_proto()?)), crate::desktop::stream::TerminalEvent::NotHeld(v) => crate::encoding::protobuf::generated::newport::desktop::stream::terminal_event::Value::NotHeld(Box::new(v.as_ref().to_proto()?)), crate::desktop::stream::TerminalEvent::Size(v) => crate::encoding::protobuf::generated::newport::desktop::stream::terminal_event::Value::Size(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::stream::terminal_event::Value::Ready(v) => crate::desktop::stream::TerminalEvent::Ready(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::stream::terminal_event::Value::Persistent(v) => crate::desktop::stream::TerminalEvent::Persistent(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::stream::terminal_event::Value::Reconnecting(v) => crate::desktop::stream::TerminalEvent::Reconnecting(Box::new(<crate::desktop::stream::Reconnecting as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::stream::terminal_event::Value::Exit(v) => crate::desktop::stream::TerminalEvent::Exit(Box::new(<crate::desktop::stream::Exit as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::stream::terminal_event::Value::Error(v) => crate::desktop::stream::TerminalEvent::Error(Box::new(<crate::desktop::Failure as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::stream::terminal_event::Value::Ended(v) => crate::desktop::stream::TerminalEvent::Ended(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::stream::terminal_event::Value::Active(v) => crate::desktop::stream::TerminalEvent::Active(Box::new(<crate::desktop::stream::Active as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::stream::terminal_event::Value::DisplayLost(v) => crate::desktop::stream::TerminalEvent::DisplayLost(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::stream::terminal_event::Value::NotHeld(v) => crate::desktop::stream::TerminalEvent::NotHeld(Box::new(<crate::desktop::stream::NotHeld as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::stream::terminal_event::Value::Size(v) => crate::desktop::stream::TerminalEvent::Size(Box::new(<crate::desktop::stream::Resize as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::stream::TerminalServer {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::TerminalServer;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::stream::TerminalServer { value: Some(match self { crate::desktop::stream::TerminalServer::Output(v) => crate::encoding::protobuf::generated::newport::desktop::stream::terminal_server::Value::Output(Box::new(v.as_ref().to_proto()?)), crate::desktop::stream::TerminalServer::Taken(v) => crate::encoding::protobuf::generated::newport::desktop::stream::terminal_server::Value::Taken(Box::new(v.as_ref().to_proto()?)), crate::desktop::stream::TerminalServer::Restore(v) => crate::encoding::protobuf::generated::newport::desktop::stream::terminal_server::Value::Restore(Box::new(v.as_ref().to_proto()?)), crate::desktop::stream::TerminalServer::Event(v) => crate::encoding::protobuf::generated::newport::desktop::stream::terminal_server::Value::Event(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::stream::terminal_server::Value::Output(v) => crate::desktop::stream::TerminalServer::Output(Box::new(<crate::desktop::stream::Output as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::stream::terminal_server::Value::Taken(v) => crate::desktop::stream::TerminalServer::Taken(Box::new(<crate::desktop::stream::Taken as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::stream::terminal_server::Value::Restore(v) => crate::desktop::stream::TerminalServer::Restore(Box::new(<crate::desktop::stream::Restore as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::stream::terminal_server::Value::Event(v) => crate::desktop::stream::TerminalServer::Event(Box::new(<crate::desktop::stream::TerminalEvent as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::stream::Operation {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::stream::Operation;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::stream::Operation { value: Some(match self { crate::desktop::stream::Operation::TerminalOpen(v) => crate::encoding::protobuf::generated::newport::desktop::stream::operation::Value::Terminal(Box::new(v.as_ref().to_proto()?)), crate::desktop::stream::Operation::CommandOpen(v) => crate::encoding::protobuf::generated::newport::desktop::stream::operation::Value::Command(Box::new(v.as_ref().to_proto()?)), crate::desktop::stream::Operation::RemoteAppsOpen(v) => crate::encoding::protobuf::generated::newport::desktop::stream::operation::Value::RemoteApps(Box::new(v.as_ref().to_proto()?)), crate::desktop::stream::Operation::LspOpen(v) => crate::encoding::protobuf::generated::newport::desktop::stream::operation::Value::Lsp(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::stream::operation::Value::Terminal(v) => crate::desktop::stream::Operation::TerminalOpen(Box::new(<crate::desktop::stream::TerminalOpenArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::stream::operation::Value::Command(v) => crate::desktop::stream::Operation::CommandOpen(Box::new(<crate::desktop::stream::CommandOpenArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::stream::operation::Value::RemoteApps(v) => crate::desktop::stream::Operation::RemoteAppsOpen(Box::new(<crate::desktop::stream::RemoteAppsOpenArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::stream::operation::Value::Lsp(v) => crate::desktop::stream::Operation::LspOpen(Box::new(<crate::desktop::stream::LspOpenArgs as Convert>::from_proto(*v)?)) })
    }
}
