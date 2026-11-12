use super::*;
impl Convert for crate::desktop::Operation {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::Operation;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::Operation { value: Some(match self { crate::desktop::Operation::Account(v) => crate::encoding::protobuf::generated::newport::desktop::operation::Value::Account(Box::new(v.as_ref().to_proto()?)), crate::desktop::Operation::Connection(v) => crate::encoding::protobuf::generated::newport::desktop::operation::Value::Connection(Box::new(v.as_ref().to_proto()?)), crate::desktop::Operation::Configuration(v) => crate::encoding::protobuf::generated::newport::desktop::operation::Value::Configuration(Box::new(v.as_ref().to_proto()?)), crate::desktop::Operation::Files(v) => crate::encoding::protobuf::generated::newport::desktop::operation::Value::Files(Box::new(v.as_ref().to_proto()?)), crate::desktop::Operation::Metrics(v) => crate::encoding::protobuf::generated::newport::desktop::operation::Value::Metrics(Box::new(v.as_ref().to_proto()?)), crate::desktop::Operation::Services(v) => crate::encoding::protobuf::generated::newport::desktop::operation::Value::Services(Box::new(v.as_ref().to_proto()?)), crate::desktop::Operation::Containers(v) => crate::encoding::protobuf::generated::newport::desktop::operation::Value::Containers(Box::new(v.as_ref().to_proto()?)), crate::desktop::Operation::Preferences(v) => crate::encoding::protobuf::generated::newport::desktop::operation::Value::Preferences(Box::new(v.as_ref().to_proto()?)), crate::desktop::Operation::Git(v) => crate::encoding::protobuf::generated::newport::desktop::operation::Value::Git(Box::new(v.as_ref().to_proto()?)), crate::desktop::Operation::Applications(v) => crate::encoding::protobuf::generated::newport::desktop::operation::Value::Applications(Box::new(v.as_ref().to_proto()?)), crate::desktop::Operation::Host(v) => crate::encoding::protobuf::generated::newport::desktop::operation::Value::Host(Box::new(v.as_ref().to_proto()?)), crate::desktop::Operation::Events(v) => crate::encoding::protobuf::generated::newport::desktop::operation::Value::Events(Box::new(v.as_ref().to_proto()?)), crate::desktop::Operation::Terminal(v) => crate::encoding::protobuf::generated::newport::desktop::operation::Value::Terminal(Box::new(v.as_ref().to_proto()?)), crate::desktop::Operation::Stream(v) => crate::encoding::protobuf::generated::newport::desktop::operation::Value::Stream(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::operation::Value::Account(v) => crate::desktop::Operation::Account(Box::new(<crate::desktop::account::Operation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::operation::Value::Connection(v) => crate::desktop::Operation::Connection(Box::new(<crate::desktop::connection::Operation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::operation::Value::Configuration(v) => crate::desktop::Operation::Configuration(Box::new(<crate::desktop::configuration::Operation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::operation::Value::Files(v) => crate::desktop::Operation::Files(Box::new(<crate::desktop::files::Operation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::operation::Value::Metrics(v) => crate::desktop::Operation::Metrics(Box::new(<crate::desktop::metrics::Operation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::operation::Value::Services(v) => crate::desktop::Operation::Services(Box::new(<crate::desktop::services::Operation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::operation::Value::Containers(v) => crate::desktop::Operation::Containers(Box::new(<crate::desktop::containers::Operation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::operation::Value::Preferences(v) => crate::desktop::Operation::Preferences(Box::new(<crate::desktop::preferences::Operation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::operation::Value::Git(v) => crate::desktop::Operation::Git(Box::new(<crate::desktop::git::Operation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::operation::Value::Applications(v) => crate::desktop::Operation::Applications(Box::new(<crate::desktop::applications::Operation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::operation::Value::Host(v) => crate::desktop::Operation::Host(Box::new(<crate::desktop::host::Operation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::operation::Value::Events(v) => crate::desktop::Operation::Events(Box::new(<crate::desktop::events::Operation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::operation::Value::Terminal(v) => crate::desktop::Operation::Terminal(Box::new(<crate::desktop::terminal::Operation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::operation::Value::Stream(v) => crate::desktop::Operation::Stream(Box::new(<crate::desktop::stream::Operation as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::Reply {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::Reply;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::Reply { value: Some(match self { crate::desktop::Reply::Account(v) => crate::encoding::protobuf::generated::newport::desktop::reply::Value::Account(Box::new(v.as_ref().to_proto()?)), crate::desktop::Reply::Connection(v) => crate::encoding::protobuf::generated::newport::desktop::reply::Value::Connection(Box::new(v.as_ref().to_proto()?)), crate::desktop::Reply::Configuration(v) => crate::encoding::protobuf::generated::newport::desktop::reply::Value::Configuration(Box::new(v.as_ref().to_proto()?)), crate::desktop::Reply::Files(v) => crate::encoding::protobuf::generated::newport::desktop::reply::Value::Files(Box::new(v.as_ref().to_proto()?)), crate::desktop::Reply::Metrics(v) => crate::encoding::protobuf::generated::newport::desktop::reply::Value::Metrics(Box::new(v.as_ref().to_proto()?)), crate::desktop::Reply::Services(v) => crate::encoding::protobuf::generated::newport::desktop::reply::Value::Services(Box::new(v.as_ref().to_proto()?)), crate::desktop::Reply::Containers(v) => crate::encoding::protobuf::generated::newport::desktop::reply::Value::Containers(Box::new(v.as_ref().to_proto()?)), crate::desktop::Reply::Preferences(v) => crate::encoding::protobuf::generated::newport::desktop::reply::Value::Preferences(Box::new(v.as_ref().to_proto()?)), crate::desktop::Reply::Git(v) => crate::encoding::protobuf::generated::newport::desktop::reply::Value::Git(Box::new(v.as_ref().to_proto()?)), crate::desktop::Reply::Applications(v) => crate::encoding::protobuf::generated::newport::desktop::reply::Value::Applications(Box::new(v.as_ref().to_proto()?)), crate::desktop::Reply::Host(v) => crate::encoding::protobuf::generated::newport::desktop::reply::Value::Host(Box::new(v.as_ref().to_proto()?)), crate::desktop::Reply::Events(v) => crate::encoding::protobuf::generated::newport::desktop::reply::Value::Events(Box::new(v.as_ref().to_proto()?)), crate::desktop::Reply::Terminal(v) => crate::encoding::protobuf::generated::newport::desktop::reply::Value::Terminal(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? {
            crate::encoding::protobuf::generated::newport::desktop::reply::Value::Account(v) => {
                crate::desktop::Reply::Account(Box::new(
                    <crate::desktop::account::Reply as Convert>::from_proto(*v)?,
                ))
            }
            crate::encoding::protobuf::generated::newport::desktop::reply::Value::Connection(v) => {
                crate::desktop::Reply::Connection(Box::new(
                    <crate::desktop::connection::Reply as Convert>::from_proto(*v)?,
                ))
            }
            crate::encoding::protobuf::generated::newport::desktop::reply::Value::Configuration(
                v,
            ) => crate::desktop::Reply::Configuration(Box::new(
                <crate::desktop::configuration::Reply as Convert>::from_proto(*v)?,
            )),
            crate::encoding::protobuf::generated::newport::desktop::reply::Value::Files(v) => {
                crate::desktop::Reply::Files(Box::new(
                    <crate::desktop::files::Reply as Convert>::from_proto(*v)?,
                ))
            }
            crate::encoding::protobuf::generated::newport::desktop::reply::Value::Metrics(v) => {
                crate::desktop::Reply::Metrics(Box::new(
                    <crate::desktop::metrics::Reply as Convert>::from_proto(*v)?,
                ))
            }
            crate::encoding::protobuf::generated::newport::desktop::reply::Value::Services(v) => {
                crate::desktop::Reply::Services(Box::new(
                    <crate::desktop::services::Reply as Convert>::from_proto(*v)?,
                ))
            }
            crate::encoding::protobuf::generated::newport::desktop::reply::Value::Containers(v) => {
                crate::desktop::Reply::Containers(Box::new(
                    <crate::desktop::containers::Reply as Convert>::from_proto(*v)?,
                ))
            }
            crate::encoding::protobuf::generated::newport::desktop::reply::Value::Preferences(
                v,
            ) => crate::desktop::Reply::Preferences(Box::new(
                <crate::desktop::preferences::Reply as Convert>::from_proto(*v)?,
            )),
            crate::encoding::protobuf::generated::newport::desktop::reply::Value::Git(v) => {
                crate::desktop::Reply::Git(Box::new(
                    <crate::desktop::git::Reply as Convert>::from_proto(*v)?,
                ))
            }
            crate::encoding::protobuf::generated::newport::desktop::reply::Value::Applications(
                v,
            ) => crate::desktop::Reply::Applications(Box::new(
                <crate::desktop::applications::Reply as Convert>::from_proto(*v)?,
            )),
            crate::encoding::protobuf::generated::newport::desktop::reply::Value::Host(v) => {
                crate::desktop::Reply::Host(Box::new(
                    <crate::desktop::host::Reply as Convert>::from_proto(*v)?,
                ))
            }
            crate::encoding::protobuf::generated::newport::desktop::reply::Value::Events(v) => {
                crate::desktop::Reply::Events(Box::new(
                    <crate::desktop::events::Reply as Convert>::from_proto(*v)?,
                ))
            }
            crate::encoding::protobuf::generated::newport::desktop::reply::Value::Terminal(v) => {
                crate::desktop::Reply::Terminal(Box::new(
                    <crate::desktop::terminal::Reply as Convert>::from_proto(*v)?,
                ))
            }
        })
    }
}

impl Convert for crate::desktop::Request {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::Request;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::Request { id: Some(self.id.get()), operation: Some(match &self.operation { crate::desktop::Operation::Account(v) => crate::encoding::protobuf::generated::newport::desktop::request::Operation::Account(Box::new(v.as_ref().to_proto()?)), crate::desktop::Operation::Connection(v) => crate::encoding::protobuf::generated::newport::desktop::request::Operation::Connection(Box::new(v.as_ref().to_proto()?)), crate::desktop::Operation::Configuration(v) => crate::encoding::protobuf::generated::newport::desktop::request::Operation::Configuration(Box::new(v.as_ref().to_proto()?)), crate::desktop::Operation::Files(v) => crate::encoding::protobuf::generated::newport::desktop::request::Operation::Files(Box::new(v.as_ref().to_proto()?)), crate::desktop::Operation::Metrics(v) => crate::encoding::protobuf::generated::newport::desktop::request::Operation::Metrics(Box::new(v.as_ref().to_proto()?)), crate::desktop::Operation::Services(v) => crate::encoding::protobuf::generated::newport::desktop::request::Operation::Services(Box::new(v.as_ref().to_proto()?)), crate::desktop::Operation::Containers(v) => crate::encoding::protobuf::generated::newport::desktop::request::Operation::Containers(Box::new(v.as_ref().to_proto()?)), crate::desktop::Operation::Preferences(v) => crate::encoding::protobuf::generated::newport::desktop::request::Operation::Preferences(Box::new(v.as_ref().to_proto()?)), crate::desktop::Operation::Git(v) => crate::encoding::protobuf::generated::newport::desktop::request::Operation::Git(Box::new(v.as_ref().to_proto()?)), crate::desktop::Operation::Applications(v) => crate::encoding::protobuf::generated::newport::desktop::request::Operation::Applications(Box::new(v.as_ref().to_proto()?)), crate::desktop::Operation::Host(v) => crate::encoding::protobuf::generated::newport::desktop::request::Operation::Host(Box::new(v.as_ref().to_proto()?)), crate::desktop::Operation::Events(v) => crate::encoding::protobuf::generated::newport::desktop::request::Operation::Events(Box::new(v.as_ref().to_proto()?)), crate::desktop::Operation::Terminal(v) => crate::encoding::protobuf::generated::newport::desktop::request::Operation::Terminal(Box::new(v.as_ref().to_proto()?)), crate::desktop::Operation::Stream(v) => crate::encoding::protobuf::generated::newport::desktop::request::Operation::Stream(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self { id: crate::desktop::RequestId::new(required(value.id)?)?, operation: match required(value.operation)? { crate::encoding::protobuf::generated::newport::desktop::request::Operation::Account(v) => crate::desktop::Operation::Account(Box::new(<crate::desktop::account::Operation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::request::Operation::Connection(v) => crate::desktop::Operation::Connection(Box::new(<crate::desktop::connection::Operation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::request::Operation::Configuration(v) => crate::desktop::Operation::Configuration(Box::new(<crate::desktop::configuration::Operation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::request::Operation::Files(v) => crate::desktop::Operation::Files(Box::new(<crate::desktop::files::Operation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::request::Operation::Metrics(v) => crate::desktop::Operation::Metrics(Box::new(<crate::desktop::metrics::Operation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::request::Operation::Services(v) => crate::desktop::Operation::Services(Box::new(<crate::desktop::services::Operation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::request::Operation::Containers(v) => crate::desktop::Operation::Containers(Box::new(<crate::desktop::containers::Operation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::request::Operation::Preferences(v) => crate::desktop::Operation::Preferences(Box::new(<crate::desktop::preferences::Operation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::request::Operation::Git(v) => crate::desktop::Operation::Git(Box::new(<crate::desktop::git::Operation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::request::Operation::Applications(v) => crate::desktop::Operation::Applications(Box::new(<crate::desktop::applications::Operation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::request::Operation::Host(v) => crate::desktop::Operation::Host(Box::new(<crate::desktop::host::Operation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::request::Operation::Events(v) => crate::desktop::Operation::Events(Box::new(<crate::desktop::events::Operation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::request::Operation::Terminal(v) => crate::desktop::Operation::Terminal(Box::new(<crate::desktop::terminal::Operation as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::request::Operation::Stream(v) => crate::desktop::Operation::Stream(Box::new(<crate::desktop::stream::Operation as Convert>::from_proto(*v)?)) } })
    }
}

impl Convert for crate::desktop::Outcome {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::Outcome;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::Outcome { value: Some(match self { crate::desktop::Outcome::Completed(v) => crate::encoding::protobuf::generated::newport::desktop::outcome::Value::Completed(Box::new(v.as_ref().to_proto()?)), crate::desktop::Outcome::Failed(v) => crate::encoding::protobuf::generated::newport::desktop::outcome::Value::Failed(Box::new(v.as_ref().to_proto()?)), crate::desktop::Outcome::Opened(v) => crate::encoding::protobuf::generated::newport::desktop::outcome::Value::Opened(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? {
            crate::encoding::protobuf::generated::newport::desktop::outcome::Value::Completed(
                v,
            ) => crate::desktop::Outcome::Completed(Box::new(
                <crate::desktop::Reply as Convert>::from_proto(*v)?,
            )),
            crate::encoding::protobuf::generated::newport::desktop::outcome::Value::Failed(v) => {
                crate::desktop::Outcome::Failed(Box::new(
                    <crate::desktop::Failure as Convert>::from_proto(*v)?,
                ))
            }
            crate::encoding::protobuf::generated::newport::desktop::outcome::Value::Opened(v) => {
                crate::desktop::Outcome::Opened(Box::new(
                    <crate::desktop::stream::Opened as Convert>::from_proto(*v)?,
                ))
            }
        })
    }
}

impl Convert for crate::desktop::Response {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::Response;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::Response { id: Some(self.id.get()), outcome: Some(match &self.outcome { crate::desktop::Outcome::Completed(v) => crate::encoding::protobuf::generated::newport::desktop::response::Outcome::Completed(Box::new(v.as_ref().to_proto()?)), crate::desktop::Outcome::Failed(v) => crate::encoding::protobuf::generated::newport::desktop::response::Outcome::Failed(Box::new(v.as_ref().to_proto()?)), crate::desktop::Outcome::Opened(v) => crate::encoding::protobuf::generated::newport::desktop::response::Outcome::Opened(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self { id: crate::desktop::RequestId::new(required(value.id)?)?, outcome: match required(value.outcome)? { crate::encoding::protobuf::generated::newport::desktop::response::Outcome::Completed(v) => crate::desktop::Outcome::Completed(Box::new(<crate::desktop::Reply as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::response::Outcome::Failed(v) => crate::desktop::Outcome::Failed(Box::new(<crate::desktop::Failure as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::response::Outcome::Opened(v) => crate::desktop::Outcome::Opened(Box::new(<crate::desktop::stream::Opened as Convert>::from_proto(*v)?)) } })
    }
}

impl Convert for crate::desktop::ClientMessage {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::ClientMessage;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::ClientMessage { value: Some(match self { crate::desktop::ClientMessage::Hello(v) => crate::encoding::protobuf::generated::newport::desktop::client_message::Value::Hello(Box::new(v.as_ref().to_proto()?)), crate::desktop::ClientMessage::Request(v) => crate::encoding::protobuf::generated::newport::desktop::client_message::Value::Request(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::client_message::Value::Hello(v) => crate::desktop::ClientMessage::Hello(Box::new(<crate::desktop::Hello as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::client_message::Value::Request(v) => crate::desktop::ClientMessage::Request(Box::new(<crate::desktop::Request as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::ServerMessage {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::ServerMessage;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::ServerMessage { value: Some(match self { crate::desktop::ServerMessage::Ready(v) => crate::encoding::protobuf::generated::newport::desktop::server_message::Value::Ready(Box::new(v.as_ref().to_proto()?)), crate::desktop::ServerMessage::Refused(v) => crate::encoding::protobuf::generated::newport::desktop::server_message::Value::Refused(Box::new(v.as_ref().to_proto()?)), crate::desktop::ServerMessage::Progress(v) => crate::encoding::protobuf::generated::newport::desktop::server_message::Value::Progress(Box::new(v.as_ref().to_proto()?)), crate::desktop::ServerMessage::Response(v) => crate::encoding::protobuf::generated::newport::desktop::server_message::Value::Response(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::server_message::Value::Ready(v) => crate::desktop::ServerMessage::Ready(Box::new(<crate::desktop::Ready as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::server_message::Value::Refused(v) => crate::desktop::ServerMessage::Refused(Box::new(<crate::common::Error as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::server_message::Value::Progress(v) => crate::desktop::ServerMessage::Progress(Box::new(<crate::desktop::Progress as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::server_message::Value::Response(v) => crate::desktop::ServerMessage::Response(Box::new(<crate::desktop::Response as Convert>::from_proto(*v)?)) })
    }
}
